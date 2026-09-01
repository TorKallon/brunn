"""Source-level and opt-in live HTTP checks for native asset routes.

The live class is intentionally disabled unless BRUNN_STATE_BINARY_LIVE=1. It
mutates the configured account with uniquely named fixture records and should
be pointed at a disposable evaluation user.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping


REPO_ROOT = Path(__file__).resolve().parents[1]
EVAL_ROOT = REPO_ROOT / "eval"
if str(EVAL_ROOT) not in sys.path:
    sys.path.insert(0, str(EVAL_ROOT))

from binary_asset_fixture import make_png, sha256_bytes  # noqa: E402


class AssetRouteSourceContractTest(unittest.TestCase):
    def test_api_wires_authenticated_workspace_binary_routes(self) -> None:
        api = (REPO_ROOT / "apps/api/src/api.rs").read_text(encoding="utf-8")
        for route, handler in (
            ('"/workspace/binaries"', "get(simple_core::list_binaries)"),
            (
                '"/workspace/binaries/{entry_ref}"',
                "get(simple_core::binary_metadata)",
            ),
            ('"/workspace/binaries"', "post(simple_core::upload_binary)"),
            (
                '"/workspace/binaries/content"',
                "put(simple_core::upload_binary_stream)",
            ),
            (
                '"/workspace/binaries/{entry_ref}/content"',
                "get(simple_core::fetch_binary)",
            ),
        ):
            with self.subTest(route=route, handler=handler):
                self.assertIn(route, api)
                self.assertIn(handler, api)
        transfers = api[
            api.index("let workspace_transfers = Router::new()") :
            api.index("let account_transfers = Router::new()")
        ]
        self.assertIn("DefaultBodyLimit::disable()", transfers)
        protected = api[
            api.index("let protected = ordinary") :
            api.index("let web_auth_routes = Router::new()")
        ]
        self.assertIn(".merge(transfers)", protected)
        self.assertIn("auth::middleware", protected)

    def test_binary_handlers_enforce_capabilities_and_exact_integrity(self) -> None:
        service = (REPO_ROOT / "apps/api/src/simple_core.rs").read_text(
            encoding="utf-8"
        )
        for function, capability in (
            ("list_binaries", "Read"),
            ("binary_metadata", "Read"),
            ("fetch_binary", "Read"),
            ("upload_binary", "Stage"),
            ("upload_binary_stream", "Stage"),
        ):
            start = service.index(f"pub async fn {function}")
            end = service.find("\npub async fn ", start + 1)
            body = service[start : None if end < 0 else end]
            with self.subTest(function=function):
                self.assertIn(f"auth.require(Capability::{capability})?", body)
        download = service[
            service.index("pub async fn fetch_binary") :
            service.index("pub async fn binary_metadata")
        ]
        self.assertIn("get_stream_version(key, entry.object_version_id.as_deref())", download)
        self.assertIn("content_size_mismatch", download)
        self.assertIn("x-brunn-state-sha256", service)
        self.assertIn("x-brunn-state-integrity", download)
        self.assertIn('HeaderValue::from_static("private, no-store")', download)
        self.assertIn('HeaderValue::from_static("nosniff")', download)

    def test_generated_description_contract_is_explicitly_non_authoritative(self) -> None:
        descriptor = (REPO_ROOT / "apps/api/src/asset_description.rs").read_text(
            encoding="utf-8"
        )
        workspace = (REPO_ROOT / "apps/api/src/simple_core.rs").read_text(
            encoding="utf-8"
        )
        self.assertIn("Generated, non-authoritative description", descriptor)
        self.assertIn("against the exact binary bytes", descriptor)
        self.assertIn('format!(".brunn/binaries/{}.md"', workspace)
        self.assertIn("The immutable binary bytes are authoritative", workspace)
        self.assertIn('"kind": "binary_description"', workspace)
        self.assertIn("should_queue_description", workspace)
        self.assertIn("VALUES ($1,'describe_binary',$2)", workspace)

    def test_binary_commit_uses_stable_paths_versions_and_exact_objects(self) -> None:
        writer = (REPO_ROOT / "apps/api/src/simple_core.rs").read_text(
            encoding="utf-8"
        )
        migration = (
            REPO_ROOT
            / "apps/api/migrations/0051_simple_workspace_core.sql"
        ).read_text(encoding="utf-8")
        commit = writer[writer.index("async fn commit_binary_with_companion") :]
        self.assertIn("object_version_id.ok_or_else", commit)
        self.assertIn("portable_path_key(path)", commit)
        self.assertIn("require_local_publish_lock", commit)
        self.assertIn("FOR UPDATE OF entry", commit)
        self.assertIn("entry_version_conflict", commit)
        self.assertIn('row.get::<i64, _>("current_version") + 1', commit)
        self.assertIn("INSERT INTO brunn.entry_versions", commit)
        self.assertIn("INSERT INTO brunn.workspace_changes", commit)
        self.assertIn("tx.commit().await?", commit)
        self.assertIn("CREATE UNIQUE INDEX entries_user_path_unique_idx", migration)
        self.assertIn("UNIQUE (user_id, entry_id, version)", migration)
        self.assertIn("object_version_id IS NOT NULL", migration)

    def test_streaming_upload_is_hash_bound_bounded_and_cleaned_up(self) -> None:
        upload = (REPO_ROOT / "apps/api/src/simple_core.rs").read_text(
            encoding="utf-8"
        )
        stream = upload[
            upload.index("pub async fn upload_binary_stream") :
            upload.index("pub async fn fetch_binary")
        ]
        self.assertIn("validate_sha256(&query.expected_content_hash)?", stream)
        self.assertIn("MAX_STREAMED_BINARY_BYTES", stream)
        self.assertIn("binary upload size overflow", stream)
        self.assertIn("workspace binary uploads are limited to 4 GiB", stream)
        self.assertIn("OpenOptions::new()", stream)
        self.assertIn(".create_new(true)", stream)
        self.assertIn("put_user_file_blob", stream)
        self.assertIn("tokio::fs::remove_file(&temporary_path)", stream)
        self.assertIn("content_hash_mismatch", stream)
        self.assertIn("commit_binary_with_companion", stream)

    def test_stage_reclamation_discovers_references_and_locks_storage_first(self) -> None:
        migration = (
            REPO_ROOT
            / "apps/api/migrations/0039_safe_stage_reclamation.sql"
        ).read_text(encoding="utf-8")
        self.assertIn("foreign_key_reference_exists", migration)
        self.assertIn("FROM pg_constraint", migration)
        self.assertIn("constraint_row.confrelid=p_parent", migration)
        storage_lock = migration.index(
            "hashtextextended('storage:' || target_stage.user_id::text,0)"
        )
        stage_row_lock = migration.index("WHERE stage.id=p_stage_id\n  FOR UPDATE")
        self.assertLess(storage_lock, stage_row_lock)
        self.assertIn("CONTINUE candidate_loop", migration)

    def test_openapi_advertises_native_asset_and_portability_routes(self) -> None:
        service = (REPO_ROOT / "apps/api/src/service.rs").read_text(encoding="utf-8")
        for operation in (
            "listAssets",
            "downloadAssetVersion",
            "createAssetUpload",
            "putAssetUploadPart",
            "completeAssetUpload",
            "getVaultManifest",
            "downloadVaultAssetVersion",
        ):
            with self.subTest(operation=operation):
                self.assertIn(f'"operationId": "{operation}"', service)


@dataclass(frozen=True)
class HttpResponse:
    status: int
    headers: Mapping[str, str]
    body: bytes

    def json(self) -> Any:
        return json.loads(self.body.decode("utf-8"))


class LiveClient:
    def __init__(self, base_url: str, token: str, *, timeout: float = 30.0) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout = timeout

    def request(
        self,
        method: str,
        path: str,
        *,
        body: bytes | None = None,
        json_body: Any | None = None,
        headers: Mapping[str, str] | None = None,
        expected: int | Iterable[int] = 200,
    ) -> HttpResponse:
        if body is not None and json_body is not None:
            raise ValueError("body and json_body are mutually exclusive")
        request_headers = {
            "Accept": "application/json",
            "Authorization": f"Bearer {self.token}",
            "User-Agent": "brunn-state-binary-live-conformance/1",
        }
        if headers:
            request_headers.update(headers)
        payload = body
        if json_body is not None:
            payload = json.dumps(
                json_body,
                separators=(",", ":"),
                ensure_ascii=True,
            ).encode("utf-8")
            request_headers["Content-Type"] = "application/json"
        request = urllib.request.Request(
            self.base_url + path,
            data=payload,
            headers=request_headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                result = HttpResponse(
                    response.status,
                    {key.casefold(): value for key, value in response.headers.items()},
                    response.read(),
                )
        except urllib.error.HTTPError as error:
            try:
                result = HttpResponse(
                    error.code,
                    {
                        key.casefold(): value
                        for key, value in error.headers.items()
                    },
                    error.read(),
                )
            finally:
                error.close()
        expected_statuses = {expected} if isinstance(expected, int) else set(expected)
        if result.status not in expected_statuses:
            safe_body = result.body.decode("utf-8", errors="replace")
            safe_body = safe_body.replace(self.token, "<redacted>")[:4_000]
            raise AssertionError(
                f"{method} {path} returned {result.status}, expected "
                f"{sorted(expected_statuses)}: {safe_body}"
            )
        return result


def envelope_data(response: HttpResponse) -> tuple[dict[str, Any], dict[str, Any]]:
    envelope = response.json()
    if not isinstance(envelope, dict):
        raise AssertionError("API envelope is not an object")
    data = envelope.get("data")
    if not isinstance(data, dict):
        raise AssertionError(f"API envelope lacks object data: {envelope}")
    return envelope, data


def encode_multipart(
    fields: Mapping[str, str],
    files: Iterable[tuple[str, str, str, bytes]],
) -> tuple[str, bytes]:
    boundary = "----brunn-state-binary-conformance-" + uuid.uuid4().hex
    chunks: list[bytes] = []

    def append(text: str) -> None:
        chunks.append(text.encode("utf-8"))

    for name, value in fields.items():
        append(f"--{boundary}\r\n")
        append(f'Content-Disposition: form-data; name="{name}"\r\n\r\n')
        append(value)
        append("\r\n")
    for field_name, filename, media_type, content in files:
        safe = filename.replace('"', "_").replace("\r", "_").replace("\n", "_")
        append(f"--{boundary}\r\n")
        append(
            f'Content-Disposition: form-data; name="{field_name}"; '
            f'filename="{safe}"\r\n'
        )
        append(f"Content-Type: {media_type}\r\n\r\n")
        chunks.append(content)
        append("\r\n")
    append(f"--{boundary}--\r\n")
    return f"multipart/form-data; boundary={boundary}", b"".join(chunks)


@unittest.skipUnless(
    os.environ.get("BRUNN_STATE_BINARY_LIVE") == "1",
    "set BRUNN_STATE_BINARY_LIVE=1 with disposable RW/RO credentials",
)
class LiveBinaryAssetRouteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        required = (
            "BRUNN_STATE_BINARY_LIVE_RW_TOKEN",
            "BRUNN_STATE_BINARY_LIVE_RO_TOKEN",
        )
        missing = [name for name in required if not os.environ.get(name)]
        if missing:
            raise unittest.SkipTest(
                "live binary route test is missing " + ", ".join(missing)
            )
        cls.base_url = os.environ.get(
            "BRUNN_STATE_BINARY_LIVE_BASE_URL",
            "http://localhost:18110",
        )
        cls.scope = os.environ.get(
            "BRUNN_STATE_BINARY_LIVE_SCOPE",
            "scope:root",
        )
        cls.describe = os.environ.get(
            "BRUNN_STATE_BINARY_LIVE_DESCRIPTIONS"
        ) == "1"
        cls.rw = LiveClient(
            cls.base_url,
            os.environ["BRUNN_STATE_BINARY_LIVE_RW_TOKEN"],
        )
        cls.ro = LiveClient(
            cls.base_url,
            os.environ["BRUNN_STATE_BINARY_LIVE_RO_TOKEN"],
        )

    def _stage(
        self,
        *,
        stable_import_id: str,
        files: list[tuple[str, str, str, bytes]],
        describe: bool,
    ) -> dict[str, Any]:
        media_type, body = encode_multipart(
            {
                "scope": self.scope,
                "stable_import_id": stable_import_id,
                "describe_binaries": "true" if describe else "false",
            },
            files,
        )
        _, data = envelope_data(
            self.rw.request(
                "POST",
                "/v1/memory/stage",
                body=body,
                headers={"Content-Type": media_type},
            )
        )
        if data.get("status") != "inspecting":
            return data
        stage_ref = str(data["stage_ref"])
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            status = self.rw.request(
                "GET",
                "/v1/stages/" + urllib.parse.quote(stage_ref, safe=":"),
            ).json()
            if status.get("status") == "ready":
                return status
            if status.get("status") == "failed":
                raise AssertionError(f"asset description stage failed: {status}")
            time.sleep(0.5)
        raise AssertionError("asset description stage did not become ready in 120 seconds")

    def _promote(
        self,
        *,
        stage: Mapping[str, Any],
        idempotency_key: str,
    ) -> tuple[str, dict[str, Any]]:
        entries = stage.get("entries")
        if not isinstance(entries, list) or not entries:
            raise AssertionError(f"stage has no entries: {stage}")
        selected = [
            item["path"]
            for item in entries
            if isinstance(item, dict) and isinstance(item.get("path"), str)
        ]
        envelope, data = envelope_data(
            self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body={
                    "intent": "promote binary conformance fixture",
                    "scope": self.scope,
                    "root_refs": [],
                    "source_refs": [],
                    "items": [
                        {
                            "action": "create",
                            "kind": "import_receipt",
                            "ref": None,
                            "payload": {
                                "stage_ref": stage["stage_ref"],
                                "selected_entries": selected,
                            },
                        }
                    ],
                    "idempotency_key": idempotency_key,
                },
            )
        )
        revision = data.get("corpus_revision") or envelope.get("corpus_revision")
        if not isinstance(revision, str):
            raise AssertionError(f"promotion lacks corpus revision: {data}")
        return revision, data

    def _open_read_only(self, revision: str, marker: str) -> str:
        envelope, _ = envelope_data(
            self.ro.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Inspect binary conformance fixture {marker}",
                    "hints": {
                        "authorization_scope": self.scope,
                        "root_refs": [],
                        "open_object_refs": [],
                    },
                    "as_of": revision,
                    "mode": "continuation",
                    "token_budget": 4_000,
                },
            )
        )
        session_id = envelope.get("session_id")
        if not isinstance(session_id, str):
            raise AssertionError(f"memory.open lacks session_id: {envelope}")
        return session_id

    def _list_assets(self, session_id: str) -> list[dict[str, Any]]:
        query = urllib.parse.urlencode({"session_id": session_id, "limit": 500})
        body = self.ro.request("GET", f"/v1/assets?{query}").json()
        items = body.get("items") if isinstance(body, dict) else None
        if not isinstance(items, list):
            raise AssertionError(f"asset list is malformed: {body}")
        return [item for item in items if isinstance(item, dict)]

    def _admin_sql(self, sql: str) -> str:
        container = os.environ.get(
            "BRUNN_STATE_BINARY_LIVE_DB_CONTAINER",
            "brunn-db-1",
        )
        database = os.environ.get(
            "BRUNN_STATE_BINARY_LIVE_DB_NAME",
            "brunn",
        )
        user = os.environ.get(
            "BRUNN_STATE_BINARY_LIVE_DB_USER",
            "admin",
        )
        result = subprocess.run(
            [
                "docker",
                "exec",
                container,
                "psql",
                "--username",
                user,
                "--dbname",
                database,
                "--no-psqlrc",
                "--tuples-only",
                "--no-align",
                "--set",
                "ON_ERROR_STOP=1",
                "--command",
                sql,
            ],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError(
                f"live binary administrative SQL failed: {result.stderr[:2_000]}"
            )
        return result.stdout.strip()

    def _expire_stage_and_wait(self, stage_ref: str) -> dict[str, Any]:
        stage_id = uuid.UUID(stage_ref.removeprefix("stage:"))
        updated = self._admin_sql(
            "UPDATE brunn.stages "
            "SET created_at=clock_timestamp()-interval '2 minutes',"
            "expires_at=clock_timestamp()-interval '1 minute' "
            f"WHERE id='{stage_id}' RETURNING id;"
        )
        self.assertIn(str(stage_id), updated)
        encoded_ref = urllib.parse.quote(stage_ref, safe=":")
        deadline = time.monotonic() + 75
        while time.monotonic() < deadline:
            status = self.rw.request(
                "GET",
                f"/v1/stages/{encoded_ref}",
            ).json()
            if status.get("status") == "expired":
                return status
            time.sleep(0.5)
        raise AssertionError("worker did not reclaim the expired stage in 75 seconds")

    def test_binary_lifecycle_read_only_download_range_and_version(self) -> None:
        marker = uuid.uuid4().hex
        stable_import_id = f"binary-live:{marker}"
        first_bytes = make_png(
            41,
            29,
            palette=((240, 240, 240), (35, 88, 120), (198, 101, 63)),
        )
        path = f"tests/binary-conformance/{marker}/payload.png"
        copy_path = f"tests/binary-conformance/{marker}/copy.png"
        first_stage = self._stage(
            stable_import_id=stable_import_id,
            files=[
                ("file", path, "image/png", first_bytes),
                ("file", copy_path, "image/png", first_bytes),
            ],
            describe=self.describe,
        )
        first_entries = [
            item
            for item in first_stage["entries"]
            if item.get("path") in {path, copy_path}
        ]
        self.assertEqual(len(first_entries), 2)
        self.assertNotEqual(
            first_entries[0]["asset_ref"],
            first_entries[1]["asset_ref"],
        )
        self.assertEqual(first_entries[0]["hash"], first_entries[1]["hash"])
        first_ref = next(
            item["asset_ref"] for item in first_entries if item["path"] == path
        )

        revision_1, _ = self._promote(
            stage=first_stage,
            idempotency_key=f"binary-live-promote:{marker}:r1",
        )
        session_1 = self._open_read_only(revision_1, marker)
        asset_1 = next(
            item for item in self._list_assets(session_1) if item.get("path") == path
        )
        self.assertEqual(asset_1["asset_ref"], first_ref)
        self.assertEqual(asset_1["content_hash"], f"sha256:{sha256_bytes(first_bytes)}")
        if self.describe:
            self.assertIs(asset_1["description_available"], True)

        asset_ref_path = urllib.parse.quote(asset_1["asset_ref"], safe=":")
        session_query = urllib.parse.urlencode({"session_id": session_1})
        metadata = self.ro.request(
            "GET",
            f"/v1/assets/{asset_ref_path}?{session_query}",
        ).json()
        self.assertEqual(metadata["content_hash"], asset_1["content_hash"])
        self.assertEqual(metadata["size_bytes"], len(first_bytes))

        content_path = (
            f"/v1/assets/{asset_ref_path}/versions/{asset_1['version']}/content?"
            f"{session_query}"
        )
        full = self.ro.request(
            "GET",
            content_path,
            headers={"Accept": "application/octet-stream"},
        )
        self.assertEqual(full.body, first_bytes)
        self.assertEqual(
            full.headers.get("x-brunn-state-sha256"),
            sha256_bytes(first_bytes),
        )
        self.assertEqual(full.headers.get("accept-ranges"), "bytes")
        self.assertEqual(
            full.headers.get("x-brunn-state-integrity"),
            "expected-sha256-verified-on-complete",
        )
        self.assertIn(
            "no-store",
            full.headers.get("cache-control", "").casefold(),
        )
        self.assertEqual(full.headers.get("x-content-type-options"), "nosniff")

        partial = self.ro.request(
            "GET",
            content_path,
            headers={
                "Accept": "application/octet-stream",
                "Range": "bytes=7-19",
            },
            expected=206,
        )
        self.assertEqual(partial.body, first_bytes[7:20])
        self.assertEqual(
            partial.headers.get("content-range"),
            f"bytes 7-19/{len(first_bytes)}",
        )
        self.assertEqual(
            partial.headers.get("x-brunn-state-sha256"),
            sha256_bytes(first_bytes),
        )
        self.assertEqual(
            partial.headers.get("x-brunn-state-integrity"),
            "range-from-verified-object-identity",
        )
        self.ro.request(
            "GET",
            content_path,
            headers={"Range": f"bytes={len(first_bytes)}-"},
            expected=416,
        )

        denied_type, denied_body = encode_multipart(
            {
                "scope": self.scope,
                "stable_import_id": f"binary-live-ro-denied:{marker}",
            },
            [("file", "denied.bin", "application/octet-stream", b"denied")],
        )
        self.ro.request(
            "POST",
            "/v1/memory/stage",
            body=denied_body,
            headers={"Content-Type": denied_type},
            expected=403,
        )

        second_bytes = first_bytes + b"\x00version-two"
        second_stage = self._stage(
            stable_import_id=stable_import_id,
            files=[("file", path, "image/png", second_bytes)],
            describe=False,
        )
        second_entry = next(
            item for item in second_stage["entries"] if item.get("path") == path
        )
        self.assertEqual(second_entry["asset_ref"], first_ref)
        self.assertEqual(second_entry["asset_version"], asset_1["version"] + 1)
        revision_2, _ = self._promote(
            stage=second_stage,
            idempotency_key=f"binary-live-promote:{marker}:r2",
        )
        session_2 = self._open_read_only(revision_2, marker)
        asset_2 = next(
            item for item in self._list_assets(session_2) if item.get("path") == path
        )
        self.assertEqual(asset_2["asset_ref"], first_ref)
        self.assertEqual(asset_2["version"], asset_1["version"] + 1)
        self.assertEqual(
            asset_2["content_hash"],
            f"sha256:{hashlib.sha256(second_bytes).hexdigest()}",
        )

    def test_multiple_import_batches_commit_as_one_atomic_generation(self) -> None:
        marker = uuid.uuid4().hex
        canonical_import_id = f"vault:binary-atomic:{marker}"
        generation_id = f"vault-generation:binary-atomic:{marker}"
        paths = [
            f"tests/binary-conformance/{marker}/batch-a.txt",
            f"tests/binary-conformance/{marker}/batch-b.txt",
        ]
        stages = [
            self._stage(
                stable_import_id=f"vault-batch:{marker}:{index}",
                files=[
                    (
                        "file",
                        path,
                        "text/plain",
                        f"atomic generation {marker} batch {index}\n".encode("utf-8"),
                    )
                ],
                describe=False,
            )
            for index, path in enumerate(paths)
        ]
        base_revisions = {stage.get("base_corpus_revision") for stage in stages}
        self.assertEqual(len(base_revisions), 1)
        base_revision = next(iter(base_revisions))
        self.assertIsInstance(base_revision, str)

        before_session = self._open_read_only(str(base_revision), marker + ":before")
        before_paths = {
            item.get("path") for item in self._list_assets(before_session)
        }
        self.assertTrue(before_paths.isdisjoint(paths))

        request_body = {
            "intent": "binary conformance atomic vault generation",
            "scope": self.scope,
            "root_refs": [],
            "source_refs": [],
            "base_corpus_revision": base_revision,
            "idempotency_key": f"binary-atomic-promote:{marker}",
            "items": [
                {
                    "action": "create",
                    "kind": "import_receipt",
                    "payload": {
                        "stage_ref": stage["stage_ref"],
                        "selected_entries": [path],
                        "stable_import_id": canonical_import_id,
                        "generation_id": generation_id,
                    },
                }
                for stage, path in zip(stages, paths, strict=True)
            ],
        }
        committed_envelope, committed = envelope_data(
            self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body=request_body,
            )
        )
        revision = committed.get("corpus_revision") or committed_envelope.get(
            "corpus_revision"
        )
        self.assertIsInstance(revision, str)
        self.assertNotEqual(revision, base_revision)
        self.assertEqual(len(committed.get("import_receipts", [])), 2)

        after_session = self._open_read_only(str(revision), marker + ":after")
        after_paths = {item.get("path") for item in self._list_assets(after_session)}
        self.assertTrue(set(paths).issubset(after_paths))

        replay_envelope, replay = envelope_data(
            self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body=request_body,
            )
        )
        self.assertEqual(
            replay.get("corpus_revision")
            or replay_envelope.get("corpus_revision"),
            revision,
        )
        self.assertEqual(
            replay.get("import_receipts"),
            committed.get("import_receipts"),
        )

    def test_expired_uncommitted_second_version_restores_committed_head(self) -> None:
        marker = uuid.uuid4().hex
        stable_import_id = f"binary-expiry-live:{marker}"
        path = f"tests/binary-conformance/{marker}/rollback.bin"
        first_bytes = f"committed:{marker}\n".encode("ascii")
        second_bytes = f"uncommitted:{marker}\n".encode("ascii")

        first_stage = self._stage(
            stable_import_id=stable_import_id,
            files=[
                (
                    "file",
                    path,
                    "application/octet-stream",
                    first_bytes,
                )
            ],
            describe=False,
        )
        first_entry = next(
            item for item in first_stage["entries"] if item.get("path") == path
        )
        revision, _ = self._promote(
            stage=first_stage,
            idempotency_key=f"binary-expiry-promote:{marker}",
        )

        second_stage = self._stage(
            stable_import_id=stable_import_id,
            files=[
                (
                    "file",
                    path,
                    "application/octet-stream",
                    second_bytes,
                )
            ],
            describe=False,
        )
        second_entry = next(
            item for item in second_stage["entries"] if item.get("path") == path
        )
        self.assertEqual(second_entry["asset_ref"], first_entry["asset_ref"])
        self.assertEqual(
            second_entry["asset_version"],
            first_entry["asset_version"] + 1,
        )

        expired = self._expire_stage_and_wait(str(second_stage["stage_ref"]))
        self.assertEqual(expired.get("status"), "expired")

        asset_id = uuid.UUID(str(first_entry["asset_ref"]).removeprefix("asset:"))
        database_state = self._admin_sql(
            "SELECT asset.current_version,"
            "(SELECT count(*) FROM brunn.asset_versions AS version "
            " WHERE version.user_id=asset.user_id "
            " AND version.asset_id=asset.id) "
            "FROM brunn.assets AS asset "
            f"WHERE asset.id='{asset_id}';"
        )
        self.assertEqual("1|1", database_state)

        session = self._open_read_only(revision, marker)
        asset = next(
            item for item in self._list_assets(session) if item.get("path") == path
        )
        self.assertEqual(asset["version"], first_entry["asset_version"])
        asset_ref = urllib.parse.quote(asset["asset_ref"], safe=":")
        query = urllib.parse.urlencode({"session_id": session})
        downloaded = self.ro.request(
            "GET",
            (
                f"/v1/assets/{asset_ref}/versions/{asset['version']}/content?"
                f"{query}"
            ),
            headers={"Accept": "application/octet-stream"},
        )
        self.assertEqual(first_bytes, downloaded.body)

    def test_resumable_upload_exceeds_inline_stage_limit_and_round_trips(self) -> None:
        marker = uuid.uuid4().hex
        path = f"tests/binary-conformance/{marker}/large-dataset.bin"
        stable_import_id = f"binary-large-live:{marker}"
        part_size = 8 * 1024 * 1024
        total_size = 72 * 1024 * 1024 + 137
        part_count = (total_size + part_size - 1) // part_size

        def part_bytes(part_number: int) -> bytes:
            offset = (part_number - 1) * part_size
            size = min(part_size, total_size - offset)
            seed = hashlib.sha256(
                f"brunn-state-large-part:{part_number}".encode("ascii")
            ).digest()
            return (seed * ((size + len(seed) - 1) // len(seed)))[:size]

        expected_hasher = hashlib.sha256()
        for number in range(1, part_count + 1):
            expected_hasher.update(part_bytes(number))
        expected_hash = expected_hasher.hexdigest()
        create_body = {
            "scope": self.scope,
            "path": path,
            "content_hash": f"sha256:{expected_hash}",
            "size_bytes": total_size,
            "media_type": "application/octet-stream",
            "stable_import_id": stable_import_id,
            "describe_binary": False,
        }
        self.ro.request(
            "POST",
            "/v1/asset-uploads",
            json_body=create_body,
            expected=403,
        )
        upload = self.rw.request(
            "POST",
            "/v1/asset-uploads",
            json_body=create_body,
        ).json()
        self.assertEqual(upload["part_size_bytes"], part_size)
        self.assertEqual(upload["part_count"], part_count)
        upload_ref = str(upload["upload_ref"])
        upload_path = urllib.parse.quote(upload_ref, safe=":")
        for number in range(1, part_count + 1):
            content = part_bytes(number)
            response = self.rw.request(
                "PUT",
                f"/v1/asset-uploads/{upload_path}/parts/{number}",
                body=content,
                headers={
                    "Content-Type": "application/octet-stream",
                    "x-brunn-state-part-sha256": hashlib.sha256(content).hexdigest(),
                },
            ).json()
            self.assertEqual(response["size_bytes"], len(content))
            self.assertIs(response["replayed"], False)
            if number == 1:
                replay = self.rw.request(
                    "PUT",
                    f"/v1/asset-uploads/{upload_path}/parts/{number}",
                    body=content,
                    headers={
                        "Content-Type": "application/octet-stream",
                        "x-brunn-state-part-sha256": hashlib.sha256(content).hexdigest(),
                    },
                ).json()
                self.assertIs(replay["replayed"], True)

        status = self.rw.request(
            "GET",
            f"/v1/asset-uploads/{upload_path}",
        ).json()
        self.assertEqual(len(status["uploaded_parts"]), part_count)
        self.ro.request(
            "GET",
            f"/v1/asset-uploads/{upload_path}",
            expected=403,
        )
        stage = self.rw.request(
            "POST",
            f"/v1/asset-uploads/{upload_path}/complete",
        ).json()
        self.assertEqual(stage["status"], "ready")
        entry = next(item for item in stage["entries"] if item.get("path") == path)
        self.assertEqual(entry["hash"], f"sha256:{expected_hash}")
        revision, _ = self._promote(
            stage=stage,
            idempotency_key=f"binary-large-live-promote:{marker}",
        )
        session = self._open_read_only(revision, marker)
        asset = next(
            item for item in self._list_assets(session) if item.get("path") == path
        )
        self.assertEqual(asset["size_bytes"], total_size)
        self.assertEqual(asset["content_hash"], f"sha256:{expected_hash}")
        asset_ref = urllib.parse.quote(asset["asset_ref"], safe=":")
        query = urllib.parse.urlencode({"session_id": session})
        downloaded = self.ro.request(
            "GET",
            (
                f"/v1/assets/{asset_ref}/versions/{asset['version']}/content?"
                f"{query}"
            ),
            headers={"Accept": "application/octet-stream"},
        )
        self.assertEqual(len(downloaded.body), total_size)
        self.assertEqual(hashlib.sha256(downloaded.body).hexdigest(), expected_hash)

    def test_account_export_includes_completed_upload_before_stage(self) -> None:
        marker = uuid.uuid4().hex
        stable_import_id = f"binary-export-crash:{marker}"
        path = f"tests/binary-conformance/{marker}/pre-stage-export.bin"
        content = f"completed-before-stage:{marker}\n".encode("ascii")
        content_hash = hashlib.sha256(content).hexdigest()
        trigger_function = "brunn.test_fail_pre_stage_export"
        trigger_name = "test_fail_pre_stage_export"
        self._admin_sql(
            f"""
            CREATE OR REPLACE FUNCTION {trigger_function}()
            RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
              IF NEW.stable_import_id = '{stable_import_id}' THEN
                RAISE EXCEPTION 'intentional pre-stage export test failure';
              END IF;
              RETURN NEW;
            END;
            $$;
            DROP TRIGGER IF EXISTS {trigger_name} ON brunn.stages;
            CREATE TRIGGER {trigger_name}
            BEFORE INSERT ON brunn.stages
            FOR EACH ROW EXECUTE FUNCTION {trigger_function}();
            """
        )
        upload_ref = ""
        try:
            upload = self.rw.request(
                "POST",
                "/v1/asset-uploads",
                json_body={
                    "scope": self.scope,
                    "path": path,
                    "content_hash": f"sha256:{content_hash}",
                    "size_bytes": len(content),
                    "media_type": "application/octet-stream",
                    "stable_import_id": stable_import_id,
                    "describe_binary": False,
                },
            ).json()
            upload_ref = str(upload["upload_ref"])
            encoded_upload = urllib.parse.quote(upload_ref, safe=":")
            self.rw.request(
                "PUT",
                f"/v1/asset-uploads/{encoded_upload}/parts/1",
                body=content,
                headers={
                    "Content-Type": "application/octet-stream",
                    "x-brunn-state-part-sha256": content_hash,
                },
            )
            self.rw.request(
                "POST",
                f"/v1/asset-uploads/{encoded_upload}/complete",
                expected=500,
            )
        finally:
            self._admin_sql(
                f"""
                DROP TRIGGER IF EXISTS {trigger_name} ON brunn.stages;
                DROP FUNCTION IF EXISTS {trigger_function}();
                """
            )

        upload_id = uuid.UUID(upload_ref.removeprefix("upload:"))
        upload_state = self._admin_sql(
            "SELECT status || '|' || canonical_object_key || '|' || "
            "canonical_object_version_id "
            "FROM brunn.asset_uploads "
            f"WHERE id='{upload_id}';"
        )
        self.assertTrue(upload_state.startswith("completed|"), upload_state)

        export = self.rw.request("POST", "/v1/account/exports").json()
        export_ref = str(export["id"])
        encoded_export = urllib.parse.quote(export_ref, safe=":")
        deadline = time.monotonic() + 120
        ready: dict[str, Any] | None = None
        while time.monotonic() < deadline:
            status = self.rw.request(
                "GET",
                f"/v1/account/exports/{encoded_export}",
            ).json()
            if status.get("status") == "ready":
                ready = status
                break
            if status.get("status") in {"failed", "canceled", "deleted", "expired"}:
                self.fail(f"account export did not become ready: {status}")
            time.sleep(0.5)
        self.assertIsNotNone(ready, "account export did not become ready in 120 seconds")

        archive = self.rw.request(
            "GET",
            f"/v1/account/exports/{encoded_export}/content",
            headers={"Accept": "application/gzip"},
        ).body
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as exported:
            manifest_member = exported.extractfile(
                "brunn-export/manifest.json"
            )
            self.assertIsNotNone(manifest_member)
            manifest = json.load(manifest_member)
            matched = [
                item
                for item in manifest["objects"]
                if {
                    (reference.get("kind"), reference.get("ref"))
                    for reference in item.get("references", [])
                }
                >= {("completed_upload", str(upload_id))}
            ]
            self.assertEqual(len(matched), 1, matched)
            exported_bytes = exported.extractfile(
                "brunn-export/" + matched[0]["path"]
            )
            self.assertIsNotNone(exported_bytes)
            self.assertEqual(exported_bytes.read(), content)
            self.assertEqual(
                matched[0]["content_hash"],
                f"sha256:{content_hash}",
            )

        self.rw.request(
            "DELETE",
            f"/v1/account/exports/{encoded_export}",
        )


if __name__ == "__main__":
    unittest.main()

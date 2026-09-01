#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any


class ContractFailure(RuntimeError):
    pass


def load_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.split("=", 1)
        values[name] = value
    return values


class Client:
    def __init__(self, base_url: str, token: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        content_type: str = "application/json",
        expected: int = 200,
        json_response: bool = True,
    ) -> tuple[dict[str, Any], bytes]:
        payload = None
        if body is not None:
            payload = (
                json.dumps(body).encode("utf-8")
                if content_type == "application/json"
                else bytes(body)
            )
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=payload,
            method=method,
            headers={
                "Accept": "application/json",
                "Authorization": f"Bearer {self.token}",
                **({"Content-Type": content_type} if payload is not None else {}),
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        if status != expected:
            parsed = parse_json(raw)
            raise ContractFailure(
                f"{method} {path} returned HTTP {status}, expected {expected}: "
                f"{json.dumps(parsed, sort_keys=True)[:1000]}"
            )
        return (parse_json(raw) if json_response else {}), raw


def parse_json(raw: bytes) -> dict[str, Any]:
    if not raw:
        return {}
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ContractFailure("API response was not a JSON object")
    return value


def data(body: dict[str, Any]) -> dict[str, Any]:
    value = body.get("data", body)
    if not isinstance(value, dict):
        raise ContractFailure("API response data was not an object")
    return value


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractFailure(message)


def multipart(
    fields: dict[str, str],
    *,
    filename: str,
    media_type: str,
    content: bytes,
) -> tuple[str, bytes]:
    boundary = f"----brunn-contract-{uuid.uuid4().hex}"
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.extend([
            f"--{boundary}\r\n".encode(),
            f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode(),
            value.encode(),
            b"\r\n",
        ])
    chunks.extend([
        f"--{boundary}\r\n".encode(),
        (
            f'Content-Disposition: form-data; name="file"; '
            f'filename="{filename}"\r\n'
        ).encode(),
        f"Content-Type: {media_type}\r\n\r\n".encode(),
        content,
        b"\r\n",
        f"--{boundary}--\r\n".encode(),
    ])
    return f"multipart/form-data; boundary={boundary}", b"".join(chunks)


def write(
    client: Client,
    path: str,
    content: str,
    *,
    expected_version: int | None = None,
    metadata: dict[str, Any] | None = None,
    expected: int = 200,
) -> dict[str, Any]:
    body: dict[str, Any] = {
        "path": path,
        "content": content,
        "media_type": "text/markdown",
        "metadata": metadata or {},
    }
    if expected_version is not None:
        body["expected_version"] = expected_version
    response, _ = client.request(
        "POST",
        "/v1/workspace/write",
        body=body,
        expected=expected,
    )
    return response


def read_path(client: Client, path: str) -> dict[str, Any]:
    response, _ = client.request(
        "POST",
        "/v1/workspace/read",
        body={"requests": [{"path": path, "view": "full"}]},
    )
    return response


def checkpoint(
    client: Client,
    session_id: str,
    key: str,
    *,
    parent: str | None = None,
    source_refs: list[str] | None = None,
    expected: int = 200,
) -> dict[str, Any]:
    body: dict[str, Any] = {
        "session_id": session_id,
        "idempotency_key": key,
        "state": {
            "objective": "Live simple workspace contract",
            "current_state": ["Contract work is in progress."],
            "decisions": [],
            "open_questions": [],
            "next_actions": [],
            "artifacts": [],
        },
        "source_refs": source_refs or [],
    }
    if parent is not None:
        body["parent_checkpoint_id"] = parent
    response, _ = client.request(
        "POST",
        "/v1/workspace/checkpoint",
        body=body,
        expected=expected,
    )
    return response


def provision_user(admin: Client, run_id: str, label: str) -> Client:
    response, _ = admin.request(
        "POST",
        "/v1/admin/users",
        body={
            "external_ref": f"contract:{run_id}:{label}",
            "display_name": f"Contract {label}",
            "credential_name": f"Contract {label} owner",
        },
    )
    token = (
        response.get("credential", {}).get("token")
        if isinstance(response.get("credential"), dict)
        else None
    )
    require(isinstance(token, str) and bool(token), "provisioning omitted owner token")
    return Client(admin.base_url, token)


def upload_binary(
    client: Client,
    path: str,
    content: bytes,
    *,
    expected_hash: str | None = None,
    expected_version: int = 0,
    expected: int = 200,
) -> dict[str, Any]:
    digest = expected_hash or hashlib.sha256(content).hexdigest()
    content_type, body = multipart(
        {
            "path": path,
            "media_type": "application/octet-stream",
            "expected_content_hash": f"sha256:{digest}",
            "expected_version": str(expected_version),
            "description": f"Exact test bytes for {path}.",
        },
        filename=Path(path).name,
        media_type="application/octet-stream",
        content=content,
    )
    response, _ = client.request(
        "POST",
        "/v1/workspace/binaries",
        body=body,
        content_type=content_type,
        expected=expected,
    )
    return response


def run(base_url: str, env: dict[str, str], brunn_state: Path) -> dict[str, Any]:
    run_id = uuid.uuid4().hex
    admin = Client(base_url, env["BRUNN_DEV_READ_WRITE_TOKEN"])
    read_only = Client(base_url, env["BRUNN_DEV_READ_ONLY_TOKEN"])
    isolated = provision_user(admin, run_id, "isolated")
    started = time.monotonic()

    stable_request = {
        "content": "The stable contract answer is cobalt.",
        "source": {"origin": "contract"},
        "intent": "test replay",
        "idempotency_key": f"{run_id}:stable",
    }
    first, _ = admin.request("POST", "/v1/workspace/capture", body=stable_request)
    replay, _ = admin.request("POST", "/v1/workspace/capture", body=stable_request)
    require(first.get("status") == "committed", "first capture did not commit")
    require(replay.get("status") == "no_op", "stable capture replay was not a no-op")
    changed_capture = dict(stable_request)
    changed_capture["content"] = "The stable contract answer changed."
    admin.request(
        "POST",
        "/v1/workspace/capture",
        body=changed_capture,
        expected=409,
    )

    natural_path = f"Contracts/{run_id}/family-calendar.md"
    write(
        admin,
        natural_path,
        (
            "# Family calendar reconciliation\n\n"
            "Seven airline-generated events cover every confirmed Europe flight. "
            "No duplicate calendar write is needed."
        ),
        expected_version=0,
    )
    natural_search, _ = admin.request(
        "POST",
        "/v1/workspace/search",
        body={
            "queries": [{
                "id": "natural",
                "query": (
                    "Handle the request to add every confirmed Europe flight to the "
                    "family calendar. State what the bounded search found, whether "
                    "any write is needed, and how the work should be reported."
                ),
                "modes": ["lexical"],
                "limit": 5,
            }]
        },
    )
    natural_items = data(natural_search).get("results")
    natural_results = (
        natural_items[0].get("candidates")
        if isinstance(natural_items, list)
        and natural_items
        and isinstance(natural_items[0], dict)
        else None
    )
    require(
        isinstance(natural_results, list)
        and any(
            isinstance(result, dict) and result.get("path") == natural_path
            for result in natural_results
        ),
        "natural-language lexical fallback did not return the owning source",
    )
    hinted_path = f"Contracts/{run_id}/exact-hint.md"
    write(
        admin,
        hinted_path,
        "# Exact hinted source\n\nOnly the explicit root hint should select this text.",
        expected_version=0,
    )
    hinted_open, _ = admin.request(
        "POST",
        "/v1/workspace/open",
        body={
            "task": "Prepare unrelated work with no matching vocabulary.",
            "hints": {
                "authorization_scope": "scope:root",
                "root_refs": [hinted_path],
                "open_object_refs": [],
            },
        },
    )
    require(
        "Only the explicit root hint should select this text."
        in json.dumps(hinted_open),
        "exact open root hint was not hydrated",
    )

    many_headings_path = f"Contracts/{run_id}/many-headings.md"
    many_headings = "\n".join(
        f"## Heading {index}\n\nBoundary marker {index}."
        for index in range(7_000)
    )
    many_headings_write = data(
        write(
            admin,
            many_headings_path,
            many_headings,
            expected_version=0,
        )
    )
    require(
        many_headings_write.get("version") == 1,
        "entry-local publication failed for a document with 7,000 headings",
    )
    many_headings_read, _ = admin.request(
        "POST",
        "/v1/workspace/read",
        body={
            "requests": [{
                "path": many_headings_path,
                "view": "full",
                "max_chars": 4 * 1024 * 1024,
            }]
        },
    )
    require(
        "Boundary marker 6999" in json.dumps(many_headings_read),
        "large heading document was not readable after publication",
    )

    reserved_id = uuid.uuid4()
    write(
        admin,
        f".brunn/checkpoints/{reserved_id}.md",
        "not a checkpoint",
        expected_version=0,
        expected=400,
    )
    write(admin, r"Contracts\backslash.md", "no", expected_version=0, expected=400)

    markdown_path = f"Contracts/{run_id}/restore.md"
    created = data(write(admin, markdown_path, "version one", expected_version=0))
    entry_ref = str(created["entry_ref"])
    admin.request(
        "DELETE",
        f"/v1/workspace/entries/{urllib.parse.quote(entry_ref, safe='')}"
        "?expected_version=1",
    )
    restored = data(write(admin, markdown_path, "version one", expected_version=0))
    require(restored.get("no_op") is False, "same-byte tombstone restore was a false no-op")
    require(
        "version one" in json.dumps(read_path(admin, markdown_path)),
        "same-byte tombstone restore was not readable",
    )
    admin.request(
        "DELETE",
        f"/v1/workspace/entries/{urllib.parse.quote(entry_ref, safe='')}"
        "?expected_version=1",
    )
    changed = data(write(admin, markdown_path, "version two", expected_version=0))
    require(changed.get("version") == 2, "changed tombstone restore did not advance version")

    nfc_path = f"Contracts/{run_id}/Caf\u00e9.md"
    nfd_path = f"Contracts/{run_id}/Cafe\u0301.md"
    write(admin, nfd_path, "unicode one", expected_version=0)
    write(admin, nfc_path, "unicode two", expected_version=0, expected=409)

    binary_path = f"Contracts/{run_id}/receipt.bin"
    original_bytes = b"receipt-original-\x00\xff"
    uploaded = data(upload_binary(admin, binary_path, original_bytes))
    binary_ref = str(uploaded["entry_ref"])
    admin.request(
        "DELETE",
        f"/v1/workspace/entries/{urllib.parse.quote(binary_ref, safe='')}"
        "?expected_version=1",
    )
    binary_restored = data(upload_binary(admin, binary_path, original_bytes))
    require(binary_restored.get("no_op") is False, "same-byte binary restore was a false no-op")
    admin.request(
        "DELETE",
        f"/v1/workspace/entries/{urllib.parse.quote(binary_ref, safe='')}"
        "?expected_version=1",
    )
    replacement_bytes = b"receipt-replacement-\x01\xfe"
    binary_changed = data(upload_binary(admin, binary_path, replacement_bytes))
    require(binary_changed.get("version") == 2, "changed binary restore did not advance version")
    _, downloaded = admin.request(
        "GET",
        f"/v1/workspace/binaries/{urllib.parse.quote(binary_ref, safe='')}/content",
        json_response=False,
    )
    require(downloaded == replacement_bytes, "binary restore did not preserve exact bytes")
    bad_path = f"Contracts/{run_id}/bad-hash.bin"
    upload_binary(
        admin,
        bad_path,
        b"actual bytes",
        expected_hash="0" * 64,
        expected=409,
    )
    missing = read_path(admin, bad_path)
    require(
        missing.get("status") == "degraded",
        "hash-mismatched binary became visible",
    )

    opened, _ = admin.request(
        "POST",
        "/v1/workspace/open",
        body={"task": "Start the changes continuation contract."},
    )
    session_id = str(opened["session_id"])
    base_checkpoint = checkpoint(
        admin,
        session_id,
        f"{run_id}:base",
        source_refs=[markdown_path],
    )
    base_checkpoint_ref = str(data(base_checkpoint)["checkpoint_ref"])
    checkpoint_entry_ref = str(data(base_checkpoint)["write"]["entry_ref"])
    admin.request(
        "DELETE",
        f"/v1/workspace/entries/{urllib.parse.quote(checkpoint_entry_ref, safe='')}",
        expected=400,
    )
    for index in range(201):
        write(
            admin,
            f"Contracts/{run_id}/changes/{index:03d}.md",
            f"change {index}",
            expected_version=0,
        )
    resumed, _ = admin.request(
        "POST",
        "/v1/workspace/open",
        body={
            "task": "Resume all changes.",
            "resume_checkpoint_ref": base_checkpoint_ref,
        },
    )
    resume_data = data(resumed)
    changes = resume_data.get("changes_since_checkpoint")
    require(isinstance(changes, list) and len(changes) == 200, "resume did not return 200 changes")
    require(resume_data.get("changes_truncated") is True, "resume hid change truncation")
    next_generation = resume_data.get("next_changes_generation")
    require(isinstance(next_generation, int), "resume omitted continuation generation")
    resumed_paths = {
        item.get("path")
        for field in ("evidence", "evidence_leads")
        for item in resume_data.get(field, [])
        if isinstance(item, dict)
    }
    require(
        markdown_path in resumed_paths
        and any(
            isinstance(path, str)
            and path.startswith(f"Contracts/{run_id}/changes/")
            for path in resumed_paths
        ),
        "resume did not prioritize both checkpoint sources and changed paths",
    )
    follow_up, _ = admin.request(
        "GET",
        f"/v1/workspace/changes?since_generation={next_generation}&limit=200",
    )
    follow_up_changes = data(follow_up).get("changes")
    require(
        isinstance(follow_up_changes, list) and len(follow_up_changes) >= 2,
        "change continuation omitted remaining writes",
    )

    ordinary = data(
        write(
            admin,
            f"Contracts/{run_id}/ordinary.md",
            "ordinary",
            expected_version=0,
        )
    )
    checkpoint(
        admin,
        session_id,
        f"{run_id}:non-checkpoint-parent",
        parent=str(ordinary["entry_ref"]),
        expected=400,
    )
    checkpoint(
        admin,
        session_id,
        f"{run_id}:unknown-parent",
        parent=f"checkpoint:{uuid.uuid4()}",
        expected=404,
    )
    child = checkpoint(
        admin,
        session_id,
        f"{run_id}:child",
        parent=base_checkpoint_ref,
    )
    require(data(child).get("checkpoint_ref"), "valid child checkpoint failed")
    checkpoint(
        isolated,
        f"session:{uuid.uuid4()}",
        f"{run_id}:cross-user-parent",
        parent=base_checkpoint_ref,
        expected=404,
    )

    imported_checkpoint_id = uuid.uuid4()
    imported_checkpoint_path = (
        f".brunn/checkpoints/{imported_checkpoint_id}.md"
    )
    imported_checkpoint_ref = f"checkpoint:{imported_checkpoint_id}"
    imported_source_path = f"Contracts/{run_id}/imported-source.md"
    imported_source = data(
        write(
            isolated,
            imported_source_path,
            "Imported source marker.",
            expected_version=0,
        )
    )
    origin_generation = 9_000_000_000_000
    imported_checkpoint_content = (
        "---\nbrunn_kind: checkpoint\n"
        f"checkpoint_id: {imported_checkpoint_id}\n"
        f"workspace_generation: {origin_generation}\n---\n\n"
        "# Imported checkpoint\n\nPortable checkpoint marker.\n"
    )
    imported = write(
        isolated,
        imported_checkpoint_path,
        imported_checkpoint_content,
        expected_version=0,
        metadata={
            "kind": "checkpoint",
            "checkpoint_ref": imported_checkpoint_ref,
            "workspace_generation": origin_generation,
            "source_entries": [{
                "entry_ref": imported_source["entry_ref"],
                "path": imported_source_path,
                "version": imported_source["version"],
                "content_hash": imported_source["content_hash"],
            }],
            "_brunn_import": {
                "format": "brunn-workspace-import-manifest@v1"
            },
        },
    )
    require(imported.get("status") == "committed", "checkpoint restore did not commit")
    replayed_import = write(
        isolated,
        imported_checkpoint_path,
        imported_checkpoint_content,
        expected_version=0,
        metadata={
            "kind": "checkpoint",
            "checkpoint_ref": imported_checkpoint_ref,
            "workspace_generation": origin_generation,
            "_brunn_import": {
                "format": "brunn-workspace-import-manifest@v1"
            },
        },
    )
    require(
        replayed_import.get("status") == "no_op",
        "identical checkpoint restore replay was not a no-op",
    )
    unresolved_import_id = uuid.uuid4()
    unresolved_parent_id = uuid.uuid4()
    write(
        isolated,
        f".brunn/checkpoints/{unresolved_import_id}.md",
        (
            "---\nbrunn_kind: checkpoint\n"
            f"checkpoint_id: {unresolved_import_id}\n"
            f"parent_checkpoint_id: checkpoint:{unresolved_parent_id}\n"
            "---\n\nUnresolved portable child.\n"
        ),
        expected_version=0,
        metadata={
            "kind": "checkpoint",
            "checkpoint_ref": f"checkpoint:{unresolved_import_id}",
            "parent_checkpoint_ref": f"checkpoint:{unresolved_parent_id}",
            "_brunn_import": {
                "format": "brunn-workspace-import-manifest@v1"
            },
        },
        expected=409,
    )
    imported_child_id = uuid.uuid4()
    imported_child_ref = f"checkpoint:{imported_child_id}"
    imported_child_path = f".brunn/checkpoints/{imported_child_id}.md"
    imported_child = write(
        isolated,
        imported_child_path,
        (
            "---\nbrunn_kind: checkpoint\n"
            f"checkpoint_id: {imported_child_id}\n"
            f"parent_checkpoint_id: {imported_checkpoint_ref}\n"
            "---\n\nPortable child checkpoint.\n"
        ),
        expected_version=0,
        metadata={
            "kind": "checkpoint",
            "checkpoint_ref": imported_child_ref,
            "parent_checkpoint_ref": imported_checkpoint_ref,
            "workspace_generation": origin_generation + 1,
            "_brunn_import": {
                "format": "brunn-workspace-import-manifest@v1"
            },
        },
    )
    require(
        imported_child.get("status") == "committed",
        "portable checkpoint child did not resolve its imported parent",
    )
    opened_child, _ = isolated.request(
        "POST",
        "/v1/workspace/open",
        body={
            "task": "Resume the imported portable child.",
            "resume_checkpoint_ref": imported_child_ref,
        },
    )
    require(
        "Portable child checkpoint" in json.dumps(opened_child),
        "imported checkpoint child was not resumable",
    )
    write(
        isolated,
        imported_checkpoint_path,
        imported_checkpoint_content + "\nforged revision\n",
        expected_version=1,
        metadata={
            "kind": "checkpoint",
            "checkpoint_ref": imported_checkpoint_ref,
            "workspace_generation": origin_generation,
            "_brunn_import": {
                "format": "brunn-workspace-import-manifest@v1"
            },
        },
        expected=409,
    )
    post_import_change = f"Contracts/{run_id}/after-import.md"
    write(
        isolated,
        post_import_change,
        "This local change must remain visible after resume.",
        expected_version=0,
    )
    reopened, _ = isolated.request(
        "POST",
        "/v1/workspace/open",
        body={
            "task": "Resume the portable checkpoint marker.",
            "resume_checkpoint_ref": imported_checkpoint_ref,
        },
    )
    require(
        "Portable checkpoint marker" in json.dumps(reopened),
        "path-based checkpoint identity did not survive import",
    )
    reopened_data = data(reopened)
    reopened_checkpoint = reopened_data.get("checkpoint")
    require(
        isinstance(reopened_checkpoint, dict)
        and isinstance(reopened_checkpoint.get("workspace_generation"), int)
        and reopened_checkpoint["workspace_generation"] < origin_generation,
        "imported checkpoint generation was not rebased locally",
    )
    require(
        all(
            not isinstance(source, dict) or "entry_ref" not in source
            for source in reopened_checkpoint.get("source_entries", [])
        ),
        "imported checkpoint retained stale origin entry IDs",
    )
    require(
        any(
            isinstance(change, dict) and change.get("path") == post_import_change
            for change in reopened_data.get("changes_since_checkpoint", [])
        ),
        "local changes were hidden by an imported checkpoint generation",
    )

    export_source = provision_user(admin, run_id, "export-source")
    export_target = provision_user(admin, run_id, "export-target")
    export_path = f"Contracts/{run_id}/export-source.md"
    write(export_source, export_path, "Exact export source.", expected_version=0)
    export_open, _ = export_source.request(
        "POST",
        "/v1/workspace/open",
        body={"task": "Create an exportable checkpoint."},
    )
    exported_checkpoint = checkpoint(
        export_source,
        str(export_open["session_id"]),
        f"{run_id}:export-checkpoint",
        source_refs=[export_path],
    )
    exported_checkpoint_ref = str(data(exported_checkpoint)["checkpoint_ref"])
    with tempfile.TemporaryDirectory(prefix="brunn-contract-export-") as temporary:
        root = Path(temporary)
        exported_root = root / "export"
        state_root = root / "state"
        base_env = {
            **os.environ,
            "BRUNN_STATE_API_URL": base_url,
        }
        exported = subprocess.run(
            [
                str(brunn_state),
                "workspace",
                "export",
                "--output",
                str(exported_root),
            ],
            env={**base_env, "BRUNN_STATE_API_TOKEN": export_source.token},
            text=True,
            capture_output=True,
            check=False,
        )
        require(
            exported.returncode == 0,
            f"workspace export failed: {exported.stderr[-500:]}",
        )
        imported_result = subprocess.run(
            [
                str(brunn_state),
                "workspace",
                "import",
                "--root",
                str(exported_root),
                "--state-dir",
                str(state_root),
                "--describe-binaries",
                "false",
            ],
            env={**base_env, "BRUNN_STATE_API_TOKEN": export_target.token},
            text=True,
            capture_output=True,
            check=False,
        )
        require(
            imported_result.returncode == 0,
            f"workspace import failed: {imported_result.stderr[-500:]}",
        )
    exported_reopen, _ = export_target.request(
        "POST",
        "/v1/workspace/open",
        body={
            "task": "Resume the checkpoint after a portable export and import.",
            "resume_checkpoint_ref": exported_checkpoint_ref,
        },
    )
    require(
        "Live simple workspace contract" in json.dumps(exported_reopen),
        "checkpoint did not resume after actual CLI export and import",
    )
    require(
        "Exact export source" in json.dumps(read_path(export_target, export_path)),
        "source bytes did not survive actual CLI export and import",
    )

    private_path = f"Contracts/{run_id}/private.md"
    write(admin, private_path, "owner-only marker", expected_version=0)
    isolated_read = read_path(isolated, private_path)
    require(
        isolated_read.get("status") == "degraded",
        "another user could read the owner's path",
    )
    read_only.request(
        "POST",
        "/v1/workspace/open",
        body={"task": "Read-only access should open."},
    )
    write(
        read_only,
        f"Contracts/{run_id}/read-only-denied.md",
        "must not persist",
        expected_version=0,
        expected=403,
    )
    read_only.request(
        "POST",
        "/v1/workspace/checkpoint",
        body={
            "session_id": f"session:{uuid.uuid4()}",
            "idempotency_key": f"{run_id}:read-only",
            "state": {"objective": "must fail"},
            "source_refs": [],
        },
        expected=403,
    )

    return {
        "schema": "brunn-simple-workspace-live-contract@v1",
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "checks": {
            "stable_capture_replay": True,
            "natural_language_lexical_fallback": True,
            "exact_open_hints": True,
            "large_entry_publication": True,
            "reserved_namespace": True,
            "markdown_tombstone_restore": True,
            "binary_hash_and_tombstone_restore": True,
            "portable_path_collision": True,
            "changes_continuation": True,
            "continuation_source_priority": True,
            "checkpoint_lineage": True,
            "checkpoint_portable_identity": True,
            "checkpoint_generation_rebase": True,
            "checkpoint_immutability": True,
            "checkpoint_cli_export_import": True,
            "user_isolation": True,
            "read_only_boundary": True,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:55213")
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument(
        "--brunn-state",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "apps"
        / "api"
        / "target"
        / "debug"
        / "brunn-state",
    )
    args = parser.parse_args()
    try:
        result = run(args.base_url, load_env(args.env_file), args.brunn_state.resolve())
    except (ContractFailure, KeyError, OSError, ValueError) as error:
        result = {
            "schema": "brunn-simple-workspace-live-contract@v1",
            "status": "fail",
            "error": str(error),
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

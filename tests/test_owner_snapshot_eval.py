from __future__ import annotations

import hashlib
import json
import os
import stat
import tempfile
import threading
import unittest
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from typing import Iterator

from owner_snapshot_eval import (
    AUDIT_SCHEMA,
    INVENTORY_SCHEMA,
    LEAK_REQUEST_SCHEMA,
    LINK_CANDIDATES_SCHEMA,
    build_audit,
    build_inventory,
    build_leak_report,
    build_link_candidates,
    fidelity_differences,
    import_and_audit,
    import_documents,
    load_credential,
    load_inventory,
    manifest_entries,
    sha256_bytes,
    write_credential,
    write_json,
)


ROOT = Path(__file__).resolve().parents[1]
PARENT_ID = "11111111-1111-4111-8111-111111111111"
CHILD_ID = "22222222-2222-4222-8222-222222222222"


class SnapshotApiHandler(BaseHTTPRequestHandler):
    imported_documents: list[dict] = []
    requests: list[dict] = []

    def log_message(self, format: str, *args: object) -> None:
        return

    def send_json(self, status: int, value: dict) -> None:
        payload = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length))
        type(self).requests.append(
            {
                "method": "POST",
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
                "body": body,
            }
        )
        if self.path != "/v1/workspace/admin/eval/import":
            self.send_json(404, {"error": "not_found"})
            return
        type(self).imported_documents.extend(body["documents"])
        self.send_json(
            200,
            {
                "status": "indexing",
                "ready_for_evaluation": False,
                "import_id": "simple-import:test",
                "status_url": "/v1/workspace/admin/eval/imports/simple-import:test",
                "authorization_scope": body["authorization_scope"],
                "requested_authorization_scope": body["authorization_scope"],
                "base_corpus_revision": "generation:0",
                "corpus_revision": "generation:1",
                "checkpoint_id": None,
                "index_status": {
                    "exact": "ready",
                    "lexical": "ready",
                    "semantic": "pending",
                },
                "credential_token": "scoped-read-only-token",
            },
        )

    def do_GET(self) -> None:
        type(self).requests.append(
            {
                "method": "GET",
                "path": self.path,
                "authorization": self.headers.get("Authorization"),
            }
        )
        if not self.path.startswith("/v1/workspace/manifest?"):
            self.send_json(404, {"error": "not_found"})
            return
        entries = [
            {
                "path": document["path"],
                "kind": "markdown",
                "content_hash": "sha256:" + document["content_sha256"],
                "size_bytes": len(document["content"].encode("utf-8")),
            }
            for document in self.imported_documents
        ]
        self.send_json(200, {"data": {"entries": entries, "next": None}})


@contextmanager
def snapshot_api() -> Iterator[str]:
    SnapshotApiHandler.imported_documents = []
    SnapshotApiHandler.requests = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), SnapshotApiHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        thread.join(timeout=5)
        server.server_close()


class OwnerSnapshotEvalTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="owner-snapshot-test-")
        self.root = Path(self.temporary.name) / "vault"
        self.root.mkdir()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_bytes(self, relative: str, payload: bytes) -> Path:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        return path

    def inventory_path(self) -> tuple[dict, Path]:
        inventory = build_inventory(self.root)
        path = Path(self.temporary.name) / "inventory.json"
        write_json(path, inventory)
        return inventory, path

    def test_inventory_preserves_exact_text_bytes_and_inventory_boundaries(self) -> None:
        exact = b"\xef\xbb\xbf# Current\r\n\r\nExact CRLF bytes.\r\n"
        self.write_bytes("Current.md", exact)
        self.write_bytes("asset.bin", b"\x00\xffbinary")
        symlink = self.root / "outside-link"
        symlink.symlink_to("/tmp")

        inventory, path = self.inventory_path()
        loaded = load_inventory(path)

        self.assertEqual(loaded["schema"], INVENTORY_SCHEMA)
        by_path = {entry["path"]: entry for entry in loaded["entries"]}
        self.assertEqual(by_path["Current.md"]["classification"], "supported_text")
        self.assertEqual(by_path["Current.md"]["size_bytes"], len(exact))
        self.assertEqual(by_path["Current.md"]["sha256"], sha256_bytes(exact))
        self.assertEqual(by_path["asset.bin"]["classification"], "unsupported_binary")
        self.assertEqual(by_path["outside-link"]["classification"], "ignored_symlink")
        self.assertFalse(loaded["boundaries"]["history"]["included"])

        documents = import_documents(inventory, self.root, batch_size=100)
        self.assertEqual(len(documents), 1)
        self.assertEqual(documents[0]["content"].encode("utf-8"), exact)
        self.assertEqual(
            documents[0]["content_sha256"],
            hashlib.sha256(exact).hexdigest(),
        )

    def test_inventory_detects_source_checkpoint_lineage_without_claiming_service_rows(self) -> None:
        self.write_bytes(
            f".straylight/checkpoints/{PARENT_ID}.md",
            (
                "---\n"
                f"checkpoint_id: checkpoint:{PARENT_ID}\n"
                "parent_checkpoint_id:\n"
                "---\n"
            ).encode(),
        )
        self.write_bytes(
            f".straylight/checkpoints/{CHILD_ID}.md",
            (
                "---\n"
                f"checkpoint_id: checkpoint:{CHILD_ID}\n"
                f"parent_checkpoint_id: checkpoint:{PARENT_ID}\n"
                "---\n"
            ).encode(),
        )

        inventory = build_inventory(self.root)
        lineage = inventory["checkpoint_lineage"]

        self.assertTrue(lineage["source_resolution_pass"])
        self.assertEqual(lineage["definitions"], 2)
        self.assertEqual(lineage["counts"]["resolved"], 1)
        self.assertEqual(lineage["service_representation"], "source_text_only_on_evaluation_import")

    def test_inventory_reports_unresolved_parent_checkpoint(self) -> None:
        self.write_bytes(
            "checkpoint.md",
            (
                "---\n"
                f"checkpoint_id: checkpoint:{CHILD_ID}\n"
                f"parent_checkpoint_id: checkpoint:{PARENT_ID}\n"
                "---\n"
            ).encode(),
        )

        lineage = build_inventory(self.root)["checkpoint_lineage"]

        self.assertFalse(lineage["source_resolution_pass"])
        self.assertEqual(lineage["counts"]["unresolved"], 1)

    def test_inventory_fingerprint_detects_artifact_tampering(self) -> None:
        self.write_bytes("Current.md", b"current")
        inventory, path = self.inventory_path()
        inventory["entries"][0]["size_bytes"] += 1
        write_json(path, inventory)

        with self.assertRaisesRegex(RuntimeError, "fingerprint mismatch"):
            load_inventory(path)

    def test_import_documents_refuses_source_mutation(self) -> None:
        path = self.write_bytes("Current.md", b"before")
        inventory = build_inventory(self.root)
        path.write_bytes(b"after")

        with self.assertRaisesRegex(RuntimeError, "changed after inventory"):
            import_documents(inventory, self.root, batch_size=10)

    def test_fidelity_audit_requires_exact_path_size_hash_and_no_extras(self) -> None:
        self.write_bytes("Current.md", b"current")
        inventory = build_inventory(self.root)
        expected = inventory["entries"][0]
        actual = [
            {
                "path": "Current.md",
                "kind": "markdown",
                "size_bytes": expected["size_bytes"],
                "content_hash": expected["sha256"],
            }
        ]

        self.assertTrue(fidelity_differences(inventory, actual)["pass"])
        actual[0]["size_bytes"] += 1
        actual.append(
            {
                "path": "Unexpected.md",
                "kind": "markdown",
                "size_bytes": 1,
                "content_hash": "sha256:" + "0" * 64,
            }
        )
        result = fidelity_differences(inventory, actual)
        self.assertFalse(result["pass"])
        self.assertEqual(result["unexpected_paths"], ["Unexpected.md"])
        self.assertEqual(result["mismatches"][0]["path"], "Current.md")

    def test_audit_calls_supported_text_pass_but_full_d14_blocked(self) -> None:
        self.write_bytes("Current.md", b"current")
        self.write_bytes("image.png", b"\x89PNG\r\n\x1a\npayload")
        inventory = build_inventory(self.root)
        expected = next(
            entry
            for entry in inventory["entries"]
            if entry["classification"] == "supported_text"
        )
        audit = build_audit(
            inventory,
            [
                {
                    "path": expected["path"],
                    "kind": "markdown",
                    "size_bytes": expected["size_bytes"],
                    "content_hash": expected["sha256"],
                }
            ],
            api_url="http://127.0.0.1:19999",
        )

        self.assertEqual(audit["schema"], AUDIT_SCHEMA)
        self.assertEqual(audit["tier_a_assessment"]["supported_text_scope"], "pass")
        self.assertEqual(
            audit["tier_a_assessment"]["full_d14_zero_diff_gate"],
            "blocked",
        )
        self.assertTrue(
            any("binary" in blocker for blocker in audit["tier_a_assessment"]["blockers"])
        )
        self.assertFalse(audit["checkpoint_lineage"]["service_resolution_verified"])

    def test_import_uses_read_only_eval_route_then_verifies_manifest(self) -> None:
        exact = b"# Current\r\n"
        self.write_bytes("Current.md", exact)
        inventory = build_inventory(self.root)
        credential = Path(self.temporary.name) / "credential.json"

        with snapshot_api() as api_url:
            audit = import_and_audit(
                inventory,
                root=self.root,
                api_url=api_url,
                admin_token="administrative-token",
                run_id="owner-run",
                case_id="owner-case",
                batch_size=100,
                credential_out=credential,
                timeout_seconds=5,
            )

        self.assertTrue(audit["supported_text_fidelity"]["pass"])
        self.assertEqual(audit["target"]["access_mode"], "read_only")
        self.assertEqual(
            load_credential(credential),
            (api_url, "scoped-read-only-token"),
        )
        posted = SnapshotApiHandler.requests[0]
        self.assertEqual(posted["path"], "/v1/workspace/admin/eval/import")
        self.assertEqual(posted["authorization"], "Bearer administrative-token")
        self.assertEqual(posted["body"]["access_mode"], "read_only")
        self.assertEqual(
            posted["body"]["documents"][0]["content"].encode("utf-8"),
            exact,
        )
        self.assertEqual(
            SnapshotApiHandler.requests[-1]["authorization"],
            "Bearer scoped-read-only-token",
        )
        self.assertNotIn("credential_token", json.dumps(audit))
        self.assertNotIn("scoped-read-only-token", json.dumps(audit))

    def test_link_candidates_resolve_unique_wiki_links_without_content_or_questions(self) -> None:
        self.write_bytes("Alpha.md", b"# Alpha\n")
        self.write_bytes("Sub/Beta.md", b"# Beta\n")
        self.write_bytes("Gamma.md", b"# Gamma\n")
        self.write_bytes("Delta.md", b"# Delta\n")
        self.write_bytes(
            "Hub.md",
            (
                "# Hub\n"
                "[[Alpha|A]]\n"
                "[[Sub/Beta.md]]\n"
                "[[Gamma#Detail]]\n"
                "[[Missing]]\n"
            ).encode(),
        )
        inventory = build_inventory(self.root)

        artifact = build_link_candidates(
            inventory,
            root=self.root,
            min_resolved_links=3,
        )

        self.assertEqual(artifact["schema"], LINK_CANDIDATES_SCHEMA)
        self.assertEqual([item["path"] for item in artifact["candidates"]], ["Hub.md"])
        candidate = artifact["candidates"][0]
        self.assertEqual(
            candidate["resolved_paths"],
            ["Alpha.md", "Gamma.md", "Sub/Beta.md"],
        )
        rendered = json.dumps(artifact)
        self.assertNotIn("# Hub", rendered)
        self.assertFalse(
            artifact["selection_policy"]["questions_or_rubric_answers_in_artifact"]
        )

    def test_leak_report_hashes_claim_text_and_lists_only_matching_paths(self) -> None:
        self.write_bytes("Alpha.md", b"The exact hidden rubric claim is here.")
        self.write_bytes("Beta.md", b"Unrelated.")
        inventory = build_inventory(self.root)
        request = {
            "schema": LEAK_REQUEST_SCHEMA,
            "checks": [
                {
                    "case_id": "owner-1",
                    "claim_id": "c1",
                    "text": "exact hidden rubric claim",
                },
                {
                    "case_id": "owner-1",
                    "claim_id": "c2",
                    "text": "does not occur",
                },
            ],
        }

        report = build_leak_report(inventory, root=self.root, request=request)

        self.assertFalse(report["summary"]["pass"])
        self.assertEqual(report["summary"]["leaks"], 1)
        self.assertEqual(report["checks"][0]["matching_paths"], ["Alpha.md"])
        rendered = json.dumps(report)
        self.assertNotIn("exact hidden rubric claim", rendered)

    def test_credential_file_is_private_and_refuses_reuse(self) -> None:
        path = Path(self.temporary.name) / "credential.json"
        write_credential(path, api_url="http://127.0.0.1:1", token="secret")

        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertEqual(
            load_credential(path),
            ("http://127.0.0.1:1", "secret"),
        )
        with self.assertRaisesRegex(RuntimeError, "already exists"):
            write_credential(path, api_url="http://127.0.0.1:1", token="secret")

    def test_manifest_pagination_uses_cursor_and_collects_all_entries(self) -> None:
        class FakeClient:
            def __init__(self) -> None:
                self.paths: list[str] = []

            def get(self, path: str) -> SimpleNamespace:
                self.paths.append(path)
                if len(self.paths) == 1:
                    data = {
                        "entries": [{"path": "A.md"}],
                        "next": {
                            "after_path": "A.md",
                            "after_entry_ref": "entry:1",
                        },
                    }
                else:
                    data = {"entries": [{"path": "B.md"}], "next": None}
                return SimpleNamespace(data=data)

        client = FakeClient()
        entries = manifest_entries(client)  # type: ignore[arg-type]

        self.assertEqual([entry["path"] for entry in entries], ["A.md", "B.md"])
        self.assertIn("after_path=A.md", client.paths[1])
        self.assertIn("after_entry_ref=entry%3A1", client.paths[1])

    def test_artifact_schemas_are_valid_json_and_cover_all_outputs(self) -> None:
        schemas = {
            "owner_snapshot_inventory.schema.json": INVENTORY_SCHEMA,
            "owner_snapshot_audit.schema.json": AUDIT_SCHEMA,
            "owner_link_candidates.schema.json": LINK_CANDIDATES_SCHEMA,
            "owner_link_leak_request.schema.json": LEAK_REQUEST_SCHEMA,
            "owner_link_leak_report.schema.json": "straylight-owner-link-leak-report@v1",
        }
        for filename, schema_value in schemas.items():
            with self.subTest(filename=filename):
                schema = json.loads((ROOT / "eval" / filename).read_text())
                self.assertEqual(
                    schema["properties"]["schema"]["const"],
                    schema_value,
                )


if __name__ == "__main__":
    unittest.main()

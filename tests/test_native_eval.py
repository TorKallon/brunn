from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Iterator

from agent_work_eval import select_conditions
from native_eval import (
    NativeApiClient,
    NativeResponse,
    provisioning_matches_run_case,
    provision_evaluation,
    public_provisioning,
    text_documents,
    write_native_memory_wrapper,
)
from native_memory import (
    build_parser,
    operation_request,
    render_native_response,
    response_payload_metrics,
)
from transition_eval import (
    attach_filesystem_sidecar_lineage,
    attach_native_lineage,
    build_codex_command as build_transition_codex_command,
    build_parser as build_transition_parser,
    run_mutation_hook,
    select_transition_conditions,
    validate as validate_transition,
)


ROOT = Path(__file__).resolve().parents[1]


class FakeNativeHandler(BaseHTTPRequestHandler):
    requests: list[dict[str, Any]] = []
    deny_checkpoint = False
    open_delay = 0.0
    import_failures = 0
    import_lexical_status = "building"
    issued_credential_token: str | None = "case-token"
    issued_authorization_scope: str | None = None
    session_revision = "revision:delta"
    evidence_sources: dict[str, str] = {}
    asset_bytes = b"verified asset bytes"
    child = {
        "checkpoint_id": "checkpoint:child",
        "parent_checkpoint_id": "checkpoint:seed",
        "corpus_revision": "revision:delta",
        "source_refs": ["prior.md", "delta.md"],
    }
    workspace_sources: dict[str, dict[str, Any]] = {}

    def log_message(self, format: str, *args: Any) -> None:
        return

    def send_json(self, status: int, body: dict[str, Any]) -> None:
        rendered = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(rendered)))
        self.send_header("X-Request-Id", f"http-{len(self.requests)}")
        self.end_headers()
        self.wfile.write(rendered)

    def read_json(self) -> dict[str, Any]:
        length = int(self.headers.get("Content-Length", "0"))
        return json.loads(self.rfile.read(length) or b"{}")

    def record(self, body: dict[str, Any] | None = None) -> None:
        self.requests.append({
            "method": self.command,
            "path": self.path,
            "authorization": self.headers.get("Authorization"),
            "run": self.headers.get("X-Straylight-Eval-Run"),
            "case": self.headers.get("X-Straylight-Eval-Case"),
            "body": body,
        })

    def do_GET(self) -> None:
        self.record()
        if self.path == "/asset.bin":
            digest = hashlib.sha256(self.asset_bytes).hexdigest()
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(self.asset_bytes)))
            self.send_header("X-CarryState-SHA256", f"sha256:{digest}")
            self.end_headers()
            self.wfile.write(self.asset_bytes)
        elif self.path == "/v1/admin/eval/imports/import:1":
            self.send_json(200, {
                "status": "complete",
                "corpus_revision": "revision:delta",
                "data": {
                    "status": "ready",
                    "corpus_revision": "revision:delta",
                    "index_status": {"exact": "ready", "lexical": "ready", "semantic": "ready"},
                },
            })
        elif self.path.startswith("/v1/sessions/"):
            self.send_json(200, {
                "status": "complete",
                "data": {
                    "session_id": "session:s1",
                    "corpus_revision": self.session_revision,
                    "checkpoints": [{
                        "checkpoint_id": self.child["checkpoint_id"],
                        "corpus_revision": self.child["corpus_revision"],
                        "checkpoint": self.child,
                    }],
                },
            })
        elif self.path.startswith("/v1/checkpoints/"):
            self.send_json(200, {"status": "complete", "data": self.child})
        else:
            self.send_json(404, {"error": {"code": "not_found"}})

    def do_POST(self) -> None:
        body = self.read_json()
        self.record(body)
        if self.path == "/v1/admin/eval/import":
            if self.import_failures > 0:
                type(self).import_failures -= 1
                self.send_json(503, {
                    "error": {
                        "code": "dependency_unavailable",
                        "message": "OpenAI embeddings temporarily throttled",
                    },
                })
                return
            data = {
                "import_id": "import:1",
                "status_url": "/v1/admin/eval/imports/import:1",
                "checkpoint_id": "checkpoint:seed" if body.get("seed_checkpoint") else None,
                "base_corpus_revision": "revision:base",
                "corpus_revision": "revision:delta",
                "requested_authorization_scope": body["authorization_scope"],
                "index_status": {
                    "exact": "ready",
                    "lexical": self.import_lexical_status,
                    "semantic": "building",
                },
            }
            if self.issued_authorization_scope != "__missing__":
                data["authorization_scope"] = (
                    self.issued_authorization_scope
                    if self.issued_authorization_scope is not None
                    else body["authorization_scope"]
                )
            if self.issued_credential_token is not None:
                data["credential_token"] = self.issued_credential_token
            self.send_json(200, {
                "status": "accepted_processing",
                "data": data,
            })
        elif self.path == "/v1/memory/open":
            time.sleep(self.open_delay)
            self.send_json(200, {
                "request_id": "req-open",
                "session_id": "session:s1",
                "corpus_revision": "revision:delta",
                "status": "complete",
                "data": {"session_id": "session:s1", "resumed_checkpoint": "checkpoint:seed"},
            })
        elif self.path == "/v1/memory/checkpoint" and self.deny_checkpoint:
            self.send_json(403, {
                "error": {"code": "capability_denied", "operation": "memory.checkpoint"},
            })
        elif self.path == "/v1/memory/checkpoint":
            child = {
                **self.child,
                "parent_checkpoint_id": body.get("parent_checkpoint_id"),
                "source_refs": body.get("source_refs", []),
            }
            type(self).child = child
            self.send_json(200, {
                "request_id": "req-checkpoint",
                "session_id": "session:s1",
                "corpus_revision": "revision:delta",
                "status": "committed",
                "data": child,
            })
        elif self.path == "/v1/memory/read" and self.evidence_sources:
            self.send_json(200, {
                "request_id": "req-read",
                "session_id": body.get("session_id", "session:s1"),
                "corpus_revision": "revision:delta",
                "status": "complete",
                "data": {
                    "items": [{
                        "reference": request["ref"],
                        "status": "complete",
                        "data": {
                            "metadata": {
                                "ref": request["ref"],
                                "source_ref": self.evidence_sources[request["ref"]],
                                "locator": {
                                    "path": self.evidence_sources[request["ref"]],
                                },
                            },
                            "text": "x",
                        },
                    } for request in body["requests"]],
                },
            })
        elif self.path == "/v1/workspace/read":
            requested_paths = [
                request.get("path")
                for request in body.get("requests", [])
                if isinstance(request, dict) and isinstance(request.get("path"), str)
            ]
            if requested_paths and self.workspace_sources:
                self.send_json(200, {
                    "request_id": "req-workspace-read",
                    "corpus_revision": "generation:43",
                    "status": "complete",
                    "data": {
                        "items": [
                            {
                                "reference": f"entry:{index}",
                                "path": path,
                                "version": self.workspace_sources[path]["version"],
                                "content_hash": self.workspace_sources[path]["content_hash"],
                                "media_type": "text/markdown",
                                "view": "full",
                                "text": self.workspace_sources[path]["text"],
                            }
                            for index, path in enumerate(requested_paths, 1)
                        ],
                        "requested_count": len(requested_paths),
                    },
                })
                return
            source_refs = self.child.get("source_refs", [])
            self.send_json(200, {
                "request_id": "req-workspace-read",
                "session_id": body.get("session_id", "session:s1"),
                "corpus_revision": "generation:43",
                "status": "complete",
                "data": {
                    "items": [{
                        "reference": "entry:child",
                        "path": ".straylight/checkpoints/child.md",
                        "text": "# Child checkpoint",
                        "metadata": {
                            "kind": "checkpoint",
                            "checkpoint_ref": self.child["checkpoint_id"],
                            "parent_checkpoint_ref": self.child[
                                "parent_checkpoint_id"
                            ],
                            "workspace_generation": 42,
                            "source_refs": source_refs,
                            "source_entries": [
                                {
                                    "path": source_ref,
                                    "entry_ref": f"entry:{index}",
                                    "version": 1,
                                    "content_hash": f"sha256:{index:064x}",
                                }
                                for index, source_ref in enumerate(source_refs, 1)
                            ],
                        },
                    }],
                    "requested_count": 1,
                },
            })
        elif self.path == "/v1/workspace/write":
            path = body["path"]
            current = self.workspace_sources[path]
            if body.get("expected_version") != current["version"]:
                self.send_json(409, {
                    "error": {
                        "code": "entry_version_conflict",
                        "message": "wrong expected version",
                    }
                })
                return
            content_hash = "sha256:" + hashlib.sha256(
                body["content"].encode()
            ).hexdigest()
            current.update({
                "text": body["content"],
                "version": current["version"] + 1,
                "content_hash": content_hash,
            })
            self.send_json(200, {
                "request_id": "req-workspace-write",
                "corpus_revision": f"generation:{42 + current['version']}",
                "status": "committed",
                "data": {
                    "path": path,
                    "version": current["version"],
                    "content_hash": content_hash,
                },
            })
        elif self.path.startswith("/v1/memory/"):
            operation = self.path.rsplit("/", 1)[-1]
            self.send_json(200, {
                "request_id": f"req-{operation}",
                "session_id": body.get("session_id", "session:s1"),
                "corpus_revision": "revision:delta",
                "status": "complete" if operation != "save" else "committed",
                "data": {"operation": operation, "request": body},
            })
        else:
            self.send_json(404, {"error": {"code": "not_found"}})


@contextmanager
def fake_server(
    *,
    deny_checkpoint: bool = False,
    open_delay: float = 0.0,
    import_failures: int = 0,
    import_lexical_status: str = "building",
    issued_credential_token: str | None = "case-token",
    issued_authorization_scope: str | None = None,
) -> Iterator[tuple[str, type[FakeNativeHandler]]]:
    handler = type("ConfiguredFakeNativeHandler", (FakeNativeHandler,), {})
    handler.requests = []
    handler.deny_checkpoint = deny_checkpoint
    handler.open_delay = open_delay
    handler.import_failures = import_failures
    handler.import_lexical_status = import_lexical_status
    handler.issued_credential_token = issued_credential_token
    handler.issued_authorization_scope = issued_authorization_scope
    handler.session_revision = FakeNativeHandler.session_revision
    handler.evidence_sources = {}
    handler.workspace_sources = {}
    handler.asset_bytes = FakeNativeHandler.asset_bytes
    handler.child = dict(FakeNativeHandler.child)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", handler
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


class NativeEvaluationTests(unittest.TestCase):
    def test_open_passes_project_scope_into_retrieval_task(self):
        with tempfile.TemporaryDirectory() as temporary:
            task = Path(temporary) / "task.txt"
            task.write_text("Resume final parser validation.", encoding="utf-8")
            args = build_parser().parse_args([
                "--state", str(Path(temporary) / "state.json"),
                "--task-file", str(task),
                "--scope", "Warmind",
                "--authorization-scope", "eval:run/case",
                "--protocol", "simple",
                "open",
                "--root-ref", "Projects/Warmind/Current.md",
                "--root-ref", "Projects/Warmind/Rules.md",
                "--open-object-ref", "entry:one",
            ])
            method, path, payload = operation_request(args, {})
            args.scope = "Recent Work"
            _, _, generic_payload = operation_request(args, {})

        self.assertEqual((method, path), ("POST", "/v1/workspace/open"))
        self.assertEqual(payload["task"], "Warmind: Resume final parser validation.")
        self.assertEqual(
            payload["hints"]["root_refs"],
            ["Projects/Warmind/Current.md", "Projects/Warmind/Rules.md"],
        )
        self.assertEqual(payload["hints"]["open_object_refs"], ["entry:one"])
        self.assertEqual(
            generic_payload["task"],
            "Resume final parser validation.",
        )

    def test_simple_changes_exposes_generation_pagination_without_a_session(self):
        args = build_parser().parse_args([
            "--state", "/tmp/state.json",
            "--task-file", "/tmp/task.txt",
            "--scope", "Alpha",
            "--authorization-scope", "eval:run/case",
            "--protocol", "simple",
            "changes",
            "--since-generation", "240",
            "--limit", "200",
        ])

        method, path, payload = operation_request(args, {})

        self.assertEqual(method, "GET")
        self.assertEqual(
            path,
            "/v1/workspace/changes?since_generation=240&limit=200",
        )
        self.assertIsNone(payload)

    def test_simple_read_encodes_paths_and_refs_once(self):
        args = build_parser().parse_args([
            "--state", "/tmp/state.json",
            "--task-file", "/tmp/task.txt",
            "--scope", "Alpha",
            "--authorization-scope", "eval:run/case",
            "--protocol", "simple",
            "read",
            "--ref", "entry:one",
            "--path", "one.md",
            "--path", "two.md",
        ])

        method, path, payload = operation_request(
            args,
            {"session_id": "session:s1"},
        )

        self.assertEqual(method, "POST")
        self.assertEqual(path, "/v1/workspace/read")
        self.assertEqual(payload, {
            "session_id": "session:s1",
            "requests": [
                {
                    "ref": "entry:one",
                    "view": "full",
                    "max_chars": 20_000,
                },
                {
                    "path": "one.md",
                    "view": "full",
                    "max_chars": 20_000,
                },
                {
                    "path": "two.md",
                    "view": "full",
                    "max_chars": 20_000,
                },
            ],
        })

        neighbor_args = build_parser().parse_args([
            "--state", "/tmp/state.json",
            "--task-file", "/tmp/task.txt",
            "--scope", "Alpha",
            "--authorization-scope", "eval:run/case",
            "--protocol", "simple",
            "read",
            "--ref", "entry:one",
            "--neighbors", "4",
        ])

        _, _, neighbor_payload = operation_request(
            neighbor_args,
            {"session_id": "session:s1"},
        )
        self.assertEqual(neighbor_payload["requests"], [{
            "ref": "entry:one",
            "view": "full",
            "max_chars": 20_000,
        }])

    def test_filesystem_asset_helper_refuses_symlink_outside_root_and_wrong_hash(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            corpus.mkdir()
            expected_bytes = b"authoritative asset bytes"
            expected_hash = hashlib.sha256(expected_bytes).hexdigest()
            (corpus / "asset.bin").write_bytes(expected_bytes)
            (corpus / "link.bin").symlink_to(corpus / "asset.bin")
            (corpus / "wrong.bin").write_bytes(b"tampered bytes")
            (root / "outside.bin").write_bytes(expected_bytes)
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({
                "corpus_root": str(corpus),
                "assets": [
                    {
                        "path": logical_path,
                        "content_hash": f"sha256:{expected_hash}",
                        "size_bytes": len(expected_bytes),
                    }
                    for logical_path in (
                        "asset.bin",
                        "link.bin",
                        "wrong.bin",
                        "../outside.bin",
                    )
                ],
            }), encoding="utf-8")
            trace = root / "trace.jsonl"
            base = [
                sys.executable,
                str(ROOT / "filesystem_asset.py"),
                "--manifest",
                str(manifest),
                "--trace",
                str(trace),
            ]
            attempts = {
                "symlink": [*base, "metadata", "link.bin"],
                "outside": [*base, "metadata", "../outside.bin"],
                "wrong_hash": [*base, "fetch", "wrong.bin"],
            }

            for label, command in attempts.items():
                with self.subTest(label=label):
                    result = subprocess.run(
                        command,
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    self.assertNotEqual(result.returncode, 0, result.stdout)

            self.assertFalse(trace.exists())

    def test_local_inspection_helper_refuses_symlink_outside_root_and_wrong_hash(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            run_root = root / "run"
            corpus.mkdir()
            run_root.mkdir()
            expected_bytes = b"authoritative asset bytes"
            expected_hash = hashlib.sha256(expected_bytes).hexdigest()
            expected = corpus / "asset.bin"
            expected.write_bytes(expected_bytes)
            symlink = run_root / "link.bin"
            symlink.symlink_to(expected)
            outside = root / "outside.bin"
            outside.write_bytes(expected_bytes)
            wrong = run_root / "wrong.bin"
            wrong.write_bytes(b"tampered bytes")
            manifest = run_root / "manifest.json"
            manifest.write_text(json.dumps({
                "corpus_root": str(corpus),
                "assets": [{
                    "path": "asset.bin",
                    "content_hash": f"sha256:{expected_hash}",
                    "size_bytes": len(expected_bytes),
                }],
            }), encoding="utf-8")
            trace = run_root / "trace.jsonl"
            base = [
                sys.executable,
                str(ROOT / "local_asset_inspect.py"),
                "--manifest",
                str(manifest),
                "--trace",
                str(trace),
                "--run-root",
                str(run_root),
            ]

            for label, target in {
                "symlink": symlink,
                "outside": outside,
                "wrong_hash": wrong,
            }.items():
                with self.subTest(label=label):
                    result = subprocess.run(
                        [*base, str(target)],
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    self.assertNotEqual(result.returncode, 0, result.stdout)

            self.assertFalse(trace.exists())

    def test_verified_download_privately_reuses_only_exact_existing_bytes(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            destination = Path(temporary) / "asset.bin"
            digest = hashlib.sha256(handler.asset_bytes).hexdigest()
            client = NativeApiClient(url, "case-token")

            first = client.download_verified(
                "/asset.bin",
                destination,
                expected_hash=f"sha256:{digest}",
                expected_size=len(handler.asset_bytes),
            )
            second = client.download_verified(
                "/asset.bin",
                destination,
                expected_hash=f"sha256:{digest}",
                expected_size=len(handler.asset_bytes),
            )

            self.assertEqual(first.content_hash, second.content_hash)
            self.assertEqual(destination.read_bytes(), handler.asset_bytes)
            self.assertEqual(destination.stat().st_mode & 0o777, 0o600)
            self.assertEqual(
                [request["path"] for request in handler.requests].count("/asset.bin"),
                1,
            )

            destination.chmod(0o644)
            with self.assertRaises(PermissionError):
                client.download_verified(
                    "/asset.bin",
                    destination,
                    expected_hash=f"sha256:{digest}",
                    expected_size=len(handler.asset_bytes),
                )

    def test_persisted_provisioning_requires_exact_requested_scope_and_provenance(self):
        valid = {
            "authorization_scope": "scope:root",
            "requested_authorization_scope": "eval:run-a/case-a",
            "token": "one-time-secret",
            "credential_provenance": "service_issued_case_scope",
            "provisioning": {
                "import_response": {
                    "requested_authorization_scope": "eval:run-a/case-a",
                }
            },
        }
        self.assertTrue(
            provisioning_matches_run_case(
                valid,
                run_id="run-a",
                case_id="case-a",
            )
        )
        self.assertFalse(
            provisioning_matches_run_case(
                {**valid, "requested_authorization_scope": "eval:run-a/case-b"},
                run_id="run-a",
                case_id="case-a",
            )
        )
        self.assertFalse(
            provisioning_matches_run_case(
                valid,
                run_id="run-a",
                case_id="case-b",
            )
        )
        self.assertFalse(
            provisioning_matches_run_case(
                {**valid, "credential_provenance": None},
                run_id="run-a",
                case_id="case-a",
            )
        )

    def test_continuation_projection_keeps_delta_content_without_corpus_samples(self):
        response = NativeResponse(
            body={
                "request_id": "request:1",
                "session_id": "session:s1",
                "corpus_revision": "revision:delta",
                "status": "complete",
                "data": {
                    "session_id": "session:s1",
                    "resume_checkpoint": {"checkpoint_ref": "checkpoint:seed"},
                    "revision_delta": {
                        "self_contained": True,
                        "source_changes": [{"source_ref": "delta.md", "content": "changed"}],
                    },
                    "initial_evidence": [],
                    "corpus_map": {
                        "record_counts": {"document": 2},
                        "available_views": ["full"],
                        "records": {"document": [{"ref": "document:noise"}]},
                        "truncated": True,
                    },
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("resume", response))
        self.assertEqual(
            rendered["data"]["revision_delta"]["source_changes"][0]["content"],
            "changed",
        )
        self.assertNotIn("records", rendered["data"]["corpus_map"])

    def test_one_shot_open_uses_the_same_compact_reasoning_projection(self):
        response = NativeResponse(
            body={
                "request_id": "request:1",
                "session_id": "session:s1",
                "corpus_revision": "revision:1",
                "status": "complete",
                "freshness": {
                    "source_updated_at": "2026-07-22T00:00:00Z",
                    "normalized_at": "2026-07-22T00:00:01Z",
                },
                "coverage": {
                    "searched": [{
                        "lane": "lexical",
                        "completeness": "best_effort",
                        "candidate_count": 12,
                        "searched_count": 1000,
                        "index_revision": "revision:1",
                    }],
                    "unsearched": [],
                    "absence_safe": False,
                },
                "data": {
                    "session_id": "session:s1",
                    "corpus_revision": "revision:1",
                    "corpus_map": {
                        "record_counts": {"chunk": 1000},
                        "records": {"chunk": [{"ref": "chunk:noise"}]},
                        "truncated": True,
                    },
                    "initial_evidence": [{
                        "reference": "chunk:1",
                        "path": "Source.md",
                        "content": "Exact evidence.",
                        "evidence_refs": ["evidence:1"],
                        "content_hash": "sha256:noise",
                        "lane_scores": {"lexical": 10},
                        "score": 0.4,
                    }],
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("open", response))
        self.assertNotIn("records", rendered["data"]["corpus_map"])
        candidate = rendered["data"]["initial_evidence"][0]
        self.assertEqual(candidate["content"], "Exact evidence.")
        self.assertEqual(candidate["evidence_refs"], ["evidence:1"])
        self.assertNotIn("content_hash", candidate)
        self.assertNotIn("lane_scores", candidate)
        self.assertEqual(
            rendered["coverage"]["searched"],
            [{
                "lane": "lexical",
                "completeness": "best_effort",
                "candidate_count": 12,
            }],
        )

    def test_query_projection_removes_flattened_candidate_duplicate(self):
        candidate = {
            "reference": "chunk:1",
            "source_ref": "Source.md",
            "path": "Source.md",
            "content": "Exact evidence.",
            "evidence_refs": ["evidence:1"],
            "content_hash": "sha256:noise",
            "source_version": "sha256:source-version",
            "why_selected": ["semantic_recall"],
        }
        response = NativeResponse(
            body={
                "status": "complete",
                "data": {
                    "results": [candidate],
                    "items": [{"id": "q0", "status": "complete", "results": [candidate]}],
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("query", response))
        self.assertNotIn("results", rendered["data"])
        self.assertEqual(len(rendered["data"]["items"]), 1)
        compact_candidate = rendered["data"]["items"][0]["results"][0]
        self.assertEqual(compact_candidate["reference"], "chunk:1")
        self.assertEqual(compact_candidate["evidence_refs"], ["evidence:1"])
        self.assertNotIn("source_ref", compact_candidate)
        self.assertNotIn("source_version", compact_candidate)
        self.assertNotIn("why_selected", compact_candidate)

    def test_simple_query_projection_preserves_search_excerpts(self):
        response = NativeResponse(
            body={
                "status": "complete",
                "data": {
                    "workspace_generation": 9,
                    "results": [{
                        "id": "q0",
                        "query_status": "complete",
                        "candidates": [{
                            "reference": "entry:1",
                            "path": "Projects/Warmind/Final handoff.md",
                            "title": "Final handoff",
                            "version": 3,
                            "heading": "Current target",
                            "excerpt": "The exact current target is commit abc123 on master.",
                            "additional_sections": [{
                                "heading": "Prior target",
                                "excerpt": "The previous branch is historical.",
                            }],
                        }],
                    }],
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("query", response))
        self.assertEqual(
            rendered["data"]["items"][0]["results"][0],
            {
                "reference": "entry:1",
                "path": "Projects/Warmind/Final handoff.md",
                "title": "Final handoff",
                "version": 3,
                "heading": "Current target",
                "excerpt": "The exact current target is commit abc123 on master.",
                "additional_sections": [{
                    "heading": "Prior target",
                    "excerpt": "The previous branch is historical.",
                }],
            },
        )

    def test_open_projection_returns_complete_sources_and_pointer_tail(self):
        response = NativeResponse(
            body={
                "status": "complete",
                "data": {
                    "hydrated_sources": [
                        {
                            "source_ref": f"Source-{index}.md",
                            "path": f"Source-{index}.md",
                            "text": f"Evidence {index}",
                            "complete": index < 4,
                            "ranges": [{"start_line": 1, "end_line": 2}],
                            "selected_references": [f"chunk:{index}"],
                        }
                        for index in range(13)
                    ],
                    "initial_evidence": [
                        {
                            "source_ref": f"Source-{index}.md",
                            "path": f"Source-{index}.md",
                            "evidence_refs": [f"evidence:{index}"],
                        }
                        for index in range(13)
                    ],
                    "retrieval_sufficiency": {
                        "status": "likely_sufficient",
                        "complete_source_count": 4,
                    },
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered_text = render_native_response("open", response)
        rendered = json.loads(rendered_text)
        self.assertNotIn("\n", rendered_text)
        self.assertEqual(len(rendered["data"]["initial_evidence"]), 12)
        self.assertEqual(
            rendered["data"]["initial_evidence"][0]["content_scope"],
            "complete_source",
        )
        complete_source = rendered["data"]["initial_evidence"][0]
        self.assertEqual(complete_source["path"], "Source-0.md")
        self.assertEqual(complete_source["evidence_refs"], ["evidence:0"])
        self.assertNotIn("source_ref", complete_source)
        self.assertNotIn("reference", complete_source)
        self.assertNotIn("ranges", complete_source)
        self.assertNotIn("complete", complete_source)
        self.assertEqual(len(rendered["data"]["evidence_leads"]), 1)
        self.assertEqual(
            rendered["data"]["evidence_leads"][0]["content_scope"],
            "source_lead",
        )
        self.assertEqual(
            rendered["data"]["evidence_leads"][0]["evidence_refs"],
            ["evidence:12"],
        )
        self.assertNotIn("content", rendered["data"]["evidence_leads"][0])

    def test_simple_open_projection_preserves_pointer_leads(self):
        response = NativeResponse(
            body={
                "status": "complete",
                "data": {
                    "workspace_generation": 9,
                    "evidence": [{
                        "reference": "entry:1",
                        "path": "Trips/Current.md",
                        "title": "Current trip",
                        "version": 2,
                        "representation": "complete_source",
                        "text": "Current exact evidence.",
                    }],
                    "evidence_leads": [{
                        "reference": "entry:2",
                        "path": "Trips/Related.md",
                        "title": "Related trip",
                        "version": 1,
                        "annotation": "changed since checkpoint: version 1 → 2",
                        "delta_omitted_reason": "delta character budget",
                    }],
                    "resume_deltas": [{
                        "path": "Trips/Current.md",
                        "pinned_version": 1,
                        "pinned_sha256": f"sha256:{'a' * 64}",
                        "current_version": 2,
                        "current_sha256": f"sha256:{'b' * 64}",
                        "mode": "whole_pair",
                        "before": "Old plan.",
                        "after": "Current plan.",
                    }],
                    "changes_since_checkpoint": [{
                        "generation": 201,
                        "path": "Trips/Current.md",
                    }],
                    "changes_truncated": True,
                    "next_changes_generation": 201,
                    "checkpoint_text_truncated": False,
                    "retrieval_sufficiency": {
                        "status": "bounded_evidence",
                        "complete_source_count": 1,
                        "selected_source_count": 1,
                        "pointer_source_count": 1,
                    },
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("open", response))
        self.assertEqual(
            rendered["data"]["initial_evidence"][0]["content"],
            "Current exact evidence.",
        )
        self.assertEqual(
            rendered["data"]["evidence_leads"],
            [{
                "reference": "entry:2",
                "path": "Trips/Related.md",
                "title": "Related trip",
                "version": 1,
                "annotation": "changed since checkpoint: version 1 → 2",
                "delta_omitted_reason": "delta character budget",
            }],
        )
        self.assertEqual(
            rendered["data"]["resume_deltas"][0]["after"],
            "Current plan.",
        )
        self.assertTrue(rendered["data"]["changes_truncated"])
        self.assertEqual(rendered["data"]["next_changes_generation"], 201)
        self.assertFalse(rendered["data"]["checkpoint_text_truncated"])

    def test_open_projection_applies_total_evidence_text_budget(self):
        response = NativeResponse(
            body={
                "status": "complete",
                "data": {
                    "hydrated_sources": [
                        {
                            "source_ref": f"Large-{index}.md",
                            "text": character * 20_000,
                            "complete": True,
                        }
                        for index, character in enumerate(("a", "b"))
                    ],
                    "retrieval_sufficiency": {"status": "likely_sufficient"},
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("open", response))
        self.assertEqual(len(rendered["data"]["initial_evidence"]), 1)
        self.assertEqual(len(rendered["data"]["evidence_leads"]), 1)
        self.assertEqual(
            rendered["data"]["retrieval_sufficiency"]["pointer_only_sources"],
            1,
        )

    def test_response_metrics_separate_evidence_text_from_transport_metadata(self):
        rendered = json.dumps({
            "status": "complete",
            "data": {
                "initial_evidence": [{
                    "content": "Exact evidence.",
                    "content_scope": "complete_source",
                }],
                "evidence_leads": [{
                    "source_ref": "Later.md",
                    "content_scope": "source_lead",
                }],
                "retrieval_sufficiency": {"status": "likely_sufficient"},
            },
        }, separators=(",", ":"))
        metrics = response_payload_metrics(rendered)
        self.assertEqual(metrics["source_text_chars"], len("Exact evidence."))
        self.assertEqual(metrics["metadata_chars"], len(rendered) - len("Exact evidence."))
        self.assertEqual(metrics["evidence_items"], 1)
        self.assertEqual(metrics["complete_sources"], 1)
        self.assertEqual(metrics["pointer_sources"], 1)
        self.assertEqual(metrics["sufficiency_status"], "likely_sufficient")

    def test_read_projection_preserves_text_lines_and_continuation(self):
        response = NativeResponse(
            body={
                "status": "partial",
                "data": {
                    "items": [{
                        "reference": "Source.md",
                        "view": "range",
                        "status": "partial",
                        "data": {
                            "metadata": {
                                "ref": "document:1",
                                "source_ref": "Source.md",
                                "source_version": "v2",
                                "content_hash": "sha256:noise",
                                "native_locator": {"path": "Source.md"},
                            },
                            "range": {
                                "start_byte": 0,
                                "end_byte_exclusive": 12,
                                "start_line": 1,
                                "end_line": 2,
                            },
                            "text": "Exact text.",
                            "instruction_boundary": "evidence",
                        },
                        "error": None,
                        "truncation": {
                            "truncated": True,
                            "returned_tokens": 3,
                            "limit_tokens": 3,
                            "continuation_token": "opaque",
                        },
                    }],
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        item = json.loads(render_native_response("read", response))["data"]["items"][0]
        self.assertEqual(item["text"], "Exact text.")
        self.assertEqual(item["lines"], [1, 2])
        self.assertEqual(
            item["truncation"],
            {"truncated": True, "continuation_token": "opaque"},
        )
        self.assertNotIn("content_hash", json.dumps(item))
        self.assertNotIn("start_byte", json.dumps(item))

    def test_simple_partial_read_preserves_valid_items_and_missing_path(self):
        response = NativeResponse(
            body={
                "status": "partial",
                "gaps": [{
                    "kind": "read_entries_not_found",
                    "missing_requests": 1,
                }],
                "data": {
                    "requested_count": 2,
                    "missing_requests": 1,
                    "response_truncated": False,
                    "items": [
                        {
                            "reference": "entry:one",
                            "path": "one.md",
                            "view": "full",
                            "text": "Exact text.",
                        },
                        {
                            "path": "missing.md",
                            "status": "not_found",
                            "error": {
                                "code": "entry_not_found",
                                "message": "the requested reference was not found",
                            },
                        },
                    ],
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )

        rendered = json.loads(render_native_response("read", response))

        self.assertEqual(rendered["status"], "partial")
        self.assertEqual(rendered["data"]["requested_count"], 2)
        self.assertEqual(rendered["data"]["missing_requests"], 1)
        self.assertEqual(rendered["data"]["items"][0]["text"], "Exact text.")
        self.assertEqual(rendered["data"]["items"][1]["path"], "missing.md")
        self.assertEqual(
            rendered["data"]["items"][1]["error"]["code"],
            "entry_not_found",
        )

    def test_checkpoint_projection_keeps_lineage_and_compacts_receipt(self):
        response = NativeResponse(
            body={
                "session_id": "session:1",
                "corpus_revision": "revision:2",
                "status": "committed",
                "data": {
                    "checkpoint_id": "checkpoint:2",
                    "base_corpus_revision": "revision:1",
                    "resulting_corpus_revision": "revision:2",
                    "receipt": "commit:1",
                    "request_hash": "sha256:noise",
                    "credential_id": "credential:noise",
                    "items": [{
                        "details": {
                            "parent_checkpoint_id": "checkpoint:1",
                            "source_refs": ["source:1"],
                        },
                    }],
                },
            },
            http_status=200,
            elapsed_ms=1.0,
            headers={},
        )
        rendered = json.loads(render_native_response("checkpoint", response))
        self.assertEqual(rendered["data"]["checkpoint_id"], "checkpoint:2")
        self.assertEqual(rendered["data"]["parent_checkpoint_id"], "checkpoint:1")
        self.assertEqual(rendered["data"]["source_refs"], ["source:1"])
        self.assertNotIn("request_hash", rendered["data"])
        self.assertNotIn("credential_id", rendered["data"])

    def test_provision_retries_transient_embedding_dependency_failure(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server(import_failures=1) as (url, handler):
            corpus = Path(temporary) / "corpus"
            corpus.mkdir()
            (corpus / "one.md").write_text("# One\n\nEvidence.\n", encoding="utf-8")
            client = NativeApiClient(url, "admin-token", run_id="run", case_id="case")
            metadata = provision_evaluation(
                client,
                run_id="run",
                case_id="case",
                display_scope="Alpha",
                access_mode="read_only",
                documents=text_documents(corpus),
                timeout_seconds=2,
                dependency_retry_seconds=0.01,
            )

            self.assertEqual(metadata["token"], "case-token")
            imports = [item for item in handler.requests if item["path"] == "/v1/admin/eval/import"]
            self.assertEqual(len(imports), 2)

    def test_provision_fails_closed_without_distinct_exact_scoped_credential(self):
        scenarios = (
            (
                {"issued_credential_token": None},
                "did not issue a case-scoped credential",
            ),
            (
                {"issued_credential_token": "admin-token"},
                "parent/admin credential",
            ),
            (
                {"issued_authorization_scope": "__missing__"},
                "effective scope",
            ),
        )
        with tempfile.TemporaryDirectory() as temporary:
            corpus = Path(temporary) / "corpus"
            corpus.mkdir()
            (corpus / "one.md").write_text("# One\n\nEvidence.\n", encoding="utf-8")
            documents = text_documents(corpus)
            for server_options, expected_error in scenarios:
                with self.subTest(server_options=server_options):
                    with fake_server(**server_options) as (url, _):
                        client = NativeApiClient(
                            url,
                            "admin-token",
                            run_id="run",
                            case_id="case",
                        )
                        with self.assertRaisesRegex(RuntimeError, expected_error):
                            provision_evaluation(
                                client,
                                run_id="run",
                                case_id="case",
                                display_scope="Alpha",
                                access_mode="read_only",
                                documents=documents,
                                timeout_seconds=2,
                            )

    def test_provision_streams_text_waits_for_indexes_and_redacts_token(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            corpus = Path(temporary) / "corpus"
            corpus.mkdir()
            (corpus / "one.md").write_text("# One\n\nEvidence.\n", encoding="utf-8")
            (corpus / "two.json").write_text('{"state":"ready"}\n', encoding="utf-8")
            (corpus / "ignored.bin").write_bytes(b"\x00\xff")
            client = NativeApiClient(url, "admin-token", run_id="run 1", case_id="case 1")
            metadata = provision_evaluation(
                client,
                run_id="run 1",
                case_id="case 1",
                display_scope="Alpha Scope",
                access_mode="read_only",
                documents=text_documents(corpus),
                timeout_seconds=2,
            )

            self.assertEqual(metadata["token"], "case-token")
            self.assertEqual(metadata["corpus_revision"], "revision:delta")
            self.assertEqual(metadata["authorization_scope"], "eval:run-1/case-1")
            self.assertEqual(
                metadata["requested_authorization_scope"],
                "eval:run-1/case-1",
            )
            self.assertEqual(metadata["provisioning"]["documents"], 2)
            public = public_provisioning(metadata)
            self.assertNotIn("token", json.dumps(public).casefold())
            imported = next(item for item in handler.requests if item["path"] == "/v1/admin/eval/import")
            self.assertEqual(imported["authorization"], "Bearer admin-token")
            self.assertEqual(imported["run"], "run 1")
            self.assertEqual(imported["case"], "case 1")
            self.assertEqual([item["path"] for item in imported["body"]["documents"]], ["one.md", "two.json"])
            self.assertTrue(all(item["content_sha256"] for item in imported["body"]["documents"]))
            status = next(
                item
                for item in handler.requests
                if item["path"] == "/v1/admin/eval/imports/import:1"
            )
            self.assertEqual(status["authorization"], "Bearer case-token")

    def test_provision_can_return_as_soon_as_exact_and_lexical_indexes_are_ready(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server(
            import_lexical_status="ready",
        ) as (url, handler):
            corpus = Path(temporary) / "corpus"
            corpus.mkdir()
            (corpus / "one.md").write_text("# One\n\nEvidence.\n", encoding="utf-8")
            client = NativeApiClient(url, "admin-token", run_id="run", case_id="case")

            metadata = provision_evaluation(
                client,
                run_id="run",
                case_id="case",
                display_scope="Scale test",
                access_mode="read_write",
                documents=text_documents(corpus),
                timeout_seconds=2,
                wait_for_semantic=False,
            )

            self.assertFalse(metadata["provisioning"]["semantic_waited"])
            self.assertFalse(metadata["provisioning"]["semantic_ready_at_start"])
            self.assertEqual(
                metadata["provisioning"]["index_status_at_start"]["lexical"],
                "ready",
            )
            self.assertFalse(any(item["method"] == "GET" for item in handler.requests))

    def test_provision_batches_large_simple_imports_under_one_credential(self):
        with fake_server(import_lexical_status="ready") as (url, handler):
            client = NativeApiClient(
                url,
                "admin-token",
                run_id="run",
                case_id="case",
            )
            documents = [
                {
                    "path": f"Fixture/{index}.md",
                    "content": f"# Fixture {index}\n",
                    "content_sha256": hashlib.sha256(
                        f"# Fixture {index}\n".encode(),
                    ).hexdigest(),
                    "media_type": "text/markdown",
                }
                for index in range(5)
            ]
            metadata = provision_evaluation(
                client,
                run_id="run",
                case_id="case",
                display_scope="Scale test",
                access_mode="read_write",
                documents=documents,
                timeout_seconds=2,
                wait_for_semantic=False,
                batch_size=2,
            )

            imports = [
                item for item in handler.requests
                if item["path"] == "/v1/admin/eval/import"
            ]
            self.assertEqual(len(imports), 3)
            self.assertEqual(
                [len(item["body"]["documents"]) for item in imports],
                [2, 2, 1],
            )
            self.assertEqual(
                [item["body"]["batch_index"] for item in imports],
                [0, 1, 2],
            )
            self.assertTrue(all(
                item["body"]["batch_count"] == 3 for item in imports
            ))
            self.assertEqual(metadata["token"], "case-token")
            self.assertEqual(metadata["provisioning"]["import_batch_count"], 3)
            self.assertEqual(metadata["provisioning"]["max_batch_documents"], 2)

    def test_native_memory_supports_batched_operations_and_records_metrics(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            root = Path(temporary)
            state = root / "state.json"
            task = root / "task.txt"
            task.write_text("Advance Alpha.\n", encoding="utf-8")
            base = [
                sys.executable,
                str(ROOT / "native_memory.py"),
                "--state", str(state),
                "--task-file", str(task),
                "--scope", "Alpha",
                "--authorization-scope", "eval:run/case",
                "--protocol", "legacy",
                "--checkpoint-id", "checkpoint:seed",
                "--run-id", "run",
                "--case-id", "case",
            ]
            env = {**os.environ, "STRAYLIGHT_API_URL": url, "STRAYLIGHT_EVAL_TOKEN": "case-token"}
            commands = [
                ["open"],
                ["query", "--scope", "Alpha", "--goal", "continue", "--limit", "4",
                 "--batch", "alpha", "beta"],
                ["read", json.dumps({"requests": [
                    {"ref": "source:one", "view": "full"},
                    {"ref": "source:two", "view": "outline"},
                ]})],
                ["compute", json.dumps({"steps": [{"id": "s1", "op": "catalog", "input": {}}]})],
                ["verify", json.dumps({"claims": [{"id": "c1", "claim": "Alpha is ready"}]})],
                ["checkpoint", "--scope", "Alpha", "--json", json.dumps({
                    "objective": "Continue", "current_state": ["Ready"], "next_actions": ["Ship"],
                    "source_refs": ["prior.md", "delta.md"],
                })],
                ["status"],
            ]
            for command in commands:
                result = subprocess.run([*base, *command], env=env, text=True, capture_output=True, check=False)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

            saved = json.loads(state.read_text())
            self.assertEqual([item["operation"] for item in saved["operations"]], [
                "open", "query", "read", "compute", "verify", "checkpoint", "status",
            ])
            self.assertEqual(saved["session_id"], "session:s1")
            self.assertEqual(saved["checkpoint_id"], "checkpoint:child")
            self.assertTrue(all(item["result_chars"] > 0 for item in saved["operations"]))
            self.assertTrue(all(item["elapsed_ms"] >= 0 for item in saved["operations"]))
            self.assertTrue(all("source_text_chars" in item for item in saved["operations"]))
            self.assertTrue(all("metadata_chars" in item for item in saved["operations"]))
            self.assertNotIn("case-token", state.read_text())
            query = next(item for item in handler.requests if item["path"] == "/v1/memory/query")
            self.assertEqual(len(query["body"]["queries"]), 2)
            self.assertEqual(
                [item["query"] for item in query["body"]["queries"]],
                ["alpha", "Alpha: beta"],
            )
            self.assertTrue(all(item["scope"] == {
                "authorization_scope": "eval:run/case",
                "root_refs": [],
            } for item in query["body"]["queries"]))
            self.assertTrue(all("modes" not in item for item in query["body"]["queries"]))
            self.assertEqual(query["body"]["session_id"], "session:s1")

            neighbor = subprocess.run(
                [*base, "read", "--ref", "chunk:one", "--view", "neighbors",
                 "--before", "1", "--after", "8"],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(neighbor.returncode, 0, neighbor.stdout + neighbor.stderr)
            neighbor_request = [
                item for item in handler.requests if item["path"] == "/v1/memory/read"
            ][-1]
            self.assertEqual(neighbor_request["body"]["requests"], [{
                "ref": "chunk:one",
                "view": "neighbors",
                "max_chars": 20_000,
                "before": 1,
                "after": 8,
            }])

            natural_neighbor = subprocess.run(
                [*base, "read", "--ref", "chunk:one", "--path", "one.md", "--neighbors", "4"],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(
                natural_neighbor.returncode,
                0,
                natural_neighbor.stdout + natural_neighbor.stderr,
            )
            natural_request = [
                item for item in handler.requests if item["path"] == "/v1/memory/read"
            ][-1]
            self.assertEqual(natural_request["body"]["requests"], [
                {
                    "ref": "chunk:one",
                    "view": "neighbors",
                    "max_chars": 20_000,
                    "before": 4,
                    "after": 4,
                },
                {"ref": "one.md", "view": "full", "max_chars": 20_000},
            ])

            natural_range = subprocess.run(
                [*base, "read", "--path", "one.md", "--range", "1:140"],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(natural_range.returncode, 0, natural_range.stdout + natural_range.stderr)
            range_request = [
                item for item in handler.requests if item["path"] == "/v1/memory/read"
            ][-1]
            self.assertEqual(range_request["body"]["requests"], [{
                "ref": "one.md",
                "view": "range",
                "max_chars": 20_000,
                "start": 1,
                "end": 140,
            }])

            multiple = subprocess.run(
                [*base, "read", "--path", "one.md", "--path", "two.md"],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(multiple.returncode, 0, multiple.stdout + multiple.stderr)
            multiple_request = [
                item for item in handler.requests if item["path"] == "/v1/memory/read"
            ][-1]
            self.assertEqual(
                [item["ref"] for item in multiple_request["body"]["requests"]],
                ["one.md", "two.md"],
            )

    def test_native_memory_repeated_resume_is_local_and_not_counted(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            root = Path(temporary)
            state = root / "state.json"
            task = root / "task.txt"
            task.write_text("Continue Alpha.\n", encoding="utf-8")
            command = [
                sys.executable,
                str(ROOT / "native_memory.py"),
                "--state", str(state),
                "--task-file", str(task),
                "--scope", "Alpha",
                "--authorization-scope", "eval:run/case",
                "--protocol", "legacy",
                "--checkpoint-id", "checkpoint:seed",
                "--run-id", "run",
                "--case-id", "case",
                "resume",
            ]
            env = {**os.environ, "STRAYLIGHT_API_URL": url, "STRAYLIGHT_EVAL_TOKEN": "case-token"}
            first = subprocess.run(command, env=env, text=True, capture_output=True, check=False)
            second = subprocess.run(command, env=env, text=True, capture_output=True, check=False)
            self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
            self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
            self.assertEqual(json.loads(second.stdout)["status"], "already_open")
            saved = json.loads(state.read_text())
            self.assertEqual([item["operation"] for item in saved["operations"]], ["resume"])
            opens = [item for item in handler.requests if item["path"] == "/v1/memory/open"]
            self.assertEqual(len(opens), 1)

    def test_native_memory_serializes_open_and_query_started_together(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server(open_delay=0.25) as (url, handler):
            root = Path(temporary)
            state = root / "state.json"
            task = root / "task.txt"
            task.write_text("Advance Alpha.\n", encoding="utf-8")
            base = [
                sys.executable,
                str(ROOT / "native_memory.py"),
                "--state", str(state),
                "--task-file", str(task),
                "--scope", "Alpha",
                "--authorization-scope", "eval:run/case",
                "--protocol", "legacy",
                "--run-id", "run",
                "--case-id", "case",
            ]
            env = {**os.environ, "STRAYLIGHT_API_URL": url, "STRAYLIGHT_EVAL_TOKEN": "case-token"}
            opened = subprocess.Popen(
                [*base, "open"], env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
            )
            queried = subprocess.Popen(
                [*base, "query", "alpha"],
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            open_stdout, open_stderr = opened.communicate(timeout=3)
            query_stdout, query_stderr = queried.communicate(timeout=3)

            self.assertEqual(opened.returncode, 0, open_stdout + open_stderr)
            self.assertEqual(queried.returncode, 0, query_stdout + query_stderr)
            saved = json.loads(state.read_text())
            self.assertEqual([item["operation"] for item in saved["operations"]], ["open", "query"])
            self.assertEqual([item["path"] for item in handler.requests], [
                "/v1/memory/open", "/v1/memory/query",
            ])
            self.assertEqual(handler.requests[1]["body"]["session_id"], "session:s1")

    def test_native_memory_records_read_only_denial_without_mutating_checkpoint(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server(deny_checkpoint=True) as (url, _):
            root = Path(temporary)
            state = root / "state.json"
            task = root / "task.txt"
            task.write_text("Read only.\n", encoding="utf-8")
            base = [
                sys.executable, str(ROOT / "native_memory.py"),
                "--state", str(state), "--task-file", str(task),
                "--scope", "Alpha", "--authorization-scope", "eval:run/read-only",
                "--protocol", "legacy",
                "--run-id", "run", "--case-id", "read-only",
            ]
            env = {**os.environ, "STRAYLIGHT_API_URL": url, "STRAYLIGHT_EVAL_TOKEN": "read-token"}
            opened = subprocess.run([*base, "open"], env=env, text=True, capture_output=True, check=False)
            self.assertEqual(opened.returncode, 0)
            denied = subprocess.run(
                [*base, "checkpoint", "--objective", "must not persist"],
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(denied.returncode, 77)
            self.assertEqual(json.loads(denied.stdout)["error"]["code"], "capability_denied")
            saved = json.loads(state.read_text())
            self.assertIsNone(saved["checkpoint"])
            self.assertEqual(saved["operations"][-1]["operation"], "denied:checkpoint")

    def test_generated_wrapper_exposes_no_corpus_path_or_secret(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            write_native_memory_wrapper(
                run_dir,
                task="Inspect Alpha",
                display_scope="Alpha",
                authorization_scope="eval:run/case",
                run_id="run",
                case_id="case",
            )
            wrapper = (run_dir / "memory").read_text()
            self.assertNotIn("--corpus", wrapper)
            self.assertNotIn("STRAYLIGHT_EVAL_TOKEN", wrapper)
            self.assertNotIn("secret", wrapper.casefold())
            self.assertIn("native_memory.py", wrapper)

    def test_filesystem_native_shortcuts_do_not_change_manifest_defaults(self):
        manifest = {"conditions": ["fixed_pack", "filesystem", "workspace"]}
        selected = select_conditions(manifest, SimpleNamespace(filesystem_native=True, condition=None))
        self.assertEqual(selected, ["filesystem", "service_api"])
        self.assertEqual(manifest["conditions"], ["fixed_pack", "filesystem", "workspace"])

        transition_manifest = {"conditions": ["filesystem_rebuild", "workspace_resume"]}
        transition = select_transition_conditions(
            transition_manifest,
            SimpleNamespace(filesystem_native=True, condition=None),
        )
        self.assertEqual(transition, ["filesystem_rebuild", "service_api_resume"])
        self.assertEqual(transition_manifest["conditions"], ["filesystem_rebuild", "workspace_resume"])

    def test_native_query_defaults_to_bounded_eight_result_packet(self):
        parser = build_parser()
        args = parser.parse_args([
            "--state", "/tmp/state.json",
            "--task-file", "/tmp/task.txt",
            "--scope", "Alpha",
            "--authorization-scope", "eval:run/case",
            "query", "find evidence",
        ])
        self.assertEqual(args.limit, 8)

    def test_native_transition_enables_network_only_for_service_resume(self):
        common = {
            "codex": Path("/tmp/codex"),
            "model": "gpt-test",
            "schema": ROOT / "eval" / "agent_answer.schema.json",
            "run_dir": Path("/tmp/run"),
        }
        native = build_transition_codex_command(
            **common,
            condition="service_api_resume",
        )
        filesystem = build_transition_codex_command(
            **common,
            condition="filesystem_rebuild",
        )
        self.assertIn("sandbox_workspace_write.network_access=true", native)
        self.assertNotIn("sandbox_workspace_write.network_access=true", filesystem)
        self.assertIn("workspace-write", native)

    def test_transition_filesystem_sidecar_requires_linked_child_checkpoint(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            (run_dir / "sidecar").mkdir()
            checkpoint = {
                "parent_checkpoint_id": "checkpoint:parent",
                "corpus_revision": "revision:delta",
                "source_refs": ["prior.md", "delta.md"],
                "state": {
                    "objective": "Advance safely",
                    "current_state": ["Delta incorporated"],
                    "decisions": [],
                    "open_questions": [],
                    "next_actions": ["Verify"],
                    "artifacts": ["delta.md"],
                },
            }
            (run_dir / "sidecar" / "checkpoint.json").write_text(
                json.dumps(checkpoint) + "\n",
                encoding="utf-8",
            )
            record = {
                "answer_path": str(run_dir / "answer.json"),
                "grade": {"pass": True},
            }
            case = {"delta_path": "delta.md"}
            metadata = {
                "seed_checkpoint": {
                    "checkpoint_id": "checkpoint:parent",
                    "source_refs": ["prior.md"],
                },
                "delta_revision": {"revision_id": "revision:delta"},
            }
            attach_filesystem_sidecar_lineage(record, case, metadata)
            self.assertTrue(record["transition_pass"])
            self.assertEqual(record["persisted_checkpoint"], checkpoint)
            self.assertTrue(record["lineage"]["state_valid"])

            del checkpoint["state"]["artifacts"]
            (run_dir / "sidecar" / "checkpoint.json").write_text(
                json.dumps(checkpoint) + "\n",
                encoding="utf-8",
            )
            invalid_record = {
                "answer_path": str(run_dir / "answer.json"),
                "grade": {"pass": True},
            }
            attach_filesystem_sidecar_lineage(
                invalid_record,
                case,
                metadata,
            )
            self.assertFalse(invalid_record["transition_pass"])
            self.assertIsNone(invalid_record["persisted_checkpoint"])
            self.assertFalse(invalid_record["lineage"]["state_valid"])

    def test_e06_mutation_manifest_is_valid_and_exercises_both_delta_modes(self):
        validated = validate_transition(
            ROOT / "eval" / "transition_cases.json",
            mutation_script=ROOT / "eval" / "e06_mutate.py",
            mutation_seed="e06-draw1",
        )

        self.assertEqual(validated["errors"], [])
        self.assertEqual(len(validated["mutation_plans"]), 5)
        for case in validated["manifest"]["cases"]:
            plan = validated["mutation_plans"][case["id"]]
            self.assertEqual(len(plan["source_refs"]), 3)
            self.assertEqual(case["delta_path"], plan["source_refs"][0])
            self.assertIn("whole_pair", case["mutation"]["modes_exercised"])
            self.assertTrue(any(
                target["before_chars"] > 2_400 for target in plan["targets"]
            ))
            self.assertTrue(any(
                target["before_chars"] <= 2_400
                and target["after_chars"] <= 2_400
                for target in plan["targets"]
            ))

    def test_e06_mutation_hook_writes_exactly_three_sources_and_mirrors_bytes(self):
        script = ROOT / "eval" / "e06_mutate.py"
        base_root = ROOT / "eval" / "corpus-v0.2"
        plan = run_mutation_hook(
            script,
            "plan",
            case_id="straylight-api-gate-transition",
            base_root=base_root,
            seed="e06-draw1",
        )
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            handler.workspace_sources = {
                target["path"]: {
                    "text": target["before"],
                    "version": 1,
                    "content_hash": target["before_sha256"],
                }
                for target in plan["targets"]
            }
            environment = {
                **os.environ,
                "STRAYLIGHT_API_URL": url,
                "STRAYLIGHT_EVAL_TOKEN": "case-token",
                "STRAYLIGHT_EVAL_RUN": "e06-a-draw1",
            }
            receipt = run_mutation_hook(
                script,
                "apply",
                case_id="straylight-api-gate-transition",
                base_root=base_root,
                seed="e06-draw1",
                mirror_root=Path(temporary),
                environment=environment,
            )

            writes = [
                request for request in handler.requests
                if request["path"] == "/v1/workspace/write"
            ]
            reads = [
                request for request in handler.requests
                if request["path"] == "/v1/workspace/read"
            ]
            self.assertEqual(len(writes), 3)
            self.assertEqual(len(reads), 2)
            self.assertTrue(receipt["exactly_three_sources"])
            self.assertTrue(receipt["workspace_vault_byte_identical"])
            self.assertEqual(receipt["corpus_revision"], "generation:43")
            for target in plan["targets"]:
                mirrored = (
                    Path(temporary) / target["path"]
                ).read_text(encoding="utf-8")
                self.assertEqual(mirrored, target["after"])
                self.assertEqual(
                    handler.workspace_sources[target["path"]]["text"],
                    mirrored,
                )

    def test_transition_cli_accepts_documented_e06_mutation_options(self):
        args = build_transition_parser().parse_args([
            "validate",
            "--mutation-script",
            "eval/e06_mutate.py",
            "--mutation-seed",
            "e06-draw2",
        ])

        self.assertEqual(args.command, "validate")
        self.assertEqual(args.mutation_seed, "e06-draw2")

    def test_native_transition_reads_child_checkpoint_over_http(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            old_url = os.environ.get("STRAYLIGHT_API_URL")
            os.environ["STRAYLIGHT_API_URL"] = url
            try:
                handler.child = {
                    "checkpoint_id": "checkpoint:aaaaaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa",
                    "parent_checkpoint_id": "checkpoint:bbbbbbbb-bbbb-7bbb-8bbb-bbbbbbbbbbbb",
                    "corpus_revision": "revision:cccccccc-cccc-7ccc-8ccc-cccccccccccc",
                    "source_refs": ["source:prior", "source:delta"],
                    "sources": [
                        {"ref": "source:prior", "locator": {"ref": "prior.md"}},
                        {"ref": "source:delta", "locator": {"ref": "delta.md"}},
                    ],
                }
                handler.session_revision = "revision:dddddddd-dddd-7ddd-8ddd-dddddddddddd"
                run_dir = Path(temporary)
                (run_dir / "native-session.json").write_text(json.dumps({
                    "session_id": "session:s1",
                    "checkpoint_id": handler.child["checkpoint_id"],
                    "checkpoint": handler.child,
                    "operations": [
                        {"operation": "resume", "result_chars": 100, "elapsed_ms": 2.5},
                        {"operation": "checkpoint", "result_chars": 80, "elapsed_ms": 3.5},
                    ],
                }))
                record = {
                    "answer_path": str(run_dir / "answer.json"),
                    "condition": "service_api_resume",
                    "grade": {"pass": True},
                    "run_id": "run",
                }
                case = {"id": "case", "delta_path": "delta.md"}
                metadata = {
                    "token": "case-token",
                    "checkpoint_id": "checkpoint:bbbbbbbbbbbb7bbb8bbbbbbbbbbbbbbb",
                    "corpus_revision": "revision:dddddddddddd7ddd8ddddddddddddddd",
                    "seed_source_refs": ["prior.md"],
                }
                attach_native_lineage(record, case, metadata, max_calls=4)
                self.assertTrue(record["transition_pass"])
                self.assertTrue(record["lineage"]["checkpoint_read_via_http"])
                self.assertTrue(record["lineage"]["parent_match"])
                self.assertEqual(record["service_calls"], 2)
                self.assertTrue(any(item["path"] == "/v1/sessions/session:s1" for item in handler.requests))
            finally:
                if old_url is None:
                    os.environ.pop("STRAYLIGHT_API_URL", None)
                else:
                    os.environ["STRAYLIGHT_API_URL"] = old_url

    def test_simple_native_transition_reads_durable_checkpoint_entry(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            old_url = os.environ.get("STRAYLIGHT_API_URL")
            os.environ["STRAYLIGHT_API_URL"] = url
            try:
                handler.child = {
                    "checkpoint_id": "checkpoint:aaaaaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa",
                    "parent_checkpoint_id": "checkpoint:bbbbbbbb-bbbb-7bbb-8bbb-bbbbbbbbbbbb",
                    "corpus_revision": "generation:42",
                    "source_refs": ["prior.md", "delta.md"],
                }
                run_dir = Path(temporary)
                (run_dir / "native-session.json").write_text(json.dumps({
                    "session_id": "session:s1",
                    "checkpoint_id": handler.child["checkpoint_id"],
                    "checkpoint": handler.child,
                    "corpus_revision": "generation:43",
                    "operations": [
                        {"operation": "resume", "result_chars": 100, "elapsed_ms": 2.5},
                        {"operation": "checkpoint", "result_chars": 80, "elapsed_ms": 3.5},
                    ],
                }))
                record = {
                    "answer_path": str(run_dir / "answer.json"),
                    "condition": "service_api_resume",
                    "grade": {"pass": True},
                    "run_id": "run",
                }
                case = {"id": "case", "delta_path": "delta.md"}
                metadata = {
                    "token": "case-token",
                    "checkpoint_id": "checkpoint:bbbbbbbbbbbb7bbb8bbbbbbbbbbbbbbb",
                    "corpus_revision": "generation:42",
                    "seed_source_refs": ["prior.md"],
                }

                attach_native_lineage(
                    record,
                    case,
                    metadata,
                    max_calls=4,
                    service_protocol="simple",
                )

                self.assertTrue(record["transition_pass"])
                self.assertTrue(record["lineage"]["checkpoint_read_via_http"])
                self.assertTrue(record["lineage"]["parent_match"])
                self.assertTrue(record["lineage"]["revision_match"])
                self.assertTrue(record["lineage"]["prior_source_preserved"])
                self.assertTrue(record["lineage"]["delta_source_preserved"])
                self.assertTrue(any(
                    item["path"] == "/v1/workspace/read"
                    for item in handler.requests
                ))
                self.assertFalse(any(
                    item["path"].startswith("/v1/sessions/")
                    for item in handler.requests
                ))
            finally:
                if old_url is None:
                    os.environ.pop("STRAYLIGHT_API_URL", None)
                else:
                    os.environ["STRAYLIGHT_API_URL"] = old_url

    def test_native_transition_resolves_evidence_refs_to_source_paths(self):
        with tempfile.TemporaryDirectory() as temporary, fake_server() as (url, handler):
            old_url = os.environ.get("STRAYLIGHT_API_URL")
            os.environ["STRAYLIGHT_API_URL"] = url
            try:
                prior_ref = "source:11111111-1111-7111-8111-111111111111"
                delta_ref = "evidence:22222222-2222-7222-8222-222222222222"
                handler.evidence_sources = {
                    prior_ref: "prior.md",
                    delta_ref: "delta.md",
                }
                handler.child = {
                    "checkpoint_id": "checkpoint:aaaaaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa",
                    "parent_checkpoint_id": "checkpoint:bbbbbbbb-bbbb-7bbb-8bbb-bbbbbbbbbbbb",
                    "corpus_revision": "revision:cccccccc-cccc-7ccc-8ccc-cccccccccccc",
                    "source_refs": [prior_ref, delta_ref],
                }
                handler.session_revision = "revision:dddddddd-dddd-7ddd-8ddd-dddddddddddd"
                run_dir = Path(temporary)
                (run_dir / "native-session.json").write_text(json.dumps({
                    "session_id": "session:s1",
                    "checkpoint_id": handler.child["checkpoint_id"],
                    "checkpoint": handler.child,
                    "operations": [
                        {"operation": "resume", "result_chars": 100, "elapsed_ms": 2.5},
                        {"operation": "checkpoint", "result_chars": 80, "elapsed_ms": 3.5},
                    ],
                }))
                record = {
                    "answer_path": str(run_dir / "answer.json"),
                    "condition": "service_api_resume",
                    "grade": {"pass": True},
                    "run_id": "run",
                }
                case = {"id": "case", "delta_path": "delta.md"}
                metadata = {
                    "token": "case-token",
                    "checkpoint_id": "checkpoint:bbbbbbbbbbbb7bbb8bbbbbbbbbbbbbbb",
                    "corpus_revision": "revision:dddddddddddd7ddd8ddddddddddddddd",
                    "seed_source_refs": ["prior.md"],
                }
                attach_native_lineage(record, case, metadata, max_calls=4)
                self.assertTrue(record["transition_pass"])
                self.assertIsNone(
                    record["lineage"]["provenance_resolution_error"]
                )
                self.assertTrue(record["lineage"]["prior_source_preserved"])
                self.assertTrue(record["lineage"]["delta_source_preserved"])
                self.assertTrue(any(
                    item["path"] == "/v1/memory/read"
                    for item in handler.requests
                ))
            finally:
                if old_url is None:
                    os.environ.pop("STRAYLIGHT_API_URL", None)
                else:
                    os.environ["STRAYLIGHT_API_URL"] = old_url


if __name__ == "__main__":
    unittest.main()

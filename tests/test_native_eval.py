from __future__ import annotations

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
from native_memory import render_native_response
from transition_eval import (
    attach_native_lineage,
    build_codex_command as build_transition_codex_command,
    select_transition_conditions,
)


ROOT = Path(__file__).resolve().parents[1]


class FakeNativeHandler(BaseHTTPRequestHandler):
    requests: list[dict[str, Any]] = []
    deny_checkpoint = False
    open_delay = 0.0
    import_failures = 0
    session_revision = "revision:delta"
    child = {
        "checkpoint_id": "checkpoint:child",
        "parent_checkpoint_id": "checkpoint:seed",
        "corpus_revision": "revision:delta",
        "source_refs": ["prior.md", "delta.md"],
    }

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
        if self.path == "/v1/admin/eval/imports/import:1":
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
            self.send_json(200, {
                "status": "accepted_processing",
                "data": {
                    "import_id": "import:1",
                    "status_url": "/v1/admin/eval/imports/import:1",
                    "authorization_scope": body["authorization_scope"],
                    "credential_token": "case-token",
                    "checkpoint_id": "checkpoint:seed" if body.get("seed_checkpoint") else None,
                    "base_corpus_revision": "revision:base",
                    "corpus_revision": "revision:delta",
                    "index_status": {"exact": "ready", "lexical": "building", "semantic": "building"},
                },
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
    *, deny_checkpoint: bool = False, open_delay: float = 0.0, import_failures: int = 0
) -> Iterator[tuple[str, type[FakeNativeHandler]]]:
    handler = type("ConfiguredFakeNativeHandler", (FakeNativeHandler,), {})
    handler.requests = []
    handler.deny_checkpoint = deny_checkpoint
    handler.open_delay = open_delay
    handler.import_failures = import_failures
    handler.session_revision = FakeNativeHandler.session_revision
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
    def test_persisted_provisioning_validates_requested_not_effective_scope(self):
        metadata = {
            "authorization_scope": "scope:root",
            "token": "one-time-secret",
            "provisioning": {
                "import_response": {
                    "requested_authorization_scope": "eval:run-a/case-a",
                }
            },
        }
        self.assertTrue(
            provisioning_matches_run_case(
                metadata,
                run_id="run-a",
                case_id="case-a",
            )
        )
        self.assertFalse(
            provisioning_matches_run_case(
                metadata,
                run_id="run-a",
                case_id="case-b",
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
            self.assertEqual(metadata["provisioning"]["documents"], 2)
            public = public_provisioning(metadata)
            self.assertNotIn("token", json.dumps(public).casefold())
            imported = next(item for item in handler.requests if item["path"] == "/v1/admin/eval/import")
            self.assertEqual(imported["authorization"], "Bearer admin-token")
            self.assertEqual(imported["run"], "run 1")
            self.assertEqual(imported["case"], "case 1")
            self.assertEqual([item["path"] for item in imported["body"]["documents"]], ["one.md", "two.json"])
            self.assertTrue(all(item["content_sha256"] for item in imported["body"]["documents"]))

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


if __name__ == "__main__":
    unittest.main()

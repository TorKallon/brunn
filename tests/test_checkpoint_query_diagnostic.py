import argparse
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from eval.checkpoint_query_diagnostic import (
    CORRELATION_SCHEMA,
    MIN_SAMPLES,
    RUN_SCHEMA,
    correlate_log,
    event_sql,
    normalize_sql,
    run_diagnostic,
    runtime_snapshot,
    validate_run_args,
    write_new_artifact,
)
from native_eval import NativeResponse


REVISION = "a" * 40
ADMIN_SECRET = "admin-secret-that-must-not-appear"
SCOPED_SECRET = "scoped-secret-that-must-not-appear"


def response(
    body,
    *,
    headers=None,
    status=200,
    elapsed_ms=1.0,
):
    return NativeResponse(
        body=body,
        http_status=status,
        elapsed_ms=elapsed_ms,
        headers=headers or {},
    )


class FakeAdmin:
    base_url = "http://127.0.0.1:18110"
    token = ADMIN_SECRET

    def get(self, path):
        if path != "/ready":
            raise AssertionError(path)
        return response(
            {
                "status": "ready",
                "data": {
                    "build_revision": REVISION,
                    "runtime_features": {"semantic_lane": True},
                    "embedding_provider": "openai",
                    "embedding_model": "mock-embedding",
                    "dependencies": {
                        "database": "ready",
                        "object_store": "ready",
                        "embeddings": "ready",
                    },
                },
            }
        )


class FakeScoped:
    def __init__(self):
        self.base_url = FakeAdmin.base_url
        self.token = SCOPED_SECRET
        self.checkpoint_calls = 0

    def get(self, path):
        if path != "/v1/admin/eval/imports/test":
            raise AssertionError(path)
        return response(
            {
                "status": "ready",
                "data": {
                    "index_status": {
                        "exact": "ready",
                        "lexical": "ready",
                        "semantic": "ready",
                    }
                },
            }
        )

    def post(self, path, payload):
        if path != "/v1/workspace/checkpoint":
            raise AssertionError(path)
        index = self.checkpoint_calls
        self.checkpoint_calls += 1
        document_path = payload["state"]["artifacts"][0]
        return response(
            {
                "status": "committed",
                "query_count": 29,
                "data": {
                    "checkpoint_ref": f"checkpoint:{index:032x}",
                    "source_entries": [{"path": document_path}],
                    "write": {"no_op": False},
                },
            },
            headers={"x-request-id": f"req-checkpoint-{index:04d}"},
        )


def args(**overrides):
    values = {
        "run_id": "checkpoint-query-unit",
        "runtime_posture": "semantic-ready",
        "expect_build_revision": REVISION,
        "samples": MIN_SAMPLES,
        "timeout": 30.0,
        "provision_timeout": 30.0,
        "poll_seconds": 0.01,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def provision(*_args, **_kwargs):
    return {
        "authorization_scope": "scope:unit",
        "access_mode": "read_write",
        "status_url": "/v1/admin/eval/imports/test",
        "token": SCOPED_SECRET,
        "credential_provenance": "service_issued_case_scope",
        "provisioning": {
            "documents": 1,
            "delta_documents": 0,
            "import_http_status": 200,
            "import_elapsed_ms": 1.0,
            "import_batch_count": 1,
            "max_batch_documents": 1,
            "index_status_at_start": {
                "exact": "ready",
                "lexical": "ready",
                "semantic": "pending",
            },
        },
    }


def client_factory(*_args, token=None, **_kwargs):
    return FakeScoped() if token else FakeAdmin()


def cleanup(_scoped, *, status_url):
    return {
        "attempted": True,
        "status_url_sha256": "b" * 64,
        "credential_revocation_verified": True,
        "pass": status_url == "/v1/admin/eval/imports/test",
    }


def provenance():
    return {
        "script": "eval/checkpoint_query_diagnostic.py",
        "script_sha256": "c" * 64,
        "git_revision": "d" * 40,
        "git_tree": "e" * 40,
        "tracked_tree_clean": True,
        "pass": True,
    }


def synthetic_run_artifact(query_count=3):
    return {
        "schema": RUN_SCHEMA,
        "pass": True,
        "samples": [
            {
                "sample_index": index,
                "x_request_id": f"req-checkpoint-{index:04d}",
                "query_count": query_count,
                "pass": True,
            }
            for index in range(MIN_SAMPLES)
        ],
    }


def sqlx_event(request_id, statement):
    return {
        "target": "sqlx::query",
        "fields": {
            "message": " ".join(statement.split()[:4]),
            "db.statement": statement,
        },
        "span": {
            "name": "http_request",
            "request_id": request_id,
        },
    }


class CheckpointQueryDiagnosticTest(unittest.TestCase):
    def test_runtime_snapshot_accepts_exact_mode1_pending_posture(self):
        class Mode1Ready(FakeAdmin):
            def get(self, path):
                ready = super().get(path)
                data = ready.body["data"]
                data["runtime_features"]["semantic_lane"] = False
                data["embedding_provider"] = "hashing"
                data["embedding_model"] = "straylight-hashing-v1"
                data["dependencies"]["embeddings"] = "degraded"
                return ready

        snapshot = runtime_snapshot(
            Mode1Ready(),
            expected_revision=REVISION,
            runtime_posture="mode1-pending",
        )
        self.assertTrue(snapshot["pass"])
        self.assertEqual(snapshot["embedding_provider"], "hashing")
        self.assertEqual(snapshot["embedding_status"], "degraded")

    def test_runtime_snapshot_rejects_wrong_mode1_provider_model_or_status(self):
        class Mode1Ready(FakeAdmin):
            def __init__(self, field, value):
                self.field = field
                self.value = value

            def get(self, path):
                ready = super().get(path)
                data = ready.body["data"]
                data["runtime_features"]["semantic_lane"] = False
                data["embedding_provider"] = "hashing"
                data["embedding_model"] = "straylight-hashing-v1"
                data["dependencies"]["embeddings"] = "degraded"
                if self.field == "embeddings":
                    data["dependencies"]["embeddings"] = self.value
                else:
                    data[self.field] = self.value
                return ready

        for field, value in (
            ("embedding_provider", "openai"),
            ("embedding_model", "wrong-model"),
            ("embeddings", "ready"),
        ):
            with self.subTest(field=field):
                with self.assertRaisesRegex(RuntimeError, "runtime posture"):
                    runtime_snapshot(
                        Mode1Ready(field, value),
                        expected_revision=REVISION,
                        runtime_posture="mode1-pending",
                    )

    def test_event_sql_uses_summary_when_statement_is_empty(self):
        self.assertEqual(
            event_sql(
                {
                    "fields": {
                        "db.statement": "",
                        "summary": "SELECT   set_config('statement_timeout', $1, true)",
                        "message": "query completed",
                    }
                }
            ),
            "SELECT set_config('statement_timeout', $1, true)",
        )

    def test_runtime_snapshot_rejects_degraded_embedding_dependency(self):
        class DegradedReady(FakeAdmin):
            def get(self, path):
                ready = super().get(path)
                ready.body["data"]["dependencies"]["embeddings"] = "degraded"
                return ready

        with self.assertRaisesRegex(RuntimeError, "runtime posture"):
            runtime_snapshot(
                DegradedReady(),
                expected_revision=REVISION,
                runtime_posture="semantic-ready",
            )

    def test_run_records_thirty_fresh_samples_and_no_credentials(self):
        artifact = run_diagnostic(
            args(),
            client_factory=client_factory,
            provision=provision,
            cleanup=cleanup,
            sleep=lambda _seconds: None,
            provenance=provenance,
        )
        self.assertTrue(artifact["pass"])
        self.assertEqual(len(artifact["samples"]), MIN_SAMPLES)
        self.assertEqual(
            artifact["query_count_distribution"]["histogram"],
            {"29": MIN_SAMPLES},
        )
        self.assertTrue(artifact["freshness"]["unique_request_ids"])
        self.assertTrue(artifact["freshness"]["unique_checkpoint_refs"])
        self.assertFalse(artifact["contract"]["query_budget_mutated"])
        self.assertTrue(artifact["implementation_provenance"]["pass"])
        rendered = json.dumps(artifact)
        self.assertNotIn(ADMIN_SECRET, rendered)
        self.assertNotIn(SCOPED_SECRET, rendered)
        self.assertNotIn('"token"', rendered)

    def test_run_fails_closed_when_cleanup_does_not_prove_revocation(self):
        artifact = run_diagnostic(
            args(),
            client_factory=client_factory,
            provision=provision,
            cleanup=lambda *_args, **_kwargs: {
                "attempted": True,
                "credential_revocation_verified": False,
                "pass": False,
            },
            sleep=lambda _seconds: None,
            provenance=provenance,
        )
        self.assertFalse(artifact["pass"])
        self.assertEqual(artifact["status"], "failed")

    def test_run_requires_at_least_thirty_samples(self):
        with self.assertRaisesRegex(ValueError, "--samples"):
            validate_run_args(args(samples=MIN_SAMPLES - 1))

    def test_correlation_preserves_full_commit_and_rollback_sequence(self):
        artifact = synthetic_run_artifact()
        lines = []
        for sample in artifact["samples"]:
            request_id = sample["x_request_id"]
            lines.extend(
                json.dumps(sqlx_event(request_id, statement))
                for statement in (
                    "SELECT set_config('statement_timeout', $1, true)",
                    "COMMIT",
                    "ROLLBACK",
                )
            )
        lines.append(
            json.dumps(sqlx_event("req-unrelated", "SELECT secret FROM elsewhere"))
        )
        artifact_bytes = json.dumps(artifact, indent=2).encode()
        correlated = correlate_log(
            artifact,
            ("\n".join(lines) + "\n").encode(),
            run_artifact_bytes=artifact_bytes,
        )
        self.assertEqual(correlated["schema"], CORRELATION_SCHEMA)
        self.assertTrue(correlated["pass"])
        self.assertEqual(len(correlated["sequence_groups"]), 1)
        statements = correlated["sequence_groups"][0]["statements"]
        self.assertEqual(
            [item["normalized_sql"] for item in statements],
            [
                "SELECT set_config('statement_timeout', $1, true)",
                "COMMIT",
                "ROLLBACK",
            ],
        )
        self.assertEqual(
            correlated["source"]["unmatched_sqlx_events"],
            1,
        )
        self.assertEqual(
            correlated["source"]["run_artifact_sha256"],
            hashlib.sha256(artifact_bytes).hexdigest(),
        )

    def test_correlation_fails_when_log_count_differs_from_response(self):
        artifact = synthetic_run_artifact(query_count=2)
        lines = [
            json.dumps(
                sqlx_event(sample["x_request_id"], "COMMIT")
            )
            for sample in artifact["samples"]
        ]
        correlated = correlate_log(
            artifact,
            ("\n".join(lines) + "\n").encode(),
            run_artifact_bytes=json.dumps(artifact).encode(),
        )
        self.assertFalse(correlated["pass"])
        self.assertTrue(
            all(not item["pass"] for item in correlated["correlations"])
        )

    def test_normalization_and_artifact_write_are_immutable(self):
        self.assertEqual(normalize_sql("  SELECT   1 ;\n"), "SELECT 1")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "artifact.json"
            write_new_artifact(path, {"pass": True})
            with self.assertRaises(FileExistsError):
                write_new_artifact(path, {"pass": False})


if __name__ == "__main__":
    unittest.main()

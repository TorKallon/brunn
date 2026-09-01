import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from agent_work_eval import build_run_ledger  # noqa: E402
from eval.audit_accepted_sources import audit  # noqa: E402


REVISION = "a" * 40
IMAGE_ID = "sha256:" + "f" * 64


def accepted_source_payload():
    billing = {
        "route": "chatgpt_subscription",
        "api_fallback": "forbidden",
        "codex_path": "/opt/codex",
        "codex_version": "codex-test",
        "auth_checked_at": "2026-07-27T12:00:00-07:00",
        "auth_status": "Logged in using ChatGPT",
    }
    source = {
        "revision": REVISION,
        "tracked_source_clean": True,
        "untracked_source_files": [],
        "clean": True,
    }
    runtime_snapshot = {
        "schema": "brunn-service-runtime-snapshot@v1",
        "captured_at": "2026-07-27T12:00:30-07:00",
        "status": "ready",
        "build_revision": REVISION,
        "runtime_features": {},
    }
    image = {
        "schema": "brunn-service-image-fingerprint@v1",
        "api_container": "brunn-api",
        "api_container_id": "container-control-1",
        "api_container_started_at": "2026-07-27T12:00:20-07:00",
        "api_container_running": True,
        "api_image_id": IMAGE_ID,
        "api_image_revision": REVISION,
    }
    image_provenance = {
        "schema": "brunn-service-image-provenance@v1",
        "stable": True,
        "before": image,
        "after": dict(image),
    }
    experiment_parameters = {
        "service_retrieval_modes": ["exact", "lexical"],
        "service_image_fingerprint": image,
    }
    with patch("agent_work_eval.git_source_fingerprint", return_value=source):
        ledger = build_run_ledger(
            run_id="e04-control-run-1",
            reasoning_billing=billing,
            model="gpt-test",
            conditions=["service_api"],
            service_protocol="simple",
            manifest_sha256="b" * 64,
            schema_sha256="c" * 64,
            harness_sha256="d" * 64,
            experiment_arm="control",
            paired_draw_id="e04-draw-1",
            runtime_snapshot=runtime_snapshot,
            expected_runtime_features={},
            expected_build_revision=REVISION,
            experiment_parameters=experiment_parameters,
            service_image_provenance=image_provenance,
        )
    metrics = {
        "schema": "brunn-agent-response-character-metrics@v1",
        "service_result_chars": 130,
        "service_source_text_chars": 100,
        "service_metadata_chars": 30,
        "service_replay_weighted_chars": 130,
        "model_visible_tool_output_chars": 100,
    }
    return {
        "benchmark_version": "e04-test-v1",
        "run_id": "e04-control-run-1",
        "experiment_arm": "control",
        "paired_draw_id": "e04-draw-1",
        "manifest_sha256": "b" * 64,
        "harness_sha256": "d" * 64,
        "service_protocol": "simple",
        "service_retrieval_modes": ["exact", "lexical"],
        "service_runtime_snapshot": runtime_snapshot,
        "service_image_provenance": image_provenance,
        "expected_runtime_features": {},
        "expected_build_revision": REVISION,
        "experiment_parameters": experiment_parameters,
        "manifest": {
            "benchmark_version": "e04-test-v1",
            "model": "gpt-test",
            "conditions": ["service_api"],
            "cases": [{
                "id": "case-a",
                "rubric": [{
                    "id": "c1",
                    "tags": [],
                    "sources_any": ["Evidence/accepted.md"],
                }, {
                    "id": "c2",
                    "tags": [],
                    "sources_any": ["Evidence/missing.md"],
                }],
                "forbidden": [],
            }],
        },
        "reasoning_billing": billing,
        "run_ledger": ledger,
        "records": [{
            "case_id": "case-a",
            "condition": "service_api",
            "grade": {
                "claims_passed": 0,
                "claims_total": 2,
                "pass": False,
                "claims": [
                    {
                        "id": "c1",
                        "pass": False,
                        "tags": [],
                        "exact_value": False,
                    },
                    {
                        "id": "c2",
                        "pass": False,
                        "tags": [],
                        "exact_value": False,
                    },
                ],
                "forbidden_hits": [],
            },
            "service_operations": [{
                "operation": "query",
                "source_paths": [
                    "./Evidence/accepted.md",
                    "Evidence/other.md",
                ],
            }],
            "response_character_metrics": metrics,
            **{
                key: value
                for key, value in metrics.items()
                if key != "schema"
            },
        }],
    }


class AcceptedSourceAuditTests(unittest.TestCase):
    def test_missed_claim_rate_uses_saved_returned_source_paths(self):
        payload = accepted_source_payload()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "run.json"
            path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(
                ValueError,
                "exactly cover every definitive service-backed arm",
            ):
                audit([path])
            result = audit(
                [path],
                expected_arm_retrieval_modes=[
                    "control=exact,lexical",
                ],
            )
        self.assertEqual(
            result["summary"]["missed_claims"],
            {
                "claims": 2,
                "accepted_source_in_context": 1,
                "rate": 0.5,
            },
        )
        self.assertEqual(
            result["summary"]["by_arm"]["control"]["missed_claims"]["rate"],
            0.5,
        )
        self.assertEqual(
            result["service_provenance"]["api_image_id"],
            IMAGE_ID,
        )
        self.assertEqual(
            result["input_artifacts"][0]["service_retrieval_modes"],
            ["exact", "lexical"],
        )

    def test_audit_detects_retrieval_mode_provenance_tampering(self):
        payload = accepted_source_payload()
        payload["service_retrieval_modes"] = ["exact", "semantic"]
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "tampered.json"
            path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(
                ValueError,
                "retrieval modes disagree",
            ):
                audit(
                    [path],
                    expected_arm_retrieval_modes=[
                        "control=exact,lexical",
                    ],
                )

    def test_missing_saved_source_path_metrics_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "invalid.json"
            path.write_text("{}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "immutable run_ledger"):
                audit([path])


if __name__ == "__main__":
    unittest.main()

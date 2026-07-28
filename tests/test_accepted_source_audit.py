import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from agent_work_eval import build_run_ledger  # noqa: E402
from eval.audit_accepted_sources import audit  # noqa: E402


class AcceptedSourceAuditTests(unittest.TestCase):
    def test_missed_claim_rate_uses_saved_returned_source_paths(self):
        billing = {
            "route": "chatgpt_subscription",
            "api_fallback": "forbidden",
            "codex_path": "/opt/codex",
            "codex_version": "codex-test",
            "auth_checked_at": "2026-07-27T12:00:00-07:00",
            "auth_status": "Logged in using ChatGPT",
        }
        source = {
            "revision": "a" * 40,
            "tracked_source_clean": True,
            "untracked_source_files": [],
            "clean": True,
        }
        runtime_snapshot = {
            "schema": "straylight-service-runtime-snapshot@v1",
            "captured_at": "2026-07-27T12:00:30-07:00",
            "status": "ready",
            "build_revision": "a" * 40,
            "runtime_features": {},
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
            )
        payload = {
            "benchmark_version": "e04-test-v1",
            "run_id": "e04-control-run-1",
            "experiment_arm": "control",
            "paired_draw_id": "e04-draw-1",
            "manifest_sha256": "b" * 64,
            "harness_sha256": "d" * 64,
            "service_runtime_snapshot": runtime_snapshot,
            "manifest": {
                "benchmark_version": "e04-test-v1",
                "model": "gpt-test",
                "conditions": ["service_api"],
                "cases": [{
                    "id": "case-a",
                    "rubric": [{
                        "id": "c1",
                        "sources_any": ["Evidence/accepted.md"],
                    }, {
                        "id": "c2",
                        "sources_any": ["Evidence/missing.md"],
                    }],
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
                        {"id": "c1", "pass": False},
                        {"id": "c2", "pass": False},
                    ],
                },
                "service_operations": [{
                    "operation": "query",
                    "source_paths": [
                        "./Evidence/accepted.md",
                        "Evidence/other.md",
                    ],
                }],
                "model_visible_tool_output_chars": 100,
            }],
        }
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "run.json"
            path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
            result = audit([path])
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

    def test_missing_saved_source_path_metrics_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "invalid.json"
            path.write_text("{}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "immutable run_ledger"):
                audit([path])


if __name__ == "__main__":
    unittest.main()

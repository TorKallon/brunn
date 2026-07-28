import argparse
import json
import shlex
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from eval.e03_mode2 import build_performance_command
from eval.e03_quality_backfill import (
    DEFAULT_MANIFESTS,
    DEFAULT_SCHEMA,
    actual_accounting,
    cost_preflight,
    load_suites,
    semantic_gap,
)
from eval.e09_step_policy import (
    DEFAULT_MANIFESTS as E09_MANIFESTS,
    load_manifest,
    step_plan,
)
from eval.semantic_http_probe import run_probe, source_text_contains
from native_eval import NativeResponse


class SemanticExperimentInfrastructureTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.suites = load_suites(DEFAULT_MANIFESTS, DEFAULT_SCHEMA)

    def test_quality_backfill_cost_covers_case_isolated_e09_imports(self) -> None:
        preflight = cost_preflight(
            self.suites,
            provider_mode="openai",
            usd_per_million_tokens=0.02,
            ceiling_usd=5.0,
        )
        self.assertTrue(preflight["ceiling_pass"])
        self.assertGreater(
            preflight["e09_isolated_provisioning_projection"][
                "estimated_input_tokens"
            ],
            preflight["quality_backfill"]["estimated_input_tokens"],
        )
        self.assertLess(preflight["projected_total_usd"], 5.0)

    def test_mock_backfill_actual_is_explicitly_zero(self) -> None:
        actual = actual_accounting(
            self.suites,
            provider_mode="mock",
            usd_per_million_tokens=0.02,
            billed_input_tokens=None,
            billed_usd=None,
        )
        self.assertEqual(actual["accounting_status"], "mock_provider_zero_spend")
        self.assertEqual(actual["accounted_usd"], 0.0)

    def test_semantic_gap_accepts_deadline_deferral_spelling(self) -> None:
        self.assertTrue(
            semantic_gap({"lane_failures": ["semantic_deferred"]})
        )
        self.assertFalse(semantic_gap({"candidates": [{"path": "ok.md"}]}))

    def test_http_probe_marker_must_come_from_source_text(self) -> None:
        self.assertFalse(
            source_text_contains(
                {"query": "terminal-marker", "candidates": []},
                "terminal-marker",
            )
        )
        self.assertTrue(
            source_text_contains(
                {"candidates": [{"excerpt": "terminal-marker"}]},
                "terminal-marker",
            )
        )

    def test_e09_step_is_one_bounded_step_under_ceiling(self) -> None:
        manifests = [load_manifest(path) for path in E09_MANIFESTS]
        plan = step_plan(
            manifests,
            losing_suite="recent_work",
            actual_base_usd=None,
            per_case_run_usd=0.24,
            ceiling_usd=100.0,
            max_step_cases=12,
        )
        self.assertEqual(plan["status"], "approved_one_step_bounded_subset")
        self.assertEqual(plan["step"]["selected_cases"], 12)
        self.assertLessEqual(plan["step"]["maximum_total_usd"], 100.0)
        self.assertFalse(plan["policy"]["automatic_1000ms_step_allowed"])
        self.assertIn(
            "owner_decision_required",
            plan["step"]["inferential_status"],
        )

    def test_mode2_orchestration_builds_distinct_failure_and_restore_hooks(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(
                mock_port=55321,
                mock_state=root / "mock.pid",
                mock_log=root / "mock.log",
                mock_config=root / "mock.json",
                label="mode2-unit",
                out=root / "result.json",
                failure_settle_seconds=0.1,
                quick=True,
                samples=3,
                future_soak=False,
                scales=[64_000],
                api_container=None,
                db_container=None,
                feature_state=[],
            )
            command = build_performance_command(args)
        failed = command[
            command.index("--semantic-failure-start-command") + 1
        ]
        restored = command[
            command.index("--semantic-failure-stop-command") + 1
        ]
        self.assertNotEqual(shlex.split(failed), shlex.split(restored))
        self.assertEqual(shlex.split(failed)[-1], "503")
        self.assertEqual(shlex.split(restored)[-1], "0")

    def test_slow_provider_probe_proves_full_http_deadline_and_warm_cache(
        self,
    ) -> None:
        class FakeClient:
            def __init__(self, *args, **kwargs):
                self.responses = [
                    NativeResponse(
                        body={"candidates": [{"path": "baseline.md"}]},
                        http_status=200,
                        elapsed_ms=20.0,
                        headers={},
                    ),
                    NativeResponse(
                        body={
                            "lane_failures": ["semantic_deferred"],
                            "candidates": [
                                {
                                    "path": "marker.md",
                                    "excerpt": "terminal-marker",
                                }
                            ],
                        },
                        http_status=200,
                        elapsed_ms=340.0,
                        headers={},
                    ),
                    NativeResponse(
                        body={
                            "candidates": [
                                {
                                    "path": "marker.md",
                                    "excerpt": "terminal-marker",
                                }
                            ]
                        },
                        http_status=200,
                        elapsed_ms=30.0,
                        headers={},
                    ),
                    NativeResponse(
                        body={"candidates": [{"path": "restored.md"}]},
                        http_status=200,
                        elapsed_ms=25.0,
                        headers={},
                    ),
                ]

            def get(self, path):
                self.assert_path = path
                return NativeResponse(
                    body={
                        "runtime_features": {
                            "semantic_lane": True,
                            "embed_cache": True,
                            "semantic_deadline_ms": 300,
                        },
                        "build_revision": "unit",
                    },
                    http_status=200,
                    elapsed_ms=1.0,
                    headers={},
                )

            def post(self, path, payload):
                return self.responses.pop(0)

        args = argparse.Namespace(
            query="locate current source",
            marker="terminal-marker",
            slow_command="/usr/bin/true slow",
            restore_command="/usr/bin/true restore",
            injected_delay_ms=800,
            deadline_ms=300,
            max_response_ms=750,
            settle_ms=0,
            warm_wait_ms=0,
            hook_timeout=2.0,
        )
        hook_results = [
            {"pass": True, "exit_code": 0},
            {"pass": True, "exit_code": 0},
        ]
        with (
            patch(
                "eval.semantic_http_probe.NativeApiClient",
                FakeClient,
            ),
            patch(
                "eval.semantic_http_probe.run_hook",
                side_effect=hook_results,
            ),
            patch("eval.semantic_http_probe.time.sleep"),
        ):
            result = run_probe(args)
        self.assertTrue(result["pass"])
        self.assertTrue(result["cold_deadline"]["semantic_failure"])
        self.assertTrue(result["warm_async_cache"]["pass"])


if __name__ == "__main__":
    unittest.main()

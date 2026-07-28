import argparse
import hashlib
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
    artifact_base,
    cost_preflight,
    load_suites,
    reconcile_receipt,
    semantic_gap,
)
from eval.e09_step_authorization import (
    canonical_json_sha256,
    load_step_authorization,
)
from eval.e09_step_policy import (
    DEFAULT_MANIFESTS as E09_MANIFESTS,
    load_manifest,
    step_plan,
)
from eval.semantic_http_probe import (
    cleanup_fixture,
    run_contract,
    run_probe,
    source_text_contains,
)
from native_eval import NativeApiError, NativeResponse


PROJECT_ROOT = Path(__file__).resolve().parents[1]


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

    def test_quality_receipt_reconciliation_is_immutable_and_hash_bound(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source_path = root / "run.json"
            receipt_path = root / "provider-receipt.json"
            args = argparse.Namespace(
                provider_mode="openai",
                usd_per_million_tokens=0.02,
                ceiling_usd=5.0,
                run_id="quality-run-one",
            )
            artifact = artifact_base(args, self.suites)
            artifact.update({
                "status": "complete",
                "pass": True,
                "errors": [],
                "backfills": [
                    {"warm_probe": {"pass": True}}
                    for _ in self.suites
                ],
            })
            artifact["cost"]["actual"] = actual_accounting(
                self.suites,
                provider_mode="openai",
                usd_per_million_tokens=0.02,
                billed_input_tokens=None,
                billed_usd=None,
            )
            source_path.write_text(
                json.dumps(artifact) + "\n",
                encoding="utf-8",
            )
            receipt_path.write_text(
                '{"provider":"openai","window":"unit"}\n',
                encoding="utf-8",
            )
            source_before = source_path.read_bytes()
            source_sha256 = hashlib.sha256(source_before).hexdigest()
            reconciled = reconcile_receipt(
                input_path=source_path,
                expected_input_sha256=source_sha256,
                run_id="quality-run-one",
                provider_receipt_path=receipt_path,
                billed_input_tokens=1_000,
                billed_usd=0.00002,
            )
            self.assertEqual(source_path.read_bytes(), source_before)
            self.assertTrue(reconciled["pass"])
            self.assertEqual(
                reconciled["reconciled_actual"]["accounting_status"],
                "provider_receipt_reconciled",
            )
            self.assertEqual(
                reconciled["provider_receipt"]["sha256"],
                hashlib.sha256(receipt_path.read_bytes()).hexdigest(),
            )
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                reconcile_receipt(
                    input_path=source_path,
                    expected_input_sha256="0" * 64,
                    run_id="quality-run-one",
                    provider_receipt_path=receipt_path,
                    billed_input_tokens=1_000,
                    billed_usd=0.00002,
                )
            with self.assertRaisesRegex(ValueError, "inconsistent"):
                reconcile_receipt(
                    input_path=source_path,
                    expected_input_sha256=source_sha256,
                    run_id="quality-run-one",
                    provider_receipt_path=receipt_path,
                    billed_input_tokens=1_000,
                    billed_usd=0.5,
                )

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
        for ceiling_usd, max_step_cases in ((100.01, 12), (100.0, 13)):
            with self.assertRaises(ValueError):
                step_plan(
                    manifests,
                    losing_suite="recent_work",
                    actual_base_usd=None,
                    per_case_run_usd=0.24,
                    ceiling_usd=ceiling_usd,
                    max_step_cases=max_step_cases,
                )

    def test_evaluation_cleanup_migration_is_scoped_and_revoking(self) -> None:
        migration = (
            PROJECT_ROOT
            / "apps/api/migrations/0056_atomic_evaluation_cleanup.sql"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "CREATE FUNCTION straylight_auth.cleanup_evaluation_user(",
            migration,
        )
        self.assertIn(
            "external_ref !~ '^eval-user:[0-9a-f]{64}$'",
            migration,
        )
        self.assertIn(
            "DELETE FROM straylight.search_chunks",
            migration,
        )
        self.assertIn(
            "UPDATE straylight.api_credentials",
            migration,
        )
        self.assertIn(
            "SET disabled_at = coalesce(credential.disabled_at, cleanup_time)",
            migration,
        )

    def test_e09_step_authorization_binds_source_manifest_and_cases(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "step.json"
            source_revision = "a" * 40
            manifest_sha256 = "b" * 64
            selected = ["case-a", "case-b"]
            artifact = {
                "schema": "straylight-e09-step-policy@v1",
                "status": "approved_one_step_bounded_subset",
                "pass": True,
                "policy": {
                    "one_step_only": True,
                    "deadline_before_ms": 300,
                    "deadline_after_ms": 600,
                    "automatic_1000ms_step_allowed": False,
                    "step_draws": 3,
                    "max_step_cases": 12,
                    "reasoning_ceiling_usd": 100.0,
                    "second_step_status": (
                        "forbidden_without_new_owner_authorization"
                    ),
                },
                "losing_suite": {
                    "manifest": "eval/synthetic_cases.json",
                    "manifest_sha256": manifest_sha256,
                },
                "step": {
                    "selected_case_ids": selected,
                    "selected_cases": 2,
                    "case_runs": 6,
                    "maximum_total_usd": 92.0,
                },
                "implementation": {
                    "source_revision": source_revision,
                    "source_clean": True,
                    "harness_sha256": "c" * 64,
                },
            }
            artifact["authorization_id"] = canonical_json_sha256({
                "schema": artifact["schema"],
                "source_revision": source_revision,
                "manifest": artifact["losing_suite"]["manifest"],
                "manifest_sha256": manifest_sha256,
                "selected_case_ids": selected,
                "deadline_before_ms": 300,
                "deadline_after_ms": 600,
                "maximum_total_usd": 92.0,
            })
            path.write_text(json.dumps(artifact) + "\n", encoding="utf-8")
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            binding = load_step_authorization(
                path,
                digest,
                expected_source_revision=source_revision,
                expected_manifest_path="eval/synthetic_cases.json",
                expected_manifest_sha256=manifest_sha256,
                expected_case_ids=selected,
            )
            self.assertEqual(binding["deadline_after_ms"], 600)
            self.assertFalse(binding["automatic_1000ms_step_allowed"])
            with self.assertRaisesRegex(ValueError, "selected cases"):
                load_step_authorization(
                    path,
                    digest,
                    expected_source_revision=source_revision,
                    expected_case_ids=["case-a"],
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
                expect_build_revision=None,
                feature_state=[],
                expect_feature_flag=[],
                expect_runtime_config=[],
                query_budget_profile="default-safe",
                query_budget_contract=None,
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
        self.assertIn("semantic_lane=on", command)
        self.assertIn("semantic_deadline_ms=null", command)

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
            slow_command="/usr/bin/true slow",
            restore_command="/usr/bin/true restore",
            injected_delay_ms=800,
            deadline_ms=300,
            max_response_ms=750,
            settle_ms=0,
            warm_wait_ms=0,
            hook_timeout=2.0,
            require_hook_attestation=False,
        )
        hook_results = [
            {"pass": True, "exit_code": 0},
            {"pass": True, "exit_code": 0},
        ]
        with (
            patch(
                "eval.semantic_http_probe.run_hook",
                side_effect=hook_results,
            ),
            patch("eval.semantic_http_probe.time.sleep"),
        ):
            result = run_contract(
                args,
                client=FakeClient(),
                query="locate current source",
                marker="terminal-marker",
            )
        self.assertTrue(result["pass"])
        self.assertTrue(result["cold_deadline"]["semantic_failure"])
        self.assertTrue(result["warm_async_cache"]["pass"])

    def test_probe_cleanup_requires_a_revoked_scoped_credential(self) -> None:
        class ScopedClient:
            def request(self, method, path):
                self.method = method
                self.path = path
                return NativeResponse(
                    body={
                        "status": "cleaned",
                        "import_id": "simple-import:unit",
                        "entries_removed": 1,
                        "search_chunks_removed": 2,
                        "jobs_removed": 1,
                        "credentials_revoked": 1,
                        "revoked_at": "2026-07-27T12:00:00-07:00",
                    },
                    http_status=200,
                    elapsed_ms=1.0,
                    headers={},
                )

            def get(self, path):
                raise NativeApiError(
                    401,
                    {"error": "unauthenticated"},
                )

        client = ScopedClient()
        receipt = cleanup_fixture(
            client,  # type: ignore[arg-type]
            status_url="/v1/workspace/admin/eval/imports/simple-import:unit",
        )
        self.assertTrue(receipt["pass"])
        self.assertTrue(receipt["credential_revocation_verified"])
        self.assertEqual(client.method, "DELETE")

    def test_probe_self_provisions_a_unique_fixture_without_recording_token(
        self,
    ) -> None:
        class FakeClient:
            def __init__(self, *args, **kwargs):
                self.base_url = "http://unit"

        captured: dict[str, object] = {}

        def provision(_client, **kwargs):
            captured.update(kwargs)
            return {
                "token": "must-not-be-recorded",
                "authorization_scope": "eval:unit/probe",
                "requested_authorization_scope": "eval:unit/probe",
                "status_url": "/v1/workspace/admin/eval/imports/simple-import:unit",
                "credential_provenance": "service_issued_case_scope",
                "provisioning": {},
            }

        args = argparse.Namespace(
            run_id="probe-run",
            query="locate current source",
            marker_prefix="terminal-marker",
            slow_command="/usr/bin/true slow",
            restore_command="/usr/bin/true restore",
            injected_delay_ms=800,
            deadline_ms=300,
            max_response_ms=750,
            settle_ms=0,
            warm_wait_ms=0,
            hook_timeout=2.0,
            provision_timeout=30.0,
            require_hook_attestation=True,
        )
        with (
            patch("eval.semantic_http_probe.NativeApiClient", FakeClient),
            patch(
                "eval.semantic_http_probe.provision_evaluation",
                side_effect=provision,
            ),
            patch(
                "eval.semantic_http_probe.run_contract",
                return_value={
                    "schema": "straylight-semantic-http-probe@v1",
                    "status": "passed",
                    "pass": True,
                    "error": None,
                },
            ),
            patch(
                "eval.semantic_http_probe.cleanup_fixture",
                return_value={"attempted": True, "pass": True},
            ),
        ):
            result = run_probe(args)
        self.assertTrue(result["pass"])
        self.assertEqual(captured["access_mode"], "read_write")
        document = captured["documents"][0]  # type: ignore[index]
        self.assertIn("terminal-marker-", document["content"])
        self.assertNotIn(
            "must-not-be-recorded",
            json.dumps(result),
        )


if __name__ == "__main__":
    unittest.main()

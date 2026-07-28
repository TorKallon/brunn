import hashlib
import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import ANY, patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from agent_work_eval import (  # noqa: E402
    CURRENT_RUNTIME_FEATURES as AGENT_CURRENT_RUNTIME_FEATURES,
)
from performance_eval import (  # noqa: E402
    BOOLEAN_RUNTIME_FEATURES,
    CONCURRENT_SEARCHES_PER_ROUND,
    CURRENT_RUNTIME_FEATURES,
    D03_QUERY_BUDGETS_PATH,
    D03_RESUME_DELTAS_GATE_PROFILE,
    D03_RESUME_QUERY_COUNT_DELTA,
    DEFAULT_THRESHOLDS,
    DEFINITIVE_SAMPLES,
    FUTURE_RECORDS,
    LEXICAL_CONSOLIDATION_GATE_PROFILE,
    LEXICAL_CONSOLIDATION_REQUIRED_GATES,
    PRODUCTION_RECORDS,
    QUERY_BUDGETS_PATH,
    RETRIEVAL_PLAN_CONTRACT_PATH,
    DatabaseSnapshot,
    apply_retrieval_plan_applicability,
    benchmark_flat_files,
    build_parser,
    candidate_audit,
    compare_results,
    capture_service_runtime_snapshot,
    counter_growth,
    evaluate_gates,
    evaluate_d03_resume_delta_gates,
    evaluate_query_budgets,
    evaluate_retrieval_plan,
    expected_query_count_sample_cardinality,
    extract_sql_function_body,
    load_query_budgets,
    load_reused_flat_controls,
    isolated_semantic_failure_probe,
    old_source_marker,
    parse_feature_states,
    expected_runtime_features,
    percentile,
    resolve_run_profile,
    resolve_query_budget_contract,
    retrieval_plan_assertions,
    response_character_metrics,
    response_reports_lane_failure,
    response_reports_gap_kind,
    semantic_failure_probe,
    source_text_contains,
    require_stable_runtime_configuration,
    response_timings,
    resume_delta_lineage_receipt,
    sql_fingerprint,
    summarize_query_counts,
    summarize_timing_samples,
    timing_phase_sum_sane,
    verify_service_feature_states,
    validate_e09_request_modes,
    validate_lexical_consolidation_request,
    validate_resume_delta_fixture_request,
    validate_d03_resume_control_compatibility,
    validate_semantic_failure_probe_posture,
    validate_verbatim_feature_acceptance_posture,
    lexical_overflow_marker,
    simple_checkpoint_footprint,
    summarize_response_accounting,
    synthetic_discovery_key,
    synthetic_discovery_task,
    synthetic_documents,
    verbatim_identifier_probe,
    table_growth,
)


def runtime_features(**overrides):
    values = {name: False for name in BOOLEAN_RUNTIME_FEATURES}
    values.update({
        "embed_cache": True,
        "embedding_backfill_guard": True,
        "observability_timings_ms": True,
        "embedding_backfill_batch_chunks": 64,
        "embedding_backfill_foreground_status_timeout_ms": 1_000,
        "embedding_backfill_inter_batch_ms": 250,
        "embedding_backfill_open_p95_limit_ms": 120,
        "embedding_backfill_search_p95_limit_ms": 107,
        "materialize_token_budget": 24_000,
        "search_section_demotion_top_n": None,
        "semantic_deadline_ms": 300,
        "supersession_demotion_weight": 1.5,
    })
    values.update(overrides)
    assert set(values) == set(CURRENT_RUNTIME_FEATURES)
    return values


class PerformanceEvalTests(unittest.TestCase):
    def test_runtime_feature_contract_matches_reasoning_harness(self):
        self.assertEqual(
            CURRENT_RUNTIME_FEATURES,
            AGENT_CURRENT_RUNTIME_FEATURES,
        )

    def test_feature_states_are_normalized_and_conflicts_fail(self):
        self.assertEqual(
            parse_feature_states([
                "lexical_single_scan=on",
                "read_path_roundtrip_v1=false",
            ]),
            {
                "lexical_single_scan": True,
                "read_path_roundtrip_v1": False,
            },
        )
        with self.assertRaisesRegex(ValueError, "conflicting"):
            parse_feature_states(["lexical_single_scan=on", "lexical_single_scan=off"])
        with self.assertRaisesRegex(ValueError, "unknown service feature"):
            parse_feature_states(["unreported_experiment=on"])

    def test_expected_runtime_features_cover_boolean_and_typed_controls(self):
        self.assertEqual(
            expected_runtime_features(
                ["resume_deltas=on", "semantic_lane=off"],
                [
                    "semantic_deadline_ms=null",
                    "search_section_demotion_top_n=8",
                    "supersession_demotion_weight=1.5",
                ],
            ),
            {
                "resume_deltas": True,
                "search_section_demotion_top_n": 8,
                "semantic_deadline_ms": None,
                "semantic_lane": False,
                "supersession_demotion_weight": 1.5,
            },
        )
        with self.assertRaisesRegex(ValueError, "duplicate"):
            expected_runtime_features(
                ["semantic_lane=off"],
                ["semantic_lane=false"],
            )

    def test_feature_state_preflight_matches_the_service_snapshot(self):
        class Response:
            data = {
                "status": "ready",
                "build_revision": "abc123",
                "runtime_features": runtime_features(
                    lexical_single_scan=True,
                    read_path_roundtrip_v1=False,
                ),
                "embeddings": {"provider": "hashing"},
            }

        class Client:
            def get(self, path):
                self.path = path
                return Response()

        client = Client()
        self.assertEqual(
            verify_service_feature_states(
                client,
                {
                    "lexical_single_scan": True,
                    "read_path_roundtrip_v1": False,
                },
            ),
            {
                "lexical_single_scan": True,
                "read_path_roundtrip_v1": False,
            },
        )
        self.assertEqual(client.path, "/v1/status")
        with self.assertRaisesRegex(ValueError, "mismatch"):
            verify_service_feature_states(
                client,
                {"lexical_single_scan": False},
            )

    def test_runtime_snapshot_checks_all_features_build_and_stability(self):
        status = {
            "status": "ready",
            "build_revision": "abc123",
            "runtime_features": runtime_features(resume_deltas=True),
            "embeddings": {"provider": "hashing", "status": "ready"},
            "semantic_runtime": {"requested": 0},
        }
        before = capture_service_runtime_snapshot(
            status,
            expected_features={"resume_deltas": True},
            expected_build_revision="abc123",
        )
        after = capture_service_runtime_snapshot(
            status,
            expected_features={"resume_deltas": True},
            expected_build_revision="abc123",
        )
        require_stable_runtime_configuration(before, after)

        drifted = {
            **after,
            "runtime_features": {
                **after["runtime_features"],
                "resume_deltas": False,
            },
        }
        with self.assertRaisesRegex(ValueError, "runtime_features drifted"):
            require_stable_runtime_configuration(before, drifted)
        incomplete = {
            **status,
            "runtime_features": {
                "resume_deltas": True,
            },
        }
        with self.assertRaisesRegex(ValueError, "incomplete"):
            capture_service_runtime_snapshot(
                incomplete,
                expected_features={},
                expected_build_revision="abc123",
            )

    def test_e05_gate_profile_uses_the_two_shipped_guard_names(self):
        self.assertIn(
            "bounded_lexical_overflow_returns_late_relevant_source",
            LEXICAL_CONSOLIDATION_REQUIRED_GATES,
        )
        self.assertIn(
            "old_relevant_source_survives_many_newer_writes",
            LEXICAL_CONSOLIDATION_REQUIRED_GATES,
        )
        self.assertIn(
            "query_count_sample_cardinality_is_authoritative",
            LEXICAL_CONSOLIDATION_REQUIRED_GATES,
        )
        self.assertIn(
            "verbatim_identifier_measurement_integrity",
            LEXICAL_CONSOLIDATION_REQUIRED_GATES,
        )
        self.assertNotIn(
            "verbatim_identifier",
            LEXICAL_CONSOLIDATION_REQUIRED_GATES,
        )

    def test_query_count_summary_and_budget_reject_one_extra_statement(self):
        budgets = {
            "schema": "straylight-query-budgets@v1",
            "operations": {
                "read": {"comparison": "exact", "count": 11},
                "search": {"comparison": "at_most", "max": 23},
            },
        }
        summary = summarize_query_counts([
            ("read", {"query_count": 11}),
            ("search", {"query_count": 23}),
            ("broad_search", {"query_count": 22}),
        ])
        passing = {
            gate["name"]: gate
            for gate in evaluate_query_budgets(summary, budgets)
        }
        self.assertTrue(passing["query_budget_read"]["pass"])
        self.assertTrue(passing["query_budget_search"]["pass"])
        self.assertEqual(
            summary["by_sample_name"],
            {
                "broad_search": {
                    "operation": "search",
                    "expected_samples": 1,
                    "observed_samples": 1,
                    "missing_query_counts": 0,
                    "samples": 1,
                    "min": 22,
                    "max": 22,
                    "counts": [22],
                },
                "read": {
                    "operation": "read",
                    "expected_samples": 1,
                    "observed_samples": 1,
                    "missing_query_counts": 0,
                    "samples": 1,
                    "min": 11,
                    "max": 11,
                    "counts": [11],
                },
                "search": {
                    "operation": "search",
                    "expected_samples": 1,
                    "observed_samples": 1,
                    "missing_query_counts": 0,
                    "samples": 1,
                    "min": 23,
                    "max": 23,
                    "counts": [23],
                },
            },
        )
        self.assertEqual(summary["missing_by_sample_name"], {})
        self.assertFalse(summary["sample_cardinality"]["authoritative"])
        self.assertTrue(summary["sample_cardinality"]["pass"])

        summary["by_operation"]["search"]["counts"].append(24)
        failing = {
            gate["name"]: gate
            for gate in evaluate_query_budgets(summary, budgets)
        }
        self.assertFalse(failing["query_budget_search"]["pass"])

    def test_gate_budgets_use_canonical_samples_not_same_operation_probes(self):
        scale = {
            "scale": FUTURE_RECORDS,
            "protocol": "simple",
            "open_found": [True],
            "open_p95_ms": 10.0,
            "search_p95_ms": 10.0,
            "read_p95_ms": 10.0,
            "checkpoint_ms": 10.0,
            "resume_ms": 10.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
            "query_counts": {
                "by_sample_name": {
                    "read": {"operation": "read", "counts": [11]},
                    "max_batch_read": {
                        "operation": "read",
                        "counts": [166],
                    },
                    "checkpoint": {
                        "operation": "checkpoint",
                        "counts": [28],
                    },
                    "resume_delta_checkpoint": {
                        "operation": "checkpoint",
                        "counts": [33],
                    },
                    "max_checkpoint_sources": {
                        "operation": "checkpoint",
                        "counts": [343],
                    },
                },
                "missing_by_sample_name": {},
                "sample_cardinality": {
                    "authoritative": True,
                    "pass": True,
                },
            },
            "resume_delta_fixture": {
                "requested": True,
                "applicable": True,
                "status": "complete",
                "pass": True,
                "source_path": "Synthetic/records/640000/target.md",
                "checkpoint_source_entries": 1,
                "checkpoint_source_version": 1,
                "checkpoint_source_content_hash": "sha256:" + "a" * 64,
                "mutation_version": 2,
                "mutation_content_hash": "sha256:" + "b" * 64,
                "mutation_no_op": False,
                "verified_read_version": 2,
                "verified_read_content_hash": "sha256:" + "b" * 64,
                "verified_read_exact_content": True,
                "verified_read_response_truncated": False,
                "mutation_marker": "PERF-RESUME-DELTA-640000",
                "expected_treatment_statement_delta": (
                    D03_RESUME_QUERY_COUNT_DELTA
                ),
                "statement_delta_accounting": [
                    "transaction_context_validation",
                    "transaction_context_setup",
                    "statement_timeout_setup",
                    "batched_version_pair_select",
                    "transaction_commit",
                ],
            },
        }
        budgets = {
            "schema": "straylight-query-budgets@v1",
            "operations": {
                "read": {"comparison": "exact", "count": 11},
                "checkpoint": {"comparison": "at_most", "max": 28},
            },
        }
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
                query_budgets=budgets,
            )
        }
        self.assertTrue(gates["query_budget_read"]["pass"])
        self.assertTrue(gates["query_budget_checkpoint"]["pass"])
        self.assertTrue(
            gates[
                "resume_delta_fixture_mutates_checkpoint_source_at_640000"
            ]["pass"]
        )
        self.assertEqual(
            gates["query_budget_read"]["observed"]["sample_name"],
            "read",
        )
        self.assertEqual(
            gates["query_budget_checkpoint"]["observed"]["counts"],
            [28],
        )

        scale["query_counts"]["by_sample_name"]["read"]["counts"] = [12]
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
                query_budgets=budgets,
            )
        }
        self.assertFalse(gates["query_budget_read"]["pass"])

        scale["query_counts"]["by_sample_name"]["read"]["counts"] = [11]
        scale["query_counts"]["by_sample_name"]["checkpoint"]["counts"] = [29]
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
                query_budgets=budgets,
            )
        }
        self.assertFalse(gates["query_budget_checkpoint"]["pass"])
        self.assertEqual(
            gates["query_budget_checkpoint"]["observed"]["counts"],
            [29],
        )

        del scale["query_counts"]["by_sample_name"]["read"]
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
                query_budgets=budgets,
            )
        }
        self.assertFalse(gates["query_budget_read"]["pass"])

    def test_resume_delta_checkpoint_is_separately_classified_and_audited(self):
        expected = expected_query_count_sample_cardinality(
            scale=FUTURE_RECORDS,
            samples_per_retrieval=1,
            verbatim_identifier_probes=1,
            concurrent_rounds=1,
            concurrent_searches_per_round=1,
            resume_delta_fixture_checkpoint=True,
        )
        self.assertNotIn("checkpoint", expected)
        self.assertEqual(expected["resume_delta_checkpoint"], 1)

        summary = summarize_query_counts(
            [("resume_delta_checkpoint", {"query_count": 33})],
            expected_cardinality={"resume_delta_checkpoint": 1},
        )

        self.assertTrue(summary["sample_cardinality"]["pass"])
        self.assertEqual(
            summary["by_sample_name"]["resume_delta_checkpoint"]["counts"],
            [33],
        )
        self.assertEqual(summary["by_operation"]["checkpoint"]["counts"], [33])

    def test_query_count_summary_enforces_authoritative_sample_cardinality(self):
        expected = expected_query_count_sample_cardinality(
            scale=PRODUCTION_RECORDS,
            samples_per_retrieval=2,
            verbatim_identifier_probes=3,
            concurrent_rounds=2,
            concurrent_searches_per_round=CONCURRENT_SEARCHES_PER_ROUND,
        )
        responses = [
            (sample_name, {"query_count": 7})
            for sample_name, cardinality in expected.items()
            for _ in range(cardinality)
        ]

        complete = summarize_query_counts(
            responses,
            expected_cardinality=expected,
        )

        self.assertTrue(complete["sample_cardinality"]["authoritative"])
        self.assertTrue(complete["sample_cardinality"]["pass"])
        self.assertEqual(
            complete["sample_cardinality"]["expected_by_sample_name"],
            expected,
        )
        self.assertEqual(
            complete["sample_cardinality"]["observed_by_sample_name"],
            expected,
        )
        self.assertEqual(
            complete["by_sample_name"]["max_batch_read"]["samples"],
            1,
        )
        self.assertEqual(
            complete["by_sample_name"][
                "verbatim_identifier_search"
            ]["samples"],
            3,
        )

        missing_response = summarize_query_counts(
            responses[:-1],
            expected_cardinality=expected,
        )
        self.assertFalse(missing_response["sample_cardinality"]["pass"])
        self.assertEqual(
            missing_response["sample_cardinality"][
                "missing_response_samples"
            ],
            {responses[-1][0]: 1},
        )

        missing_count_responses = list(responses)
        missing_count_responses[0] = (responses[0][0], {})
        missing_count = summarize_query_counts(
            missing_count_responses,
            expected_cardinality=expected,
        )
        self.assertFalse(missing_count["sample_cardinality"]["pass"])
        self.assertEqual(
            missing_count["missing_by_sample_name"],
            {responses[0][0]: 1},
        )

        with self.assertRaisesRegex(ValueError, "unknown response sample"):
            summarize_query_counts([("typo_search", {"query_count": 1})])

    def test_checked_in_query_budget_contract_covers_six_operations(self):
        contract = load_query_budgets()
        self.assertEqual(contract["profile"], "default-safe")
        self.assertEqual(
            set(contract["operations"]),
            {"open", "search", "read", "write", "checkpoint", "resume"},
        )
        self.assertEqual(QUERY_BUDGETS_PATH.name, "query_budgets.json")

    def test_d03_non_resume_query_budgets_track_default_safe(self):
        default_safe = load_query_budgets()
        d03 = load_query_budgets(
            D03_QUERY_BUDGETS_PATH,
            expected_profile=D03_RESUME_DELTAS_GATE_PROFILE,
        )

        self.assertTrue(d03["runtime_features"]["resume_deltas"])
        self.assertNotIn("resume", d03["operations"])
        self.assertEqual(
            set(d03["operations"]),
            set(default_safe["operations"]) - {"resume"},
        )
        for operation, budget in d03["operations"].items():
            self.assertEqual(
                budget,
                default_safe["operations"][operation],
                operation,
            )

    def test_query_budget_selection_is_profile_and_runtime_bound(self):
        snapshot = {"runtime_features": runtime_features()}
        selected = resolve_query_budget_contract(
            profile="default-safe",
            path=None,
            runtime_snapshot=snapshot,
            gate_profile=None,
            protocol="simple",
        )
        self.assertIsNotNone(selected)
        self.assertEqual(selected["profile"], "default-safe")

        with self.assertRaisesRegex(ValueError, "not applicable"):
            resolve_query_budget_contract(
                profile="default-safe",
                path=None,
                runtime_snapshot={
                    "runtime_features": runtime_features(
                        read_path_roundtrip_v1=True,
                    ),
                },
                gate_profile=None,
                protocol="simple",
            )
        with self.assertRaisesRegex(ValueError, "explicit"):
            resolve_query_budget_contract(
                profile="launch",
                path=None,
                runtime_snapshot=snapshot,
                gate_profile=None,
                protocol="simple",
            )
        calibration = resolve_query_budget_contract(
            profile="calibration",
            path=None,
            runtime_snapshot=snapshot,
            gate_profile=None,
            protocol="simple",
        )
        self.assertFalse(calibration["acceptance_eligible"])
        self.assertEqual(calibration["contract"]["operations"], {})
        d03 = resolve_query_budget_contract(
            profile=D03_RESUME_DELTAS_GATE_PROFILE,
            path=None,
            runtime_snapshot={
                "runtime_features": runtime_features(resume_deltas=True),
            },
            gate_profile=D03_RESUME_DELTAS_GATE_PROFILE,
            protocol="simple",
        )
        self.assertNotIn("resume", d03["contract"]["operations"])

    def test_e02_query_budget_contract_binds_modes_and_variable_feature(self):
        contract_path = (
            Path(__file__).resolve().parents[1]
            / "eval/query_budgets.e02-verbatim.json"
        )
        selected = resolve_query_budget_contract(
            profile="e02-verbatim",
            path=contract_path,
            runtime_snapshot={"runtime_features": runtime_features()},
            gate_profile=None,
            protocol="simple",
            retrieval_modes=("exact", "lexical"),
        )
        self.assertEqual(
            selected["contract"]["experiment_variable_features"],
            ["verbatim_spans"],
        )
        self.assertEqual(selected["contract"]["operations"]["write"]["count"], 14)
        self.assertEqual(
            selected["contract"]["operations"]["checkpoint"]["max"],
            28,
        )

        with self.assertRaisesRegex(ValueError, "bound to retrieval modes"):
            resolve_query_budget_contract(
                profile="e02-verbatim",
                path=contract_path,
                runtime_snapshot={"runtime_features": runtime_features()},
                gate_profile=None,
                protocol="simple",
                retrieval_modes=("exact", "lexical", "semantic"),
            )

    def test_d03_gate_pairs_query_counts_and_enforces_150ms(self):
        checkpoint_content = "Original source."
        mutation_content = (
            checkpoint_content + "\nPERF-RESUME-DELTA-640000"
        )
        checkpoint_hash = "sha256:" + hashlib.sha256(
            checkpoint_content.encode("utf-8")
        ).hexdigest()
        mutation_hash = "sha256:" + hashlib.sha256(
            mutation_content.encode("utf-8")
        ).hexdigest()
        fixture = {
            "requested": True,
            "applicable": True,
            "status": "complete",
            "pass": True,
            "source_path": "Synthetic/records/0639999.md",
            "checkpoint_source_entries": 1,
            "checkpoint_source_version": 1,
            "checkpoint_source_content_hash": checkpoint_hash,
            "mutation_version": 2,
            "mutation_content_hash": mutation_hash,
            "mutation_no_op": False,
            "verified_read_version": 2,
            "verified_read_content_hash": mutation_hash,
            "verified_read_exact_content": True,
            "verified_read_response_truncated": False,
            "mutation_marker": "PERF-RESUME-DELTA-640000",
            "expected_treatment_statement_delta": (
                D03_RESUME_QUERY_COUNT_DELTA
            ),
            "statement_delta_accounting": [
                "transaction_context_validation",
                "transaction_context_setup",
                "statement_timeout_setup",
                "batched_version_pair_select",
                "transaction_commit",
            ],
        }
        lineage = {
            "status": "complete",
            "pass": True,
            "delta_count": 1,
            "path": fixture["source_path"],
            "pinned_version": 1,
            "pinned_sha256": fixture[
                "checkpoint_source_content_hash"
            ],
            "current_version": 2,
            "current_sha256": fixture["mutation_content_hash"],
            "mode": "whole_pair",
            "before_sha256": checkpoint_hash,
            "after_sha256": mutation_hash,
            "mutation_marker_found": True,
        }
        control = {
            "resume_query_counts": [44] * DEFINITIVE_SAMPLES,
            "label": "d03-control",
            "resume_delta_fixture": fixture,
        }
        scale = {
            "scale": FUTURE_RECORDS,
            "resume_ms": 149.9,
            "resume_delta_fixture": dict(fixture),
            "resume_delta_lineage_samples": [
                dict(lineage)
                for _ in range(DEFINITIVE_SAMPLES)
            ],
            "query_counts": {
                "by_operation": {
                    "resume": {
                        "counts": [49] * DEFINITIVE_SAMPLES,
                    },
                },
            },
        }
        gates, record = evaluate_d03_resume_delta_gates([scale], control)
        self.assertTrue(all(gate["pass"] for gate in gates))
        self.assertEqual(
            record["paired_query_count_deltas"],
            [D03_RESUME_QUERY_COUNT_DELTA] * DEFINITIVE_SAMPLES,
        )

        scale["resume_ms"] = 150.1
        scale["query_counts"]["by_operation"]["resume"]["counts"][-1] = 50
        gates, _ = evaluate_d03_resume_delta_gates([scale], control)
        self.assertFalse(gates[0]["pass"])
        self.assertFalse(gates[1]["pass"])

        scale["resume_ms"] = 149.9
        scale["query_counts"]["by_operation"]["resume"]["counts"][-1] = 49
        scale["resume_delta_lineage_samples"][-1]["pass"] = False
        gates, _ = evaluate_d03_resume_delta_gates([scale], control)
        self.assertFalse(gates[2]["pass"])

    def test_resume_delta_lineage_receipt_binds_exact_versions_and_hashes(self):
        checkpoint_content = "Original source."
        mutation_content = (
            checkpoint_content + "\nPERF-RESUME-DELTA-640000"
        )
        checkpoint_hash = "sha256:" + hashlib.sha256(
            checkpoint_content.encode("utf-8")
        ).hexdigest()
        mutation_hash = "sha256:" + hashlib.sha256(
            mutation_content.encode("utf-8")
        ).hexdigest()
        fixture = {
            "requested": True,
            "applicable": True,
            "status": "complete",
            "pass": True,
            "source_path": "Synthetic/records/0639999.md",
            "checkpoint_source_entries": 1,
            "checkpoint_source_version": 1,
            "checkpoint_source_content_hash": checkpoint_hash,
            "mutation_version": 2,
            "mutation_content_hash": mutation_hash,
            "mutation_no_op": False,
            "verified_read_version": 2,
            "verified_read_content_hash": mutation_hash,
            "verified_read_exact_content": True,
            "verified_read_response_truncated": False,
            "mutation_marker": "PERF-RESUME-DELTA-640000",
            "expected_treatment_statement_delta": (
                D03_RESUME_QUERY_COUNT_DELTA
            ),
            "statement_delta_accounting": [
                "transaction_context_validation",
                "transaction_context_setup",
                "statement_timeout_setup",
                "batched_version_pair_select",
                "transaction_commit",
            ],
        }
        response = {
            "data": {
                "resume_deltas": [{
                    "path": fixture["source_path"],
                    "pinned_version": 1,
                    "pinned_sha256": fixture[
                        "checkpoint_source_content_hash"
                    ],
                    "current_version": 2,
                    "current_sha256": fixture[
                        "mutation_content_hash"
                    ],
                    "mode": "whole_pair",
                    "before": checkpoint_content,
                    "after": mutation_content,
                }],
            },
        }
        self.assertTrue(
            resume_delta_lineage_receipt(
                response,
                fixture,
            )["pass"]
        )
        response["data"]["resume_deltas"][0]["current_version"] = 3
        self.assertFalse(
            resume_delta_lineage_receipt(
                response,
                fixture,
            )["pass"]
        )

    def test_d03_pair_requires_identical_runtime_except_resume_flag(self):
        treatment_features = runtime_features(resume_deltas=True)
        control = {
            "source_revision": "abc123",
            "build_revision": "abc123",
            "retrieval_modes": ["exact", "lexical"],
            "runtime_features": {
                **treatment_features,
                "resume_deltas": False,
            },
        }
        validate_d03_resume_control_compatibility(
            control,
            runtime_snapshot={
                "build_revision": "abc123",
                "runtime_features": treatment_features,
            },
            implementation={"source_revision": "abc123"},
            retrieval_modes=["exact", "lexical"],
        )

        control["runtime_features"]["read_path_roundtrip_v1"] = True
        with self.assertRaisesRegex(ValueError, "identical"):
            validate_d03_resume_control_compatibility(
                control,
                runtime_snapshot={
                    "build_revision": "abc123",
                    "runtime_features": treatment_features,
                },
                implementation={"source_revision": "abc123"},
                retrieval_modes=["exact", "lexical"],
            )

    def test_plan_assertion_rejects_missing_index_and_search_chunk_seq_scan(self):
        healthy = [{
            "Plan": {
                "Node Type": "Bitmap Heap Scan",
                "Relation Name": "search_chunks",
                "Plans": [{
                    "Node Type": "Bitmap Index Scan",
                    "Index Name": "search_chunks_fts_idx",
                }],
            },
        }]
        self.assertTrue(evaluate_retrieval_plan(
            healthy,
            lane="lexical",
            expected_index="search_chunks_fts_idx",
        )["pass"])
        self.assertFalse(evaluate_retrieval_plan(
            [{"Plan": {
                "Node Type": "Seq Scan",
                "Relation Name": "search_chunks",
            }}],
            lane="lexical",
            expected_index="search_chunks_fts_idx",
        )["pass"])
        self.assertFalse(evaluate_retrieval_plan(
            [{"Plan": {
                "Node Type": "Index Scan",
                "Index Name": "some_other_index",
                "Relation Name": "search_chunks",
            }}],
            lane="semantic",
            expected_index="search_chunks_embedding_hnsw_idx",
        )["pass"])

    def test_plan_contract_fingerprints_migration_bodies_and_shared_calls(self):
        contract = json.loads(
            RETRIEVAL_PLAN_CONTRACT_PATH.read_text(encoding="utf-8")
        )
        shared = (
            Path(__file__).resolve().parents[1]
            / "apps/api/src/retrieval_sql.rs"
        ).read_text(encoding="utf-8")
        for lane in contract["lanes"].values():
            source = (
                Path(__file__).resolve().parents[1] / lane["migration"]
            ).read_text(encoding="utf-8")
            body = extract_sql_function_body(source, lane["function_name"])
            self.assertEqual(sql_fingerprint(body), lane["body_sha256"])
            self.assertIn(lane["invocation_sql"], shared)

    def test_plan_assertions_do_not_explain_unrequested_semantic_lane(self):
        contract = json.loads(
            RETRIEVAL_PLAN_CONTRACT_PATH.read_text(encoding="utf-8")
        )
        bodies = {}
        for lane, lane_contract in contract["lanes"].items():
            migration = (
                Path(__file__).resolve().parents[1]
                / lane_contract["migration"]
            )
            bodies[lane] = extract_sql_function_body(
                migration.read_text(encoding="utf-8"),
                lane_contract["function_name"],
            )

        def fake_run_psql(_container, sql):
            if "SELECT entry.user_id::text" in sql:
                return (
                    "11111111-1111-1111-1111-111111111111|"
                    "22222222-2222-2222-2222-222222222222"
                )
            if "::regprocedure" in sql:
                lane = next(
                    name
                    for name, lane_contract in contract["lanes"].items()
                    if lane_contract["regprocedure"] in sql
                )
                return bodies[lane]
            self.fail(f"unexpected SQL in plan assertion test: {sql}")

        lexical_plan = [{
            "Plan": {
                "Node Type": "Bitmap Index Scan",
                "Index Name": "search_chunks_fts_idx",
            },
        }]
        with (
            patch("performance_eval.run_psql", side_effect=fake_run_psql),
            patch("performance_eval.container_env_value", return_value="30"),
            patch(
                "performance_eval.explain_json",
                return_value=lexical_plan,
            ) as explain,
        ):
            result = retrieval_plan_assertions(
                "test-db",
                target_path="memory/test.md",
                query="needle",
                retrieval_modes=("exact", "lexical"),
                semantic_lane_enabled=False,
            )

        self.assertTrue(result["pass"])
        self.assertEqual(explain.call_count, 2)
        self.assertEqual(
            result["lanes"]["semantic"]["status"],
            "not_applicable",
        )

    def test_64k_regression_tier_fails_before_the_outer_slo(self):
        scale = {
            "scale": PRODUCTION_RECORDS,
            "open_found": [True],
            "open_p95_ms": 501.0,
            "search_p95_ms": 80.0,
            "broad_search_p95_ms": 90.0,
            "overflow_search_p95_ms": 90.0,
            "old_source_search_p95_ms": 90.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "critical_lane_failures": [],
            "concurrent_probe": {
                "rounds": 1,
                "write_committed": True,
                "write_ms": 30.0,
                "write_p95_ms": 30.0,
                "search_p95_ms": 100.0,
                "search_found": [True],
                "search_lane_failures": [],
            },
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
        }
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
            )
        }
        self.assertTrue(gates["every_scale_open_p95_ms"]["pass"])
        self.assertFalse(
            gates[
                f"regression_tier_{PRODUCTION_RECORDS}_open_p95_ms"
            ]["pass"]
        )

    def test_checkpoint_footprint_rejects_untrusted_ids_before_running_sql(self):
        with self.assertRaises(ValueError):
            simple_checkpoint_footprint("unused", "checkpoint:not-a-uuid'; drop table")

    def test_synthetic_corpus_has_one_exact_target(self):
        documents, target_path, marker = synthetic_documents(1_000)

        self.assertEqual(len(documents), 1_000)
        self.assertEqual(documents[-1]["path"], target_path)
        self.assertEqual(
            sum(marker in document["content"] for document in documents),
            1,
        )
        self.assertIn(
            "Recent incomplete coordination lead",
            documents[-2]["content"],
        )
        self.assertNotIn(old_source_marker(1_000), documents[-2]["content"])
        overflow_marker = lexical_overflow_marker(1_000)
        self.assertEqual(
            sum(overflow_marker in document["content"] for document in documents),
            1,
        )
        self.assertTrue(
            all(len(document["content_sha256"]) == 64 for document in documents)
        )
        task = synthetic_discovery_task(1_000)
        self.assertNotIn(target_path, task)
        self.assertNotIn(marker, task)
        self.assertIn(synthetic_discovery_key(1_000), task)
        self.assertEqual(
            sum(
                synthetic_discovery_key(1_000) in document["content"]
                for document in documents
            ),
            1,
        )

    def test_synthetic_corpus_plants_deterministic_verbatim_identifiers(self):
        documents, _, _, manifest = synthetic_documents(
            1_000,
            include_fixture_manifest=True,
        )
        probes = manifest["verbatim_identifiers"]
        self.assertEqual(len(probes), 30)
        self.assertEqual(len({item["identifier"] for item in probes}), 30)
        by_path = {document["path"]: document["content"] for document in documents}
        for probe in probes:
            self.assertRegex(
                probe["identifier"],
                r"^STRAYID-1000-\d+-[0-9a-f]{8}$",
            )
            self.assertGreater(probe["byte_offset"], 2_400)
            content = by_path[probe["path"]]
            encoded = content.encode("utf-8")
            identifier = probe["identifier"].encode("utf-8")
            self.assertEqual(
                encoded[probe["byte_offset"]:probe["byte_offset"] + len(identifier)],
                identifier,
            )

    def test_verbatim_identifier_probe_is_exact_only_and_checks_source_text(self):
        identifier = "STRAYID-1000-1-deadbeef"

        class Response:
            def __init__(self, body):
                self.body = body
                self.elapsed_ms = 1.25

        class Client:
            def __init__(self):
                self.payloads = []

            def post(self, _path, payload):
                self.payloads.append(payload)
                return Response({
                    "data": {
                        "query": payload["queries"][0]["query"],
                        "results": [{"path": "Synthetic/a.md", "excerpt": "truncated"}],
                    }
                })

        client = Client()
        response_samples = []
        result = verbatim_identifier_probe(
            client,  # type: ignore[arg-type]
            protocol="simple",
            authorization_scope="scope:test",
            session_id="session:test",
            probes=[{
                "path": "Synthetic/a.md",
                "identifier": identifier,
                "byte_offset": 2_500,
            }],
            response_samples=response_samples,
        )
        self.assertEqual(result["returned"], 0)
        self.assertFalse(result["pass"])
        self.assertEqual(client.payloads[0]["queries"][0]["modes"], ["exact"])
        self.assertEqual(
            response_samples,
            [(
                "verbatim_identifier_search",
                {
                    "data": {
                        "query": (
                            client.payloads[0]["queries"][0]["query"]
                        ),
                        "results": [{
                            "path": "Synthetic/a.md",
                            "excerpt": "truncated",
                        }],
                    },
                },
            )],
        )
        self.assertFalse(source_text_contains(
            {"query": identifier, "excerpt": "truncated"},
            identifier,
        ))
        self.assertTrue(source_text_contains(
            {"results": [{"text": identifier}]},
            identifier,
        ))

    def test_default_profile_is_definitive_and_includes_production_scale(self):
        parser = build_parser()
        args = parser.parse_args([
            "run",
            "--label",
            "default",
            "--out",
            "result.json",
        ])

        profile = resolve_run_profile(args)

        self.assertTrue(profile.definitive)
        self.assertEqual(profile.samples, DEFINITIVE_SAMPLES)
        self.assertIn(PRODUCTION_RECORDS, profile.scales)
        self.assertNotIn(FUTURE_RECORDS, profile.scales)
        self.assertTrue(profile.semantic_failure_required)

    def test_semantic_failure_probe_defaults_required_and_opt_out_is_explicit(self):
        parser = build_parser()
        required = parser.parse_args([
            "run",
            "--label",
            "required",
            "--out",
            "result.json",
        ])
        not_applicable = parser.parse_args([
            "run",
            "--label",
            "not-applicable",
            "--semantic-failure-probe",
            "not-applicable",
            "--out",
            "result.json",
        ])

        self.assertEqual(required.semantic_failure_probe, "required")
        self.assertTrue(resolve_run_profile(required).semantic_failure_required)
        self.assertFalse(
            resolve_run_profile(not_applicable).semantic_failure_required
        )

    def test_semantic_failure_not_applicable_is_fail_closed_from_runtime(self):
        snapshot = {"runtime_features": runtime_features(semantic_lane=False)}
        accepted = validate_semantic_failure_probe_posture(
            posture="not-applicable",
            runtime_snapshot=snapshot,
            retrieval_modes=["exact", "lexical"],
            wait_for_semantic=False,
            e09_arm="no_semantic",
            protocol="simple",
            hooks_configured=False,
        )
        self.assertTrue(accepted["eligible"])
        self.assertEqual(accepted["posture"], "not-applicable")

        with self.assertRaisesRegex(ValueError, "not-applicable"):
            validate_semantic_failure_probe_posture(
                posture="not-applicable",
                runtime_snapshot={
                    "runtime_features": runtime_features(semantic_lane=True),
                },
                retrieval_modes=["exact", "lexical"],
                wait_for_semantic=False,
                e09_arm=None,
                protocol="simple",
                hooks_configured=False,
            )
        with self.assertRaisesRegex(ValueError, "not-applicable"):
            validate_semantic_failure_probe_posture(
                posture="not-applicable",
                runtime_snapshot=snapshot,
                retrieval_modes=["exact", "lexical", "semantic"],
                wait_for_semantic=False,
                e09_arm=None,
                protocol="simple",
                hooks_configured=False,
            )

    def test_verbatim_feature_acceptance_defaults_required_and_opt_out_is_bound(self):
        parser = build_parser()
        required = parser.parse_args([
            "run",
            "--label",
            "required",
            "--out",
            "result.json",
        ])
        not_applicable = parser.parse_args([
            "run",
            "--label",
            "not-applicable",
            "--verbatim-feature-acceptance",
            "not-applicable",
            "--out",
            "result.json",
        ])
        self.assertEqual(required.verbatim_feature_acceptance, "required")
        self.assertEqual(
            not_applicable.verbatim_feature_acceptance,
            "not-applicable",
        )

        snapshot = {
            "runtime_features": runtime_features(verbatim_spans=False),
        }
        accepted = validate_verbatim_feature_acceptance_posture(
            posture="not-applicable",
            runtime_snapshot=snapshot,
            expected_features={"verbatim_spans": False},
            protocol="simple",
        )
        self.assertEqual(accepted["posture"], "not-applicable")
        with self.assertRaisesRegex(ValueError, "not-applicable"):
            validate_verbatim_feature_acceptance_posture(
                posture="not-applicable",
                runtime_snapshot=snapshot,
                expected_features={},
                protocol="simple",
            )
        with self.assertRaisesRegex(ValueError, "not-applicable"):
            validate_verbatim_feature_acceptance_posture(
                posture="not-applicable",
                runtime_snapshot={
                    "runtime_features": runtime_features(
                        verbatim_spans=True,
                    ),
                },
                expected_features={"verbatim_spans": False},
                protocol="simple",
            )

    def test_e05_profile_accepts_explicit_off_and_on_but_not_omitted(self):
        args = Namespace(
            gate_profile=LEXICAL_CONSOLIDATION_GATE_PROFILE,
            future_soak=True,
            protocol="simple",
            verbatim_feature_acceptance="not-applicable",
        )
        for state in (False, True):
            validate_lexical_consolidation_request(
                args,
                ["exact", "lexical"],
                {"lexical_single_scan": state},
            )
        with self.assertRaisesRegex(ValueError, "on\\|off"):
            validate_lexical_consolidation_request(
                args,
                ["exact", "lexical"],
                {},
            )
        with self.assertRaisesRegex(ValueError, "exact lexical"):
            validate_lexical_consolidation_request(
                args,
                ["lexical", "exact"],
                {"lexical_single_scan": False},
            )
        args.verbatim_feature_acceptance = "required"
        with self.assertRaisesRegex(
            ValueError,
            "verbatim-feature-acceptance not-applicable",
        ):
            validate_lexical_consolidation_request(
                args,
                ["exact", "lexical"],
                {"lexical_single_scan": False},
            )

    def test_resume_delta_fixture_is_explicit_and_profile_required(self):
        control = Namespace(
            gate_profile=None,
            future_soak=True,
            protocol="simple",
            exercise_resume_delta_fixture=True,
        )
        validate_resume_delta_fixture_request(
            control,
            ["exact", "lexical"],
        )
        treatment = Namespace(
            gate_profile=D03_RESUME_DELTAS_GATE_PROFILE,
            future_soak=True,
            protocol="simple",
            exercise_resume_delta_fixture=False,
        )
        with self.assertRaisesRegex(
            ValueError,
            "requires --exercise-resume-delta-fixture",
        ):
            validate_resume_delta_fixture_request(
                treatment,
                ["exact", "lexical"],
            )
        control.exercise_resume_delta_fixture = True
        with self.assertRaisesRegex(ValueError, "exact lexical"):
            validate_resume_delta_fixture_request(
                control,
                ["lexical", "exact"],
            )

    def test_required_semantic_failure_probe_needs_active_lane_and_hooks(self):
        accepted = validate_semantic_failure_probe_posture(
            posture="required",
            runtime_snapshot={
                "runtime_features": runtime_features(semantic_lane=True),
            },
            retrieval_modes=["exact", "lexical", "semantic"],
            wait_for_semantic=True,
            e09_arm="deadline_cache",
            protocol="simple",
            hooks_configured=True,
        )
        self.assertTrue(accepted["eligible"])
        with self.assertRaisesRegex(ValueError, "--wait-semantic"):
            validate_semantic_failure_probe_posture(
                posture="required",
                runtime_snapshot={
                    "runtime_features": runtime_features(semantic_lane=True),
                },
                retrieval_modes=["exact", "lexical", "semantic"],
                wait_for_semantic=False,
                e09_arm=None,
                protocol="simple",
                hooks_configured=True,
            )
        with self.assertRaisesRegex(ValueError, "needs --protocol simple"):
            validate_semantic_failure_probe_posture(
                posture="required",
                runtime_snapshot={
                    "runtime_features": runtime_features(semantic_lane=False),
                },
                retrieval_modes=["exact", "lexical"],
                wait_for_semantic=False,
                e09_arm=None,
                protocol="simple",
                hooks_configured=False,
            )

    def test_e09_no_semantic_requires_exact_then_lexical_modes(self):
        validate_e09_request_modes("no_semantic", ["exact", "lexical"])
        validate_e09_request_modes(
            "deadline_cache",
            ["exact", "lexical", "semantic"],
        )
        with self.assertRaisesRegex(ValueError, "exact lexical"):
            validate_e09_request_modes(
                "no_semantic",
                ["exact", "lexical", "semantic"],
            )
        with self.assertRaisesRegex(ValueError, "exact lexical"):
            validate_e09_request_modes(
                "no_semantic",
                ["lexical", "exact"],
            )

    def test_future_soak_is_explicit_and_adds_640k(self):
        parser = build_parser()
        args = parser.parse_args([
            "run",
            "--label",
            "future",
            "--future-soak",
            "--out",
            "result.json",
        ])

        profile = resolve_run_profile(args)

        self.assertEqual(profile.scales[-1], FUTURE_RECORDS)
        self.assertIn(PRODUCTION_RECORDS, profile.scales)
        self.assertTrue(profile.future_soak_requested)
        self.assertEqual(profile.import_timeout_seconds, 7_200.0)

    def test_reused_flat_controls_require_matching_complete_samples(self):
        parser = build_parser()
        args = parser.parse_args([
            "run",
            "--label",
            "future",
            "--future-soak",
            "--scales",
            str(PRODUCTION_RECORDS),
            "--out",
            "result.json",
        ])
        profile = resolve_run_profile(args)
        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary) / "prior.json"
            artifact.write_text(json.dumps({
                "scales": [
                    {
                        "scale": scale,
                        "flat_file_control": {
                            "files": scale,
                            "samples": DEFINITIVE_SAMPLES,
                            "discovery_found": [True] * DEFINITIVE_SAMPLES,
                            "read_found": [True] * DEFINITIVE_SAMPLES,
                            "broad_found": [True] * DEFINITIVE_SAMPLES,
                        },
                    }
                    for scale in profile.scales
                ],
            }), encoding="utf-8")
            controls = load_reused_flat_controls(artifact, profile)
            self.assertEqual(set(controls), set(profile.scales))
            controls[FUTURE_RECORDS]["discovery_found"][0] = False
            artifact.write_text(json.dumps({
                "scales": [
                    {
                        "scale": scale,
                        "flat_file_control": control,
                    }
                    for scale, control in controls.items()
                ],
            }), encoding="utf-8")
            with self.assertRaises(ValueError):
                load_reused_flat_controls(artifact, profile)

    def test_quick_profile_is_explicitly_non_definitive(self):
        parser = build_parser()
        args = parser.parse_args([
            "run",
            "--label",
            "quick",
            "--quick",
            "--scales",
            "20",
            "--out",
            "result.json",
        ])

        profile = resolve_run_profile(args)

        self.assertFalse(profile.definitive)
        self.assertEqual(profile.samples, 3)
        self.assertEqual(profile.scales, (20,))
        self.assertTrue(profile.semantic_failure_required)
        self.assertEqual(profile.semantic_failure_probe_posture, "required")

    def test_definitive_profile_rejects_missing_64k_or_too_few_samples(self):
        base = {
            "quick": False,
            "future_soak": False,
            "import_timeout": None,
            "samples": DEFINITIVE_SAMPLES,
            "scales": [10_000],
        }
        with self.assertRaisesRegex(ValueError, "64,000"):
            resolve_run_profile(Namespace(**base))

        base["scales"] = [PRODUCTION_RECORDS]
        base["samples"] = DEFINITIVE_SAMPLES - 1
        with self.assertRaisesRegex(ValueError, "at least 30"):
            resolve_run_profile(Namespace(**base))

    def test_quick_and_future_soak_are_not_conflated(self):
        with self.assertRaisesRegex(ValueError, "cannot be combined"):
            resolve_run_profile(Namespace(
                quick=True,
                future_soak=True,
                import_timeout=None,
                samples=None,
                scales=None,
            ))

    def test_640k_scale_requires_explicit_future_soak_flag(self):
        with self.assertRaisesRegex(ValueError, "use --future-soak"):
            resolve_run_profile(Namespace(
                quick=False,
                future_soak=False,
                import_timeout=None,
                samples=DEFINITIVE_SAMPLES,
                scales=[PRODUCTION_RECORDS, FUTURE_RECORDS],
            ))

    def test_flat_file_control_measures_discovery_read_and_broad_search(self):
        documents, target_path, marker = synthetic_documents(25)

        control = benchmark_flat_files(
            documents,
            scale=25,
            target_path=target_path,
            marker=marker,
            samples=3,
            timeout_seconds=10.0,
        )

        self.assertEqual(control["files"], 25)
        self.assertEqual(control["samples"], 3)
        self.assertFalse(control["discovery_path_was_provided"])
        self.assertTrue(all(control["discovery_found"]))
        self.assertTrue(all(control["read_found"]))
        self.assertTrue(all(control["broad_found"]))
        self.assertEqual(len(control["discovery_ms"]), 3)
        self.assertEqual(len(control["read_ms"]), 3)
        self.assertEqual(len(control["broad_search_ms"]), 3)

    def test_response_accounting_separates_source_and_transport(self):
        body = {
            "status": "complete",
            "data": {
                "evidence": [
                    {"path": "one.md", "text": "source one"},
                    {"path": "two.md", "excerpt": "source two"},
                ],
            },
        }

        metrics = response_character_metrics(body)
        summary = summarize_response_accounting([
            ("open", body),
            ("search", body),
        ])

        self.assertEqual(
            metrics["source_text_chars"],
            len("source one") + len("source two"),
        )
        self.assertEqual(
            metrics["source_identity_chars"],
            len("one.md") + len("two.md"),
        )
        self.assertEqual(
            metrics["payload_chars"],
            metrics["source_text_chars"] + metrics["metadata_chars"],
        )
        self.assertEqual(
            metrics["payload_chars"],
            metrics["evidence_chars"] + metrics["protocol_chars"],
        )
        self.assertEqual(
            summary["source_text_chars"],
            metrics["source_text_chars"] * 2,
        )
        self.assertEqual(summary["by_operation"]["open"]["samples"], 1)
        self.assertIn("estimated_payload_tokens", summary)

    def test_timing_samples_are_flattened_and_summarized(self):
        body = {
            "timings_ms": {
                "generation": 1.0,
                "retrieval_wall": 4.0,
                "unattributed": 1.0,
                "total": 6.0,
                "lanes": {"exact": 2.0, "lexical": 3.0},
            }
        }
        timings = response_timings(body)
        summary = summarize_timing_samples([timings, timings])

        self.assertEqual(summary["generation"]["samples"], 2)
        self.assertEqual(summary["lanes.exact"]["p95"], 2.0)
        self.assertTrue(timing_phase_sum_sane(timings))
        self.assertFalse(timing_phase_sum_sane({"total": 4.0, "exact": 9.0}))

    def test_e03_parser_exposes_semantic_ready_and_unique_query_modes(self):
        args = build_parser().parse_args([
            "run",
            "--label",
            "e03",
            "--wait-semantic",
            "--unique-queries",
            "--out",
            "result.json",
        ])

        self.assertTrue(args.wait_semantic)
        self.assertTrue(args.unique_queries)

    def test_e09_600ms_parser_requires_explicit_policy_inputs_at_execution(self):
        args = build_parser().parse_args([
            "run",
            "--label",
            "e09-step",
            "--e09-arm",
            "deadline_cache_600",
            "--e09-step-policy",
            "results/step-policy.json",
            "--e09-step-policy-sha256",
            "a" * 64,
            "--require-semantic-failure-hook-attestation",
            "--out",
            "result.json",
        ])
        self.assertEqual(args.e09_arm, "deadline_cache_600")
        self.assertEqual(
            args.e09_step_policy,
            Path("results/step-policy.json"),
        )
        self.assertTrue(args.require_semantic_failure_hook_attestation)

    def test_lane_failure_detection_handles_query_and_gap_shapes(self):
        self.assertTrue(response_reports_lane_failure(
            {"lane_failures": ["semantic"]},
            "semantic",
        ))
        self.assertTrue(response_reports_lane_failure(
            {"lane_failures": ["semantic_unavailable"]},
            "semantic",
        ))
        self.assertTrue(response_reports_lane_failure(
            {"gaps": [{"kind": "retrieval_lane_failed", "lane": "semantic"}]},
            "semantic",
        ))
        self.assertFalse(response_reports_lane_failure(
            {"status": "complete", "results": []},
            "semantic",
        ))
        self.assertTrue(response_reports_gap_kind(
            {
                "gaps": [{
                    "kind": "retrieval_lane_deferred",
                    "lane": "semantic",
                }]
            },
            "retrieval_lane_deferred",
        ))
        self.assertFalse(response_reports_gap_kind(
            {
                "gaps": [{
                    "kind": "retrieval_lane_unavailable",
                    "lane": "semantic",
                }]
            },
            "retrieval_lane_deferred",
        ))

    def test_semantic_failure_without_hooks_is_explicitly_unproven(self):
        result = semantic_failure_probe(
            None,  # type: ignore[arg-type]
            protocol="simple",
            authorization_scope="scope:test",
            session_id="session:test",
            query="locate semantic failure marker",
            marker="marker",
            target_path="Synthetic/records/0063999.md",
            required=True,
            start_command=None,
            stop_command=None,
            settle_seconds=0.0,
            timeout_seconds=1.0,
        )

        self.assertEqual(result["status"], "not_run")
        self.assertFalse(result["pass"])
        self.assertEqual(
            result["required_arguments"],
            [
                "--semantic-failure-start-command",
                "--semantic-failure-stop-command",
            ],
        )

    def test_semantic_failure_probe_requires_failure_fallback_and_restore(self):
        marker = "narrow-fact-64000-cobalt"
        target_path = "Synthetic/records/0063999.md"

        class Response:
            def __init__(self, body):
                self.body = body
                self.elapsed_ms = 1.0

        class Client:
            def __init__(self):
                self.payloads = []
                self.responses = iter([
                    {
                        "data": {
                            "candidates": [{
                                "path": target_path,
                                "excerpt": marker,
                            }]
                        }
                    },
                    {"data": {"candidates": [], "lane_failures": ["semantic"]}},
                    {
                        "data": {
                            "candidates": [{
                                "path": target_path,
                                "excerpt": marker,
                            }]
                        }
                    },
                    {
                        "data": {
                            "candidates": [{
                                "path": target_path,
                                "excerpt": marker,
                            }],
                            "lane_failures": ["semantic"],
                        },
                    },
                    {
                        "data": {
                            "candidates": [{
                                "path": target_path,
                                "excerpt": marker,
                            }]
                        }
                    },
                ])

            def post(self, _path, payload):
                self.payloads.append(payload)
                return Response(next(self.responses))

        client = Client()
        result = semantic_failure_probe(
            client,  # type: ignore[arg-type]
            protocol="simple",
            authorization_scope="scope:test",
            session_id=None,
            query="locate narrow fact",
            marker=marker,
            target_path=target_path,
            required=True,
            start_command="/usr/bin/true",
            stop_command="/usr/bin/true",
            settle_seconds=0.0,
            timeout_seconds=1.0,
        )

        self.assertEqual(result["status"], "passed")
        self.assertTrue(result["pass"])
        self.assertTrue(result["semantic_failure_observed"])
        self.assertTrue(result["exact_lexical_found_during_failure"])
        self.assertTrue(result["mixed_lane_found_during_failure"])
        self.assertTrue(result["semantic_lane_healthy_after_restore"])
        self.assertTrue(
            all("session_id" not in payload for payload in client.payloads)
        )

    def test_semantic_failure_probe_rejects_empty_baseline_candidates(self):
        class Response:
            body = {"data": {"candidates": []}}
            elapsed_ms = 1.0

        class Client:
            def post(self, _path, _payload):
                return Response()

        result = semantic_failure_probe(
            Client(),  # type: ignore[arg-type]
            protocol="simple",
            authorization_scope="scope:test",
            session_id="session:test",
            query="locate narrow fact",
            marker="narrow-fact-64000-cobalt",
            target_path="Synthetic/records/0063999.md",
            required=True,
            start_command="/usr/bin/true",
            stop_command="/usr/bin/true",
            settle_seconds=0.0,
            timeout_seconds=1.0,
        )

        self.assertEqual(result["status"], "baseline_failed")
        self.assertFalse(result["baseline_semantic_target_found"])
        self.assertEqual(
            result["candidate_audits"]["baseline_semantic"][
                "candidate_count"
            ],
            0,
        )

    def test_candidate_audit_binds_path_and_marker_to_same_candidate(self):
        marker = "semantic-marker"
        target_path = "eval/probe.md"
        split_evidence = candidate_audit(
            {
                "candidates": [
                    {
                        "path": target_path,
                        "metadata": {"note": marker},
                    },
                    {
                        "path": "eval/other.md",
                        "excerpt": marker,
                    },
                ]
            },
            marker=marker,
            target_path=target_path,
        )
        self.assertFalse(split_evidence["target_found"])

        bound_evidence = candidate_audit(
            {
                "candidates": [{
                    "path": target_path,
                    "excerpt": f"The answer is {marker}.",
                    "lanes": ["semantic"],
                    "score": 2.5,
                }]
            },
            marker=marker,
            target_path=target_path,
        )
        self.assertTrue(bound_evidence["target_found"])
        self.assertTrue(
            bound_evidence["candidates"][0]["marker_in_source_text"]
        )

    def test_isolated_failure_probe_provisions_and_revokes_unique_fixture(self):
        class Admin:
            base_url = "http://unit"
            token = "admin-token"

        captured = {}

        def provision(_client, **kwargs):
            captured["provision"] = kwargs
            return {
                "token": "scoped-token-must-not-be-recorded",
                "authorization_scope": "eval:unit/probe",
                "requested_authorization_scope": "eval:unit/probe",
                "status_url": (
                    "/v1/workspace/admin/eval/imports/simple-import:unit"
                ),
                "credential_provenance": "service_issued_case_scope",
                "provisioning": {},
            }

        def core(_client, **kwargs):
            captured["core"] = kwargs
            return {
                "status": "passed",
                "pass": True,
                "required": True,
            }

        with (
            patch("performance_eval.NativeApiClient", return_value=object()),
            patch(
                "performance_eval.provision_evaluation",
                side_effect=provision,
            ),
            patch(
                "performance_eval.semantic_failure_probe",
                side_effect=core,
            ),
            patch(
                "performance_eval.cleanup_fixture",
                return_value={
                    "attempted": True,
                    "credential_revocation_verified": True,
                    "pass": True,
                },
            ) as cleanup,
        ):
            result = isolated_semantic_failure_probe(
                Admin(),  # type: ignore[arg-type]
                run_id="perf-unit",
                protocol="simple",
                required=True,
                start_command="/usr/bin/true start",
                stop_command="/usr/bin/true stop",
                settle_seconds=0.0,
                timeout_seconds=30.0,
            )

        self.assertTrue(result["pass"])
        self.assertTrue(captured["provision"]["wait_for_semantic"])
        self.assertEqual(len(captured["provision"]["documents"]), 1)
        self.assertIsNone(captured["core"]["session_id"])
        self.assertIn(
            captured["core"]["marker"],
            captured["provision"]["documents"][0]["content"],
        )
        self.assertNotIn(
            "scoped-token-must-not-be-recorded",
            json.dumps(result),
        )
        cleanup.assert_called_once_with(
            ANY,
            status_url=(
                "/v1/workspace/admin/eval/imports/simple-import:unit"
            ),
        )

    def test_isolated_failure_probe_cleanup_failure_is_fatal(self):
        class Admin:
            base_url = "http://unit"
            token = "admin-token"

        metadata = {
            "token": "scoped-token",
            "authorization_scope": "eval:unit/probe",
            "requested_authorization_scope": "eval:unit/probe",
            "status_url": "/v1/workspace/admin/eval/imports/unit",
            "credential_provenance": "service_issued_case_scope",
            "provisioning": {},
        }
        with (
            patch("performance_eval.NativeApiClient", return_value=object()),
            patch(
                "performance_eval.provision_evaluation",
                return_value=metadata,
            ),
            patch(
                "performance_eval.semantic_failure_probe",
                return_value={
                    "status": "passed",
                    "pass": True,
                    "required": True,
                },
            ),
            patch(
                "performance_eval.cleanup_fixture",
                return_value={
                    "attempted": True,
                    "credential_revocation_verified": False,
                    "pass": False,
                    "error": "credential remained live",
                },
            ),
        ):
            result = isolated_semantic_failure_probe(
                Admin(),  # type: ignore[arg-type]
                run_id="perf-unit",
                protocol="simple",
                required=True,
                start_command="/usr/bin/true start",
                stop_command="/usr/bin/true stop",
                settle_seconds=0.0,
                timeout_seconds=30.0,
            )

        self.assertFalse(result["pass"])
        self.assertEqual(result["status"], "cleanup_failed")

    def test_percentile_interpolates_small_samples(self):
        self.assertEqual(percentile([], 0.95), 0.0)
        self.assertEqual(percentile([10.0], 0.95), 10.0)
        self.assertEqual(percentile([10.0, 20.0, 30.0], 0.50), 20.0)
        self.assertEqual(percentile([10.0, 20.0], 0.50), 15.0)

    def test_table_growth_reports_only_changed_tables(self):
        before = DatabaseSnapshot(
            size_bytes=1_000,
            table_rows={"entries": 10, "entry_versions": 10, "jobs": 2},
        )
        after = DatabaseSnapshot(
            size_bytes=1_500,
            table_rows={"entries": 10, "entry_versions": 11, "jobs": 2},
        )
        self.assertEqual(table_growth(before, after), {"entry_versions": 1})

    def test_counter_growth_reports_index_use(self):
        self.assertEqual(
            counter_growth(
                {"search_chunks_fts_idx": 4},
                {
                    "search_chunks_fts_idx": 11,
                    "search_chunks_embedding_hnsw_idx": 2,
                },
            ),
            {
                "search_chunks_embedding_hnsw_idx": 2,
                "search_chunks_fts_idx": 7,
            },
        )

    def test_gin_gate_is_preserved_for_definitive_not_tiny_quick_runs(self):
        scale = {
            "scale": 100,
            "open_found": [True],
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "index_scan_growth": {"search_chunks_fts_idx": 0},
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
        }

        definitive = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=True,
            )
        }
        quick = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
            )
        }

        self.assertFalse(definitive["lexical_search_uses_gin_index"]["pass"])
        self.assertNotIn("lexical_search_uses_gin_index", quick)

    def test_verbatim_identifier_is_a_named_blocking_gate(self):
        scale = {
            "scale": 1_000,
            "protocol": "simple",
            "open_found": [True],
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
            "verbatim_identifier": {
                "status": "complete",
                "returned": 0,
                "expected": 30,
                "pass": False,
            },
        }
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
            )
        }
        self.assertIn("verbatim_identifier", gates)
        self.assertFalse(gates["verbatim_identifier"]["pass"])

    def test_verbatim_not_applicable_omits_only_feature_acceptance(self):
        scale = {
            "scale": 1_000,
            "protocol": "simple",
            "open_found": [True],
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
            "fixture_manifest": {"verbatim_identifiers": []},
            "verbatim_identifier": {
                "status": "complete",
                "results": [],
                "returned": 0,
                "expected": 0,
                "pass": True,
            },
        }
        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                require_gin_index=False,
                verbatim_feature_acceptance_required=False,
            )
        }
        self.assertIn("verbatim_identifier_measurement_integrity", gates)
        self.assertNotIn("verbatim_identifier", gates)

    def test_gates_reject_corpus_sized_checkpoint_growth(self):
        base = {
            "scale": 1_000,
            "open_found": [True],
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "checkpoint_database_growth": {
                "rows": 1_100,
                "bytes": 12_000_000,
                "tables": {"corpus_revision_members": 1_000},
            },
        }
        large = {
            **base,
            "scale": 10_000,
            "open_p95_ms": 1_800.0,
            "checkpoint_database_growth": {
                "rows": 10_100,
                "bytes": 120_000_000,
                "tables": {"corpus_revision_members": 10_000},
            },
        }
        thresholds = {
            "open_p95_ms": 5_000.0,
            "search_p95_ms": 3_000.0,
            "read_p95_ms": 1_000.0,
            "checkpoint_ms": 2_000.0,
            "resume_ms": 5_000.0,
            "checkpoint_row_growth": 100,
            "checkpoint_bytes_growth": 4_000_000,
            "ten_x_latency_growth": 6.0,
            "latency_growth_floor_ms": 1_000.0,
        }

        gates = {gate["name"]: gate for gate in evaluate_gates([base, large], thresholds)}

        self.assertFalse(gates["checkpoint_row_growth_is_bounded"]["pass"])
        self.assertFalse(gates["checkpoint_storage_growth_is_bounded"]["pass"])
        self.assertFalse(
            gates["open_latency_growth_is_materially_bounded"]["pass"]
        )

    def test_fast_absolute_latency_is_not_failed_by_small_baseline_ratio(self):
        base = {
            "scale": 1_000,
            "open_found": [True],
            "open_p95_ms": 20.0,
            "search_p95_ms": 15.0,
            "read_p95_ms": 10.0,
            "checkpoint_ms": 20.0,
            "resume_ms": 20.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "checkpoint_database_growth": {"rows": 1, "bytes": 100},
        }
        large = {
            **base,
            "scale": 64_000,
            "open_p95_ms": 220.0,
            "search_p95_ms": 150.0,
        }

        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [base, large],
                DEFAULT_THRESHOLDS,
            )
        }

        self.assertTrue(
            gates["open_latency_growth_is_materially_bounded"]["pass"]
        )
        self.assertTrue(
            gates["search_latency_growth_is_materially_bounded"]["pass"]
        )

    def test_every_scale_must_meet_latency_and_lane_health_gates(self):
        healthy = {
            "scale": 64_000,
            "open_found": [True],
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "broad_search_p95_ms": 90.0,
            "overflow_search_p95_ms": 90.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "read_found": [True],
            "resume_found": True,
            "critical_lane_failures": [],
            "overflow_found": [True],
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
        }
        unhealthy_smaller_scale = {
            **healthy,
            "scale": 10_000,
            "search_p95_ms": DEFAULT_THRESHOLDS["search_p95_ms"] + 1,
            "critical_lane_failures": [{
                "operation": "search",
                "exact": False,
                "lexical": True,
            }],
        }

        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [unhealthy_smaller_scale, healthy],
                DEFAULT_THRESHOLDS,
            )
        }

        self.assertFalse(gates["every_scale_search_p95_ms"]["pass"])
        self.assertFalse(gates["no_exact_or_lexical_lane_failures"]["pass"])
        self.assertTrue(
            gates[
                "bounded_lexical_overflow_returns_late_relevant_source"
            ]["pass"]
        )

    def test_definitive_gates_reject_metadata_and_unproven_fallback(self):
        scale = {
            "scale": PRODUCTION_RECORDS,
            "samples": DEFINITIVE_SAMPLES,
            "discovery_path_was_provided": False,
            "open_found": [True] * DEFINITIVE_SAMPLES,
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "broad_search_p95_ms": 90.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True] * DEFINITIVE_SAMPLES,
            "broad_found": [True] * DEFINITIVE_SAMPLES,
            "read_found": [True] * DEFINITIVE_SAMPLES,
            "resume_found": True,
            "flat_file_control": {
                "discovery_found": [True] * DEFINITIVE_SAMPLES,
                "read_found": [True] * DEFINITIVE_SAMPLES,
                "broad_found": [True] * DEFINITIVE_SAMPLES,
            },
            "response_accounting": {
                "source_text_chars": 100,
                "source_identity_chars": 10,
                "evidence_chars": 110,
                "protocol_chars": 111,
                "protocol_to_evidence_ratio": 1.009091,
            },
            "semantic_failure_probe": {
                "status": "not_run",
                "pass": False,
            },
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {"entry_versions": 1},
            },
        }

        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                required_scales=[PRODUCTION_RECORDS, FUTURE_RECORDS],
                minimum_samples=DEFINITIVE_SAMPLES,
                semantic_failure_required=True,
            )
        }

        self.assertTrue(
            gates["unknown_path_discovery_does_not_reveal_target_path"]["pass"]
        )
        self.assertTrue(gates["retrieval_sample_count_is_definitive"]["pass"])
        self.assertFalse(gates["all_required_scales_completed"]["pass"])
        self.assertFalse(
            gates["service_protocol_overhead_does_not_exceed_evidence"]["pass"]
        )
        self.assertFalse(
            gates[
                "semantic_provider_failure_falls_back_to_exact_and_lexical"
            ]["pass"]
        )

    def test_flat_file_evidence_gates_fail_on_any_miss(self):
        scale = {
            "scale": PRODUCTION_RECORDS,
            "open_found": [True],
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "broad_search_p95_ms": 90.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True],
            "broad_found": [True],
            "read_found": [True],
            "resume_found": True,
            "flat_file_control": {
                "discovery_found": [True, False],
                "read_found": [True, True],
                "broad_found": [True, True],
            },
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
        }

        gates = {
            gate["name"]: gate
            for gate in evaluate_gates([scale], DEFAULT_THRESHOLDS)
        }

        self.assertFalse(gates["flat_file_discovery_returns_target"]["pass"])
        self.assertTrue(gates["flat_file_exact_read_returns_target"]["pass"])
        self.assertTrue(
            gates["flat_file_broad_search_returns_sources"]["pass"]
        )

    def test_checkpoint_pressure_and_foreground_write_samples_are_gated(self):
        scale = {
            "scale": PRODUCTION_RECORDS,
            "samples": DEFINITIVE_SAMPLES,
            "open_found": [True] * DEFINITIVE_SAMPLES,
            "open_p95_ms": 100.0,
            "search_p95_ms": 80.0,
            "read_p95_ms": 20.0,
            "checkpoint_ms": 50.0,
            "resume_ms": 100.0,
            "search_found": [True] * DEFINITIVE_SAMPLES,
            "read_found": [True] * DEFINITIVE_SAMPLES,
            "resume_found": True,
            "concurrent_probe": {
                "rounds": DEFINITIVE_SAMPLES - 1,
                "write_ms": 100.0,
                "write_committed": True,
                "search_p95_ms": 100.0,
                "search_found": [True],
                "search_lane_failures": [],
            },
            "checkpoint_pressure": {
                "requested_checkpoints_per_minute": 2.0,
            },
            "checkpoint_database_growth": {
                "rows": 1,
                "bytes": None,
                "tables": {},
            },
        }

        gates = {
            gate["name"]: gate
            for gate in evaluate_gates(
                [scale],
                DEFAULT_THRESHOLDS,
                minimum_samples=DEFINITIVE_SAMPLES,
                require_gin_index=False,
            )
        }

        self.assertFalse(
            gates["foreground_write_sample_count_is_definitive"]["pass"]
        )
        self.assertFalse(gates["requested_checkpoint_rate_is_bounded"]["pass"])

    def test_compare_reports_before_and_after_amplification(self):
        def result(label, checkpoint_rows):
            scale = {
                "scale": 10_000,
                "import_ms": 100.0,
                "open_p95_ms": 100.0,
                "search_p95_ms": 80.0,
                "read_p95_ms": 20.0,
                "checkpoint_ms": 50.0,
                "resume_ms": 100.0,
                "checkpoint_database_growth": {
                    "rows": checkpoint_rows,
                    "bytes": checkpoint_rows * 1_000,
                    "tables": {},
                },
            }
            return {"label": label, "scales": [scale], "pass": checkpoint_rows < 100}

        compared = compare_results(result("old", 10_000), result("new", 4))

        self.assertEqual(compared["shared_scales"], [10_000])
        self.assertEqual(compared["rows"][0]["checkpoint_rows_before"], 10_000)
        self.assertEqual(compared["rows"][0]["checkpoint_rows_after"], 4)
        self.assertTrue(compared["after_pass"])


    def test_retrieval_plan_applicability_skips_unrequested_semantic_lane(self):
        result = apply_retrieval_plan_applicability(
            {
                "status": "complete",
                "sql_drift": [{"lane": "lexical", "pass": True}],
                "lanes": {
                    "lexical": {"pass": True},
                    "semantic": {"pass": False},
                },
            },
            retrieval_modes=("exact", "lexical"),
            semantic_lane_enabled=False,
        )
        self.assertTrue(result["pass"])
        self.assertEqual(result["required_lanes"], ["lexical"])
        self.assertEqual(
            result["lanes"]["semantic"]["status"],
            "not_applicable",
        )
        self.assertTrue(result["lanes"]["semantic"]["pass"])

    def test_retrieval_plan_applicability_fails_requested_semantic_mismatch(self):
        result = apply_retrieval_plan_applicability(
            {
                "status": "complete",
                "sql_drift": [
                    {"lane": "lexical", "pass": True},
                    {"lane": "semantic", "pass": True},
                ],
                "lanes": {
                    "lexical": {"pass": True},
                    "semantic": {"pass": True},
                },
            },
            retrieval_modes=("exact", "lexical", "semantic"),
            semantic_lane_enabled=False,
        )
        self.assertFalse(result["pass"])
        self.assertEqual(
            result["lanes"]["semantic"]["status"],
            "runtime_mismatch",
        )
        self.assertFalse(result["lanes"]["semantic"]["pass"])


if __name__ == "__main__":
    unittest.main()

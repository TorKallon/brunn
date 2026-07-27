import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from performance_eval import (  # noqa: E402
    DEFAULT_THRESHOLDS,
    DEFINITIVE_SAMPLES,
    FUTURE_RECORDS,
    PRODUCTION_RECORDS,
    DatabaseSnapshot,
    benchmark_flat_files,
    build_parser,
    compare_results,
    counter_growth,
    evaluate_gates,
    load_reused_flat_controls,
    old_source_marker,
    percentile,
    resolve_run_profile,
    response_character_metrics,
    response_reports_lane_failure,
    response_reports_gap_kind,
    semantic_failure_probe,
    source_text_contains,
    response_timings,
    summarize_timing_samples,
    timing_phase_sum_sane,
    lexical_overflow_marker,
    simple_checkpoint_footprint,
    summarize_response_accounting,
    synthetic_discovery_key,
    synthetic_discovery_task,
    synthetic_documents,
    verbatim_identifier_probe,
    table_growth,
)


class PerformanceEvalTests(unittest.TestCase):
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
        )
        self.assertEqual(result["returned"], 0)
        self.assertFalse(result["pass"])
        self.assertEqual(client.payloads[0]["queries"][0]["modes"], ["exact"])
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
        self.assertFalse(profile.semantic_failure_required)

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

    def test_lane_failure_detection_handles_query_and_gap_shapes(self):
        self.assertTrue(response_reports_lane_failure(
            {"lane_failures": ["semantic"]},
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
            scale=PRODUCTION_RECORDS,
            marker="marker",
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

        class Response:
            def __init__(self, body):
                self.body = body
                self.elapsed_ms = 1.0

        class Client:
            def __init__(self):
                self.responses = iter([
                    {"data": {"results": []}},
                    {"data": {"results": [], "lane_failures": ["semantic"]}},
                    {"data": {"results": [{"text": marker}]}},
                    {
                        "data": {
                            "results": [{"text": marker}],
                            "lane_failures": ["semantic"],
                        },
                    },
                    {"data": {"results": []}},
                ])

            def post(self, _path, _payload):
                return Response(next(self.responses))

        result = semantic_failure_probe(
            Client(),  # type: ignore[arg-type]
            protocol="simple",
            authorization_scope="scope:test",
            session_id="session:test",
            scale=PRODUCTION_RECORDS,
            marker=marker,
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


if __name__ == "__main__":
    unittest.main()

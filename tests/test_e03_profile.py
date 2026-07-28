import argparse
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from eval.e03_mode1 import build_performance_command as mode1_command
from eval.e03_mode2 import (
    MOCK,
    build_performance_command as mode2_command,
    main as mode2_main,
    mock_network_probe,
)
from eval.e03_mode3 import (
    build_performance_command as mode3_command,
    container_host_route_allowed,
    main as mode3_main,
    normalize_official_upstream,
    usage_receipt_contract,
)
from performance_eval import (
    E03_COMMON_RUNTIME_EXPECTATIONS,
    E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS,
    E03_SEMANTIC_READY_GATE_PROFILE,
    E03_WRAPPER_TIMEOUT_SECONDS,
    DEFAULT_THRESHOLDS,
    PRODUCTION_RECORDS,
    apply_e03_gate_policy,
    build_parser,
    evaluate_gates,
    e03_api_route_binding,
    iter_plan_nodes,
    resolve_run_profile,
    validate_e03_request,
    validate_e03_runtime_metadata,
    verbatim_identifier_measurement_evidence,
)


def verbatim_measurement(*, rows: int = 30, returned: int = 0):
    planted = [
        {
            "path": f"Synthetic/{index}.md",
            "identifier": f"STRAYID-64000-{index}-deadbeef",
            "byte_offset": 2_500 + index,
        }
        for index in range(30)
    ]
    results = [
        {
            **item,
            "modes": ["exact"],
            "verbatim_in_source_payload": index < returned,
        }
        for index, item in enumerate(planted[:rows])
    ]
    return {
        "fixture_manifest": {"verbatim_identifiers": planted},
        "verbatim_identifier": {
            "status": "complete",
            "expected": 30,
            "returned": returned,
            "pass": returned == 30,
            "results": results,
        },
    }


def scale_fixture(*, search_p95_ms: float = 80.0):
    pair_phase = {
        "pass": True,
        "max_ms": 100.0,
        "p95_ms": 80.0,
        "counter_contract_pass": True,
    }
    return {
        "scale": PRODUCTION_RECORDS,
        "protocol": "simple",
        "samples": 30,
        "open_found": [True] * 30,
        "search_found": [True] * 30,
        "read_found": [True] * 30,
        "resume_found": True,
        "open_p95_ms": 100.0,
        "search_p95_ms": search_p95_ms,
        "broad_search_p95_ms": 90.0,
        "overflow_search_p95_ms": 90.0,
        "old_source_search_p95_ms": 90.0,
        "read_p95_ms": 20.0,
        "checkpoint_ms": 50.0,
        "resume_ms": 100.0,
        "critical_lane_failures": [],
        "checkpoint_database_growth": {"rows": 1, "bytes": 1, "tables": {}},
        "concurrent_probe": {
            "rounds": 30,
            "write_committed": True,
            "write_ms": 30.0,
            "write_p95_ms": 30.0,
            "search_p95_ms": 100.0,
            "search_found": [True],
            "search_lane_failures": [],
        },
        "retrieval_plan_assertions": {"status": "complete", "pass": True},
        "e03_mode3_paired_query": {
            "status": "complete",
            "pass": True,
            "single_provisioning_event": True,
            "cold_before_warm": True,
            "same_query_strings": True,
            "same_session_id": True,
            "cardinality": {"stable": True},
            "cold": dict(pair_phase),
            "warm": dict(pair_phase),
        },
        **verbatim_measurement(),
    }


def request_namespace(arm: str):
    return argparse.Namespace(
        gate_profile=E03_SEMANTIC_READY_GATE_PROFILE,
        e03_arm=arm,
        protocol="simple",
        e09_arm=None,
        query_budget_profile="default-safe",
        semantic_failure_start_command=None if arm == "mode1" else "start",
        semantic_failure_stop_command=None if arm == "mode1" else "stop",
        semantic_failure_probe=(
            "not-applicable" if arm == "mode1" else "required"
        ),
        verbatim_feature_acceptance="not-applicable",
        wait_semantic=arm != "mode1",
        require_semantic_failure_hook_attestation=arm == "mode3",
        unique_queries=False,
        worker_container=None if arm == "mode1" else "worker",
        scales=[PRODUCTION_RECORDS],
        future_soak=False,
        quick=False,
        samples=30,
    )


class E03ProfileTests(unittest.TestCase):
    def test_semantic_arms_default_to_documented_twelve_hour_import_boundary(self):
        for arm in ("mode2", "mode3"):
            parser = build_parser()
            args = parser.parse_args([
                "run",
                "--label",
                f"e03-{arm}",
                "--gate-profile",
                E03_SEMANTIC_READY_GATE_PROFILE,
                "--e03-arm",
                arm,
                "--scales",
                str(PRODUCTION_RECORDS),
                "--out",
                "result.json",
            ])
            self.assertEqual(
                resolve_run_profile(args).import_timeout_seconds,
                E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS,
            )
            args.import_timeout = E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS - 1
            with self.assertRaisesRegex(ValueError, "require --import-timeout"):
                resolve_run_profile(args)

    def test_mode_wrappers_outlive_semantic_import_boundary(self):
        from eval.e03_mode2 import build_parser as mode2_parser
        from eval.e03_mode3 import build_parser as mode3_parser

        for parser in (mode2_parser(), mode3_parser()):
            self.assertEqual(
                parser.get_default("timeout"),
                E03_WRAPPER_TIMEOUT_SECONDS,
            )
            self.assertGreater(
                parser.get_default("timeout"),
                E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS,
            )

    def test_mode_wrappers_reject_shortened_tail_margin(self):
        shortened = str(E03_WRAPPER_TIMEOUT_SECONDS - 1)
        common = [
            "--label",
            "e03-timeout-test",
            "--api-container",
            "api",
            "--db-container",
            "db",
            "--worker-container",
            "worker",
            "--expect-build-revision",
            "a" * 40,
            "--expect-api-image-id",
            "sha256:" + "b" * 64,
            "--expect-db-image-id",
            "sha256:" + "c" * 64,
            "--timeout",
            shortened,
            "--out",
            "result.json",
        ]
        with patch("sys.argv", [
            "e03_mode2.py",
            *common,
            "--mock-port",
            "55212",
            "--mock-state",
            "mock.state",
            "--mock-log",
            "mock.log",
            "--mock-config",
            "mock.config",
            "--mock-instance-id",
            "e03-mode2-timeout-test",
            "--expected-openai-base-url",
            "http://host.docker.internal:55212/v1",
        ]):
            with self.assertRaisesRegex(ValueError, "30-minute"):
                mode2_main()
        with patch("sys.argv", [
            "e03_mode3.py",
            *common,
            "--proxy-port",
            "55213",
            "--proxy-state",
            "proxy.state",
            "--proxy-config",
            "proxy.config",
            "--proxy-log",
            "proxy.log",
            "--proxy-instance-id",
            "e03-mode3-timeout-test",
            "--expected-proxy-base-url",
            "http://host.docker.internal:55213/v1",
        ]):
            with self.assertRaisesRegex(ValueError, "30-minute"):
                mode3_main()

    def test_parser_requires_profile_and_arm_to_be_validated_together(self):
        parser = build_parser()
        parsed = parser.parse_args([
            "run",
            "--label",
            "e03",
            "--gate-profile",
            "e03-semantic-ready",
            "--e03-arm",
            "mode3",
            "--out",
            "result.json",
        ])
        self.assertEqual(parsed.e03_arm, "mode3")
        with self.assertRaisesRegex(ValueError, "requires --gate-profile"):
            args = request_namespace("mode1")
            args.gate_profile = None
            validate_e03_request(
                args,
                ["exact", "lexical"],
                {**E03_COMMON_RUNTIME_EXPECTATIONS, "semantic_lane": False},
            )

    def test_each_arm_requires_its_exact_posture(self):
        for arm, modes in (
            ("mode1", ["exact", "lexical"]),
            ("mode2", ["exact", "lexical", "semantic"]),
            ("mode3", ["exact", "lexical", "semantic"]),
        ):
            validate_e03_request(
                request_namespace(arm),
                modes,
                {
                    **E03_COMMON_RUNTIME_EXPECTATIONS,
                    "semantic_lane": arm != "mode1",
                },
            )
        args = request_namespace("mode1")
        args.verbatim_feature_acceptance = "required"
        with self.assertRaisesRegex(
            ValueError,
            "verbatim-feature-acceptance not-applicable",
        ):
            validate_e03_request(
                args,
                ["exact", "lexical"],
                {**E03_COMMON_RUNTIME_EXPECTATIONS, "semantic_lane": False},
            )
        args = request_namespace("mode3")
        args.unique_queries = True
        with self.assertRaisesRegex(ValueError, "built-in paired"):
            validate_e03_request(
                args,
                ["exact", "lexical", "semantic"],
                {**E03_COMMON_RUNTIME_EXPECTATIONS, "semantic_lane": True},
            )

    def test_runtime_provider_is_arm_specific(self):
        validate_e03_runtime_metadata("mode1", {
            "embeddings": {
                "provider": "hashing",
                "model": "straylight-hashing-v1",
                "dimensions": 1536,
                "status": "degraded",
            }
        })
        validate_e03_runtime_metadata("mode3", {
            "embeddings": {
                "provider": "openai",
                "model": "text-embedding-3-small",
                "dimensions": 1536,
                "status": "ready",
            }
        })
        with self.assertRaisesRegex(ValueError, "provider"):
            validate_e03_runtime_metadata("mode2", {
                "embeddings": {
                    "provider": "hashing",
                    "model": "straylight-hashing-v1",
                    "dimensions": 1536,
                    "status": "degraded",
                }
            })
        with self.assertRaisesRegex(ValueError, "model"):
            validate_e03_runtime_metadata("mode3", {
                "embeddings": {
                    "provider": "openai",
                    "model": "text-embedding-3-large",
                    "dimensions": 1536,
                    "status": "ready",
                }
            })
        with self.assertRaisesRegex(ValueError, "dimensions"):
            validate_e03_runtime_metadata("mode3", {
                "embeddings": {
                    "provider": "openai",
                    "model": "text-embedding-3-small",
                    "dimensions": 3072,
                    "status": "ready",
                }
            })

    def test_profile_rejects_sample_count_that_can_outgrow_cost_budget(self):
        args = request_namespace("mode3")
        args.samples = 31
        with self.assertRaisesRegex(ValueError, "exactly 30"):
            validate_e03_request(
                args,
                ["exact", "lexical", "semantic"],
                {**E03_COMMON_RUNTIME_EXPECTATIONS, "semantic_lane": True},
            )

    def test_mode3_requires_official_upstream_and_complete_usage_receipt(self):
        self.assertEqual(
            normalize_official_upstream("https://api.openai.com/v1/"),
            "https://api.openai.com/v1",
        )
        with self.assertRaisesRegex(ValueError, "exact upstream"):
            normalize_official_upstream("https://embeddings.example/v1")
        status = SimpleNamespace(
            returncode=0,
            stdout=json.dumps({
                "state": {
                    "instance_id": "e03-test",
                    "implementation_sha256": "b" * 64,
                    "upstream_base_url_sha256": "c" * 64,
                },
                "behavior": {
                    "mode": "forward",
                    "delay_ms": 0,
                    "error_status": 0,
                },
                "usage": {
                    "schema": "straylight-openai-embedding-usage-receipt@v1",
                    "successful_embedding_responses": 2,
                    "usage_reported_responses": 2,
                    "usage_missing_responses": 0,
                    "prompt_tokens": 1_000,
                    "total_tokens": 1_000,
                },
            }),
        )
        args = SimpleNamespace(
            proxy_instance_id="e03-test",
            proxy_implementation_sha256="b" * 64,
            upstream_base_url_sha256="c" * 64,
        )
        receipt = usage_receipt_contract(
            status,
            args,
            {
                "e03_embedding_cost": {
                    "preflight": {"estimated_max_usd": 2.5}
                }
            },
        )
        self.assertTrue(receipt["pass"])
        self.assertEqual(receipt["actual_usd"], 0.00002)
        missing = json.loads(status.stdout)
        missing["usage"]["usage_reported_responses"] = 1
        missing["usage"]["usage_missing_responses"] = 1
        incomplete = usage_receipt_contract(
            SimpleNamespace(returncode=0, stdout=json.dumps(missing)),
            args,
            {
                "e03_embedding_cost": {
                    "preflight": {"estimated_max_usd": 2.5}
                }
            },
        )
        self.assertFalse(incomplete["pass"])

    def test_mode3_container_host_route_is_exact_and_platform_bounded(self):
        self.assertTrue(container_host_route_allowed("172.28.233.1"))
        self.assertTrue(container_host_route_allowed("host.docker.internal"))
        self.assertFalse(container_host_route_allowed("localhost"))
        self.assertFalse(
            container_host_route_allowed("host.docker.internal.example")
        )

    @patch("eval.e03_mode2.run_command")
    def test_mode2_network_probe_runs_in_exact_container_namespace(
        self,
        run_command,
    ):
        mock_config = Path("/tmp/e03-mode2-test-config.json")
        implementation = hashlib.sha256(MOCK.read_bytes()).hexdigest()
        config_digest = hashlib.sha256(
            str(mock_config.resolve()).encode("utf-8")
        ).hexdigest()
        expected_identity = {
            "instance_id": "e03-mode2-test",
            "implementation_sha256": implementation,
            "host": "0.0.0.0",
            "port": 19235,
            "config_path_sha256": config_digest,
            "pid": 1234,
        }
        run_command.return_value = SimpleNamespace(
            returncode=0,
            stdout=json.dumps({
                "schema": "straylight-mock-openai-embeddings-health@v1",
                "status": "ok",
                "behavior": {"delay_ms": 0, "error_status": 0},
                "identity": expected_identity,
            }),
        )
        args = SimpleNamespace(
            expected_openai_base_url=(
                "http://host.docker.internal:19235/v1"
            ),
            probe_image="busybox@sha256:" + "a" * 64,
            mock_instance_id="e03-mode2-test",
            mock_port=19235,
            mock_config=mock_config,
        )
        endpoint = {
            "api": {"container_id": "api-container-id"},
            "probe_image": {"image_id": "sha256:" + "b" * 64},
        }
        result = mock_network_probe(
            args,
            endpoint,
            role="api",
            expected_identity=expected_identity,
        )
        self.assertTrue(result["pass"])
        command = run_command.call_args.args[0]
        self.assertIn("container:api-container-id", command)
        self.assertEqual(
            command[-1],
            "http://host.docker.internal:19235/health",
        )
        self.assertIn("--pull", command)
        self.assertIn("never", command)

    def test_e03_api_route_binds_exact_loopback_port_and_named_db(self):
        api = {
            "Id": "api-id",
            "Config": {
                "Labels": {
                    "com.docker.compose.project": "e03-one",
                    "com.docker.compose.service": "api",
                },
                "Env": [
                    "STRAYLIGHT_DATABASE_URL=postgres://u:p@db:5432/straylight",
                    (
                        "STRAYLIGHT_READ_ONLY_DATABASE_URL="
                        "postgres://r:p@db:5432/straylight"
                    ),
                ],
            },
            "State": {"Running": True},
            "NetworkSettings": {
                "Networks": {
                    "e03-one_default": {
                        "Aliases": ["api"],
                        "Gateway": "172.31.0.1",
                    }
                },
                "Ports": {
                    "8080/tcp": [{
                        "HostIp": "127.0.0.1",
                        "HostPort": "18142",
                    }]
                },
            },
        }
        db = {
            "Id": "db-id",
            "Config": {
                "Labels": {
                    "com.docker.compose.project": "e03-one",
                    "com.docker.compose.service": "db",
                },
                "Env": [],
            },
            "State": {"Running": True},
            "NetworkSettings": {
                "Networks": {
                    "e03-one_default": {
                        "Aliases": ["e03-one-db-1", "db"],
                        "Gateway": "172.31.0.1",
                    }
                },
                "Ports": {},
            },
        }

        def inspect(command, **_kwargs):
            record = api if command[-1] == "api" else db
            return SimpleNamespace(
                returncode=0,
                stdout=json.dumps([record]),
            )

        with patch("performance_eval.subprocess.run", side_effect=inspect):
            binding = e03_api_route_binding(
                api_base_url="http://127.0.0.1:18142",
                api_container="api",
                db_container="db",
            )
            self.assertTrue(binding["pass"])
            with self.assertRaisesRegex(ValueError, "route mismatch"):
                e03_api_route_binding(
                    api_base_url="http://127.0.0.1:18143",
                    api_container="api",
                    db_container="db",
                )

    def test_complete_zero_of_30_is_integrity_green_and_feature_red(self):
        scale = scale_fixture()
        evidence = verbatim_identifier_measurement_evidence(scale)
        self.assertTrue(evidence["pass"])
        gates = evaluate_gates(
            [scale],
            DEFAULT_THRESHOLDS,
            require_gin_index=False,
            verbatim_feature_acceptance_required=False,
        )
        gates, findings = apply_e03_gate_policy(
            [scale],
            gates,
            arm="mode2",
        )
        by_name = {gate["name"]: gate for gate in gates}
        self.assertTrue(
            by_name["verbatim_identifier_measurement_integrity"]["pass"]
        )
        self.assertNotIn("verbatim_identifier", by_name)
        finding = {
            item["name"]: item for item in findings
        }["verbatim_identifier_feature_acceptance"]
        self.assertFalse(finding["pass"])
        self.assertEqual(finding["outcome"], "red")

    def test_partial_verbatim_rows_fail_measurement_integrity(self):
        scale = scale_fixture()
        scale.update(verbatim_measurement(rows=29))
        evidence = verbatim_identifier_measurement_evidence(scale)
        self.assertFalse(evidence["pass"])
        self.assertFalse(
            evidence["checks"]["results_have_exactly_30_rows"]
        )

    def test_mode3_moves_regression_tiers_but_keeps_hard_slo(self):
        scale = scale_fixture(search_p95_ms=600.0)
        gates = evaluate_gates(
            [scale],
            DEFAULT_THRESHOLDS,
            require_gin_index=False,
            verbatim_feature_acceptance_required=False,
        )
        gates, findings = apply_e03_gate_policy(
            [scale],
            gates,
            arm="mode3",
        )
        gate_names = {item["name"] for item in gates}
        self.assertFalse(any(
            name.startswith("regression_tier_") for name in gate_names
        ))
        self.assertIn("every_scale_search_p95_ms", gate_names)
        self.assertTrue(any(
            item["name"].startswith("regression_tier_")
            for item in findings
        ))
        self.assertIn("mode3_cold_warm_pair_is_single_corpus", gate_names)

    def mode1_zero_vector_scale(self, semantic_plan):
        scale = scale_fixture()
        scale["retrieval_plan_assertions"] = {
            "status": "complete",
            "pass": False,
            "sql_drift": [
                {"lane": "lexical", "pass": True},
                {"lane": "semantic", "pass": True},
            ],
            "lanes": {
                "lexical": {"plan_assertion": {"pass": True}},
                "semantic": {
                    "plan_assertion": {
                        "pass": False,
                        "lane": "semantic",
                        "expected": {
                            "node_type": "Index Scan",
                            "index_name": (
                                "search_chunks_embedding_hnsw_idx"
                            ),
                            "no_seq_scan_on": "search_chunks",
                        },
                        "matched": [],
                        "forbidden": [],
                    },
                    "function_owner_body_explain": [semantic_plan],
                },
            },
        }
        scale["e03_mode1_pending"] = {
            "pass": True,
            "before_sampling": {
                "database": {
                    "chunks": 128000,
                    "semantic_ready_chunks": 0,
                    "pending_chunks": 128000,
                },
            },
            "after_sampling": {
                "database": {
                    "chunks": 128032,
                    "semantic_ready_chunks": 0,
                    "pending_chunks": 128032,
                },
            },
        }
        return scale

    def test_mode1_requires_lexical_plan_but_not_empty_hnsw_plan(self):
        scale = self.mode1_zero_vector_scale({
            "Plan": {
                "Node Type": "Bitmap Heap Scan",
                "Relation Name": "search_chunks",
                "Recheck Cond": "embedding IS NOT NULL",
                "Plans": [{
                    "Node Type": "Bitmap Index Scan",
                    "Index Name": "search_chunks_semantic_coverage_idx",
                }],
            },
        })
        gates = evaluate_gates(
            [scale],
            DEFAULT_THRESHOLDS,
            require_gin_index=False,
            verbatim_feature_acceptance_required=False,
        )
        gates, findings = apply_e03_gate_policy(
            [scale],
            gates,
            arm="mode1",
        )
        by_name = {gate["name"]: gate for gate in gates}
        self.assertTrue(
            by_name[
                f"retrieval_plan_assertions_at_{PRODUCTION_RECORDS}"
            ]["pass"]
        )
        finding = {
            item["name"]: item for item in findings
        }["mode1_semantic_plan_is_inapplicable_without_ready_vectors"]
        self.assertIsNone(finding["pass"])
        self.assertEqual(finding["outcome"], "not_applicable")

    def test_mode1_accepts_direct_empty_semantic_coverage_index_scan(self):
        scale = self.mode1_zero_vector_scale({
            "Plan": {
                "Node Type": "Index Scan",
                "Relation Name": "search_chunks",
                "Index Name": "search_chunks_semantic_coverage_idx",
                "Filter": "(path !~~ '.straylight/checkpoints/%'::text)",
            },
        })
        gates = evaluate_gates(
            [scale],
            DEFAULT_THRESHOLDS,
            require_gin_index=False,
            verbatim_feature_acceptance_required=False,
        )
        gates, findings = apply_e03_gate_policy(
            [scale],
            gates,
            arm="mode1",
        )
        by_name = {gate["name"]: gate for gate in gates}
        self.assertTrue(
            by_name[
                f"retrieval_plan_assertions_at_{PRODUCTION_RECORDS}"
            ]["pass"]
        )
        self.assertIn(
            "mode1_semantic_plan_is_inapplicable_without_ready_vectors",
            {finding["name"] for finding in findings},
        )

    def test_mode1_does_not_waive_unsafe_semantic_plans(self):
        for unsafe_plan in (
            {
                "Plan": {
                    "Node Type": "Seq Scan",
                    "Relation Name": "search_chunks",
                },
            },
            {
                "Plan": {
                    "Node Type": "Index Scan",
                    "Relation Name": "search_chunks",
                    "Index Name": "wrong_semantic_index",
                },
            },
            {
                "Plan": {
                    "Node Type": "Bitmap Heap Scan",
                    "Relation Name": "search_chunks",
                    "Recheck Cond": "embedding IS NOT NULL",
                },
            },
            {
                "Plan": {
                    "Node Type": "Bitmap Heap Scan",
                    "Relation Name": "search_chunks",
                    "Recheck Cond": "embedding IS NOT NULL",
                    "Plans": [{
                        "Node Type": "Bitmap Index Scan",
                        "Index Name": "search_chunks_wrong_index",
                    }],
                },
            },
            {
                "Plan": {
                    "Node Type": "Nested Loop",
                    "Plans": [
                        {
                            "Node Type": "Index Scan",
                            "Relation Name": "search_chunks",
                            "Index Name": (
                                "search_chunks_semantic_coverage_idx"
                            ),
                        },
                        {
                            "Node Type": "Seq Scan",
                            "Relation Name": "search_chunks",
                        },
                    ],
                },
            },
        ):
            with self.subTest(plan=unsafe_plan):
                scale = self.mode1_zero_vector_scale(unsafe_plan)
                has_seq_scan = any(
                    node.get("Node Type") == "Seq Scan"
                    and node.get("Relation Name") == "search_chunks"
                    for node in iter_plan_nodes(unsafe_plan)
                )
                forbidden = (
                    [{
                        "node_type": "Seq Scan",
                        "relation": "search_chunks",
                    }]
                    if has_seq_scan
                    else []
                )
                scale["retrieval_plan_assertions"]["lanes"]["semantic"][
                    "plan_assertion"
                ]["forbidden"] = forbidden
                gates = evaluate_gates(
                    [scale],
                    DEFAULT_THRESHOLDS,
                    require_gin_index=False,
                    verbatim_feature_acceptance_required=False,
                )
                gates, findings = apply_e03_gate_policy(
                    [scale],
                    gates,
                    arm="mode1",
                )
                by_name = {gate["name"]: gate for gate in gates}
                self.assertFalse(
                    by_name[
                        f"retrieval_plan_assertions_at_{PRODUCTION_RECORDS}"
                    ]["pass"]
                )
                self.assertNotIn(
                    (
                        "mode1_semantic_plan_is_inapplicable_without_"
                        "ready_vectors"
                    ),
                    {finding["name"] for finding in findings},
                )

    def test_mode1_does_not_waive_plan_when_vectors_are_ready(self):
        scale = self.mode1_zero_vector_scale({
            "Plan": {
                "Node Type": "Index Scan",
                "Relation Name": "search_chunks",
                "Index Name": "search_chunks_semantic_coverage_idx",
            },
        })
        scale["e03_mode1_pending"]["after_sampling"]["database"].update({
            "semantic_ready_chunks": 1,
            "pending_chunks": 128031,
        })
        gates = evaluate_gates(
            [scale],
            DEFAULT_THRESHOLDS,
            require_gin_index=False,
            verbatim_feature_acceptance_required=False,
        )
        gates, findings = apply_e03_gate_policy(
            [scale],
            gates,
            arm="mode1",
        )
        by_name = {gate["name"]: gate for gate in gates}
        self.assertFalse(
            by_name[
                f"retrieval_plan_assertions_at_{PRODUCTION_RECORDS}"
            ]["pass"]
        )
        self.assertNotIn(
            "mode1_semantic_plan_is_inapplicable_without_ready_vectors",
            {finding["name"] for finding in findings},
        )

    def test_wrappers_emit_current_profile_and_verbatim_off(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            common = dict(
                label="e03",
                api_container="api",
                db_container="db",
                worker_container="worker",
                expect_build_revision="a" * 40,
                out=root / "result.json",
                samples=30,
            )
            mode2 = mode2_command(argparse.Namespace(
                **common,
                mock_port=55212,
                mock_state=root / "mock.state",
                mock_log=root / "mock.log",
                mock_config=root / "mock.config",
                mock_instance_id="e03-mode2-test",
                failure_settle_seconds=0.1,
                quick=False,
                future_soak=False,
                scales=[64_000],
                feature_state=[],
                expect_feature_flag=[],
                expect_runtime_config=[],
                query_budget_profile="default-safe",
                query_budget_contract=None,
            ))
            mode1 = mode1_command(argparse.Namespace(
                **{key: value for key, value in common.items()
                   if key != "worker_container"},
                quick=False,
                future_soak=False,
                scales=[64_000],
                reuse_flat_controls_from=None,
            ))
            mode3_args = argparse.Namespace(
                **common,
                proxy_port=55213,
                proxy_state=root / "proxy.state",
                proxy_config=root / "proxy.config",
                proxy_log=root / "proxy.log",
                proxy_instance_id="e03-test",
                upstream_base_url="https://api.openai.com/v1",
                control_token_file=None,
                proxy_implementation_sha256="b" * 64,
                upstream_base_url_sha256="c" * 64,
            )
            mode3 = mode3_command(mode3_args)
        for command, arm in (
            (mode1, "mode1"),
            (mode2, "mode2"),
            (mode3, "mode3"),
        ):
            self.assertIn("e03-semantic-ready", command)
            self.assertIn(arm, command)
            self.assertIn("verbatim_spans=off", command)
            self.assertEqual(
                command[
                    command.index("--verbatim-feature-acceptance") + 1
                ],
                "not-applicable",
            )
        self.assertNotIn("--unique-queries", mode3)
        self.assertEqual(
            mode3[mode3.index("--scales") + 1],
            "64000",
        )


if __name__ == "__main__":
    unittest.main()

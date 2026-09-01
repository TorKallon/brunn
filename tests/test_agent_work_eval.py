import hashlib
import json
import os
import re
import subprocess
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from agent_work_eval import (  # noqa: E402
    aggregate_adoption_measurements,
    attach_workspace_metrics,
    build_run_ledger,
    build_codex_command,
    candidate_matches,
    capture_service_image_fingerprint,
    capture_service_runtime_snapshot,
    canonical_json_sha256,
    definitive_service_run_provenance,
    ensure_run_binding,
    evaluation_source_fingerprint,
    experiment_provenance_lines,
    forbidden_is_asserted,
    expected_feature_flags,
    expected_runtime_features,
    grade_answer,
    git_source_fingerprint,
    normalize,
    parse_feature_states,
    parse_event_metrics,
    require_codex_subscription,
    render_fixed_context,
    render_prompt,
    read_sidecar_checkpoint,
    response_reports_semantic_gap,
    resolve_codex_path,
    resolve_service_retrieval_modes,
    select_cases,
    summarize,
    stable_service_image_provenance,
    subscription_reasoning_environment,
    summarize_local_cli_failures,
    summarize_service_accounting,
    validate_experiment_identity,
    validate_native_service_accounting,
    load_native_provisioning_state,
    measure_adoption,
    validate,
    write_native_provisioning_state,
    build_parser as build_agent_parser,
)
from brunn_eval import BM25Index  # noqa: E402
from workspace_cli import corpus_hash, load_corpus, safe_compute  # noqa: E402
from native_memory import authored_frontmatter  # noqa: E402
from transition_eval import select_transition_cases  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]


def definitive_service_run_fixture(
    *,
    run_id="run-1",
    arm="e07-adoption",
    draw="e07-adoption-draw1",
    benchmark="frontmatter-adoption-v0.1",
    manifest_sha256="b" * 64,
    case_ids=("case-a",),
    runtime_features=None,
):
    revision = "a" * 40
    features = runtime_features or {
        "semantic_lane": False,
        "verbatim_spans": False,
        "supersession_demotion": True,
        "intention_ledger": False,
        "resume_deltas": False,
    }
    expected_features = dict(features)
    runtime_snapshot = {
        "schema": "brunn-service-runtime-snapshot@v1",
        "captured_at": "2026-07-28T12:00:00-07:00",
        "status": "ready",
        "build_revision": revision,
        "corpus_revision": "generation:1",
        "revision_sequence": 1,
        "read_only": False,
        "runtime_features": features,
        "embeddings": {"enabled": False},
    }
    image = {
        "schema": "brunn-service-image-fingerprint@v1",
        "api_container": "api-container",
        "api_container_id": "container-id",
        "api_container_started_at": "2026-07-28T11:00:00Z",
        "api_container_running": True,
        "api_image_id": "sha256:" + "c" * 64,
        "api_image_revision": revision,
    }
    image_provenance = {
        "schema": "brunn-service-image-provenance@v1",
        "stable": True,
        "before": image,
        "after": dict(image),
    }
    parameters = {
        "declared_feature_states": {},
        "e09_arm": None,
        "e09_step_authorization": None,
        "run_tags": [],
        "service_retrieval_modes": ["exact", "lexical"],
        "service_image_fingerprint": image,
    }
    billing = {
        "route": "chatgpt_subscription",
        "api_fallback": "forbidden",
        "codex_path": "/opt/codex",
        "codex_version": "codex-test",
        "auth_checked_at": "2026-07-28T11:30:00-07:00",
        "auth_status": "Logged in using ChatGPT",
    }
    artifacts = {
        "manifest_sha256": manifest_sha256,
        "schema_sha256": "d" * 64,
        "harness_sha256": "e" * 64,
        "runtime_snapshot_sha256": canonical_json_sha256(runtime_snapshot),
        "service_image_provenance_sha256": canonical_json_sha256(
            image_provenance
        ),
    }
    operation = {
        "operation": "open",
        "result_chars": 20,
        "source_text_chars": 10,
        "metadata_chars": 10,
        "elapsed_ms": 2.0,
        "http_status": 200,
    }
    return {
        "benchmark_version": benchmark,
        "run_id": run_id,
        "experiment_arm": arm,
        "paired_draw_id": draw,
        "manifest_sha256": manifest_sha256,
        "schema_sha256": artifacts["schema_sha256"],
        "harness_sha256": artifacts["harness_sha256"],
        "manifest": {
            "benchmark_version": benchmark,
            "conditions": ["service_api"],
            "cases": [{"id": case_id} for case_id in case_ids],
        },
        "service_protocol": "simple",
        "service_retrieval_modes": ["exact", "lexical"],
        "experiment_parameters": parameters,
        "reasoning_billing": billing,
        "implementation_fingerprint": {
            "source_revision": revision,
            "tracked_source_clean": True,
            "untracked_source_files": [],
            "reproducible_source": True,
        },
        "expected_runtime_features": expected_features,
        "expected_build_revision": revision,
        "service_runtime_snapshot": runtime_snapshot,
        "service_image_provenance": image_provenance,
        "records": [
            {
                "case_id": case_id,
                "condition": "service_api",
                "error": None,
                "grade": {"pass": True},
                "service_accounting_valid": True,
                "service_operations": [dict(operation)],
                "local_cli_failures": (
                    summarize_local_cli_failures([operation])
                ),
                **summarize_service_accounting([operation]),
            }
            for case_id in case_ids
        ],
        "run_ledger": {
            "schema": "brunn-eval-run-ledger@v1",
            "run_id": run_id,
            "source": {
                "revision": revision,
                "tracked_source_clean": True,
                "untracked_source_files": [],
                "clean": True,
            },
            "codex": {
                "auth_route": billing["route"],
                "api_fallback": billing["api_fallback"],
                "auth_status": billing["auth_status"],
            },
            "configuration": {
                "conditions": ["service_api"],
                "service_protocol": "simple",
                "experiment_arm": arm,
                "paired_draw_id": draw,
                "expected_runtime_features": expected_features,
                "expected_build_revision": revision,
                "experiment_parameters": parameters,
            },
            "artifacts": artifacts,
        },
    }


class AgentWorkEvalTests(unittest.TestCase):
    def test_service_image_fingerprint_is_running_source_bound_and_stable(self):
        revision = "a" * 40
        values = {
            "{{.Id}}": "container-id",
            "{{.State.StartedAt}}": "2026-07-28T11:00:00Z",
            "{{.State.Running}}": "true",
            "{{.Image}}": "sha256:" + "b" * 64,
            (
                '{{index .Config.Labels '
                '"org.opencontainers.image.revision"}}'
            ): revision,
        }

        def inspect(command, **kwargs):
            template = command[2].removeprefix("--format=")
            return subprocess.CompletedProcess(
                command,
                0,
                stdout=values[template] + "\n",
                stderr="",
            )

        with patch("agent_work_eval.subprocess.run", side_effect=inspect):
            fingerprint = capture_service_image_fingerprint(
                "api-container",
                source_revision=revision,
                expected_build_revision=revision,
            )
        self.assertTrue(fingerprint["api_container_running"])
        self.assertEqual(
            fingerprint["api_image_id"],
            "sha256:" + "b" * 64,
        )
        provenance = stable_service_image_provenance(
            fingerprint,
            dict(fingerprint),
        )
        self.assertTrue(provenance["stable"])
        changed = dict(fingerprint)
        changed["api_container_id"] = "replacement-container"
        with self.assertRaisesRegex(ValueError, "drifted"):
            stable_service_image_provenance(fingerprint, changed)

    def test_definitive_service_provenance_rejects_image_or_ledger_drift(self):
        run = definitive_service_run_fixture()
        provenance = definitive_service_run_provenance(run)
        self.assertEqual(provenance["experiment_arm"], "e07-adoption")
        self.assertEqual(provenance["service_retrieval_modes"], [
            "exact",
            "lexical",
        ])

        drifted = json.loads(json.dumps(run))
        drifted["service_image_provenance"]["after"][
            "api_container_id"
        ] = "replacement"
        with self.assertRaisesRegex(ValueError, "drifted"):
            definitive_service_run_provenance(drifted)

    def test_definitive_service_provenance_rejects_measured_failures(self):
        run = definitive_service_run_fixture()
        failed_operation = {
            "operation": "denied:checkpoint",
            "result_chars": 30,
            "source_text_chars": 0,
            "metadata_chars": 30,
            "elapsed_ms": 2.0,
            "http_status": 403,
            "service_status": "capability_denied",
        }
        run["records"][0]["service_operations"] = [failed_operation]
        with self.assertRaisesRegex(ValueError, "measured failed or denied"):
            definitive_service_run_provenance(run)

        run["manifest"]["cases"][0]["native_failure_contract"] = {
            "schema": "brunn-native-failure-contract@v1",
            "expected_failures": [{
                "operation": "denied:checkpoint",
                "http_status": 403,
                "service_status": "capability_denied",
                "occurrences": 1,
            }],
        }
        with self.assertRaisesRegex(ValueError, "measured failed or denied"):
            definitive_service_run_provenance(run)

    def test_definitive_service_provenance_allows_only_recovered_local_failures(
        self,
    ):
        run = definitive_service_run_fixture()
        local_failure = {
            "operation": "failed:open",
            "result_chars": 30,
            "source_text_chars": 0,
            "metadata_chars": 30,
            "elapsed_ms": 0,
            "http_status": 0,
            "service_status": "invalid_request",
        }
        record = run["records"][0]
        successful_open = record["service_operations"][0]
        record["service_operations"] = [
            local_failure,
            successful_open,
        ]
        record["local_cli_failures"] = summarize_local_cli_failures(
            record["service_operations"]
        )
        provenance = definitive_service_run_provenance(run)
        self.assertEqual(provenance["run_id"], run["run_id"])

        tampered = json.loads(json.dumps(run))
        tampered["records"][0]["local_cli_failures"]["result_chars"] += 1
        with self.assertRaisesRegex(
            ValueError,
            "inconsistent local_cli_failures",
        ):
            definitive_service_run_provenance(tampered)

        inflated_service_calls = json.loads(json.dumps(run))
        inflated_service_calls["records"][0]["service_calls"] += 1
        with self.assertRaisesRegex(
            ValueError,
            "local-failure-excluding service accounting",
        ):
            definitive_service_run_provenance(inflated_service_calls)

        unrecovered = json.loads(json.dumps(run))
        unrecovered_record = unrecovered["records"][0]
        unrecovered_record["service_operations"] = [local_failure]
        unrecovered_record["local_cli_failures"] = (
            summarize_local_cli_failures([local_failure])
        )
        with self.assertRaisesRegex(ValueError, "unrecovered local CLI"):
            definitive_service_run_provenance(unrecovered)

        out_of_order = json.loads(json.dumps(run))
        out_of_order_record = out_of_order["records"][0]
        out_of_order_record["service_operations"] = [
            successful_open,
            local_failure,
        ]
        out_of_order_record["local_cli_failures"] = (
            summarize_local_cli_failures(
                out_of_order_record["service_operations"]
            )
        )
        with self.assertRaisesRegex(ValueError, "unrecovered local CLI"):
            definitive_service_run_provenance(out_of_order)

        missing_summary = json.loads(json.dumps(run))
        del missing_summary["records"][0]["local_cli_failures"]
        with self.assertRaisesRegex(
            ValueError,
            "missing or inconsistent local_cli_failures",
        ):
            definitive_service_run_provenance(missing_summary)

    def test_git_source_provenance_fails_closed_on_probe_errors(self):
        revision = subprocess.CompletedProcess(
            ["git", "rev-parse", "HEAD"],
            0,
            stdout="a" * 40 + "\n",
            stderr="",
        )
        tracked = subprocess.CompletedProcess(
            ["git", "diff", "--quiet", "HEAD", "--"],
            0,
            stdout="",
            stderr="",
        )
        failed_untracked = subprocess.CompletedProcess(
            ["git", "ls-files"],
            128,
            stdout="",
            stderr="index unreadable",
        )
        with patch(
            "agent_work_eval.subprocess.run",
            side_effect=[revision, tracked, failed_untracked],
        ):
            with self.assertRaisesRegex(
                ValueError,
                "untracked source state",
            ):
                git_source_fingerprint()

        with patch(
            "agent_work_eval.subprocess.run",
            side_effect=subprocess.TimeoutExpired(
                ["git", "rev-parse", "HEAD"],
                15,
            ),
        ):
            with self.assertRaisesRegex(
                ValueError,
                "git source provenance",
            ):
                git_source_fingerprint()

        source = {
            "revision": "b" * 40,
            "tracked_source_clean": True,
            "untracked_source_files": [],
            "clean": True,
        }
        with patch(
            "agent_work_eval.git_source_fingerprint",
            return_value=source,
        ):
            self.assertEqual(
                evaluation_source_fingerprint(),
                {
                    "source_revision": "b" * 40,
                    "tracked_source_clean": True,
                    "untracked_source_files": [],
                    "reproducible_source": True,
                },
            )

    def test_service_retrieval_modes_are_explicit_validated_and_parsed(self):
        args = build_agent_parser().parse_args([
            "run",
            "--condition",
            "service_api",
            "--service-protocol",
            "simple",
            "--service-retrieval-modes",
            "exact",
            "lexical",
            "--out",
            "result.json",
        ])
        self.assertEqual(
            args.service_retrieval_modes,
            ["exact", "lexical"],
        )
        self.assertEqual(
            resolve_service_retrieval_modes(
                args.service_retrieval_modes,
                conditions=["service_api"],
                service_protocol="simple",
            ),
            ("exact", "lexical"),
        )
        with self.assertRaisesRegex(ValueError, "must not contain duplicates"):
            resolve_service_retrieval_modes(
                ["exact", "exact"],
                conditions=["service_api"],
                service_protocol="simple",
            )
        with self.assertRaisesRegex(ValueError, "requires the service_api"):
            resolve_service_retrieval_modes(
                ["exact", "lexical"],
                conditions=["filesystem"],
                service_protocol="simple",
            )
        with self.assertRaisesRegex(ValueError, "requires --service-protocol"):
            resolve_service_retrieval_modes(
                ["exact", "lexical"],
                conditions=["service_api"],
                service_protocol="legacy",
            )
        with self.assertRaisesRegex(
            ValueError,
            "explicit semantic_lane=off",
        ):
            resolve_service_retrieval_modes(
                None,
                conditions=["service_api"],
                service_protocol="simple",
                experiment_arm="e08-flag",
                expected_runtime_features={"semantic_lane": False},
            )
        self.assertEqual(
            resolve_service_retrieval_modes(
                ["exact", "lexical"],
                conditions=["service_api"],
                service_protocol="simple",
                experiment_arm="e08-flag",
                expected_runtime_features={"semantic_lane": False},
            ),
            ("exact", "lexical"),
        )

    def test_service_retrieval_modes_preserve_e09_fail_closed_policy(self):
        self.assertEqual(
            resolve_service_retrieval_modes(
                None,
                conditions=["service_api"],
                service_protocol="simple",
                e09_arm="no_semantic",
            ),
            ("exact", "lexical"),
        )
        with self.assertRaisesRegex(ValueError, "conflicts with"):
            resolve_service_retrieval_modes(
                ["exact"],
                conditions=["service_api"],
                service_protocol="simple",
                e09_arm="no_semantic",
            )

    def test_agent_parser_exposes_only_the_policy_gated_600ms_step(self):
        args = build_agent_parser().parse_args([
            "--manifest",
            str(ROOT / "eval" / "work_cases.json"),
            "run",
            "--condition",
            "service_api",
            "--e09-arm",
            "deadline_cache_600",
            "--experiment-arm",
            "e09-deadline-cache-600",
            "--paired-draw-id",
            "e09-work-draw1",
            "--e09-step-policy",
            "results/step-policy.json",
            "--e09-step-policy-sha256",
            "a" * 64,
            "--out",
            "result.json",
        ])
        self.assertEqual(args.e09_arm, "deadline_cache_600")
        self.assertEqual(
            args.e09_step_policy,
            Path("results/step-policy.json"),
        )

    def test_e09_semantic_coverage_probe_rejects_all_gap_shapes(self):
        self.assertTrue(response_reports_semantic_gap({
            "data": {
                "results": [{
                    "lane_failures": ["semantic_deferred"],
                }],
            },
        }))
        self.assertTrue(response_reports_semantic_gap({
            "gaps": [{
                "kind": "retrieval_lane_unavailable",
                "lane": "semantic",
            }],
        }))
        self.assertFalse(response_reports_semantic_gap({
            "data": {"results": [{"candidates": []}]},
        }))

    @classmethod
    def setUpClass(cls):
        cls.manifest = json.loads((ROOT / "eval" / "work_cases.json").read_text())
        cls.documents, cls.chunks = load_corpus(ROOT / cls.manifest["corpus_root"])

    def test_frozen_corpus_shape_and_hash(self):
        self.assertEqual(len(self.documents), 73)
        self.assertGreater(len(self.chunks), 1500)
        self.assertEqual(
            corpus_hash(self.documents),
            "c6ead8b64fd859d6fcf187e15593d8829d3c82efead51488c4eb98533ec6096c",
        )

    def test_codex_path_resolution_skips_a_broken_or_missing_install(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fallback = root / "current-codex"
            fallback.write_text("#!/bin/sh\nexit 0\n")
            fallback.chmod(0o755)
            self.assertEqual(
                resolve_codex_path([root / "missing-codex", fallback]),
                fallback,
            )

    def test_e05_targeted_manifest_is_the_exact_two_case_subset(self):
        validated = validate(
            ROOT / "eval" / "e05_targeted_cases.json",
            ROOT / "eval" / "work_answer_schema.json",
        )
        self.assertEqual(validated["errors"], [])
        self.assertEqual(
            {case["id"] for case in validated["manifest"]["cases"]},
            {"star-rupture-plan-revision", "warmind-parser-learning"},
        )
        self.assertEqual(
            validated["manifest"]["experiment_tags"],
            ["E05", "lexical-consolidation", "targeted"],
        )

    def test_agent_run_feature_states_are_normalized(self):
        self.assertEqual(
            parse_feature_states(["lexical_single_scan=off"]),
            {"lexical_single_scan": False},
        )
        with self.assertRaisesRegex(ValueError, "unknown service feature"):
            parse_feature_states(["unreported_experiment=on"])

    def test_reasoning_environment_never_forwards_paid_api_credentials(self):
        env = subscription_reasoning_environment({
            "PATH": "/usr/bin:/bin",
            "OPENAI_API_KEY": "embedding-key",
            "OPENAI_BASE_URL": "https://paid-api.example.test/v1",
            "AZURE_OPENAI_API_KEY": "alternate-paid-key",
            "FUTURE_PROVIDER_API_KEY": "future-paid-key",
            "CODEX_BASE_URL": "https://alternate-route.example.test",
            "CODEX_API_KEY": "paid-reasoning-key",
            "BRUNN_STATE_EVAL_DIRECT_OPENAI": "1",
        })
        self.assertEqual(env["PATH"], "/usr/bin:/bin")
        self.assertNotIn("OPENAI_API_KEY", env)
        self.assertNotIn("OPENAI_BASE_URL", env)
        self.assertNotIn("AZURE_OPENAI_API_KEY", env)
        self.assertNotIn("FUTURE_PROVIDER_API_KEY", env)
        self.assertNotIn("CODEX_BASE_URL", env)
        self.assertNotIn("CODEX_API_KEY", env)
        self.assertNotIn("BRUNN_STATE_EVAL_DIRECT_OPENAI", env)

    def test_adoption_measurement_uses_bounded_authored_frontmatter_receipts(self):
        frontmatter = authored_frontmatter({
            "content": (
                "---\nsupersedes:\n"
                "  - Projects/Old.md\n"
                "kind: intention\n"
                "trigger: [gmail, oauth-scopes]\n"
                "due: 2026-08-03\n"
                "status: pending\n---\n# Correction"
            )
        })
        self.assertEqual(frontmatter["supersedes"], ["Projects/Old.md"])
        self.assertEqual(frontmatter["trigger"], ["gmail", "oauth-scopes"])
        self.assertEqual(authored_frontmatter({"content": "---\nkind: intention\n"}), {})

        cases = []
        records = []
        for index in range(6):
            case_id = f"correction-{index}"
            cases.append({
                "id": case_id,
                "adoption_eligibility": {
                    "feature": "supersession",
                    "supersedes_path": "Projects/Old.md",
                },
            })
            records.append({
                "case_id": case_id,
                "condition": "service_api",
                "service_operations": [{
                    "operation": "save",
                    "write_path": "Projects/New.md",
                    "authored_frontmatter": frontmatter,
                }],
            })
        for index in range(6):
            case_id = f"intention-{index}"
            cases.append({
                "id": case_id,
                "adoption_eligibility": {"feature": "intention"},
            })
            records.append({
                "case_id": case_id,
                "condition": "service_api",
                "service_operations": [{
                    "operation": "save",
                    "write_path": "Intentions/New.md",
                    "authored_frontmatter": frontmatter,
                }],
            })
        run = {
            "run_id": "adoption-run-1",
            "benchmark_version": "frontmatter-adoption-v0.1",
            "manifest": {"cases": cases},
            "records": records,
        }
        provenance = {
            "experiment_arm": "e07-adoption",
            "paired_draw_id": "e07-adoption-draw1",
            "manifest_sha256": "b" * 64,
        }
        with patch(
            "agent_work_eval.definitive_service_run_provenance",
            return_value=provenance,
        ):
            measurement = measure_adoption(
                run,
                source_artifact_path="/tmp/adoption-raw.json",
                source_artifact_sha256="a" * 64,
                expected_manifest_sha256="b" * 64,
                expected_case_ids=[case["id"] for case in cases],
            )
        self.assertEqual(measurement["eligible_sessions"], 12)
        self.assertEqual(measurement["emitted_sessions"], 12)
        self.assertEqual(measurement["adoption_rate"], 1.0)
        self.assertEqual(
            measurement["by_feature"]["supersession"]["adoption_rate"],
            1.0,
        )

    def test_adoption_aggregate_requires_exact_three_stable_feature_draws(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            measurement_paths = []
            expected_draws = []
            expected_case_ids = [
                f"{feature}-{index}"
                for feature in ("supersession", "intention")
                for index in range(6)
            ]
            for number in range(1, 4):
                draw = f"e07-adoption-draw{number}"
                expected_draws.append(draw)
                run = definitive_service_run_fixture(
                    run_id=f"e07-adoption-run{number}",
                    draw=draw,
                    case_ids=expected_case_ids,
                )
                for case, record in zip(
                    run["manifest"]["cases"],
                    run["records"],
                    strict=True,
                ):
                    feature, raw_index = case["id"].rsplit("-", 1)
                    index = int(raw_index)
                    if feature == "supersession":
                        case["adoption_eligibility"] = {
                            "feature": feature,
                            "supersedes_path": "Projects/Old.md",
                        }
                        frontmatter = {
                            "supersedes": ["Projects/Old.md"],
                        }
                    else:
                        case["adoption_eligibility"] = {
                            "feature": feature,
                        }
                        frontmatter = {
                            "kind": "intention",
                            "status": "pending",
                            "trigger": ["gmail"],
                            "due": "2026-08-03",
                        }
                    operation = {
                        "operation": "save",
                        "result_chars": 20,
                        "source_text_chars": 0,
                        "metadata_chars": 20,
                        "elapsed_ms": 2.0,
                        "http_status": 200,
                        "write_path": f"Notes/{case['id']}.md",
                    }
                    if index < 3:
                        operation["authored_frontmatter"] = frontmatter
                    record["service_operations"] = [operation]
                    record.update(
                        summarize_service_accounting([operation])
                    )
                raw_path = root / f"raw-{number}.json"
                raw_path.write_text(
                    json.dumps(run) + "\n",
                    encoding="utf-8",
                )
                measurement = measure_adoption(
                    run,
                    source_artifact_path=str(raw_path.resolve()),
                    source_artifact_sha256=hashlib.sha256(
                        raw_path.read_bytes()
                    ).hexdigest(),
                    expected_manifest_sha256="b" * 64,
                    expected_case_ids=expected_case_ids,
                )
                measurement_path = root / f"measurement-{number}.json"
                measurement_path.write_text(
                    json.dumps(measurement) + "\n",
                    encoding="utf-8",
                )
                measurement_paths.append(measurement_path)

            aggregate = aggregate_adoption_measurements(
                measurement_paths,
                expected_manifest_sha256="b" * 64,
                expected_case_ids=expected_case_ids,
                expected_draws=expected_draws,
            )
            self.assertTrue(aggregate["pass"])
            self.assertEqual(
                aggregate["by_feature"]["supersession"][
                    "eligible_sessions"
                ],
                18,
            )
            self.assertEqual(
                aggregate["by_feature"]["intention"]["adoption_rate"],
                0.5,
            )

            original_measurement = measurement_paths[0].read_text(
                encoding="utf-8"
            )
            type_tampered = json.loads(original_measurement)
            type_tampered["eligible_sessions"] = 12.0
            measurement_paths[0].write_text(
                json.dumps(type_tampered) + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                ValueError,
                "canonically match",
            ):
                aggregate_adoption_measurements(
                    measurement_paths,
                    expected_manifest_sha256="b" * 64,
                    expected_case_ids=expected_case_ids,
                    expected_draws=expected_draws,
                )

            measurement_paths[0].write_text(
                original_measurement,
                encoding="utf-8",
            )
            tampered = json.loads(original_measurement)
            session = next(
                item
                for item in tampered["sessions"]
                if (
                    item["feature"] == "intention"
                    and not item["emitted_valid_frontmatter"]
                )
            )
            session["emitted_valid_frontmatter"] = True
            tampered["by_feature"]["intention"]["emitted_sessions"] += 1
            tampered["by_feature"]["intention"]["adoption_rate"] = 4 / 6
            tampered["emitted_sessions"] += 1
            tampered["adoption_rate"] = 7 / 12
            measurement_paths[0].write_text(
                json.dumps(tampered) + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                ValueError,
                "canonically match",
            ):
                aggregate_adoption_measurements(
                    measurement_paths,
                    expected_manifest_sha256="b" * 64,
                    expected_case_ids=expected_case_ids,
                    expected_draws=expected_draws,
                )

    def test_feature_flag_preflight_is_explicit_and_fail_closed(self):
        self.assertEqual(
            expected_feature_flags([
                "supersession_demotion=on",
                "intention_ledger=off",
                "read_path_roundtrip_v1=on",
                "lexical_single_scan=off",
                "embedding_backfill_foreground_status_url_configured=on",
            ]),
            {
                "supersession_demotion": True,
                "intention_ledger": False,
                "read_path_roundtrip_v1": True,
                "lexical_single_scan": False,
                "embedding_backfill_foreground_status_url_configured": True,
            },
        )
        with self.assertRaisesRegex(ValueError, "expect-feature-flag"):
            expected_feature_flags(["unknown=on"])

    def test_runtime_snapshot_records_all_current_features_and_detects_drift(self):
        runtime_features = {
            "allow_degraded_embeddings": False,
            "embed_cache": True,
            "embedding_backfill_batch_chunks": 64,
            "embedding_backfill_foreground_status_timeout_ms": 1000,
            "embedding_backfill_foreground_status_url_configured": True,
            "embedding_backfill_guard": True,
            "embedding_backfill_inter_batch_ms": 25,
            "embedding_backfill_open_p95_limit_ms": 150,
            "embedding_backfill_search_p95_limit_ms": 3000,
            "lexical_single_scan": False,
            "read_path_roundtrip_v1": True,
            "resume_deltas": False,
            "search_fair_share": True,
            "search_top1_hydration": True,
            "search_char_cap": True,
            "search_section_demotion_top_n": 8,
            "verbatim_spans": False,
            "supersession_demotion": True,
            "supersession_demotion_weight": 1.5,
            "intention_ledger": False,
            "observability_timings_ms": True,
            "materialize_token_budget": 24000,
            "semantic_deadline_ms": 300,
            "semantic_lane": True,
        }
        status = {
            "status": "ready",
            "build_revision": "a" * 40,
            "corpus_revision": "revision:test",
            "revision_sequence": 7,
            "read_only": False,
            "runtime_features": runtime_features,
            "embeddings": {
                "provider": "mock",
                "model": "mock",
                "dimensions": 8,
                "status": "ready",
            },
            "semantic_runtime": {
                "cache_hits": 2,
                "cache_misses": 3,
                "deferred": 1,
                "embed_requests": 4,
            },
        }
        expected = expected_runtime_features(
            ["search_fair_share=on", "verbatim_spans=off"],
            [
                "embedding_backfill_foreground_status_timeout_ms=1000",
                "search_section_demotion_top_n=8",
            ],
        )
        snapshot = capture_service_runtime_snapshot(
            status,
            expected_features=expected,
            expected_build_revision="a" * 40,
        )
        self.assertEqual(snapshot["runtime_features"], runtime_features)
        self.assertEqual(snapshot["semantic_runtime"], status["semantic_runtime"])
        self.assertEqual(snapshot["build_revision"], "a" * 40)
        provenance = experiment_provenance_lines({
            "experiment_arm": "treatment",
            "paired_draw_id": "draw-1",
            "expected_runtime_features": expected,
            "service_runtime_snapshot": snapshot,
        })
        self.assertIn("- Experiment arm: `treatment`", provenance)
        self.assertTrue(
            any("Actual runtime features" in line for line in provenance)
        )
        with self.assertRaisesRegex(ValueError, "runtime feature mismatch"):
            capture_service_runtime_snapshot(
                status,
                expected_features={"verbatim_spans": True},
                expected_build_revision="a" * 40,
            )
        with self.assertRaisesRegex(ValueError, "runtime feature mismatch"):
            capture_service_runtime_snapshot(
                status,
                expected_features={"search_fair_share": 1},
                expected_build_revision="a" * 40,
            )

    def test_experiment_identity_and_resume_binding_are_fail_closed(self):
        self.assertEqual(
            validate_experiment_identity("flag-on", "draw-1", ["service_api"]),
            ("flag-on", "draw-1"),
        )
        with self.assertRaisesRegex(ValueError, "supplied together"):
            validate_experiment_identity("flag-on", None, ["service_api"])
        with self.assertRaisesRegex(ValueError, "single-condition"):
            validate_experiment_identity(
                "flag-on",
                "draw-1",
                ["service_api", "filesystem"],
            )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binding = {
                "run_id": "run-1",
                "experiment_arm": "flag-on",
                "paired_draw_id": "draw-1",
                "conditions": ["service_api"],
                "case_ids": ["case-a"],
                "model": "gpt-test",
                "service_protocol": "simple",
                "manifest_sha256": "b" * 64,
                "experiment_parameters": {
                    "service_retrieval_modes": ["exact", "lexical"],
                },
            }
            ensure_run_binding(root, resume=False, **binding)
            ensure_run_binding(root, resume=True, **binding)
            with self.assertRaisesRegex(ValueError, "immutable experiment binding"):
                ensure_run_binding(
                    root,
                    resume=True,
                    **{**binding, "experiment_arm": "flag-off"},
                )
            with self.assertRaisesRegex(ValueError, "immutable experiment binding"):
                ensure_run_binding(
                    root,
                    resume=True,
                    **{
                        **binding,
                        "experiment_parameters": {
                            "service_retrieval_modes": ["exact"],
                        },
                    },
                )

    def test_transition_case_selection_is_exact_and_ordered(self):
        manifest = {
            "cases": [
                {"id": "case-a"},
                {"id": "case-b"},
            ]
        }
        self.assertEqual(
            [
                case["id"]
                for case in select_transition_cases(
                    manifest,
                    ["case-b", "case-a", "case-b"],
                )
            ],
            ["case-b", "case-a"],
        )
        with self.assertRaisesRegex(ValueError, "unknown requested transition"):
            select_transition_cases(manifest, ["missing"])

    def test_reasoning_preflight_requires_chatgpt_authentication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            chatgpt = root / "chatgpt-codex"
            chatgpt.write_text(
                (
                    "#!/bin/sh\n"
                    "if [ \"$1\" = \"--version\" ]; then\n"
                    "  echo 'codex-test 1.2.3'\n"
                    "else\n"
                    "  echo 'Logged in using ChatGPT'\n"
                    "fi\n"
                ),
                encoding="utf-8",
            )
            chatgpt.chmod(0o755)
            api_key = root / "api-key-codex"
            api_key.write_text(
                (
                    "#!/bin/sh\n"
                    "if [ \"$1\" = \"--version\" ]; then\n"
                    "  echo 'codex-test 1.2.3'\n"
                    "else\n"
                    "  echo 'Logged in using an API key'\n"
                    "fi\n"
                ),
                encoding="utf-8",
            )
            api_key.chmod(0o755)
            misleading = root / "misleading-codex"
            misleading.write_text(
                (
                    "#!/bin/sh\n"
                    "if [ \"$1\" = \"--version\" ]; then\n"
                    "  echo 'codex-test 1.2.3'\n"
                    "else\n"
                    "  echo 'Not Logged in using ChatGPT'\n"
                    "fi\n"
                ),
                encoding="utf-8",
            )
            misleading.chmod(0o755)

            billing = require_codex_subscription(chatgpt)
            self.assertEqual(billing["route"], "chatgpt_subscription")
            self.assertEqual(billing["api_fallback"], "forbidden")
            self.assertEqual(billing["codex_version"], "codex-test 1.2.3")
            self.assertEqual(billing["auth_status"], "Logged in using ChatGPT")
            self.assertTrue(billing["auth_checked_at"])
            with self.assertRaisesRegex(ValueError, "require Codex logged in"):
                require_codex_subscription(api_key)
            with self.assertRaisesRegex(ValueError, "require Codex logged in"):
                require_codex_subscription(misleading)

    def test_reasoning_preflight_rejects_direct_api_override(self):
        with patch.dict(
            os.environ,
            {"BRUNN_STATE_EVAL_DIRECT_OPENAI": "1"},
            clear=False,
        ):
            with self.assertRaisesRegex(ValueError, "Direct OpenAI reasoning is forbidden"):
                require_codex_subscription(Path("/unused/codex"))

    def test_native_provisioning_state_is_private_resumable_and_run_scoped(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".native-provisioning.json"
            cases = {
                "case-a": {
                    "authorization_scope": "eval:run-a/case-a",
                    "token": "one-time-secret",
                }
            }
            write_native_provisioning_state(path, "run-a", cases)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(load_native_provisioning_state(path, "run-a"), cases)
            with self.assertRaises(ValueError):
                load_native_provisioning_state(path, "run-b")

    def test_native_service_enables_network_without_relaxing_filesystem_sandbox(self):
        common = {
            "codex": Path("/tmp/codex"),
            "model": "gpt-5.6",
            "schema": Path("/tmp/schema.json"),
            "run_dir": Path("/tmp/run"),
        }
        native = build_codex_command(condition="service_api", **common)
        filesystem = build_codex_command(condition="filesystem", **common)

        self.assertIn("sandbox_workspace_write.network_access=true", native)
        self.assertNotIn("sandbox_workspace_write.network_access=true", filesystem)

        for command in (native, filesystem):
            sandbox_index = command.index("--sandbox")
            self.assertEqual(command[sandbox_index + 1], "workspace-write")

    def test_run_ledger_records_clean_source_and_codex_auth_provenance(self):
        billing = {
            "route": "chatgpt_subscription",
            "api_fallback": "forbidden",
            "codex_path": "/opt/codex",
            "codex_version": "codex-test 1.2.3",
            "auth_checked_at": "2026-07-27T12:00:00-07:00",
            "auth_status": "Logged in using ChatGPT",
        }
        source = {
            "revision": "a" * 40,
            "tracked_source_clean": True,
            "untracked_source_files": [],
            "clean": True,
        }
        with patch("agent_work_eval.git_source_fingerprint", return_value=source):
            ledger = build_run_ledger(
                run_id="draw-1",
                reasoning_billing=billing,
                model="gpt-test",
                conditions=["filesystem", "filesystem_sidecar"],
                service_protocol="simple",
                manifest_sha256="b" * 64,
                schema_sha256="c" * 64,
                harness_sha256="d" * 64,
                experiment_parameters={
                    "service_retrieval_modes": ["exact", "lexical"],
                },
            )
        self.assertEqual(ledger["schema"], "brunn-eval-run-ledger@v1")
        self.assertEqual(ledger["source"], source)
        self.assertEqual(ledger["codex"]["path"], "/opt/codex")
        self.assertEqual(
            ledger["codex"]["auth_checked_at"],
            "2026-07-27T12:00:00-07:00",
        )
        self.assertEqual(
            ledger["configuration"]["experiment_parameters"][
                "service_retrieval_modes"
            ],
            ["exact", "lexical"],
        )

    def test_filesystem_sidecar_prompt_and_checkpoint_accounting(self):
        case = self.manifest["cases"][0]
        prompt = render_prompt(case, "filesystem_sidecar")
        self.assertIn("./sidecar/checkpoint.json", prompt)
        self.assertIn("only writable durable-work surface", prompt)
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            (run_dir / "sidecar").mkdir()
            checkpoint = {
                "objective": "Resume safely",
                "current_state": ["Evidence recovered"],
                "decisions": [],
                "open_questions": [],
                "next_actions": ["Continue"],
                "artifacts": ["Evidence.md"],
            }
            path = run_dir / "sidecar" / "checkpoint.json"
            path.write_text(json.dumps(checkpoint) + "\n", encoding="utf-8")
            measured = read_sidecar_checkpoint(run_dir)
            self.assertTrue(measured["valid"])
            self.assertEqual(measured["checkpoint"], checkpoint)
            self.assertEqual(len(measured["sha256"]), 64)
            record = {
                "condition": "filesystem_sidecar",
                "events": {"command_output_chars": 123},
            }
            attach_workspace_metrics(record, run_dir)
            self.assertEqual(record["persisted_checkpoint"], checkpoint)
            self.assertEqual(record["sidecar_checkpoint"], checkpoint)
            self.assertEqual(
                record["response_character_metrics"],
                {
                    "schema": (
                        "brunn-agent-response-character-metrics@v1"
                    ),
                    "service_result_chars": 0,
                    "service_source_text_chars": 0,
                    "service_metadata_chars": 0,
                    "service_replay_weighted_chars": 0,
                    "model_visible_tool_output_chars": 123,
                },
            )

    def test_native_response_character_metrics_are_canonical(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            (run_dir / "native-session.json").write_text(
                json.dumps({
                    "operations": [
                        {
                            "operation": "open",
                            "result_chars": 100,
                            "source_text_chars": 70,
                            "metadata_chars": 30,
                            "elapsed_ms": 2.5,
                            "http_status": 200,
                        },
                        {
                            "operation": "read",
                            "result_chars": 40,
                            "source_text_chars": 25,
                            "metadata_chars": 15,
                            "elapsed_ms": 1.0,
                            "http_status": 200,
                        },
                    ],
                    "checkpoint": {"objective": "Preserve progress"},
                })
                + "\n",
                encoding="utf-8",
            )
            record = {
                "condition": "service_api",
                "events": {"command_output_chars": 180},
                "model_visible_tool_output_chars": 180,
            }
            attach_workspace_metrics(record, run_dir)
            self.assertTrue(record["service_accounting_valid"])
            self.assertEqual(
                record["response_character_metrics"],
                {
                    "schema": (
                        "brunn-agent-response-character-metrics@v1"
                    ),
                    "service_result_chars": 140,
                    "service_source_text_chars": 95,
                    "service_metadata_chars": 45,
                    "service_replay_weighted_chars": 240,
                    "model_visible_tool_output_chars": 180,
                },
            )

    def test_native_accounting_retains_local_failure_and_rejects_fake_http(self):
        local_failure = {
            "operation": "failed:checkpoint",
            "result_chars": 30,
            "source_text_chars": 0,
            "metadata_chars": 30,
            "elapsed_ms": 0,
            "http_status": 0,
            "service_status": "invalid_request",
        }
        success = {
            "operation": "checkpoint",
            "result_chars": 50,
            "source_text_chars": 10,
            "metadata_chars": 40,
            "elapsed_ms": 4.0,
            "http_status": 200,
        }
        self.assertEqual(
            validate_native_service_accounting({
                "operations": [local_failure, success],
            }),
            [local_failure, success],
        )
        ordinary_zero = {**local_failure, "operation": "open"}
        with self.assertRaisesRegex(ValueError, "invalid http_status"):
            validate_native_service_accounting({
                "operations": [ordinary_zero],
            })
        server_failure = {
            **local_failure,
            "operation": "failed:open",
            "http_status": 503,
            "service_status": "unavailable",
        }
        with self.assertRaisesRegex(ValueError, "invalid http_status"):
            validate_native_service_accounting({
                "operations": [server_failure],
            })

        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            (run_dir / "native-session.json").write_text(
                json.dumps({"operations": [local_failure, success]}) + "\n",
                encoding="utf-8",
            )
            record = {
                "condition": "service_api",
                "events": {"command_output_chars": 80},
            }
            attach_workspace_metrics(record, run_dir)
            self.assertTrue(record["service_accounting_valid"])
            self.assertEqual(record["service_operations"], [
                local_failure,
                success,
            ])
            self.assertEqual(
                record["local_cli_failures"],
                {
                    "schema": (
                        "brunn-local-cli-failure-summary@v1"
                    ),
                    "count": 1,
                    "operation_counts": {
                        "failed:checkpoint": 1,
                    },
                    "result_chars": 30,
                    "source_text_chars": 0,
                    "metadata_chars": 30,
                    "operations": [local_failure],
                },
            )
            self.assertEqual(record["service_calls"], 1)
            self.assertEqual(record["service_http_calls"], 1)
            self.assertEqual(record["service_result_chars"], 50)
            self.assertEqual(record["service_source_text_chars"], 10)
            self.assertEqual(record["service_metadata_chars"], 40)
            self.assertEqual(record["service_replay_weighted_chars"], 50)
            self.assertEqual(record["service_latency_ms"], 4.0)
            self.assertEqual(record["workspace_result_chars"], 50)
            self.assertEqual(
                record["response_character_metrics"],
                {
                    "schema": (
                        "brunn-agent-response-character-metrics@v1"
                    ),
                    "service_result_chars": 50,
                    "service_source_text_chars": 10,
                    "service_metadata_chars": 40,
                    "service_replay_weighted_chars": 50,
                    "model_visible_tool_output_chars": 80,
                },
            )

            (run_dir / "native-session.json").write_text(
                json.dumps({"operations": [local_failure]}) + "\n",
                encoding="utf-8",
            )
            failed_record = {
                "condition": "service_api",
                "events": {"command_output_chars": 30},
            }
            attach_workspace_metrics(failed_record, run_dir)
            self.assertFalse(failed_record["service_accounting_valid"])
            self.assertEqual(
                failed_record["local_cli_failures"]["operations"],
                [local_failure],
            )
            self.assertEqual(failed_record["service_calls"], 0)
            self.assertEqual(failed_record["service_result_chars"], 0)
            self.assertEqual(
                failed_record["response_character_metrics"][
                    "model_visible_tool_output_chars"
                ],
                30,
            )

    def test_service_record_rejects_missing_or_uncontracted_zero_accounting(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            record = {
                "condition": "service_api",
                "events": {"command_output_chars": 0},
            }
            attach_workspace_metrics(record, run_dir)
            self.assertFalse(record["service_accounting_valid"])
            self.assertIn("native-session.json is missing", record["error"])

            (run_dir / "native-session.json").write_text(
                json.dumps({"operations": []}) + "\n",
                encoding="utf-8",
            )
            record = {
                "condition": "service_api",
                "events": {"command_output_chars": 0},
            }
            attach_workspace_metrics(record, run_dir)
            self.assertFalse(record["service_accounting_valid"])
            self.assertIn("zero service calls", record["error"])

    def test_response_character_metrics_survive_invalid_native_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            (run_dir / "native-session.json").write_text(
                "{invalid",
                encoding="utf-8",
            )
            record = {
                "condition": "service_api",
                "events": {"command_output_chars": 19},
            }
            attach_workspace_metrics(record, run_dir)
            self.assertIsNotNone(record["workspace_state_error"])
            self.assertFalse(record["service_accounting_valid"])
            self.assertIn(
                "Invalid native service accounting",
                record["error"],
            )
            self.assertEqual(
                record["response_character_metrics"]["schema"],
                "brunn-agent-response-character-metrics@v1",
            )
            self.assertEqual(
                record["response_character_metrics"][
                    "model_visible_tool_output_chars"
                ],
                19,
            )

    def test_filesystem_sidecar_is_checkpoint_eligible_for_read_only_cases(self):
        manifest = {
            "conditions": ["filesystem_sidecar"],
            "cases": [{
                "id": "read-only-case",
                "workload": "work",
                "capability": "read",
                "workspace_access": "read_only",
            }],
        }
        record = {
            "case_id": "read-only-case",
            "condition": "filesystem_sidecar",
            "elapsed_seconds": 1.0,
            "fixed_context_chars": 0,
            "workspace_result_chars": 0,
            "persisted_checkpoint": {"objective": "Preserve progress"},
            "events": {
                "tokens": {
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "cached_input_tokens": 0,
                },
                "commands": 1,
                "command_output_chars": 1,
            },
            "grade": {
                "pass": True,
                "score": 1.0,
                "claims_passed": 1,
                "claims_total": 1,
            },
        }
        result = summarize(manifest, [record])["by_condition"]["filesystem_sidecar"]
        self.assertEqual(result["persisted_checkpoints"], 1)
        self.assertEqual(result["checkpoint_eligible_runs"], 1)

    def test_rupture_ops_suite_shape_hash_and_code_artifacts(self):
        manifest_path = ROOT / "eval" / "rupture_ops_cases.json"
        validated = validate(manifest_path, ROOT / "eval" / "work_answer_schema.json")
        self.assertEqual(validated["errors"], [])
        self.assertEqual(len(validated["manifest"]["cases"]), 12)
        self.assertEqual(
            sum(len(case["rubric"]) for case in validated["manifest"]["cases"]),
            48,
        )
        self.assertEqual(len(validated["documents"]), 65)
        self.assertEqual(len(validated["chunks"]), 587)
        self.assertEqual(
            validated["corpus_sha256"],
            "f58627c8c1c1a4d50044ca946ab7a909722914a287a3e040dde3867417a725fe",
        )
        self.assertEqual(
            validated["artifact_tree_sha256"],
            "ec693c7f31481a70d07754b2eaa2b6a1ca09114954f653db151fa4fe10fc6a37",
        )
        paths = {document.path for document in validated["documents"]}
        self.assertIn(
            "Projects/RuptureOps/Repository/ios/RuptureOps/Domain/RuptureCycle.swift",
            paths,
        )

        case = next(
            case for case in validated["manifest"]["cases"]
            if case["id"] == "ruptureops-interrupted-ios-continuation"
        )
        self.assertIn(
            './memory open --scope "RuptureOps"',
            render_prompt(case, "workspace"),
        )
        service_prompt = render_prompt(case, "service_api", "simple")
        self.assertIn("treat its initial evidence", service_prompt)
        self.assertIn("repeat `--path` or `--ref`", service_prompt)
        self.assertIn("complete entry", service_prompt)
        self.assertIn("intentionally has no compute or verify operation", service_prompt)
        self.assertNotIn("--neighbors", service_prompt)
        self.assertIn("exactly from authoritative evidence", service_prompt)
        self.assertIn("Build a facet checklist", service_prompt)
        self.assertIn("repeat a fact when it is needed in more than one slot", service_prompt)

    def test_service_prompt_has_shell_safe_checkpoint_contract(self):
        case = self.manifest["cases"][0]
        invocation = (
            "`./memory checkpoint "
            "'{\"state\":{\"objective\":\"...\",\"current_state\":[],"
            "\"decisions\":[],\"open_questions\":[],\"next_actions\":[],"
            "\"artifacts\":[]},\"source_refs\":[\"exact/source/path\"]}'`"
        )
        for protocol in ("simple", "legacy"):
            prompt = render_prompt(case, "service_api", protocol)
            self.assertEqual(prompt.count(invocation), 1)
            self.assertIn(
                "Pass the JSON directly as the single-quoted shell literal",
                prompt,
            )
            self.assertIn(
                r"encode any apostrophe inside a JSON string as `\u0027`",
                prompt,
            )
            for forbidden_shape in (
                "Do not use raw prose",
                "`--text`",
                "stdin or a pipe",
                "`--json-stdin`",
                "shell-variable indirection",
                "including an unset variable",
            ):
                self.assertIn(forbidden_shape, prompt)

    def test_e02_identifier_subset_is_frozen_at_five_cases_twenty_claims(self):
        full = json.loads(
            (ROOT / "eval" / "recent_work_cases.json").read_text(encoding="utf-8")
        )
        subset_path = ROOT / "eval" / "e02_identifier_cases.json"
        validated = validate(
            subset_path,
            ROOT / "eval" / "work_answer_schema.json",
        )
        self.assertEqual(validated["errors"], [])
        subset = validated["manifest"]
        expected_ids = full["feature_families"]["identifier_heavy"]
        self.assertEqual(
            [case["id"] for case in subset["cases"]],
            expected_ids,
        )
        self.assertEqual(len(subset["cases"]), 5)
        self.assertEqual(
            sum(len(case["rubric"]) for case in subset["cases"]),
            20,
        )

    def test_e04_chronic_manifests_are_exact_case_subsets(self):
        specifications = [
            (
                "e04_chronic_rupture_cases.json",
                "rupture_ops_cases.json",
                {
                    "ruptureops-archive-import-reconciliation",
                    "ruptureops-flowworks-campaign-revision",
                    "ruptureops-spatial-evidence",
                    "ruptureops-forked-agent-idempotency",
                },
            ),
            (
                "e04_chronic_guard_cases.json",
                "work_cases.json",
                {
                    "star-rupture-plan-revision",
                    "warmind-parser-learning",
                },
            ),
        ]
        for chronic_name, base_name, expected_ids in specifications:
            with self.subTest(manifest=chronic_name):
                chronic_path = ROOT / "eval" / chronic_name
                validated = validate(
                    chronic_path,
                    ROOT / "eval" / "work_answer_schema.json",
                )
                self.assertEqual(validated["errors"], [])
                chronic = json.loads(chronic_path.read_text())
                base = json.loads((ROOT / "eval" / base_name).read_text())
                chronic_by_id = {case["id"]: case for case in chronic["cases"]}
                base_by_id = {case["id"]: case for case in base["cases"]}
                self.assertEqual(set(chronic_by_id), expected_ids)
                self.assertEqual(
                    chronic_by_id,
                    {
                        case_id: base_by_id[case_id]
                        for case_id in chronic_by_id
                    },
                )

    def test_exact_value_slots_are_validated_and_carried_into_grades(self):
        manifest_path = ROOT / "eval" / "rupture_ops_cases.json"
        validated = validate(
            manifest_path,
            ROOT / "eval" / "work_answer_schema.json",
        )
        self.assertEqual(validated["errors"], [])
        case = next(
            case
            for case in validated["manifest"]["cases"]
            if case["id"] == "ruptureops-archive-import-reconciliation"
        )
        claims = [
            {
                "id": rubric["id"],
                "value": " ".join(check["any"][0] for check in rubric["checks"]),
                "source_paths": [rubric["sources_any"][0]],
                "confidence": "high",
            }
            for rubric in case["rubric"]
        ]
        answer = {
            "answer": "Audit result.",
            "claims": claims,
            "checkpoint": {
                "objective": "Preserve the audit.",
                "current_state": ["Complete"],
                "decisions": ["Keep one canonical import"],
                "open_questions": [],
                "next_actions": ["Recheck live state"],
                "artifacts": ["receipt"],
            },
        }
        grade = grade_answer(
            case,
            answer,
            {document.path for document in validated["documents"]},
        )
        self.assertTrue(all(claim["exact_value"] for claim in grade["claims"]))
        self.assertTrue(
            all("exact_value" in claim["tags"] for claim in grade["claims"])
        )

    def test_exact_value_slot_validation_rejects_unknown_claims(self):
        manifest = json.loads((ROOT / "eval" / "work_cases.json").read_text())
        manifest["exact_value_claim_slots"] = {
            "warmind-parser-learning": ["not-a-claim"],
        }
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            validated = validate(
                path,
                ROOT / "eval" / "work_answer_schema.json",
            )
        self.assertIn(
            "warmind-parser-learning: unknown exact-value claim ID not-a-claim",
            validated["errors"],
        )

    def test_retired_cases_are_excluded_by_default_but_remain_reproducible(self):
        active = select_cases(self.manifest, None, include_retired=False)
        all_cases = select_cases(self.manifest, None, include_retired=True)
        explicit = select_cases(
            self.manifest,
            ["brunn-trust-handoff"],
            include_retired=False,
        )

        self.assertEqual(len(all_cases), len(active) + 1)
        self.assertNotIn("brunn-trust-handoff", {case["id"] for case in active})
        self.assertEqual([case["id"] for case in explicit], ["brunn-trust-handoff"])

    def test_rupture_ops_rubrics_are_satisfiable_and_fixed_packs_are_fair(self):
        manifest = json.loads((ROOT / "eval" / "rupture_ops_cases.json").read_text())
        documents, chunks = load_corpus(ROOT / manifest["corpus_root"])
        paths = {document.path for document in documents}
        index = BM25Index(chunks)
        for case in manifest["cases"]:
            context = render_fixed_context(case, index, manifest)
            claims = []
            for rubric in case["rubric"]:
                self.assertTrue(
                    any(path in context for path in rubric["sources_any"]),
                    f"{case['id']}:{rubric['id']} has no expected source in its fixed pack",
                )
                claims.append({
                    "id": rubric["id"],
                    "value": " ".join(check["any"][0] for check in rubric["checks"]),
                    "source_paths": [rubric["sources_any"][0]],
                    "confidence": "high",
                })
            answer = {
                "answer": "Evidence-backed continuation.",
                "claims": claims,
                "checkpoint": {
                    "objective": "Continue the work safely.",
                    "current_state": ["Recovered evidence"],
                    "decisions": ["Preserve source boundaries"],
                    "open_questions": ["Refresh live state"],
                    "next_actions": ["Advance from the verified checkpoint"],
                    "artifacts": ["Source-bearing corpus artifacts"],
                },
            }
            self.assertTrue(grade_answer(case, answer, paths)["pass"], case["id"])

    def test_personal_coordination_suite_shape_scope_and_hashes(self):
        manifest_path = ROOT / "eval" / "personal_coordination_cases.json"
        validated = validate(manifest_path, ROOT / "eval" / "work_answer_schema.json")
        manifest = validated["manifest"]
        cases = manifest["cases"]

        self.assertEqual(validated["errors"], [])
        self.assertEqual(manifest["benchmark_version"], "personal-coordination-v0.1")
        self.assertEqual(manifest["grading_mode"], "concept_tokens_v1")
        self.assertEqual(
            manifest["conditions"],
            ["fixed_pack", "filesystem", "workspace"],
        )
        self.assertEqual(manifest["fixed_pack_chars"], 24000)
        self.assertEqual(manifest["fixed_pack_chunks"], 16)
        self.assertEqual(
            manifest["generalization"]["profile_types"],
            [
                "person", "organization", "group", "place", "event",
                "arrangement", "resource", "work-item", "artifact",
            ],
        )
        self.assertEqual(
            [case["id"] for case in cases],
            [
                "coord-person-resolution",
                "coord-identity-equivalence-reversal",
                "coord-person-dossier",
                "coord-role-relationship-provenance",
                "coord-canonical-contract-normalization",
                "coord-series-exceptions",
                "coord-schedule-supersession",
                "coord-participation-independent-state",
                "coord-deadline-readiness",
                "coord-handoff-logistics",
                "coord-arrangement-independent-state",
                "coord-vacation-game-continuity",
                "coord-weekly-brief-change-impact",
                "coord-read-only-capability-boundary",
                "coord-minor-export-policy",
            ],
        )
        self.assertTrue(all(case["scope"] == "Personal Coordination" for case in cases))
        self.assertTrue(all(case["workload"] == "Personal Coordination" for case in cases))
        self.assertTrue(all(set(case["claim_slots"]) == {"c1", "c2", "c3", "c4"} for case in cases))
        self.assertTrue(all(len(case["rubric"]) == 4 for case in cases))
        self.assertTrue(all(case["forbidden"] for case in cases))
        self.assertEqual(sum(len(case["rubric"]) for case in cases), 60)

        corpus_files = [path for path in validated["corpus_root"].rglob("*") if path.is_file()]
        self.assertEqual(len(corpus_files), 29)
        self.assertTrue(all(path.suffix in {".md", ".json", ".csv"} for path in corpus_files))
        self.assertEqual(len(validated["documents"]), 29)
        self.assertEqual(len(validated["chunks"]), 36)
        self.assertEqual(sum(len(document.text) for document in validated["documents"]), 29347)
        self.assertEqual(
            validated["corpus_sha256"],
            "b481b734c73e73242553cd16df6f698dffb311258ab643a2741c40ff229352ea",
        )
        self.assertEqual(
            validated["artifact_tree_sha256"],
            "b481b734c73e73242553cd16df6f698dffb311258ab643a2741c40ff229352ea",
        )
        self.assertEqual(
            hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
            "b6482bfc873a967f470985019afff5ef3b23df00de106cc0f38de3438ee60ece",
        )

    def test_personal_coordination_rubrics_are_satisfiable_and_fixed_packs_are_fair(self):
        manifest = json.loads(
            (ROOT / "eval" / "personal_coordination_cases.json").read_text()
        )
        documents, chunks = load_corpus(ROOT / manifest["corpus_root"])
        documents_by_path = {document.path: document for document in documents}
        paths = set(documents_by_path)
        index = BM25Index(chunks)

        for case in manifest["cases"]:
            context = render_fixed_context(case, index, manifest)
            self.assertLessEqual(len(context), manifest["fixed_pack_chars"] + 2000)
            claims = []
            for rubric in case["rubric"]:
                self.assertGreaterEqual(len(rubric["sources_any"]), 1)
                self.assertLessEqual(len(rubric["sources_any"]), 6)
                source_text = "\n".join(
                    documents_by_path[path].text for path in rubric["sources_any"]
                )
                for check in rubric["checks"]:
                    self.assertTrue(
                        any(normalize(candidate) in normalize(source_text) for candidate in check["any"]),
                        f"{case['id']}:{rubric['id']} has an unsatisfied source check {check}",
                    )
                self.assertTrue(
                    any(path in context for path in rubric["sources_any"]),
                    f"{case['id']}:{rubric['id']} has no expected source in its fixed pack",
                )
                claims.append({
                    "id": rubric["id"],
                    "value": " ".join(check["any"][0] for check in rubric["checks"]),
                    "source_paths": [rubric["sources_any"][0]],
                    "confidence": "high",
                })
            answer = {
                "answer": "Source-preserving personal coordination checkpoint.",
                "claims": claims,
                "checkpoint": {
                    "objective": "Continue coordination from frozen evidence.",
                    "current_state": ["Recovered source-bearing state"],
                    "decisions": ["Keep independent facts independent"],
                    "open_questions": ["Await new authoritative evidence"],
                    "next_actions": ["Advance the cited work item"],
                    "artifacts": ["Frozen corpus sources"],
                },
            }
            self.assertTrue(grade_answer(case, answer, paths)["pass"], case["id"])

    def test_recent_work_suite_shape_and_source_boundaries(self):
        manifest_path = ROOT / "eval" / "recent_work_cases.json"
        validated = validate(manifest_path, ROOT / "eval" / "work_answer_schema.json")
        manifest = validated["manifest"]

        self.assertEqual(validated["errors"], [])
        self.assertEqual(manifest["benchmark_version"], "recent-work-v0.3")
        self.assertEqual(len(manifest["cases"]), 14)
        self.assertEqual(
            sum(len(case["rubric"]) for case in manifest["cases"]),
            56,
        )
        self.assertEqual(len(validated["documents"]), 28)
        self.assertEqual(len(validated["chunks"]), 41)
        self.assertEqual(
            validated["corpus_sha256"],
            "ee599a95aad9c9811b270b996cf14fb13edcfc61a87426e8625ba72cc1f88df1",
        )
        self.assertTrue(
            {
                "recent-europe-source-authority",
                "recent-europe-calendar-dedup",
                "recent-europe-rail-resume",
                "recent-tracker-no-delta",
                "recent-aether-heartbeat-healthy",
                "recent-aether-gmail-actions",
                "recent-aether-morning-brief",
                "recent-current-over-history",
            }
            <= {case["id"] for case in manifest["cases"]}
        )

        frozen_text = manifest_path.read_text() + "\n" + "\n".join(
            document.text for document in validated["documents"]
        )
        self.assertNotIn("/Users/aether/", frozen_text)
        self.assertNotIn("@rourkem.com", frozen_text)
        self.assertNotRegex(frozen_text, r"\b[A-Z0-9]{6}\b")
        emails = re.findall(
            r"[A-Za-z0-9._%+-]+@([A-Za-z0-9.-]+\.[A-Za-z]{2,})",
            frozen_text,
        )
        self.assertEqual(set(emails), {"example.com"})

    def test_recent_work_rubrics_are_satisfiable_and_fixed_packs_are_fair(self):
        manifest = json.loads(
            (ROOT / "eval" / "recent_work_cases.json").read_text()
        )
        documents, chunks = load_corpus(ROOT / manifest["corpus_root"])
        documents_by_path = {document.path: document for document in documents}
        paths = set(documents_by_path)
        index = BM25Index(chunks)

        for case in manifest["cases"]:
            context = render_fixed_context(case, index, manifest)
            self.assertLessEqual(len(context), manifest["fixed_pack_chars"] + 2000)
            claims = []
            for rubric in case["rubric"]:
                source_text = "\n".join(
                    documents_by_path[path].text for path in rubric["sources_any"]
                )
                for check in rubric["checks"]:
                    self.assertTrue(
                        any(
                            candidate_matches(
                                candidate,
                                source_text,
                                manifest["grading_mode"],
                            )
                            for candidate in check["any"]
                        ),
                        f"{case['id']}:{rubric['id']} has an unsatisfied source check {check}",
                    )
                self.assertTrue(
                    any(path in context for path in rubric["sources_any"]),
                    f"{case['id']}:{rubric['id']} has no expected source in its fixed pack",
                )
                claims.append({
                    "id": rubric["id"],
                    "value": " ".join(check["any"][0] for check in rubric["checks"]),
                    "source_paths": [rubric["sources_any"][0]],
                    "confidence": "high",
                })
            answer = {
                "answer": "Source-backed recent-work continuation.",
                "claims": claims,
                "checkpoint": {
                    "objective": "Continue from current evidence.",
                    "current_state": ["Recovered the latest source-backed state"],
                    "decisions": ["Respect authority and action boundaries"],
                    "open_questions": ["Await new evidence where needed"],
                    "next_actions": ["Advance only the cited current work"],
                    "artifacts": ["Frozen recent-work corpus"],
                },
            }
            self.assertTrue(grade_answer(case, answer, paths)["pass"], case["id"])

    def test_personal_coordination_fixture_has_no_live_paths_or_contact_domains(self):
        manifest_path = ROOT / "eval" / "personal_coordination_cases.json"
        manifest = json.loads(manifest_path.read_text())
        documents, _ = load_corpus(ROOT / manifest["corpus_root"])
        frozen_text = manifest_path.read_text() + "\n" + "\n".join(
            document.text for document in documents
        )

        self.assertNotRegex(frozen_text, r"/(?:Users|home|Volumes)/")
        self.assertNotIn("obsidian", frozen_text.casefold())
        emails = re.findall(
            r"[A-Za-z0-9._%+-]+@([A-Za-z0-9.-]+\.[A-Za-z]{2,})",
            frozen_text,
        )
        self.assertGreater(len(emails), 0)
        self.assertEqual(set(emails), {"example.com"})

    def test_personal_coordination_canonical_contract_and_projection_receipt(self):
        corpus = ROOT / "eval" / "corpus-personal-coordination-v0.1"
        contract = json.loads(
            (corpus / "Contracts" / "canonical-contract-snapshot.json").read_text()
        )
        self.assertEqual(contract["schema"], "brunn-context-contract-fixture@v1")
        self.assertEqual(contract["object"]["object_id"], "object:person-p301")
        self.assertGreaterEqual(len(contract["object"]["type_profiles"]), 2)
        self.assertEqual(contract["qualified_relation"]["version"], "v2")
        self.assertEqual(contract["qualified_relation"]["previous_version"], "v1")
        self.assertEqual(len(contract["qualified_relation"]["endpoints"]), 2)
        recurrence = contract["recurring_event"]["recurrence"]
        self.assertEqual(contract["recurring_event"]["temporal_schema"], "temporal-spec@v1")
        self.assertEqual(recurrence["start"]["kind"], "local_datetime")
        self.assertEqual(recurrence["start"]["time_zone"], "America/Los_Angeles")
        self.assertEqual(
            {claim["predicate"] for claim in contract["state_assignments"]},
            {
                "state:arrangement.booking@v1",
                "state:arrangement.payment@v1",
                "state:arrangement.allocation@v1",
                "state:resource.availability@v1",
                "state:arrangement.use@v1",
            },
        )
        self.assertTrue(all(claim["claim_mode"] == "state_assignment" for claim in contract["state_assignments"]))
        claims_by_predicate = {
            claim["predicate"]: claim for claim in contract["state_assignments"]
        }
        self.assertEqual(
            claims_by_predicate["state:resource.availability@v1"]["about_refs"],
            [contract["resource"]["object_id"]],
        )
        self.assertTrue(
            all(
                claim["about_refs"] == [contract["arrangement"]["object_id"]]
                for predicate, claim in claims_by_predicate.items()
                if predicate != "state:resource.availability@v1"
            )
        )

        projection = json.loads(
            (corpus / "Policy" / "redacted-minor-projection.json").read_text()
        )
        for field in {
            "source_revision", "policy_version", "audience", "purpose",
            "path_basis", "included_paths", "withheld", "transforms", "generated_at",
            "audit_receipt",
        }:
            self.assertIn(field, projection)
        self.assertTrue(all(item.get("path") and item.get("reason") for item in projection["withheld"]))
        self.assertEqual(projection["path_basis"]["kind"], "source_revision")
        self.assertEqual(
            projection["path_basis"]["revision_ref"],
            projection["source_revision"],
        )
        self.assertTrue(all(path.startswith("/") for path in projection["included_paths"]))

    def test_concept_token_grading_accepts_paraphrase_and_preserves_negation(self):
        self.assertTrue(candidate_matches(
            "Each row retains its source ID",
            "The rebuilt view keeps provenance by retaining each row's source ID.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "schedule_authority: none",
            "The family copy has no schedule authority.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "no references are rewritten to person:p-101",
            "Do not rewrite references; relation:participation-103 stays on person:p-103, not person:p-101.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "240/min",
            "Each extractor produces 240 items/min.",
            "concept_tokens_v1",
        ))
        self.assertFalse(candidate_matches(
            "does not change the tentative response",
            "The update changes the tentative response.",
            "concept_tokens_v1",
        ))
        self.assertTrue(candidate_matches(
            "exclude",
            "Remove all dependencies on private source data.",
            "concept_tokens_v1",
        ))

    def test_forbidden_conclusions_ignore_explicit_negation(self):
        forbidden = "rewrite relation:participation-103 to person:p-101"
        self.assertFalse(forbidden_is_asserted(
            forbidden,
            normalize(
                "Do not delete or merge either person, select a survivor, "
                "or rewrite relation:participation-103 to person:p-101."
            ),
        ))
        self.assertTrue(forbidden_is_asserted(
            forbidden,
            normalize("Rewrite relation:participation-103 to person:p-101."),
        ))

    def test_read_only_workspace_allows_reasoning_and_denies_mutation(self):
        corpus = ROOT / "eval" / "corpus-personal-coordination-v0.1"
        documents, _ = load_corpus(corpus)
        before = corpus_hash(documents)
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary) / "session.json"
            base = [
                sys.executable,
                str(ROOT / "workspace_cli.py"),
                "--corpus", str(corpus),
                "--session", str(session),
                "--access-mode", "read_only",
            ]
            opened = subprocess.run(
                [*base, "open", "--scope", "Personal Coordination"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(opened.returncode, 0, opened.stderr)

            allowed_commands = [
                ["query", "read-only capability", "--scope", "Personal Coordination"],
                ["read", "--path", "Authorization/read-only-capability-contract.md"],
                ["compute", "1 + 1"],
                ["verify", "read-only credentials cannot mutate corpus state", "--scope", "Personal Coordination"],
            ]
            for command in allowed_commands:
                allowed = subprocess.run(
                    [*base, *command],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(allowed.returncode, 0, allowed.stderr)

            denied = subprocess.run(
                [*base, "checkpoint", "--objective", "must not persist"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(denied.returncode, 77)
            self.assertEqual(json.loads(denied.stdout)["error"]["code"], "capability_denied")
            state = json.loads(session.read_text())
            self.assertIsNone(state["checkpoint"])
            self.assertEqual(state["operations"][-1]["operation"], "denied:checkpoint")

            for operation in ["save", "stage", "correct", "delete", "dream"]:
                unavailable = subprocess.run(
                    [*base, operation],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(unavailable.returncode, 77, operation)
                self.assertEqual(
                    json.loads(unavailable.stdout)["error"]["code"],
                    "capability_denied",
                )

        after = corpus_hash(load_corpus(corpus)[0])
        self.assertEqual(after, before)

        manifest = json.loads(
            (ROOT / "eval" / "personal_coordination_cases.json").read_text()
        )
        case = next(
            item for item in manifest["cases"]
            if item["id"] == "coord-read-only-capability-boundary"
        )
        prompt = render_prompt(case, "workspace")
        self.assertIn("cannot checkpoint or mutate corpus or staged state", prompt)
        self.assertIn("output proposal only", prompt)

    def test_workspace_compute(self):
        self.assertEqual(safe_compute("45.67 + 32.31 + 32.31"), 110.29)
        with self.assertRaises(ValueError):
            safe_compute("__import__('os').getcwd()")

    def test_fixed_pack_is_bounded_and_source_bearing(self):
        case = next(case for case in self.manifest["cases"] if case["id"] == "star-rupture-rail-and-heat-plan")
        context = render_fixed_context(case, BM25Index(self.chunks), self.manifest)
        self.assertLessEqual(len(context), self.manifest["fixed_pack_chars"] + 2000)
        self.assertIn("Topics/Star Rupture/Star Rupture production.md", context)
        self.assertIn("480", context)

    def test_structured_grader_accepts_corpus_prefix(self):
        case = next(case for case in self.manifest["cases"] if case["id"] == "star-rupture-rail-and-heat-plan")
        claims = []
        for rubric in case["rubric"]:
            value = " ".join(check["any"][0] for check in rubric["checks"])
            claims.append({
                "id": rubric["id"],
                "value": value,
                "source_paths": ["corpus/" + rubric["sources_any"][0]],
                "confidence": "high",
            })
        answer = {
            "answer": "Evidence-backed plan.",
            "claims": claims,
            "checkpoint": {
                "objective": "Plan the factory.",
                "current_state": ["Known state"],
                "decisions": ["Safe decision"],
                "open_questions": ["Verify in game"],
                "next_actions": ["Build the first line"],
                "artifacts": ["Source notes"],
            },
        }
        grade = grade_answer(case, answer, {document.path for document in self.documents})
        self.assertTrue(grade["pass"])
        self.assertEqual(grade["citation_validity"], 1.0)

    def test_structured_grader_rejects_duplicate_missing_and_extra_claim_ids(self):
        case = {
            "rubric": [
                {
                    "id": "c1",
                    "checks": [{"any": ["alpha"]}],
                    "sources_any": ["Evidence/source.md"],
                },
                {
                    "id": "c2",
                    "checks": [{"any": ["beta"]}],
                    "sources_any": ["Evidence/source.md"],
                },
            ],
        }
        checkpoint = {
            "objective": "Test exact claim sets.",
            "current_state": ["Evidence is available."],
            "next_actions": ["Reject malformed claim sets."],
            "artifacts": ["Evidence/source.md"],
        }
        valid_claims = [
            {
                "id": "c1",
                "value": "alpha",
                "source_paths": ["Evidence/source.md"],
            },
            {
                "id": "c2",
                "value": "beta",
                "source_paths": ["Evidence/source.md"],
            },
        ]
        corpus = {"Evidence/source.md"}
        valid = grade_answer(
            case,
            {"answer": "", "claims": valid_claims, "checkpoint": checkpoint},
            corpus,
        )
        duplicate = grade_answer(
            case,
            {
                "answer": "",
                "claims": [valid_claims[0], valid_claims[0], valid_claims[1]],
                "checkpoint": checkpoint,
            },
            corpus,
        )
        missing = grade_answer(
            case,
            {
                "answer": "",
                "claims": [valid_claims[0]],
                "checkpoint": checkpoint,
            },
            corpus,
        )
        extra = grade_answer(
            case,
            {
                "answer": "",
                "claims": [
                    *valid_claims,
                    {
                        "id": "c3",
                        "value": "gamma",
                        "source_paths": ["Evidence/source.md"],
                    },
                ],
                "checkpoint": checkpoint,
            },
            corpus,
        )

        self.assertTrue(valid["pass"])
        self.assertTrue(valid["claim_set_valid"])
        self.assertFalse(duplicate["pass"])
        self.assertEqual(duplicate["duplicate_claim_ids"], ["c1"])
        self.assertFalse(missing["pass"])
        self.assertEqual(missing["missing_claim_ids"], ["c2"])
        self.assertFalse(extra["pass"])
        self.assertEqual(extra["extra_claim_ids"], ["c3"])

    def test_claim_cannot_borrow_required_content_from_global_answer(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [{"any": ["required claim detail"]}],
                "sources_any": ["Evidence/source.md"],
            }],
        }
        answer = {
            "answer": "The required claim detail appears only in the global answer.",
            "claims": [{
                "id": "c1",
                "value": "This claim omits it.",
                "source_paths": ["Evidence/source.md"],
            }],
            "checkpoint": {
                "objective": "Test claim isolation.",
                "current_state": ["A claim is incomplete."],
                "next_actions": ["Keep grading claim-local."],
                "artifacts": ["Evidence/source.md"],
            },
        }

        grade = grade_answer(case, answer, {"Evidence/source.md"})

        self.assertFalse(grade["pass"])
        self.assertFalse(grade["claims"][0]["pass"])
        self.assertIsNone(grade["claims"][0]["checks"][0]["matched"])

    def test_claim_with_only_two_of_three_required_checks_fails(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [
                    {"any": ["alpha"]},
                    {"any": ["beta"]},
                    {"any": ["gamma"]},
                ],
                "sources_any": ["Evidence/source.md"],
            }],
        }
        answer = {
            "answer": "",
            "claims": [{
                "id": "c1",
                "value": "alpha beta",
                "source_paths": ["Evidence/source.md"],
            }],
            "checkpoint": {
                "objective": "Test complete checks.",
                "current_state": ["Two details are present."],
                "next_actions": ["Require the third detail."],
                "artifacts": ["Evidence/source.md"],
            },
        }

        grade = grade_answer(case, answer, {"Evidence/source.md"})

        self.assertFalse(grade["pass"])
        self.assertEqual(grade["claims"][0]["content_fraction"], 0.6667)
        self.assertFalse(grade["claims"][0]["pass"])

    def test_fabricated_suffix_citation_does_not_match_expected_path(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [{"any": ["supported fact"]}],
                "sources_any": ["Authority/source.md"],
            }],
        }
        answer = {
            "answer": "",
            "claims": [{
                "id": "c1",
                "value": "supported fact",
                "source_paths": ["fabricated/Authority/source.md"],
            }],
            "checkpoint": {
                "objective": "Test exact citations.",
                "current_state": ["The claim has a fabricated path."],
                "next_actions": ["Reject suffix matching."],
                "artifacts": ["Authority/source.md"],
            },
        }

        grade = grade_answer(case, answer, {"Authority/source.md"})

        self.assertFalse(grade["pass"])
        self.assertFalse(grade["claims"][0]["source_hit"])
        self.assertEqual(grade["citation_validity"], 0.0)

    def test_sources_all_requires_every_exact_path(self):
        case = {
            "rubric": [{
                "id": "c1",
                "checks": [{"any": ["joined conclusion"]}],
                "sources_any": ["Evidence/alpha.md", "Evidence/beta.md"],
                "sources_all": ["Evidence/alpha.md", "Evidence/beta.md"],
            }],
        }
        answer = {
            "answer": "",
            "claims": [{
                "id": "c1",
                "value": "joined conclusion",
                "source_paths": ["Evidence/alpha.md"],
            }],
            "checkpoint": {
                "objective": "Test all-source evidence.",
                "current_state": ["Only one source is cited."],
                "next_actions": ["Require both exact paths."],
                "artifacts": ["Evidence/alpha.md", "Evidence/beta.md"],
            },
        }
        corpus_paths = {"Evidence/alpha.md", "Evidence/beta.md"}

        incomplete = grade_answer(case, answer, corpus_paths)
        answer["claims"][0]["source_paths"] = [
            "Evidence/alpha.md",
            "fabricated/Evidence/beta.md",
        ]
        fabricated = grade_answer(case, answer, corpus_paths)
        answer["claims"][0]["source_paths"] = [
            "Evidence/alpha.md",
            "Evidence/beta.md",
        ]
        complete = grade_answer(case, answer, corpus_paths)

        self.assertFalse(incomplete["pass"])
        self.assertFalse(incomplete["claims"][0]["source_hit"])
        self.assertFalse(fabricated["pass"])
        self.assertFalse(fabricated["claims"][0]["source_hit"])
        self.assertTrue(complete["pass"])

    def test_event_metrics_count_completed_calls_and_cached_input(self):
        with tempfile.TemporaryDirectory() as temp:
            events = Path(temp) / "events.jsonl"
            rows = [
                {"type": "item.started", "item": {"type": "command_execution"}},
                {
                    "type": "item.completed",
                    "item": {"type": "command_execution", "aggregated_output": "hello"},
                },
                {
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 80,
                        "output_tokens": 10,
                    },
                },
            ]
            events.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
            metrics = parse_event_metrics(events)
            self.assertEqual(metrics["commands"], 1)
            self.assertEqual(metrics["command_output_chars"], 5)
            self.assertEqual(metrics["tokens"]["input_tokens"], 100)
            self.assertEqual(metrics["tokens"]["cached_input_tokens"], 80)

    def test_workspace_session_commits_are_atomic_and_reopenable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            corpus = root / "corpus"
            corpus.mkdir()
            (corpus / "Alpha.md").write_text("# Alpha\n\nDurable state survives concurrent operations.\n")
            session = root / "session.json"
            command = [
                sys.executable,
                str(ROOT / "workspace_cli.py"),
                "--corpus",
                str(corpus),
                "--session",
                str(session),
                "compute",
            ]
            with ThreadPoolExecutor(max_workers=6) as executor:
                results = list(executor.map(
                    lambda value: subprocess.run(
                        [*command, f"{value} + 1"],
                        check=False,
                        capture_output=True,
                        text=True,
                    ),
                    range(6),
                ))
            self.assertTrue(all(result.returncode == 0 for result in results))
            state = json.loads(session.read_text())
            self.assertEqual(len(state["operations"]), 6)

            checkpoint = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "workspace_cli.py"),
                    "--corpus",
                    str(corpus),
                    "--session",
                    str(session),
                    "checkpoint",
                    "--objective",
                    "Resume safely",
                    "--current-state",
                    "Concurrent writes passed",
                    "--next-action",
                    "Reopen the checkpoint",
                    "--artifact",
                    "Alpha.md",
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertIn("Resume safely", checkpoint.stdout)
            status = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "workspace_cli.py"),
                    "--corpus",
                    str(corpus),
                    "--session",
                    str(session),
                    "status",
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            reopened = json.loads(status.stdout)
            self.assertEqual(reopened["checkpoint"]["objective"], "Resume safely")
            self.assertEqual(len(reopened["operations"]), 7)


if __name__ == "__main__":
    unittest.main()

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from eval.aggregate_draws import (  # noqa: E402
    aggregate as aggregate_draws,
    load_case_extension_plan,
)
from eval.e09_step_authorization import canonical_json_sha256  # noqa: E402


REVISION = "a" * 40
REGRADING_REVISION = "e" * 40
IMAGE_ID = "sha256:" + "f" * 64


def aggregate(paths, *args, **kwargs):
    if "expected_arm_retrieval_modes" not in kwargs:
        service_arms = set()
        for path in paths:
            payload = json.loads(Path(path).read_text(encoding="utf-8"))
            manifest = payload.get("manifest", {})
            conditions = manifest.get("conditions", [])
            if any(
                condition in {"service_api", "service_api_resume"}
                for condition in conditions
            ):
                service_arms.add(
                    payload.get("experiment_arm") or "service_api"
                )
        kwargs["expected_arm_retrieval_modes"] = [
            f"{arm}=exact,lexical" for arm in sorted(service_arms)
        ]
    return aggregate_draws(paths, *args, **kwargs)


def response_character_metrics(characters):
    return {
        "schema": "brunn-agent-response-character-metrics@v1",
        "service_result_chars": characters + 50,
        "service_source_text_chars": characters,
        "service_metadata_chars": 50,
        "service_replay_weighted_chars": characters * 2,
        "model_visible_tool_output_chars": characters,
    }


def draw_payload(run_id, rows):
    conditions = ["service_api", "filesystem_sidecar"]
    case_ids = sorted({row[0] for row in rows})
    records = []
    for case_id, condition, claims, case_pass, characters in rows:
        metrics = response_character_metrics(characters)
        records.append({
            "case_id": case_id,
            "condition": condition,
            "error": None,
            "grade": {
                "claims_passed": claims,
                "claims_total": 4,
                "pass": case_pass,
                "claims": [
                    {
                        "id": f"c{index}",
                        "pass": index <= claims,
                        "tags": [],
                        "exact_value": False,
                    }
                    for index in range(1, 5)
                ],
                "forbidden_hits": [],
            },
            "response_character_metrics": metrics,
            **{
                key: value
                for key, value in metrics.items()
                if key != "schema"
            },
            "persisted_checkpoint": {"objective": "resume"},
        })
    runtime_snapshot = {
        "schema": "brunn-service-runtime-snapshot@v1",
        "captured_at": "2026-07-27T12:00:30-07:00",
        "status": "ready",
        "build_revision": REVISION,
        "runtime_features": {},
    }
    image = {
        "schema": "brunn-service-image-fingerprint@v1",
        "api_container": f"api-{run_id}",
        "api_container_id": f"container-{run_id}",
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
    return {
        "benchmark_version": "synthetic-v1",
        "run_id": run_id,
        "manifest_sha256": "b" * 64,
        "harness_sha256": "c" * 64,
        "service_protocol": "simple",
        "service_retrieval_modes": ["exact", "lexical"],
        "service_runtime_snapshot": runtime_snapshot,
        "service_image_provenance": image_provenance,
        "expected_runtime_features": {},
        "expected_build_revision": REVISION,
        "experiment_parameters": experiment_parameters,
        "manifest": {
            "benchmark_version": "synthetic-v1",
            "model": "gpt-test",
            "conditions": conditions,
            "cases": [
                {
                    "id": case_id,
                    "rubric": [
                        {"id": f"c{index}", "tags": []}
                        for index in range(1, 5)
                    ],
                    "forbidden": [],
                }
                for case_id in case_ids
            ],
        },
        "reasoning_billing": {
            "route": "chatgpt_subscription",
            "api_fallback": "forbidden",
            "auth_status": "Logged in using ChatGPT",
        },
        "run_ledger": {
            "schema": "brunn-eval-run-ledger@v1",
            "run_id": run_id,
            "captured_at": "2026-07-27T12:01:00-07:00",
            "source": {
                "revision": REVISION,
                "tracked_source_clean": True,
                "untracked_source_files": [],
                "clean": True,
            },
            "codex": {
                "path": "/opt/codex",
                "version": "codex-test",
                "auth_checked_at": "2026-07-27T12:00:00-07:00",
                "auth_route": "chatgpt_subscription",
                "auth_status": "Logged in using ChatGPT",
                "api_fallback": "forbidden",
            },
            "configuration": {
                "model": "gpt-test",
                "conditions": conditions,
                "service_protocol": "simple",
                "expected_runtime_features": {},
                "expected_build_revision": REVISION,
                "experiment_parameters": experiment_parameters,
            },
            "artifacts": {
                "manifest_sha256": "b" * 64,
                "schema_sha256": "d" * 64,
                "harness_sha256": "c" * 64,
                "runtime_snapshot_sha256": hashlib.sha256(
                    json.dumps(
                        runtime_snapshot,
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode("utf-8")
                ).hexdigest(),
                "service_image_provenance_sha256": hashlib.sha256(
                    json.dumps(
                        image_provenance,
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode("utf-8")
                ).hexdigest(),
            },
        },
        "records": records,
    }


def explicit_arm_payload(run_id, paired_draw_id, experiment_arm, rows):
    payload = draw_payload(run_id, rows)
    payload["experiment_arm"] = experiment_arm
    payload["paired_draw_id"] = paired_draw_id
    payload["manifest"]["conditions"] = ["service_api"]
    payload["run_ledger"]["configuration"]["conditions"] = ["service_api"]
    payload["run_ledger"]["configuration"]["experiment_arm"] = experiment_arm
    payload["run_ledger"]["configuration"]["paired_draw_id"] = paired_draw_id
    return payload


def set_service_modes(payload, modes):
    normalized = list(modes)
    payload["service_retrieval_modes"] = normalized
    payload["experiment_parameters"]["service_retrieval_modes"] = normalized
    payload["run_ledger"]["configuration"]["experiment_parameters"][
        "service_retrieval_modes"
    ] = normalized


def set_service_image(payload, image_id):
    image = dict(payload["service_image_provenance"]["before"])
    image["api_image_id"] = image_id
    provenance = {
        "schema": "brunn-service-image-provenance@v1",
        "stable": True,
        "before": image,
        "after": dict(image),
    }
    payload["service_image_provenance"] = provenance
    payload["experiment_parameters"]["service_image_fingerprint"] = image
    payload["run_ledger"]["configuration"]["experiment_parameters"][
        "service_image_fingerprint"
    ] = image
    payload["run_ledger"]["artifacts"][
        "service_image_provenance_sha256"
    ] = hashlib.sha256(
        json.dumps(
            provenance,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def set_mutation_binding(payload, *, seed, plans, script_sha256="e" * 64):
    parameters = dict(payload["experiment_parameters"])
    parameters.update({
        "mutation_script_sha256": script_sha256,
        "mutation_seed": seed,
        "mutation_plans_sha256": hashlib.sha256(
            json.dumps(
                plans,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
        ).hexdigest(),
    })
    payload["experiment_parameters"] = parameters
    payload["run_ledger"]["configuration"][
        "experiment_parameters"
    ] = parameters
    receipts = {}
    for case_id, plan in plans.items():
        receipts[case_id] = {
            "schema": "brunn-e06-mutation-receipt@v1",
            "case_id": case_id,
            "seed": seed,
            "paths": list(plan["source_refs"]),
            "exactly_three_sources": True,
            "workspace_mutated": True,
            "workspace_vault_byte_identical": True,
            "corpus_revision": f"generation:{seed}",
            "receipts": [
                {
                    "path": target["path"],
                    "pinned_version": 1,
                    "current_version": 2,
                    "before_sha256": target["before_sha256"],
                    "after_sha256": target["after_sha256"],
                    "write_no_op": False,
                    "verified_read_version": 2,
                    "verified_read_sha256": target["after_sha256"],
                }
                for target in plan["targets"]
            ],
        }
    payload["mutation"] = {
        "script": "eval/e06_mutate.py",
        "script_sha256": script_sha256,
        "seed": seed,
        "plans": plans,
        "receipts": receipts,
    }
    payload["run_ledger"]["artifacts"][
        "mutation_sha256"
    ] = hashlib.sha256(
        json.dumps(
            payload["mutation"],
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def mutation_plans(seed, *, variant=""):
    targets = []
    for index in range(3):
        before = f"before-{index}\n"
        after = f"after-{index}-{seed}-{variant}\n"
        targets.append({
            "path": f"Sources/source-{index}.md",
            "fixture": None,
            "operation": "append_delta",
            "before": before,
            "after": after,
            "before_chars": len(before),
            "after_chars": len(after),
            "before_sha256": "sha256:" + hashlib.sha256(
                before.encode("utf-8")
            ).hexdigest(),
            "after_sha256": "sha256:" + hashlib.sha256(
                after.encode("utf-8")
            ).hexdigest(),
        })
    return {
        "case-a": {
            "schema": "brunn-e06-mutation-plan@v1",
            "case_id": "case-a",
            "seed": seed,
            "delta_path": "Deltas/case-a.md",
            "source_refs": [
                target["path"]
                for target in targets
            ],
            "targets": targets,
        },
    }


def step_policy_payload(selected_case_ids):
    value = {
        "schema": "brunn-e09-step-policy@v1",
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
            "second_step_status": "forbidden_without_new_owner_authorization",
        },
        "losing_suite": {
            "manifest": "eval/synthetic_cases.json",
            "manifest_sha256": "b" * 64,
        },
        "step": {
            "selected_case_ids": selected_case_ids,
            "selected_cases": len(selected_case_ids),
            "case_runs": len(selected_case_ids) * 3,
            "maximum_total_usd": 92.0,
        },
        "implementation": {
            "source_revision": REVISION,
            "source_clean": True,
            "harness_sha256": "f" * 64,
        },
    }
    value["authorization_id"] = canonical_json_sha256({
        "schema": value["schema"],
        "source_revision": REVISION,
        "manifest": value["losing_suite"]["manifest"],
        "manifest_sha256": value["losing_suite"]["manifest_sha256"],
        "selected_case_ids": selected_case_ids,
        "deadline_before_ms": 300,
        "deadline_after_ms": 600,
        "maximum_total_usd": 92.0,
    })
    return value


class AggregateDrawTests(unittest.TestCase):
    def write_payload(self, root, name, payload):
        path = root / f"{name}.json"
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        return path

    def write_draw(self, root, draw, rows):
        return self.write_payload(
            root,
            draw,
            draw_payload(draw, rows),
        )

    def test_self_vs_self_has_exact_p_one_and_zero_centered_ci(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                rows = []
                for case_id, claims, passed, chars in (
                    ("case-a", 4, True, 100),
                    ("case-b", 3, False, 200),
                ):
                    rows.extend([
                        (case_id, "service_api", claims, passed, chars),
                        (
                            case_id,
                            "filesystem_sidecar",
                            claims,
                            passed,
                            chars,
                        ),
                    ])
                paths.append(self.write_draw(root, f"draw-{draw}", rows))
            result = aggregate(paths)
            pair = result["pairings"][
                "service_api__vs__filesystem_sidecar"
            ]["overall"]
            self.assertEqual(pair["case_clusters"], 2)
            self.assertEqual(
                pair["exact_mcnemar"]["two_sided_exact_p"],
                1.0,
            )
            self.assertEqual(
                pair["corpus_total_claim_difference"][
                    "bootstrap_95_ci_claims"
                ],
                {"lower": 0.0, "upper": 0.0},
            )
            self.assertTrue(pair["non_inferiority"]["declared"])

    def test_service_api_resume_preserves_service_provenance(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                payload = draw_payload(f"resume-{draw}", [
                    ("case-a", "service_api", 4, True, 100),
                    ("case-a", "filesystem_sidecar", 4, True, 100),
                ])
                payload["manifest"]["conditions"][0] = "service_api_resume"
                payload["run_ledger"]["configuration"]["conditions"][
                    0
                ] = "service_api_resume"
                payload["records"][0]["condition"] = "service_api_resume"
                paths.append(self.write_payload(
                    root,
                    f"resume-{draw}",
                    payload,
                ))
            result = aggregate(paths)
            self.assertEqual(
                result["service_provenance"]["service_arms"],
                ["service_api"],
            )
            self.assertEqual(
                result["input_artifacts"][0]["service_retrieval_modes"],
                ["exact", "lexical"],
            )
            self.assertEqual(
                result["conditions"],
                ["service_api", "filesystem_sidecar"],
            )

    def test_mixed_agent_and_transition_suites_share_service_arm(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                paths.append(self.write_payload(
                    root,
                    f"agent-{draw}",
                    draw_payload(f"agent-{draw}", [
                        ("case-a", "service_api", 4, True, 100),
                        (
                            "case-a",
                            "filesystem_sidecar",
                            4,
                            True,
                            100,
                        ),
                    ]),
                ))
                transition = draw_payload(f"transition-{draw}", [
                    ("case-a", "service_api", 4, True, 100),
                    ("case-a", "filesystem_sidecar", 4, True, 100),
                ])
                transition["benchmark_version"] = "transition-v1"
                transition["manifest"][
                    "benchmark_version"
                ] = "transition-v1"
                transition["manifest"]["conditions"] = [
                    "service_api_resume",
                    "filesystem_rebuild",
                ]
                transition["run_ledger"]["configuration"]["conditions"] = [
                    "service_api_resume",
                    "filesystem_rebuild",
                ]
                transition["records"][0][
                    "condition"
                ] = "service_api_resume"
                transition["records"][1][
                    "condition"
                ] = "filesystem_rebuild"
                paths.append(self.write_payload(
                    root,
                    f"transition-{draw}",
                    transition,
                ))
            result = aggregate(paths)
            self.assertIn(
                "service_api__vs__filesystem_sidecar",
                result["pairings"],
            )
            self.assertIn(
                "service_api__vs__filesystem",
                result["pairings"],
            )
            self.assertEqual(
                result["service_provenance"]["service_arms"],
                ["service_api"],
            )

    def test_repeated_draws_are_collapsed_inside_case_clusters(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                paths.append(self.write_draw(root, f"draw-{draw}", [
                    ("case-a", "service_api", 4, True, 100),
                    ("case-a", "filesystem_sidecar", 2, False, 80),
                    ("case-b", "service_api", 3, False, 90),
                    ("case-b", "filesystem_sidecar", 3, False, 90),
                ]))
            result = aggregate(paths)
            pair = result["pairings"][
                "service_api__vs__filesystem_sidecar"
            ]["overall"]
            self.assertEqual(pair["case_clusters"], 2)
            self.assertEqual(pair["draws_per_case"], [3])
            self.assertEqual(
                pair["exact_mcnemar"]["discordant_cases"],
                1,
            )
            self.assertEqual(pair["case_claim_outcomes"], {
                "a_wins": 1,
                "b_wins": 0,
                "ties": 1,
            })

    def test_incomplete_draw_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = self.write_draw(root, "draw-0", [
                ("case-a", "service_api", 4, True, 100),
            ])
            with self.assertRaisesRegex(ValueError, "incomplete draw"):
                aggregate([path])

    def test_fewer_than_three_complete_draws_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = self.write_draw(root, "draw-0", [
                ("case-a", "service_api", 4, True, 100),
                ("case-a", "filesystem_sidecar", 4, True, 100),
            ])
            with self.assertRaisesRegex(ValueError, "at least 3 complete draws"):
                aggregate([path])

    def test_definitive_aggregate_rejects_missing_or_malformed_character_envelope(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = draw_payload("missing-envelope", [
                ("case-a", "service_api", 4, True, 100),
                ("case-a", "filesystem_sidecar", 4, True, 100),
            ])
            payload["records"][0].pop("response_character_metrics")
            path = self.write_payload(root, "missing-envelope", payload)
            with self.assertRaisesRegex(
                ValueError,
                "canonical response_character_metrics",
            ):
                aggregate([path])

            payload = draw_payload("malformed-envelope", [
                ("case-a", "service_api", 4, True, 100),
                ("case-a", "filesystem_sidecar", 4, True, 100),
            ])
            payload["records"][0]["response_character_metrics"][
                "service_result_chars"
            ] = True
            path = self.write_payload(root, "malformed-envelope", payload)
            with self.assertRaisesRegex(
                ValueError,
                "malformed response character metric service_result_chars",
            ):
                aggregate([path])

    def test_legacy_fields_require_explicit_non_definitive_opt_out(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                payload = draw_payload(f"legacy-{draw}", [
                    ("case-a", "service_api", 4, True, 100),
                    ("case-a", "filesystem_sidecar", 4, True, 100),
                ])
                for record in payload["records"]:
                    record.pop("response_character_metrics")
                    record["grade"].pop("claims")
                    record["grade"].pop("forbidden_hits")
                paths.append(
                    self.write_payload(root, f"legacy-{draw}", payload)
                )
            with self.assertRaisesRegex(
                ValueError,
                "canonical response_character_metrics",
            ):
                aggregate(paths)
            result = aggregate(paths, definitive=False)
            self.assertFalse(result["definitive"])
            self.assertEqual(
                result["input_artifacts"][0]["legacy_derivations"],
                {
                    "response_character_metrics": 2,
                    "claim_metadata": 0,
                    "forbidden_hits": 2,
                },
            )

    def test_exact_value_claim_tags_are_summarized_by_arm_and_suite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm, claims in (("treatment", 4), ("control", 3)):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [("case-a", "service_api", claims, True, 100)],
                    )
                    payload["manifest"]["cases"][0]["rubric"][3][
                        "tags"
                    ] = ["exact_value"]
                    payload["records"][0]["grade"]["claims"][3].update({
                        "tags": ["exact_value"],
                        "exact_value": True,
                    })
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
                required_claim_tags=["exact_value"],
            )
            tags = result["claim_tags"]["by_arm"]
            self.assertEqual(
                tags["treatment"]["overall"]["by_tag"]["exact_value"][
                    "tagged"
                ],
                {"passed": 3, "total": 3, "rate": 1.0},
            )
            self.assertEqual(
                tags["control"]["by_suite"]["synthetic-v1"]["by_tag"][
                    "exact_value"
                ]["tagged"],
                {"passed": 0, "total": 3, "rate": 0.0},
            )
            self.assertEqual(
                tags["treatment"]["overall"]["by_tag"]["exact_value"][
                    "untagged"
                ]["total"],
                9,
            )
            self.assertEqual(
                result["required_strata"]["required_claim_tags"][
                    "exact_value"
                ]["by_arm"]["control"],
                {
                    "tagged_claim_instances": 3,
                    "untagged_claim_instances": 9,
                },
            )

    def test_exact_value_tag_requires_a_real_matching_boolean(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = draw_payload("bad-exact-value", [
                ("case-a", "service_api", 4, True, 100),
                ("case-a", "filesystem_sidecar", 4, True, 100),
            ])
            payload["manifest"]["cases"][0]["rubric"][0]["tags"] = [
                "exact_value"
            ]
            payload["records"][0]["grade"]["claims"][0].update({
                "tags": ["exact_value"],
                "exact_value": 1,
            })
            path = self.write_payload(root, "bad-exact-value", payload)
            with self.assertRaisesRegex(ValueError, "inconsistent exact_value"):
                aggregate([path])

    def test_supersession_family_reports_raw_net_and_complement(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm, family_claims, complement_claims in (
                    ("treatment", 4, 2),
                    ("control", 2, 3),
                ):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [
                            (
                                "case-family",
                                "service_api",
                                family_claims,
                                family_claims == 4,
                                100,
                            ),
                            (
                                "case-complement",
                                "service_api",
                                complement_claims,
                                False,
                                90,
                            ),
                        ],
                    )
                    payload["manifest"]["feature_families"] = {
                        "supersession_dedup": [
                            "case-family",
                            "unselected-parent-case",
                        ],
                    }
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
                required_feature_families=["supersession_dedup"],
            )
            family_pair = result["pairings"][
                "treatment__vs__control"
            ]["by_feature_family"]["supersession_dedup"]
            self.assertEqual(
                family_pair["family"]["paired_draw_instances"][
                    "claim_net_a_minus_b"
                ],
                6,
            )
            self.assertEqual(
                family_pair["complement"]["paired_draw_instances"][
                    "claim_net_a_minus_b"
                ],
                -3,
            )
            self.assertEqual(
                family_pair["family"]["paired_draw_instances"]["by_draw"][0][
                    "a_wins"
                ],
                1,
            )

    def test_prospective_family_exposes_a_paired_complement(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm, prospective_claims in (
                    ("treatment", 4),
                    ("control", 2),
                ):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [
                            (
                                "prospective-case",
                                "service_api",
                                prospective_claims,
                                prospective_claims == 4,
                                100,
                            ),
                            (
                                "ordinary-case",
                                "service_api",
                                4,
                                True,
                                90,
                            ),
                        ],
                    )
                    payload["manifest"]["feature_families"] = {
                        "prospective": ["prospective-case"],
                    }
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
                required_feature_families=["prospective"],
            )
            prospective = result["pairings"][
                "treatment__vs__control"
            ]["by_feature_family"]["prospective"]
            self.assertEqual(prospective["family"]["case_clusters"], 1)
            self.assertEqual(prospective["complement"]["case_clusters"], 1)
            self.assertEqual(
                prospective["complement"]["per_case"][0]["case_id"],
                "ordinary-case",
            )

    def test_required_strata_fail_closed_without_each_complement(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-family-{draw}",
                        f"family-draw-{draw}",
                        arm,
                        [("case-family", "service_api", 4, True, 100)],
                    )
                    payload["manifest"]["feature_families"] = {
                        "prospective": ["case-family"],
                    }
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-family-{draw}",
                        payload,
                    ))
            with self.assertRaisesRegex(
                ValueError,
                "lacks a nonempty family or complement population",
            ):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                    required_feature_families=["prospective"],
                )

            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-tag-{draw}",
                        f"tag-draw-{draw}",
                        arm,
                        [("case-tagged", "service_api", 4, True, 100)],
                    )
                    for rubric in payload["manifest"]["cases"][0]["rubric"]:
                        rubric["tags"] = ["exact_value"]
                    for claim in payload["records"][0]["grade"]["claims"]:
                        claim["tags"] = ["exact_value"]
                        claim["exact_value"] = True
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-tag-{draw}",
                        payload,
                    ))
            with self.assertRaisesRegex(
                ValueError,
                "lacks a nonempty tagged or untagged population",
            ):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                    required_claim_tags=["exact_value"],
                )

    def test_feature_family_membership_must_stay_stable_within_suite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [
                            ("case-a", "service_api", 4, True, 100),
                            ("case-b", "service_api", 4, True, 100),
                        ],
                    )
                    payload["manifest"]["feature_families"] = {
                        "prospective": [
                            "case-a" if draw != 2 else "case-b"
                        ],
                    }
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            with self.assertRaisesRegex(
                ValueError,
                "feature_families changed",
            ):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                )

    def test_forbidden_hits_are_stratified_and_paired(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [
                            ("case-family", "service_api", 4, True, 100),
                            ("case-complement", "service_api", 4, True, 90),
                        ],
                    )
                    payload["manifest"]["feature_families"] = {
                        "supersession_dedup": ["case-family"],
                    }
                    for case in payload["manifest"]["cases"]:
                        case["forbidden"] = ["bad-family", "bad-complement"]
                    if arm == "treatment":
                        payload["records"][0]["grade"]["forbidden_hits"] = [
                            "bad-family"
                        ]
                    else:
                        payload["records"][1]["grade"]["forbidden_hits"] = [
                            "bad-complement"
                        ]
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
            )
            treatment = result["forbidden_hits"]["by_arm"]["treatment"]
            self.assertEqual(treatment["overall"]["hit_instances"], 3)
            self.assertEqual(
                treatment["by_suite"]["synthetic-v1"]["hit_instances"],
                3,
            )
            strata = treatment["by_feature_family"][
                "supersession_dedup"
            ]
            self.assertEqual(strata["family"]["hit_instances"], 3)
            self.assertEqual(strata["complement"]["hit_instances"], 0)
            paired = result["pairings"][
                "treatment__vs__control"
            ]["by_feature_family"]["supersession_dedup"]["family"]
            self.assertEqual(paired["forbidden_hits"]["new_in_a_count"], 3)
            self.assertEqual(
                paired["forbidden_hits"]["new_in_a_instances"][0]["hit"],
                "bad-family",
            )

    def test_all_character_metrics_are_paired_per_case_and_overall(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm, characters in (
                    ("treatment", 100),
                    ("control", 80),
                ):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [(
                            "case-a",
                            "service_api",
                            4,
                            True,
                            characters,
                        )],
                    )
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
            )
            pair = result["pairings"]["treatment__vs__control"]["overall"]
            metrics = pair["response_character_metrics"]
            self.assertEqual(
                metrics["service_result_chars"]["mean_difference"],
                20.0,
            )
            self.assertEqual(
                metrics["service_source_text_chars"]["mean_difference"],
                20.0,
            )
            self.assertEqual(
                metrics["service_metadata_chars"]["mean_difference"],
                0.0,
            )
            self.assertEqual(
                metrics["service_replay_weighted_chars"]["mean_difference"],
                40.0,
            )
            self.assertEqual(
                metrics["model_visible_tool_output_chars"]["mean_difference"],
                20.0,
            )
            self.assertEqual(
                pair["per_case"][0]["response_character_metrics"][
                    "service_source_text_chars"
                ]["mean_a"],
                100.0,
            )

    def test_regrade_keeps_execution_ledger_and_records_grader_revision(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                path = self.write_draw(root, f"draw-{draw}", [
                    ("case-a", "service_api", 4, True, 100),
                    ("case-a", "filesystem_sidecar", 4, True, 100),
                ])
                payload = json.loads(path.read_text(encoding="utf-8"))
                payload["execution_fingerprints"] = {
                    "manifest_sha256": payload["manifest_sha256"],
                    "harness_sha256": payload["harness_sha256"],
                }
                payload["manifest_sha256"] = "f" * 64
                payload["harness_sha256"] = "1" * 64
                payload["regraded_at"] = "2026-07-27T13:00:00-07:00"
                payload["regrade_fingerprints"] = {
                    "manifest_sha256": payload["manifest_sha256"],
                    "harness_sha256": payload["harness_sha256"],
                    "schema_sha256": "2" * 64,
                    "source": {
                        "revision": REGRADING_REVISION,
                        "tracked_source_clean": True,
                        "untracked_source_files": [],
                        "clean": True,
                    },
                    "captured_at": payload["regraded_at"],
                }
                path.write_text(
                    json.dumps(payload) + "\n",
                    encoding="utf-8",
                )
                paths.append(path)
            result = aggregate(paths)
            self.assertEqual(result["source_revision"], REVISION)
            self.assertEqual(result["grading_revision"], REGRADING_REVISION)

    def test_separate_service_invocations_pair_by_explicit_arm_and_draw(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm, claims, passed in (
                    ("treatment", 4, True),
                    ("control", 2, False),
                ):
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(
                        json.dumps(explicit_arm_payload(
                            f"{arm}-run-{draw}",
                            f"draw-{draw}",
                            arm,
                            [("case-a", "service_api", claims, passed, 100)],
                        )) + "\n",
                        encoding="utf-8",
                    )
                    paths.append(path)
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
            )
            pair = result["pairings"]["treatment__vs__control"]["overall"]
            self.assertEqual(result["arms"], ["treatment", "control"])
            self.assertEqual(pair["draws_per_case"], [3])
            self.assertEqual(pair["case_claim_outcomes"]["a_wins"], 1)
            case = pair["per_case"][0]
            self.assertEqual(case["case_pass_votes_a"], 3)
            self.assertEqual(case["case_pass_votes_b"], 0)
            self.assertEqual(len(case["paired_draw_outcomes"]), 3)
            self.assertEqual(
                case["paired_draw_outcomes"][0],
                {
                    "draw": "draw-0",
                    "case_pass_a": True,
                    "case_pass_b": False,
                    "claims_passed_a": 4,
                    "claims_passed_b": 2,
                },
            )

    def test_service_provenance_allows_distinct_containers_not_images(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [("case-a", "service_api", 4, True, 100)],
                    )
                    if arm == "control":
                        set_service_modes(payload, ["exact", "semantic"])
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            with self.assertRaisesRegex(
                ValueError,
                "exactly cover every definitive service-backed arm",
            ):
                aggregate_draws(
                    paths,
                    expected_arms=["treatment", "control"],
                )
            result = aggregate_draws(
                paths,
                expected_arms=["treatment", "control"],
                expected_arm_retrieval_modes={
                    "treatment": ["exact", "lexical"],
                    "control": ["exact", "semantic"],
                },
            )
            provenance = result["service_provenance"]
            self.assertEqual(provenance["status"], "complete")
            self.assertEqual(provenance["api_image_id"], IMAGE_ID)
            self.assertTrue(
                provenance["distinct_container_instances_allowed"]
            )
            self.assertEqual(
                provenance["expected_arm_retrieval_modes"]["control"],
                ["exact", "semantic"],
            )
            containers = {
                artifact["service_image_provenance"]["before"][
                    "api_container_id"
                ]
                for artifact in result["input_artifacts"]
            }
            self.assertEqual(len(containers), 6)

    def test_service_provenance_detects_mode_and_image_tampering(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = draw_payload("tampered-modes", [
                ("case-a", "service_api", 4, True, 100),
                ("case-a", "filesystem_sidecar", 4, True, 100),
            ])
            payload["service_retrieval_modes"] = ["exact", "semantic"]
            path = self.write_payload(root, "tampered-modes", payload)
            with self.assertRaisesRegex(
                ValueError,
                "retrieval modes disagree",
            ):
                aggregate([path])

            payload = draw_payload("tampered-image", [
                ("case-a", "service_api", 4, True, 100),
                ("case-a", "filesystem_sidecar", 4, True, 100),
            ])
            payload["service_image_provenance"]["after"][
                "api_image_id"
            ] = "sha256:" + "1" * 64
            path = self.write_payload(root, "tampered-image", payload)
            with self.assertRaisesRegex(
                ValueError,
                "image fingerprint is malformed, unstable",
            ):
                aggregate([path])

            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-image-{draw}",
                        f"image-draw-{draw}",
                        arm,
                        [("case-a", "service_api", 4, True, 100)],
                    )
                    if arm == "control" and draw == 2:
                        set_service_image(
                            payload,
                            "sha256:" + "1" * 64,
                        )
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-image-{draw}",
                        payload,
                    ))
            with self.assertRaisesRegex(
                ValueError,
                "do not share one api_image_id",
            ):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                )

    def test_explicit_arm_aggregate_rejects_an_incomplete_draw(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                arms = ("treatment", "control") if draw != 1 else ("treatment",)
                for arm in arms:
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(
                        json.dumps(explicit_arm_payload(
                            f"{arm}-run-{draw}",
                            f"draw-{draw}",
                            arm,
                            [("case-a", "service_api", 4, True, 100)],
                        )) + "\n",
                        encoding="utf-8",
                    )
                    paths.append(path)
            with self.assertRaisesRegex(ValueError, "incomplete or mixed arm set"):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                )

    def test_mutation_provenance_is_bound_within_each_paired_draw(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            expected_plans = {}
            for draw in range(3):
                paired_draw_id = f"e06-draw{draw}"
                plans = mutation_plans(paired_draw_id)
                expected_plans[paired_draw_id] = hashlib.sha256(
                    json.dumps(
                        plans,
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode("utf-8")
                ).hexdigest()
                for arm in ("e06-B", "e06-A"):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        paired_draw_id,
                        arm,
                        [("case-a", "service_api", 4, True, 100)],
                    )
                    set_mutation_binding(
                        payload,
                        seed=paired_draw_id,
                        plans=plans,
                    )
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            result = aggregate(
                paths,
                expected_arms=["e06-B", "e06-A"],
            )
            provenance = result["mutation_provenance"]
            self.assertEqual(provenance["status"], "complete")
            self.assertEqual(
                provenance["mutation_script_sha256"],
                "e" * 64,
            )
            self.assertEqual(
                {
                    draw["paired_draw_id"]:
                    draw["mutation_plans_sha256"]
                    for draw in provenance["draw_bindings"]
                },
                expected_plans,
            )
            self.assertTrue(all(
                draw["experiment_arms"] == ["e06-A", "e06-B"]
                for draw in provenance["draw_bindings"]
            ))

    def test_mutation_provenance_rejects_cross_arm_plan_or_seed_drift(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)

            def write_inputs(*, drift):
                paths = []
                for draw in range(3):
                    paired_draw_id = f"e06-draw{draw}"
                    for arm in ("e06-B", "e06-A"):
                        seed = paired_draw_id
                        plans = mutation_plans(paired_draw_id)
                        if draw == 1 and arm == "e06-A":
                            if drift == "seed":
                                seed = "wrong-seed"
                                plans = mutation_plans(seed)
                            elif drift == "plans":
                                plans = mutation_plans(
                                    paired_draw_id,
                                    variant="wrong-plan",
                                )
                        payload = explicit_arm_payload(
                            f"{drift}-{arm}-run-{draw}",
                            paired_draw_id,
                            arm,
                            [("case-a", "service_api", 4, True, 100)],
                        )
                        set_mutation_binding(
                            payload,
                            seed=seed,
                            plans=plans,
                        )
                        paths.append(self.write_payload(
                            root,
                            f"{drift}-{arm}-{draw}",
                            payload,
                        ))
                return paths

            with self.assertRaisesRegex(
                ValueError,
                "must equal its paired-draw ID",
            ):
                aggregate(
                    write_inputs(drift="seed"),
                    expected_arms=["e06-B", "e06-A"],
                )
            with self.assertRaisesRegex(
                ValueError,
                "mutation plans differ across arms",
            ):
                aggregate(
                    write_inputs(drift="plans"),
                    expected_arms=["e06-B", "e06-A"],
                )

    def test_mutation_provenance_rejects_unbound_or_mixed_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                paired_draw_id = f"e06-draw{draw}"
                for arm in ("e06-B", "e06-A"):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        paired_draw_id,
                        arm,
                        [("case-a", "service_api", 4, True, 100)],
                    )
                    if not (draw == 2 and arm == "e06-A"):
                        set_mutation_binding(
                            payload,
                            seed=paired_draw_id,
                            plans=mutation_plans(paired_draw_id),
                        )
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-{draw}",
                        payload,
                    ))
            with self.assertRaisesRegex(
                ValueError,
                "mixes mutation-bound and non-mutation",
            ):
                aggregate(
                    paths,
                    expected_arms=["e06-B", "e06-A"],
                )

            payload = json.loads(paths[0].read_text(encoding="utf-8"))
            payload["mutation"]["plans"]["case-a"]["seed"] = "tampered"
            paths[0].write_text(
                json.dumps(payload) + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                ValueError,
                "run ledger does not match the mutation evidence",
            ):
                aggregate(
                    paths[:1],
                    definitive=True,
                )

    def test_predeclared_case_extension_is_explicit_and_arm_complete(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project_root = Path(__file__).resolve().parents[1]
            parent_path = project_root / "eval/work_cases.json"
            parent_body = parent_path.read_bytes()
            parent = json.loads(parent_body)
            parent_case_ids = [
                case["id"] for case in parent["cases"]
            ]
            base_case_ids = [
                case["id"]
                for case in parent["cases"]
                if case.get("active", True) is not False
            ]
            extension_case_ids = [
                "warmind-parser-learning",
                "star-rupture-plan-revision",
            ]
            parent_sha256 = hashlib.sha256(parent_body).hexdigest()
            plan = {
                "schema": "brunn-case-extension-plan@v1",
                "experiment_id": "E04-test",
                "suites": [{
                    "suite": parent["benchmark_version"],
                    "parent_manifest": "eval/work_cases.json",
                    "parent_manifest_sha256": parent_sha256,
                    "parent_case_ids": parent_case_ids,
                    "base_case_ids": base_case_ids,
                    "extension_case_ids": extension_case_ids,
                    "base_draws": 3,
                    "extension_draws": 2,
                }],
            }
            plan_path = root / "case-extension-plan.json"
            plan_path.write_text(
                json.dumps(plan, indent=2) + "\n",
                encoding="utf-8",
            )
            plan_sha256 = hashlib.sha256(plan_path.read_bytes()).hexdigest()
            paths = []
            for draw in range(5):
                case_ids = (
                    base_case_ids
                    if draw < 3
                    else extension_case_ids
                )
                for arm in ("treatment", "control"):
                    rows = [
                        (case_id, "service_api", 4, True, 100)
                        for case_id in case_ids
                    ]
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        rows,
                    )
                    payload["benchmark_version"] = parent[
                        "benchmark_version"
                    ]
                    payload["manifest"]["benchmark_version"] = parent[
                        "benchmark_version"
                    ]
                    payload["manifest_sha256"] = parent_sha256
                    payload["run_ledger"]["artifacts"][
                        "manifest_sha256"
                    ] = parent_sha256
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(
                        json.dumps(payload) + "\n",
                        encoding="utf-8",
                    )
                    paths.append(path)
            for draw in range(3):
                for arm in ("treatment", "control"):
                    paths.append(self.write_payload(
                        root,
                        f"{arm}-unextended-{draw}",
                        explicit_arm_payload(
                            f"{arm}-unextended-run-{draw}",
                            f"unextended-draw-{draw}",
                            arm,
                            [(
                                "unextended-case",
                                "service_api",
                                4,
                                True,
                                100,
                            )],
                        ),
                    ))
            with self.assertRaisesRegex(ValueError, "case-extension-plan"):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                )
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
                case_extension_plan=plan_path,
                case_extension_plan_sha256=plan_sha256,
            )
            pair = result["pairings"]["treatment__vs__control"]["overall"]
            self.assertEqual(pair["draws_per_case"], [3, 5])
            self.assertTrue(result["case_extension"]["enabled"])
            self.assertEqual(
                result["case_extension"]["suites"][
                    parent["benchmark_version"]
                ]["base_draw_ids"],
                ["draw-0", "draw-1", "draw-2"],
            )

    def test_checked_in_e04_extension_plan_is_frozen_to_parent_manifests(self):
        project_root = Path(__file__).resolve().parents[1]
        plan_path = project_root / "eval/e04_case_extension_plan.json"
        expected_sha256 = (
            "13a4a41c3dc123c91e83e0006e98704d"
            "88e3fccafc7f32c998941c415c77e275"
        )
        self.assertEqual(
            hashlib.sha256(plan_path.read_bytes()).hexdigest(),
            expected_sha256,
        )
        plan = load_case_extension_plan(
            plan_path,
            expected_sha256,
        )
        self.assertEqual(plan["experiment_id"], "E04")
        self.assertEqual(
            set(plan["suites"]),
            {"0.2", "rupture-ops-v0.1"},
        )
        self.assertEqual(
            plan["suites"]["0.2"]["extension_case_ids"],
            [
                "warmind-parser-learning",
                "star-rupture-plan-revision",
            ],
        )

    def test_runtime_snapshot_is_ledger_bound_and_tamper_evident(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm in ("treatment", "control"):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [("case-a", "service_api", 4, True, 100)],
                    )
                    snapshot = {
                        "schema": "brunn-service-runtime-snapshot@v1",
                        "captured_at": "2026-07-27T12:00:30-07:00",
                        "status": "ready",
                        "build_revision": REVISION,
                        "runtime_features": {"flag": arm == "treatment"},
                    }
                    payload["service_runtime_snapshot"] = snapshot
                    payload["expected_runtime_features"] = {
                        "flag": arm == "treatment",
                    }
                    payload["expected_build_revision"] = REVISION
                    payload["run_ledger"]["configuration"].update({
                        "expected_runtime_features": payload[
                            "expected_runtime_features"
                        ],
                        "expected_build_revision": REVISION,
                    })
                    payload["run_ledger"]["artifacts"][
                        "runtime_snapshot_sha256"
                    ] = hashlib.sha256(
                        json.dumps(
                            snapshot,
                            sort_keys=True,
                            separators=(",", ":"),
                        ).encode("utf-8")
                    ).hexdigest()
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
                    paths.append(path)
            aggregate(paths, expected_arms=["treatment", "control"])
            tampered = json.loads(paths[0].read_text(encoding="utf-8"))
            tampered["service_runtime_snapshot"]["runtime_features"]["flag"] = False
            paths[0].write_text(json.dumps(tampered) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "runtime snapshot"):
                aggregate(paths, expected_arms=["treatment", "control"])

    def test_optional_claim_mcnemar_is_one_sided_and_draw_grouped(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(3):
                for arm, outcome in (("treatment", True), ("control", False)):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        [(
                            "case-a",
                            "service_api",
                            4 if outcome else 0,
                            outcome,
                            100,
                        )],
                    )
                    payload["records"][0]["grade"]["claims"] = [
                        {
                            "id": f"c{index}",
                            "pass": outcome,
                            "tags": [],
                            "exact_value": False,
                        }
                        for index in range(1, 5)
                    ]
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
                    paths.append(path)
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
                claim_mcnemar_alternative="a_greater",
            )
            pair = result["pairings"]["treatment__vs__control"]["overall"]
            claim_test = pair["claim_level_exact_mcnemar"]
            self.assertEqual(claim_test["claim_clusters"], 4)
            self.assertEqual(claim_test["draws_per_claim"], [3])
            self.assertEqual(claim_test["one_sided_exact_p"], 0.0625)
            self.assertIn("two_sided_exact_p", pair["exact_mcnemar"])

    def test_authorized_600ms_step_reuses_only_the_policy_subset(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            policy_path = root / "step-policy.json"
            policy_path.write_text(
                json.dumps(step_policy_payload(["case-b"])) + "\n",
                encoding="utf-8",
            )
            policy_sha256 = hashlib.sha256(
                policy_path.read_bytes()
            ).hexdigest()
            from eval.e09_step_authorization import load_step_authorization

            binding = load_step_authorization(
                policy_path,
                policy_sha256,
                expected_source_revision=REVISION,
            )
            paths = []
            for draw in range(3):
                for arm, rows in (
                    (
                        "e09-deadline-cache",
                        [
                            ("case-a", "service_api", 4, True, 100),
                            ("case-b", "service_api", 3, False, 120),
                        ],
                    ),
                    (
                        "e09-deadline-cache-600",
                        [
                            ("case-b", "service_api", 4, True, 110),
                        ],
                    ),
                ):
                    payload = explicit_arm_payload(
                        f"{arm}-run-{draw}",
                        f"draw-{draw}",
                        arm,
                        rows,
                    )
                    parameters = dict(payload["experiment_parameters"])
                    if arm == "e09-deadline-cache-600":
                        parameters["e09_step_authorization"] = binding
                    payload["experiment_parameters"] = parameters
                    payload["run_ledger"]["configuration"][
                        "experiment_parameters"
                    ] = parameters
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(
                        json.dumps(payload) + "\n",
                        encoding="utf-8",
                    )
                    paths.append(path)
            result = aggregate(
                paths,
                expected_arms=[
                    "e09-deadline-cache",
                    "e09-deadline-cache-600",
                ],
                e09_step_policy=policy_path,
                e09_step_policy_sha256=policy_sha256,
            )
            pair = result["pairings"][
                "e09-deadline-cache__vs__e09-deadline-cache-600"
            ]["overall"]
            self.assertEqual(pair["case_clusters"], 1)
            self.assertEqual(pair["per_case"][0]["case_id"], "case-b")
            self.assertEqual(
                result["e09_step_authorization"]["authorization_id"],
                binding["authorization_id"],
            )


if __name__ == "__main__":
    unittest.main()

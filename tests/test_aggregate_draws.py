import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from eval.aggregate_draws import aggregate  # noqa: E402


REVISION = "a" * 40
REGRADING_REVISION = "e" * 40


def draw_payload(run_id, rows):
    conditions = ["service_api", "filesystem_sidecar"]
    case_ids = sorted({row[0] for row in rows})
    records = []
    for case_id, condition, claims, case_pass, characters in rows:
        records.append({
            "case_id": case_id,
            "condition": condition,
            "error": None,
            "grade": {
                "claims_passed": claims,
                "claims_total": 4,
                "pass": case_pass,
            },
            "model_visible_tool_output_chars": characters,
            "persisted_checkpoint": {"objective": "resume"},
        })
    return {
        "benchmark_version": "synthetic-v1",
        "run_id": run_id,
        "manifest_sha256": "b" * 64,
        "harness_sha256": "c" * 64,
        "manifest": {
            "benchmark_version": "synthetic-v1",
            "model": "gpt-test",
            "conditions": conditions,
            "cases": [{"id": case_id} for case_id in case_ids],
        },
        "reasoning_billing": {
            "route": "chatgpt_subscription",
            "api_fallback": "forbidden",
            "auth_status": "Logged in using ChatGPT",
        },
        "run_ledger": {
            "schema": "straylight-eval-run-ledger@v1",
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
            },
            "artifacts": {
                "manifest_sha256": "b" * 64,
                "schema_sha256": "d" * 64,
                "harness_sha256": "c" * 64,
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


class AggregateDrawTests(unittest.TestCase):
    def write_draw(self, root, draw, rows):
        path = root / f"{draw}.json"
        path.write_text(
            json.dumps(draw_payload(draw, rows)) + "\n",
            encoding="utf-8",
        )
        return path

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

    def test_predeclared_case_extension_is_explicit_and_arm_complete(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for draw in range(5):
                case_ids = ("case-a", "case-b") if draw < 3 else ("case-b",)
                for arm in ("treatment", "control"):
                    rows = [
                        (case_id, "service_api", 4, True, 100)
                        for case_id in case_ids
                    ]
                    path = root / f"{arm}-{draw}.json"
                    path.write_text(
                        json.dumps(explicit_arm_payload(
                            f"{arm}-run-{draw}",
                            f"draw-{draw}",
                            arm,
                            rows,
                        )) + "\n",
                        encoding="utf-8",
                    )
                    paths.append(path)
            with self.assertRaisesRegex(ValueError, "allow-case-extension"):
                aggregate(
                    paths,
                    expected_arms=["treatment", "control"],
                )
            result = aggregate(
                paths,
                expected_arms=["treatment", "control"],
                allow_case_extension=True,
            )
            pair = result["pairings"]["treatment__vs__control"]["overall"]
            self.assertEqual(pair["draws_per_case"], [3, 5])
            self.assertTrue(result["case_extension"]["enabled"])

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
                        "schema": "straylight-service-runtime-snapshot@v1",
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
                        {"id": f"c{index}", "pass": outcome}
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


if __name__ == "__main__":
    unittest.main()

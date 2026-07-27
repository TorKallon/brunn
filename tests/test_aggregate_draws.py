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


if __name__ == "__main__":
    unittest.main()

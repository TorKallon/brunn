from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MVP = ROOT / "docs" / "mvp"


def document(name: str) -> str:
    return (MVP / name).read_text(encoding="utf-8")


class ExecutableExperimentDocsTests(unittest.TestCase):
    def test_e05_freezes_targeted_artifacts_and_aggregate(self) -> None:
        text = document("E05-lexical-consolidation-guard.md")
        for arm in ("A", "B"):
            self.assertIn(
                f"e05-lexical-targeted-arm{arm}-draw${{N}}.json",
                text,
            )
            self.assertIn(f"e05-targeted-arm{arm}-draw${{N}}", text)
        self.assertIn(
            "E05_TARGETED=(results/2026-MM-DD-e05-lexical-targeted-"
            "arm{A,B}-draw{1,2,3,4,5}.json)",
            text,
        )
        self.assertIn(
            "--out results/2026-MM-DD-e05-targeted-aggregate.json",
            text,
        )
        self.assertNotIn("analogous targeted-only array", text)

    def test_e07_keeps_three_arm_and_service_extension_separate(self) -> None:
        text = document("E07-supersession-experiment.md")
        self.assertIn(
            "E07_MAIN=(results/2026-MM-DD-e07-supersession-"
            "{flag,base,filesystem}-draw{1,2,3}.json)",
            text,
        )
        self.assertIn(
            "E07_SERVICE5=(results/2026-MM-DD-e07-supersession-"
            "{flag,base}-draw{1,2,3,4,5}.json)",
            text,
        )
        self.assertIn(
            "--out results/2026-MM-DD-e07-service-five-draw-aggregate.json",
            text,
        )
        self.assertIn("218 runs = **$52.32**", text)
        self.assertNotIn("{base,flag,fs}", text)
        self.assertNotIn("{flag,base,fs}", text)

        filesystem_command = next(
            line
            for line in text.splitlines()
            if "run --condition filesystem" in line
        )
        self.assertIn("e07-supersession-filesystem-draw${N}.json", filesystem_command)
        self.assertNotIn("--expect-", filesystem_command)

    def test_e07_names_measurable_write_latency_preflight(self) -> None:
        text = document("E07-supersession-experiment.md")
        self.assertIn("e07-base-write-latency.json", text)
        self.assertIn("e07-supersession-write-calibration.json", text)
        self.assertIn("e07-supersession-write-latency.json", text)
        self.assertIn("--query-budget-profile e07-supersession", text)
        self.assertIn('E07_QUERY_BUDGET_CONTRACT="results/', text)
        self.assertIn("scales[].concurrent_probe.write_p95_ms", text)
        self.assertIn("≤58.0ms", text)

    def test_e08_freezes_reviewed_query_contract_before_acceptance(self) -> None:
        text = document("E08-intention-ledger-experiment.md")
        self.assertIn("--query-budget-profile calibration", text)
        self.assertIn('E08_QUERY_BUDGET_CONTRACT="results/', text)
        self.assertIn('p["profile"]=="e08-intention-ledger"', text)
        self.assertIn("E08_QUERY_BUDGET_SHA256=", text)
        self.assertIn("chmod 0444", text)
        self.assertIn(
            "--query-budget-profile e08-intention-ledger "
            '--query-budget-contract "$E08_QUERY_BUDGET_CONTRACT"',
            text,
        )
        self.assertIn("Do not copy thresholds from `default-safe`", text)

    def test_e08_commands_cover_every_arm_audit_and_aggregate(self) -> None:
        text = document("E08-intention-ledger-experiment.md")
        expected_outputs = (
            "e08-intention-base-draw${N}.json",
            "e08-intention-flag-draw${N}.json",
            "e08-prospective-base-draw${N}.json",
            "e08-prospective-flag-draw${N}.json",
            "e08-prospective-filesystem-draw${N}.json",
            "e08-intention-audit.json",
            "e08-full-aggregate.json",
            "e08-prospective-aggregate.json",
        )
        for output in expected_outputs:
            self.assertIn(output, text)
        self.assertIn(
            "E08_FULL=(results/2026-MM-DD-e08-intention-"
            "{flag,base}-draw{1,2,3}.json)",
            text,
        )
        self.assertIn(
            "E08_PROSPECTIVE=(results/2026-MM-DD-e08-prospective-"
            "{flag,base,filesystem}-draw{1,2,3}.json)",
            text,
        )
        self.assertIn(
            "--expected-arm e08-filesystem "
            "--out results/2026-MM-DD-e08-prospective-aggregate.json",
            text,
        )

        filesystem_command = next(
            line
            for line in text.splitlines()
            if "run --condition filesystem" in line
        )
        self.assertNotIn("--expect-", filesystem_command)


if __name__ == "__main__":
    unittest.main()

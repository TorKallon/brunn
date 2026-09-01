from __future__ import annotations

import hashlib
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MVP = ROOT / "docs" / "mvp"


def document(name: str) -> str:
    return (MVP / name).read_text(encoding="utf-8")


class ExecutableExperimentDocsTests(unittest.TestCase):
    def test_shared_infrastructure_freezes_local_cli_failure_accounting(
        self,
    ) -> None:
        text = document("Experiment-run-infrastructure.md")
        for contract in (
            "`http_status=0` `failed:X`",
            "`local_cli_failures`",
            "only when a later",
            "successful HTTP operation named `X`",
            "`model_visible_tool_output_chars`",
            "excluded from service call/HTTP-call",
            "Any measured 4xx or `denied:*` operation",
            "any unrecovered local failure",
            "canonical shell-safe positional-JSON checkpoint",
        ):
            self.assertIn(contract, text)

    def test_e04_through_e08_freeze_rejected_d02_posture(self) -> None:
        names = (
            "E04-result-budget-experiment.md",
            "E05-lexical-consolidation-guard.md",
            "E06-resume-delta-experiment.md",
            "E07-supersession-experiment.md",
            "E08-intention-ledger-experiment.md",
        )
        for name in names:
            with self.subTest(name=name):
                text = document(name)
                self.assertIn("E02 rejected D02", text)
                self.assertIn("BRUNN_VERBATIM_SPANS=false", text)
                self.assertIn(
                    "--expect-feature-flag verbatim_spans=off",
                    text,
                )
                self.assertIn("cannot rehabilitate D02", text)
                self.assertNotIn("verbatim_spans=on", text)

                measured_commands = [
                    line
                    for line in text.splitlines()
                    if (
                        "performance_eval.py run" in line
                        or (
                            "agent_work_eval.py" in line
                            and "--condition service_api" in line
                        )
                        or (
                            "transition_eval.py" in line
                            and "--condition service_api_resume" in line
                        )
                    )
                ]
                self.assertTrue(measured_commands)
                for command in measured_commands:
                    self.assertIn(
                        "--expect-feature-flag verbatim_spans=off",
                        command,
                    )

    def test_e09_through_e11_record_current_prerequisite_aborts(self) -> None:
        e09 = document("E09-semantic-existence-experiment.md")
        self.assertIn("Status: Prerequisite abort", e09)
        self.assertIn("E03 Mode 2 failed", e09)
        self.assertIn("quality backfill was not run", e09)
        self.assertIn("BRUNN_VERBATIM_SPANS=false", e09)
        self.assertNotIn("verbatim_spans=on", e09)

        e10 = document("E10-combined-preflight.md")
        self.assertIn("Status: Prerequisite abort", e10)
        self.assertIn("immutable launch flag manifest is incomplete", e10)
        self.assertIn("E01 is\ncomplete", e10)
        self.assertIn("E04 rejected D01", e10)
        self.assertIn("E06 rejected D03", e10)
        normalized_e10 = " ".join(e10.split())
        self.assertIn(
            "E07's mechanism passed but its unprompted-adoption gate failed",
            normalized_e10,
        )
        self.assertIn(
            "E08 stopped before a D05 feature verdict",
            normalized_e10,
        )
        self.assertIn(
            "E09 remains prerequisite-aborted because E03 Mode 2 failed",
            normalized_e10,
        )
        self.assertIn("--expect-feature-flag verbatim_spans=off", e10)
        self.assertIn("--expect-feature-flag resume_deltas=off", e10)
        self.assertIn("--expect-feature-flag supersession_demotion=off", e10)
        self.assertIn("--expect-feature-flag intention_ledger=off", e10)
        self.assertIn("--expect-feature-flag lexical_single_scan=off", e10)
        self.assertNotIn("verbatim_spans=on", e10)
        self.assertNotIn("lexical_single_scan=on", e10)
        self.assertIn(
            "calibration artifacts intentionally fail acceptance",
            e10,
        )
        self.assertIn(
            "expected\n  ineligible verdict is not itself an abort",
            e10,
        )

        e11 = document("E11-wiki-link-leads-experiment.md")
        self.assertIn("Status: Prerequisite abort", e11)
        self.assertIn("E02 rejected", e11)
        self.assertIn(
            "D06 and its\n`link_leads` runtime surface are not implemented",
            e11,
        )
        self.assertIn("owner-authored,", e11)
        self.assertIn("owner-signed-off manifest", e11)

    def test_e09_e11_abort_artifacts_are_zero_cost_nonverdicts(self) -> None:
        expected = {
            "E09": (
                351,
                84.24,
                "a18980a2e1dbfc373b3aac4fa91b01a40abe582a5edac12d6f33e750674f0d4f",
            ),
            "E10": (
                354,
                84.96,
                "6581cd5faa3d3769426594fc727a9ed3e2f88688849ac07acaa954225520c921",
            ),
            "E11": (
                168,
                40.32,
                "8632bcfc5f79647086f342104664993670c8585ae05dfcd17f97a1cc07228976",
            ),
        }
        for experiment, (case_runs, equivalent_usd, frozen_sha256) in expected.items():
            with self.subTest(experiment=experiment):
                artifact_path = (
                    ROOT
                    / "results"
                    / (
                        "2026-07-28-"
                        f"{experiment.lower()}-prerequisite-abort.json"
                    )
                )
                artifact_bytes = artifact_path.read_bytes()
                self.assertEqual(
                    hashlib.sha256(artifact_bytes).hexdigest(),
                    frozen_sha256,
                )
                artifact = json.loads(artifact_bytes)
                self.assertTrue(
                    artifact["schema"].endswith(
                        "-experiment-prerequisite-abort@v1"
                    )
                )
                self.assertEqual(artifact["status"], "prerequisite_abort")
                self.assertFalse(artifact["experiment_executed"])
                self.assertEqual(
                    artifact["experiment_verdict"],
                    "not_evaluated",
                )
                self.assertEqual(
                    artifact["execution"]["reasoning_case_runs"],
                    0,
                )
                self.assertEqual(
                    artifact["execution"]["embedding_requests"],
                    0,
                )
                self.assertEqual(
                    artifact["future_cost_preflight"]["reasoning"][
                        "base_case_runs"
                    ],
                    case_runs,
                )
                self.assertEqual(
                    artifact["future_cost_preflight"]["reasoning"][
                        "base_subscription_equivalent_usd"
                    ],
                    equivalent_usd,
                )
                self.assertTrue(
                    all(value == 0 for value in artifact["actual_cost"].values())
                )

    def test_e02_uses_a_calibrated_mode_specific_query_contract(self) -> None:
        text = document("E02-verbatim-identifier-gate.md")
        self.assertIn("--query-budget-profile e02-verbatim", text)
        self.assertIn(
            "--query-budget-contract eval/query_budgets.e02-verbatim.json",
            text,
        )
        self.assertIn("--query-budget-profile calibration", text)
        self.assertIn("Calibration is intentionally ineligible", text)
        self.assertIn(
            "declares `verbatim_spans` as the sole experiment",
            text,
        )

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

    def test_e04_e05_bind_service_modes_and_pair_query_counts(self) -> None:
        for name in (
            "E04-result-budget-experiment.md",
            "E05-lexical-consolidation-guard.md",
        ):
            with self.subTest(name=name):
                text = document(name)
                service_commands = [
                    line
                    for line in text.splitlines()
                    if (
                        "agent_work_eval.py" in line
                        and "--condition service_api" in line
                    )
                ]
                self.assertTrue(service_commands)
                for command in service_commands:
                    self.assertIn(
                        "--service-retrieval-modes exact lexical",
                        command,
                    )
                    self.assertIn(
                        '--api-container "$API_CONTAINER"',
                        command,
                    )
                self.assertIn("eval/compare_query_counts.py", text)

        e04 = document("E04-result-budget-experiment.md")
        self.assertIn("--feature search_top1_hydration", e04)
        self.assertIn("--min-delta 0 --max-delta 5", e04)
        self.assertIn("--allowed-delta 0 --allowed-delta 5", e04)
        self.assertIn("--require-strict-increase", e04)
        self.assertIn(
            "e04-hydration-query-count-comparison.json",
            e04,
        )
        self.assertIn("fresh definitive E01 personal artifact", e04)
        self.assertIn("exactly 60 graded claims", e04)

        e05 = document("E05-lexical-consolidation-guard.md")
        self.assertIn("--feature lexical_single_scan", e05)
        self.assertIn("--min-delta -2 --max-delta 0", e05)
        self.assertIn("--require-strict-improvement", e05)
        self.assertIn("e05-query-count-comparison.json", e05)

    def test_e02_e11_explicit_service_examples_bind_current_runtime(
        self,
    ) -> None:
        for name in (
            "E02-verbatim-identifier-gate.md",
            "E11-wiki-link-leads-experiment.md",
        ):
            with self.subTest(name=name):
                service_commands = [
                    line
                    for line in document(name).splitlines()
                    if (
                        "agent_work_eval.py" in line
                        and "--condition service_api" in line
                    )
                ]
                self.assertTrue(service_commands)
                for command in service_commands:
                    self.assertIn(
                        '--api-container "$API_CONTAINER"',
                        command,
                    )
                    self.assertIn(
                        '--expect-build-revision "$REV"',
                        command,
                    )
                    self.assertIn(
                        "--expect-feature-flag semantic_lane=off",
                        command,
                    )

        e11 = document("E11-wiki-link-leads-experiment.md")
        self.assertIn("--expect-feature-flag verbatim_spans=on", e11)
        self.assertIn(
            "remain ineligible until D06 exposes authenticated runtime "
            "status keys",
            e11,
        )

    def test_e07_keeps_three_arm_and_service_extension_separate(self) -> None:
        text = document("E07-supersession-experiment.md")
        self.assertEqual(
            text.count("--require-feature-family supersession_dedup"),
            2,
        )
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

    def test_e06_e08_bind_service_image_and_exact_lexical_modes(self) -> None:
        for name in (
            "E06-resume-delta-experiment.md",
            "E07-supersession-experiment.md",
            "E08-intention-ledger-experiment.md",
        ):
            with self.subTest(name=name):
                text = document(name)
                service_commands = [
                    line
                    for line in text.splitlines()
                    if (
                        (
                            "agent_work_eval.py" in line
                            and "--condition service_api" in line
                        )
                        or (
                            "transition_eval.py" in line
                            and "--condition service_api_resume" in line
                        )
                    )
                ]
                self.assertTrue(service_commands)
                for command in service_commands:
                    self.assertIn(
                        "--service-retrieval-modes exact lexical",
                        command,
                    )
                    self.assertIn(
                        '--api-container "$API_CONTAINER"',
                        command,
                    )
                    self.assertIn(
                        '--expect-build-revision "$REV"',
                        command,
                    )

        e07 = document("E07-supersession-experiment.md")
        self.assertIn("--experiment-arm e07-adoption", e07)
        self.assertIn(
            '--paired-draw-id "e07-adoption-draw${N}"',
            e07,
        )
        self.assertIn("aggregate-adoption", e07)
        self.assertIn("--expected-draw e07-adoption-draw3", e07)
        self.assertIn("both supersession and intention adoption", e07)

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
        full_aggregate_command = next(
            line for line in text.splitlines() if "E08_FULL=" in line
        )
        prospective_aggregate_command = next(
            line for line in text.splitlines() if "E08_PROSPECTIVE=" in line
        )
        self.assertIn(
            "--require-feature-family prospective",
            full_aggregate_command,
        )
        self.assertNotIn(
            "--require-feature-family prospective",
            prospective_aggregate_command,
        )
        self.assertIn(
            "--expected-arm e08-filesystem",
            text,
        )
        self.assertIn(
            "--out results/2026-MM-DD-e08-prospective-aggregate.json",
            text,
        )
        self.assertIn("--full-manifest eval/recent_work_cases.json", text)
        self.assertIn(
            "--prospective-manifest eval/e08_prospective_cases.json",
            text,
        )
        self.assertIn("--expected-full-draw e08-full-draw3", text)
        self.assertIn(
            "--expected-prospective-draw e08-prospective-draw3",
            text,
        )
        self.assertIn("exits nonzero", text)
        self.assertNotIn("Open p95 delta > 25ms", text)

        filesystem_command = next(
            line
            for line in text.splitlines()
            if "run --condition filesystem" in line
        )
        self.assertNotIn("--expect-", filesystem_command)

    def test_definitive_aggregates_bind_exact_service_retrieval_modes(
        self,
    ) -> None:
        required_bindings = {
            "E01-paired-draw-machinery-and-baseline.md": {
                "service_api=exact,lexical": 1,
                "service_api_resume=exact,lexical": 1,
            },
            "E02-verbatim-identifier-gate.md": {
                "verbatim-on=exact,lexical": 1,
                "verbatim-off=exact,lexical": 1,
            },
            "E05-lexical-consolidation-guard.md": {
                "e05-B=exact,lexical": 2,
                "e05-A=exact,lexical": 2,
            },
            "E06-resume-delta-experiment.md": {
                "e06-B=exact,lexical": 1,
                "e06-A=exact,lexical": 1,
            },
            "E07-supersession-experiment.md": {
                "e07-flag=exact,lexical": 2,
                "e07-base=exact,lexical": 2,
            },
            "E08-intention-ledger-experiment.md": {
                "e08-flag=exact,lexical": 2,
                "e08-base=exact,lexical": 2,
            },
            "E09-semantic-existence-experiment.md": {
                "e09-deadline-cache=exact,lexical,semantic": 2,
                "e09-unbounded-semantic=exact,lexical,semantic": 1,
                "e09-no-semantic=exact,lexical": 1,
                "e09-deadline-cache-600=exact,lexical,semantic": 1,
            },
            "E10-combined-preflight.md": {
                "service_api=exact,lexical": 1,
                "service_api_resume=exact,lexical": 1,
            },
            "E11-wiki-link-leads-experiment.md": {
                "e11-treatment=exact,lexical": 1,
                "e11-control=exact,lexical": 1,
            },
        }
        for name, bindings in required_bindings.items():
            with self.subTest(name=name):
                text = document(name)
                for binding, minimum_count in bindings.items():
                    rendered = (
                        "--expected-arm-retrieval-modes "
                        f"{binding}"
                    )
                    self.assertGreaterEqual(
                        text.count(rendered),
                        minimum_count,
                    )

        for name, filesystem_arm in (
            ("E06-resume-delta-experiment.md", "e06-C"),
            ("E07-supersession-experiment.md", "e07-filesystem"),
            ("E08-intention-ledger-experiment.md", "e08-filesystem"),
        ):
            with self.subTest(name=name, filesystem_arm=filesystem_arm):
                self.assertNotIn(
                    f"--expected-arm-retrieval-modes {filesystem_arm}=",
                    document(name),
                )

    def test_nonsemantic_run_examples_bind_exact_lexical_modes(self) -> None:
        for name in (
            "E01-paired-draw-machinery-and-baseline.md",
            "E02-verbatim-identifier-gate.md",
            "E11-wiki-link-leads-experiment.md",
        ):
            with self.subTest(name=name):
                service_commands = [
                    line
                    for line in document(name).splitlines()
                    if (
                        " run " in line
                        and (
                            "--condition service_api" in line
                            or "--condition service_api_resume" in line
                        )
                    )
                ]
                self.assertTrue(service_commands)
                for command in service_commands:
                    self.assertIn(
                        "--service-retrieval-modes exact lexical",
                        command,
                    )
                    self.assertIn(
                        '--api-container "$API_CONTAINER"',
                        command,
                    )


if __name__ == "__main__":
    unittest.main()

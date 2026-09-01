from __future__ import annotations

import hashlib
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch


from eval.audit_resume_payloads import (
    COMMON_FEATURES,
    CONTROL_ARM,
    EXPECTED_ARMS,
    EXPECTED_CASE_IDS,
    EXPECTED_DRAWS,
    EXPECTED_MODES,
    TREATMENT_ARM,
    _resume_observation,
    audit_resume_payloads,
    load_e06_runs,
    main,
)


ROOT = Path(__file__).resolve().parents[1]
REVISION = "a" * 40
IMAGE_ID = "sha256:" + "b" * 64
HARNESS_SHA256 = "c" * 64
SCHEMA_SHA256 = "d" * 64
MUTATION_SCRIPT_SHA256 = "e" * 64


def operation(result_chars: int) -> dict:
    return {
        "operation": "resume",
        "http_status": 200,
        "elapsed_ms": 1.0,
        "result_chars": result_chars,
        "source_text_chars": result_chars,
        "metadata_chars": 0,
    }


def record(case_id: str, result_chars: int, run_id: str) -> dict:
    return {
        "case_id": case_id,
        "condition": "service_api_resume",
        "run_id": run_id,
        "error": None,
        "service_accounting_valid": True,
        "service_operations": [
            operation(result_chars),
            {
                **operation(9_999),
                "operation": "checkpoint",
            },
        ],
        # Deliberately unrelated to the operation-level measurement.
        "service_result_chars": 99_999,
        "model_visible_tool_output_chars": 199_999,
    }


def accepted_evidence(
    *,
    control_chars: int = 100,
    treatment_chars: int = 90,
) -> dict:
    observations = []
    for draw in EXPECTED_DRAWS:
        for case_id in EXPECTED_CASE_IDS:
            observations.extend([
                {
                    "draw": draw,
                    "case_id": case_id,
                    "arm": CONTROL_ARM,
                    "run_id": f"{CONTROL_ARM}-{draw}",
                    "http_status": 200,
                    "result_chars": control_chars,
                },
                {
                    "draw": draw,
                    "case_id": case_id,
                    "arm": TREATMENT_ARM,
                    "run_id": f"{TREATMENT_ARM}-{draw}",
                    "http_status": 200,
                    "result_chars": treatment_chars,
                },
            ])
    return {
        "manifest": {
            "path": str(ROOT / "eval/transition_cases.json"),
            "sha256": "f" * 64,
            "benchmark_version": "0.3",
            "model": "gpt-5.6-sol",
            "case_ids": list(EXPECTED_CASE_IDS),
        },
        "inputs": [],
        "provenance": {},
        "observations": observations,
    }


def service_summary() -> dict:
    return {
        "schema": "brunn-aggregate-service-provenance-summary@v1",
        "status": "complete",
        "expected_arm_retrieval_modes": {
            arm: list(EXPECTED_MODES)
            for arm in EXPECTED_ARMS
        },
        "service_arms": list(EXPECTED_ARMS),
        "source_revision": REVISION,
        "build_revision": REVISION,
        "api_image_id": IMAGE_ID,
        "api_image_revision": REVISION,
    }


def mutation_summary() -> dict:
    return {
        "schema": "brunn-aggregate-mutation-provenance@v1",
        "status": "complete",
        "mutation_script_sha256": MUTATION_SCRIPT_SHA256,
        "draw_bindings": [
            {
                "suite": "0.3",
                "paired_draw_id": draw,
                "mutation_seed": draw,
                "mutation_plans_sha256": str(index + 1) * 64,
                "experiment_arms": list(EXPECTED_ARMS),
            }
            for index, draw in enumerate(EXPECTED_DRAWS)
        ],
    }


class ResumePayloadAuditTests(unittest.TestCase):
    def test_pairs_only_resume_operation_chars_with_zero_tolerance(self):
        result = audit_resume_payloads(accepted_evidence())

        self.assertTrue(result["pass"])
        self.assertEqual(result["totals"], {
            "control_result_chars": 1_500,
            "treatment_result_chars": 1_350,
            "treatment_minus_control": -150,
        })
        self.assertEqual(result["means"]["treatment_minus_control"], -10.0)
        self.assertEqual(len(result["pairs"]), 15)
        self.assertEqual(result["violations"], [])
        self.assertFalse(
            result["contract"]["whole_record_character_metrics_used"]
        )
        self.assertEqual(result["contract"]["tolerance_chars"], 0)

    def test_any_positive_pair_delta_fails_even_when_total_is_lower(self):
        accepted = accepted_evidence(
            control_chars=100,
            treatment_chars=50,
        )
        target = next(
            item
            for item in accepted["observations"]
            if (
                item["arm"] == TREATMENT_ARM
                and item["draw"] == "e06-draw2"
                and item["case_id"] == EXPECTED_CASE_IDS[2]
            )
        )
        target["result_chars"] = 101

        result = audit_resume_payloads(accepted)

        self.assertFalse(result["pass"])
        self.assertEqual(result["violations"], [{
            "draw": "e06-draw2",
            "case_id": EXPECTED_CASE_IDS[2],
            "treatment_minus_control": 1,
        }])
        self.assertLess(
            result["totals"]["treatment_result_chars"],
            result["totals"]["control_result_chars"],
        )

    def test_resume_operation_must_be_unique_successful_and_uncontaminated(self):
        base = record(EXPECTED_CASE_IDS[0], 100, "run-a")
        observed = _resume_observation(
            base,
            path=Path("/tmp/e06.json"),
            arm=CONTROL_ARM,
            draw=EXPECTED_DRAWS[0],
            run_id="run-a",
        )
        self.assertEqual(observed["result_chars"], 100)

        duplicate = json.loads(json.dumps(base))
        duplicate["service_operations"].append(operation(50))
        with self.assertRaisesRegex(ValueError, "exactly one"):
            _resume_observation(
                duplicate,
                path=Path("/tmp/e06.json"),
                arm=CONTROL_ARM,
                draw=EXPECTED_DRAWS[0],
                run_id="run-a",
            )

        failed_then_recovered = json.loads(json.dumps(base))
        failed_then_recovered["service_operations"].insert(0, {
            **operation(10),
            "operation": "failed:resume",
            "http_status": 0,
            "http_calls": 0,
            "service_status": "cli_error",
        })
        with self.assertRaisesRegex(ValueError, "uncontaminated"):
            _resume_observation(
                failed_then_recovered,
                path=Path("/tmp/e06.json"),
                arm=CONTROL_ARM,
                draw=EXPECTED_DRAWS[0],
                run_id="run-a",
            )

        unsuccessful = json.loads(json.dumps(base))
        unsuccessful["service_operations"][0]["http_status"] = 300
        with self.assertRaisesRegex(ValueError, "not successful"):
            _resume_observation(
                unsuccessful,
                path=Path("/tmp/e06.json"),
                arm=CONTROL_ARM,
                draw=EXPECTED_DRAWS[0],
                run_id="run-a",
            )

        with self.assertRaisesRegex(ValueError, "successful service resume"):
            _resume_observation(
                base,
                path=Path("/tmp/e06.json"),
                arm=CONTROL_ARM,
                draw=EXPECTED_DRAWS[0],
                run_id="different-run",
            )

    def _write_inputs(self, root: Path) -> tuple[list[Path], dict[str, dict]]:
        paths = []
        runs = {}
        for arm in EXPECTED_ARMS:
            for draw in EXPECTED_DRAWS:
                run_id = f"{arm}-{draw}"
                resume_deltas = arm == TREATMENT_ARM
                runtime_features = {
                    **COMMON_FEATURES,
                    "resume_deltas": resume_deltas,
                    "lexical_single_scan": False,
                }
                payload = {
                    "run_id": run_id,
                    "feature_flags": {
                        "resume_deltas": resume_deltas,
                    },
                    "embeddings": {
                        "kind": "hashing",
                        "model": "hashing",
                    },
                    "records": [
                        record(case_id, 90 if resume_deltas else 100, run_id)
                        for case_id in EXPECTED_CASE_IDS
                    ],
                    "_runtime_features": runtime_features,
                }
                path = root / f"{run_id}.json"
                path.write_text(
                    json.dumps(payload) + "\n",
                    encoding="utf-8",
                )
                paths.append(path)
                runs[str(path.resolve())] = payload
        return paths, runs

    def _fake_load_draw(
        self,
        runs: dict[str, dict],
        manifest_sha256: str,
    ):
        def fake(path: Path, *, definitive: bool):
            self.assertTrue(definitive)
            run = runs[str(path.resolve())]
            # The run ID form is e06-A-e06-draw1.
            arm = run["run_id"][:5]
            draw = run["run_id"][6:]
            runtime_features = run["_runtime_features"]
            expected_features = {
                **COMMON_FEATURES,
                "resume_deltas": arm == TREATMENT_ARM,
            }
            artifact = {
                "path": str(path.resolve()),
                "sha256": hashlib.sha256(
                    path.read_bytes()
                ).hexdigest(),
                "run_id": run["run_id"],
                "experiment_arm": arm,
                "paired_draw_id": draw,
                "identity_mode": "explicit_arm",
                "suite": "0.3",
                "conditions": ["service_api_resume"],
                "arms": [arm],
                "cases": len(EXPECTED_CASE_IDS),
                "case_ids": list(EXPECTED_CASE_IDS),
                "model": "gpt-5.6-sol",
                "source_revision": REVISION,
                "manifest_sha256": manifest_sha256,
                "expected_runtime_features": expected_features,
                "experiment_parameters": {
                    "mutation_script_sha256": MUTATION_SCRIPT_SHA256,
                    "mutation_seed": draw,
                    "mutation_plans_sha256": "1" * 64,
                },
                "mutation": {},
                "service_runtime_snapshot": {
                    "runtime_features": runtime_features,
                    "embeddings": {"provider": "hashing"},
                },
                "service_retrieval_modes": list(EXPECTED_MODES),
                "service_arms": [arm],
                "service_provenance": {},
                "run_ledger": {
                    "artifacts": {
                        "manifest_sha256": manifest_sha256,
                        "schema_sha256": SCHEMA_SHA256,
                        "harness_sha256": HARNESS_SHA256,
                        "mutation_sha256": "9" * 64,
                    },
                },
            }
            normalized = [
                {
                    "draw": draw,
                    "case": case_id,
                    "arm": arm,
                    "source_condition": "service_api_resume",
                }
                for case_id in EXPECTED_CASE_IDS
            ]
            return artifact, normalized

        return fake

    def test_loader_requires_exact_grid_and_runs_shared_provenance_checks(self):
        manifest_path = ROOT / "eval/transition_cases.json"
        manifest_sha256 = hashlib.sha256(
            manifest_path.read_bytes()
        ).hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            paths, runs = self._write_inputs(Path(temporary))
            with (
                patch(
                    "eval.audit_resume_payloads.load_draw",
                    side_effect=self._fake_load_draw(runs, manifest_sha256),
                ),
                patch(
                    "eval.audit_resume_payloads."
                    "validate_service_provenance_pairing",
                    return_value=service_summary(),
                ) as service_validator,
                patch(
                    "eval.audit_resume_payloads."
                    "validate_mutation_provenance_pairing",
                    return_value=mutation_summary(),
                ) as mutation_validator,
            ):
                accepted = load_e06_runs(
                    paths,
                    manifest_path=manifest_path,
                )

            self.assertEqual(len(accepted["observations"]), 30)
            self.assertTrue(audit_resume_payloads(accepted)["pass"])
            service_validator.assert_called_once()
            mutation_validator.assert_called_once()

    def test_loader_rejects_runtime_drift_and_provenance_failure(self):
        manifest_path = ROOT / "eval/transition_cases.json"
        manifest_sha256 = hashlib.sha256(
            manifest_path.read_bytes()
        ).hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            paths, runs = self._write_inputs(Path(temporary))
            drift_path = next(
                path
                for path in paths
                if path.name.startswith(f"{TREATMENT_ARM}-")
            )
            runs[str(drift_path.resolve())]["_runtime_features"][
                "lexical_single_scan"
            ] = True
            with (
                patch(
                    "eval.audit_resume_payloads.load_draw",
                    side_effect=self._fake_load_draw(runs, manifest_sha256),
                ),
                patch(
                    "eval.audit_resume_payloads."
                    "validate_service_provenance_pairing",
                    return_value=service_summary(),
                ),
                patch(
                    "eval.audit_resume_payloads."
                    "validate_mutation_provenance_pairing",
                    return_value=mutation_summary(),
                ),
            ):
                with self.assertRaisesRegex(
                    ValueError,
                    "runtime feature projection",
                ):
                    load_e06_runs(paths, manifest_path=manifest_path)

            runs[str(drift_path.resolve())]["_runtime_features"][
                "lexical_single_scan"
            ] = False
            with (
                patch(
                    "eval.audit_resume_payloads.load_draw",
                    side_effect=self._fake_load_draw(runs, manifest_sha256),
                ),
                patch(
                    "eval.audit_resume_payloads."
                    "validate_service_provenance_pairing",
                    side_effect=ValueError("image drift"),
                ),
                patch(
                    "eval.audit_resume_payloads."
                    "validate_mutation_provenance_pairing",
                    return_value=mutation_summary(),
                ),
            ):
                with self.assertRaisesRegex(ValueError, "image drift"):
                    load_e06_runs(paths, manifest_path=manifest_path)

    def test_cli_emits_machine_readable_failure_artifact(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = []
            for index in range(6):
                path = root / f"malformed-{index}.json"
                path.write_text("{}\n", encoding="utf-8")
                inputs.append(str(path))
            output = root / "failure.json"
            with redirect_stdout(io.StringIO()):
                exit_code = main([
                    *inputs,
                    "--manifest",
                    str(ROOT / "eval/transition_cases.json"),
                    "--out",
                    str(output),
                ])
            result = json.loads(output.read_text(encoding="utf-8"))

        self.assertEqual(exit_code, 2)
        self.assertFalse(result["pass"])
        self.assertIn(
            "current immutable run_ledger",
            result["errors"][0]["message"],
        )

    def test_e06_spec_freezes_operation_level_zero_tolerance_contract(self):
        text = (
            ROOT / "docs/mvp/E06-resume-delta-experiment.md"
        ).read_text(encoding="utf-8")
        for expected in (
            "eval/audit_resume_payloads.py",
            "e06-resume-payload-comparison.json",
            "service_operations",
            "treatment `result_chars` ≤ control `result_chars`",
            "zero tolerance",
        ):
            self.assertIn(expected, text)


if __name__ == "__main__":
    unittest.main()

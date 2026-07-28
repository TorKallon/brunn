#!/usr/bin/env python3
"""Fail-closed E06 comparison of operation-level resume payload characters."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from agent_work_eval import validate_native_service_accounting  # noqa: E402
from eval.aggregate_draws import (  # noqa: E402
    load_draw,
    validate_mutation_provenance_pairing,
    validate_service_provenance_pairing,
)


SCHEMA = "straylight-e06-resume-payload-comparison@v1"
CONTROL_ARM = "e06-A"
TREATMENT_ARM = "e06-B"
EXPECTED_ARMS = (CONTROL_ARM, TREATMENT_ARM)
EXPECTED_DRAWS = ("e06-draw1", "e06-draw2", "e06-draw3")
EXPECTED_CASE_IDS = (
    "warmind-alpha-budget-transition",
    "charlemagne-storage-observation-transition",
    "star-rupture-second-facturer-transition",
    "switzerland-reservation-transition",
    "straylight-api-gate-transition",
)
EXPECTED_MODES = ("exact", "lexical")
EXPECTED_CONDITION = "service_api_resume"
COMMON_FEATURES = {
    "semantic_lane": False,
    "verbatim_spans": False,
    "supersession_demotion": False,
    "intention_ledger": False,
}


def _require_mapping(value: Any, message: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError(message)
    return value


def _read_json(path: Path, *, label: str) -> tuple[dict[str, Any], str]:
    try:
        body = path.read_bytes()
        value = json.loads(body)
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: could not load {label}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path}: {label} must be a JSON object")
    return value, hashlib.sha256(body).hexdigest()


def _load_manifest(path: Path) -> dict[str, Any]:
    manifest, sha256 = _read_json(path, label="E06 manifest")
    cases = manifest.get("cases")
    case_ids = (
        [
            case.get("id")
            for case in cases
            if isinstance(case, Mapping)
        ]
        if isinstance(cases, list)
        else []
    )
    if (
        manifest.get("benchmark_version") != "0.3"
        or manifest.get("model") != "gpt-5.6-sol"
        or not isinstance(cases, list)
        or tuple(case_ids) != EXPECTED_CASE_IDS
    ):
        raise ValueError(
            f"{path}: manifest is not the exact five-case E06 transition set"
        )
    return {
        "path": str(path.resolve()),
        "sha256": sha256,
        "benchmark_version": manifest["benchmark_version"],
        "model": manifest["model"],
        "case_ids": list(EXPECTED_CASE_IDS),
    }


def _single_value(
    items: Sequence[dict[str, Any]],
    *,
    label: str,
    getter: Callable[[dict[str, Any]], Any],
) -> Any:
    values = [getter(item) for item in items]
    first = values[0]
    if any(value != first for value in values[1:]):
        raise ValueError(f"E06 A/B artifacts do not share one {label}")
    return first


def _resume_observation(
    record: Mapping[str, Any],
    *,
    path: Path,
    arm: str,
    draw: str,
    run_id: str,
) -> dict[str, Any]:
    case_id = record.get("case_id")
    location = f"{path}: {arm}/{draw}/{case_id}"
    if (
        case_id not in EXPECTED_CASE_IDS
        or record.get("condition") != EXPECTED_CONDITION
        or record.get("run_id") != run_id
        or record.get("error")
        or record.get("service_accounting_valid") is not True
    ):
        raise ValueError(f"{location} is not a successful service resume record")
    try:
        operations = validate_native_service_accounting({
            "operations": record.get("service_operations"),
        })
    except ValueError as error:
        raise ValueError(f"{location} has malformed service operations: {error}") from error
    resume_operations = [
        operation
        for operation in operations
        if operation.get("operation") == "resume"
    ]
    contaminated_resume_operations = [
        operation
        for operation in operations
        if operation.get("operation") in {"failed:resume", "denied:resume"}
    ]
    if len(resume_operations) != 1 or contaminated_resume_operations:
        raise ValueError(
            f"{location} must contain exactly one uncontaminated resume operation"
        )
    resume = resume_operations[0]
    http_status = resume.get("http_status")
    result_chars = resume.get("result_chars")
    if (
        type(http_status) is not int
        or not 200 <= http_status <= 299
        or type(result_chars) is not int
        or result_chars < 0
    ):
        raise ValueError(
            f"{location} resume operation is not successful or lacks result_chars"
        )
    return {
        "draw": draw,
        "case_id": case_id,
        "arm": arm,
        "run_id": run_id,
        "http_status": http_status,
        "result_chars": result_chars,
    }


def load_e06_runs(
    paths: Sequence[Path],
    *,
    manifest_path: Path,
) -> dict[str, Any]:
    if len(paths) != 6:
        raise ValueError(
            "E06 resume-payload audit requires exactly six A/B artifacts"
        )
    resolved_paths = [path.resolve() for path in paths]
    if len(resolved_paths) != len(set(resolved_paths)):
        raise ValueError("E06 resume-payload audit received duplicate input paths")

    manifest = _load_manifest(manifest_path)
    artifacts: list[dict[str, Any]] = []
    inputs: list[dict[str, Any]] = []
    observations: list[dict[str, Any]] = []
    actual_feature_projections: list[dict[str, Any]] = []
    expected_feature_projections: list[dict[str, Any]] = []
    run_ids: set[str] = set()
    artifact_hashes: set[str] = set()

    for path in paths:
        run, input_sha256 = _read_json(path, label="E06 result artifact")
        artifact, normalized = load_draw(path, definitive=True)
        arm = artifact.get("experiment_arm")
        draw = artifact.get("paired_draw_id")
        if (
            artifact.get("sha256") != input_sha256
            or artifact.get("identity_mode") != "explicit_arm"
            or arm not in EXPECTED_ARMS
            or draw not in EXPECTED_DRAWS
            or artifact.get("suite") != manifest["benchmark_version"]
            or artifact.get("conditions") != [EXPECTED_CONDITION]
            or artifact.get("arms") != [arm]
            or artifact.get("cases") != len(EXPECTED_CASE_IDS)
            or tuple(artifact.get("case_ids", ())) != EXPECTED_CASE_IDS
            or artifact.get("model") != manifest["model"]
            or artifact.get("manifest_sha256") != manifest["sha256"]
            or artifact.get("service_retrieval_modes") != list(EXPECTED_MODES)
            or artifact.get("service_arms") != [arm]
        ):
            raise ValueError(
                f"{path}: artifact is not an exact definitive E06 A/B draw"
            )
        normalized_grid = {
            (
                row.get("draw"),
                row.get("case"),
                row.get("arm"),
                row.get("source_condition"),
            )
            for row in normalized
            if isinstance(row, Mapping)
        }
        expected_normalized_grid = {
            (draw, case_id, arm, EXPECTED_CONDITION)
            for case_id in EXPECTED_CASE_IDS
        }
        if (
            len(normalized) != len(EXPECTED_CASE_IDS)
            or normalized_grid != expected_normalized_grid
        ):
            raise ValueError(
                f"{path}: normalized E06 draw does not cover the exact case grid"
            )

        run_id = artifact.get("run_id")
        if (
            not isinstance(run_id, str)
            or not run_id
            or run_id in run_ids
            or input_sha256 in artifact_hashes
        ):
            raise ValueError(
                f"{path}: duplicate or malformed E06 run/artifact identity"
            )
        run_ids.add(run_id)
        artifact_hashes.add(input_sha256)

        expected_resume_deltas = arm == TREATMENT_ARM
        snapshot = _require_mapping(
            artifact.get("service_runtime_snapshot"),
            f"{path}: missing runtime snapshot",
        )
        actual_features = _require_mapping(
            snapshot.get("runtime_features"),
            f"{path}: missing runtime feature snapshot",
        )
        expected_features = _require_mapping(
            artifact.get("expected_runtime_features"),
            f"{path}: missing runtime feature expectations",
        )
        required_features = {
            **COMMON_FEATURES,
            "resume_deltas": expected_resume_deltas,
        }
        feature_mismatches = {
            feature: {
                "required": required,
                "expected": expected_features.get(feature),
                "actual": actual_features.get(feature),
            }
            for feature, required in required_features.items()
            if (
                expected_features.get(feature) is not required
                or actual_features.get(feature) is not required
            )
        }
        feature_flags = _require_mapping(
            run.get("feature_flags"),
            f"{path}: missing top-level feature flags",
        )
        runtime_embeddings = _require_mapping(
            snapshot.get("embeddings"),
            f"{path}: missing runtime embedding posture",
        )
        run_embeddings = _require_mapping(
            run.get("embeddings"),
            f"{path}: missing run embedding posture",
        )
        if (
            feature_mismatches
            or feature_flags.get("resume_deltas") is not expected_resume_deltas
            or runtime_embeddings.get("provider") != "hashing"
            or run_embeddings.get("kind") != "hashing"
        ):
            raise ValueError(
                f"{path}: E06 runtime, flag, or hashing posture drifted: "
                f"{feature_mismatches}"
            )

        records = run.get("records")
        if not isinstance(records, list) or len(records) != len(EXPECTED_CASE_IDS):
            raise ValueError(f"{path}: E06 draw must contain exactly five records")
        raw_case_ids = [
            record.get("case_id")
            for record in records
            if isinstance(record, Mapping)
        ]
        if (
            len(raw_case_ids) != len(records)
            or set(raw_case_ids) != set(EXPECTED_CASE_IDS)
            or len(raw_case_ids) != len(set(raw_case_ids))
        ):
            raise ValueError(f"{path}: E06 records do not cover the exact cases")
        for record in records:
            observations.append(_resume_observation(
                record,
                path=path,
                arm=arm,
                draw=draw,
                run_id=run_id,
            ))

        ledger = _require_mapping(
            artifact.get("run_ledger"),
            f"{path}: missing immutable run ledger",
        )
        ledger_artifacts = _require_mapping(
            ledger.get("artifacts"),
            f"{path}: missing run-ledger artifact bindings",
        )
        inputs.append({
            "path": str(path.resolve()),
            "sha256": input_sha256,
            "run_id": run_id,
            "experiment_arm": arm,
            "paired_draw_id": draw,
            "manifest_sha256": artifact["manifest_sha256"],
            "schema_sha256": ledger_artifacts.get("schema_sha256"),
            "harness_sha256": ledger_artifacts.get("harness_sha256"),
            "mutation_sha256": ledger_artifacts.get("mutation_sha256"),
        })
        actual_feature_projections.append({
            key: value
            for key, value in actual_features.items()
            if key != "resume_deltas"
        })
        expected_feature_projections.append({
            key: value
            for key, value in expected_features.items()
            if key != "resume_deltas"
        })
        artifacts.append(artifact)

    expected_artifact_grid = {
        (arm, draw)
        for arm in EXPECTED_ARMS
        for draw in EXPECTED_DRAWS
    }
    actual_artifact_grid = {
        (artifact["experiment_arm"], artifact["paired_draw_id"])
        for artifact in artifacts
    }
    if actual_artifact_grid != expected_artifact_grid:
        raise ValueError(
            "E06 resume-payload artifacts do not cover the exact 2x3 arm/draw grid"
        )

    service_provenance = validate_service_provenance_pairing(
        artifacts,
        {
            arm: EXPECTED_MODES
            for arm in EXPECTED_ARMS
        },
        definitive=True,
    )
    if (
        service_provenance.get("status") != "complete"
        or service_provenance.get("service_arms") != list(EXPECTED_ARMS)
    ):
        raise ValueError("E06 service provenance is incomplete")

    mutation_provenance = validate_mutation_provenance_pairing(artifacts)
    mutation_draws = mutation_provenance.get("draw_bindings")
    if (
        mutation_provenance.get("status") != "complete"
        or not isinstance(mutation_draws, list)
        or len(mutation_draws) != len(EXPECTED_DRAWS)
        or not all(
            isinstance(binding, Mapping)
            for binding in mutation_draws
        )
        or {
            (
                binding.get("suite"),
                binding.get("paired_draw_id"),
                tuple(binding.get("experiment_arms", ())),
            )
            for binding in mutation_draws
            if isinstance(binding, Mapping)
        }
        != {
            (manifest["benchmark_version"], draw, EXPECTED_ARMS)
            for draw in EXPECTED_DRAWS
        }
    ):
        raise ValueError("E06 mutation provenance does not cover exact A/B draws")

    schema_sha256 = _single_value(
        artifacts,
        label="schema SHA-256",
        getter=lambda artifact: artifact["run_ledger"]["artifacts"][
            "schema_sha256"
        ],
    )
    harness_sha256 = _single_value(
        artifacts,
        label="harness SHA-256",
        getter=lambda artifact: artifact["run_ledger"]["artifacts"][
            "harness_sha256"
        ],
    )
    actual_feature_projection = _single_value(
        actual_feature_projections,
        label="non-treatment runtime feature projection",
        getter=lambda projection: projection,
    )
    expected_feature_projection = _single_value(
        expected_feature_projections,
        label="non-treatment expected feature projection",
        getter=lambda projection: projection,
    )
    if len(observations) != 30:
        raise ValueError("E06 resume-payload audit requires exactly 30 observations")

    return {
        "manifest": manifest,
        "inputs": sorted(
            inputs,
            key=lambda item: (
                EXPECTED_ARMS.index(item["experiment_arm"]),
                EXPECTED_DRAWS.index(item["paired_draw_id"]),
            ),
        ),
        "provenance": {
            "source_revision": service_provenance["source_revision"],
            "build_revision": service_provenance["build_revision"],
            "api_image_id": service_provenance["api_image_id"],
            "api_image_revision": service_provenance["api_image_revision"],
            "schema_sha256": schema_sha256,
            "harness_sha256": harness_sha256,
            "service_retrieval_modes": list(EXPECTED_MODES),
            "runtime_features_except_resume_deltas": actual_feature_projection,
            "expected_features_except_resume_deltas": (
                expected_feature_projection
            ),
            "mutation": mutation_provenance,
        },
        "observations": observations,
    }


def audit_resume_payloads(accepted: Mapping[str, Any]) -> dict[str, Any]:
    observations = accepted.get("observations")
    if not isinstance(observations, list) or len(observations) != 30:
        raise ValueError("accepted E06 evidence must contain 30 observations")
    by_key: dict[tuple[str, str, str], Mapping[str, Any]] = {}
    for observation in observations:
        if not isinstance(observation, Mapping):
            raise ValueError("accepted E06 observation is malformed")
        key = (
            str(observation.get("draw")),
            str(observation.get("case_id")),
            str(observation.get("arm")),
        )
        if key in by_key:
            raise ValueError(f"duplicate E06 resume observation for {key}")
        by_key[key] = observation

    expected_keys = {
        (draw, case_id, arm)
        for draw in EXPECTED_DRAWS
        for case_id in EXPECTED_CASE_IDS
        for arm in EXPECTED_ARMS
    }
    if set(by_key) != expected_keys:
        raise ValueError("accepted E06 observations do not form the exact grid")

    pairs = []
    for draw in EXPECTED_DRAWS:
        for case_id in EXPECTED_CASE_IDS:
            control = by_key[(draw, case_id, CONTROL_ARM)]
            treatment = by_key[(draw, case_id, TREATMENT_ARM)]
            control_chars = control.get("result_chars")
            treatment_chars = treatment.get("result_chars")
            if (
                type(control_chars) is not int
                or control_chars < 0
                or type(treatment_chars) is not int
                or treatment_chars < 0
            ):
                raise ValueError(
                    f"{draw}/{case_id}: result_chars pair is malformed"
                )
            delta = treatment_chars - control_chars
            pairs.append({
                "draw": draw,
                "case_id": case_id,
                "control_result_chars": control_chars,
                "treatment_result_chars": treatment_chars,
                "treatment_minus_control": delta,
                "pass": delta <= 0,
            })

    control_total = sum(pair["control_result_chars"] for pair in pairs)
    treatment_total = sum(pair["treatment_result_chars"] for pair in pairs)
    pair_count = len(pairs)
    violations = [
        {
            "draw": pair["draw"],
            "case_id": pair["case_id"],
            "treatment_minus_control": pair["treatment_minus_control"],
        }
        for pair in pairs
        if not pair["pass"]
    ]
    return {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(),
        "pass": not violations,
        "errors": [],
        "contract": {
            "control_arm": CONTROL_ARM,
            "treatment_arm": TREATMENT_ARM,
            "draws": list(EXPECTED_DRAWS),
            "case_ids": list(EXPECTED_CASE_IDS),
            "pairs": pair_count,
            "measurement_source": (
                "records[].service_operations[operation=resume].result_chars"
            ),
            "successful_resume_operations_per_record": 1,
            "threshold": (
                "treatment_result_chars <= control_result_chars in every pair"
            ),
            "tolerance_chars": 0,
            "whole_record_character_metrics_used": False,
        },
        "manifest": accepted["manifest"],
        "inputs": accepted["inputs"],
        "provenance": accepted["provenance"],
        "totals": {
            "control_result_chars": control_total,
            "treatment_result_chars": treatment_total,
            "treatment_minus_control": treatment_total - control_total,
        },
        "means": {
            "control_result_chars": round(control_total / pair_count, 6),
            "treatment_result_chars": round(treatment_total / pair_count, 6),
            "treatment_minus_control": round(
                (treatment_total - control_total) / pair_count,
                6,
            ),
        },
        "violations": violations,
        "pairs": pairs,
    }


def _input_metadata(paths: Sequence[Path]) -> list[dict[str, Any]]:
    metadata = []
    for path in paths:
        row: dict[str, Any] = {
            "path": str(path.resolve()),
            "sha256": None,
        }
        try:
            row["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        except OSError as error:
            row["read_error"] = {
                "type": type(error).__name__,
                "message": str(error),
            }
        metadata.append(row)
    return metadata


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Audit the exact six definitive E06 A/B artifacts for operation-level "
            "resume payload budget neutrality"
        )
    )
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        protected_paths = {
            path.resolve()
            for path in args.inputs
        } | {args.manifest.resolve()}
        if args.out.resolve() in protected_paths:
            raise ValueError("--out must not overwrite an input or manifest")
        accepted = load_e06_runs(
            args.inputs,
            manifest_path=args.manifest,
        )
        result = audit_resume_payloads(accepted)
        exit_code = 0 if result["pass"] else 2
    except (OSError, TypeError, json.JSONDecodeError, ValueError) as error:
        result = {
            "schema": SCHEMA,
            "created_at": datetime.now().astimezone().isoformat(),
            "pass": False,
            "errors": [{
                "type": type(error).__name__,
                "message": str(error),
            }],
            "inputs": _input_metadata(args.inputs),
        }
        exit_code = 2
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(result, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(result, indent=2))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

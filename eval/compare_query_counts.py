#!/usr/bin/env python3
"""Fail-closed paired comparison of performance-eval query counts."""

from __future__ import annotations

import argparse
import hashlib
import json
from datetime import datetime
from pathlib import Path
from typing import Any, Mapping, Sequence


SCHEMA = "brunn-paired-query-count-comparison@v1"
PERFORMANCE_SCHEMA = "brunn-performance-eval@v2"
PRODUCTION_RECORDS = 64_000
QUERY_COUNT_SAMPLE_OPERATIONS = {
    "open": "open",
    "search": "search",
    "broad_search": "search",
    "bounded_overflow_search": "search",
    "old_source_search": "search",
    "verbatim_identifier_search": "search",
    "concurrent_search": "search",
    "read": "read",
    "max_batch_read": "read",
    "write": "write",
    "checkpoint": "checkpoint",
    "max_checkpoint_sources": "checkpoint",
    "resume": "resume",
}


def _require_mapping(value: Any, message: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError(message)
    return value


def _runtime_projection(
    snapshot: Mapping[str, Any],
    *,
    feature: str,
) -> dict[str, Any]:
    features = _require_mapping(
        snapshot.get("runtime_features"),
        "runtime snapshot lacks runtime_features",
    )
    return {
        "status": snapshot.get("status"),
        "build_revision": snapshot.get("build_revision"),
        "read_only": snapshot.get("read_only"),
        "embeddings": snapshot.get("embeddings"),
        "runtime_features": {
            name: value
            for name, value in features.items()
            if name != feature
        },
    }


def _normalize_expected_retrieval_modes(
    modes: Sequence[str],
) -> list[str]:
    if (
        isinstance(modes, (str, bytes))
        or not modes
        or any(
            not isinstance(mode, str)
            or mode not in {"exact", "lexical", "semantic"}
            for mode in modes
        )
        or len(modes) != len(set(modes))
    ):
        raise ValueError("expected retrieval modes are invalid")
    return list(modes)


def _validate_artifact(
    artifact: Mapping[str, Any],
    *,
    label: str,
    feature: str,
    feature_state: bool,
    expected_retrieval_modes: Sequence[str],
) -> dict[str, Any]:
    if artifact.get("schema") != PERFORMANCE_SCHEMA:
        raise ValueError(f"{label} is not a performance-eval v2 artifact")
    if artifact.get("protocol") != "simple":
        raise ValueError(f"{label} is not a simple-protocol artifact")
    if artifact.get("pass") is not True or artifact.get("errors") != []:
        raise ValueError(f"{label} is not a passing artifact")
    profile = _require_mapping(
        artifact.get("run_profile"),
        f"{label} lacks a run profile",
    )
    if (
        profile.get("definitive") is not True
        or profile.get("mode") != "definitive"
    ):
        raise ValueError(f"{label} is not definitive")

    fingerprint = _require_mapping(
        artifact.get("implementation_fingerprint"),
        f"{label} lacks an implementation fingerprint",
    )
    source_revision = fingerprint.get("source_revision")
    image_id = fingerprint.get("api_image_id")
    if (
        fingerprint.get("reproducible") is not True
        or fingerprint.get("tracked_source_clean") is not True
        or fingerprint.get("untracked_source_files") != []
        or not isinstance(source_revision, str)
        or not source_revision
        or not isinstance(image_id, str)
        or not image_id
        or fingerprint.get("api_image_revision") != source_revision
        or artifact.get("expected_build_revision") != source_revision
    ):
        raise ValueError(f"{label} lacks clean source/build/image provenance")

    runtime = _require_mapping(
        artifact.get("runtime_configuration"),
        f"{label} lacks runtime configuration",
    )
    before = _require_mapping(
        runtime.get("before"),
        f"{label} lacks its before runtime snapshot",
    )
    after = _require_mapping(
        runtime.get("after"),
        f"{label} lacks its after runtime snapshot",
    )
    if runtime.get("stable") is not True:
        raise ValueError(f"{label} runtime configuration is not stable")
    if _runtime_projection(before, feature=feature) != _runtime_projection(
        after,
        feature=feature,
    ):
        raise ValueError(f"{label} runtime configuration drifted")
    before_features = _require_mapping(
        before.get("runtime_features"),
        f"{label} lacks runtime features",
    )
    after_features = _require_mapping(
        after.get("runtime_features"),
        f"{label} lacks final runtime features",
    )
    if before_features != after_features:
        raise ValueError(f"{label} runtime features drifted")
    if (
        before.get("build_revision") != source_revision
        or after.get("build_revision") != source_revision
    ):
        raise ValueError(f"{label} runtime build does not match source")
    if before_features.get(feature) is not feature_state:
        raise ValueError(
            f"{label} runtime does not declare {feature}={feature_state}"
        )

    expected = _require_mapping(
        artifact.get("expected_runtime_features"),
        f"{label} lacks expected runtime features",
    )
    if expected.get(feature) is not feature_state:
        raise ValueError(
            f"{label} does not explicitly expect {feature}={feature_state}"
        )
    mismatches = {
        name: {"expected": value, "actual": before_features.get(name)}
        for name, value in expected.items()
        if (
            name not in before_features
            or type(before_features[name]) is not type(value)
            or before_features[name] != value
        )
    }
    if mismatches:
        raise ValueError(
            f"{label} expected runtime features do not match: {mismatches}"
        )
    verbatim_posture = _require_mapping(
        artifact.get("verbatim_feature_acceptance_posture"),
        f"{label} lacks verbatim feature-acceptance posture",
    )
    verbatim_observed = _require_mapping(
        verbatim_posture.get("observed"),
        f"{label} verbatim feature-acceptance posture lacks observations",
    )
    if (
        verbatim_posture.get("posture") not in {
            "required",
            "not-applicable",
        }
        or verbatim_posture.get("eligible") is not True
        or not isinstance(verbatim_posture.get("reason"), str)
        or not verbatim_posture["reason"]
        or verbatim_observed.get("verbatim_spans")
        is not before_features.get("verbatim_spans")
        or (
            verbatim_posture.get("posture") == "not-applicable"
            and before_features.get("verbatim_spans") is not False
        )
    ):
        raise ValueError(
            f"{label} has invalid verbatim feature-acceptance posture"
        )

    retrieval_modes = artifact.get("retrieval_modes")
    if retrieval_modes != list(expected_retrieval_modes):
        raise ValueError(
            f"{label} retrieval modes do not match the explicit expected modes"
        )

    scales = artifact.get("scales")
    if (
        not isinstance(scales, list)
        or not scales
        or not all(isinstance(scale, Mapping) for scale in scales)
        or any(
            not isinstance(scale.get("scale"), int)
            or isinstance(scale.get("scale"), bool)
            or scale["scale"] <= 0
            for scale in scales
        )
        or len({scale["scale"] for scale in scales}) != len(scales)
    ):
        raise ValueError(f"{label} has no completed scales")
    scale_ids = [scale["scale"] for scale in scales]
    if profile.get("requested_scales") != scale_ids:
        raise ValueError(f"{label} scales do not match its run profile")
    samples_per_retrieval = profile.get("samples_per_retrieval")
    if (
        not isinstance(samples_per_retrieval, int)
        or isinstance(samples_per_retrieval, bool)
        or samples_per_retrieval < 1
    ):
        raise ValueError(f"{label} has invalid retrieval sample cardinality")
    if artifact.get("production_reference_records") != PRODUCTION_RECORDS:
        raise ValueError(f"{label} has an invalid production reference scale")
    return {
        "source_revision": source_revision,
        "api_image_id": image_id,
        "runtime_before": before,
        "expected_runtime_features": expected,
        "retrieval_modes": retrieval_modes,
        "profile": profile,
        "scales": scales,
    }


def _expected_sample_cardinality(
    scale: Mapping[str, Any],
    *,
    profile: Mapping[str, Any],
    label: str,
) -> dict[str, int]:
    scale_id = scale.get("scale")
    samples = profile.get("samples_per_retrieval")
    if scale.get("samples") != samples:
        raise ValueError(
            f"{label} scale samples do not match its run profile"
        )
    fixture = _require_mapping(
        scale.get("fixture_manifest"),
        f"{label} scale lacks a fixture manifest",
    )
    identifiers = fixture.get("verbatim_identifiers")
    if not isinstance(identifiers, list):
        raise ValueError(f"{label} scale lacks verbatim fixture probes")
    concurrent = _require_mapping(
        scale.get("concurrent_probe"),
        f"{label} scale lacks concurrent-probe cardinality",
    )
    rounds = concurrent.get("rounds")
    searches = concurrent.get("searches_per_round")
    if (
        not isinstance(rounds, int)
        or isinstance(rounds, bool)
        or rounds < 1
        or not isinstance(searches, int)
        or isinstance(searches, bool)
        or searches < 1
    ):
        raise ValueError(
            f"{label} scale has invalid concurrent-probe cardinality"
        )
    expected = {
        "open": samples,
        "search": samples,
        "broad_search": samples,
        "bounded_overflow_search": samples,
        "old_source_search": samples,
        "verbatim_identifier_search": len(identifiers),
        "read": samples,
        "checkpoint": 1,
        "resume": samples,
        "write": rounds,
        "concurrent_search": rounds * searches,
    }
    if scale_id >= PRODUCTION_RECORDS:
        expected.update({
            "max_batch_read": 1,
            "max_checkpoint_sources": 1,
        })
    return dict(sorted(expected.items()))


def _sample_counts(
    scale: Mapping[str, Any],
    *,
    profile: Mapping[str, Any],
    label: str,
) -> tuple[dict[str, dict[str, Any]], dict[str, int]]:
    query_counts = _require_mapping(
        scale.get("query_counts"),
        f"{label} scale lacks query counts",
    )
    if (
        query_counts.get("missing_by_operation") != {}
        or query_counts.get("missing_by_sample_name") != {}
    ):
        raise ValueError(f"{label} scale has missing query counts")
    expected = _expected_sample_cardinality(
        scale,
        profile=profile,
        label=label,
    )
    cardinality = _require_mapping(
        query_counts.get("sample_cardinality"),
        f"{label} scale lacks authoritative sample cardinality",
    )
    if (
        cardinality.get("schema")
        != "brunn-query-count-sample-cardinality@v1"
        or cardinality.get("authoritative") is not True
        or cardinality.get("pass") is not True
        or cardinality.get("expected_by_sample_name") != expected
        or cardinality.get("observed_by_sample_name") != expected
        or cardinality.get("counted_by_sample_name") != expected
        or cardinality.get("missing_response_samples") != {}
        or cardinality.get("extra_response_samples") != {}
        or cardinality.get("missing_query_count_samples") != {}
    ):
        raise ValueError(
            f"{label} scale sample cardinality is not authoritative and exact"
        )
    samples = _require_mapping(
        query_counts.get("by_sample_name"),
        f"{label} scale lacks per-sample-name query counts",
    )
    if set(samples) != set(expected):
        raise ValueError(
            f"{label} scale named query-count catalog is not authoritative"
        )
    normalized: dict[str, dict[str, Any]] = {}
    for sample_name, raw in samples.items():
        summary = _require_mapping(
            raw,
            f"{label} sample {sample_name!r} is malformed",
        )
        operation = summary.get("operation")
        counts = summary.get("counts")
        expected_samples = expected[sample_name]
        expected_operation = QUERY_COUNT_SAMPLE_OPERATIONS[sample_name]
        if (
            not isinstance(sample_name, str)
            or not sample_name
            or operation != expected_operation
            or not isinstance(counts, list)
            or any(
                not isinstance(count, int)
                or isinstance(count, bool)
                or count < 0
                for count in counts
            )
            or len(counts) != expected_samples
            or summary.get("samples") != expected_samples
            or summary.get("expected_samples") != expected_samples
            or summary.get("observed_samples") != expected_samples
            or summary.get("missing_query_counts") != 0
            or summary.get("min")
            != (min(counts) if counts else None)
            or summary.get("max")
            != (max(counts) if counts else None)
        ):
            raise ValueError(f"{label} sample {sample_name!r} is malformed")
        normalized[sample_name] = {
            "operation": operation,
            "counts": counts,
        }
    return normalized, expected


def compare_query_count_artifacts(
    control: Mapping[str, Any],
    treatment: Mapping[str, Any],
    *,
    feature: str,
    operation: str,
    min_delta: int,
    max_delta: int,
    expected_retrieval_modes: Sequence[str],
    require_strict_improvement: bool = False,
    require_strict_increase: bool = False,
    allowed_deltas: Sequence[int] | None = None,
) -> dict[str, Any]:
    control = _require_mapping(control, "control artifact must be an object")
    treatment = _require_mapping(
        treatment,
        "treatment artifact must be an object",
    )
    if not feature or not operation:
        raise ValueError("feature and operation must be non-empty")
    if min_delta > max_delta:
        raise ValueError("minimum delta cannot exceed maximum delta")
    if require_strict_improvement and require_strict_increase:
        raise ValueError(
            "strict improvement and strict increase are mutually exclusive"
        )
    normalized_allowed_deltas = (
        sorted(set(allowed_deltas))
        if allowed_deltas is not None
        else None
    )
    if (
        normalized_allowed_deltas is not None
        and (
            not normalized_allowed_deltas
            or any(
                not isinstance(delta, int) or isinstance(delta, bool)
                for delta in normalized_allowed_deltas
            )
            or any(
                delta < min_delta or delta > max_delta
                for delta in normalized_allowed_deltas
            )
        )
    ):
        raise ValueError(
            "allowed deltas must be non-empty integers inside the delta bounds"
        )
    expected_modes = _normalize_expected_retrieval_modes(
        expected_retrieval_modes
    )
    control_meta = _validate_artifact(
        control,
        label="control",
        feature=feature,
        feature_state=False,
        expected_retrieval_modes=expected_modes,
    )
    treatment_meta = _validate_artifact(
        treatment,
        label="treatment",
        feature=feature,
        feature_state=True,
        expected_retrieval_modes=expected_modes,
    )

    for key in ("source_revision", "api_image_id"):
        if control_meta[key] != treatment_meta[key]:
            raise ValueError(f"control/treatment {key} differs")
    if control_meta["profile"] != treatment_meta["profile"]:
        raise ValueError("control/treatment run profiles differ")
    for key in (
        "gate_profile",
        "semantic_failure_probe_posture",
        "verbatim_feature_acceptance_posture",
        "production_reference_records",
        "future_reference_records",
        "query_budget_contract",
    ):
        if control.get(key) != treatment.get(key):
            raise ValueError(f"control/treatment {key} differs")

    control_features = control_meta["runtime_before"]["runtime_features"]
    treatment_features = treatment_meta["runtime_before"]["runtime_features"]
    if set(control_features) != set(treatment_features):
        raise ValueError("control/treatment runtime feature sets differ")
    feature_differences = {
        name
        for name in control_features
        if control_features[name] != treatment_features[name]
    }
    if feature_differences != {feature}:
        raise ValueError(
            "runtime configurations must differ only in the declared feature"
        )
    if _runtime_projection(
        control_meta["runtime_before"],
        feature=feature,
    ) != _runtime_projection(
        treatment_meta["runtime_before"],
        feature=feature,
    ):
        raise ValueError(
            "control/treatment runtime configurations differ beyond the feature"
        )

    control_expected = control_meta["expected_runtime_features"]
    treatment_expected = treatment_meta["expected_runtime_features"]
    if set(control_expected) != set(treatment_expected):
        raise ValueError("control/treatment declared feature sets differ")
    expected_differences = {
        name
        for name in control_expected
        if control_expected[name] != treatment_expected[name]
    }
    if expected_differences != {feature}:
        raise ValueError(
            "declared runtime expectations must differ only in the feature"
        )

    control_scales = control_meta["scales"]
    treatment_scales = treatment_meta["scales"]
    control_scale_ids = [item.get("scale") for item in control_scales]
    treatment_scale_ids = [item.get("scale") for item in treatment_scales]
    if control_scale_ids != treatment_scale_ids:
        raise ValueError("control/treatment scales differ")

    comparisons: list[dict[str, Any]] = []
    selected_deltas: list[int] = []
    sample_catalogs: list[dict[str, Any]] = []
    for control_scale, treatment_scale in zip(
        control_scales,
        treatment_scales,
    ):
        shape_fields = (
            "scale",
            "protocol",
            "documents",
            "target_path",
            "marker",
            "fixture_manifest",
            "samples",
            "discovery_query",
            "discovery_task",
            "discovery_path_was_provided",
        )
        if any(
            control_scale.get(field) != treatment_scale.get(field)
            for field in shape_fields
        ):
            raise ValueError(
                f"scale {control_scale.get('scale')} fixture shape differs"
            )
        scale_id = control_scale.get("scale")
        control_concurrent = _require_mapping(
            control_scale.get("concurrent_probe"),
            f"control scale {scale_id} lacks concurrent-probe shape",
        )
        treatment_concurrent = _require_mapping(
            treatment_scale.get("concurrent_probe"),
            f"treatment scale {scale_id} lacks concurrent-probe shape",
        )
        if any(
            control_concurrent.get(key) != treatment_concurrent.get(key)
            for key in ("rounds", "searches_per_round")
        ):
            raise ValueError(
                f"scale {scale_id} concurrent-probe shape differs"
            )
        control_samples, control_catalog = _sample_counts(
            control_scale,
            profile=control_meta["profile"],
            label=f"control scale {scale_id}",
        )
        treatment_samples, treatment_catalog = _sample_counts(
            treatment_scale,
            profile=treatment_meta["profile"],
            label=f"treatment scale {scale_id}",
        )
        if control_catalog != treatment_catalog:
            raise ValueError(
                f"scale {scale_id} authoritative sample catalogs differ"
            )
        sample_catalogs.append({
            "scale": scale_id,
            "expected_by_sample_name": control_catalog,
        })
        for sample_name in sorted(control_samples):
            control_sample = control_samples[sample_name]
            treatment_sample = treatment_samples[sample_name]
            if control_sample["operation"] != treatment_sample["operation"]:
                raise ValueError(
                    f"scale {scale_id} sample {sample_name} operation differs"
                )
            control_counts = control_sample["counts"]
            treatment_counts = treatment_sample["counts"]
            if len(control_counts) != len(treatment_counts):
                raise ValueError(
                    f"scale {scale_id} sample {sample_name} is not paired"
                )
            deltas = [
                after - before
                for before, after in zip(control_counts, treatment_counts)
            ]
            selected = control_sample["operation"] == operation
            if selected:
                selected_deltas.extend(deltas)
                if any(
                    delta < min_delta or delta > max_delta
                    for delta in deltas
                ):
                    raise ValueError(
                        f"scale {scale_id} sample {sample_name} delta "
                        f"falls outside [{min_delta}, {max_delta}]"
                    )
                if (
                    normalized_allowed_deltas is not None
                    and any(
                        delta not in normalized_allowed_deltas
                        for delta in deltas
                    )
                ):
                    raise ValueError(
                        f"scale {scale_id} sample {sample_name} delta is not "
                        f"in the allowed set {normalized_allowed_deltas}"
                    )
            elif any(delta != 0 for delta in deltas):
                raise ValueError(
                    f"scale {scale_id} untouched sample {sample_name} changed"
                )
            comparisons.append({
                "scale": scale_id,
                "sample_name": sample_name,
                "operation": control_sample["operation"],
                "selected": selected,
                "control_counts": control_counts,
                "treatment_counts": treatment_counts,
                "paired_deltas": deltas,
            })
    if not selected_deltas:
        raise ValueError(f"no query-count samples matched operation {operation}")
    if require_strict_improvement and not any(
        delta < 0 for delta in selected_deltas
    ):
        raise ValueError("strict improvement requires at least one negative delta")
    if require_strict_increase and not any(
        delta > 0 for delta in selected_deltas
    ):
        raise ValueError("strict increase requires at least one positive delta")

    return {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(),
        "pass": True,
        "errors": [],
        "feature_transition": {
            "feature": feature,
            "control": False,
            "treatment": True,
        },
        "query_count_contract": {
            "operation": operation,
            "minimum_delta": min_delta,
            "maximum_delta": max_delta,
            "allowed_deltas": normalized_allowed_deltas,
            "require_strict_improvement": require_strict_improvement,
            "require_strict_increase": require_strict_increase,
            "unselected_operations_must_be_unchanged": True,
        },
        "provenance": {
            "source_revision": control_meta["source_revision"],
            "api_image_id": control_meta["api_image_id"],
            "retrieval_modes": expected_modes,
            "scales": control_scale_ids,
            "run_profile": dict(control_meta["profile"]),
        },
        "sample_catalogs": sample_catalogs,
        "selected_paired_samples": len(selected_deltas),
        "selected_deltas": selected_deltas,
        "comparisons": comparisons,
    }


def _read_input(
    path: Path,
) -> tuple[dict[str, Any], bytes | None, OSError | None]:
    metadata: dict[str, Any] = {
        "path": str(path.resolve()),
        "sha256": None,
    }
    try:
        body = path.read_bytes()
    except OSError as error:
        metadata["read_error"] = {
            "type": type(error).__name__,
            "message": str(error),
        }
        return metadata, None, error
    metadata["sha256"] = hashlib.sha256(body).hexdigest()
    return metadata, body, None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Compare paired per-sample query counts from two definitive "
            "performance-eval artifacts"
        ),
    )
    parser.add_argument("--control", type=Path, required=True)
    parser.add_argument("--treatment", type=Path, required=True)
    parser.add_argument("--feature", required=True)
    parser.add_argument("--operation", required=True)
    parser.add_argument(
        "--expected-retrieval-modes",
        nargs="+",
        choices=("exact", "lexical", "semantic"),
        required=True,
    )
    parser.add_argument("--min-delta", type=int, required=True)
    parser.add_argument("--max-delta", type=int, required=True)
    parser.add_argument("--require-strict-improvement", action="store_true")
    parser.add_argument("--require-strict-increase", action="store_true")
    parser.add_argument(
        "--allowed-delta",
        action="append",
        type=int,
        dest="allowed_deltas",
    )
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    control_input, control_body, control_error = _read_input(args.control)
    treatment_input, treatment_body, treatment_error = _read_input(
        args.treatment
    )
    inputs = {
        "control": control_input,
        "treatment": treatment_input,
    }
    try:
        if control_error is not None:
            raise control_error
        if treatment_error is not None:
            raise treatment_error
        if control_body is None or treatment_body is None:
            raise OSError("paired input bytes were unavailable")
        control = json.loads(control_body)
        treatment = json.loads(treatment_body)
        result = compare_query_count_artifacts(
            control,
            treatment,
            feature=args.feature,
            operation=args.operation,
            min_delta=args.min_delta,
            max_delta=args.max_delta,
            expected_retrieval_modes=args.expected_retrieval_modes,
            require_strict_improvement=args.require_strict_improvement,
            require_strict_increase=args.require_strict_increase,
            allowed_deltas=args.allowed_deltas,
        )
        result["inputs"] = inputs
        exit_code = 0
    except (OSError, TypeError, json.JSONDecodeError, ValueError) as error:
        result = {
            "schema": SCHEMA,
            "created_at": datetime.now().astimezone().isoformat(),
            "pass": False,
            "errors": [{
                "type": type(error).__name__,
                "message": str(error),
            }],
            "inputs": inputs,
        }
        exit_code = 2
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

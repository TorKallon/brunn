#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import math
import random
import re
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from eval.e09_step_authorization import (
    BASE_EXPERIMENT_ARM,
    STEP_EXPERIMENT_ARM,
    load_step_authorization,
)

SCHEMA = "straylight-paired-draw-aggregate@v1"
RUN_LEDGER_SCHEMA = "straylight-eval-run-ledger@v1"
DEFAULT_ITERATIONS = 10_000
DEFAULT_SEED = 20_260_727
DEFAULT_NON_INFERIORITY_MARGIN_CLAIMS = 5.0
CANONICAL_CONDITIONS = {
    "filesystem_rebuild": "filesystem",
    "service_api_resume": "service_api",
    "workspace_resume": "workspace",
}
EXPERIMENT_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")
RESPONSE_CHARACTER_METRICS_SCHEMA = (
    "straylight-agent-response-character-metrics@v1"
)
RESPONSE_CHARACTER_FIELDS = (
    "service_result_chars",
    "service_source_text_chars",
    "service_metadata_chars",
    "service_replay_weighted_chars",
    "model_visible_tool_output_chars",
)
SERVICE_CONDITIONS = frozenset({"service_api", "service_api_resume"})
SERVICE_IMAGE_PROVENANCE_SCHEMA = "straylight-service-image-provenance@v1"
SERVICE_IMAGE_FINGERPRINT_SCHEMA = "straylight-service-image-fingerprint@v1"
CASE_EXTENSION_PLAN_SCHEMA = "straylight-case-extension-plan@v1"
RETRIEVAL_MODES = frozenset({"exact", "lexical", "semantic"})
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
DOCKER_IMAGE_ID_PATTERN = re.compile(r"^sha256:[0-9a-f]{64}$")


def percentile(values: Iterable[float], quantile: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] * (1.0 - fraction) + ordered[upper] * fraction


def confidence_interval(values: Sequence[float]) -> dict[str, float]:
    return {
        "lower": round(percentile(values, 0.025), 6),
        "upper": round(percentile(values, 0.975), 6),
    }


def exact_mcnemar(discordant_a: int, discordant_b: int) -> dict[str, Any]:
    discordant = discordant_a + discordant_b
    if discordant == 0:
        p_value = 1.0
    else:
        tail = sum(
            math.comb(discordant, value)
            for value in range(min(discordant_a, discordant_b) + 1)
        )
        p_value = min(1.0, 2.0 * tail / (2**discordant))
    return {
        "a_pass_b_fail": discordant_a,
        "a_fail_b_pass": discordant_b,
        "discordant_cases": discordant,
        "two_sided_exact_p": round(p_value, 12),
        "interpretation": (
            "Tests asymmetry of collapsed binary case outcomes; a non-significant "
            "p-value is not evidence of non-inferiority."
        ),
    }


def exact_one_sided_mcnemar(
    discordant_a: int,
    discordant_b: int,
    alternative: str,
) -> dict[str, Any]:
    discordant = discordant_a + discordant_b
    if alternative not in {"a_greater", "b_greater"}:
        raise ValueError(f"unsupported one-sided McNemar alternative: {alternative}")
    if discordant == 0:
        p_value = 1.0
    elif alternative == "a_greater":
        p_value = sum(
            math.comb(discordant, value)
            for value in range(discordant_a, discordant + 1)
        ) / (2**discordant)
    else:
        p_value = sum(
            math.comb(discordant, value)
            for value in range(0, discordant_a + 1)
        ) / (2**discordant)
    return {
        "a_pass_b_fail": discordant_a,
        "a_fail_b_pass": discordant_b,
        "discordant_claims": discordant,
        "alternative": alternative,
        "one_sided_exact_p": round(p_value, 12),
        "interpretation": (
            "Claim outcomes are paired by suite, case, and claim ID, then "
            "strict-majority collapsed across repeated draws before the exact test."
        ),
    }


def canonical_json_sha256(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(
            value,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def _normalize_expected_arm_retrieval_modes(
    bindings: Sequence[str] | Mapping[str, Sequence[str]] | None,
) -> dict[str, list[str]]:
    if bindings is None:
        return {}
    parsed: dict[str, list[str]] = {}
    if isinstance(bindings, Mapping):
        items = bindings.items()
    else:
        if isinstance(bindings, (str, bytes)):
            raise ValueError(
                "expected arm retrieval modes must be repeatable ARM=mode,... "
                "bindings"
            )
        split_items = []
        for binding in bindings:
            if not isinstance(binding, str) or binding.count("=") != 1:
                raise ValueError(
                    "expected arm retrieval modes must use ARM=mode,..."
                )
            arm, rendered_modes = binding.split("=", 1)
            split_items.append((arm, rendered_modes.split(",")))
        items = split_items
    for arm, raw_modes in items:
        if isinstance(raw_modes, (str, bytes)):
            raise ValueError(
                f"invalid expected retrieval modes for arm {arm!r}"
            )
        try:
            modes = list(raw_modes)
        except TypeError as error:
            raise ValueError(
                f"invalid expected retrieval modes for arm {arm!r}"
            ) from error
        if (
            not isinstance(arm, str)
            or not EXPERIMENT_ID_PATTERN.fullmatch(arm)
            or not modes
            or any(mode not in RETRIEVAL_MODES for mode in modes)
            or len(modes) != len(set(modes))
        ):
            raise ValueError(
                f"invalid expected retrieval modes for arm {arm!r}"
            )
        if arm in parsed:
            raise ValueError(
                f"duplicate expected retrieval-mode binding for arm {arm}"
            )
        parsed[arm] = modes
    return dict(sorted(parsed.items()))


def _service_run_provenance(
    run: dict[str, Any],
    path: Path,
    *,
    source_revision: str,
    service_arms: Sequence[str],
    definitive: bool,
) -> dict[str, Any] | None:
    if not service_arms:
        return None
    modes = run.get("service_retrieval_modes")
    image_provenance = run.get("service_image_provenance")
    if not definitive and modes is None and image_provenance is None:
        return None
    if (
        run.get("service_protocol") != "simple"
        or not isinstance(modes, list)
        or not modes
        or any(mode not in RETRIEVAL_MODES for mode in modes)
        or len(modes) != len(set(modes))
    ):
        raise ValueError(
            f"{path}: service-backed run lacks explicit simple-protocol "
            "retrieval modes"
        )
    parameters = run.get("experiment_parameters")
    ledger = run.get("run_ledger")
    configuration = ledger.get("configuration") if isinstance(ledger, dict) else None
    ledger_parameters = (
        configuration.get("experiment_parameters")
        if isinstance(configuration, dict)
        else None
    )
    if (
        not isinstance(parameters, dict)
        or parameters.get("service_retrieval_modes") != modes
        or not isinstance(configuration, dict)
        or configuration.get("service_protocol") != "simple"
        or not isinstance(ledger_parameters, dict)
        or ledger_parameters.get("service_retrieval_modes") != modes
    ):
        raise ValueError(
            f"{path}: top-level, experiment-parameter, and ledger retrieval "
            "modes disagree"
        )
    runtime = run.get("service_runtime_snapshot")
    expected_build_revision = run.get("expected_build_revision")
    if (
        not isinstance(runtime, dict)
        or runtime.get("schema") != "straylight-service-runtime-snapshot@v1"
        or runtime.get("status") != "ready"
        or runtime.get("build_revision") != source_revision
        or expected_build_revision != source_revision
        or configuration.get("expected_build_revision") != source_revision
    ):
        raise ValueError(
            f"{path}: service runtime/build does not match source revision"
        )
    if (
        not isinstance(image_provenance, dict)
        or image_provenance.get("schema") != SERVICE_IMAGE_PROVENANCE_SCHEMA
        or image_provenance.get("stable") is not True
    ):
        raise ValueError(
            f"{path}: service-backed run lacks stable image provenance"
        )
    before = image_provenance.get("before")
    after = image_provenance.get("after")
    required_fingerprint_fields = (
        "schema",
        "api_container",
        "api_container_id",
        "api_container_started_at",
        "api_container_running",
        "api_image_id",
        "api_image_revision",
    )
    if (
        not isinstance(before, dict)
        or not isinstance(after, dict)
        or before != after
        or before.get("schema") != SERVICE_IMAGE_FINGERPRINT_SCHEMA
        or any(field not in before for field in required_fingerprint_fields)
        or before.get("api_container_running") is not True
        or any(
            not isinstance(before.get(field), str) or not before[field]
            for field in (
                "api_container",
                "api_container_id",
                "api_container_started_at",
            )
        )
        or not isinstance(before.get("api_image_id"), str)
        or not DOCKER_IMAGE_ID_PATTERN.fullmatch(before["api_image_id"])
        or before.get("api_image_revision") != source_revision
        or parameters.get("service_image_fingerprint") != before
    ):
        raise ValueError(
            f"{path}: service image fingerprint is malformed, unstable, or "
            "not source-bound"
        )
    artifacts = ledger.get("artifacts") if isinstance(ledger, dict) else None
    if (
        not isinstance(artifacts, dict)
        or artifacts.get("service_image_provenance_sha256")
        != canonical_json_sha256(image_provenance)
    ):
        raise ValueError(
            f"{path}: run ledger does not hash-bind service image provenance"
        )
    return {
        "schema": "straylight-aggregate-service-provenance@v1",
        "service_arms": list(service_arms),
        "service_retrieval_modes": list(modes),
        "source_revision": source_revision,
        "build_revision": runtime["build_revision"],
        "api_image_id": before["api_image_id"],
        "api_image_revision": before["api_image_revision"],
        "service_image_provenance": image_provenance,
    }


def validate_service_provenance_pairing(
    artifacts: Sequence[dict[str, Any]],
    expected_arm_retrieval_modes: (
        Sequence[str] | Mapping[str, Sequence[str]] | None
    ),
    *,
    definitive: bool,
) -> dict[str, Any]:
    expected = _normalize_expected_arm_retrieval_modes(
        expected_arm_retrieval_modes
    )
    service_arms = sorted({
        arm
        for artifact in artifacts
        for arm in artifact.get("service_arms", [])
    })
    if definitive and set(expected) != set(service_arms):
        raise ValueError(
            "expected arm retrieval-mode mappings must exactly cover every "
            f"definitive service-backed arm: expected {service_arms}, "
            f"declared {sorted(expected)}"
        )
    proven = [
        artifact
        for artifact in artifacts
        if artifact.get("service_arms")
    ]
    for artifact in proven:
        provenance = artifact.get("service_provenance")
        if not isinstance(provenance, dict):
            if definitive:
                raise ValueError(
                    f"{artifact['path']}: definitive service artifact lacks "
                    "validated provenance"
                )
            continue
        for arm in artifact["service_arms"]:
            declared = expected.get(arm)
            if declared is not None and (
                provenance["service_retrieval_modes"] != declared
            ):
                raise ValueError(
                    f"{artifact['path']}: service retrieval modes for arm "
                    f"{arm} do not match the explicit mapping"
                )
    validated = [
        artifact["service_provenance"]
        for artifact in proven
        if isinstance(artifact.get("service_provenance"), dict)
    ]
    for field in (
        "source_revision",
        "build_revision",
        "api_image_id",
        "api_image_revision",
    ):
        values = {item[field] for item in validated}
        if len(values) > 1:
            raise ValueError(
                f"paired service artifacts do not share one {field}: "
                f"{sorted(values)}"
            )
    return {
        "schema": "straylight-aggregate-service-provenance-summary@v1",
        "status": "complete" if validated else "not_applicable",
        "expected_arm_retrieval_modes": expected,
        "service_arms": service_arms,
        "source_revision": (
            validated[0]["source_revision"] if validated else None
        ),
        "build_revision": (
            validated[0]["build_revision"] if validated else None
        ),
        "api_image_id": (
            validated[0]["api_image_id"] if validated else None
        ),
        "api_image_revision": (
            validated[0]["api_image_revision"] if validated else None
        ),
        "distinct_container_instances_allowed": True,
    }


def _validate_run_ledger(run: dict[str, Any]) -> str:
    ledger = run.get("run_ledger")
    if not isinstance(ledger, dict) or ledger.get("schema") != RUN_LEDGER_SCHEMA:
        raise ValueError("input is missing a current immutable run_ledger")
    source = ledger.get("source")
    if (
        not isinstance(source, dict)
        or source.get("clean") is not True
        or source.get("tracked_source_clean") is not True
        or source.get("untracked_source_files") != []
    ):
        raise ValueError("input run ledger does not record a clean source tree")
    revision = source.get("revision")
    if not isinstance(revision, str) or not revision:
        raise ValueError("input run ledger is missing the git revision")
    codex = ledger.get("codex")
    if not isinstance(codex, dict):
        raise ValueError("input run ledger is missing Codex provenance")
    if (
        not all(
            isinstance(codex.get(key), str) and codex[key]
            for key in ("path", "version", "auth_checked_at")
        )
        or codex.get("auth_route") != "chatgpt_subscription"
        or codex.get("auth_status") != "Logged in using ChatGPT"
        or codex.get("api_fallback") != "forbidden"
    ):
        raise ValueError("input run ledger lacks fail-closed ChatGPT auth proof")
    if (
        ledger.get("run_id") != run.get("run_id")
        or not isinstance(ledger.get("captured_at"), str)
        or not ledger["captured_at"]
    ):
        raise ValueError("input run ledger is not bound to this run")
    configuration = ledger.get("configuration")
    manifest = run.get("manifest")
    if (
        not isinstance(configuration, dict)
        or not isinstance(manifest, dict)
        or configuration.get("model") != manifest.get("model")
        or configuration.get("conditions") != manifest.get("conditions")
        or configuration.get("experiment_arm") != run.get("experiment_arm")
        or configuration.get("paired_draw_id") != run.get("paired_draw_id")
        or configuration.get("expected_runtime_features", {})
        != run.get("expected_runtime_features", {})
        or configuration.get("expected_build_revision")
        != run.get("expected_build_revision")
        or configuration.get("experiment_parameters", {})
        != run.get("experiment_parameters", {})
    ):
        raise ValueError("input run ledger does not match the run configuration")
    experiment_arm = run.get("experiment_arm")
    paired_draw_id = run.get("paired_draw_id")
    if bool(experiment_arm) != bool(paired_draw_id):
        raise ValueError("input run has an incomplete experiment arm/draw identity")
    if experiment_arm is not None and len(manifest.get("conditions", [])) != 1:
        raise ValueError("explicit experiment arms require one condition per artifact")
    artifacts = ledger.get("artifacts")
    execution_fingerprints = run.get("execution_fingerprints")
    original_manifest_sha256 = (
        execution_fingerprints.get("manifest_sha256")
        if isinstance(execution_fingerprints, dict)
        else run.get("manifest_sha256")
    )
    original_harness_sha256 = (
        execution_fingerprints.get("harness_sha256")
        if isinstance(execution_fingerprints, dict)
        else run.get("harness_sha256")
    )
    if (
        not isinstance(artifacts, dict)
        or not all(
            isinstance(artifacts.get(key), str) and artifacts[key]
            for key in (
                "manifest_sha256",
                "schema_sha256",
                "harness_sha256",
            )
        )
        or artifacts["manifest_sha256"] != original_manifest_sha256
        or artifacts["harness_sha256"] != original_harness_sha256
    ):
        raise ValueError("input run ledger does not match the run fingerprints")
    runtime_snapshot = run.get("service_runtime_snapshot")
    runtime_snapshot_sha256 = artifacts.get("runtime_snapshot_sha256")
    if runtime_snapshot is None:
        if runtime_snapshot_sha256 is not None:
            raise ValueError("run ledger references a missing runtime snapshot")
    else:
        if (
            not isinstance(runtime_snapshot, dict)
            or runtime_snapshot_sha256
            != hashlib.sha256(
                json.dumps(
                    runtime_snapshot,
                    ensure_ascii=False,
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode("utf-8")
            ).hexdigest()
        ):
            raise ValueError("run ledger does not match the runtime snapshot")
        runtime_features = runtime_snapshot.get("runtime_features")
        build_revision = runtime_snapshot.get("build_revision")
        if (
            runtime_snapshot.get("schema")
            != "straylight-service-runtime-snapshot@v1"
            or runtime_snapshot.get("status") != "ready"
            or not isinstance(runtime_snapshot.get("captured_at"), str)
            or not runtime_snapshot["captured_at"]
            or not isinstance(runtime_features, dict)
            or not isinstance(build_revision, str)
            or not build_revision
            or build_revision == "unknown"
        ):
            raise ValueError("run contains a malformed service runtime snapshot")
        expected_runtime_features = run.get("expected_runtime_features", {})
        if not isinstance(expected_runtime_features, dict):
            raise ValueError("run contains malformed runtime expectations")
        mismatches = {
            name: {"expected": expected, "actual": runtime_features.get(name)}
            for name, expected in expected_runtime_features.items()
            if (
                name not in runtime_features
                or type(runtime_features[name]) is not type(expected)
                or runtime_features[name] != expected
            )
        }
        if mismatches:
            raise ValueError(
                f"run runtime snapshot violates its expectations: {mismatches}"
            )
        expected_build_revision = run.get("expected_build_revision")
        if (
            expected_build_revision is not None
            and build_revision != expected_build_revision
        ):
            raise ValueError(
                "run runtime snapshot violates its expected build revision"
            )
    selected_conditions = manifest.get("conditions", [])
    uses_service = any(
        condition in {"service_api", "service_api_resume"}
        for condition in selected_conditions
    )
    if (
        uses_service
        and "expected_runtime_features" in configuration
        and runtime_snapshot is None
    ):
        raise ValueError("service run is missing its authenticated runtime snapshot")
    return revision


def _grading_revision(run: dict[str, Any], execution_revision: str) -> str:
    if not run.get("regraded_at"):
        return execution_revision
    fingerprints = run.get("regrade_fingerprints")
    source = fingerprints.get("source") if isinstance(fingerprints, dict) else None
    if (
        not isinstance(fingerprints, dict)
        or fingerprints.get("captured_at") != run.get("regraded_at")
        or fingerprints.get("manifest_sha256") != run.get("manifest_sha256")
        or fingerprints.get("harness_sha256") != run.get("harness_sha256")
        or not isinstance(source, dict)
        or source.get("clean") is not True
        or source.get("tracked_source_clean") is not True
        or source.get("untracked_source_files") != []
        or not isinstance(source.get("revision"), str)
        or not source["revision"]
    ):
        raise ValueError("regraded input lacks a clean, matching grader fingerprint")
    return source["revision"]


def _manifest_case_metadata(
    path: Path,
    manifest: dict[str, Any],
    cases: Sequence[dict[str, Any]],
    *,
    definitive: bool,
) -> tuple[
    dict[str, list[str]],
    dict[str, list[str]],
    dict[str, dict[str, tuple[str, ...]]],
    dict[str, tuple[str, ...]],
]:
    case_ids = {str(case["id"]) for case in cases}
    raw_families = manifest.get("feature_families", {})
    if not isinstance(raw_families, dict):
        raise ValueError(f"{path}: manifest feature_families must be an object")
    source_feature_families: dict[str, list[str]] = {}
    feature_families: dict[str, list[str]] = {}
    for family_id, members in sorted(raw_families.items()):
        if (
            not isinstance(family_id, str)
            or not EXPERIMENT_ID_PATTERN.fullmatch(family_id)
            or not isinstance(members, list)
            or not members
            or not all(isinstance(member, str) and member for member in members)
            or len(members) != len(set(members))
        ):
            raise ValueError(
                f"{path}: manifest feature family {family_id!r} is malformed"
            )
        source_feature_families[family_id] = list(members)
        feature_families[family_id] = [
            member for member in members if member in case_ids
        ]

    claim_tags_by_case: dict[str, dict[str, tuple[str, ...]]] = {}
    forbidden_by_case: dict[str, tuple[str, ...]] = {}
    for case in cases:
        case_id = str(case["id"])
        rubric = case.get("rubric")
        if rubric is None and not definitive:
            rubric = []
        if not isinstance(rubric, list):
            raise ValueError(f"{path}: {case_id} lacks a structured rubric")
        claim_tags: dict[str, tuple[str, ...]] = {}
        for claim in rubric:
            tags = claim.get("tags", []) if isinstance(claim, dict) else None
            if (
                not isinstance(claim, dict)
                or not isinstance(claim.get("id"), str)
                or not claim["id"]
                or not isinstance(tags, list)
                or not all(isinstance(tag, str) and tag for tag in tags)
                or len(tags) != len(set(tags))
                or claim["id"] in claim_tags
            ):
                raise ValueError(
                    f"{path}: {case_id} has malformed manifest claim tags"
                )
            claim_tags[claim["id"]] = tuple(sorted(tags))
        claim_tags_by_case[case_id] = claim_tags

        forbidden = case.get("forbidden")
        if forbidden is None and not definitive:
            forbidden = []
        if (
            not isinstance(forbidden, list)
            or not all(isinstance(item, str) and item for item in forbidden)
            or len(forbidden) != len(set(forbidden))
        ):
            raise ValueError(
                f"{path}: {case_id} has malformed forbidden assertions"
            )
        forbidden_by_case[case_id] = tuple(forbidden)
    return (
        feature_families,
        source_feature_families,
        claim_tags_by_case,
        forbidden_by_case,
    )


def _response_character_metrics(
    path: Path,
    key: tuple[str, str],
    record: dict[str, Any],
    *,
    definitive: bool,
) -> dict[str, Any]:
    envelope = record.get("response_character_metrics")
    if envelope is None and not definitive:
        model_visible = record.get("model_visible_tool_output_chars")
        service_result = record.get("service_result_chars")
        service_source = record.get("service_source_text_chars")
        service_metadata = record.get("service_metadata_chars")
        service_replay = record.get("service_replay_weighted_chars")
        if type(service_result) is not int:
            service_result = model_visible
        if type(service_source) is not int:
            service_source = 0
        if type(service_metadata) is not int:
            service_metadata = (
                max(0, service_result - service_source)
                if type(service_result) is int
                else None
            )
        if type(service_replay) is not int:
            service_replay = service_result
        envelope = {
            "schema": RESPONSE_CHARACTER_METRICS_SCHEMA,
            "service_result_chars": service_result,
            "service_source_text_chars": service_source,
            "service_metadata_chars": service_metadata,
            "service_replay_weighted_chars": service_replay,
            "model_visible_tool_output_chars": model_visible,
            "derivation": "legacy_flat_fields",
        }
    if (
        not isinstance(envelope, dict)
        or envelope.get("schema") != RESPONSE_CHARACTER_METRICS_SCHEMA
    ):
        raise ValueError(
            f"{path}: {key} lacks the canonical response_character_metrics "
            "envelope"
        )
    normalized: dict[str, int | str] = {
        "schema": RESPONSE_CHARACTER_METRICS_SCHEMA,
    }
    if envelope.get("derivation") == "legacy_flat_fields":
        normalized["derivation"] = "legacy_flat_fields"
    for field in RESPONSE_CHARACTER_FIELDS:
        value = envelope.get(field)
        if type(value) is not int or value < 0:
            raise ValueError(
                f"{path}: {key} has malformed response character metric {field}"
            )
        flat_value = record.get(field)
        if flat_value is not None and (
            type(flat_value) is not int
            or flat_value < 0
            or flat_value != value
        ):
            raise ValueError(
                f"{path}: {key} response character metric {field} disagrees "
                "with its flat record field"
            )
        normalized[field] = value
    return normalized


def load_draw(
    path: Path,
    *,
    definitive: bool = True,
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    try:
        run = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"{path}: could not load result JSON: {exc}") from exc
    if not isinstance(run, dict):
        raise ValueError(f"{path}: result must be a JSON object")
    revision = _validate_run_ledger(run)
    grading_revision = _grading_revision(run, revision)
    billing = run.get("reasoning_billing")
    if (
        not isinstance(billing, dict)
        or billing.get("route") != "chatgpt_subscription"
        or billing.get("api_fallback") != "forbidden"
        or billing.get("auth_status") != "Logged in using ChatGPT"
    ):
        raise ValueError(f"{path}: missing verified subscription reasoning metadata")
    run_id = run.get("run_id")
    manifest = run.get("manifest")
    records = run.get("records")
    if (
        not isinstance(run_id, str)
        or not run_id
        or not isinstance(manifest, dict)
        or not isinstance(records, list)
    ):
        raise ValueError(f"{path}: missing run_id, manifest, or records")
    suite = str(run.get("benchmark_version") or manifest.get("benchmark_version") or "")
    if not suite:
        raise ValueError(f"{path}: missing benchmark version")
    conditions = manifest.get("conditions")
    cases = manifest.get("cases")
    if not isinstance(conditions, list) or not conditions or not isinstance(cases, list):
        raise ValueError(f"{path}: manifest lacks selected conditions or cases")
    if (
        not all(isinstance(condition, str) and condition for condition in conditions)
        or len(conditions) != len(set(conditions))
    ):
        raise ValueError(f"{path}: manifest has malformed or duplicate conditions")
    case_ids = [
        case.get("id")
        for case in cases
        if isinstance(case, dict)
    ]
    if (
        len(case_ids) != len(cases)
        or not all(isinstance(case_id, str) and case_id for case_id in case_ids)
        or len(case_ids) != len(set(case_ids))
    ):
        raise ValueError(f"{path}: manifest has malformed or duplicate case IDs")
    (
        feature_families,
        source_feature_families,
        claim_tags_by_case,
        forbidden_by_case,
    ) = (
        _manifest_case_metadata(
            path,
            manifest,
            cases,
            definitive=definitive,
        )
    )
    families_by_case: dict[str, list[str]] = defaultdict(list)
    for family_id, members in feature_families.items():
        for case_id in members:
            families_by_case[case_id].append(family_id)
    experiment_arm = run.get("experiment_arm")
    paired_draw_id = run.get("paired_draw_id")
    explicit_identity = experiment_arm is not None
    if explicit_identity:
        if (
            not isinstance(experiment_arm, str)
            or not EXPERIMENT_ID_PATTERN.fullmatch(experiment_arm)
            or not isinstance(paired_draw_id, str)
            or not EXPERIMENT_ID_PATTERN.fullmatch(paired_draw_id)
        ):
            raise ValueError(f"{path}: invalid explicit experiment arm identity")
        if len(conditions) != 1:
            raise ValueError(
                f"{path}: explicit experiment arms require one condition per artifact"
            )
    expected = {
        (str(case["id"]), str(condition))
        for case in cases
        for condition in conditions
    }
    observed: set[tuple[str, str]] = set()
    normalized = []
    for record in records:
        if not isinstance(record, dict):
            raise ValueError(f"{path}: record is not an object")
        key = (str(record.get("case_id")), str(record.get("condition")))
        if key in observed:
            raise ValueError(f"{path}: duplicate record for {key}")
        observed.add(key)
        if key not in expected:
            raise ValueError(f"{path}: unexpected record for {key}")
        grade = record.get("grade")
        case_pass = (
            record.get("transition_pass")
            if "transition_pass" in record
            else grade.get("pass") if isinstance(grade, dict) else None
        )
        if (
            record.get("error")
            or not isinstance(grade, dict)
            or type(grade.get("claims_passed")) is not int
            or type(grade.get("claims_total")) is not int
            or grade["claims_total"] <= 0
            or not 0 <= grade["claims_passed"] <= grade["claims_total"]
            or not isinstance(case_pass, bool)
        ):
            raise ValueError(f"{path}: incomplete or failed record for {key}")
        legacy_derivations: list[str] = []
        character_metrics = _response_character_metrics(
            path,
            key,
            record,
            definitive=definitive,
        )
        if character_metrics.get("derivation") == "legacy_flat_fields":
            legacy_derivations.append("response_character_metrics")
        canonical = CANONICAL_CONDITIONS.get(key[1], key[1])
        claim_rows = grade.get("claims")
        claim_outcomes = None
        claim_tags = None
        expected_claim_tags = claim_tags_by_case[key[0]]
        if claim_rows is None and definitive:
            raise ValueError(
                f"{path}: {key} lacks structured claim-level outcomes"
            )
        if isinstance(claim_rows, list):
            claim_outcomes = {}
            claim_tags = {}
            derived_claim_metadata = False
            for claim in claim_rows:
                tags = claim.get("tags") if isinstance(claim, dict) else None
                if (
                    tags is None
                    and not definitive
                    and isinstance(claim, dict)
                    and isinstance(claim.get("id"), str)
                ):
                    tags = list(
                        expected_claim_tags.get(claim["id"], ())
                    )
                    derived_claim_metadata = True
                if (
                    not isinstance(claim, dict)
                    or not isinstance(claim.get("id"), str)
                    or not isinstance(claim.get("pass"), bool)
                    or not isinstance(tags, list)
                    or not all(isinstance(tag, str) and tag for tag in tags)
                    or len(tags) != len(set(tags))
                    or claim["id"] in claim_outcomes
                ):
                    raise ValueError(
                        f"{path}: {key} has malformed claim-level outcomes"
                    )
                claim_outcomes[claim["id"]] = claim["pass"]
                claim_tags[claim["id"]] = tuple(sorted(tags))
                exact_value = claim.get("exact_value")
                if exact_value is None and not definitive:
                    exact_value = "exact_value" in claim_tags[claim["id"]]
                    derived_claim_metadata = True
                if (
                    type(exact_value) is not bool
                    or exact_value
                    != ("exact_value" in claim_tags[claim["id"]])
                ):
                    raise ValueError(
                        f"{path}: {key} claim {claim['id']} has inconsistent "
                        "exact_value tagging"
                    )
            if len(claim_outcomes) != grade["claims_total"]:
                raise ValueError(
                    f"{path}: {key} claim-level outcomes do not match claims_total"
                )
            if (
                (definitive or expected_claim_tags)
                and set(claim_outcomes) != set(expected_claim_tags)
            ):
                raise ValueError(
                    f"{path}: {key} claim IDs do not match the manifest rubric"
                )
            tag_mismatches = {
                claim_id: {
                    "manifest": expected_claim_tags[claim_id],
                    "record": claim_tags[claim_id],
                }
                for claim_id in sorted(claim_tags)
                if (
                    claim_id in expected_claim_tags
                    and claim_tags[claim_id] != expected_claim_tags[claim_id]
                )
            }
            if tag_mismatches:
                raise ValueError(
                    f"{path}: {key} claim tags do not match the manifest rubric: "
                    f"{tag_mismatches}"
                )
            if derived_claim_metadata:
                legacy_derivations.append("claim_metadata")
        elif claim_rows is not None:
            raise ValueError(f"{path}: {key} has malformed claim-level outcomes")
        forbidden_hits = grade.get("forbidden_hits")
        if forbidden_hits is None and not definitive:
            forbidden_hits = []
            legacy_derivations.append("forbidden_hits")
        if (
            not isinstance(forbidden_hits, list)
            or not all(isinstance(hit, str) and hit for hit in forbidden_hits)
            or len(forbidden_hits) != len(set(forbidden_hits))
            or not set(forbidden_hits) <= set(forbidden_by_case[key[0]])
        ):
            raise ValueError(f"{path}: {key} has malformed forbidden_hits")
        normalized.append({
            "suite": suite,
            "draw": paired_draw_id if explicit_identity else run_id,
            "case": key[0],
            "case_key": f"{suite}:{key[0]}",
            "arm": experiment_arm if explicit_identity else canonical,
            "identity_mode": (
                "explicit_arm" if explicit_identity else "condition_arm"
            ),
            "condition": canonical,
            "source_condition": key[1],
            "claims_passed": grade["claims_passed"],
            "claims_total": grade["claims_total"],
            "case_pass": case_pass,
            "response_character_metrics": character_metrics,
            **{
                field: character_metrics[field]
                for field in RESPONSE_CHARACTER_FIELDS
            },
            "persisted_checkpoint": bool(record.get("persisted_checkpoint")),
            "claim_outcomes": claim_outcomes,
            "claim_tags": claim_tags,
            "feature_families": tuple(sorted(families_by_case[key[0]])),
            "forbidden_hits": tuple(forbidden_hits),
            "legacy_derivations": tuple(legacy_derivations),
        })
    missing = sorted(expected - observed)
    if missing:
        raise ValueError(f"{path}: incomplete draw; missing {missing[:5]}")
    service_arms = sorted({
        record["arm"]
        for record in normalized
        if record["source_condition"] in SERVICE_CONDITIONS
    })
    service_provenance = _service_run_provenance(
        run,
        path,
        source_revision=revision,
        service_arms=service_arms,
        definitive=definitive,
    )
    artifact = {
        "path": str(path.resolve()),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "run_id": run_id,
        "paired_draw_id": paired_draw_id if explicit_identity else run_id,
        "experiment_arm": experiment_arm,
        "identity_mode": "explicit_arm" if explicit_identity else "condition_arm",
        "suite": suite,
        "conditions": list(conditions),
        "arms": list(dict.fromkeys(record["arm"] for record in normalized)),
        "cases": len(cases),
        "feature_families": feature_families,
        "source_feature_families": source_feature_families,
        "source_revision": revision,
        "grading_revision": grading_revision,
        "manifest_sha256": run.get("manifest_sha256"),
        "execution_manifest_sha256": run["run_ledger"]["artifacts"][
            "manifest_sha256"
        ],
        "model": manifest.get("model"),
        "expected_runtime_features": run.get("expected_runtime_features", {}),
        "expected_build_revision": run.get("expected_build_revision"),
        "experiment_parameters": run.get("experiment_parameters", {}),
        "service_runtime_snapshot": run.get("service_runtime_snapshot"),
        "service_retrieval_modes": run.get("service_retrieval_modes"),
        "service_image_provenance": run.get("service_image_provenance"),
        "service_arms": service_arms,
        "service_provenance": service_provenance,
        "legacy_derivations": {
            derivation: sum(
                derivation in record["legacy_derivations"]
                for record in normalized
            )
            for derivation in (
                "response_character_metrics",
                "claim_metadata",
                "forbidden_hits",
            )
        },
        "run_ledger": run["run_ledger"],
    }
    return artifact, normalized


def _case_clusters(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
) -> list[dict[str, Any]]:
    by_observation = {
        (
            record["suite"],
            record["draw"],
            record["case"],
            record["arm"],
        ): record
        for record in records
    }
    case_draws: dict[str, list[tuple[dict[str, Any], dict[str, Any]]]] = defaultdict(list)
    for suite, draw, case, arm in sorted(by_observation):
        if arm != arm_a:
            continue
        left = by_observation[(suite, draw, case, arm_a)]
        right = by_observation.get((suite, draw, case, arm_b))
        if right is not None:
            case_draws[left["case_key"]].append((left, right))
    clusters = []
    for case_key, pairs in sorted(case_draws.items()):
        claim_totals = {
            (left["claims_total"], right["claims_total"])
            for left, right in pairs
        }
        if any(left != right for left, right in claim_totals) or len(claim_totals) != 1:
            raise ValueError(f"{case_key}: paired claim totals differ")
        claims_total = next(iter(claim_totals))[0]
        a_pass_votes = sum(left["case_pass"] for left, _ in pairs)
        b_pass_votes = sum(right["case_pass"] for _, right in pairs)
        draw_count = len(pairs)
        a_binary = (
            True if a_pass_votes * 2 > draw_count
            else False if a_pass_votes * 2 < draw_count
            else None
        )
        b_binary = (
            True if b_pass_votes * 2 > draw_count
            else False if b_pass_votes * 2 < draw_count
            else None
        )
        mean_a = sum(left["claims_passed"] for left, _ in pairs) / draw_count
        mean_b = sum(right["claims_passed"] for _, right in pairs) / draw_count
        response_character_metrics = {}
        for field in RESPONSE_CHARACTER_FIELDS:
            mean_chars_a = sum(
                left["response_character_metrics"][field]
                for left, _ in pairs
            ) / draw_count
            mean_chars_b = sum(
                right["response_character_metrics"][field]
                for _, right in pairs
            ) / draw_count
            response_character_metrics[field] = {
                "mean_a": mean_chars_a,
                "mean_b": mean_chars_b,
                "mean_difference": mean_chars_a - mean_chars_b,
            }
        model_visible = response_character_metrics[
            "model_visible_tool_output_chars"
        ]
        clusters.append({
            "case_key": case_key,
            "suite": pairs[0][0]["suite"],
            "case_id": pairs[0][0]["case"],
            "draws": draw_count,
            "claims_total": claims_total,
            "mean_claims_a": mean_a,
            "mean_claims_b": mean_b,
            "mean_claim_difference": mean_a - mean_b,
            "mean_chars_a": model_visible["mean_a"],
            "mean_chars_b": model_visible["mean_b"],
            "mean_char_difference": model_visible["mean_difference"],
            "response_character_metrics": response_character_metrics,
            "case_pass_votes_a": a_pass_votes,
            "case_pass_votes_b": b_pass_votes,
            "case_pass_a": a_binary,
            "case_pass_b": b_binary,
            "paired_draw_outcomes": [
                {
                    "draw": left["draw"],
                    "case_pass_a": left["case_pass"],
                    "case_pass_b": right["case_pass"],
                    "claims_passed_a": left["claims_passed"],
                    "claims_passed_b": right["claims_passed"],
                }
                for left, right in pairs
            ],
            "persisted_checkpoint_rate_a": sum(
                left["persisted_checkpoint"] for left, _ in pairs
            ) / draw_count,
            "persisted_checkpoint_rate_b": sum(
                right["persisted_checkpoint"] for _, right in pairs
            ) / draw_count,
        })
    return clusters


def _paired_observations(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
) -> list[tuple[dict[str, Any], dict[str, Any]]]:
    by_observation = {
        (
            record["suite"],
            record["draw"],
            record["case"],
            record["arm"],
        ): record
        for record in records
    }
    pairs = []
    for suite, draw, case, arm in sorted(by_observation):
        if arm != arm_a:
            continue
        right = by_observation.get((suite, draw, case, arm_b))
        if right is not None:
            pairs.append((by_observation[(suite, draw, case, arm_a)], right))
    return pairs


def _paired_draw_instance_summary(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
) -> dict[str, Any]:
    pairs = _paired_observations(records, arm_a, arm_b)
    by_draw: dict[tuple[str, str], list[tuple[dict[str, Any], dict[str, Any]]]] = (
        defaultdict(list)
    )
    for left, right in pairs:
        by_draw[(left["suite"], left["draw"])].append((left, right))

    def summarize(
        selected: Sequence[tuple[dict[str, Any], dict[str, Any]]],
    ) -> dict[str, Any]:
        claims_total_a = sum(left["claims_total"] for left, _ in selected)
        claims_total_b = sum(right["claims_total"] for _, right in selected)
        if claims_total_a != claims_total_b:
            raise ValueError("paired draw instances have unequal claim totals")
        claims_passed_a = sum(left["claims_passed"] for left, _ in selected)
        claims_passed_b = sum(right["claims_passed"] for _, right in selected)
        a_wins = sum(
            left["claims_passed"] > right["claims_passed"]
            for left, right in selected
        )
        b_wins = sum(
            left["claims_passed"] < right["claims_passed"]
            for left, right in selected
        )
        return {
            "case_instances": len(selected),
            "a_wins": a_wins,
            "b_wins": b_wins,
            "ties": len(selected) - a_wins - b_wins,
            "claim_slots_per_arm": claims_total_a,
            "claims_passed_a": claims_passed_a,
            "claims_passed_b": claims_passed_b,
            "claim_net_a_minus_b": claims_passed_a - claims_passed_b,
            "claim_rate_a": round(
                claims_passed_a / max(1, claims_total_a),
                8,
            ),
            "claim_rate_b": round(
                claims_passed_b / max(1, claims_total_b),
                8,
            ),
        }

    overall = summarize(pairs)
    overall["by_draw"] = [
        {
            "suite": suite,
            "draw": draw,
            **summarize(draw_pairs),
        }
        for (suite, draw), draw_pairs in sorted(by_draw.items())
    ]
    return overall


def _paired_forbidden_hit_summary(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
) -> dict[str, Any]:
    pairs = _paired_observations(records, arm_a, arm_b)
    new_in_a = []
    new_in_b = []
    shared = []
    for left, right in pairs:
        identity = {
            "suite": left["suite"],
            "draw": left["draw"],
            "case_id": left["case"],
        }
        left_hits = set(left["forbidden_hits"])
        right_hits = set(right["forbidden_hits"])
        new_in_a.extend(
            {**identity, "hit": hit}
            for hit in sorted(left_hits - right_hits)
        )
        new_in_b.extend(
            {**identity, "hit": hit}
            for hit in sorted(right_hits - left_hits)
        )
        shared.extend(
            {**identity, "hit": hit}
            for hit in sorted(left_hits & right_hits)
        )
    return {
        "hit_instances_a": sum(
            len(left["forbidden_hits"]) for left, _ in pairs
        ),
        "hit_instances_b": sum(
            len(right["forbidden_hits"]) for _, right in pairs
        ),
        "case_instances_with_hits_a": sum(
            bool(left["forbidden_hits"]) for left, _ in pairs
        ),
        "case_instances_with_hits_b": sum(
            bool(right["forbidden_hits"]) for _, right in pairs
        ),
        "new_in_a_count": len(new_in_a),
        "new_in_b_count": len(new_in_b),
        "shared_count": len(shared),
        "new_in_a_instances": new_in_a,
        "new_in_b_instances": new_in_b,
        "shared_instances": shared,
    }


def _claim_level_mcnemar(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
    alternative: str,
) -> dict[str, Any]:
    by_observation = {
        (
            record["suite"],
            record["draw"],
            record["case"],
            record["arm"],
        ): record
        for record in records
    }
    claim_draws: dict[
        tuple[str, str, str],
        list[tuple[bool, bool]],
    ] = defaultdict(list)
    for suite, draw, case, arm in sorted(by_observation):
        if arm != arm_a:
            continue
        left = by_observation[(suite, draw, case, arm_a)]
        right = by_observation.get((suite, draw, case, arm_b))
        if right is None:
            continue
        left_claims = left.get("claim_outcomes")
        right_claims = right.get("claim_outcomes")
        if not isinstance(left_claims, dict) or not isinstance(right_claims, dict):
            raise ValueError(
                "claim-level McNemar requires structured claim outcomes in every record"
            )
        if set(left_claims) != set(right_claims):
            raise ValueError(
                f"{suite}:{case}: paired arms have different claim IDs"
            )
        for claim_id in sorted(left_claims):
            claim_draws[(suite, case, claim_id)].append(
                (left_claims[claim_id], right_claims[claim_id])
            )
    if not claim_draws:
        raise ValueError(f"no paired claim outcomes for {arm_a} vs {arm_b}")
    expected_draws_by_case: dict[tuple[str, str], int] = defaultdict(int)
    for suite, draw, case, arm in sorted(by_observation):
        if arm == arm_a and (suite, draw, case, arm_b) in by_observation:
            expected_draws_by_case[(suite, case)] += 1
    incomplete_claim_draws = {
        key: len(pairs)
        for key, pairs in claim_draws.items()
        if len(pairs) != expected_draws_by_case[(key[0], key[1])]
    }
    if incomplete_claim_draws:
        first = next(iter(sorted(incomplete_claim_draws.items())))
        raise ValueError(
            "claim IDs changed across paired draws; "
            f"{first[0]} appears in {first[1]} draws"
        )
    discordant_a = 0
    discordant_b = 0
    collapsed_ties = 0
    per_claim = []
    for (suite, case, claim_id), pairs in sorted(claim_draws.items()):
        draw_count = len(pairs)
        a_votes = sum(left for left, _ in pairs)
        b_votes = sum(right for _, right in pairs)
        a_binary = (
            True if a_votes * 2 > draw_count
            else False if a_votes * 2 < draw_count
            else None
        )
        b_binary = (
            True if b_votes * 2 > draw_count
            else False if b_votes * 2 < draw_count
            else None
        )
        if a_binary is None or b_binary is None:
            collapsed_ties += 1
        elif a_binary and not b_binary:
            discordant_a += 1
        elif not a_binary and b_binary:
            discordant_b += 1
        per_claim.append({
            "suite": suite,
            "case_id": case,
            "claim_id": claim_id,
            "draws": draw_count,
            "a_pass_votes": a_votes,
            "b_pass_votes": b_votes,
            "collapsed_a_pass": a_binary,
            "collapsed_b_pass": b_binary,
        })
    result = exact_one_sided_mcnemar(discordant_a, discordant_b, alternative)
    result.update({
        "claim_clusters": len(per_claim),
        "draws_per_claim": sorted({item["draws"] for item in per_claim}),
        "collapsed_ties_excluded": collapsed_ties,
        "pairing_unit": "suite + case_id + claim_id",
        "per_claim": per_claim,
    })
    return result


def summarize_pair(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
    *,
    iterations: int,
    seed: int,
    non_inferiority_margin_claims: float | None,
    claim_mcnemar_alternative: str | None,
) -> dict[str, Any]:
    clusters = _case_clusters(records, arm_a, arm_b)
    if not clusters:
        raise ValueError(f"no paired cases for {arm_a} vs {arm_b}")
    wins = sum(item["mean_claim_difference"] > 0 for item in clusters)
    losses = sum(item["mean_claim_difference"] < 0 for item in clusters)
    ties = len(clusters) - wins - losses
    discordant_a = sum(
        item["case_pass_a"] is True and item["case_pass_b"] is False
        for item in clusters
    )
    discordant_b = sum(
        item["case_pass_a"] is False and item["case_pass_b"] is True
        for item in clusters
    )
    rng = random.Random(f"{seed}:{arm_a}:{arm_b}:{len(clusters)}")
    claim_differences = []
    rate_differences = []
    character_differences: dict[str, list[float]] = {
        field: [] for field in RESPONSE_CHARACTER_FIELDS
    }
    for _ in range(iterations):
        sampled = [clusters[rng.randrange(len(clusters))] for _ in clusters]
        claim_difference = sum(
            item["mean_claim_difference"]
            for item in sampled
        )
        claim_total = sum(item["claims_total"] for item in sampled)
        claim_differences.append(claim_difference)
        rate_differences.append(claim_difference / max(1.0, claim_total))
        for field in RESPONSE_CHARACTER_FIELDS:
            character_differences[field].append(
                sum(
                    item["response_character_metrics"][field][
                        "mean_difference"
                    ]
                    for item in sampled
                )
                / len(sampled)
            )
    total_claims = sum(item["claims_total"] for item in clusters)
    point_claim_difference = sum(
        item["mean_claim_difference"]
        for item in clusters
    )
    claim_ci = confidence_interval(claim_differences)
    response_character_metrics = {}
    for field in RESPONSE_CHARACTER_FIELDS:
        response_character_metrics[field] = {
            "mean_a": round(
                sum(
                    item["response_character_metrics"][field]["mean_a"]
                    for item in clusters
                )
                / len(clusters),
                3,
            ),
            "mean_b": round(
                sum(
                    item["response_character_metrics"][field]["mean_b"]
                    for item in clusters
                )
                / len(clusters),
                3,
            ),
            "mean_difference": round(
                sum(
                    item["response_character_metrics"][field][
                        "mean_difference"
                    ]
                    for item in clusters
                )
                / len(clusters),
                3,
            ),
            "bootstrap_95_ci_difference": confidence_interval(
                character_differences[field]
            ),
        }
    result = {
        "arm_a": arm_a,
        "arm_b": arm_b,
        "condition_a": arm_a,
        "condition_b": arm_b,
        "case_clusters": len(clusters),
        "draws_per_case": sorted({item["draws"] for item in clusters}),
        "total_claims_per_draw": total_claims,
        "case_claim_outcomes": {
            "a_wins": wins,
            "b_wins": losses,
            "ties": ties,
        },
        "paired_draw_instances": _paired_draw_instance_summary(
            records,
            arm_a,
            arm_b,
        ),
        "exact_mcnemar": exact_mcnemar(discordant_a, discordant_b),
        "corpus_total_claim_difference": {
            "point": round(point_claim_difference, 6),
            "rate": round(point_claim_difference / max(1, total_claims), 8),
            "bootstrap_95_ci_claims": claim_ci,
            "bootstrap_95_ci_rate": confidence_interval(rate_differences),
            "method": (
                "Cases are resampled with replacement. Repeated draws are averaged "
                "within each case before resampling, preserving the case cluster."
            ),
        },
        "response_character_metrics": response_character_metrics,
        "model_visible_tool_output_chars": response_character_metrics[
            "model_visible_tool_output_chars"
        ],
        "forbidden_hits": _paired_forbidden_hit_summary(
            records,
            arm_a,
            arm_b,
        ),
        "persisted_checkpoint_rate": {
            "condition_a": round(
                sum(item["persisted_checkpoint_rate_a"] for item in clusters)
                / len(clusters),
                6,
            ),
            "condition_b": round(
                sum(item["persisted_checkpoint_rate_b"] for item in clusters)
                / len(clusters),
                6,
            ),
        },
        "per_case": clusters,
    }
    if claim_mcnemar_alternative is not None:
        result["claim_level_exact_mcnemar"] = _claim_level_mcnemar(
            records,
            arm_a,
            arm_b,
            claim_mcnemar_alternative,
        )
    if non_inferiority_margin_claims is not None:
        result["non_inferiority"] = {
            "margin_claims": -abs(non_inferiority_margin_claims),
            "margin_rate": round(
                -abs(non_inferiority_margin_claims) / max(1, total_claims),
                8,
            ),
            "bootstrap_lower_bound_claims": claim_ci["lower"],
            "declared": (
                claim_ci["lower"] > -abs(non_inferiority_margin_claims)
            ),
            "basis": (
                "Declared only from the clustered corpus-total bootstrap; "
                "McNemar p-values are not used as proof of non-inferiority."
            ),
        }
    return result


def _claim_instances(
    records: Sequence[dict[str, Any]],
) -> list[tuple[bool, tuple[str, ...]]]:
    instances = []
    for record in records:
        claim_outcomes = record.get("claim_outcomes")
        claim_tags = record.get("claim_tags")
        if not isinstance(claim_outcomes, dict) or not isinstance(claim_tags, dict):
            continue
        for claim_id, passed in claim_outcomes.items():
            instances.append((passed, tuple(claim_tags[claim_id])))
    return instances


def _claim_rate(
    instances: Sequence[tuple[bool, tuple[str, ...]]],
) -> dict[str, int | float | None]:
    passed = sum(outcome for outcome, _ in instances)
    return {
        "passed": passed,
        "total": len(instances),
        "rate": (
            round(passed / len(instances), 8)
            if instances
            else None
        ),
    }


def _summarize_claim_tags(
    records: Sequence[dict[str, Any]],
    *,
    tags: Sequence[str] | None = None,
) -> dict[str, Any]:
    instances = _claim_instances(records)
    observed_tags = sorted({
        tag
        for _, claim_tags in instances
        for tag in claim_tags
    })
    reported_tags = list(tags) if tags is not None else observed_tags
    return {
        "all_claims": _claim_rate(instances),
        "without_any_tag": _claim_rate([
            instance for instance in instances if not instance[1]
        ]),
        "by_tag": {
            tag: {
                "tagged": _claim_rate([
                    instance
                    for instance in instances
                    if tag in instance[1]
                ]),
                "untagged": _claim_rate([
                    instance
                    for instance in instances
                    if tag not in instance[1]
                ]),
            }
            for tag in reported_tags
        },
    }


def _claim_tag_summary(
    records: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    arms = sorted({record["arm"] for record in records})
    suites = sorted({record["suite"] for record in records})
    tags = sorted({
        tag
        for _, claim_tags in _claim_instances(records)
        for tag in claim_tags
    })
    return {
        "unit": "claim_instance",
        "tags": tags,
        "by_arm": {
            arm: {
                "overall": _summarize_claim_tags([
                    record for record in records if record["arm"] == arm
                ], tags=tags),
                "by_suite": {
                    suite: _summarize_claim_tags([
                        record
                        for record in records
                        if record["arm"] == arm and record["suite"] == suite
                    ], tags=tags)
                    for suite in suites
                    if any(
                        record["arm"] == arm and record["suite"] == suite
                        for record in records
                    )
                },
            }
            for arm in arms
        },
    }


def _required_strata_evidence(
    records: Sequence[dict[str, Any]],
    *,
    required_feature_families: Sequence[str],
    required_claim_tags: Sequence[str],
) -> dict[str, Any]:
    families = list(dict.fromkeys(required_feature_families))
    tags = list(dict.fromkeys(required_claim_tags))
    if len(families) != len(required_feature_families):
        raise ValueError("--require-feature-family values must be unique")
    if len(tags) != len(required_claim_tags):
        raise ValueError("--require-claim-tag values must be unique")
    if any(
        not isinstance(value, str)
        or not EXPERIMENT_ID_PATTERN.fullmatch(value)
        for value in families + tags
    ):
        raise ValueError("required stratum identifiers are malformed")
    arms = sorted({record["arm"] for record in records})
    family_evidence: dict[str, Any] = {}
    for family in families:
        by_arm = {}
        for arm in arms:
            arm_records = [
                record for record in records if record["arm"] == arm
            ]
            family_count = sum(
                family in record["feature_families"]
                for record in arm_records
            )
            complement_count = len(arm_records) - family_count
            by_arm[arm] = {
                "family_case_instances": family_count,
                "complement_case_instances": complement_count,
            }
            if family_count == 0 or complement_count == 0:
                raise ValueError(
                    f"required feature family {family!r} lacks a nonempty "
                    f"family or complement population in arm {arm!r}"
                )
        family_evidence[family] = {"by_arm": by_arm}
    tag_evidence: dict[str, Any] = {}
    for tag in tags:
        by_arm = {}
        for arm in arms:
            instances = _claim_instances([
                record for record in records if record["arm"] == arm
            ])
            tagged = sum(tag in claim_tags for _, claim_tags in instances)
            untagged = len(instances) - tagged
            by_arm[arm] = {
                "tagged_claim_instances": tagged,
                "untagged_claim_instances": untagged,
            }
            if tagged == 0 or untagged == 0:
                raise ValueError(
                    f"required claim tag {tag!r} lacks a nonempty tagged or "
                    f"untagged population in arm {arm!r}"
                )
        tag_evidence[tag] = {"by_arm": by_arm}
    return {
        "required_feature_families": family_evidence,
        "required_claim_tags": tag_evidence,
    }


def _summarize_forbidden_hits(
    records: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    instances = [
        {
            "suite": record["suite"],
            "draw": record["draw"],
            "case_id": record["case"],
            "hit": hit,
            "feature_families": list(record["feature_families"]),
        }
        for record in records
        for hit in record["forbidden_hits"]
    ]
    return {
        "case_instances": len(records),
        "case_instances_with_hits": sum(
            bool(record["forbidden_hits"]) for record in records
        ),
        "hit_instances": len(instances),
        "instances": instances,
    }


def _forbidden_hit_summary(
    records: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    arms = sorted({record["arm"] for record in records})
    suites = sorted({record["suite"] for record in records})
    family_ids = sorted({
        family
        for record in records
        for family in record["feature_families"]
    })
    by_arm = {}
    for arm in arms:
        arm_records = [record for record in records if record["arm"] == arm]
        arm_summary = {
            "overall": _summarize_forbidden_hits(arm_records),
            "by_suite": {
                suite: _summarize_forbidden_hits([
                    record
                    for record in arm_records
                    if record["suite"] == suite
                ])
                for suite in suites
                if any(record["suite"] == suite for record in arm_records)
            },
            "by_feature_family": {},
        }
        for family_id in family_ids:
            family_records = [
                record
                for record in arm_records
                if family_id in record["feature_families"]
            ]
            if not family_records:
                continue
            complement_records = [
                record
                for record in arm_records
                if family_id not in record["feature_families"]
            ]
            family_suites = sorted({
                record["suite"] for record in family_records
            })
            arm_summary["by_feature_family"][family_id] = {
                "family": _summarize_forbidden_hits(family_records),
                "complement": _summarize_forbidden_hits(complement_records),
                "by_suite": {
                    suite: {
                        "family": _summarize_forbidden_hits([
                            record
                            for record in arm_records
                            if (
                                record["suite"] == suite
                                and family_id in record["feature_families"]
                            )
                        ]),
                        "complement": _summarize_forbidden_hits([
                            record
                            for record in arm_records
                            if (
                                record["suite"] == suite
                                and family_id not in record["feature_families"]
                            )
                        ]),
                    }
                    for suite in family_suites
                },
            }
        by_arm[arm] = arm_summary
    return {
        "unit": "assertion_hit_instance",
        "by_arm": by_arm,
    }


def _feature_family_pairings(
    records: Sequence[dict[str, Any]],
    arm_a: str,
    arm_b: str,
    *,
    iterations: int,
    seed: int,
    claim_mcnemar_alternative: str | None,
) -> dict[str, Any]:
    family_ids = sorted({
        family
        for record in records
        for family in record["feature_families"]
    })

    def maybe_summarize(
        selected: Sequence[dict[str, Any]],
    ) -> dict[str, Any] | None:
        if not _case_clusters(selected, arm_a, arm_b):
            return None
        return summarize_pair(
            selected,
            arm_a,
            arm_b,
            iterations=iterations,
            seed=seed,
            non_inferiority_margin_claims=None,
            claim_mcnemar_alternative=claim_mcnemar_alternative,
        )

    output = {}
    for family_id in family_ids:
        family_records = [
            record
            for record in records
            if family_id in record["feature_families"]
        ]
        if not family_records:
            continue
        complement_records = [
            record
            for record in records
            if family_id not in record["feature_families"]
        ]
        family_suites = sorted({
            record["suite"] for record in family_records
        })
        output[family_id] = {
            "family": maybe_summarize(family_records),
            "complement": maybe_summarize(complement_records),
            "by_suite": {
                suite: {
                    "family": maybe_summarize([
                        record
                        for record in family_records
                        if record["suite"] == suite
                    ]),
                    "complement": maybe_summarize([
                        record
                        for record in complement_records
                        if record["suite"] == suite
                    ]),
                }
                for suite in family_suites
            },
        }
    return output


def load_case_extension_plan(
    path: Path,
    expected_sha256: str,
) -> dict[str, Any]:
    if not SHA256_PATTERN.fullmatch(expected_sha256):
        raise ValueError("case-extension plan SHA-256 is malformed")
    try:
        body = path.read_bytes()
        payload = json.loads(body)
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(
            f"could not load case-extension plan {path}: {error}"
        ) from error
    actual_sha256 = hashlib.sha256(body).hexdigest()
    if actual_sha256 != expected_sha256:
        raise ValueError(
            "case-extension plan SHA-256 does not match the declared hash"
        )
    if (
        not isinstance(payload, dict)
        or payload.get("schema") != CASE_EXTENSION_PLAN_SCHEMA
        or not isinstance(payload.get("experiment_id"), str)
        or not EXPERIMENT_ID_PATTERN.fullmatch(payload["experiment_id"])
        or not isinstance(payload.get("suites"), list)
        or not payload["suites"]
    ):
        raise ValueError("case-extension plan is malformed")
    normalized_suites: dict[str, dict[str, Any]] = {}
    for raw in payload["suites"]:
        if not isinstance(raw, dict):
            raise ValueError("case-extension plan suite entry is malformed")
        suite = raw.get("suite")
        manifest_name = raw.get("parent_manifest")
        manifest_sha256 = raw.get("parent_manifest_sha256")
        parent_case_ids = raw.get("parent_case_ids")
        extension_case_ids = raw.get("extension_case_ids")
        base_draws = raw.get("base_draws")
        extension_draws = raw.get("extension_draws")
        if (
            not isinstance(suite, str)
            or not suite
            or suite in normalized_suites
            or not isinstance(manifest_name, str)
            or not manifest_name
            or not isinstance(manifest_sha256, str)
            or not SHA256_PATTERN.fullmatch(manifest_sha256)
            or not isinstance(parent_case_ids, list)
            or not parent_case_ids
            or not all(
                isinstance(case_id, str) and case_id
                for case_id in parent_case_ids
            )
            or len(parent_case_ids) != len(set(parent_case_ids))
            or not isinstance(extension_case_ids, list)
            or not extension_case_ids
            or not all(
                isinstance(case_id, str) and case_id
                for case_id in extension_case_ids
            )
            or len(extension_case_ids) != len(set(extension_case_ids))
            or not set(extension_case_ids) < set(parent_case_ids)
            or type(base_draws) is not int
            or base_draws < 3
            or type(extension_draws) is not int
            or extension_draws < 1
        ):
            raise ValueError(
                f"case-extension plan suite {suite!r} is malformed"
            )
        manifest_path = (PROJECT_ROOT / manifest_name).resolve()
        try:
            manifest_path.relative_to(PROJECT_ROOT)
        except ValueError as error:
            raise ValueError(
                "case-extension parent manifest must remain inside the repo"
            ) from error
        try:
            manifest_body = manifest_path.read_bytes()
            manifest = json.loads(manifest_body)
        except (OSError, json.JSONDecodeError) as error:
            raise ValueError(
                f"could not load parent manifest {manifest_name}: {error}"
            ) from error
        actual_manifest_sha256 = hashlib.sha256(manifest_body).hexdigest()
        actual_case_ids = (
            [
                case.get("id")
                for case in manifest.get("cases", [])
                if isinstance(case, dict)
            ]
            if isinstance(manifest, dict)
            else []
        )
        actual_suite = (
            manifest.get("benchmark_version")
            if isinstance(manifest, dict)
            else None
        )
        if (
            actual_manifest_sha256 != manifest_sha256
            or actual_suite != suite
            or actual_case_ids != parent_case_ids
        ):
            raise ValueError(
                f"case-extension plan does not exactly bind {manifest_name}"
            )
        normalized_suites[suite] = {
            "suite": suite,
            "parent_manifest": manifest_name,
            "parent_manifest_sha256": manifest_sha256,
            "parent_case_ids": list(parent_case_ids),
            "extension_case_ids": list(extension_case_ids),
            "base_draws": base_draws,
            "extension_draws": extension_draws,
        }
    return {
        "schema": CASE_EXTENSION_PLAN_SCHEMA,
        "experiment_id": payload["experiment_id"],
        "path": str(path.resolve()),
        "sha256": actual_sha256,
        "suites": normalized_suites,
    }


def _validate_case_extension(
    *,
    artifacts: Sequence[dict[str, Any]],
    case_set_fingerprints: Mapping[str, set[tuple[str, ...]]],
    cases_by_suite_draw: Mapping[tuple[str, str], set[str]],
    plan: dict[str, Any] | None,
) -> dict[str, Any]:
    mixed_suites = {
        suite
        for suite, case_sets in case_set_fingerprints.items()
        if len(case_sets) != 1
    }
    if mixed_suites and plan is None:
        raise ValueError(
            "paired draws use mixed case sets within a suite; provide a "
            "hash-bound --case-extension-plan and "
            "--case-extension-plan-sha256"
        )
    if plan is None:
        return {
            "enabled": False,
            "plan": None,
            "case_sets_by_suite": {
                suite: [list(case_set) for case_set in sorted(case_sets)]
                for suite, case_sets in sorted(case_set_fingerprints.items())
            },
        }
    plan_suites = plan["suites"]
    if set(plan_suites) != mixed_suites:
        raise ValueError(
            "case-extension plan suites do not exactly match the aggregate "
            "suites with extended case sets"
        )
    observations = {}
    for suite, specification in sorted(plan_suites.items()):
        manifest_hashes = {
            artifact["execution_manifest_sha256"]
            for artifact in artifacts
            if artifact["suite"] == suite
        }
        if manifest_hashes != {specification["parent_manifest_sha256"]}:
            raise ValueError(
                f"case-extension suite {suite} is not parent-manifest bound"
            )
        parent_set = tuple(sorted(specification["parent_case_ids"]))
        extension_set = tuple(sorted(specification["extension_case_ids"]))
        observed_sets = case_set_fingerprints[suite]
        if observed_sets != {parent_set, extension_set}:
            raise ValueError(
                f"case-extension suite {suite} has undeclared case sets"
            )
        base_draw_ids = sorted(
            draw
            for (draw_suite, draw), cases in cases_by_suite_draw.items()
            if draw_suite == suite and tuple(sorted(cases)) == parent_set
        )
        extension_draw_ids = sorted(
            draw
            for (draw_suite, draw), cases in cases_by_suite_draw.items()
            if draw_suite == suite and tuple(sorted(cases)) == extension_set
        )
        if (
            len(base_draw_ids) != specification["base_draws"]
            or len(extension_draw_ids) != specification["extension_draws"]
        ):
            raise ValueError(
                f"case-extension suite {suite} draw counts do not match the "
                "declared base/extension plan"
            )
        observations[suite] = {
            **specification,
            "base_draw_ids": base_draw_ids,
            "extension_draw_ids": extension_draw_ids,
        }
    return {
        "enabled": True,
        "plan": {
            "schema": plan["schema"],
            "experiment_id": plan["experiment_id"],
            "path": plan["path"],
            "sha256": plan["sha256"],
        },
        "suites": observations,
        "case_sets_by_suite": {
            suite: [list(case_set) for case_set in sorted(case_sets)]
            for suite, case_sets in sorted(case_set_fingerprints.items())
        },
    }


def aggregate(
    paths: Sequence[Path],
    *,
    iterations: int = DEFAULT_ITERATIONS,
    seed: int = DEFAULT_SEED,
    non_inferiority_margin_claims: float = DEFAULT_NON_INFERIORITY_MARGIN_CLAIMS,
    expected_arms: Sequence[str] | None = None,
    expected_arm_retrieval_modes: (
        Sequence[str] | Mapping[str, Sequence[str]] | None
    ) = None,
    claim_mcnemar_alternative: str | None = None,
    case_extension_plan: Path | None = None,
    case_extension_plan_sha256: str | None = None,
    required_feature_families: Sequence[str] = (),
    required_claim_tags: Sequence[str] = (),
    e09_step_policy: Path | None = None,
    e09_step_policy_sha256: str | None = None,
    definitive: bool = True,
) -> dict[str, Any]:
    if iterations < DEFAULT_ITERATIONS:
        raise ValueError(
            f"paired aggregates require at least {DEFAULT_ITERATIONS:,} bootstrap iterations"
        )
    if len(paths) < 1:
        raise ValueError("at least one result artifact is required")
    if bool(case_extension_plan) != bool(case_extension_plan_sha256):
        raise ValueError(
            "--case-extension-plan and --case-extension-plan-sha256 must be "
            "supplied together"
        )
    extension_plan = (
        load_case_extension_plan(
            case_extension_plan,
            str(case_extension_plan_sha256),
        )
        if case_extension_plan is not None
        else None
    )
    artifacts = []
    records = []
    for path in paths:
        artifact, draw_records = load_draw(path, definitive=definitive)
        artifacts.append(artifact)
        records.extend(draw_records)
    service_provenance = validate_service_provenance_pairing(
        artifacts,
        expected_arm_retrieval_modes,
        definitive=definitive,
    )
    if bool(e09_step_policy) != bool(e09_step_policy_sha256):
        raise ValueError(
            "--e09-step-policy and --e09-step-policy-sha256 must be supplied "
            "together"
        )
    source_revisions = {
        artifact["source_revision"] for artifact in artifacts
    }
    step_authorization: dict[str, Any] | None = None
    observed_arms = {record["arm"] for record in records}
    if STEP_EXPERIMENT_ARM in observed_arms and e09_step_policy is None:
        raise ValueError(
            "deadline-cache-600 inputs require a checked step-policy artifact"
        )
    if e09_step_policy is not None:
        if len(source_revisions) != 1:
            raise ValueError(
                "step aggregation inputs span multiple source revisions"
            )
        required_step_arms = {
            BASE_EXPERIMENT_ARM,
            STEP_EXPERIMENT_ARM,
        }
        requested_step_arms = set(expected_arms or ())
        if requested_step_arms != required_step_arms:
            raise ValueError(
                "step aggregation requires exactly the 300ms and 600ms "
                "deadline-cache expected arms"
            )
        step_authorization = load_step_authorization(
            e09_step_policy,
            str(e09_step_policy_sha256),
            expected_source_revision=next(iter(source_revisions)),
        )
        authorized_cases = set(
            step_authorization["selected_case_ids"]
        )
        authorized_manifest_sha256 = step_authorization["manifest_sha256"]
        if any(
            artifact.get("manifest_sha256")
            != authorized_manifest_sha256
            for artifact in artifacts
        ):
            raise ValueError(
                "step aggregation includes an unauthorized suite manifest"
            )

        def stable_binding(value: Any) -> dict[str, Any] | None:
            if not isinstance(value, dict):
                return None
            return {
                key: item
                for key, item in value.items()
                if key != "artifact"
            }

        expected_binding = stable_binding(step_authorization)
        for artifact in artifacts:
            if STEP_EXPERIMENT_ARM not in artifact["arms"]:
                continue
            actual_binding = stable_binding(
                artifact.get("experiment_parameters", {}).get(
                    "e09_step_authorization"
                )
            )
            if actual_binding != expected_binding:
                raise ValueError(
                    "600ms run is not bound to the checked step-policy "
                    "authorization"
                )
        records = [
            record
            for record in records
            if record["case"] in authorized_cases
        ]
        if not records:
            raise ValueError(
                "step-policy selected case set is absent from input artifacts"
            )
        observed_authorized_cases = {
            record["case"] for record in records
        }
        if observed_authorized_cases != authorized_cases:
            raise ValueError(
                "step aggregate does not contain every authorized case ID"
            )
    observation_keys = [
        (
            record["suite"],
            record["draw"],
            record["case"],
            record["arm"],
        )
        for record in records
    ]
    if len(observation_keys) != len(set(observation_keys)):
        raise ValueError("input artifacts contain duplicate draw observations")
    requested_arms = list(dict.fromkeys(expected_arms or []))
    if expected_arms and len(requested_arms) != len(expected_arms):
        raise ValueError("expected arms must be unique")
    if requested_arms and len(requested_arms) < 2:
        raise ValueError("paired aggregates require at least two expected arms")
    arm_order = requested_arms or list(
        dict.fromkeys(record["arm"] for record in records)
    )
    if len(arm_order) < 2:
        raise ValueError("input artifacts contain fewer than two experiment arms")

    arms_by_suite: dict[str, set[str]] = defaultdict(set)
    arms_by_suite_draw_case: dict[tuple[str, str, str], set[str]] = defaultdict(set)
    cases_by_suite_draw: dict[tuple[str, str], set[str]] = defaultdict(set)
    source_conditions_by_arm: dict[str, set[str]] = defaultdict(set)
    for record in records:
        arms_by_suite[record["suite"]].add(record["arm"])
        arms_by_suite_draw_case[
            (record["suite"], record["draw"], record["case"])
        ].add(record["arm"])
        cases_by_suite_draw[(record["suite"], record["draw"])].add(record["case"])
        arm_condition = (
            record["source_condition"]
            if record["identity_mode"] == "explicit_arm"
            else record["condition"]
        )
        source_conditions_by_arm[record["arm"]].add(arm_condition)
    if requested_arms:
        expected_set = set(requested_arms)
        mismatched_suites = {
            suite: sorted(arms)
            for suite, arms in arms_by_suite.items()
            if arms != expected_set
        }
        if mismatched_suites:
            raise ValueError(
                "input suites do not match --expected-arm set: "
                f"{mismatched_suites}"
            )
    incomplete_arm_sets = {
        key: sorted(arms)
        for key, arms in arms_by_suite_draw_case.items()
        if arms != arms_by_suite[key[0]]
    }
    if incomplete_arm_sets:
        first = next(iter(sorted(incomplete_arm_sets.items())))
        raise ValueError(
            "input artifacts contain an incomplete or mixed arm set; "
            f"{first[0]} has {first[1]}, expected "
            f"{sorted(arms_by_suite[first[0][0]])}"
        )
    mixed_arm_conditions = {
        arm: sorted(conditions)
        for arm, conditions in source_conditions_by_arm.items()
        if len(conditions) != 1
    }
    if mixed_arm_conditions:
        raise ValueError(
            "experiment arms change effective condition across draws: "
            f"{mixed_arm_conditions}"
        )
    identity_modes_by_suite: dict[str, set[str]] = defaultdict(set)
    for artifact in artifacts:
        identity_modes_by_suite[artifact["suite"]].add(artifact["identity_mode"])
    mixed_identity_modes = {
        suite: sorted(modes)
        for suite, modes in identity_modes_by_suite.items()
        if len(modes) != 1
    }
    if mixed_identity_modes:
        raise ValueError(
            "input suites mix explicit-arm and condition-arm identities: "
            f"{mixed_identity_modes}"
        )
    case_set_fingerprints: dict[str, set[tuple[str, ...]]] = defaultdict(set)
    for (suite, _draw), cases in cases_by_suite_draw.items():
        case_set_fingerprints[suite].add(tuple(sorted(cases)))
    case_extension = _validate_case_extension(
        artifacts=artifacts,
        case_set_fingerprints=case_set_fingerprints,
        cases_by_suite_draw=cases_by_suite_draw,
        plan=extension_plan,
    )

    case_arm_draws: dict[tuple[str, str, str], set[str]] = defaultdict(set)
    for record in records:
        case_arm_draws[
            (record["suite"], record["case"], record["arm"])
        ].add(record["draw"])
    insufficient_draws = {
        key: len(draws)
        for key, draws in case_arm_draws.items()
        if len(draws) < 3
    }
    if insufficient_draws:
        first = next(iter(sorted(insufficient_draws.items())))
        raise ValueError(
            "paired aggregates require at least 3 complete draws per "
            f"case and arm; {first[0]} has {first[1]}"
        )
    revisions = {artifact["source_revision"] for artifact in artifacts}
    if len(revisions) != 1:
        raise ValueError(f"draws span multiple source revisions: {sorted(revisions)}")
    grading_revisions = {artifact["grading_revision"] for artifact in artifacts}
    if len(grading_revisions) != 1:
        raise ValueError(
            "draws span multiple grader revisions: "
            f"{sorted(grading_revisions)}"
        )
    suite_manifest_hashes: dict[str, set[Any]] = defaultdict(set)
    suite_models: dict[str, set[Any]] = defaultdict(set)
    for artifact in artifacts:
        suite_manifest_hashes[artifact["suite"]].add(artifact["manifest_sha256"])
        suite_models[artifact["suite"]].add(artifact["model"])
    if any(len(values) != 1 for values in suite_manifest_hashes.values()):
        raise ValueError("draws for a suite do not share one manifest fingerprint")
    if any(len(values) != 1 for values in suite_models.values()):
        raise ValueError("draws for a suite do not share one model")
    source_feature_families_by_suite: dict[str, set[str]] = defaultdict(set)
    for artifact in artifacts:
        source_feature_families_by_suite[artifact["suite"]].add(
            json.dumps(
                artifact["source_feature_families"],
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
        )
    unstable_feature_families = {
        suite: len(values)
        for suite, values in source_feature_families_by_suite.items()
        if len(values) != 1
    }
    if unstable_feature_families:
        raise ValueError(
            "manifest feature_families changed within a suite: "
            f"{unstable_feature_families}"
        )
    runtime_build_revisions = {
        snapshot["build_revision"]
        for artifact in artifacts
        if isinstance(
            (snapshot := artifact.get("service_runtime_snapshot")),
            dict,
        )
    }
    if len(runtime_build_revisions) > 1:
        raise ValueError(
            "service-backed draws span multiple runtime build revisions: "
            f"{sorted(runtime_build_revisions)}"
        )
    runtime_features_by_arm: dict[str, set[str]] = defaultdict(set)
    for artifact in artifacts:
        snapshot = artifact.get("service_runtime_snapshot")
        if not isinstance(snapshot, dict):
            continue
        if artifact["identity_mode"] == "explicit_arm":
            runtime_arms = artifact["arms"]
        else:
            runtime_arms = [
                arm
                for arm in artifact["arms"]
                if arm == "service_api"
            ]
        rendered_features = json.dumps(
            snapshot["runtime_features"],
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        for arm in runtime_arms:
            runtime_features_by_arm[arm].add(rendered_features)
    mixed_runtime_features = {
        arm: len(features)
        for arm, features in runtime_features_by_arm.items()
        if len(features) != 1
    }
    if mixed_runtime_features:
        raise ValueError(
            "runtime feature snapshot changed within an experiment arm: "
            f"{mixed_runtime_features}"
        )
    required_strata = _required_strata_evidence(
        records,
        required_feature_families=required_feature_families,
        required_claim_tags=required_claim_tags,
    )
    conditions = list(dict.fromkeys(record["condition"] for record in records))
    pairings: dict[str, Any] = {}
    suites = sorted({record["suite"] for record in records})
    for arm_a, arm_b in itertools.combinations(arm_order, 2):
        shared_records = [
            record
            for record in records
            if record["arm"] in {arm_a, arm_b}
        ]
        if not _case_clusters(shared_records, arm_a, arm_b):
            continue
        key = f"{arm_a}__vs__{arm_b}"
        pairings[key] = {
            "overall": summarize_pair(
                shared_records,
                arm_a,
                arm_b,
                iterations=iterations,
                seed=seed,
                non_inferiority_margin_claims=non_inferiority_margin_claims,
                claim_mcnemar_alternative=claim_mcnemar_alternative,
            ),
            "by_suite": {},
            "by_feature_family": _feature_family_pairings(
                shared_records,
                arm_a,
                arm_b,
                iterations=iterations,
                seed=seed,
                claim_mcnemar_alternative=claim_mcnemar_alternative,
            ),
        }
        for suite in suites:
            suite_records = [
                record
                for record in shared_records
                if record["suite"] == suite
            ]
            if _case_clusters(suite_records, arm_a, arm_b):
                pairings[key]["by_suite"][suite] = summarize_pair(
                    suite_records,
                    arm_a,
                    arm_b,
                    iterations=iterations,
                    seed=seed,
                    non_inferiority_margin_claims=None,
                    claim_mcnemar_alternative=claim_mcnemar_alternative,
                )
    if not pairings:
        raise ValueError("input artifacts contain no paired condition observations")
    return {
        "schema": SCHEMA,
        "definitive": definitive,
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "source_revision": next(iter(revisions)),
        "grading_revision": next(iter(grading_revisions)),
        "bootstrap": {
            "iterations": iterations,
            "seed": seed,
            "confidence": 0.95,
            "cluster": "suite plus case_id",
        },
        "input_artifacts": artifacts,
        "service_provenance": service_provenance,
        "feature_families_by_suite": {
            suite: json.loads(next(iter(values)))
            for suite, values in sorted(
                source_feature_families_by_suite.items()
            )
        },
        "claim_tags": _claim_tag_summary(records),
        "required_strata": required_strata,
        "forbidden_hits": _forbidden_hit_summary(records),
        "arms": arm_order,
        "arm_sets_by_suite": {
            suite: [arm for arm in arm_order if arm in arms]
            for suite, arms in sorted(arms_by_suite.items())
        },
        "case_extension": case_extension,
        "e09_step_authorization": step_authorization,
        "conditions": conditions,
        "suites": suites,
        "draws": len({(record["suite"], record["draw"]) for record in records}),
        "case_draw_records": len(records),
        "pairings": pairings,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Aggregate complete paired Straylight reasoning draws",
    )
    parser.add_argument("inputs", type=Path, nargs="+")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=DEFAULT_ITERATIONS)
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument(
        "--expected-arm",
        action="append",
        help=(
            "declare the complete arm set and ordering; repeat once per arm"
        ),
    )
    parser.add_argument(
        "--expected-arm-retrieval-modes",
        action="append",
        metavar="ARM=MODE,...",
        help=(
            "declare the exact retrieval modes for one service-backed arm; "
            "repeat for every definitive service arm"
        ),
    )
    parser.add_argument(
        "--claim-mcnemar-alternative",
        choices=("a_greater", "b_greater"),
        help=(
            "also emit a one-sided claim-level McNemar test after strict-majority "
            "collapse across draws; default case-level output remains two-sided"
        ),
    )
    parser.add_argument(
        "--case-extension-plan",
        type=Path,
        help=(
            "frozen case-extension plan that declares exact parent/subset case "
            "sets and draw counts"
        ),
    )
    parser.add_argument(
        "--case-extension-plan-sha256",
        help="expected SHA-256 of --case-extension-plan",
    )
    parser.add_argument(
        "--require-feature-family",
        action="append",
        default=[],
        help=(
            "require nonempty family and complement case populations in every "
            "arm; repeat for multiple families"
        ),
    )
    parser.add_argument(
        "--require-claim-tag",
        action="append",
        default=[],
        help=(
            "require nonempty tagged and untagged claim populations in every "
            "arm; repeat for multiple tags"
        ),
    )
    parser.add_argument(
        "--e09-step-policy",
        type=Path,
        help=(
            "checked one-step authorization used only for a 300ms versus "
            "600ms E09 aggregate"
        ),
    )
    parser.add_argument(
        "--e09-step-policy-sha256",
        help="expected SHA-256 of --e09-step-policy",
    )
    parser.add_argument(
        "--non-inferiority-margin-claims",
        type=float,
        default=DEFAULT_NON_INFERIORITY_MARGIN_CLAIMS,
    )
    parser.add_argument(
        "--non-definitive",
        action="store_true",
        help=(
            "derive explicitly marked legacy character/tag/forbidden fields; "
            "never use for an acceptance decision"
        ),
    )
    return parser


def main() -> int:
    args = build_parser().parse_args()
    result = aggregate(
        args.inputs,
        iterations=args.iterations,
        seed=args.seed,
        non_inferiority_margin_claims=args.non_inferiority_margin_claims,
        expected_arms=args.expected_arm,
        expected_arm_retrieval_modes=args.expected_arm_retrieval_modes,
        claim_mcnemar_alternative=args.claim_mcnemar_alternative,
        case_extension_plan=args.case_extension_plan,
        case_extension_plan_sha256=args.case_extension_plan_sha256,
        required_feature_families=args.require_feature_family,
        required_claim_tags=args.require_claim_tag,
        e09_step_policy=args.e09_step_policy,
        e09_step_policy_sha256=args.e09_step_policy_sha256,
        definitive=not args.non_definitive,
    )
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": "ok",
        "out": str(args.out),
        "source_revision": result["source_revision"],
        "pairings": sorted(result["pairings"]),
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

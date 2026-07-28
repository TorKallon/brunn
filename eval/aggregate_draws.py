#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import math
import random
import re
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable, Sequence


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


def load_draw(path: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
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
        characters = record.get("model_visible_tool_output_chars")
        if type(characters) is not int or characters < 0:
            raise ValueError(
                f"{path}: {key} lacks comparable model-visible character accounting"
            )
        canonical = CANONICAL_CONDITIONS.get(key[1], key[1])
        claim_rows = grade.get("claims")
        claim_outcomes = None
        if isinstance(claim_rows, list):
            claim_outcomes = {}
            for claim in claim_rows:
                if (
                    not isinstance(claim, dict)
                    or not isinstance(claim.get("id"), str)
                    or not isinstance(claim.get("pass"), bool)
                    or claim["id"] in claim_outcomes
                ):
                    raise ValueError(
                        f"{path}: {key} has malformed claim-level outcomes"
                    )
                claim_outcomes[claim["id"]] = claim["pass"]
            if len(claim_outcomes) != grade["claims_total"]:
                raise ValueError(
                    f"{path}: {key} claim-level outcomes do not match claims_total"
                )
        normalized.append({
            "suite": suite,
            "draw": paired_draw_id if explicit_identity else run_id,
            "case": key[0],
            "case_key": f"{suite}:{key[0]}",
            "arm": experiment_arm if explicit_identity else canonical,
            "condition": canonical,
            "source_condition": key[1],
            "claims_passed": grade["claims_passed"],
            "claims_total": grade["claims_total"],
            "case_pass": case_pass,
            "model_visible_tool_output_chars": characters,
            "persisted_checkpoint": bool(record.get("persisted_checkpoint")),
            "claim_outcomes": claim_outcomes,
        })
    missing = sorted(expected - observed)
    if missing:
        raise ValueError(f"{path}: incomplete draw; missing {missing[:5]}")
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
        "source_revision": revision,
        "grading_revision": grading_revision,
        "manifest_sha256": run.get("manifest_sha256"),
        "model": manifest.get("model"),
        "expected_runtime_features": run.get("expected_runtime_features", {}),
        "expected_build_revision": run.get("expected_build_revision"),
        "experiment_parameters": run.get("experiment_parameters", {}),
        "service_runtime_snapshot": run.get("service_runtime_snapshot"),
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
        mean_chars_a = sum(
            left["model_visible_tool_output_chars"]
            for left, _ in pairs
        ) / draw_count
        mean_chars_b = sum(
            right["model_visible_tool_output_chars"]
            for _, right in pairs
        ) / draw_count
        clusters.append({
            "case_key": case_key,
            "suite": pairs[0][0]["suite"],
            "case_id": pairs[0][0]["case"],
            "draws": draw_count,
            "claims_total": claims_total,
            "mean_claims_a": mean_a,
            "mean_claims_b": mean_b,
            "mean_claim_difference": mean_a - mean_b,
            "mean_chars_a": mean_chars_a,
            "mean_chars_b": mean_chars_b,
            "mean_char_difference": mean_chars_a - mean_chars_b,
            "case_pass_a": a_binary,
            "case_pass_b": b_binary,
            "persisted_checkpoint_rate_a": sum(
                left["persisted_checkpoint"] for left, _ in pairs
            ) / draw_count,
            "persisted_checkpoint_rate_b": sum(
                right["persisted_checkpoint"] for _, right in pairs
            ) / draw_count,
        })
    return clusters


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
    char_differences = []
    for _ in range(iterations):
        sampled = [clusters[rng.randrange(len(clusters))] for _ in clusters]
        claim_difference = sum(
            item["mean_claim_difference"]
            for item in sampled
        )
        claim_total = sum(item["claims_total"] for item in sampled)
        claim_differences.append(claim_difference)
        rate_differences.append(claim_difference / max(1.0, claim_total))
        char_differences.append(
            sum(item["mean_char_difference"] for item in sampled)
            / len(sampled)
        )
    total_claims = sum(item["claims_total"] for item in clusters)
    point_claim_difference = sum(
        item["mean_claim_difference"]
        for item in clusters
    )
    claim_ci = confidence_interval(claim_differences)
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
        "model_visible_tool_output_chars": {
            "mean_a": round(
                sum(item["mean_chars_a"] for item in clusters) / len(clusters),
                3,
            ),
            "mean_b": round(
                sum(item["mean_chars_b"] for item in clusters) / len(clusters),
                3,
            ),
            "mean_difference": round(
                sum(item["mean_char_difference"] for item in clusters)
                / len(clusters),
                3,
            ),
            "bootstrap_95_ci_difference": confidence_interval(char_differences),
        },
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


def aggregate(
    paths: Sequence[Path],
    *,
    iterations: int = DEFAULT_ITERATIONS,
    seed: int = DEFAULT_SEED,
    non_inferiority_margin_claims: float = DEFAULT_NON_INFERIORITY_MARGIN_CLAIMS,
    expected_arms: Sequence[str] | None = None,
    claim_mcnemar_alternative: str | None = None,
    allow_case_extension: bool = False,
) -> dict[str, Any]:
    if iterations < DEFAULT_ITERATIONS:
        raise ValueError(
            f"paired aggregates require at least {DEFAULT_ITERATIONS:,} bootstrap iterations"
        )
    if len(paths) < 1:
        raise ValueError("at least one result artifact is required")
    artifacts = []
    records = []
    for path in paths:
        artifact, draw_records = load_draw(path)
        artifacts.append(artifact)
        records.extend(draw_records)
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
        source_conditions_by_arm[record["arm"]].add(record["source_condition"])
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
            "experiment arms change source condition across draws: "
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
    mixed_case_sets = {
        suite: sorted(case_sets)
        for suite, case_sets in case_set_fingerprints.items()
        if len(case_sets) != 1
    }
    if mixed_case_sets and not allow_case_extension:
        raise ValueError(
            "paired draws use mixed case sets within a suite; use a frozen "
            "subset selection and --allow-case-extension only for a predeclared "
            f"longitudinal extension: {mixed_case_sets}"
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
        "arms": arm_order,
        "arm_sets_by_suite": {
            suite: [arm for arm in arm_order if arm in arms]
            for suite, arms in sorted(arms_by_suite.items())
        },
        "case_extension": {
            "enabled": allow_case_extension,
            "case_sets_by_suite": {
                suite: [
                    list(case_set)
                    for case_set in sorted(case_sets)
                ]
                for suite, case_sets in sorted(case_set_fingerprints.items())
            },
        },
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
        "--claim-mcnemar-alternative",
        choices=("a_greater", "b_greater"),
        help=(
            "also emit a one-sided claim-level McNemar test after strict-majority "
            "collapse across draws; default case-level output remains two-sided"
        ),
    )
    parser.add_argument(
        "--allow-case-extension",
        action="store_true",
        help=(
            "allow a predeclared subset to receive additional complete paired "
            "draws; every case still requires at least three arm-complete draws"
        ),
    )
    parser.add_argument(
        "--non-inferiority-margin-claims",
        type=float,
        default=DEFAULT_NON_INFERIORITY_MARGIN_CLAIMS,
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
        claim_mcnemar_alternative=args.claim_mcnemar_alternative,
        allow_case_extension=args.allow_case_extension,
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

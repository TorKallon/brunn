#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import math
import random
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
    ):
        raise ValueError("input run ledger does not match the run configuration")
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
        if (
            record.get("error")
            or not isinstance(grade, dict)
            or not isinstance(grade.get("claims_passed"), int)
            or not isinstance(grade.get("claims_total"), int)
            or grade["claims_total"] <= 0
            or not 0 <= grade["claims_passed"] <= grade["claims_total"]
        ):
            raise ValueError(f"{path}: incomplete or failed record for {key}")
        characters = record.get("model_visible_tool_output_chars")
        if not isinstance(characters, int) or characters < 0:
            raise ValueError(
                f"{path}: {key} lacks comparable model-visible character accounting"
            )
        canonical = CANONICAL_CONDITIONS.get(key[1], key[1])
        normalized.append({
            "suite": suite,
            "draw": run_id,
            "case": key[0],
            "case_key": f"{suite}:{key[0]}",
            "condition": canonical,
            "source_condition": key[1],
            "claims_passed": grade["claims_passed"],
            "claims_total": grade["claims_total"],
            "case_pass": bool(
                record.get("transition_pass")
                if "transition_pass" in record
                else grade.get("pass")
            ),
            "model_visible_tool_output_chars": characters,
            "persisted_checkpoint": bool(record.get("persisted_checkpoint")),
        })
    missing = sorted(expected - observed)
    if missing:
        raise ValueError(f"{path}: incomplete draw; missing {missing[:5]}")
    artifact = {
        "path": str(path.resolve()),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "run_id": run_id,
        "suite": suite,
        "conditions": list(conditions),
        "cases": len(cases),
        "source_revision": revision,
        "grading_revision": grading_revision,
        "manifest_sha256": run.get("manifest_sha256"),
        "model": manifest.get("model"),
        "run_ledger": run["run_ledger"],
    }
    return artifact, normalized


def _case_clusters(
    records: Sequence[dict[str, Any]],
    condition_a: str,
    condition_b: str,
) -> list[dict[str, Any]]:
    by_observation = {
        (
            record["suite"],
            record["draw"],
            record["case"],
            record["condition"],
        ): record
        for record in records
    }
    case_draws: dict[str, list[tuple[dict[str, Any], dict[str, Any]]]] = defaultdict(list)
    for suite, draw, case, condition in sorted(by_observation):
        if condition != condition_a:
            continue
        left = by_observation[(suite, draw, case, condition_a)]
        right = by_observation.get((suite, draw, case, condition_b))
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


def summarize_pair(
    records: Sequence[dict[str, Any]],
    condition_a: str,
    condition_b: str,
    *,
    iterations: int,
    seed: int,
    non_inferiority_margin_claims: float | None,
) -> dict[str, Any]:
    clusters = _case_clusters(records, condition_a, condition_b)
    if not clusters:
        raise ValueError(f"no paired cases for {condition_a} vs {condition_b}")
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
    rng = random.Random(f"{seed}:{condition_a}:{condition_b}:{len(clusters)}")
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
        "condition_a": condition_a,
        "condition_b": condition_b,
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
            record["condition"],
        )
        for record in records
    ]
    if len(observation_keys) != len(set(observation_keys)):
        raise ValueError("input artifacts contain duplicate draw observations")
    case_condition_draws: dict[tuple[str, str, str], set[str]] = defaultdict(set)
    for record in records:
        case_condition_draws[
            (record["suite"], record["case"], record["condition"])
        ].add(record["draw"])
    insufficient_draws = {
        key: len(draws)
        for key, draws in case_condition_draws.items()
        if len(draws) < 3
    }
    if insufficient_draws:
        first = next(iter(sorted(insufficient_draws.items())))
        raise ValueError(
            "paired aggregates require at least 3 complete draws per "
            f"case and condition; {first[0]} has {first[1]}"
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
    conditions = list(dict.fromkeys(record["condition"] for record in records))
    pairings: dict[str, Any] = {}
    suites = sorted({record["suite"] for record in records})
    for condition_a, condition_b in itertools.combinations(conditions, 2):
        shared_records = [
            record
            for record in records
            if record["condition"] in {condition_a, condition_b}
        ]
        if not _case_clusters(shared_records, condition_a, condition_b):
            continue
        key = f"{condition_a}__vs__{condition_b}"
        pairings[key] = {
            "overall": summarize_pair(
                shared_records,
                condition_a,
                condition_b,
                iterations=iterations,
                seed=seed,
                non_inferiority_margin_claims=non_inferiority_margin_claims,
            ),
            "by_suite": {},
        }
        for suite in suites:
            suite_records = [
                record
                for record in shared_records
                if record["suite"] == suite
            ]
            if _case_clusters(suite_records, condition_a, condition_b):
                pairings[key]["by_suite"][suite] = summarize_pair(
                    suite_records,
                    condition_a,
                    condition_b,
                    iterations=iterations,
                    seed=seed,
                    non_inferiority_margin_claims=None,
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

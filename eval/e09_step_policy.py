#!/usr/bin/env python3

"""Fail-closed reasoning budget policy for E09 deadline stepping."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence


PROJECT_ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "brunn-e09-step-policy@v1"
DEFAULT_MANIFESTS = (
    PROJECT_ROOT / "eval" / "work_cases.json",
    PROJECT_ROOT / "eval" / "rupture_ops_cases.json",
    PROJECT_ROOT / "eval" / "recent_work_cases.json",
)
BASE_ARMS = 3
BASE_DRAWS = 3
STEP_DRAWS = 3
DEFAULT_PER_CASE_RUN_USD = 0.24
DEFAULT_CEILING_USD = 100.0
DEFAULT_MAX_STEP_CASES = 12


def canonical_json_sha256(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(
            value,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def load_manifest(path: Path) -> dict[str, Any]:
    content = path.read_bytes()
    value = json.loads(content)
    cases = value.get("cases")
    if not isinstance(cases, list):
        raise ValueError(f"{path}: cases must be a list")
    active = [
        case
        for case in cases
        if isinstance(case, dict)
        and isinstance(case.get("id"), str)
        and case.get("active", True)
    ]
    ids = [str(case["id"]) for case in active]
    if len(ids) != len(set(ids)):
        raise ValueError(f"{path}: active case IDs are not unique")
    return {
        "path": path,
        "relative_path": str(path.relative_to(PROJECT_ROOT)),
        "sha256": hashlib.sha256(content).hexdigest(),
        "suite": path.stem.removesuffix("_cases"),
        "case_ids": sorted(ids),
    }


def base_projection(
    manifests: Sequence[dict[str, Any]],
    per_case_run_usd: float,
) -> dict[str, Any]:
    cases = sum(len(manifest["case_ids"]) for manifest in manifests)
    case_runs = cases * BASE_ARMS * BASE_DRAWS
    return {
        "cases": cases,
        "arms": BASE_ARMS,
        "draws": BASE_DRAWS,
        "case_runs": case_runs,
        "subscription_equivalent_usd": round(
            case_runs * per_case_run_usd,
            2,
        ),
    }


def step_plan(
    manifests: Sequence[dict[str, Any]],
    *,
    losing_suite: str,
    actual_base_usd: float | None,
    per_case_run_usd: float,
    ceiling_usd: float,
    max_step_cases: int,
) -> dict[str, Any]:
    if not math.isfinite(per_case_run_usd) or per_case_run_usd <= 0:
        raise ValueError("per-case-run cost must be finite and positive")
    if not math.isfinite(ceiling_usd) or ceiling_usd <= 0:
        raise ValueError("ceiling must be finite and positive")
    if ceiling_usd > DEFAULT_CEILING_USD:
        raise ValueError(
            f"ceiling cannot exceed the fixed ${DEFAULT_CEILING_USD:.0f} cap"
        )
    if max_step_cases <= 0:
        raise ValueError("max-step-cases must be positive")
    if max_step_cases > DEFAULT_MAX_STEP_CASES:
        raise ValueError(
            "max-step-cases cannot exceed the fixed 12-case bound"
        )
    projection = base_projection(manifests, per_case_run_usd)
    accounted_base = (
        projection["subscription_equivalent_usd"]
        if actual_base_usd is None
        else actual_base_usd
    )
    if not math.isfinite(accounted_base) or accounted_base < 0:
        raise ValueError("actual-base-usd must be finite and nonnegative")
    matches = [
        manifest
        for manifest in manifests
        if losing_suite
        in {
            manifest["suite"],
            manifest["relative_path"],
            str(manifest["path"]),
        }
    ]
    if len(matches) != 1:
        raise ValueError(
            f"losing suite must identify exactly one manifest: {losing_suite}"
        )
    suite = matches[0]
    available_usd = max(0.0, ceiling_usd - accounted_base)
    per_case_step_usd = STEP_DRAWS * per_case_run_usd
    budget_case_limit = math.floor(
        (available_usd + 1e-9) / per_case_step_usd
    )
    selected_count = min(
        len(suite["case_ids"]),
        max_step_cases,
        budget_case_limit,
    )
    selected = suite["case_ids"][:selected_count]
    step_usd = selected_count * per_case_step_usd
    full_suite = selected_count == len(suite["case_ids"])
    if selected_count == 0:
        status = "blocked_budget"
        inferential_status = "not_run"
    elif full_suite:
        status = "approved_one_step_full_suite"
        inferential_status = "specified_suite_retest"
    else:
        status = "approved_one_step_bounded_subset"
        inferential_status = (
            "exploratory_only_owner_decision_required_after_step"
        )
    return {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "status": status,
        "pass": selected_count > 0,
        "policy": {
            "reasoning_ceiling_usd": ceiling_usd,
            "per_case_run_subscription_equivalent_usd": per_case_run_usd,
            "one_step_only": True,
            "deadline_before_ms": 300,
            "deadline_after_ms": 600,
            "automatic_1000ms_step_allowed": False,
            "max_step_cases": max_step_cases,
            "step_draws": STEP_DRAWS,
            "second_step_status": "forbidden_without_new_owner_authorization",
        },
        "base": {
            "projection": projection,
            "accounted_subscription_equivalent_usd": round(
                accounted_base,
                2,
            ),
            "accounting_source": (
                "operator_actual"
                if actual_base_usd is not None
                else "manifest_projection"
            ),
        },
        "remaining_budget_usd": round(available_usd, 2),
        "losing_suite": {
            "manifest": suite["relative_path"],
            "manifest_sha256": suite["sha256"],
            "active_cases": len(suite["case_ids"]),
        },
        "step": {
            "selected_case_ids": selected,
            "selected_cases": selected_count,
            "case_runs": selected_count * STEP_DRAWS,
            "subscription_equivalent_usd": round(step_usd, 2),
            "maximum_total_usd": round(accounted_base + step_usd, 2),
            "full_suite": full_suite,
            "inferential_status": inferential_status,
            "agent_work_eval_case_arguments": [
                argument
                for case_id in selected
                for argument in ("--case", case_id)
            ],
        },
    }


def source_fingerprint() -> dict[str, Any]:
    revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    tracked_clean = subprocess.run(
        ["git", "diff", "--quiet", "HEAD", "--"],
        cwd=PROJECT_ROOT,
        check=False,
    ).returncode == 0
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard"],
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    untracked_source = [
        path
        for path in untracked.stdout.splitlines()
        if not path.startswith(("results/", "runs/"))
    ]
    value = revision.stdout.strip() if revision.returncode == 0 else ""
    return {
        "source_revision": value or None,
        "source_clean": bool(
            value
            and tracked_clean
            and untracked.returncode == 0
            and not untracked_source
        ),
        "untracked_source_files": untracked_source,
    }


def finalize_authorization(artifact: dict[str, Any]) -> dict[str, Any]:
    fingerprint = source_fingerprint()
    artifact["implementation"] = {
        "harness": str(Path(__file__).relative_to(PROJECT_ROOT)),
        "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        **fingerprint,
    }
    if not fingerprint["source_clean"]:
        artifact["pass"] = False
        artifact["status"] = "blocked_dirty_source"
    artifact["authorization_id"] = canonical_json_sha256({
        "schema": artifact["schema"],
        "source_revision": fingerprint["source_revision"],
        "manifest": artifact["losing_suite"]["manifest"],
        "manifest_sha256": artifact["losing_suite"]["manifest_sha256"],
        "selected_case_ids": artifact["step"]["selected_case_ids"],
        "deadline_before_ms": artifact["policy"]["deadline_before_ms"],
        "deadline_after_ms": artifact["policy"]["deadline_after_ms"],
        "maximum_total_usd": artifact["step"]["maximum_total_usd"],
    })
    return artifact


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Create the bounded E09 deadline-step execution artifact"
    )
    parser.add_argument(
        "--manifest",
        action="append",
        type=Path,
        dest="manifests",
        help="E09 suite manifest; defaults to work, rupture_ops, recent_work",
    )
    parser.add_argument("--losing-suite", required=True)
    parser.add_argument("--actual-base-usd", type=float)
    parser.add_argument(
        "--per-case-run-usd",
        type=float,
        default=DEFAULT_PER_CASE_RUN_USD,
    )
    parser.add_argument("--ceiling-usd", type=float, default=DEFAULT_CEILING_USD)
    parser.add_argument(
        "--max-step-cases",
        type=int,
        default=DEFAULT_MAX_STEP_CASES,
    )
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    manifests = [
        load_manifest(path)
        for path in (args.manifests or DEFAULT_MANIFESTS)
    ]
    artifact = finalize_authorization(step_plan(
        manifests,
        losing_suite=args.losing_suite,
        actual_base_usd=args.actual_base_usd,
        per_case_run_usd=args.per_case_run_usd,
        ceiling_usd=args.ceiling_usd,
        max_step_cases=args.max_step_cases,
    ))
    try:
        output_relative = args.out.resolve().relative_to(PROJECT_ROOT)
    except ValueError as error:
        raise ValueError(
            "--out must be under the repository results directory"
        ) from error
    if not output_relative.parts or output_relative.parts[0] != "results":
        raise ValueError(
            "--out must be under the repository results directory"
        )
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("x", encoding="utf-8") as output:
        output.write(json.dumps(artifact, indent=2) + "\n")
    artifact_sha256 = hashlib.sha256(args.out.read_bytes()).hexdigest()
    print(json.dumps(artifact, indent=2))
    print(json.dumps({
        "artifact": str(args.out),
        "artifact_sha256": artifact_sha256,
        "authorization_id": artifact["authorization_id"],
    }, indent=2))
    return 0 if artifact["pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())

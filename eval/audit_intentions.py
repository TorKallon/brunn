#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import sys
import unicodedata
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Any, Sequence


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from agent_work_eval import (  # noqa: E402
    canonical_json_sha256,
    definitive_service_run_provenance,
    sha256_file,
)


DONE_FIXTURE = "Intentions/Completed renewal invoice.md"
CHAR_BUDGET = 500
POINTER_LIMIT = 5
POINTER_FIELDS = {
    "path",
    "title_line",
    "due",
    "status_note",
    "matched_terms",
}
EXPECTED_FEATURES = {
    "semantic_lane": False,
    "verbatim_spans": False,
    "supersession_demotion": False,
    "intention_ledger": True,
    "resume_deltas": False,
}
FINAL_AUDIT_MODE = "final"
DRAW1_ABORT_AUDIT_MODE = "draw1-abort"
AUDIT_MODES = {
    FINAL_AUDIT_MODE: {
        "false_surfacing_limit": 0.10,
        "false_surfacing_threshold": "<0.10",
    },
    DRAW1_ABORT_AUDIT_MODE: {
        "false_surfacing_limit": 0.25,
        "false_surfacing_threshold": "<=0.25",
    },
}


def validate_pending_intention_pointers(
    pending: list[dict[str, Any]],
    operation: dict[str, Any],
    *,
    artifact_path: str,
    case_id: str,
    operation_index: int,
) -> list[str]:
    location = (
        f"{artifact_path}: {case_id} operation {operation_index}"
    )
    if len(pending) > POINTER_LIMIT:
        raise ValueError(
            f"{location} exceeds the {POINTER_LIMIT}-pointer intention cap"
        )
    observed_date: date | None = None
    if pending:
        observed_at = operation.get("at")
        if not isinstance(observed_at, str) or not observed_at:
            raise ValueError(
                f"{location} lacks a timestamp for intention status semantics"
            )
        try:
            observed = datetime.fromisoformat(
                observed_at.replace("Z", "+00:00")
            )
        except ValueError as exc:
            raise ValueError(
                f"{location} has an invalid operation timestamp"
            ) from exc
        if observed.tzinfo is None or observed.utcoffset() is None:
            raise ValueError(
                f"{location} operation timestamp must include a timezone"
            )
        observed_date = observed.astimezone(timezone.utc).date()

    paths = []
    for pointer_index, pointer in enumerate(pending):
        if set(pointer) != POINTER_FIELDS:
            raise ValueError(
                f"{location} pointer {pointer_index} does not match the "
                "bounded D05 pointer schema"
            )
        pointer_path = pointer.get("path")
        path_parts = (
            pointer_path.split("/")
            if isinstance(pointer_path, str)
            else []
        )
        if (
            not isinstance(pointer_path, str)
            or not pointer_path
            or len(pointer_path) > 1_024
            or pointer_path.startswith("/")
            or "\\" in pointer_path
            or any(part in {"", ".", ".."} for part in path_parts)
            or any(
                unicodedata.category(character) == "Cc"
                for character in pointer_path
            )
        ):
            raise ValueError(
                f"{location} pointer {pointer_index} has an unsafe path"
            )
        title_line = pointer.get("title_line")
        if (
            not isinstance(title_line, str)
            or not title_line
            or len(title_line) > 80
        ):
            raise ValueError(
                f"{location} pointer {pointer_index} has an invalid title_line"
            )
        due = pointer.get("due")
        if not isinstance(due, str) or not due:
            raise ValueError(
                f"{location} pointer {pointer_index} has an invalid due date"
            )
        try:
            due_date = date.fromisoformat(due)
        except ValueError as exc:
            raise ValueError(
                f"{location} pointer {pointer_index} has an invalid due date"
            ) from exc
        if due_date.isoformat() != due:
            raise ValueError(
                f"{location} pointer {pointer_index} due date is not canonical"
            )
        status_note = pointer.get("status_note")
        assert observed_date is not None
        expected_status = (
            "overdue" if due_date < observed_date else "pending"
        )
        if status_note != expected_status:
            raise ValueError(
                f"{location} pointer {pointer_index} has status_note "
                f"{status_note!r}; expected {expected_status!r}"
            )
        matched_terms = pointer.get("matched_terms")
        if (
            not isinstance(matched_terms, list)
            or not 1 <= len(matched_terms) <= 32
            or not all(
                isinstance(term, str)
                and term
                and len(term) <= 80
                for term in matched_terms
            )
            or len(matched_terms) != len(set(matched_terms))
        ):
            raise ValueError(
                f"{location} pointer {pointer_index} has invalid matched_terms"
            )
        paths.append(pointer_path)
    if len(paths) != len(set(paths)):
        raise ValueError(f"{location} has duplicate intention pointers")
    return paths


def load_manifest(
    path: Path,
    *,
    expected_benchmark: str,
) -> dict[str, Any]:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"{path}: could not load manifest: {exc}") from exc
    cases = manifest.get("cases") if isinstance(manifest, dict) else None
    oracle = (
        manifest.get("intention_oracle")
        if isinstance(manifest, dict)
        else None
    )
    if (
        not isinstance(manifest, dict)
        or manifest.get("benchmark_version") != expected_benchmark
        or not isinstance(cases, list)
        or not cases
        or not isinstance(oracle, dict)
    ):
        raise ValueError(
            f"{path}: not the expected {expected_benchmark} intention manifest"
        )
    case_ids = [
        case.get("id")
        for case in cases
        if isinstance(case, dict)
    ]
    if (
        len(case_ids) != len(cases)
        or not all(isinstance(case_id, str) and case_id for case_id in case_ids)
        or len(case_ids) != len(set(case_ids))
        or not set(oracle) <= set(case_ids)
        or any(
            not isinstance(paths, list)
            or not all(isinstance(item, str) and item for item in paths)
            or len(paths) != len(set(paths))
            for paths in oracle.values()
        )
    ):
        raise ValueError(
            f"{path}: intention oracle/case topology is malformed"
        )
    return {
        "path": str(path.resolve()),
        "sha256": sha256_file(path),
        "benchmark_version": expected_benchmark,
        "case_ids": case_ids,
        "embedded_oracle": oracle,
        "oracle": {
            case_id: list(oracle.get(case_id, []))
            for case_id in case_ids
        },
    }


def accepted_e08_runs(
    paths: Sequence[Path],
    *,
    full_manifest_path: Path,
    prospective_manifest_path: Path,
    expected_full_draws: Sequence[str],
    expected_prospective_draws: Sequence[str],
    audit_mode: str = FINAL_AUDIT_MODE,
) -> list[dict[str, Any]]:
    if audit_mode not in AUDIT_MODES:
        raise ValueError(f"unsupported E08 intention audit mode {audit_mode!r}")
    if audit_mode == DRAW1_ABORT_AUDIT_MODE:
        if (
            list(expected_full_draws) != ["e08-full-draw1"]
            or list(expected_prospective_draws)
            != ["e08-prospective-draw1"]
        ):
            raise ValueError(
                "E08 draw1-abort audit requires exactly "
                "e08-full-draw1 and e08-prospective-draw1"
            )
        required_artifact_count = 2
        required_draw_count = 1
    else:
        required_artifact_count = 6
        required_draw_count = 3
    if len(paths) != required_artifact_count:
        raise ValueError(
            f"E08 {audit_mode} intention audit requires exactly "
            f"{required_artifact_count} flag-on artifacts"
        )
    if (
        len(expected_full_draws) != required_draw_count
        or len(set(expected_full_draws)) != required_draw_count
        or len(expected_prospective_draws) != required_draw_count
        or len(set(expected_prospective_draws)) != required_draw_count
        or set(expected_full_draws) & set(expected_prospective_draws)
    ):
        raise ValueError(
            f"E08 {audit_mode} intention audit requires exact, disjoint "
            f"{required_draw_count}-draw sets for full and prospective suites"
        )
    suites = {
        "full": {
            **load_manifest(
                full_manifest_path,
                expected_benchmark="recent-work-v0.3",
            ),
            "expected_draws": set(expected_full_draws),
        },
        "prospective": {
            **load_manifest(
                prospective_manifest_path,
                expected_benchmark="e08-prospective-v0.1",
            ),
            "expected_draws": set(expected_prospective_draws),
        },
    }
    suite_by_manifest_hash = {
        suite["sha256"]: (name, suite)
        for name, suite in suites.items()
    }
    if len(suite_by_manifest_hash) != 2:
        raise ValueError("E08 manifests must have distinct hashes")

    accepted = []
    artifact_hashes: set[str] = set()
    run_ids: set[str] = set()
    draw_ids: set[str] = set()
    for path in paths:
        try:
            run = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"{path}: could not load E08 artifact: {exc}"
            ) from exc
        artifact_sha256 = sha256_file(path)
        provenance = definitive_service_run_provenance(run)
        suite_match = suite_by_manifest_hash.get(
            provenance["manifest_sha256"]
        )
        if suite_match is None:
            raise ValueError(
                f"{path}: artifact is not from either accepted E08 manifest"
            )
        suite_name, suite = suite_match
        draw = provenance["paired_draw_id"]
        if (
            provenance["experiment_arm"] != "e08-flag"
            or draw not in suite["expected_draws"]
            or run.get("manifest", {}).get("intention_oracle")
            != suite["embedded_oracle"]
            or [
                case.get("id")
                for case in run.get("manifest", {}).get("cases", [])
                if isinstance(case, dict)
            ]
            != suite["case_ids"]
            or {
                record.get("case_id")
                for record in run.get("records", [])
                if isinstance(record, dict)
            }
            != set(suite["case_ids"])
        ):
            raise ValueError(
                f"{path}: E08 arm, draw, cases, or oracle is not the exact "
                "accepted selection"
            )
        actual_features = provenance["runtime_features"]
        expected_features = provenance["expected_runtime_features"]
        feature_mismatches = {
            name: {
                "required": required,
                "expected": expected_features.get(name),
                "actual": actual_features.get(name),
            }
            for name, required in EXPECTED_FEATURES.items()
            if (
                name not in expected_features
                or expected_features[name] is not required
                or name not in actual_features
                or actual_features[name] is not required
            )
        }
        if feature_mismatches:
            raise ValueError(
                f"{path}: E08 runtime feature mismatch: {feature_mismatches}"
            )
        if (
            artifact_sha256 in artifact_hashes
            or provenance["run_id"] in run_ids
            or draw in draw_ids
        ):
            raise ValueError(
                f"{path}: duplicate artifact, run ID, or selected draw"
            )
        artifact_hashes.add(artifact_sha256)
        run_ids.add(provenance["run_id"])
        draw_ids.add(draw)
        accepted.append({
            "path": str(path.resolve()),
            "sha256": artifact_sha256,
            "suite": suite_name,
            "manifest": {
                key: suite[key]
                for key in (
                    "path",
                    "sha256",
                    "benchmark_version",
                )
            },
            "oracle": suite["oracle"],
            "run": run,
            "provenance": provenance,
        })

    for suite_name, suite in suites.items():
        actual_draws = {
            item["provenance"]["paired_draw_id"]
            for item in accepted
            if item["suite"] == suite_name
        }
        if actual_draws != suite["expected_draws"]:
            raise ValueError(
                f"E08 {suite_name} artifacts do not exactly match expected "
                "draws"
            )

    stable_fields = (
        "schema_sha256",
        "harness_sha256",
        "source_revision",
        "build_revision",
        "api_image_id",
        "api_image_revision",
        "runtime_features",
        "service_retrieval_modes",
    )
    drift = {
        field: [
            item["provenance"].get(field)
            for item in accepted
        ]
        for field in stable_fields
        if len({
            canonical_json_sha256(item["provenance"].get(field))
            for item in accepted
        }) != 1
    }
    runtime_projection_fields = (
        "schema",
        "status",
        "build_revision",
        "read_only",
        "runtime_features",
        "embeddings",
    )
    runtime_projections = [
        {
            field: item["provenance"]["runtime_snapshot"].get(field)
            for field in runtime_projection_fields
        }
        for item in accepted
    ]
    if len({
        canonical_json_sha256(projection)
        for projection in runtime_projections
    }) != 1:
        drift["runtime_projection"] = runtime_projections
    if drift:
        raise ValueError(
            f"E08 source/build/image/runtime provenance drifted: {drift}"
        )
    return accepted


def audit_runs(
    accepted: Sequence[dict[str, Any]],
    *,
    audit_mode: str = FINAL_AUDIT_MODE,
) -> dict[str, Any]:
    mode_contract = AUDIT_MODES.get(audit_mode)
    if mode_contract is None:
        raise ValueError(f"unsupported E08 intention audit mode {audit_mode!r}")
    if not accepted:
        raise ValueError("E08 intention audit requires accepted artifacts")
    opens = 0
    pointers = 0
    false_surfacings = []
    done_surfacings = []
    char_budget_violations = []
    observations = []
    for artifact in accepted:
        run = artifact["run"]
        oracle = artifact["oracle"]
        provenance = artifact["provenance"]
        for record in run["records"]:
            case_id = record["case_id"]
            allowed = set(oracle[case_id])
            record_open_count = 0
            for operation_index, operation in enumerate(
                record["service_operations"]
            ):
                if operation.get("operation") not in {"open", "resume"}:
                    continue
                record_open_count += 1
                opens += 1
                pending = operation.get("pending_intentions")
                char_count = operation.get("pending_intention_chars")
                if (
                    not isinstance(pending, list)
                    or not all(isinstance(item, dict) for item in pending)
                    or type(char_count) is not int
                    or char_count < 0
                    or char_count
                    != len(json.dumps(
                        pending,
                        separators=(",", ":"),
                        ensure_ascii=False,
                    ))
                ):
                    raise ValueError(
                        f"{artifact['path']}: {case_id} operation "
                        f"{operation_index} has malformed intention accounting"
                    )
                paths = validate_pending_intention_pointers(
                    pending,
                    operation,
                    artifact_path=artifact["path"],
                    case_id=case_id,
                    operation_index=operation_index,
                )
                observation = {
                    "artifact_sha256": artifact["sha256"],
                    "run_id": provenance["run_id"],
                    "paired_draw_id": provenance["paired_draw_id"],
                    "suite": artifact["suite"],
                    "case_id": case_id,
                    "operation_index": operation_index,
                    "chars": char_count,
                    "paths": paths,
                }
                observations.append(observation)
                if char_count > CHAR_BUDGET:
                    char_budget_violations.append(observation)
                for path in paths:
                    pointers += 1
                    row = {
                        "artifact_sha256": artifact["sha256"],
                        "run_id": provenance["run_id"],
                        "paired_draw_id": provenance["paired_draw_id"],
                        "suite": artifact["suite"],
                        "case_id": case_id,
                        "path": path,
                    }
                    if path == DONE_FIXTURE:
                        done_surfacings.append(row)
                    elif path not in allowed:
                        false_surfacings.append(row)
            if record_open_count == 0:
                raise ValueError(
                    f"{artifact['path']}: {case_id} has no auditable "
                    "open/resume response"
                )
    false_surfacing_rate = (
        len(false_surfacings) / opens if opens else None
    )
    if audit_mode == DRAW1_ABORT_AUDIT_MODE:
        false_surfacing_passed = bool(
            false_surfacing_rate is not None
            and false_surfacing_rate
            <= mode_contract["false_surfacing_limit"]
        )
    else:
        false_surfacing_passed = bool(
            false_surfacing_rate is not None
            and false_surfacing_rate
            < mode_contract["false_surfacing_limit"]
        )
    passed = bool(
        opens
        and false_surfacing_passed
        and not done_surfacings
        and not char_budget_violations
    )
    stable_provenance = accepted[0]["provenance"]
    return {
        "schema": "brunn-intention-audit@v2",
        "audit_mode": audit_mode,
        "inputs": [
            {
                "path": artifact["path"],
                "sha256": artifact["sha256"],
                "suite": artifact["suite"],
                "manifest": artifact["manifest"],
                "run_id": artifact["provenance"]["run_id"],
                "experiment_arm": artifact["provenance"][
                    "experiment_arm"
                ],
                "paired_draw_id": artifact["provenance"][
                    "paired_draw_id"
                ],
            }
            for artifact in accepted
        ],
        "provenance": {
            field: stable_provenance[field]
            for field in (
                "schema_sha256",
                "harness_sha256",
                "source_revision",
                "build_revision",
                "api_image_id",
                "api_image_revision",
                "runtime_features",
                "service_retrieval_modes",
                "reasoning_billing",
            )
        },
        "opens": opens,
        "pointers": pointers,
        "observations": observations,
        "false_surfacings": false_surfacings,
        "false_surfacing_rate": false_surfacing_rate,
        "done_surfacings": done_surfacings,
        "char_budget_violations": char_budget_violations,
        "thresholds": {
            "false_surfacing_rate": mode_contract[
                "false_surfacing_threshold"
            ],
            "pending_intention_pointers": f"<={POINTER_LIMIT}",
            "pending_intention_chars": f"<={CHAR_BUDGET}",
            "done_surfacings": 0,
        },
        "pass": passed,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Audit provenance-bound E08 flag-on artifacts for intention false "
            "surfacing"
        )
    )
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument(
        "--audit-mode",
        choices=tuple(AUDIT_MODES),
        default=FINAL_AUDIT_MODE,
        help=(
            "final requires all six artifacts and the <10%% acceptance gate; "
            "draw1-abort requires only the exact two draw-1 artifacts and "
            "enforces the >25%% abort gate"
        ),
    )
    parser.add_argument("--full-manifest", required=True, type=Path)
    parser.add_argument("--prospective-manifest", required=True, type=Path)
    parser.add_argument(
        "--expected-full-draw",
        action="append",
        required=True,
    )
    parser.add_argument(
        "--expected-prospective-draw",
        action="append",
        required=True,
    )
    parser.add_argument("--out", required=True, type=Path)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    accepted = accepted_e08_runs(
        args.inputs,
        full_manifest_path=args.full_manifest,
        prospective_manifest_path=args.prospective_manifest,
        expected_full_draws=args.expected_full_draw,
        expected_prospective_draws=args.expected_prospective_draw,
        audit_mode=args.audit_mode,
    )
    result = audit_runs(accepted, audit_mode=args.audit_mode)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(result, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(result, indent=2))
    return 0 if result["pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any, Mapping, Sequence

try:
    from .aggregate_draws import (
        load_draw,
        validate_service_provenance_pairing,
    )
except ImportError:
    from aggregate_draws import (
        load_draw,
        validate_service_provenance_pairing,
    )


SCHEMA = "straylight-accepted-source-context-audit@v1"


def normalize_path(value: str) -> str:
    normalized = value.strip().replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    if normalized.startswith("corpus/"):
        normalized = normalized.removeprefix("corpus/")
    return normalized


def returned_source_paths(record: dict[str, Any], path: Path) -> set[str]:
    operations = record.get("service_operations")
    if not isinstance(operations, list):
        raise ValueError(
            f"{path}: {record.get('case_id')} lacks saved service_operations"
        )
    returned: set[str] = set()
    for index, operation in enumerate(operations):
        if not isinstance(operation, dict):
            raise ValueError(
                f"{path}: {record.get('case_id')} operation {index} is malformed"
            )
        source_paths = operation.get("source_paths")
        if not isinstance(source_paths, list) or not all(
            isinstance(source_path, str) for source_path in source_paths
        ):
            raise ValueError(
                f"{path}: {record.get('case_id')} operation {index} lacks "
                "saved source_paths"
            )
        returned.update(normalize_path(source_path) for source_path in source_paths)
    return returned


def _claim_rows(
    run: dict[str, Any],
    path: Path,
    artifact: dict[str, Any],
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    manifest = run.get("manifest")
    records = run.get("records")
    if not isinstance(manifest, dict) or not isinstance(records, list):
        raise ValueError(f"{path}: input lacks manifest or records")
    cases: dict[str, dict[str, Any]] = {}
    for case in manifest.get("cases", []):
        if (
            not isinstance(case, dict)
            or not isinstance(case.get("id"), str)
            or not case["id"]
            or case["id"] in cases
        ):
            raise ValueError(f"{path}: manifest contains malformed case IDs")
        cases[case["id"]] = case
    arm = run.get("experiment_arm")
    paired_draw_id = run.get("paired_draw_id")
    if arm is None:
        arm = "service_api"
        paired_draw_id = run.get("run_id")
    rows = []
    service_records = 0
    observed_service_cases: set[str] = set()
    for record in records:
        if not isinstance(record, dict):
            raise ValueError(f"{path}: record is malformed")
        if record.get("condition") not in {"service_api", "service_api_resume"}:
            continue
        service_records += 1
        case_id = record.get("case_id")
        if not isinstance(case_id, str) or case_id in observed_service_cases:
            raise ValueError(f"{path}: duplicate or malformed service case {case_id}")
        observed_service_cases.add(case_id)
        case = cases.get(case_id)
        grade = record.get("grade")
        if case is None or not isinstance(grade, dict):
            raise ValueError(f"{path}: incomplete service record for {case_id}")
        returned = returned_source_paths(record, path)
        grades_by_id: dict[str, dict[str, Any]] = {}
        for claim in grade.get("claims", []):
            if (
                not isinstance(claim, dict)
                or not isinstance(claim.get("id"), str)
                or not claim["id"]
                or claim["id"] in grades_by_id
                or not isinstance(claim.get("pass"), bool)
            ):
                raise ValueError(
                    f"{path}: {case_id} contains malformed structured grades"
                )
            grades_by_id[claim["id"]] = claim
        rubric_ids = []
        rubrics = case.get("rubric")
        if not isinstance(rubrics, list) or not rubrics:
            raise ValueError(f"{path}: {case_id} contains no rubric")
        for rubric in rubrics:
            if not isinstance(rubric, dict):
                raise ValueError(f"{path}: {case_id} contains a malformed rubric")
            claim_id = rubric.get("id")
            rubric_ids.append(claim_id)
            claim_grade = grades_by_id.get(claim_id)
            if (
                not isinstance(claim_id, str)
                or not isinstance(claim_grade, dict)
            ):
                raise ValueError(
                    f"{path}: {case_id} lacks structured grade for {claim_id}"
                )
            sources_all = rubric.get("sources_all")
            if isinstance(sources_all, list):
                if not sources_all or not all(
                    isinstance(value, str) and value for value in sources_all
                ):
                    raise ValueError(
                        f"{path}: {case_id}:{claim_id} has malformed sources_all"
                    )
                accepted = [normalize_path(value) for value in sources_all]
                requirement = "all"
                present = all(source in returned for source in accepted)
                matched = [source for source in accepted if source in returned]
            else:
                sources_any = rubric.get("sources_any")
                if (
                    not isinstance(sources_any, list)
                    or not sources_any
                    or not all(
                        isinstance(value, str) and value
                        for value in sources_any
                    )
                ):
                    raise ValueError(
                        f"{path}: {case_id}:{claim_id} lacks accepted sources"
                    )
                accepted = [normalize_path(value) for value in sources_any]
                requirement = "any"
                matched = [source for source in accepted if source in returned]
                present = bool(matched)
            rows.append({
                "suite": artifact["suite"],
                "run_id": run.get("run_id"),
                "paired_draw_id": paired_draw_id,
                "arm": arm,
                "case_id": case_id,
                "claim_id": claim_id,
                "claim_pass": claim_grade.get("pass"),
                "missed_claim": claim_grade.get("pass") is not True,
                "accepted_source_requirement": requirement,
                "accepted_sources": accepted,
                "returned_source_paths": sorted(returned),
                "matched_accepted_sources": matched,
                "accepted_source_in_context": present,
                "tags": list(rubric.get("tags", [])),
            })
        if (
            len(rubric_ids) != len(set(rubric_ids))
            or set(grades_by_id) != set(rubric_ids)
        ):
            raise ValueError(
                f"{path}: {case_id} rubric and structured grade IDs differ"
            )
    if service_records == 0:
        raise ValueError(f"{path}: input contains no service records to audit")
    if not rows:
        raise ValueError(f"{path}: input contains no rubric claims to audit")
    return {
        "path": str(path.resolve()),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "run_id": run.get("run_id"),
        "paired_draw_id": paired_draw_id,
        "arm": arm,
        "suite": artifact["suite"],
        "source_revision": artifact["source_revision"],
        "grading_revision": artifact["grading_revision"],
        "service_retrieval_modes": artifact["service_retrieval_modes"],
        "service_image_provenance": artifact["service_image_provenance"],
        "service_arms": artifact["service_arms"],
        "service_provenance": artifact["service_provenance"],
    }, rows


def _rate(rows: Sequence[dict[str, Any]]) -> dict[str, Any]:
    present = sum(bool(row["accepted_source_in_context"]) for row in rows)
    return {
        "claims": len(rows),
        "accepted_source_in_context": present,
        "rate": present / len(rows) if rows else None,
    }


def audit(
    paths: Sequence[Path],
    *,
    expected_arm_retrieval_modes: (
        Sequence[str] | Mapping[str, Sequence[str]] | None
    ) = None,
) -> dict[str, Any]:
    if not paths:
        raise ValueError("at least one result artifact is required")
    artifacts = []
    rows = []
    for path in paths:
        try:
            run = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(f"{path}: could not load result JSON: {exc}") from exc
        if not isinstance(run, dict):
            raise ValueError(f"{path}: result must be an object")
        validated_artifact, _ = load_draw(path)
        artifact, artifact_rows = _claim_rows(
            run,
            path,
            validated_artifact,
        )
        artifacts.append(artifact)
        rows.extend(artifact_rows)
    service_provenance = validate_service_provenance_pairing(
        artifacts,
        expected_arm_retrieval_modes,
        definitive=True,
    )
    revisions = {artifact["source_revision"] for artifact in artifacts}
    grading_revisions = {artifact["grading_revision"] for artifact in artifacts}
    if len(revisions) != 1 or len(grading_revisions) != 1:
        raise ValueError("accepted-source audit inputs span mixed revisions")
    by_arm: dict[str, list[dict[str, Any]]] = defaultdict(list)
    by_suite: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_arm[row["arm"]].append(row)
        by_suite[row["suite"]].append(row)
    missed = [row for row in rows if row["missed_claim"]]
    return {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "source_revision": next(iter(revisions)),
        "grading_revision": next(iter(grading_revisions)),
        "service_provenance": service_provenance,
        "input_artifacts": artifacts,
        "summary": {
            "all_claims": _rate(rows),
            "missed_claims": _rate(missed),
            "by_arm": {
                arm: {
                    "all_claims": _rate(arm_rows),
                    "missed_claims": _rate(
                        [row for row in arm_rows if row["missed_claim"]]
                    ),
                }
                for arm, arm_rows in sorted(by_arm.items())
            },
            "by_suite": {
                suite: {
                    "all_claims": _rate(suite_rows),
                    "missed_claims": _rate(
                        [row for row in suite_rows if row["missed_claim"]]
                    ),
                }
                for suite, suite_rows in sorted(by_suite.items())
            },
        },
        "claims": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Audit whether rubric-accepted sources were returned in saved service context"
    )
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument(
        "--expected-arm-retrieval-modes",
        action="append",
        metavar="ARM=MODE,...",
        help=(
            "declare exact modes for one service-backed arm; repeat for every "
            "arm in the audit"
        ),
    )
    args = parser.parse_args()
    result = audit(
        args.inputs,
        expected_arm_retrieval_modes=args.expected_arm_retrieval_modes,
    )
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": "ok",
        "out": str(args.out),
        "missed_claims": result["summary"]["missed_claims"],
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

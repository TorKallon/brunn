#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


DONE_FIXTURE = "Intentions/Completed renewal invoice.md"
CHAR_BUDGET = 500


def audit_runs(runs: list[dict[str, Any]]) -> dict[str, Any]:
    opens = 0
    pointers = 0
    false_surfacings = []
    done_surfacings = []
    char_budget_violations = []
    for run in runs:
        oracle = run.get("manifest", {}).get("intention_oracle", {})
        run_id = run.get("run_id")
        for record in run.get("records", []):
            if not isinstance(record, dict) or record.get("condition") != "service_api":
                continue
            case_id = record.get("case_id")
            allowed = set(oracle.get(case_id, [])) if isinstance(oracle, dict) else set()
            for operation in record.get("service_operations", []):
                if not isinstance(operation, dict) or operation.get("operation") not in {
                    "open",
                    "resume",
                }:
                    continue
                opens += 1
                char_count = int(operation.get("pending_intention_chars", 0))
                if char_count > CHAR_BUDGET:
                    char_budget_violations.append({
                        "run_id": run_id,
                        "case_id": case_id,
                        "chars": char_count,
                    })
                for item in operation.get("pending_intentions", []):
                    if not isinstance(item, dict) or not isinstance(item.get("path"), str):
                        continue
                    path = item["path"]
                    pointers += 1
                    row = {"run_id": run_id, "case_id": case_id, "path": path}
                    if path == DONE_FIXTURE:
                        done_surfacings.append(row)
                    elif path not in allowed:
                        false_surfacings.append(row)
    return {
        "schema": "straylight-intention-audit@v1",
        "opens": opens,
        "pointers": pointers,
        "false_surfacings": false_surfacings,
        "false_surfacing_rate": len(false_surfacings) / opens if opens else None,
        "done_surfacings": done_surfacings,
        "char_budget_violations": char_budget_violations,
        "pass": (
            bool(opens)
            and len(false_surfacings) / opens < 0.10
            and not done_surfacings
            and not char_budget_violations
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Audit saved E08 open receipts for intention false surfacing"
    )
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    runs = [json.loads(path.read_text(encoding="utf-8")) for path in args.inputs]
    result = audit_runs(runs)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()

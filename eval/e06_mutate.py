#!/usr/bin/env python3
"""Deterministic E06 seed-to-resume mutation hook.

The harness invokes ``plan`` without network access, then invokes ``apply``
after the service has created the pinned checkpoint. ``apply`` changes exactly
three checkpoint sources through the ordinary workspace write API and mirrors
the same bytes into an isolated filesystem corpus.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from pathlib import Path
from typing import Any

import sys


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from native_eval import NativeApiClient  # noqa: E402


SPEC_PATH = ROOT / "eval" / "e06_mutations.json"
SOURCE_LIMIT = 3
SMALL_SOURCE_CHARS = 2_400
SHA256_RE = re.compile(r"^(?:sha256:)?[0-9a-f]{64}$")


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def sha256_text(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()


def safe_relative_path(value: str) -> Path:
    path = Path(value)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        raise ValueError(f"unsafe mutation path: {value!r}")
    return path


def case_spec(case_id: str) -> dict[str, Any]:
    spec = load_json(SPEC_PATH)
    if spec.get("schema") != "brunn-e06-mutations@v1":
        raise ValueError("unsupported E06 mutation schema")
    case = spec.get("cases", {}).get(case_id)
    if not isinstance(case, dict):
        raise ValueError(f"no E06 mutation plan for {case_id}")
    targets = case.get("targets")
    if not isinstance(targets, list) or len(targets) != SOURCE_LIMIT:
        raise ValueError(f"{case_id}: E06 requires exactly three mutation targets")
    paths = [target.get("path") for target in targets if isinstance(target, dict)]
    if len(paths) != SOURCE_LIMIT or len(set(paths)) != SOURCE_LIMIT:
        raise ValueError(f"{case_id}: mutation target paths must be three unique strings")
    return case


def initial_source(target: dict[str, Any], base_root: Path) -> tuple[Path, str]:
    relative = safe_relative_path(str(target["path"]))
    if target.get("fixture"):
        source = ROOT / safe_relative_path(str(target["fixture"]))
    else:
        source = base_root / relative
    if not source.is_file():
        raise ValueError(f"mutation source does not exist: {source}")
    return relative, source.read_text(encoding="utf-8")


def render_after(
    *,
    case_id: str,
    seed: str,
    target: dict[str, Any],
    before: str,
    delta_text: str,
    authority_path: str,
) -> str:
    operation = target.get("operation")
    marker = (
        "\n\n## E06 controlled post-checkpoint mutation\n\n"
        f"Mutation seed: `{seed}`. This section is synthetic evaluation input "
        "and is not durable project, production, game, booking, or travel truth.\n"
    )
    if operation == "append_delta":
        return (
            before.rstrip()
            + marker
            + "\n"
            + delta_text.rstrip()
            + "\n"
        )
    if operation == "append_pointer":
        return (
            before.rstrip()
            + marker
            + f"\nThe controlled changed facts for `{case_id}` are recorded in "
            f"`{authority_path}`.\n"
        )
    raise ValueError(f"{case_id}: unsupported mutation operation {operation!r}")


def build_plan(case_id: str, base_root: Path, seed: str) -> dict[str, Any]:
    case = case_spec(case_id)
    delta_path = safe_relative_path(str(case["delta_path"]))
    delta_text = (ROOT / delta_path).read_text(encoding="utf-8")
    authority_path = str(case["targets"][0]["path"])
    targets = []
    for target in case["targets"]:
        relative, before = initial_source(target, base_root)
        after = render_after(
            case_id=case_id,
            seed=seed,
            target=target,
            before=before,
            delta_text=delta_text,
            authority_path=authority_path,
        )
        if before == after:
            raise ValueError(f"{case_id}: mutation did not change {relative}")
        targets.append(
            {
                "path": relative.as_posix(),
                "fixture": target.get("fixture"),
                "operation": target["operation"],
                "before": before,
                "after": after,
                "before_chars": len(before),
                "after_chars": len(after),
                "before_sha256": sha256_text(before),
                "after_sha256": sha256_text(after),
            }
        )
    if not any(
        target["before_chars"] <= SMALL_SOURCE_CHARS
        and target["after_chars"] <= SMALL_SOURCE_CHARS
        for target in targets
    ):
        raise ValueError(f"{case_id}: mutation plan does not exercise whole_pair mode")
    if not any(target["before_chars"] > SMALL_SOURCE_CHARS for target in targets):
        raise ValueError(f"{case_id}: mutation plan does not exercise unified_diff mode")
    return {
        "schema": "brunn-e06-mutation-plan@v1",
        "case_id": case_id,
        "seed": seed,
        "delta_path": delta_path.as_posix(),
        "source_refs": [target["path"] for target in targets],
        "targets": targets,
    }


def response_items(response: dict[str, Any]) -> list[dict[str, Any]]:
    data = response.get("data", response)
    items = data.get("items") if isinstance(data, dict) else None
    if not isinstance(items, list):
        raise ValueError("workspace.read did not return an items list")
    return [item for item in items if isinstance(item, dict)]


def read_workspace_sources(
    client: NativeApiClient,
    paths: list[str],
) -> tuple[dict[str, dict[str, Any]], str | None]:
    response = client.post(
        "/v1/workspace/read",
        {
            "requests": [
                {"path": path, "view": "full", "max_chars": 4 * 1024 * 1024}
                for path in paths
            ]
        },
    )
    data = response.body.get("data", response.body)
    if (
        not isinstance(data, dict)
        or data.get("response_truncated") is not False
        or data.get("requested_count") != len(paths)
    ):
        raise ValueError(
            "workspace.read mutation verification was truncated or "
            "cardinality-mismatched"
        )
    items = response_items(response.body)
    by_path = {
        item["path"]: item
        for item in items
        if isinstance(item.get("path"), str)
    }
    missing = sorted(set(paths) - set(by_path))
    if missing:
        raise ValueError(f"workspace.read omitted mutation sources: {missing}")
    corpus_revision = response.body.get("corpus_revision")
    return by_path, corpus_revision if isinstance(corpus_revision, str) else None


def apply_plan(
    plan: dict[str, Any],
    mirror_root: Path,
    *,
    workspace: bool,
) -> dict[str, Any]:
    mirror_root.mkdir(parents=True, exist_ok=True)
    paths = [target["path"] for target in plan["targets"]]
    client = (
        NativeApiClient(run_id=os.environ.get("BRUNN_EVAL_RUN"), case_id=plan["case_id"])
        if workspace
        else None
    )
    workspace_before, _ = (
        read_workspace_sources(client, paths) if client else ({}, None)
    )
    receipts = []
    for target in plan["targets"]:
        path = target["path"]
        mirror = mirror_root / safe_relative_path(path)
        mirror.parent.mkdir(parents=True, exist_ok=True)
        if not mirror.exists():
            mirror.write_text(target["before"], encoding="utf-8")
        mirror_before = mirror.read_text(encoding="utf-8")
        if mirror_before != target["before"]:
            raise ValueError(f"{path}: filesystem mirror diverged before mutation")
        expected_version = None
        if client:
            current = workspace_before[path]
            if (
                current.get("text") != target["before"]
                or current.get("content_hash") != target["before_sha256"]
            ):
                raise ValueError(f"{path}: workspace and filesystem diverged before mutation")
            expected_version = current.get("version")
            if not isinstance(expected_version, int):
                raise ValueError(f"{path}: workspace.read omitted the pinned version")
            response = client.post(
                "/v1/workspace/write",
                {
                    "path": path,
                    "content": target["after"],
                    "media_type": "text/markdown",
                    "expected_version": expected_version,
                    "idempotency_key": (
                        f"e06:{plan['case_id']}:{plan['seed']}:{hashlib.sha256(path.encode()).hexdigest()[:16]}"
                    ),
                    "metadata": {
                        "evaluation_fixture": "E06",
                        "mutation_seed": plan["seed"],
                    },
                },
            )
            write_data = response.data
            receipt_hash = write_data.get("content_hash")
            if (
                write_data.get("path") != path
                or write_data.get("version") != expected_version + 1
                or write_data.get("no_op") is not False
                or not isinstance(receipt_hash, str)
                or not SHA256_RE.fullmatch(receipt_hash)
                or receipt_hash != target["after_sha256"]
            ):
                raise ValueError(
                    f"{path}: workspace.write receipt did not prove the "
                    "expected N+1 mutation"
                )
        mirror.write_text(target["after"], encoding="utf-8")
        receipts.append(
            {
                "path": path,
                "pinned_version": expected_version,
                "current_version": (
                    write_data["version"] if client else None
                ),
                "before_sha256": target["before_sha256"],
                "after_sha256": target["after_sha256"],
                "write_no_op": (
                    write_data["no_op"] if client else None
                ),
            }
        )
    if client:
        workspace_after, corpus_revision = read_workspace_sources(client, paths)
        receipts_by_path = {
            receipt["path"]: receipt
            for receipt in receipts
        }
        for target in plan["targets"]:
            path = target["path"]
            mirror_text = (mirror_root / safe_relative_path(path)).read_text(encoding="utf-8")
            observed = workspace_after[path]
            receipt = receipts_by_path[path]
            if (
                observed.get("text") != mirror_text
                or observed.get("version") != receipt["current_version"]
                or observed.get("content_hash") != target["after_sha256"]
                or sha256_text(mirror_text) != target["after_sha256"]
            ):
                raise ValueError(
                    f"{path}: workspace.read did not verify the exact "
                    "post-mutation version and bytes"
                )
            receipt["verified_read_version"] = observed["version"]
            receipt["verified_read_sha256"] = observed["content_hash"]
    else:
        corpus_revision = None
    return {
        "schema": "brunn-e06-mutation-receipt@v1",
        "case_id": plan["case_id"],
        "seed": plan["seed"],
        "paths": paths,
        "exactly_three_sources": len(paths) == SOURCE_LIMIT,
        "workspace_mutated": workspace,
        "workspace_vault_byte_identical": workspace,
        "corpus_revision": corpus_revision,
        "receipts": receipts,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Apply deterministic E06 checkpoint-source edits")
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("plan", "apply"):
        command = subparsers.add_parser(name)
        command.add_argument("--case-id", required=True)
        command.add_argument("--base-root", type=Path, required=True)
        command.add_argument("--seed", required=True)
        if name == "apply":
            command.add_argument("--mirror-root", type=Path, required=True)
            command.add_argument("--mirror-only", action="store_true")
    return parser


def main() -> None:
    args = build_parser().parse_args()
    plan = build_plan(args.case_id, args.base_root.resolve(), args.seed)
    if args.command == "plan":
        print(json.dumps(plan, sort_keys=True))
        return
    receipt = apply_plan(
        plan,
        args.mirror_root.resolve(),
        workspace=not args.mirror_only,
    )
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()

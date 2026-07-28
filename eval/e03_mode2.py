#!/usr/bin/env python3

"""Own the deterministic mock lifecycle for an E03 mode-2 run."""

from __future__ import annotations

import argparse
import hashlib
import json
import shlex
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parents[1]
MOCK = PROJECT_ROOT / "tests" / "mock_openai_embeddings.py"
PERFORMANCE = PROJECT_ROOT / "performance_eval.py"
SCHEMA = "straylight-e03-mode2-orchestration@v1"


def run_command(
    command: list[str],
    *,
    timeout: float,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=PROJECT_ROOT,
        stdin=subprocess.DEVNULL,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )


def mock_command(args: argparse.Namespace, action: str) -> list[str]:
    command = [
        sys.executable,
        str(MOCK),
        action,
        "--port",
        str(args.mock_port),
        "--state",
        str(args.mock_state),
        "--log",
        str(args.mock_log),
        "--config",
        str(args.mock_config),
    ]
    if action in {"start", "configure"}:
        command.extend(["--delay-ms", "0", "--error-status", "0"])
    return command


def status(args: argparse.Namespace) -> tuple[int, dict[str, Any]]:
    completed = run_command(
        mock_command(args, "status"),
        timeout=5.0,
    )
    try:
        value = json.loads(completed.stdout or "{}")
    except json.JSONDecodeError:
        value = {"parse_error": True}
    return completed.returncode, value


def command_fingerprint(command: list[str]) -> dict[str, Any]:
    rendered = json.dumps(command, separators=(",", ":"))
    return {
        "executable": Path(command[0]).name,
        "argv_sha256": hashlib.sha256(rendered.encode("utf-8")).hexdigest(),
        "argument_count": len(command),
    }


def build_performance_command(args: argparse.Namespace) -> list[str]:
    failure = mock_command(args, "configure")
    failure[-1] = "503"
    restore = mock_command(args, "configure")
    command = [
        sys.executable,
        str(PERFORMANCE),
        "run",
        "--protocol",
        "simple",
        "--wait-semantic",
        "--semantic-failure-probe",
        "required",
        "--expect-feature-flag",
        "semantic_lane=on",
        "--expect-feature-flag",
        "embed_cache=off",
        "--expect-feature-flag",
        "embedding_backfill_guard=on",
        "--expect-feature-flag",
        "verbatim_spans=on",
        "--expect-runtime-config",
        "semantic_deadline_ms=null",
        "--label",
        args.label,
        "--out",
        str(args.out),
        "--semantic-failure-start-command",
        shlex.join(failure),
        "--semantic-failure-stop-command",
        shlex.join(restore),
        "--semantic-failure-settle-seconds",
        str(args.failure_settle_seconds),
    ]
    if args.quick:
        command.append("--quick")
    else:
        command.extend(["--samples", str(args.samples)])
    if args.future_soak:
        command.append("--future-soak")
    if args.scales:
        command.extend(["--scales", *(str(value) for value in args.scales)])
    if args.api_container:
        command.extend(["--api-container", args.api_container])
    if args.db_container:
        command.extend(["--db-container", args.db_container])
    if args.expect_build_revision:
        command.extend([
            "--expect-build-revision",
            args.expect_build_revision,
        ])
    command.extend(["--query-budget-profile", args.query_budget_profile])
    if args.query_budget_contract:
        command.extend([
            "--query-budget-contract",
            str(args.query_budget_contract),
        ])
    for state in args.feature_state:
        command.extend(["--feature-state", state])
    for state in args.expect_feature_flag:
        command.extend(["--expect-feature-flag", state])
    for value in args.expect_runtime_config:
        command.extend(["--expect-runtime-config", value])
    return command


def annotate_result(
    path: Path,
    orchestration: dict[str, Any],
) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError) as error:
        value = {
            "schema": "straylight-performance-eval@v2",
            "pass": False,
            "errors": [
                {
                    "type": type(error).__name__,
                    "message": "mode-2 performance artifact was missing or invalid",
                }
            ],
        }
    value["mode2_orchestration"] = orchestration
    value["pass"] = bool(value.get("pass")) and orchestration["pass"]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    return value


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run E03 mode 2 with an owned deterministic mock lifecycle"
    )
    parser.add_argument("--label", required=True)
    parser.add_argument("--mock-port", type=int, required=True)
    parser.add_argument("--mock-state", type=Path, required=True)
    parser.add_argument("--mock-log", type=Path, required=True)
    parser.add_argument("--mock-config", type=Path, required=True)
    parser.add_argument("--api-container")
    parser.add_argument("--db-container")
    parser.add_argument("--expect-build-revision")
    parser.add_argument("--scales", type=int, nargs="+")
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--future-soak", action="store_true")
    parser.add_argument("--feature-state", action="append", default=[])
    parser.add_argument("--expect-feature-flag", action="append", default=[])
    parser.add_argument("--expect-runtime-config", action="append", default=[])
    parser.add_argument("--query-budget-profile", default="default-safe")
    parser.add_argument("--query-budget-contract", type=Path)
    parser.add_argument("--failure-settle-seconds", type=float, default=0.25)
    parser.add_argument("--timeout", type=float, default=14_400.0)
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.mock_port < 1 or args.mock_port > 65_535:
        raise ValueError("mock port is out of range")
    if args.timeout <= 0:
        raise ValueError("--timeout must be positive")
    before_code, before = status(args)
    if before_code == 0 or before.get("live"):
        raise RuntimeError(
            "refusing to adopt an already-running mock; use a run-unique "
            "port and state path so teardown cannot invalidate another run"
        )
    performance_command = build_performance_command(args)
    started: subprocess.CompletedProcess[str] | None = None
    performance: subprocess.CompletedProcess[str] | None = None
    after_start: dict[str, Any] = {}
    after_restore: dict[str, Any] = {}
    after_stop: dict[str, Any] = {}
    stop_result: subprocess.CompletedProcess[str] | None = None
    orchestration_error: dict[str, str] | None = None
    try:
        started = run_command(mock_command(args, "start"), timeout=10.0)
        if started.returncode != 0:
            raise RuntimeError("owned embedding mock failed to start")
        status_code, after_start = status(args)
        if (
            status_code != 0
            or after_start.get("behavior")
            != {"delay_ms": 0, "error_status": 0}
        ):
            raise RuntimeError(
                f"embedding mock start state was not healthy/fast: {after_start}"
            )
        performance = run_command(performance_command, timeout=args.timeout)
        _, after_restore = status(args)
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        orchestration_error = {
            "type": type(error).__name__,
            "message": str(error),
        }
    finally:
        try:
            stop_result = run_command(mock_command(args, "stop"), timeout=10.0)
            _, after_stop = status(args)
        except (OSError, subprocess.TimeoutExpired) as error:
            orchestration_error = orchestration_error or {
                "type": type(error).__name__,
                "message": str(error),
            }

    orchestration = {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "owned_mock": True,
        "port": args.mock_port,
        "preexisting_mock": before,
        "healthy_fast_after_start": after_start,
        "healthy_fast_after_failure_restore": after_restore,
        "stopped_owned_mock": after_stop,
        "start": {
            "exit_code": started.returncode if started else None,
            **command_fingerprint(mock_command(args, "start")),
        },
        "performance": {
            "exit_code": performance.returncode if performance else None,
            **command_fingerprint(performance_command),
        },
        "stop": {
            "exit_code": stop_result.returncode if stop_result else None,
            **command_fingerprint(mock_command(args, "stop")),
        },
        "error": orchestration_error,
    }
    orchestration["pass"] = bool(
        started
        and started.returncode == 0
        and orchestration_error is None
        and performance
        and performance.returncode in {0, 2}
        and after_start.get("behavior")
        == {"delay_ms": 0, "error_status": 0}
        and after_restore.get("behavior")
        == {"delay_ms": 0, "error_status": 0}
        and stop_result
        and stop_result.returncode == 0
        and not after_stop.get("live", True)
    )
    result = annotate_result(args.out, orchestration)
    print(
        json.dumps(
            {
                "status": "ok" if result.get("pass") else "failed",
                "out": str(args.out),
                "performance_exit_code": (
                    performance.returncode if performance else None
                ),
                "mode2_orchestration": orchestration,
            },
            indent=2,
        )
    )
    return 0 if result.get("pass") else 2


if __name__ == "__main__":
    raise SystemExit(main())

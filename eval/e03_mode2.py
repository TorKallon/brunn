#!/usr/bin/env python3

"""Own the deterministic mock lifecycle for an E03 mode-2 run."""

from __future__ import annotations

import argparse
import hashlib
import json
import shlex
import subprocess
import sys
import urllib.parse
from datetime import datetime
from pathlib import Path
from typing import Any

PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))
from performance_eval import (  # noqa: E402
    E03_COMMON_RUNTIME_EXPECTATIONS,
    E03_EMBEDDING_DIMENSIONS,
    E03_EMBEDDING_MODELS,
)

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


def artifact_contract(path: Path, args: argparse.Namespace) -> dict[str, Any]:
    try:
        artifact = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        artifact = {}
    fingerprint = artifact.get("implementation_fingerprint")
    fingerprint = fingerprint if isinstance(fingerprint, dict) else {}
    runtime = artifact.get("runtime_configuration")
    runtime = runtime if isinstance(runtime, dict) else {}
    before = runtime.get("before") if isinstance(runtime.get("before"), dict) else {}
    after = runtime.get("after") if isinstance(runtime.get("after"), dict) else {}
    cost = artifact.get("e03_embedding_cost")
    cost = cost if isinstance(cost, dict) else {}
    checks = {
        "artifact_pass": artifact.get("pass") is True,
        "profile_and_arm": (
            artifact.get("gate_profile") == "e03-semantic-ready"
            and artifact.get("e03_arm") == "mode2"
        ),
        "retrieval_modes": artifact.get("retrieval_modes")
        == ["exact", "lexical", "semantic"],
        "exact_revision_binding": bool(
            fingerprint.get("reproducible") is True
            and fingerprint.get("source_revision") == args.expect_build_revision
            and fingerprint.get("api_image_revision")
            == args.expect_build_revision
            and fingerprint.get("worker_image_revision")
            == args.expect_build_revision
            and fingerprint.get("worker_running") is True
        ),
        "runtime_stable": (
            runtime.get("stable") is True
            and before.get("build_revision") == args.expect_build_revision
            and after.get("build_revision") == args.expect_build_revision
            and before.get("embeddings", {}).get("provider") == "openai"
            and after.get("embeddings", {}).get("provider") == "openai"
        ),
        "mock_cost_is_zero": (
            float(cost.get("accounted_estimate_usd", -1.0)) == 0.0
            and cost.get("mode2_mock_billing_is_zero") is True
        ),
        "exact_sample_count": (
            artifact.get("run_profile", {}).get("samples_per_retrieval") == 30
        ),
    }
    return {
        "checks": checks,
        "pass": all(checks.values()),
        "performance_harness_sha256": hashlib.sha256(
            PERFORMANCE.read_bytes()
        ).hexdigest(),
        "wrapper_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "mock_sha256": hashlib.sha256(MOCK.read_bytes()).hexdigest(),
    }


def endpoint_contract(args: argparse.Namespace) -> dict[str, Any]:
    parsed = urllib.parse.urlparse(args.expected_openai_base_url)
    if (
        parsed.scheme != "http"
        or parsed.port != args.mock_port
        or parsed.path.rstrip("/") != "/v1"
        or not parsed.hostname
    ):
        raise ValueError(
            "--expected-openai-base-url must be a run-unique local HTTP /v1 "
            "URL whose port equals --mock-port"
        )

    def inspect(name: str, service: str) -> dict[str, Any]:
        completed = run_command(["docker", "inspect", name], timeout=10.0)
        if completed.returncode != 0:
            raise RuntimeError(f"could not inspect Mode 2 {service} container")
        try:
            record = json.loads(completed.stdout)[0]
            labels = record["Config"].get("Labels") or {}
            env_rows = record["Config"].get("Env") or []
            state = record["State"]
        except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
            raise RuntimeError(
                f"invalid Mode 2 {service} inspection"
            ) from error
        environment = {}
        for row in env_rows:
            if isinstance(row, str) and "=" in row:
                key, value = row.split("=", 1)
                environment[key] = value
        key_value = environment.get("OPENAI_API_KEY", "")
        credential_fingerprint = hashlib.sha256(
            ("inline\0" + key_value).encode("utf-8")
        ).hexdigest()
        checks = {
            "service": labels.get("com.docker.compose.service") == service,
            "running": state.get("Running") is True,
            "exact_api_image": record.get("Image") == args.expect_api_image_id,
            "exact_revision": (
                labels.get("org.opencontainers.image.revision")
                == args.expect_build_revision
            ),
            "endpoint": (
                environment.get("OPENAI_BASE_URL")
                == args.expected_openai_base_url
            ),
            "provider": (
                environment.get("STRAYLIGHT_EMBEDDING_PROVIDER", "openai")
                == "openai"
            ),
            "exact_model": (
                environment.get(
                    "STRAYLIGHT_EMBEDDING_MODEL",
                    E03_EMBEDDING_MODELS["mode2"],
                )
                == E03_EMBEDDING_MODELS["mode2"]
            ),
            "exact_dimensions": (
                environment.get(
                    "STRAYLIGHT_EMBEDDING_DIMENSIONS",
                    str(E03_EMBEDDING_DIMENSIONS),
                )
                == str(E03_EMBEDDING_DIMENSIONS)
            ),
            "dummy_inline_key": key_value.startswith(
                ("mock-", "dummy-", "test-")
            ),
            "no_key_file": not environment.get("OPENAI_API_KEY_FILE"),
        }
        return {
            "container_id": record.get("Id"),
            "image_id": record.get("Image"),
            "image_revision": labels.get(
                "org.opencontainers.image.revision"
            ),
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
            "endpoint_sha256": hashlib.sha256(
                args.expected_openai_base_url.encode()
            ).hexdigest(),
            "credential": {
                "dummy_inline_key": checks["dummy_inline_key"],
                "key_file_configured": not checks["no_key_file"],
                "configuration_sha256": credential_fingerprint,
                "values_recorded": False,
            },
            "checks": checks,
            "pass": all(checks.values()),
        }

    api = inspect(args.api_container, "api")
    worker = inspect(args.worker_container, "worker")
    completed = run_command(["docker", "inspect", args.db_container], timeout=10.0)
    if completed.returncode != 0:
        raise RuntimeError("could not inspect Mode 2 db container")
    try:
        db_record = json.loads(completed.stdout)[0]
        db_labels = db_record["Config"].get("Labels") or {}
        db_running = db_record["State"].get("Running") is True
    except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise RuntimeError("invalid Mode 2 db inspection") from error
    db = {
        "container_id": db_record.get("Id"),
        "image_id": db_record.get("Image"),
        "compose_project": db_labels.get("com.docker.compose.project"),
        "compose_service": db_labels.get("com.docker.compose.service"),
        "running": db_running,
        "pass": bool(
            db_running
            and db_labels.get("com.docker.compose.service") == "db"
            and db_record.get("Image") == args.expect_db_image_id
        ),
    }
    value = {
        "api": api,
        "worker": worker,
        "db": db,
        "same_endpoint": (
            api["endpoint_sha256"] == worker["endpoint_sha256"]
        ),
        "same_credential_configuration": (
            api["credential"]["configuration_sha256"]
            == worker["credential"]["configuration_sha256"]
        ),
        "same_compose_project": bool(
            api["compose_project"]
            and api["compose_project"] == worker["compose_project"]
            and api["compose_project"] == db["compose_project"]
        ),
    }
    value["pass"] = bool(
        api["pass"]
        and worker["pass"]
        and db["pass"]
        and value["same_endpoint"]
        and value["same_credential_configuration"]
        and value["same_compose_project"]
    )
    if not value["pass"]:
        raise RuntimeError(
            "Mode 2 API/worker are not bound to the same owned mock endpoint "
            "with dummy non-production credentials"
        )
    return value


def build_performance_command(args: argparse.Namespace) -> list[str]:
    failure = mock_command(args, "configure")
    failure[-1] = "503"
    restore = mock_command(args, "configure")
    command = [
        sys.executable,
        str(PERFORMANCE),
        "run",
        "--gate-profile",
        "e03-semantic-ready",
        "--e03-arm",
        "mode2",
        "--protocol",
        "simple",
        "--retrieval-modes",
        "exact",
        "lexical",
        "semantic",
        "--wait-semantic",
        "--semantic-failure-probe",
        "required",
        "--expect-feature-flag",
        "semantic_lane=on",
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
    for name, value in sorted(E03_COMMON_RUNTIME_EXPECTATIONS.items()):
        if type(value) is bool:
            command.extend([
                "--expect-feature-flag",
                f"{name}={'on' if value else 'off'}",
            ])
        else:
            command.extend([
                "--expect-runtime-config",
                f"{name}={json.dumps(value, separators=(',', ':'))}",
            ])
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
    if getattr(args, "worker_container", None):
        command.extend(["--worker-container", args.worker_container])
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
    parser.add_argument("--api-container", required=True)
    parser.add_argument("--db-container", required=True)
    parser.add_argument("--worker-container", required=True)
    parser.add_argument("--expect-build-revision", required=True)
    parser.add_argument("--expect-api-image-id", required=True)
    parser.add_argument("--expect-db-image-id", required=True)
    parser.add_argument("--expected-openai-base-url", required=True)
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
    if args.quick or args.samples != 30:
        raise ValueError("definitive E03 Mode 2 requires exactly 30 samples")
    before_code, before = status(args)
    if before_code == 0 or before.get("live"):
        raise RuntimeError(
            "refusing to adopt an already-running mock; use a run-unique "
            "port and state path so teardown cannot invalidate another run"
        )
    endpoint_binding = endpoint_contract(args)
    performance_command = build_performance_command(args)
    started: subprocess.CompletedProcess[str] | None = None
    performance: subprocess.CompletedProcess[str] | None = None
    after_start: dict[str, Any] = {}
    after_restore: dict[str, Any] = {}
    after_stop: dict[str, Any] = {}
    artifact_binding: dict[str, Any] = {"pass": False, "checks": {}}
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
        artifact_binding = artifact_contract(args.out, args)
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
        "artifact_binding": artifact_binding,
        "endpoint_binding": endpoint_binding,
    }
    orchestration["pass"] = bool(
        started
        and started.returncode == 0
        and orchestration_error is None
        and performance
        and performance.returncode == 0
        and after_start.get("behavior")
        == {"delay_ms": 0, "error_status": 0}
        and after_restore.get("behavior")
        == {"delay_ms": 0, "error_status": 0}
        and stop_result
        and stop_result.returncode == 0
        and not after_stop.get("live", True)
        and artifact_binding.get("pass") is True
        and endpoint_binding.get("pass") is True
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

#!/usr/bin/env python3

"""Run the current-build E03 Mode 1 baseline with no semantic worker."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))
from performance_eval import E03_COMMON_RUNTIME_EXPECTATIONS  # noqa: E402
from performance_eval import E03_EMBEDDING_DIMENSIONS  # noqa: E402

PERFORMANCE = PROJECT_ROOT / "performance_eval.py"
SCHEMA = "brunn-e03-mode1-orchestration@v2"


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


def command_fingerprint(command: list[str]) -> dict[str, Any]:
    rendered = json.dumps(command, separators=(",", ":"))
    return {
        "executable": Path(command[0]).name,
        "argv_sha256": hashlib.sha256(rendered.encode()).hexdigest(),
        "argument_count": len(command),
    }


def stack_contract(args: argparse.Namespace) -> dict[str, Any]:
    def inspect(name: str, service: str) -> dict[str, Any]:
        completed = run_command(["docker", "inspect", name], timeout=10.0)
        if completed.returncode != 0:
            raise RuntimeError(f"could not inspect Mode 1 {service}")
        try:
            record = json.loads(completed.stdout)[0]
            labels = record["Config"].get("Labels") or {}
            env_rows = record["Config"].get("Env") or []
        except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
            raise RuntimeError(f"invalid Mode 1 {service} inspection") from error
        environment = {}
        for row in env_rows:
            if isinstance(row, str) and "=" in row:
                key, value = row.split("=", 1)
                environment[key] = value
        return {
            "container_id": record.get("Id"),
            "image_id": record.get("Image"),
            "started_at": record["State"].get("StartedAt"),
            "running": record["State"].get("Running") is True,
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
            "image_revision": labels.get("org.opencontainers.image.revision"),
            "embedding_provider": environment.get(
                "BRUNN_EMBEDDING_PROVIDER",
                "openai",
            ),
            "embedding_dimensions": environment.get(
                "BRUNN_EMBEDDING_DIMENSIONS",
                str(E03_EMBEDDING_DIMENSIONS),
            ),
            "provider_credential": {
                "inline_key_configured": bool(
                    environment.get("OPENAI_API_KEY")
                ),
                "key_file_configured": bool(
                    environment.get("OPENAI_API_KEY_FILE")
                ),
                "values_recorded": False,
            },
        }

    api = inspect(args.api_container, "api")
    db = inspect(args.db_container, "db")
    project = api.get("compose_project")
    workers = run_command(
        [
            "docker",
            "ps",
            "-q",
            "--filter",
            f"label=com.docker.compose.project={project}",
            "--filter",
            "label=com.docker.compose.service=worker",
        ],
        timeout=10.0,
    )
    worker_ids = [
        row for row in workers.stdout.splitlines() if row.strip()
    ] if workers.returncode == 0 else ["inspection-failed"]
    checks = {
        "same_isolated_project": bool(
            project and project == db.get("compose_project")
        ),
        "api_service": (
            api["compose_service"] == "api" and api["running"] is True
        ),
        "db_service": (
            db["compose_service"] == "db" and db["running"] is True
        ),
        "exact_revision": api["image_revision"] == args.expect_build_revision,
        "exact_api_image": api["image_id"] == args.expect_api_image_id,
        "exact_db_image": db["image_id"] == args.expect_db_image_id,
        "hashing_provider": api["embedding_provider"] == "hashing",
        "exact_embedding_dimensions": (
            api["embedding_dimensions"] == str(E03_EMBEDDING_DIMENSIONS)
        ),
        "no_external_provider_credential": (
            not api["provider_credential"]["inline_key_configured"]
            and not api["provider_credential"]["key_file_configured"]
        ),
        "worker_absent": not worker_ids,
    }
    value = {
        "api": api,
        "db": db,
        "running_worker_count": len(worker_ids),
        "checks": checks,
        "pass": all(checks.values()),
    }
    if not value["pass"]:
        raise RuntimeError(
            "Mode 1 preflight requires same-project API/DB, hashing provider, "
            "no external provider credential, exact revision, and no worker"
        )
    return value


def build_performance_command(args: argparse.Namespace) -> list[str]:
    command = [
        sys.executable,
        str(PERFORMANCE),
        "run",
        "--gate-profile",
        "e03-semantic-ready",
        "--e03-arm",
        "mode1",
        "--protocol",
        "simple",
        "--retrieval-modes",
        "exact",
        "lexical",
        "--semantic-failure-probe",
        "not-applicable",
        "--verbatim-feature-acceptance",
        "not-applicable",
        "--query-budget-profile",
        "default-safe",
        "--label",
        args.label,
        "--api-container",
        args.api_container,
        "--db-container",
        args.db_container,
        "--expect-build-revision",
        args.expect_build_revision,
        "--expect-feature-flag",
        "semantic_lane=off",
        "--out",
        str(args.out),
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
    if args.reuse_flat_controls_from:
        command.extend([
            "--reuse-flat-controls-from",
            str(args.reuse_flat_controls_from),
        ])
    return command


def artifact_contract(path: Path, args: argparse.Namespace) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        value = {}
    fingerprint = value.get("implementation_fingerprint")
    fingerprint = fingerprint if isinstance(fingerprint, dict) else {}
    scales = value.get("scales") if isinstance(value.get("scales"), list) else []
    checks = {
        "artifact_pass": value.get("pass") is True,
        "profile_and_arm": (
            value.get("gate_profile") == "e03-semantic-ready"
            and value.get("e03_arm") == "mode1"
        ),
        "revision": (
            fingerprint.get("source_revision") == args.expect_build_revision
            and fingerprint.get("api_image_revision")
            == args.expect_build_revision
        ),
        "pending_proof": bool(
            scales
            and all(
                isinstance(item, dict)
                and isinstance(item.get("e03_mode1_pending"), dict)
                and item["e03_mode1_pending"].get("pass") is True
                for item in scales
            )
        ),
        "exact_sample_count": (
            value.get("run_profile", {}).get("samples_per_retrieval") == 30
        ),
    }
    return {"checks": checks, "pass": all(checks.values())}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", required=True)
    parser.add_argument("--api-container", required=True)
    parser.add_argument("--db-container", required=True)
    parser.add_argument("--expect-build-revision", required=True)
    parser.add_argument("--expect-api-image-id", required=True)
    parser.add_argument("--expect-db-image-id", required=True)
    parser.add_argument("--scales", type=int, nargs="+")
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--future-soak", action="store_true")
    parser.add_argument("--reuse-flat-controls-from", type=Path)
    parser.add_argument("--timeout", type=float, default=14_400.0)
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.timeout <= 0:
        raise ValueError("--timeout must be positive")
    if args.quick or args.samples != 30:
        raise ValueError("definitive E03 Mode 1 requires exactly 30 samples")
    preflight = stack_contract(args)
    command = build_performance_command(args)
    try:
        completed = run_command(command, timeout=args.timeout)
    except (OSError, subprocess.TimeoutExpired) as error:
        completed = None
        orchestration_error = {
            "type": type(error).__name__,
            "message": str(error),
        }
    else:
        orchestration_error = None
    artifact = artifact_contract(args.out, args)
    orchestration = {
        "schema": SCHEMA,
        "preflight": preflight,
        "performance": {
            "exit_code": completed.returncode if completed else None,
            **command_fingerprint(command),
        },
        "artifact_binding": artifact,
        "performance_harness_sha256": hashlib.sha256(
            PERFORMANCE.read_bytes()
        ).hexdigest(),
        "wrapper_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "error": orchestration_error,
    }
    orchestration["pass"] = bool(
        preflight["pass"]
        and completed
        and completed.returncode == 0
        and artifact["pass"]
        and orchestration_error is None
    )
    try:
        result = json.loads(args.out.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        result = {"schema": "brunn-performance-eval@v2", "pass": False}
    result["mode1_orchestration"] = orchestration
    result["pass"] = bool(result.get("pass")) and orchestration["pass"]
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": "ok" if result["pass"] else "failed",
        "out": str(args.out),
        "mode1_orchestration": orchestration,
    }, indent=2))
    return 0 if result["pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())

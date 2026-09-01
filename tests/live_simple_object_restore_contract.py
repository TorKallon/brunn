#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any


class ContractFailure(RuntimeError):
    pass


def load_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.split("=", 1)
        values[name] = value
    return values


def run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int = 120,
) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if result.returncode != 0:
        raise ContractFailure(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stderr[-1500:]}"
        )
    return result.stdout.strip()


def json_line(output: str) -> dict[str, Any]:
    for line in reversed(output.splitlines()):
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            return value
    raise ContractFailure(f"command returned no JSON object: {output[-1000:]}")


def compose(
    root: Path,
    env_file: Path,
    project: str,
    arguments: list[str],
    *,
    env: dict[str, str],
    timeout: int = 120,
) -> str:
    return run(
        [
            "docker",
            "compose",
            "-p",
            project,
            "--env-file",
            str(env_file),
            *arguments,
        ],
        cwd=root,
        env=env,
        timeout=timeout,
    )


def database_object_versions(
    root: Path,
    env_file: Path,
    project: str,
    *,
    env: dict[str, str],
) -> list[str]:
    output = compose(
        root,
        env_file,
        project,
        [
            "exec",
            "-T",
            "db",
            "psql",
            "-U",
            "admin",
            "-d",
            env.get("POSTGRES_DB", "brunn"),
            "-Atc",
            (
                "SELECT object_key || '|' || object_version_id "
                "FROM brunn.entry_versions "
                "WHERE object_version_id IS NOT NULL ORDER BY object_key"
            ),
        ],
        env=env,
    )
    return [line for line in output.splitlines() if "|" in line]


def run_contract(
    root: Path,
    env_file: Path,
    project: str,
) -> dict[str, Any]:
    if not any(marker in project.lower() for marker in ("test", "contract", "benchmark")):
        raise ContractFailure(
            "refusing database remap outside a test, contract, or benchmark Compose project"
        )

    started = time.monotonic()
    loaded = load_env(env_file)
    env = {**os.environ, **loaded}
    root_user = loaded.get("MINIO_ROOT_USER")
    root_password = loaded.get("MINIO_ROOT_PASSWORD")
    if not root_user or not root_password:
        raise ContractFailure("MINIO root credentials are required for the restore drill")

    original_versions = database_object_versions(
        root, env_file, project, env=env
    )
    if not original_versions:
        raise ContractFailure(
            "the disposable database has no simple workspace binary references to remap"
        )

    source_verification = json_line(
        compose(
            root,
            env_file,
            project,
            [
                "run",
                "--rm",
                "-T",
                "migrate",
                "object-store-backup",
                "verify-database",
            ],
            env=env,
        )
    )

    run_id = uuid.uuid4().hex[:12]
    target_bucket = f"brunn-restore-contract-{run_id}"
    with tempfile.TemporaryDirectory(
        prefix="brunn-simple-object-restore-"
    ) as temporary:
        drill_root = Path(temporary)
        export_root = drill_root / "export"
        mapping = drill_root / "restore-map.json"
        compose(
            root,
            env_file,
            project,
            [
                "run",
                "--rm",
                "-T",
                "--volume",
                f"{drill_root}:/drill",
                "migrate",
                "object-store-backup",
                "export",
                "--output",
                "/drill/export",
            ],
            env=env,
        )
        if not export_root.is_dir():
            raise ContractFailure("object-store export directory was not created")

        bucket_env = {
            **env,
            "TARGET_BUCKET": target_bucket,
        }
        compose(
            root,
            env_file,
            project,
            [
                "run",
                "--rm",
                "-T",
                "--entrypoint",
                "/bin/sh",
                "-e",
                "TARGET_BUCKET",
                "minio-init",
                "-c",
                (
                    'mc alias set drill http://minio:9000 "$MINIO_ROOT_USER" '
                    '"$MINIO_ROOT_PASSWORD" >/dev/null && '
                    'mc mb "drill/$TARGET_BUCKET" >/dev/null && '
                    'mc version enable "drill/$TARGET_BUCKET" >/dev/null'
                ),
            ],
            env=bucket_env,
        )

        restore_env = {
            **env,
            "BRUNN_S3_BUCKET": target_bucket,
            "BRUNN_MINIO_BUCKET": target_bucket,
            "BRUNN_S3_ACCESS_KEY": root_user,
            "BRUNN_S3_SECRET_KEY": root_password,
            "BRUNN_MINIO_ACCESS_KEY": root_user,
            "BRUNN_MINIO_SECRET_KEY": root_password,
        }
        restore_overrides = [
            "-e",
            "BRUNN_S3_BUCKET",
            "-e",
            "BRUNN_MINIO_BUCKET",
            "-e",
            "BRUNN_S3_ACCESS_KEY",
            "-e",
            "BRUNN_S3_SECRET_KEY",
            "-e",
            "BRUNN_MINIO_ACCESS_KEY",
            "-e",
            "BRUNN_MINIO_SECRET_KEY",
        ]
        restore_summary = json_line(
            compose(
                root,
                env_file,
                project,
                [
                    "run",
                    "--rm",
                    "-T",
                    "--volume",
                    f"{drill_root}:/drill",
                    *restore_overrides,
                    "migrate",
                    "object-store-backup",
                    "restore",
                    "--input",
                    "/drill/export",
                    "--mapping",
                    "/drill/restore-map.json",
                ],
                env=restore_env,
            )
        )
        if not mapping.is_file():
            raise ContractFailure("object-store restore map was not created")

        remap_summary = json_line(
            compose(
                root,
                env_file,
                project,
                [
                    "run",
                    "--rm",
                    "-T",
                    "--volume",
                    f"{drill_root}:/drill",
                    *restore_overrides,
                    "migrate",
                    "object-store-backup",
                    "remap-database",
                    "--input",
                    "/drill/export",
                    "--mapping",
                    "/drill/restore-map.json",
                ],
                env=restore_env,
            )
        )
        target_verification = json_line(
            compose(
                root,
                env_file,
                project,
                [
                    "run",
                    "--rm",
                    "-T",
                    *restore_overrides,
                    "migrate",
                    "object-store-backup",
                    "verify-database",
                ],
                env=restore_env,
            )
        )

    restored_versions = database_object_versions(
        root, env_file, project, env=env
    )
    expected = len(original_versions)
    if len(restored_versions) != expected:
        raise ContractFailure(
            f"binary reference count changed from {expected} to {len(restored_versions)}"
        )
    if original_versions == restored_versions:
        raise ContractFailure("database object version IDs were not remapped")
    if remap_summary.get("workspace_entry_versions_updated") != expected:
        raise ContractFailure(
            "database remap did not report every simple workspace binary reference"
        )
    if target_verification.get("references_verified") != expected:
        raise ContractFailure(
            "restored object store did not verify every simple workspace reference"
        )

    return {
        "schema": "brunn-simple-object-restore-contract@v1",
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "checks": {
            "source_references_verified": source_verification.get(
                "references_verified"
            ),
            "restored_object_versions": restore_summary.get("object_versions"),
            "workspace_entry_versions_remapped": remap_summary.get(
                "workspace_entry_versions_updated"
            ),
            "restored_references_verified": target_verification.get(
                "references_verified"
            ),
            "version_ids_changed": True,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
    )
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument("--project", required=True)
    args = parser.parse_args()
    try:
        result = run_contract(
            args.root.resolve(),
            args.env_file.resolve(),
            args.project,
        )
    except (
        ContractFailure,
        OSError,
        subprocess.SubprocessError,
        ValueError,
    ) as error:
        result = {
            "schema": "brunn-simple-object-restore-contract@v1",
            "status": "fail",
            "error": str(error),
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

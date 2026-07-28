"""Secret-free provenance for external evaluation fault-injection hooks."""

from __future__ import annotations

import hashlib
import json
import re
import shlex
import subprocess
import time
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parents[1]
ATTESTATION_SCHEMA = "straylight-eval-hook-attestation@v1"
ATTESTATION_FIELDS = (
    "schema",
    "pass",
    "action",
    "mode",
    "instance_id",
    "implementation_sha256",
    "upstream_base_url_sha256",
    "configuration_revision",
    "control",
)
TARGET_BINDING_FIELDS = (
    "instance_id",
    "implementation_sha256",
    "upstream_base_url_sha256",
)
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
INSTANCE_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _checked_in_asset(token: str) -> dict[str, str] | None:
    try:
        path = Path(token).resolve(strict=True)
    except (OSError, RuntimeError):
        return None
    if (
        not path.is_file()
        or not path.is_relative_to(PROJECT_ROOT)
        or path.suffix.casefold() not in {".py", ".sh"}
    ):
        return None
    return {
        "path": str(path.relative_to(PROJECT_ROOT)),
        "sha256": sha256_bytes(path.read_bytes()),
    }


def command_fingerprint(argv: list[str]) -> dict[str, Any]:
    canonical = json.dumps(
        argv,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    assets = []
    for token in argv[:2]:
        asset = _checked_in_asset(token)
        if asset is not None and asset not in assets:
            assets.append(asset)
    return {
        "executable": Path(argv[0]).name,
        "argv_sha256": sha256_bytes(canonical),
        "argument_count": len(argv),
        "checked_in_assets": assets,
    }


def parse_hook_attestation(stdout: bytes) -> dict[str, Any] | None:
    """Return only the fixed, non-secret attestation contract.

    Hook stdout is otherwise discarded. This prevents an accidental provider
    credential, request body, or arbitrary command output from entering a
    definitive result artifact.
    """

    for raw_line in reversed(stdout.splitlines()):
        if not raw_line.strip():
            continue
        try:
            value = json.loads(raw_line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if not isinstance(value, dict):
            continue
        if value.get("schema") != ATTESTATION_SCHEMA:
            continue
        public = {
            field: value.get(field)
            for field in ATTESTATION_FIELDS
            if field in value
        }
        if (
            public.get("pass") is not True
            or public.get("action") != "configure"
            or public.get("mode") not in {"forward", "slow", "error"}
            or not isinstance(public.get("instance_id"), str)
            or not INSTANCE_PATTERN.fullmatch(public["instance_id"])
            or not isinstance(public.get("implementation_sha256"), str)
            or not SHA256_PATTERN.fullmatch(public["implementation_sha256"])
            or not isinstance(public.get("upstream_base_url_sha256"), str)
            or not SHA256_PATTERN.fullmatch(
                public["upstream_base_url_sha256"]
            )
            or type(public.get("configuration_revision")) is not int
            or public["configuration_revision"] < 1
            or public.get("control") not in {"local", "authenticated"}
        ):
            return None
        return public
    return None


def hook_target_matches(
    injected: dict[str, Any],
    restored: dict[str, Any],
) -> bool:
    """Require both successful hooks to control one monotonically revised proxy."""

    injected_attestation = injected.get("attestation")
    restored_attestation = restored.get("attestation")
    return bool(
        injected.get("pass") is True
        and restored.get("pass") is True
        and isinstance(injected_attestation, dict)
        and isinstance(restored_attestation, dict)
        and all(
            injected_attestation.get(field)
            == restored_attestation.get(field)
            for field in TARGET_BINDING_FIELDS
        )
        and type(injected_attestation.get("configuration_revision")) is int
        and type(restored_attestation.get("configuration_revision")) is int
        and restored_attestation["configuration_revision"]
        > injected_attestation["configuration_revision"]
    )


def run_hook(
    command_text: str,
    *,
    timeout_seconds: float,
    require_attestation: bool = False,
    expected_mode: str | None = None,
) -> dict[str, Any]:
    started = time.monotonic()
    try:
        argv = shlex.split(command_text)
    except ValueError:
        return {
            "pass": False,
            "error": "invalid_command_syntax",
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    if not argv:
        return {
            "pass": False,
            "error": "empty_command",
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    fingerprint = command_fingerprint(argv)
    try:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=timeout_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return {
            **fingerprint,
            "pass": False,
            "exit_code": None,
            "error": "timeout",
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    except OSError as error:
        return {
            **fingerprint,
            "pass": False,
            "exit_code": None,
            "error": type(error).__name__,
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    attestation = parse_hook_attestation(completed.stdout)
    attestation_valid = (
        attestation is not None
        and (
            expected_mode is None
            or attestation.get("mode") == expected_mode
        )
    )
    passed = (
        completed.returncode == 0
        and (not require_attestation or attestation_valid)
    )
    result = {
        **fingerprint,
        "pass": passed,
        "exit_code": completed.returncode,
        "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        "attestation_required": require_attestation,
        "attestation": attestation,
    }
    if completed.returncode == 0 and require_attestation and not attestation_valid:
        result["error"] = (
            "missing_or_mismatched_hook_attestation"
            if attestation is None or expected_mode is not None
            else "invalid_hook_attestation"
        )
    return result

#!/usr/bin/env python3

"""Run E03 Mode 3 once with an owned real-provider fault proxy."""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import os
import re
import shlex
import subprocess
import sys
import urllib.parse
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))
from performance_eval import (  # noqa: E402
    E03_COMMON_RUNTIME_EXPECTATIONS,
    E03_EMBEDDING_DIMENSIONS,
    E03_EMBEDDING_MODELS,
    E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS,
    E03_WRAPPER_TIMEOUT_SECONDS,
)

PERFORMANCE = PROJECT_ROOT / "performance_eval.py"
PROXY = PROJECT_ROOT / "eval" / "openai_embedding_fault_proxy.py"
SCHEMA = "straylight-e03-mode3-orchestration@v1"
PROXY_STATE_SCHEMA = "straylight-openai-embedding-fault-proxy-state@v1"
PROXY_IDENTITY_SCHEMA = (
    "straylight-openai-embedding-fault-proxy-identity@v1"
)
PROXY_USAGE_SCHEMA = "straylight-openai-embedding-usage-receipt@v1"
OFFICIAL_OPENAI_BASE_URL = "https://api.openai.com/v1"
DEFAULT_PROBE_IMAGE = (
    "busybox@sha256:"
    "fd8d9aa63ba2f0982b5304e1ee8d3b90a210bc1ffb5314d980eb6962f1a9715d"
)
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


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


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def command_fingerprint(command: list[str]) -> dict[str, Any]:
    return {
        "executable": Path(command[0]).name,
        "argv_sha256": sha256_text(json.dumps(command, separators=(",", ":"))),
        "argument_count": len(command),
    }


def normalize_official_upstream(value: str) -> str:
    parsed = urllib.parse.urlsplit(value.rstrip("/"))
    normalized = urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, parsed.path.rstrip("/"), "", "")
    )
    if (
        normalized != OFFICIAL_OPENAI_BASE_URL
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(
            "definitive E03 Mode 3 requires exact upstream "
            f"{OFFICIAL_OPENAI_BASE_URL}"
        )
    return normalized


def container_host_route_allowed(value: str) -> bool:
    if value == "host.docker.internal":
        return True
    try:
        ipaddress.ip_address(value)
    except ValueError:
        return False
    return True


def probe_image_contract(image: str) -> dict[str, Any]:
    digest = image.rpartition("@sha256:")[2]
    pinned_reference = bool(SHA256_PATTERN.fullmatch(digest))
    pinned_id = bool(
        image.startswith("sha256:")
        and SHA256_PATTERN.fullmatch(image.removeprefix("sha256:"))
    )
    if not (pinned_reference or pinned_id):
        raise ValueError("--probe-image must be pinned by digest or image ID")
    completed = run_command(["docker", "image", "inspect", image], timeout=10.0)
    if completed.returncode != 0:
        raise RuntimeError(
            "the pinned Mode 3 probe image must already exist locally"
        )
    try:
        record = json.loads(completed.stdout)[0]
        image_id = str(record["Id"])
        repo_digests = record.get("RepoDigests") or []
    except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise RuntimeError("invalid Mode 3 probe image inspection") from error
    exact = image_id == image if pinned_id else image in repo_digests
    result = {
        "requested": image,
        "image_id": image_id,
        "locally_present": True,
        "pull_policy": "never",
        "exact_pinned_reference": exact,
        "pass": exact,
    }
    if not result["pass"]:
        raise RuntimeError("local probe image does not match its exact pin")
    return result


def container_endpoint_contract(args: argparse.Namespace) -> dict[str, Any]:
    parsed = urllib.parse.urlsplit(args.expected_proxy_base_url)
    endpoint_host = str(parsed.hostname or "")
    try:
        endpoint_port = parsed.port
    except ValueError as error:
        raise ValueError(
            "--expected-proxy-base-url contains an invalid port"
        ) from error
    docker_desktop_host = endpoint_host == "host.docker.internal"
    if (
        parsed.scheme != "http"
        or endpoint_port != args.proxy_port
        or parsed.path.rstrip("/") != "/v1"
        or not container_host_route_allowed(endpoint_host)
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(
            "--expected-proxy-base-url must be an HTTP /v1 URL whose port "
            "equals --proxy-port and whose host is either the exact Docker "
            "gateway or host.docker.internal"
        )

    def inspect(name: str, service: str) -> dict[str, Any]:
        completed = run_command(["docker", "inspect", name], timeout=10.0)
        if completed.returncode != 0:
            raise RuntimeError(f"could not inspect Mode 3 {service}")
        try:
            record = json.loads(completed.stdout)[0]
            labels = record["Config"].get("Labels") or {}
            rows = record["Config"].get("Env") or []
            state = record["State"]
            networks = record["NetworkSettings"].get("Networks") or {}
        except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
            raise RuntimeError(f"invalid Mode 3 {service} inspection") from error
        environment = {}
        for row in rows:
            if isinstance(row, str) and "=" in row:
                key, value = row.split("=", 1)
                environment[key] = value
        inline = environment.get("OPENAI_API_KEY", "")
        key_file = environment.get("OPENAI_API_KEY_FILE", "")
        credential_fingerprint = sha256_text(
            ("inline\0" + inline) if inline else ("file\0" + key_file)
        )
        checks = {
            "service": labels.get("com.docker.compose.service") == service,
            "running": state.get("Running") is True,
            "provider": (
                environment.get("STRAYLIGHT_EMBEDDING_PROVIDER", "openai")
                == "openai"
            ),
            "exact_model": (
                environment.get(
                    "STRAYLIGHT_EMBEDDING_MODEL",
                    E03_EMBEDDING_MODELS["mode3"],
                )
                == E03_EMBEDDING_MODELS["mode3"]
            ),
            "exact_dimensions": (
                environment.get(
                    "STRAYLIGHT_EMBEDDING_DIMENSIONS",
                    str(E03_EMBEDDING_DIMENSIONS),
                )
                == str(E03_EMBEDDING_DIMENSIONS)
            ),
            "endpoint": (
                environment.get("OPENAI_BASE_URL")
                == args.expected_proxy_base_url
            ),
            "exact_api_image": (
                record.get("Image") == args.expect_api_image_id
            ),
            "credential_configured": bool(inline or key_file),
            "one_credential_source": bool(inline) != bool(key_file),
            "credential_not_obviously_dummy": not inline.startswith(
                ("mock-", "dummy-", "test-")
            ),
        }
        return {
            "container_id": record.get("Id"),
            "image_id": record.get("Image"),
            "started_at": record["State"].get("StartedAt"),
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
            "image_revision": labels.get("org.opencontainers.image.revision"),
            "networks": networks,
            "endpoint_sha256": sha256_text(args.expected_proxy_base_url),
            "credential": {
                "configured": checks["credential_configured"],
                "inline": bool(inline),
                "file": bool(key_file),
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
        raise RuntimeError("could not inspect Mode 3 db")
    try:
        db_record = json.loads(completed.stdout)[0]
        db_labels = db_record["Config"].get("Labels") or {}
        db_state = db_record["State"]
        db_networks = db_record["NetworkSettings"].get("Networks") or {}
    except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise RuntimeError("invalid Mode 3 db inspection") from error
    db = {
        "container_id": db_record.get("Id"),
        "image_id": db_record.get("Image"),
        "compose_project": db_labels.get("com.docker.compose.project"),
        "compose_service": db_labels.get("com.docker.compose.service"),
        "networks": db_networks,
        "running": db_state.get("Running") is True,
    }
    api_network_names = sorted(api["networks"])
    worker_network_names = sorted(worker["networks"])
    db_network_names = sorted(db["networks"])
    network_name = (
        api_network_names[0]
        if (
            len(api_network_names) == 1
            and api_network_names == worker_network_names == db_network_names
        )
        else None
    )
    gateways = {
        str(item["networks"][network_name].get("Gateway") or "")
        for item in (api, worker, db)
    } if network_name else set()
    exact_gateway = (
        len(gateways) == 1
        and endpoint_host == next(iter(gateways))
    )
    exact_container_route = exact_gateway or docker_desktop_host
    probe_image = probe_image_contract(args.probe_image)
    value = {
        "api": api,
        "worker": worker,
        "db": db,
        "probe_image": probe_image,
        "same_endpoint": api["endpoint_sha256"] == worker["endpoint_sha256"],
        "same_credential_configuration": (
            api["credential"]["configuration_sha256"]
            == worker["credential"]["configuration_sha256"]
        ),
        "same_project": bool(
            api["compose_project"]
            and api["compose_project"] == worker["compose_project"]
            and api["compose_project"] == db["compose_project"]
        ),
        "same_single_network": network_name is not None,
        "network": network_name,
        "exact_gateway": exact_gateway,
        "docker_desktop_host": docker_desktop_host,
        "exact_container_route": exact_container_route,
        "exact_revision": (
            api["image_revision"] == args.expect_build_revision
            and worker["image_revision"] == args.expect_build_revision
        ),
        "exact_images": bool(
            api["image_id"] == worker["image_id"] == args.expect_api_image_id
            and db["image_id"] == args.expect_db_image_id
        ),
        "db_service_running": bool(
            db["compose_service"] == "db" and db["running"] is True
        ),
    }
    value["pass"] = bool(
        api["pass"]
        and worker["pass"]
        and value["db_service_running"]
        and value["same_endpoint"]
        and value["same_credential_configuration"]
        and value["same_project"]
        and value["same_single_network"]
        and value["exact_container_route"]
        and value["exact_revision"]
        and value["exact_images"]
        and probe_image["pass"]
    )
    for item in (api, worker, db):
        item.pop("networks", None)
    if not value["pass"]:
        raise RuntimeError(
            "Mode 3 API/worker must share one real-provider proxy endpoint, "
            "container host route, credential posture, Compose project, and "
            "exact images"
        )
    return value


def proxy_identity_probe(
    args: argparse.Namespace,
    endpoint: dict[str, Any],
    *,
    role: str,
) -> dict[str, Any]:
    target = endpoint[role]
    parsed = urllib.parse.urlsplit(args.expected_proxy_base_url)
    identity_url = urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, "/__straylight/identity", "", "")
    )
    command = [
        "docker",
        "run",
        "--rm",
        "--pull",
        "never",
        "--network",
        f"container:{target['container_id']}",
        "--read-only",
        "--user",
        "65534:65534",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges:true",
        "--pids-limit",
        "32",
        "--memory",
        "32m",
        "--entrypoint",
        "/bin/wget",
        args.probe_image,
        "-q",
        "-T",
        "5",
        "-O",
        "-",
        identity_url,
    ]
    completed = run_command(command, timeout=15.0)
    try:
        value = json.loads(completed.stdout)
        state = value.get("state") if isinstance(value, dict) else None
        state = state if isinstance(state, dict) else {}
    except json.JSONDecodeError:
        value = {}
        state = {}
    checks = {
        "probe_exit_zero": completed.returncode == 0,
        "identity_schema": value.get("schema") == PROXY_IDENTITY_SCHEMA,
        "instance": state.get("instance_id") == args.proxy_instance_id,
        "implementation": (
            state.get("implementation_sha256")
            == args.proxy_implementation_sha256
        ),
        "upstream": (
            state.get("upstream_base_url_sha256")
            == args.upstream_base_url_sha256
        ),
        "listener": (
            state.get("host") == "0.0.0.0"
            and state.get("port") == args.proxy_port
        ),
    }
    result = {
        "role": role,
        "target_container_id": target["container_id"],
        "probe_image_id": endpoint["probe_image"]["image_id"],
        "endpoint_sha256": sha256_text(args.expected_proxy_base_url),
        "command": command_fingerprint(command),
        "checks": checks,
        "pass": all(checks.values()),
    }
    if not result["pass"]:
        raise RuntimeError(
            f"Mode 3 {role} network namespace cannot attest the owned proxy"
        )
    return result


def process_is_live(pid: int | None) -> bool:
    if type(pid) is not int or pid <= 1:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def owned_proxy_state(args: argparse.Namespace) -> dict[str, Any]:
    if not args.proxy_state.exists():
        return {
            "state_exists": False,
            "bound": False,
            "owned": False,
            "live": False,
            "checks": {},
        }
    try:
        value = json.loads(args.proxy_state.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {
            "state_exists": True,
            "bound": False,
            "owned": False,
            "live": None,
            "checks": {"valid_json": False},
        }
    pid = value.get("pid")
    live = process_is_live(pid)
    checks = {
        "schema": value.get("schema") == PROXY_STATE_SCHEMA,
        "instance": value.get("instance_id") == args.proxy_instance_id,
        "implementation": (
            value.get("implementation_sha256")
            == args.proxy_implementation_sha256
        ),
        "upstream": (
            value.get("upstream_base_url_sha256")
            == args.upstream_base_url_sha256
        ),
        "listener": (
            value.get("host") == "0.0.0.0"
            and value.get("port") == args.proxy_port
        ),
        "config_path": (
            value.get("config") == str(args.proxy_config.resolve())
        ),
        "live_process": live,
    }
    binding_checks = {
        key: value for key, value in checks.items() if key != "live_process"
    }
    bound = all(binding_checks.values())
    return {
        "state_exists": True,
        "pid": pid if type(pid) is int else None,
        "bound": bound,
        "owned": bound and live,
        "live": live,
        "checks": checks,
    }


def usage_receipt_contract(
    completed: subprocess.CompletedProcess[str],
    args: argparse.Namespace,
    artifact: dict[str, Any],
) -> dict[str, Any]:
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError:
        value = {}
    state = value.get("state")
    state = state if isinstance(state, dict) else {}
    behavior = value.get("behavior")
    behavior = behavior if isinstance(behavior, dict) else {}
    usage = value.get("usage")
    usage = usage if isinstance(usage, dict) else {}
    cost = artifact.get("e03_embedding_cost")
    cost = cost if isinstance(cost, dict) else {}
    preflight = cost.get("preflight")
    preflight = preflight if isinstance(preflight, dict) else {}
    prompt_tokens = usage.get("prompt_tokens")
    total_tokens = usage.get("total_tokens")
    successful = usage.get("successful_embedding_responses")
    reported = usage.get("usage_reported_responses")
    missing = usage.get("usage_missing_responses")
    actual_usd = (
        round(prompt_tokens / 1_000_000 * 0.02, 6)
        if type(prompt_tokens) is int and prompt_tokens >= 0
        else None
    )
    estimated_max = preflight.get("estimated_max_usd")
    checks = {
        "status_exit_zero": completed.returncode == 0,
        "bound_instance": (
            state.get("instance_id") == args.proxy_instance_id
            and state.get("implementation_sha256")
            == args.proxy_implementation_sha256
            and state.get("upstream_base_url_sha256")
            == args.upstream_base_url_sha256
        ),
        "restored_forward": (
            behavior.get("mode") == "forward"
            and behavior.get("delay_ms") == 0
            and behavior.get("error_status") == 0
        ),
        "usage_schema": usage.get("schema") == PROXY_USAGE_SCHEMA,
        "complete_usage_receipt": bool(
            type(successful) is int
            and successful > 0
            and reported == successful
            and missing == 0
            and type(prompt_tokens) is int
            and prompt_tokens > 0
            and type(total_tokens) is int
            and total_tokens >= prompt_tokens
        ),
        "actual_within_preflight": bool(
            isinstance(actual_usd, (int, float))
            and isinstance(estimated_max, (int, float))
            and actual_usd <= estimated_max
        ),
        "actual_within_hard_ceiling": bool(
            isinstance(actual_usd, (int, float)) and actual_usd <= 5.0
        ),
    }
    return {
        "schema": PROXY_USAGE_SCHEMA,
        "successful_embedding_responses": successful,
        "usage_reported_responses": reported,
        "usage_missing_responses": missing,
        "prompt_tokens": prompt_tokens,
        "total_tokens": total_tokens,
        "usd_per_million_tokens": 0.02,
        "actual_usd": actual_usd,
        "preflight_max_usd": estimated_max,
        "checks": checks,
        "pass": all(checks.values()),
    }


def proxy_command(args: argparse.Namespace, action: str) -> list[str]:
    base = [sys.executable, str(PROXY), action]
    if action == "start":
        base.extend([
            "--host", "0.0.0.0",
            "--port", str(args.proxy_port),
            "--state", str(args.proxy_state),
            "--config", str(args.proxy_config),
            "--log", str(args.proxy_log),
            "--instance-id", args.proxy_instance_id,
            "--upstream-base-url", args.upstream_base_url,
        ])
    else:
        base.extend([
            "--state", str(args.proxy_state),
            "--instance-id", args.proxy_instance_id,
            "--expect-implementation-sha256", args.proxy_implementation_sha256,
            "--expect-upstream-base-url-sha256", args.upstream_base_url_sha256,
        ])
    if action in {"start", "configure", "status"} and args.control_token_file:
        base.extend(["--control-token-file", str(args.control_token_file)])
    if action == "configure":
        base.extend(["--mode", "error", "--error-status", "503"])
    return base


def restore_command(args: argparse.Namespace) -> list[str]:
    command = proxy_command(args, "configure")
    mode_index = command.index("--mode") + 1
    command[mode_index] = "forward"
    status_index = command.index("--error-status")
    del command[status_index:status_index + 2]
    return command


def build_performance_command(args: argparse.Namespace) -> list[str]:
    command = [
        sys.executable,
        str(PERFORMANCE),
        "run",
        "--gate-profile",
        "e03-semantic-ready",
        "--e03-arm",
        "mode3",
        "--protocol",
        "simple",
        "--retrieval-modes",
        "exact",
        "lexical",
        "semantic",
        "--wait-semantic",
        "--semantic-failure-probe",
        "required",
        "--verbatim-feature-acceptance",
        "not-applicable",
        "--semantic-failure-start-command",
        shlex.join(proxy_command(args, "configure")),
        "--semantic-failure-stop-command",
        shlex.join(restore_command(args)),
        "--require-semantic-failure-hook-attestation",
        "--query-budget-profile",
        "default-safe",
        "--label",
        args.label,
        "--scales",
        "64000",
        "--samples",
        str(args.samples),
        "--api-container",
        args.api_container,
        "--db-container",
        args.db_container,
        "--worker-container",
        args.worker_container,
        "--expect-build-revision",
        args.expect_build_revision,
        "--expect-feature-flag",
        "semantic_lane=on",
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
    return command


def artifact_contract(path: Path, args: argparse.Namespace) -> dict[str, Any]:
    try:
        artifact = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        artifact = {}
    scales = artifact.get("scales")
    scales = scales if isinstance(scales, list) else []
    pair = (
        scales[0].get("e03_mode3_paired_query", {})
        if len(scales) == 1 and isinstance(scales[0], dict)
        else {}
    )
    fingerprint = artifact.get("implementation_fingerprint")
    fingerprint = fingerprint if isinstance(fingerprint, dict) else {}
    cost = artifact.get("e03_embedding_cost")
    cost = cost if isinstance(cost, dict) else {}
    checks = {
        "artifact_pass": artifact.get("pass") is True,
        "profile_arm_scale": (
            artifact.get("gate_profile") == "e03-semantic-ready"
            and artifact.get("e03_arm") == "mode3"
            and [item.get("scale") for item in scales] == [64_000]
        ),
        "paired_single_corpus": (
            isinstance(pair, dict)
            and pair.get("status") == "complete"
            and pair.get("pass") is True
            and pair.get("single_provisioning_event") is True
            and pair.get("cold_before_warm") is True
            and pair.get("same_query_strings") is True
            and pair.get("cardinality", {}).get("stable") is True
        ),
        "exact_revision": (
            fingerprint.get("source_revision") == args.expect_build_revision
            and fingerprint.get("api_image_revision")
            == args.expect_build_revision
            and fingerprint.get("worker_image_revision")
            == args.expect_build_revision
        ),
        "exact_images": (
            fingerprint.get("api_image_id") == args.expect_api_image_id
            and fingerprint.get("worker_image_id")
            == args.expect_api_image_id
        ),
        "exact_sample_count": (
            artifact.get("run_profile", {}).get("samples_per_retrieval") == 30
        ),
        "cost_within_ceiling": (
            isinstance(cost.get("accounted_estimate_usd"), (int, float))
            and cost["accounted_estimate_usd"] <= 5.0
        ),
    }
    return {"checks": checks, "pass": all(checks.values())}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", required=True)
    parser.add_argument("--api-container", required=True)
    parser.add_argument("--db-container", required=True)
    parser.add_argument("--worker-container", required=True)
    parser.add_argument("--expect-build-revision", required=True)
    parser.add_argument("--expect-api-image-id", required=True)
    parser.add_argument("--expect-db-image-id", required=True)
    parser.add_argument("--expected-proxy-base-url", required=True)
    parser.add_argument("--proxy-port", type=int, required=True)
    parser.add_argument("--proxy-state", type=Path, required=True)
    parser.add_argument("--proxy-config", type=Path, required=True)
    parser.add_argument("--proxy-log", type=Path, required=True)
    parser.add_argument("--proxy-instance-id", required=True)
    parser.add_argument(
        "--upstream-base-url",
        default=OFFICIAL_OPENAI_BASE_URL,
    )
    parser.add_argument("--probe-image", default=DEFAULT_PROBE_IMAGE)
    parser.add_argument("--control-token-file", type=Path)
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument(
        "--timeout",
        type=float,
        default=E03_WRAPPER_TIMEOUT_SECONDS,
    )
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.samples != 30:
        raise ValueError("definitive E03 Mode 3 requires exactly 30 samples")
    if args.timeout < E03_WRAPPER_TIMEOUT_SECONDS:
        raise ValueError(
            "--timeout must preserve the E03 12-hour import boundary plus "
            "the 30-minute result-finalization margin"
        )
    if args.proxy_port < 1 or args.proxy_port > 65_535:
        raise ValueError("--proxy-port is out of range")
    if args.proxy_state.exists():
        raise RuntimeError(
            "refusing to adopt a preexisting proxy state; use a run-unique path"
        )
    args.proxy_implementation_sha256 = hashlib.sha256(PROXY.read_bytes()).hexdigest()
    args.upstream_base_url = normalize_official_upstream(
        args.upstream_base_url
    )
    args.upstream_base_url_sha256 = sha256_text(args.upstream_base_url)
    endpoint = container_endpoint_contract(args)
    performance_command = build_performance_command(args)
    start: subprocess.CompletedProcess[str] | None = None
    performance: subprocess.CompletedProcess[str] | None = None
    status_result: subprocess.CompletedProcess[str] | None = None
    stop: subprocess.CompletedProcess[str] | None = None
    ownership_after_start: dict[str, Any] = {}
    ownership_before_stop: dict[str, Any] = {}
    ownership_after_stop: dict[str, Any] = {}
    network_probes: list[dict[str, Any]] = []
    receipt: dict[str, Any] = {"pass": False, "checks": {}}
    artifact: dict[str, Any] = {"pass": False, "checks": {}}
    errors: list[dict[str, str]] = []
    try:
        start = run_command(proxy_command(args, "start"), timeout=15.0)
        ownership_after_start = owned_proxy_state(args)
        if start.returncode != 0 or not ownership_after_start.get("owned"):
            raise RuntimeError("owned Mode 3 fault proxy failed to start")
        network_probes = [
            proxy_identity_probe(args, endpoint, role=role)
            for role in ("api", "worker")
        ]
        performance = run_command(
            performance_command,
            timeout=args.timeout,
        )
        try:
            raw_artifact = json.loads(args.out.read_text(encoding="utf-8"))
        except (FileNotFoundError, json.JSONDecodeError):
            raw_artifact = {}
        status_result = run_command(
            proxy_command(args, "status"),
            timeout=10.0,
        )
        receipt = usage_receipt_contract(status_result, args, raw_artifact)
        artifact = artifact_contract(args.out, args)
    except (
        OSError,
        RuntimeError,
        ValueError,
        subprocess.TimeoutExpired,
    ) as error:
        errors.append({
            "type": type(error).__name__,
            "message": str(error),
        })
    finally:
        ownership_before_stop = owned_proxy_state(args)
        owned_pid = ownership_before_stop.get("pid")
        try:
            if ownership_before_stop.get("owned"):
                stop = run_command(proxy_command(args, "stop"), timeout=10.0)
            elif (
                ownership_before_stop.get("bound")
                and not ownership_before_stop.get("live")
            ):
                args.proxy_state.unlink(missing_ok=True)
            elif ownership_before_stop.get("state_exists"):
                raise RuntimeError(
                    "proxy state exists but is not bound to this invocation; "
                    "refusing unsafe teardown"
                )
        except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
            errors.append({
                "type": type(error).__name__,
                "message": str(error),
            })
        ownership_after_stop = owned_proxy_state(args)
        ownership_after_stop["recorded_pid_live"] = process_is_live(owned_pid)
        ownership_after_stop["cleanup_pass"] = bool(
            not ownership_after_stop.get("state_exists")
            and not ownership_after_stop["recorded_pid_live"]
            and (stop is None or stop.returncode == 0)
        )
    orchestration = {
        "schema": SCHEMA,
        "endpoint_binding": endpoint,
        "network_namespace_identity_probes": network_probes,
        "proxy": {
            "instance_id": args.proxy_instance_id,
            "implementation_sha256": args.proxy_implementation_sha256,
            "upstream_base_url_sha256": args.upstream_base_url_sha256,
            "official_upstream": args.upstream_base_url,
            "start_exit_code": start.returncode if start else None,
            "stop_exit_code": stop.returncode if stop else None,
            "ownership_after_start": ownership_after_start,
            "ownership_before_stop": ownership_before_stop,
            "ownership_after_stop": ownership_after_stop,
        },
        "performance": {
            "exit_code": performance.returncode if performance else None,
            **command_fingerprint(performance_command),
        },
        "embedding_usage_receipt": receipt,
        "artifact_binding": artifact,
        "errors": errors,
    }
    orchestration["pass"] = bool(
        endpoint["pass"]
        and start
        and start.returncode == 0
        and ownership_after_start.get("owned")
        and len(network_probes) == 2
        and all(item.get("pass") for item in network_probes)
        and performance
        and performance.returncode == 0
        and status_result
        and status_result.returncode == 0
        and receipt["pass"]
        and stop
        and stop.returncode == 0
        and ownership_after_stop.get("cleanup_pass")
        and artifact["pass"]
        and not errors
    )
    try:
        result = json.loads(args.out.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        result = {"schema": "straylight-performance-eval@v2", "pass": False}
    result["e03_embedding_usage_receipt"] = receipt
    cost = result.get("e03_embedding_cost")
    if isinstance(cost, dict):
        cost["actual_prompt_tokens"] = receipt.get("prompt_tokens")
        cost["actual_usd"] = receipt.get("actual_usd")
        cost["actual_vs_preflight_pass"] = receipt.get("pass")
    result["mode3_orchestration"] = orchestration
    result["pass"] = bool(result.get("pass")) and orchestration["pass"]
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": "ok" if result["pass"] else "failed",
        "out": str(args.out),
        "mode3_orchestration": orchestration,
    }, indent=2))
    return 0 if result["pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())

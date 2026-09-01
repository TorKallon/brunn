#!/usr/bin/env python3
"""Destructive dependency and runtime safety checks for a local Compose stack."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Sequence


class RuntimeSafetyFailure(RuntimeError):
    pass


def check(condition: bool, message: str, context: Any = None) -> None:
    if condition:
        return
    detail = "" if context is None else f"\ncontext={context!r}"
    raise RuntimeSafetyFailure(message + detail)


def command(arguments: list[str], *, capture: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        arguments,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeSafetyFailure(
            f"command failed ({result.returncode}): {' '.join(arguments)}\n"
            f"{(result.stderr or '').strip()}"
        )
    return result


class Compose:
    def __init__(self, root: Path, env_file: Path, project: str) -> None:
        self.project = project
        self.arguments = [
            "docker",
            "compose",
            "--project-name",
            project,
            "--env-file",
            str(env_file),
            "--file",
            str(root / "compose.yaml"),
        ]

    def run(self, *arguments: str, capture: bool = True) -> subprocess.CompletedProcess[str]:
        return command([*self.arguments, *arguments], capture=capture)

    def container(self, service: str) -> str:
        value = self.run("ps", "-q", service).stdout.strip()
        check(value, f"Compose service has no container: {service}")
        return value

    def network(self) -> str:
        return f"{self.project}_default"


def http(
    base_url: str,
    path: str,
    *,
    headers: dict[str, str] | None = None,
    expected: set[int] | None = None,
    timeout: float = 10,
) -> tuple[int, dict[str, str], dict[str, Any]]:
    request = urllib.request.Request(base_url.rstrip("/") + path, headers=headers or {})
    try:
        response = urllib.request.urlopen(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    status = response.status
    response_headers = {key.casefold(): value for key, value in response.headers.items()}
    raw = response.read()
    try:
        body = json.loads(raw)
    except json.JSONDecodeError as error:
        raise RuntimeSafetyFailure(
            f"{path} returned non-JSON HTTP {status}: {raw[:500]!r}"
        ) from error
    expected = expected or {200}
    check(status in expected, f"{path} returned unexpected HTTP {status}", body)
    check(isinstance(body, dict), f"{path} returned a non-object", body)
    return status, response_headers, body


def wait_ready(
    base_url: str,
    wanted_status: int,
    timeout: float,
    dependency: str | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: tuple[int, dict[str, Any]] | None = None
    while time.monotonic() < deadline:
        try:
            status, _, body = http(
                base_url,
                "/ready",
                expected={200, 503},
                timeout=5,
            )
            last = (status, body)
            if status == wanted_status:
                if dependency is not None:
                    dependencies = body.get("dependencies", {})
                    if dependencies.get(dependency) != "unavailable":
                        time.sleep(0.5)
                        continue
                return body
        except (OSError, RuntimeSafetyFailure):
            pass
        time.sleep(0.5)
    raise RuntimeSafetyFailure(
        f"/ready did not reach HTTP {wanted_status} for {dependency}; last={last!r}"
    )


def wait_service_healthy(compose: Compose, service: str, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        container = compose.container(service)
        last = command(
            [
                "docker",
                "inspect",
                "--format",
                "{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}",
                container,
            ]
        ).stdout.strip()
        if last == "healthy":
            return
        time.sleep(0.5)
    raise RuntimeSafetyFailure(f"{service} did not become healthy; last={last}")


def assert_runtime_limits(compose: Compose, service: str, hardened: bool) -> None:
    container = compose.container(service)
    raw = command(["docker", "inspect", container]).stdout
    detail = json.loads(raw)[0]
    host = detail["HostConfig"]
    check(host["Memory"] > 0, f"{service} has no memory limit")
    check(host["NanoCpus"] > 0, f"{service} has no CPU limit")
    check(host["PidsLimit"] > 0, f"{service} has no PID limit")
    if hardened:
        check("ALL" in (host.get("CapDrop") or []), f"{service} does not drop capabilities")
        check(
            "no-new-privileges:true" in (host.get("SecurityOpt") or []),
            f"{service} lacks no-new-privileges",
        )


def container_ip(container: str) -> str:
    value = command(
        [
            "docker",
            "inspect",
            "--format",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            container,
        ]
    ).stdout.strip()
    check(value, f"container has no network address: {container}")
    return value


def run(args: argparse.Namespace) -> None:
    root = Path(__file__).resolve().parents[1]
    compose = Compose(root, args.env_file, args.project)
    stopped: set[str] = set()
    try:
        wait_ready(args.base_url, 200, args.timeout)
        _, headers, body = http(
            args.base_url,
            "/v1/me",
            headers={
                "X-Request-Id": "alpha-runtime-correlation",
                "Origin": "https://untrusted.example",
            },
            expected={401},
        )
        check(
            headers.get("x-request-id") == "alpha-runtime-correlation",
            "response did not preserve the validated request ID",
            headers,
        )
        check(
            body.get("request_id") == "alpha-runtime-correlation",
            "error envelope did not reuse the response request ID",
            body,
        )
        check(
            "access-control-allow-origin" not in headers,
            "deny-by-default CORS reflected an untrusted origin",
            headers,
        )

        for service, dependency in [("minio", "object_store"), ("db", "database")]:
            print(f"[runtime-safety] stopping {service}", flush=True)
            compose.run("stop", "--timeout", "30", service, capture=False)
            stopped.add(service)
            wait_ready(args.base_url, 503, args.timeout, dependency)
            http(args.base_url, "/health")
            compose.run("start", service, capture=False)
            stopped.remove(service)
            wait_service_healthy(compose, service, args.timeout)
            wait_ready(args.base_url, 200, args.timeout)

        worker = compose.container("worker")
        command(["docker", "kill", worker], capture=False)
        compose.run("up", "-d", "worker", capture=False)
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            current = compose.container("worker")
            state = command(
                ["docker", "inspect", "--format", "{{.State.Running}}", current]
            ).stdout.strip()
            if state == "true":
                break
            time.sleep(0.5)
        else:
            raise RuntimeSafetyFailure("worker did not recover after forced termination")

        old_api = compose.container("api")
        old_api_ip = container_ip(old_api)
        occupier = f"{args.project}-dns-occupier"
        print("[runtime-safety] forcing API address change", flush=True)
        compose.run("stop", "--timeout", "30", "api", capture=False)
        compose.run("rm", "--force", "api", capture=False)
        command(
            [
                "docker",
                "run",
                "--detach",
                "--rm",
                "--network",
                compose.network(),
                "--name",
                occupier,
                "alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40",
                "sleep",
                "180",
            ],
            capture=False,
        )
        try:
            compose.run("up", "-d", "--no-deps", "api", capture=False)
            wait_service_healthy(compose, "api", args.timeout)
            new_api_ip = container_ip(compose.container("api"))
            check(
                new_api_ip != old_api_ip,
                "API address did not change during DNS recovery test",
                {"before": old_api_ip, "after": new_api_ip},
            )
            deadline = time.monotonic() + args.timeout
            last_error: Exception | None = None
            while time.monotonic() < deadline:
                try:
                    _, _, proxy_health = http(
                        args.web_base_url,
                        "/api/health",
                        timeout=5,
                    )
                    check(
                        proxy_health.get("status") == "ok",
                        "web proxy returned an invalid API health envelope",
                        proxy_health,
                    )
                    break
                except (OSError, RuntimeSafetyFailure) as error:
                    last_error = error
                    time.sleep(0.5)
            else:
                raise RuntimeSafetyFailure(
                    f"web proxy did not recover after API address change: {last_error}"
                )
        finally:
            command(["docker", "stop", occupier], capture=False)

        for service in ["db", "minio", "api", "worker", "web"]:
            assert_runtime_limits(
                compose,
                service,
                hardened=service in {"api", "worker", "web"},
            )
        wait_ready(args.base_url, 200, args.timeout)
    finally:
        for service in sorted(stopped):
            try:
                compose.run("start", service, capture=False)
            except RuntimeSafetyFailure as error:
                print(f"[runtime-safety] recovery failed: {error}", file=sys.stderr)
        try:
            compose.run("up", "-d", "minio-init", "api", "worker", "web", capture=False)
        except RuntimeSafetyFailure as error:
            print(f"[runtime-safety] final recovery failed: {error}", file=sys.stderr)

    print(
        "[runtime-safety] PASS: request correlation, CORS, dependency readiness, "
        "worker recovery, proxy DNS recovery, and container limits"
    )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    result.add_argument("--base-url", default="http://127.0.0.1:18110")
    result.add_argument("--web-base-url", default="http://127.0.0.1:13110")
    result.add_argument(
        "--env-file",
        type=Path,
        default=Path(__file__).resolve().parents[1] / ".env",
    )
    result.add_argument("--project", default="brunn")
    result.add_argument("--timeout", type=float, default=120)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        run(args)
        return 0
    except RuntimeSafetyFailure as error:
        print(f"[runtime-safety] FAIL: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("[runtime-safety] INTERRUPTED", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())

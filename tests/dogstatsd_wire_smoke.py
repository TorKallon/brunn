#!/usr/bin/env python3
"""Prove the production binary emits tagged DogStatsD distributions over UDP."""

from __future__ import annotations

import argparse
import os
import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path


EXPECTED_TYPES = {
    "brunn.runtime.starts": "c",
    "brunn.runtime.alive": "g",
    "brunn.db.pool.connect.duration_ms": "d",
    "brunn.http.requests": "c",
    "brunn.http.request.duration_ms": "d",
    "datadog.dogstatsd.client.packets_sent": "c",
}
EXPECTED_TAGS = {
    "env:wire-test",
    "service:brunn",
    "version:wire-test",
    "component:api",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--env-file", default=".env")
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--dogstatsd-host", default="host.docker.internal")
    parser.add_argument(
        "--forward-to",
        help="also relay captured packets to HOST:PORT, for example 127.0.0.1:8125",
    )
    return parser.parse_args()


def metric_parts(line: str) -> tuple[str, str, set[str]] | None:
    fields = line.strip().split("|")
    if len(fields) < 2 or ":" not in fields[0]:
        return None
    name = fields[0].split(":", 1)[0]
    metric_type = fields[1]
    tags: set[str] = set()
    for field in fields[2:]:
        if field.startswith("#"):
            tags.update(field[1:].split(","))
    return name, metric_type, tags


def main() -> int:
    args = parse_args()
    repo = Path(__file__).resolve().parents[1]
    env_file = (repo / args.env_file).resolve()
    if not env_file.is_file():
        raise SystemExit(f"missing environment file: {env_file}")

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("0.0.0.0", 0))
    sock.settimeout(0.5)
    port = sock.getsockname()[1]
    forward_target = None
    forward_sock = None
    if args.forward_to:
        host, separator, target_port = args.forward_to.rpartition(":")
        if not separator or not host or not target_port.isdigit():
            raise SystemExit("--forward-to must use HOST:PORT")
        forward_target = (host, int(target_port))
        forward_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as http_sock:
        http_sock.bind(("127.0.0.1", 0))
        http_port = http_sock.getsockname()[1]
    container_name = f"brunn-dogstatsd-wire-smoke-{os.getpid()}"
    command = [
        "docker",
        "compose",
        "--env-file",
        str(env_file),
        "-f",
        str(repo / "compose.yaml"),
        "run",
        "--rm",
        "--no-deps",
        "--name",
        container_name,
        "--publish",
        f"127.0.0.1:{http_port}:8080",
        "-e",
        "BRUNN_METRICS_ENABLED=true",
        "-e",
        f"BRUNN_DOGSTATSD_ADDR={args.dogstatsd_host}:{port}",
        "-e",
        "BRUNN_METRICS_FLUSH_SECONDS=1",
        "-e",
        "DD_ENV=wire-test",
        "-e",
        "DD_SERVICE=brunn",
        "-e",
        "DD_VERSION=wire-test",
        "api",
        "serve",
    ]
    process = subprocess.Popen(
        command,
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    observed: dict[str, tuple[str, set[str]]] = {}
    all_names: set[str] = set()
    deadline = time.monotonic() + args.timeout
    requested_health = False
    try:
        while time.monotonic() < deadline and set(observed) != set(EXPECTED_TYPES):
            if process.poll() is not None:
                output = process.stdout.read() if process.stdout else ""
                raise RuntimeError(
                    f"instrumented API exited with {process.returncode}:\n{output[-4000:]}"
                )
            if not requested_health:
                try:
                    with urllib.request.urlopen(
                        f"http://127.0.0.1:{http_port}/health", timeout=0.5
                    ) as response:
                        requested_health = response.status == 200
                except (OSError, urllib.error.URLError):
                    pass
            try:
                packet, _ = sock.recvfrom(65_535)
            except TimeoutError:
                continue
            if forward_sock is not None and forward_target is not None:
                forward_sock.sendto(packet, forward_target)
            for line in packet.decode("utf-8", errors="replace").splitlines():
                parsed = metric_parts(line)
                if parsed is None:
                    continue
                name, metric_type, tags = parsed
                all_names.add(name)
                if name in EXPECTED_TYPES:
                    observed[name] = (metric_type, tags)

        missing = set(EXPECTED_TYPES) - set(observed)
        if missing:
            exporter_names = sorted(
                name for name in all_names if "dogstatsd.client" in name
            )
            raise RuntimeError(
                f"timed out waiting for metrics: {sorted(missing)}; "
                f"exporter telemetry observed: {exporter_names}"
            )
        for name, expected_type in EXPECTED_TYPES.items():
            metric_type, tags = observed[name]
            if metric_type != expected_type:
                raise RuntimeError(
                    f"{name} used DogStatsD type {metric_type!r}, expected {expected_type!r}"
                )
            missing_tags = EXPECTED_TAGS - tags
            if missing_tags:
                raise RuntimeError(f"{name} lacks tags: {sorted(missing_tags)}")
        for name in ("brunn.http.requests", "brunn.http.request.duration_ms"):
            _, tags = observed[name]
            expected_http_tags = {"method:GET", "route:/health", "status_code:200"}
            missing_tags = expected_http_tags - tags
            if missing_tags:
                raise RuntimeError(f"{name} lacks HTTP tags: {sorted(missing_tags)}")
    finally:
        sock.close()
        if forward_sock is not None:
            forward_sock.close()
        subprocess.run(
            ["docker", "rm", "-f", container_name],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)

    print(
        "DogStatsD wire smoke passed: "
        + ", ".join(f"{name}|{EXPECTED_TYPES[name]}" for name in sorted(observed))
    )
    print("Unified tags passed: " + ", ".join(sorted(EXPECTED_TAGS)))
    if forward_target is not None:
        print(f"Captured packets relayed to {forward_target[0]}:{forward_target[1]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import math
import os
import re
import signal
import subprocess
import sys
import time
import urllib.request
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


TOKEN = re.compile(r"[A-Za-z0-9_-]+")
DEFAULT_HOST = "0.0.0.0"
DEFAULT_PORT = 55200
DEFAULT_DIMENSIONS = 1536
DEFAULT_STATE = Path("/tmp/brunn-embedding-mock-55200.pid")
DEFAULT_LOG = Path("/tmp/brunn-embedding-mock-55200.log")
DEFAULT_CONFIG = Path("/tmp/brunn-embedding-mock-55200.json")
STATE_SCHEMA = "brunn-mock-openai-embeddings-state@v1"
HEALTH_SCHEMA = "brunn-mock-openai-embeddings-health@v1"
INSTANCE_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
CONTROL_PATH: Path | None = None
RUNTIME_IDENTITY: dict[str, Any] | None = None


def embedding(text: str, dimensions: int) -> list[float]:
    vector = [0.0] * dimensions
    for token in TOKEN.findall(text.casefold()):
        digest = hashlib.sha256(token.encode("utf-8")).digest()
        index = int.from_bytes(digest[:4], "big") % dimensions
        vector[index] += 1.0 if digest[4] & 1 else -1.0
    norm = math.sqrt(sum(value * value for value in vector))
    if norm:
        vector = [value / norm for value in vector]
    return vector


class Handler(BaseHTTPRequestHandler):
    server_version = "BrunnEmbeddingMock/1"

    def do_GET(self) -> None:
        if self.path == "/health":
            self._json(200, health_payload())
        else:
            self._json(404, {"error": "not_found"})

    def do_POST(self) -> None:
        if not self.path.endswith("/embeddings"):
            self._json(404, {"error": "not_found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            payload = json.loads(self.rfile.read(length))
            inputs = payload.get("input", [])
            if isinstance(inputs, str):
                inputs = [inputs]
            if not isinstance(inputs, list) or not all(
                isinstance(value, str) for value in inputs
            ):
                raise ValueError("input must be a string or string array")
            dimensions = int(payload.get("dimensions") or DEFAULT_DIMENSIONS)
            if dimensions < 1 or dimensions > 4096:
                raise ValueError("dimensions are out of range")
        except (ValueError, TypeError, json.JSONDecodeError) as error:
            self._json(400, {"error": {"message": str(error)}})
            return
        behavior = read_behavior()
        delay_ms = int(behavior["delay_ms"])
        if delay_ms:
            time.sleep(delay_ms / 1_000)
        error_status = int(behavior["error_status"])
        if error_status:
            self._json(
                error_status,
                {
                    "error": {
                        "message": "mock embedding provider failure",
                        "type": "mock_injected_failure",
                    }
                },
            )
            return
        self._json(
            200,
            {
                "object": "list",
                "model": payload.get("model", "mock-embedding"),
                "data": [
                    {
                        "object": "embedding",
                        "index": index,
                        "embedding": embedding(text, dimensions),
                    }
                    for index, text in enumerate(inputs)
                ],
                "usage": {
                    "prompt_tokens": sum(max(1, len(text) // 4) for text in inputs),
                    "total_tokens": sum(max(1, len(text) // 4) for text in inputs),
                },
            },
        )

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _json(self, status: int, value: Any) -> None:
        body = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def validate_behavior(delay_ms: int, error_status: int) -> dict[str, int]:
    if delay_ms < 0 or delay_ms > 60_000:
        raise ValueError("delay_ms must be between 0 and 60000")
    if error_status != 0 and not 400 <= error_status <= 599:
        raise ValueError("error_status must be zero or an HTTP 4xx/5xx status")
    return {"delay_ms": delay_ms, "error_status": error_status}


def implementation_sha256() -> str:
    return hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def config_path_sha256(path: Path) -> str:
    return hashlib.sha256(
        str(path.resolve()).encode("utf-8")
    ).hexdigest()


def health_payload() -> dict[str, Any]:
    identity = RUNTIME_IDENTITY or {}
    return {
        "schema": HEALTH_SCHEMA,
        "status": "ok",
        "behavior": read_behavior(),
        "identity": {
            key: identity.get(key)
            for key in (
                "instance_id",
                "implementation_sha256",
                "host",
                "port",
                "config_path_sha256",
                "pid",
            )
        },
    }


def write_behavior(path: Path, delay_ms: int, error_status: int) -> None:
    behavior = validate_behavior(delay_ms, error_status)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(behavior, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def read_behavior(path: Path | None = None) -> dict[str, int]:
    selected = path or CONTROL_PATH
    if selected is None:
        return validate_behavior(0, 0)
    try:
        value = json.loads(selected.read_text(encoding="utf-8"))
        return validate_behavior(
            int(value.get("delay_ms", 0)),
            int(value.get("error_status", 0)),
        )
    except (FileNotFoundError, ValueError, TypeError, json.JSONDecodeError):
        return validate_behavior(0, 503)


def serve(
    host: str,
    port: int,
    config: Path,
    instance_id: str,
    expect_implementation_sha256: str,
) -> int:
    global CONTROL_PATH, RUNTIME_IDENTITY
    actual_implementation = implementation_sha256()
    if not INSTANCE_PATTERN.fullmatch(instance_id):
        raise ValueError("invalid mock instance ID")
    if not hmac.compare_digest(
        actual_implementation,
        expect_implementation_sha256,
    ):
        raise ValueError("mock implementation hash mismatch")
    CONTROL_PATH = config
    RUNTIME_IDENTITY = {
        "instance_id": instance_id,
        "implementation_sha256": actual_implementation,
        "host": host,
        "port": port,
        "config_path_sha256": config_path_sha256(config),
        "pid": os.getpid(),
    }
    if not config.exists():
        write_behavior(config, 0, 0)
    server = ThreadingHTTPServer((host, port), Handler)
    server.serve_forever()
    return 0


def write_state(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(value, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.chmod(0o600)
    temporary.replace(path)
    path.chmod(0o600)


def read_state(path: Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError("invalid mock state file") from error
    if not isinstance(value, dict) or value.get("schema") != STATE_SCHEMA:
        raise ValueError("invalid mock state schema")
    if (
        type(value.get("pid")) is not int
        or value["pid"] <= 1
        or not INSTANCE_PATTERN.fullmatch(str(value.get("instance_id") or ""))
        or not re.fullmatch(
            r"[0-9a-f]{64}",
            str(value.get("implementation_sha256") or ""),
        )
        or type(value.get("port")) is not int
        or not 1 <= value["port"] <= 65_535
        or not re.fullmatch(
            r"[0-9a-f]{64}",
            str(value.get("config_path_sha256") or ""),
        )
    ):
        raise ValueError("invalid mock state binding")
    return value


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


def health(port: int) -> dict[str, Any] | None:
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/health",
            timeout=0.25,
        ) as response:
            if response.status != 200:
                return None
            payload = json.loads(response.read() or b"{}")
            return payload if isinstance(payload, dict) else None
    except (OSError, json.JSONDecodeError):
        return None


def identity_matches(
    observed: dict[str, Any] | None,
    state: dict[str, Any],
) -> bool:
    if not isinstance(observed, dict):
        return False
    identity = observed.get("identity")
    if (
        observed.get("schema") != HEALTH_SCHEMA
        or observed.get("status") != "ok"
        or not isinstance(identity, dict)
    ):
        return False
    return all(
        identity.get(key) == state.get(key)
        for key in (
            "instance_id",
            "implementation_sha256",
            "host",
            "port",
            "config_path_sha256",
            "pid",
        )
    )


def validate_expected_state(
    state: dict[str, Any],
    *,
    instance_id: str,
    expect_implementation_sha256: str,
    port: int,
    config: Path,
) -> None:
    expected = {
        "instance_id": instance_id,
        "implementation_sha256": expect_implementation_sha256,
        "port": port,
        "config_path_sha256": config_path_sha256(config),
    }
    mismatches = {
        key: {"expected": expected_value, "actual": state.get(key)}
        for key, expected_value in expected.items()
        if not hmac.compare_digest(
            str(state.get(key) or ""),
            str(expected_value),
        )
    }
    if mismatches:
        raise ValueError(f"mock state binding mismatch: {sorted(mismatches)}")


def start(
    state: Path,
    log: Path,
    config: Path,
    port: int,
    delay_ms: int,
    error_status: int,
    instance_id: str,
    expect_implementation_sha256: str,
) -> int:
    if state.exists():
        raise FileExistsError(
            "refusing to adopt a preexisting mock state file"
        )
    if not INSTANCE_PATTERN.fullmatch(instance_id):
        raise ValueError("invalid mock instance ID")
    actual_implementation = implementation_sha256()
    if not hmac.compare_digest(
        actual_implementation,
        expect_implementation_sha256,
    ):
        raise ValueError("mock implementation hash mismatch")
    if port < 1 or port > 65_535:
        raise ValueError("mock port is out of range")
    write_behavior(config, delay_ms, error_status)
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("ab") as output:
        process = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "serve",
                "--port",
                str(port),
                "--config",
                str(config),
                "--instance-id",
                instance_id,
                "--expect-implementation-sha256",
                expect_implementation_sha256,
            ],
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=output,
            start_new_session=True,
        )
    state_value = {
        "schema": STATE_SCHEMA,
        "pid": process.pid,
        "instance_id": instance_id,
        "implementation_sha256": actual_implementation,
        "host": DEFAULT_HOST,
        "port": port,
        "config_path_sha256": config_path_sha256(config),
        "started_at": datetime.now().astimezone().isoformat(
            timespec="seconds"
        ),
    }
    write_state(state, state_value)
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        observed = health(port)
        if (
            identity_matches(observed, state_value)
            and observed
            and observed.get("behavior")
            == validate_behavior(delay_ms, error_status)
        ):
            return 0
        time.sleep(0.05)
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5.0)
    state.unlink(missing_ok=True)
    return 1


def stop(
    state_path: Path,
    *,
    instance_id: str,
    expect_implementation_sha256: str,
    port: int,
    config: Path,
) -> int:
    state = read_state(state_path)
    if state is None:
        return 0
    validate_expected_state(
        state,
        instance_id=instance_id,
        expect_implementation_sha256=expect_implementation_sha256,
        port=port,
        config=config,
    )
    pid = state["pid"]
    if not process_is_live(pid):
        state_path.unlink(missing_ok=True)
        return 0
    if not identity_matches(health(port), state):
        raise RuntimeError(
            "refusing to stop a live process without exact mock identity"
        )
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if not process_is_live(pid):
            state_path.unlink(missing_ok=True)
            return 0
        time.sleep(0.05)
    os.kill(pid, signal.SIGKILL)
    state_path.unlink(missing_ok=True)
    return 0


def configure(
    state_path: Path,
    config: Path,
    port: int,
    delay_ms: int,
    error_status: int,
    instance_id: str,
    expect_implementation_sha256: str,
) -> int:
    state = read_state(state_path)
    if state is None:
        raise FileNotFoundError("mock state is missing")
    validate_expected_state(
        state,
        instance_id=instance_id,
        expect_implementation_sha256=expect_implementation_sha256,
        port=port,
        config=config,
    )
    if not process_is_live(state["pid"]):
        raise RuntimeError("bound mock process is not live")
    if not identity_matches(health(port), state):
        raise RuntimeError("bound mock identity is not healthy")
    write_behavior(config, delay_ms, error_status)
    deadline = time.monotonic() + 2.0
    expected = validate_behavior(delay_ms, error_status)
    while time.monotonic() < deadline:
        observed = health(port)
        if (
            identity_matches(observed, state)
            and observed
            and observed.get("behavior") == expected
        ):
            print(json.dumps(observed, sort_keys=True))
            return 0
        time.sleep(0.05)
    return 1


def status(
    state_path: Path,
    *,
    instance_id: str,
    expect_implementation_sha256: str,
    port: int,
    config: Path,
) -> int:
    state = read_state(state_path)
    if state is None:
        print(json.dumps({
            "state_exists": False,
            "live": False,
            "healthy": False,
            "port": port,
        }, sort_keys=True))
        return 1
    validate_expected_state(
        state,
        instance_id=instance_id,
        expect_implementation_sha256=expect_implementation_sha256,
        port=port,
        config=config,
    )
    observed = health(port)
    live = process_is_live(state["pid"])
    healthy = live and identity_matches(observed, state)
    value = {
        "state_exists": True,
        "pid": state["pid"],
        "live": live,
        "healthy": healthy,
        "behavior": (
            observed.get("behavior")
            if healthy and observed
            else read_behavior(config)
        ),
        "identity": observed.get("identity") if healthy and observed else None,
        "schema": observed.get("schema") if healthy and observed else None,
        "port": port,
    }
    print(json.dumps(value, sort_keys=True))
    return 0 if healthy else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "command",
        choices=("serve", "start", "stop", "status", "configure"),
    )
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--state", type=Path, default=DEFAULT_STATE)
    parser.add_argument("--log", type=Path, default=DEFAULT_LOG)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--delay-ms", type=int, default=0)
    parser.add_argument("--error-status", type=int, default=0)
    parser.add_argument("--instance-id", required=True)
    parser.add_argument("--expect-implementation-sha256", required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "serve":
        return serve(
            args.host,
            args.port,
            args.config,
            args.instance_id,
            args.expect_implementation_sha256,
        )
    if args.command == "start":
        return start(
            args.state,
            args.log,
            args.config,
            args.port,
            args.delay_ms,
            args.error_status,
            args.instance_id,
            args.expect_implementation_sha256,
        )
    if args.command == "stop":
        return stop(
            args.state,
            instance_id=args.instance_id,
            expect_implementation_sha256=(
                args.expect_implementation_sha256
            ),
            port=args.port,
            config=args.config,
        )
    if args.command == "configure":
        return configure(
            args.state,
            args.config,
            args.port,
            args.delay_ms,
            args.error_status,
            args.instance_id,
            args.expect_implementation_sha256,
        )
    return status(
        args.state,
        instance_id=args.instance_id,
        expect_implementation_sha256=args.expect_implementation_sha256,
        port=args.port,
        config=args.config,
    )


if __name__ == "__main__":
    raise SystemExit(main())

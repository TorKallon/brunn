#!/usr/bin/env python3

"""Per-stack OpenAI embedding proxy with secret-free fault controls.

The data plane forwards only OpenAI-compatible embedding requests. The control
plane accepts loopback callers without a token and otherwise requires a bearer
token loaded from a private file. Request bodies, response bodies, and
authorization values are never logged or written to state.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import ipaddress
import json
import os
import re
import signal
import stat as stat_module
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


SCHEMA = "brunn-openai-embedding-fault-proxy-state@v1"
CONFIG_SCHEMA = "brunn-openai-embedding-fault-proxy-config@v1"
ATTESTATION_SCHEMA = "brunn-eval-hook-attestation@v1"
IDENTITY_SCHEMA = "brunn-openai-embedding-fault-proxy-identity@v1"
USAGE_SCHEMA = "brunn-openai-embedding-usage-receipt@v1"
DEFAULT_HOST = "0.0.0.0"
DEFAULT_PORT = 55210
MAX_BODY_BYTES = 64 * 1024 * 1024
INSTANCE_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
STATE_PATH: Path | None = None
CONFIG_PATH: Path | None = None
UPSTREAM_BASE_URL = ""
CONTROL_TOKEN: bytes | None = None
CONFIG_LOCK = threading.Lock()
USAGE_LOCK = threading.Lock()
UPSTREAM_USAGE = {
    "schema": USAGE_SCHEMA,
    "successful_embedding_responses": 0,
    "usage_reported_responses": 0,
    "usage_missing_responses": 0,
    "prompt_tokens": 0,
    "total_tokens": 0,
}


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def implementation_sha256() -> str:
    return sha256_bytes(Path(__file__).read_bytes())


def validate_upstream_base_url(value: str, *, allow_http: bool) -> str:
    parsed = urllib.parse.urlsplit(value.rstrip("/"))
    if parsed.scheme not in ({"https", "http"} if allow_http else {"https"}):
        raise ValueError("upstream base URL must use HTTPS")
    if (
        not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(
            "upstream base URL must not contain credentials, query, or fragment"
        )
    return urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, parsed.path.rstrip("/"), "", "")
    )


def validate_behavior(
    mode: str,
    delay_ms: int,
    error_status: int,
) -> dict[str, Any]:
    if mode not in {"forward", "slow", "error"}:
        raise ValueError("mode must be forward, slow, or error")
    if delay_ms < 0 or delay_ms > 60_000:
        raise ValueError("delay_ms must be between 0 and 60000")
    if error_status != 0 and not 400 <= error_status <= 599:
        raise ValueError("error_status must be zero or an HTTP 4xx/5xx status")
    if mode == "forward" and (delay_ms != 0 or error_status != 0):
        raise ValueError("forward mode cannot inject delay or error")
    if mode == "slow" and (delay_ms <= 0 or error_status != 0):
        raise ValueError("slow mode requires a positive delay and no error")
    if mode == "error" and (delay_ms != 0 or error_status == 0):
        raise ValueError("error mode requires an HTTP error and no delay")
    return {
        "schema": CONFIG_SCHEMA,
        "mode": mode,
        "delay_ms": delay_ms,
        "error_status": error_status,
    }


def _private_file(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    descriptor = os.open(
        temporary,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
        0o600,
    )
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        temporary.replace(path)
        path.chmod(0o600)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def write_behavior(
    path: Path,
    *,
    instance_id: str,
    mode: str,
    delay_ms: int,
    error_status: int,
    revision: int,
) -> dict[str, Any]:
    if not INSTANCE_PATTERN.fullmatch(instance_id):
        raise ValueError("invalid proxy instance ID")
    if revision < 1:
        raise ValueError("configuration revision must be positive")
    value = {
        **validate_behavior(mode, delay_ms, error_status),
        "instance_id": instance_id,
        "revision": revision,
        "updated_at": datetime.now().astimezone().isoformat(timespec="seconds"),
    }
    _private_file(
        path,
        (json.dumps(value, sort_keys=True) + "\n").encode("utf-8"),
    )
    return value


def read_behavior(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        behavior = validate_behavior(
            str(value.get("mode")),
            int(value.get("delay_ms")),
            int(value.get("error_status")),
        )
        instance_id = str(value.get("instance_id") or "")
        revision = value.get("revision")
        if (
            not INSTANCE_PATTERN.fullmatch(instance_id)
            or type(revision) is not int
            or revision < 1
        ):
            raise ValueError("invalid proxy configuration identity")
        return {
            **behavior,
            "instance_id": instance_id,
            "revision": revision,
        }
    except (FileNotFoundError, ValueError, TypeError, json.JSONDecodeError):
        return {
            **validate_behavior("error", 0, 503),
            "instance_id": "invalid-config",
            "revision": 0,
        }


def read_token(path: Path | None) -> bytes | None:
    if path is None:
        return None
    stat = path.stat()
    if (
        not stat_module.S_ISREG(stat.st_mode)
        or stat.st_uid != os.getuid()
        or stat.st_mode & 0o077
    ):
        raise PermissionError("control token file must be owner-only")
    token = path.read_bytes().strip()
    if len(token) < 32:
        raise ValueError("control token must contain at least 32 bytes")
    try:
        rendered = token.decode("ascii")
    except UnicodeDecodeError as error:
        raise ValueError("control token must be printable ASCII") from error
    if (
        len(token) > 512
        or any(character.isspace() for character in rendered)
        or any(
            ord(character) < 33 or ord(character) > 126
            for character in rendered
        )
    ):
        raise ValueError(
            "control token must be 32-512 printable non-whitespace bytes"
        )
    return token


def open_private_append(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags, 0o600)
    stat = os.fstat(descriptor)
    if (
        not stat_module.S_ISREG(stat.st_mode)
        or stat.st_uid != os.getuid()
        or stat.st_mode & 0o077
    ):
        os.close(descriptor)
        raise PermissionError("proxy log must be an owner-only regular file")
    return os.fdopen(descriptor, "ab")


def public_state(value: dict[str, Any]) -> dict[str, Any]:
    allowed = {
        "schema",
        "pid",
        "instance_id",
        "host",
        "port",
        "implementation_sha256",
        "upstream_base_url_sha256",
        "config",
        "started_at",
        "control_token_required",
    }
    return {key: value[key] for key in allowed if key in value}


def identity_state(value: dict[str, Any]) -> dict[str, Any]:
    allowed = {
        "instance_id",
        "host",
        "port",
        "implementation_sha256",
        "upstream_base_url_sha256",
    }
    return {key: value[key] for key in allowed if key in value}


def parse_upstream_usage(body: bytes) -> tuple[int, int] | None:
    """Extract only billable token totals without retaining response content."""
    try:
        value = json.loads(body)
        usage = value.get("usage") if isinstance(value, dict) else None
        prompt_tokens = usage.get("prompt_tokens")
        total_tokens = usage.get("total_tokens")
    except (AttributeError, TypeError, json.JSONDecodeError):
        return None
    if (
        type(prompt_tokens) is not int
        or type(total_tokens) is not int
        or prompt_tokens < 0
        or total_tokens < prompt_tokens
    ):
        return None
    return prompt_tokens, total_tokens


def record_upstream_usage(body: bytes) -> None:
    parsed = parse_upstream_usage(body)
    with USAGE_LOCK:
        UPSTREAM_USAGE["successful_embedding_responses"] += 1
        if parsed is None:
            UPSTREAM_USAGE["usage_missing_responses"] += 1
            return
        prompt_tokens, total_tokens = parsed
        UPSTREAM_USAGE["usage_reported_responses"] += 1
        UPSTREAM_USAGE["prompt_tokens"] += prompt_tokens
        UPSTREAM_USAGE["total_tokens"] += total_tokens


def usage_receipt() -> dict[str, Any]:
    with USAGE_LOCK:
        return dict(UPSTREAM_USAGE)


def read_state(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if value.get("schema") != SCHEMA:
        raise ValueError("invalid proxy state schema")
    if not INSTANCE_PATTERN.fullmatch(str(value.get("instance_id") or "")):
        raise ValueError("invalid proxy state instance")
    if not SHA256_PATTERN.fullmatch(
        str(value.get("implementation_sha256") or "")
    ):
        raise ValueError("invalid proxy implementation fingerprint")
    if not SHA256_PATTERN.fullmatch(
        str(value.get("upstream_base_url_sha256") or "")
    ):
        raise ValueError("invalid proxy upstream fingerprint")
    return public_state(value)


def read_pid(path: Path) -> int | None:
    try:
        value = read_state(path).get("pid")
        return value if type(value) is int and value > 1 else None
    except (OSError, ValueError, TypeError, json.JSONDecodeError):
        return None


def process_is_live(pid: int | None) -> bool:
    if pid is None:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _is_loopback(address: str) -> bool:
    try:
        return ipaddress.ip_address(address).is_loopback
    except ValueError:
        return False


def _control_authorized(handler: BaseHTTPRequestHandler) -> tuple[bool, str]:
    if _is_loopback(handler.client_address[0]):
        return True, "local"
    if CONTROL_TOKEN is None:
        return False, "unauthorized"
    supplied = handler.headers.get("Authorization", "")
    prefix = "Bearer "
    if not supplied.startswith(prefix):
        return False, "unauthorized"
    return (
        hmac.compare_digest(
            supplied.removeprefix(prefix).encode("utf-8"),
            CONTROL_TOKEN,
        ),
        "authenticated",
    )


def forwarded_headers(headers: Any) -> dict[str, str]:
    allowed = {
        "authorization",
        "accept",
        "content-type",
        "openai-organization",
        "openai-project",
        "user-agent",
    }
    return {
        key: value
        for key, value in headers.items()
        if key.casefold() in allowed
    }


def _attestation(
    *,
    state: dict[str, Any],
    behavior: dict[str, Any],
    control: str,
) -> dict[str, Any]:
    return {
        "schema": ATTESTATION_SCHEMA,
        "pass": True,
        "action": "configure",
        "mode": behavior["mode"],
        "instance_id": state["instance_id"],
        "implementation_sha256": state["implementation_sha256"],
        "upstream_base_url_sha256": state["upstream_base_url_sha256"],
        "configuration_revision": behavior["revision"],
        "control": control,
    }


class Handler(BaseHTTPRequestHandler):
    server_version = "BrunnEmbeddingFaultProxy/1"

    def do_GET(self) -> None:
        if urllib.parse.urlsplit(self.path).path == "/__brunn/identity":
            assert STATE_PATH is not None
            self._json(
                200,
                {
                    "schema": IDENTITY_SCHEMA,
                    "state": identity_state(read_state(STATE_PATH)),
                },
            )
            return
        if self.path not in {
            "/__brunn/health",
            "/__brunn/status",
        }:
            self._json(404, {"error": "not_found"})
            return
        authorized, control = _control_authorized(self)
        if not authorized:
            self._json(403, {"error": "control_unauthorized"})
            return
        assert STATE_PATH is not None and CONFIG_PATH is not None
        state = read_state(STATE_PATH)
        behavior = read_behavior(CONFIG_PATH)
        self._json(
            200,
            {
                "status": "ok",
                "control": control,
                "state": state,
                "behavior": behavior,
                "usage": usage_receipt(),
            },
        )

    def do_POST(self) -> None:
        if self.path == "/__brunn/control":
            self._control()
            return
        if urllib.parse.urlsplit(self.path).path != "/v1/embeddings":
            self._json(404, {"error": "not_found"})
            return
        self._proxy_embedding()

    def _control(self) -> None:
        authorized, control = _control_authorized(self)
        if not authorized:
            self._json(403, {"error": "control_unauthorized"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length < 2 or length > 4_096:
                raise ValueError("invalid control request size")
            payload = json.loads(self.rfile.read(length))
            if not isinstance(payload, dict):
                raise ValueError("control payload must be an object")
            expected_instance = str(payload.get("instance_id") or "")
            assert STATE_PATH is not None and CONFIG_PATH is not None
            state = read_state(STATE_PATH)
            if not hmac.compare_digest(
                expected_instance,
                str(state["instance_id"]),
            ):
                raise ValueError("proxy instance mismatch")
            expected_implementation = str(
                payload.get("expect_implementation_sha256") or ""
            )
            expected_upstream = str(
                payload.get("expect_upstream_base_url_sha256") or ""
            )
            if not hmac.compare_digest(
                expected_implementation,
                str(state["implementation_sha256"]),
            ):
                raise ValueError("proxy implementation fingerprint mismatch")
            if not hmac.compare_digest(
                expected_upstream,
                str(state["upstream_base_url_sha256"]),
            ):
                raise ValueError("proxy upstream fingerprint mismatch")
            with CONFIG_LOCK:
                previous = read_behavior(CONFIG_PATH)
                behavior = write_behavior(
                    CONFIG_PATH,
                    instance_id=expected_instance,
                    mode=str(payload.get("mode") or ""),
                    delay_ms=int(payload.get("delay_ms") or 0),
                    error_status=int(payload.get("error_status") or 0),
                    revision=int(previous["revision"]) + 1,
                )
            self._json(
                200,
                _attestation(
                    state=state,
                    behavior=behavior,
                    control=control,
                ),
            )
        except (
            OSError,
            ValueError,
            TypeError,
            json.JSONDecodeError,
        ) as error:
            self._json(400, {"error": type(error).__name__})

    def _proxy_embedding(self) -> None:
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length < 1 or length > MAX_BODY_BYTES:
                raise ValueError("embedding request size is out of range")
            body = self.rfile.read(length)
        except (OSError, ValueError):
            self._json(400, {"error": {"message": "invalid request"}})
            return
        assert CONFIG_PATH is not None
        behavior = read_behavior(CONFIG_PATH)
        if behavior["mode"] == "slow":
            time.sleep(int(behavior["delay_ms"]) / 1_000)
        if behavior["mode"] == "error":
            self._json(
                int(behavior["error_status"]),
                {
                    "error": {
                        "message": "injected embedding provider failure",
                        "type": "brunn_fault_proxy",
                    }
                },
            )
            return
        target = f"{UPSTREAM_BASE_URL}/embeddings"
        request = urllib.request.Request(
            target,
            data=body,
            headers=forwarded_headers(self.headers),
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                response_body = response.read()
                record_upstream_usage(response_body)
                self.send_response(response.status)
                for name in (
                    "Content-Type",
                    "OpenAI-Organization",
                    "OpenAI-Processing-Ms",
                    "X-Request-Id",
                ):
                    value = response.headers.get(name)
                    if value:
                        self.send_header(name, value)
                self.send_header("Content-Length", str(len(response_body)))
                self.end_headers()
                self.wfile.write(response_body)
        except urllib.error.HTTPError as error:
            try:
                response_body = error.read()
            finally:
                error.close()
            self.send_response(error.code)
            content_type = error.headers.get("Content-Type")
            if content_type:
                self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(response_body)))
            self.end_headers()
            self.wfile.write(response_body)
        except (OSError, urllib.error.URLError):
            self._json(
                502,
                {
                    "error": {
                        "message": "embedding upstream unavailable",
                        "type": "brunn_fault_proxy",
                    }
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


def serve(
    host: str,
    port: int,
    state_path: Path,
    config_path: Path,
    upstream_base_url: str,
    control_token_file: Path | None,
    allow_http_upstream: bool,
) -> int:
    global STATE_PATH, CONFIG_PATH, UPSTREAM_BASE_URL, CONTROL_TOKEN
    STATE_PATH = state_path
    CONFIG_PATH = config_path
    UPSTREAM_BASE_URL = validate_upstream_base_url(
        upstream_base_url,
        allow_http=allow_http_upstream,
    )
    CONTROL_TOKEN = read_token(control_token_file)
    ThreadingHTTPServer((host, port), Handler).serve_forever()
    return 0


def _control_request(
    state: dict[str, Any],
    *,
    method: str,
    path: str,
    payload: dict[str, Any] | None,
    control_token_file: Path | None,
) -> dict[str, Any]:
    headers = {"Accept": "application/json"}
    token = read_token(control_token_file)
    if token is not None:
        headers["Authorization"] = f"Bearer {token.decode('utf-8')}"
    body = None
    if payload is not None:
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(
        f"http://127.0.0.1:{state['port']}{path}",
        data=body,
        headers=headers,
        method=method,
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        value = json.loads(response.read() or b"{}")
        if not isinstance(value, dict):
            raise ValueError("proxy control returned a non-object")
        return value


def validate_expected_state(
    state: dict[str, Any],
    *,
    instance_id: str,
    expect_implementation_sha256: str,
    expect_upstream_base_url_sha256: str,
) -> None:
    expected = {
        "instance_id": instance_id,
        "implementation_sha256": expect_implementation_sha256,
        "upstream_base_url_sha256": expect_upstream_base_url_sha256,
    }
    mismatches = {
        key: {"expected": value, "actual": state.get(key)}
        for key, value in expected.items()
        if not isinstance(value, str)
        or not value
        or not hmac.compare_digest(str(state.get(key) or ""), value)
    }
    if mismatches:
        raise ValueError(f"proxy state binding mismatch: {sorted(mismatches)}")
    if not process_is_live(state.get("pid")):
        raise RuntimeError("bound proxy process is not live")


def start(args: argparse.Namespace) -> int:
    upstream = validate_upstream_base_url(
        args.upstream_base_url,
        allow_http=args.allow_http_upstream,
    )
    if not INSTANCE_PATTERN.fullmatch(args.instance_id):
        raise ValueError("invalid proxy instance ID")
    if args.port < 1 or args.port > 65_535:
        raise ValueError("proxy port is out of range")
    if args.state.exists():
        existing = read_state(args.state)
        if process_is_live(existing.get("pid")):
            raise RuntimeError("refusing to adopt an existing proxy process")
        raise FileExistsError("remove the stale proxy state before starting")
    read_token(args.control_token_file)
    write_behavior(
        args.config,
        instance_id=args.instance_id,
        mode="forward",
        delay_ms=0,
        error_status=0,
        revision=1,
    )
    command = [
        sys.executable,
        str(Path(__file__).resolve()),
        "serve",
        "--host",
        args.host,
        "--port",
        str(args.port),
        "--state",
        str(args.state),
        "--config",
        str(args.config),
        "--upstream-base-url",
        upstream,
    ]
    if args.control_token_file is not None:
        command.extend(
            ["--control-token-file", str(args.control_token_file)]
        )
    if args.allow_http_upstream:
        command.append("--allow-http-upstream")
    with open_private_append(args.log) as output:
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=output,
            start_new_session=True,
        )
    state = {
        "schema": SCHEMA,
        "pid": process.pid,
        "instance_id": args.instance_id,
        "host": args.host,
        "port": args.port,
        "implementation_sha256": implementation_sha256(),
        "upstream_base_url_sha256": sha256_bytes(upstream.encode("utf-8")),
        "config": str(args.config.resolve()),
        "started_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "control_token_required": args.control_token_file is not None,
    }
    _private_file(
        args.state,
        (json.dumps(state, sort_keys=True) + "\n").encode("utf-8"),
    )
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        try:
            status = _control_request(
                state,
                method="GET",
                path="/__brunn/health",
                payload=None,
                control_token_file=args.control_token_file,
            )
            if status.get("status") == "ok":
                print(json.dumps(public_state(state), sort_keys=True))
                return 0
        except (OSError, ValueError, urllib.error.URLError):
            pass
        time.sleep(0.05)
    stop(args.state)
    return 1


def stop(
    state_path: Path,
    *,
    instance_id: str | None = None,
    expect_implementation_sha256: str | None = None,
    expect_upstream_base_url_sha256: str | None = None,
) -> int:
    state = read_state(state_path)
    expected = (
        instance_id,
        expect_implementation_sha256,
        expect_upstream_base_url_sha256,
    )
    if any(value is not None for value in expected):
        if not all(value is not None for value in expected):
            raise ValueError("all proxy stop bindings are required together")
        validate_expected_state(
            state,
            instance_id=str(instance_id),
            expect_implementation_sha256=str(
                expect_implementation_sha256
            ),
            expect_upstream_base_url_sha256=str(
                expect_upstream_base_url_sha256
            ),
        )
    pid = state.get("pid")
    state_path.unlink(missing_ok=True)
    if not process_is_live(pid):
        return 0
    assert pid is not None
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if not process_is_live(pid):
            return 0
        time.sleep(0.05)
    os.kill(pid, signal.SIGKILL)
    return 0


def configure(args: argparse.Namespace) -> int:
    state = read_state(args.state)
    validate_expected_state(
        state,
        instance_id=args.instance_id,
        expect_implementation_sha256=args.expect_implementation_sha256,
        expect_upstream_base_url_sha256=(
            args.expect_upstream_base_url_sha256
        ),
    )
    payload = {
        "instance_id": args.instance_id,
        "expect_implementation_sha256": args.expect_implementation_sha256,
        "expect_upstream_base_url_sha256": (
            args.expect_upstream_base_url_sha256
        ),
        "mode": args.mode,
        "delay_ms": args.delay_ms,
        "error_status": args.error_status,
    }
    value = _control_request(
        state,
        method="POST",
        path="/__brunn/control",
        payload=payload,
        control_token_file=args.control_token_file,
    )
    if (
        value.get("schema") != ATTESTATION_SCHEMA
        or value.get("pass") is not True
        or value.get("mode") != args.mode
        or value.get("instance_id") != args.instance_id
    ):
        raise RuntimeError("proxy did not attest the requested configuration")
    print(json.dumps(value, sort_keys=True))
    return 0


def status(args: argparse.Namespace) -> int:
    state = read_state(args.state)
    validate_expected_state(
        state,
        instance_id=args.instance_id,
        expect_implementation_sha256=args.expect_implementation_sha256,
        expect_upstream_base_url_sha256=(
            args.expect_upstream_base_url_sha256
        ),
    )
    value = _control_request(
        state,
        method="GET",
        path="/__brunn/status",
        payload=None,
        control_token_file=args.control_token_file,
    )
    print(json.dumps(value, sort_keys=True))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Control a per-stack OpenAI embedding fault proxy"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    start_parser = subparsers.add_parser("start")
    start_parser.add_argument("--host", default=DEFAULT_HOST)
    start_parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    start_parser.add_argument("--state", type=Path, required=True)
    start_parser.add_argument("--config", type=Path, required=True)
    start_parser.add_argument("--log", type=Path, required=True)
    start_parser.add_argument("--instance-id", required=True)
    start_parser.add_argument("--upstream-base-url", required=True)
    start_parser.add_argument("--control-token-file", type=Path)
    start_parser.add_argument("--allow-http-upstream", action="store_true")

    serve_parser = subparsers.add_parser("serve")
    serve_parser.add_argument("--host", default=DEFAULT_HOST)
    serve_parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    serve_parser.add_argument("--state", type=Path, required=True)
    serve_parser.add_argument("--config", type=Path, required=True)
    serve_parser.add_argument("--upstream-base-url", required=True)
    serve_parser.add_argument("--control-token-file", type=Path)
    serve_parser.add_argument("--allow-http-upstream", action="store_true")

    stop_parser = subparsers.add_parser("stop")
    stop_parser.add_argument("--state", type=Path, required=True)
    stop_parser.add_argument("--instance-id", required=True)
    stop_parser.add_argument(
        "--expect-implementation-sha256",
        required=True,
    )
    stop_parser.add_argument(
        "--expect-upstream-base-url-sha256",
        required=True,
    )

    for action in ("status", "configure"):
        action_parser = subparsers.add_parser(action)
        action_parser.add_argument("--state", type=Path, required=True)
        action_parser.add_argument("--instance-id", required=True)
        action_parser.add_argument(
            "--expect-implementation-sha256",
            required=True,
        )
        action_parser.add_argument(
            "--expect-upstream-base-url-sha256",
            required=True,
        )
        action_parser.add_argument("--control-token-file", type=Path)
        if action == "configure":
            action_parser.add_argument(
                "--mode",
                choices=("forward", "slow", "error"),
                required=True,
            )
            action_parser.add_argument("--delay-ms", type=int, default=0)
            action_parser.add_argument("--error-status", type=int, default=0)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "start":
        return start(args)
    if args.command == "serve":
        return serve(
            args.host,
            args.port,
            args.state,
            args.config,
            args.upstream_base_url,
            args.control_token_file,
            args.allow_http_upstream,
        )
    if args.command == "stop":
        return stop(
            args.state,
            instance_id=args.instance_id,
            expect_implementation_sha256=(
                args.expect_implementation_sha256
            ),
            expect_upstream_base_url_sha256=(
                args.expect_upstream_base_url_sha256
            ),
        )
    if args.command == "configure":
        return configure(args)
    return status(args)


if __name__ == "__main__":
    raise SystemExit(main())

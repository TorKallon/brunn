#!/usr/bin/env python3
"""Repeatable, destructive smoke test for the live Brunn HTTP API.

The test intentionally writes uniquely named data inside a freshly provisioned
evaluation user. It uses only the Python standard library, reads credentials
from a local dotenv file, and keeps all response diagnostics sanitized.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


DEFAULT_BASE_URL = "http://localhost:18110"
DEFAULT_TIMEOUT_SECONDS = 60.0
DEFAULT_POLL_TIMEOUT_SECONDS = 90.0
READ_CAPABILITIES = {"open", "query", "read", "compute", "verify", "status", "task.read"}
# Present only when the messaging feature is enabled on the target stack.
OPTIONAL_READ_CAPABILITIES = {"message.read"}
WRITE_CAPABILITIES = {
    "checkpoint",
    "save",
    "stage",
    "correct",
    "delete",
    "dream",
    "task.write",
}
MANAGEMENT_CAPABILITIES = {"credential:manage"}
SENSITIVE_ENV_KEY = re.compile(
    r"(?:PASSWORD|SECRET|TOKEN|API_KEY|ACCESS_KEY|SIGNING_KEY)$",
    re.IGNORECASE,
)
TOKEN_PATTERN = re.compile(r"\b(?:sl|seval)_[A-Za-z0-9_-]{12,}\b")
BEARER_PATTERN = re.compile(r"(?i)(bearer\s+)[^\s,;\"']+")


class SmokeFailure(AssertionError):
    """A contract failure with response context suitable for sanitized output."""


@dataclass(frozen=True)
class HttpResult:
    status: int
    headers: Mapping[str, str]
    body: Any
    text: str


class Sanitizer:
    def __init__(self) -> None:
        self._secrets: set[str] = set()

    def register(self, value: Any) -> None:
        if isinstance(value, str) and value:
            self._secrets.add(value)

    def register_env(self, env: Mapping[str, str]) -> None:
        for key, value in env.items():
            if SENSITIVE_ENV_KEY.search(key):
                self.register(value)

    def text(self, value: Any) -> str:
        text = str(value)
        for secret in sorted(self._secrets, key=len, reverse=True):
            text = text.replace(secret, "<redacted>")
        text = BEARER_PATTERN.sub(r"\1<redacted>", text)
        return TOKEN_PATTERN.sub("<redacted>", text)

    def value(self, value: Any) -> Any:
        if isinstance(value, dict):
            sanitized: dict[str, Any] = {}
            for key, item in value.items():
                lowered = str(key).lower()
                if lowered in {"authorization", "cookie", "set-cookie", "token"} or lowered.endswith(
                    ("_token", "_secret", "_password", "_api_key")
                ):
                    sanitized[str(key)] = "<redacted>"
                else:
                    sanitized[str(key)] = self.value(item)
            return sanitized
        if isinstance(value, list):
            return [self.value(item) for item in value]
        if isinstance(value, tuple):
            return [self.value(item) for item in value]
        if isinstance(value, str):
            return self.text(value)
        return value

    def context(self, value: Any, limit: int = 8_000) -> str:
        try:
            rendered = json.dumps(
                self.value(value),
                indent=2,
                sort_keys=True,
                ensure_ascii=True,
                default=str,
            )
        except (TypeError, ValueError):
            rendered = self.text(value)
        if len(rendered) > limit:
            rendered = rendered[:limit] + "\n...<sanitized context truncated>"
        return rendered


def parse_env_file(path: Path) -> dict[str, str]:
    if not path.is_file():
        raise SmokeFailure(f"env file does not exist: {path}")
    values: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise SmokeFailure(f"could not read env file {path}: {error}") from error
    for line_number, original in enumerate(lines, start=1):
        line = original.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[7:].lstrip()
        if "=" not in line:
            raise SmokeFailure(f"invalid dotenv assignment at {path}:{line_number}")
        key, raw_value = line.split("=", 1)
        key = key.strip()
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
            raise SmokeFailure(f"invalid dotenv key at {path}:{line_number}")
        raw_value = raw_value.strip()
        if len(raw_value) >= 2 and raw_value[0] == raw_value[-1] == "'":
            value = raw_value[1:-1]
        elif len(raw_value) >= 2 and raw_value[0] == raw_value[-1] == '"':
            try:
                decoded = json.loads(raw_value)
            except json.JSONDecodeError as error:
                raise SmokeFailure(f"invalid quoted dotenv value at {path}:{line_number}") from error
            if not isinstance(decoded, str):
                raise SmokeFailure(f"dotenv value must be text at {path}:{line_number}")
            value = decoded
        else:
            value = raw_value.split(" #", 1)[0].rstrip()
        values[key] = value
    return values


def effective_env(path: Path) -> dict[str, str]:
    values = parse_env_file(path)
    for key, value in os.environ.items():
        if key in values or key.startswith("BRUNN_"):
            values[key] = value
    return values


class ApiClient:
    def __init__(
        self,
        base_url: str,
        sanitizer: Sanitizer,
        *,
        token: str | None = None,
        timeout: float = DEFAULT_TIMEOUT_SECONDS,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.sanitizer = sanitizer
        self.token = token
        self.timeout = timeout
        sanitizer.register(token)

    def request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any | None = None,
        raw_body: bytes | None = None,
        headers: Mapping[str, str] | None = None,
        expected: int | Iterable[int] = 200,
    ) -> HttpResult:
        if json_body is not None and raw_body is not None:
            raise ValueError("json_body and raw_body are mutually exclusive")
        if not path.startswith("/"):
            raise ValueError(f"API path must start with '/': {path}")
        request_headers = {
            "Accept": "application/json",
            "User-Agent": "brunn-live-api-smoke/1",
        }
        if headers:
            request_headers.update(headers)
        if self.token:
            request_headers["Authorization"] = f"Bearer {self.token}"
        body = raw_body
        if json_body is not None:
            body = json.dumps(json_body, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
            request_headers["Content-Type"] = "application/json"
        request = urllib.request.Request(
            self.base_url + path,
            data=body,
            headers=request_headers,
            method=method.upper(),
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                result = self._result(response.status, response.headers, response.read())
        except urllib.error.HTTPError as error:
            result = self._result(error.code, error.headers, error.read())
        except urllib.error.URLError as error:
            reason = self.sanitizer.text(getattr(error, "reason", error))
            raise SmokeFailure(f"{method.upper()} {path} could not reach API: {reason}") from error
        except TimeoutError as error:
            raise SmokeFailure(
                f"{method.upper()} {path} timed out after {self.timeout:.1f} seconds"
            ) from error

        expected_statuses = {expected} if isinstance(expected, int) else set(expected)
        if result.status not in expected_statuses:
            raise SmokeFailure(
                f"{method.upper()} {path} returned HTTP {result.status}; "
                f"expected {sorted(expected_statuses)}\n"
                f"response={self.sanitizer.context(result.body)}"
            )
        return result

    @staticmethod
    def _result(status: int, headers: Mapping[str, str], raw: bytes) -> HttpResult:
        text = raw.decode("utf-8", errors="replace")
        parsed: Any = None
        if text.strip():
            try:
                parsed = json.loads(text)
            except json.JSONDecodeError:
                parsed = text
        return HttpResult(
            status=status,
            headers={str(key).lower(): str(value) for key, value in headers.items()},
            body=parsed,
            text=text,
        )


def check(condition: Any, message: str, context: Any | None = None) -> None:
    if condition:
        return
    if context is None:
        raise SmokeFailure(message)
    try:
        rendered = json.dumps(context, sort_keys=True, ensure_ascii=True, default=str)
    except (TypeError, ValueError):
        rendered = str(context)
    raise SmokeFailure(f"{message}\ncontext={rendered[:8_000]}")


def mapping(value: Any, label: str) -> dict[str, Any]:
    check(isinstance(value, dict), f"{label} must be a JSON object", value)
    return value


def sequence(value: Any, label: str) -> list[Any]:
    check(isinstance(value, list), f"{label} must be a JSON array", value)
    return value


def envelope(body: Any, label: str, statuses: str | set[str]) -> tuple[dict[str, Any], Any]:
    document = mapping(body, label)
    allowed = {statuses} if isinstance(statuses, str) else statuses
    check(document.get("status") in allowed, f"{label} has unexpected status", document)
    check(isinstance(document.get("request_id"), str), f"{label} lacks request_id", document)
    check("data" in document, f"{label} lacks data", document)
    return document, document["data"]


def error_code(body: Any) -> str | None:
    if not isinstance(body, dict):
        return None
    error = body.get("error")
    return error.get("code") if isinstance(error, dict) else None


def ref_path(reference: str) -> str:
    return urllib.parse.quote(reference, safe=":")


def canonical_ref(reference: Any) -> str:
    if not isinstance(reference, str) or ":" not in reference:
        return str(reference)
    prefix, raw_id = reference.split(":", 1)
    try:
        parsed = uuid.UUID(raw_id)
    except (ValueError, AttributeError):
        return reference
    return f"{prefix}:{parsed.hex}"


def same_ref(left: Any, right: Any) -> bool:
    return canonical_ref(left) == canonical_ref(right)


def sha256_prefixed(content: str) -> str:
    return "sha256:" + hashlib.sha256(content.encode("utf-8")).hexdigest()


def encode_multipart(
    fields: Mapping[str, str],
    files: Sequence[tuple[str, str, str, bytes]],
) -> tuple[str, bytes]:
    boundary = f"----brunn-live-smoke-{uuid.uuid4().hex}"
    chunks: list[bytes] = []

    def append(value: str) -> None:
        chunks.append(value.encode("utf-8"))

    for name, value in fields.items():
        append(f"--{boundary}\r\n")
        append(f'Content-Disposition: form-data; name="{name}"\r\n\r\n')
        append(value)
        append("\r\n")
    for field_name, filename, media_type, content in files:
        safe_filename = filename.replace('"', "_").replace("\r", "_").replace("\n", "_")
        append(f"--{boundary}\r\n")
        append(
            f'Content-Disposition: form-data; name="{field_name}"; '
            f'filename="{safe_filename}"\r\n'
        )
        append(f"Content-Type: {media_type}\r\n\r\n")
        chunks.append(content)
        append("\r\n")
    append(f"--{boundary}--\r\n")
    return f"multipart/form-data; boundary={boundary}", b"".join(chunks)


@contextmanager
def reported_step(name: str):
    print(f"[smoke] {name} ... ", end="", flush=True)
    try:
        yield
    except Exception:
        print("FAILED", flush=True)
        raise
    else:
        print("ok", flush=True)

class LiveApiSmoke:
    """Simple-protocol live smoke: identity, retrieval, writes, checkpoints,
    changes, credential lifecycle, and evaluation isolation."""

    def __init__(
        self,
        *,
        base_url: str,
        env: Mapping[str, str],
        sanitizer: Sanitizer,
        timeout: float,
        poll_timeout: float,
        poll_interval: float,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.env = env
        self.sanitizer = sanitizer
        self.timeout = timeout
        self.poll_timeout = poll_timeout
        self.poll_interval = poll_interval
        self.run_id = f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{uuid.uuid4().hex[:10]}"
        self.marker = f"slsmoke{uuid.uuid4().hex}"
        self.rw_token = self._required_env("BRUNN_DEV_READ_WRITE_TOKEN")
        self.ro_token = self._required_env("BRUNN_DEV_READ_ONLY_TOKEN")
        self.public = ApiClient(self.base_url, sanitizer, timeout=timeout)
        self.rw = ApiClient(self.base_url, sanitizer, token=self.rw_token, timeout=timeout)
        self.ro = ApiClient(self.base_url, sanitizer, token=self.ro_token, timeout=timeout)
        self.doc_path = f"probe/smoke/{self.run_id}.md"

    def _required_env(self, key: str) -> str:
        value = self.env.get(key, "").strip()
        check(value, f"{key} must be present in the env file")
        self.sanitizer.register(value)
        return value

    def run(self) -> None:
        self._health_ready()
        self._identities()
        self._write_and_retrieve()
        self._read_only_denial()
        self._checkpoint_and_changes()
        self._credential_lifecycle()
        self._evaluation_isolation()
        print("[smoke] PASS")
        print(f"[smoke] run_id={self.run_id}")

    def _health_ready(self) -> None:
        with reported_step("health and readiness"):
            health = self.public.request("GET", "/health")
            check(health.status == 200, "health must be 200", health.body)
            ready = self.public.request("GET", "/ready")
            check(ready.status == 200, "ready must be 200", ready.body)
            dependencies = mapping(
                mapping(ready.body, "ready body").get("dependencies"), "dependencies"
            )
            for dependency in ("database", "object_store"):
                check(
                    dependencies.get(dependency) == "ready",
                    f"{dependency} must be ready",
                    dependencies,
                )

    def _identities(self) -> None:
        with reported_step("read/write and read-only identity"):
            rw_me = mapping(
                mapping(self.rw.request("GET", "/v1/me").body, "rw me").get("data")
                or self.rw.request("GET", "/v1/me").body,
                "rw me data",
            )
            ro_me = mapping(
                mapping(self.ro.request("GET", "/v1/me").body, "ro me").get("data")
                or self.ro.request("GET", "/v1/me").body,
                "ro me data",
            )
            rw_capabilities = set(sequence(rw_me.get("capabilities"), "RW capabilities"))
            ro_capabilities = set(sequence(ro_me.get("capabilities"), "RO capabilities"))
            check(
                READ_CAPABILITIES | WRITE_CAPABILITIES <= rw_capabilities,
                "RW token lacks required capabilities",
                sorted(rw_capabilities),
            )
            check(
                READ_CAPABILITIES <= ro_capabilities
                and ro_capabilities <= READ_CAPABILITIES | OPTIONAL_READ_CAPABILITIES,
                "RO token must expose exactly the read capability set",
                sorted(ro_capabilities),
            )
            check(
                (WRITE_CAPABILITIES | MANAGEMENT_CAPABILITIES).isdisjoint(ro_capabilities),
                "RO token unexpectedly exposes mutation capabilities",
                sorted(ro_capabilities),
            )

    def _write_and_retrieve(self) -> None:
        with reported_step("workspace write, search, read, and open"):
            content = (
                f"# Smoke probe {self.marker}\n\n"
                f"The decision marker {self.marker} selects the smoke document.\n"
            )
            written = self.rw.request(
                "POST",
                "/v1/workspace/write",
                json_body={
                    "path": self.doc_path,
                    "content": content,
                    "expected_version": 0,
                },
            )
            check(written.status == 200, "workspace write must succeed", written.body)
            deadline = time.monotonic() + self.poll_timeout
            found = False
            while time.monotonic() < deadline:
                search = self.rw.request(
                    "POST",
                    "/v1/workspace/search",
                    json_body={"query": self.marker, "modes": ["lexical"], "limit": 5},
                )
                results = sequence(
                    mapping(mapping(search.body, "search").get("data"), "search data").get(
                        "results"
                    ),
                    "search results",
                )
                rendered = json.dumps(results)
                if self.marker in rendered and self.doc_path in rendered:
                    found = True
                    break
                time.sleep(self.poll_interval)
            check(found, "lexical search must return the written document")
            read = self.rw.request(
                "POST",
                "/v1/workspace/read",
                json_body={"requests": [{"path": self.doc_path}]},
            )
            items = sequence(
                mapping(mapping(read.body, "read").get("data"), "read data").get("items"),
                "read items",
            )
            check(
                items and self.marker in json.dumps(items[0]),
                "exact read must return the written content",
                read.body,
            )
            opened = self.rw.request(
                "POST",
                "/v1/workspace/open",
                json_body={"task": f"find the decision marker {self.marker}"},
            )
            check(
                self.marker in json.dumps(opened.body),
                "open must surface the written document as evidence",
            )

    def _read_only_denial(self) -> None:
        with reported_step("read-only write denial"):
            denied = self.ro.request(
                "POST",
                "/v1/workspace/write",
                json_body={
                    "path": f"probe/denied/{self.run_id}.md",
                    "content": "denied",
                    "expected_version": 0,
                },
                expected={401, 403},
            )
            check(
                denied.status in {401, 403},
                "read-only token must not write",
                denied.body,
            )

    def _checkpoint_and_changes(self) -> None:
        with reported_step("checkpoint and changes watermark"):
            checkpoint = self.rw.request(
                "POST",
                "/v1/workspace/checkpoint",
                json_body={
                    "session_id": f"session:smoke-{self.run_id}",
                    "idempotency_key": f"smoke:{self.run_id}:checkpoint",
                    "state": {
                        "objective": f"Record the smoke probe {self.marker}.",
                        "current_state": [f"Wrote {self.doc_path}."],
                        "decisions": ["Treat the smoke document as current."],
                    },
                },
            )
            check(checkpoint.status == 200, "checkpoint must succeed", checkpoint.body)
            data = mapping(
                mapping(checkpoint.body, "checkpoint").get("data"), "checkpoint data"
            )
            check(
                str(data.get("checkpoint_ref", "")).startswith("checkpoint:"),
                "checkpoint must return a checkpoint ref",
                data,
            )
            query_count = data.get("query_count") or mapping(
                checkpoint.body, "checkpoint"
            ).get("query_count")
            check(
                query_count is None or isinstance(query_count, int),
                "checkpoint query count must be numeric when present",
                checkpoint.body,
            )
            changes = self.rw.request(
                "GET", "/v1/workspace/changes?since_generation=0&limit=200"
            )
            check(
                self.doc_path in json.dumps(changes.body),
                "changes must include the smoke write",
            )

    def _credential_lifecycle(self) -> None:
        with reported_step("credential mint, use, and revoke"):
            minted = self.rw.request(
                "POST",
                "/v1/credentials",
                json_body={"name": f"smoke-{self.run_id}", "access": "read_only"},
            )
            check(minted.status == 200, "credential mint must succeed", minted.body)
            data = mapping(mapping(minted.body, "minted").get("data") or minted.body, "minted data")
            token = str(data.get("token") or mapping(data.get("credential") or {}, "credential").get("token") or "")
            check(token, "minted credential must return a token once", minted.body)
            self.sanitizer.register(token)
            credential_ref = str(
                data.get("id")
                or data.get("credential_id")
                or mapping(data.get("credential") or {}, "credential").get("id")
                or ""
            )
            check(credential_ref, "minted credential must return its id", minted.body)
            minted_client = ApiClient(
                self.base_url, self.sanitizer, token=token, timeout=self.timeout
            )
            me = minted_client.request("GET", "/v1/me")
            check(me.status == 200, "minted credential must authenticate", me.body)
            revoke_path = credential_ref if credential_ref.startswith("credential:") else f"credential:{credential_ref}"
            revoked = self.rw.request(
                "DELETE", f"/v1/credentials/{urllib.parse.quote(revoke_path)}"
            )
            check(revoked.status in {200, 204}, "credential revoke must succeed", revoked.body)
            after = minted_client.request("GET", "/v1/me", expected={401, 403})
            check(after.status in {401, 403}, "revoked credential must stop working", after.body)

    def _evaluation_isolation(self) -> None:
        with reported_step("isolated evaluation import and cross-user denial"):
            case = uuid.uuid4().hex
            imported = self.rw.request(
                "POST",
                "/v1/workspace/admin/eval/import",
                json_body={
                    "schema": "brunn-eval-import@v1",
                    "run_id": f"smoke-{self.run_id}",
                    "case_id": case,
                    "authorization_scope": f"smoke-isolation-{case}",
                    "display_scope": "Smoke isolation",
                    "access_mode": "read_write",
                    "documents": [
                        {
                            "path": "eval/probe.md",
                            "content": f"# Eval probe {case}\n",
                            "content_sha256": hashlib.sha256(
                                f"# Eval probe {case}\n".encode()
                            ).hexdigest(),
                            "media_type": "text/markdown",
                        }
                    ],
                    "idempotency_key": f"smoke-eval-{case}",
                },
            )
            check(imported.status == 200, "evaluation import must succeed", imported.body)
            body = mapping(mapping(imported.body, "import").get("data") or imported.body, "import data")
            eval_token = str(body.get("credential_token") or body.get("token") or "")
            check(eval_token, "evaluation import must mint an isolated token", imported.body)
            self.sanitizer.register(eval_token)
            eval_client = ApiClient(
                self.base_url, self.sanitizer, token=eval_token, timeout=self.timeout
            )
            eval_read = eval_client.request(
                "POST",
                "/v1/workspace/read",
                json_body={"requests": [{"path": "eval/probe.md"}]},
            )
            check(case in json.dumps(eval_read.body), "evaluation user must read its corpus")
            cross = eval_client.request(
                "POST",
                "/v1/workspace/read",
                json_body={"requests": [{"path": self.doc_path}]},
                expected={200, 207, 404},
            )
            check(
                self.marker not in json.dumps(cross.body),
                "evaluation user must not see the owner workspace",
                cross.body,
            )


def build_parser() -> argparse.ArgumentParser:
    default_env = Path(__file__).resolve().parents[1] / ".env"
    parser = argparse.ArgumentParser(
        description=(
            "Run the destructive Brunn live API smoke against a local stack. "
            "Secrets are read from a dotenv file and never printed."
        )
    )
    parser.add_argument(
        "--base-url",
        default=DEFAULT_BASE_URL,
        help=f"API origin (default: {DEFAULT_BASE_URL})",
    )
    parser.add_argument(
        "--env-file",
        type=Path,
        default=default_env,
        help=f"dotenv file containing dev RW/RO tokens (default: {default_env})",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_SECONDS,
        help=f"per-request timeout in seconds (default: {DEFAULT_TIMEOUT_SECONDS:g})",
    )
    parser.add_argument(
        "--poll-timeout",
        type=float,
        default=DEFAULT_POLL_TIMEOUT_SECONDS,
        help=f"indexing poll timeout in seconds (default: {DEFAULT_POLL_TIMEOUT_SECONDS:g})",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=0.5,
        help="poll interval in seconds (default: 0.5)",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.timeout <= 0 or args.poll_timeout <= 0 or args.poll_interval <= 0:
        parser.error("timeouts and poll interval must be positive")

    sanitizer = Sanitizer()
    try:
        env = effective_env(args.env_file.expanduser().resolve())
        sanitizer.register_env(env)
        smoke = LiveApiSmoke(
            base_url=args.base_url,
            env=env,
            sanitizer=sanitizer,
            timeout=args.timeout,
            poll_timeout=args.poll_timeout,
            poll_interval=args.poll_interval,
        )
        smoke.run()
        return 0
    except SmokeFailure as error:
        print(f"[smoke] FAIL: {sanitizer.text(error)}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("[smoke] INTERRUPTED", file=sys.stderr)
        return 130
    except Exception as error:
        print(
            f"[smoke] FAIL: unexpected {type(error).__name__}: {sanitizer.text(error)}",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

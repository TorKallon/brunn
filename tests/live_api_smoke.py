#!/usr/bin/env python3
"""Repeatable, destructive smoke test for the live Straylight HTTP API.

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
READ_CAPABILITIES = {"open", "query", "read", "compute", "verify", "status"}
WRITE_CAPABILITIES = {
    "checkpoint",
    "save",
    "stage",
    "correct",
    "delete",
    "dream",
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
        if key in values or key.startswith("STRAYLIGHT_"):
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
            "User-Agent": "straylight-live-api-smoke/1",
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
    boundary = f"----straylight-live-smoke-{uuid.uuid4().hex}"
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
    def __init__(
        self,
        *,
        base_url: str,
        env: Mapping[str, str],
        sanitizer: Sanitizer,
        timeout: float,
        poll_timeout: float,
        poll_interval: float,
        deep_dream: bool,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.env = env
        self.sanitizer = sanitizer
        self.timeout = timeout
        self.poll_timeout = poll_timeout
        self.poll_interval = poll_interval
        self.deep_dream = deep_dream
        self.run_id = f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{uuid.uuid4().hex[:10]}"
        self.marker = f"slsmoke{uuid.uuid4().hex}"
        self.rw_token = self._required_env("STRAYLIGHT_DEV_READ_WRITE_TOKEN")
        self.ro_token = self._required_env("STRAYLIGHT_DEV_READ_ONLY_TOKEN")
        self.public = ApiClient(self.base_url, sanitizer, timeout=timeout)
        self.rw = ApiClient(self.base_url, sanitizer, token=self.rw_token, timeout=timeout)
        self.owner_rw = self.rw
        self.ro = ApiClient(self.base_url, sanitizer, token=self.ro_token, timeout=timeout)
        self.ready_embedding_status = "unknown"
        self.scope_ref = ""
        self.owner_scope_ref = ""
        self.dev_user_id = ""
        self.object_ref = ""
        self.source_ref = ""
        self.document_ref = ""
        self.evidence_ref = ""
        self.promoted_source_ref = ""
        self.final_smoke_revision = ""
        self.eval_user_id = ""

    def _required_env(self, key: str) -> str:
        value = self.env.get(key, "")
        check(bool(value), f"{key} is required and must be nonempty")
        return value

    def run(self) -> None:
        self._health_ready()
        rw_me = self._identities()
        self.dev_user_id = mapping(rw_me.get("user"), "RW /v1/me user").get("id", "")
        active_scope = mapping(rw_me.get("active_scope"), "RW /v1/me active_scope")
        self.scope_ref = active_scope.get("scope_ref", "")
        check(self.scope_ref.startswith("scope:"), "RW identity lacks a namespaced active scope", rw_me)
        self.owner_scope_ref = self.scope_ref
        self._read_only_projection()
        self._evaluation_isolation()
        self._automatic_capture_flow()

        save_request, source_content, verification_claim = self._canonical_request()
        self._ro_mutation_denial(save_request)
        save_revision = self._save_replay_and_conflict(save_request)
        session_id = self._pinned_operations(save_revision, source_content, verification_claim)
        self._checkpoint_resume_lineage(session_id)
        self._stage_and_promote()
        self._recurrence_flow()
        self._deletion_flow()
        self._credential_lifecycle()
        self._dream_shadow_flow()
        if self.deep_dream:
            self._deep_dream_shadow_flow()
        self._audit_visibility()

        print("[smoke] PASS")
        print(f"[smoke] run_id={self.run_id}")
        print(f"[smoke] scope={self.scope_ref}")
        print(f"[smoke] embeddings={self.ready_embedding_status}")
        print(f"[smoke] final_smoke_revision={self.final_smoke_revision}")
        print(f"[smoke] isolated_eval_user={self.eval_user_id}")

    def _health_ready(self) -> None:
        with reported_step("health and readiness"):
            health = mapping(self.public.request("GET", "/health").body, "GET /health")
            check(health.get("status") == "ok", "health status must be ok", health)
            check(health.get("service") == "straylight", "health service must be straylight", health)

            ready = mapping(self.public.request("GET", "/ready").body, "GET /ready")
            check(ready.get("status") == "ready", "ready status must be ready", ready)
            check(ready.get("database") == "ready", "database must be ready", ready)
            check(ready.get("object_store") == "ready", "object store must be ready", ready)
            embedding_status = ready.get("embeddings")
            check(
                embedding_status in {"ready", "degraded"},
                "embedding readiness may only be ready or contractually degraded",
                ready,
            )
            if embedding_status == "degraded":
                check(
                    bool(ready.get("embedding_provider")) and bool(ready.get("embedding_model")),
                    "degraded embeddings must identify provider and model",
                    ready,
                )
            self.ready_embedding_status = str(embedding_status)

    def _identities(self) -> dict[str, Any]:
        with reported_step("read/write and read-only identity"):
            rw_me = mapping(self.rw.request("GET", "/v1/me").body, "RW GET /v1/me")
            ro_me = mapping(self.ro.request("GET", "/v1/me").body, "RO GET /v1/me")
            rw_user = mapping(rw_me.get("user"), "RW user")
            ro_user = mapping(ro_me.get("user"), "RO user")
            check(same_ref(rw_user.get("id"), ro_user.get("id")), "RW and RO tokens must resolve to one user")
            check(rw_me.get("read_only") is False, "RW identity must not be read-only", rw_me)
            check(ro_me.get("read_only") is True, "RO identity must be read-only", ro_me)
            rw_capabilities = set(sequence(rw_me.get("capabilities"), "RW capabilities"))
            ro_capabilities = set(sequence(ro_me.get("capabilities"), "RO capabilities"))
            check(
                READ_CAPABILITIES | WRITE_CAPABILITIES | MANAGEMENT_CAPABILITIES <= rw_capabilities,
                "RW token lacks required live API capabilities",
                sorted(rw_capabilities),
            )
            check(
                ro_capabilities == READ_CAPABILITIES,
                "RO token must expose exactly the read capability set",
                sorted(ro_capabilities),
            )
            check(
                (WRITE_CAPABILITIES | MANAGEMENT_CAPABILITIES).isdisjoint(ro_capabilities),
                "RO token unexpectedly exposes mutation capabilities",
                sorted(ro_capabilities),
            )
            rw_scopes = {item.get("scope_ref") for item in sequence(rw_me.get("scopes"), "RW scopes")}
            ro_scopes = {item.get("scope_ref") for item in sequence(ro_me.get("scopes"), "RO scopes")}
            check(rw_scopes == ro_scopes and rw_scopes, "RW and RO scope projections must match")
            return rw_me

    def _read_only_projection(self) -> None:
        with reported_step("read-only exact/lexical retrieval and policy projection"):
            marker = f"slreadonly{uuid.uuid4().hex}"
            logical_ref = f"live-api-smoke-read-only:{marker}"
            content = (
                f"# Read-only projection smoke {marker}\n\n"
                f"The exact read-only marker is {marker}.\n"
            )
            saved_body = self.owner_rw.request(
                "POST",
                "/v1/memory/save",
                json_body={
                    "intent": "seed a read-only policy projection check",
                    "scope": self.scope_ref,
                    "root_refs": [],
                    "source_refs": [],
                    "items": [
                        {
                            "action": "create",
                            "kind": "source",
                            "ref": logical_ref,
                            "payload": {
                                "path": f"tests/read-only/{marker}.md",
                                "source_ref": logical_ref,
                                "source_kind": "live_api_smoke",
                                "source_version": sha256_prefixed(content),
                                "title": f"Read-only projection {marker}",
                                "media_type": "text/markdown",
                                "content": content,
                            },
                        }
                    ],
                    "idempotency_key": f"live-api-smoke:read-only:{self.run_id}",
                },
            ).body
            _, saved_data_value = envelope(saved_body, "read-only seed save", "committed")
            saved_data = mapping(saved_data_value, "read-only seed data")
            revision = saved_data.get("corpus_revision")
            receipts = sequence(saved_data.get("items"), "read-only seed receipts")
            source_ref = receipts[0].get("ref") if receipts else None
            check(isinstance(revision, str), "read-only seed lacks revision", saved_data)
            check(
                isinstance(source_ref, str) and source_ref.startswith("source:"),
                "read-only seed lacks source ref",
                saved_data,
            )

            before = mapping(self.ro.request("GET", "/v1/status").body, "RO status before")
            opened_body = self.ro.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Retrieve read-only marker {marker}",
                    "hints": {
                        "authorization_scope": self.scope_ref,
                        "root_refs": [source_ref],
                        "open_object_refs": [],
                    },
                    "as_of": canonical_ref(revision),
                    "mode": "continuation",
                    "token_budget": 4_000,
                },
            ).body
            opened_envelope, opened_data_value = envelope(opened_body, "RO memory.open", "complete")
            opened_data = mapping(opened_data_value, "RO open data")
            session_id = opened_envelope.get("session_id")
            check(isinstance(session_id, str), "RO open lacks session", opened_body)
            check("session_id" not in opened_data, "RO open repeats envelope session_id", opened_data)
            check(isinstance(opened_data.get("projection"), dict), "RO open lacks projection", opened_data)

            queried_body = self.ro.request(
                "POST",
                "/v1/memory/query",
                json_body={
                    "session_id": session_id,
                    "queries": [
                        {
                            "id": "read-only-exact",
                            "goal": "Resolve the exact source identity",
                            "query": logical_ref,
                            "scope": {
                                "authorization_scope": self.scope_ref,
                                "root_refs": [source_ref],
                            },
                            "modes": ["exact"],
                            "limit": 5,
                        },
                        {
                            "id": "read-only-lexical",
                            "goal": "Find the unique read-only marker",
                            "query": marker,
                            "scope": {
                                "authorization_scope": self.scope_ref,
                                "root_refs": [source_ref],
                            },
                            "modes": ["lexical"],
                            "limit": 5,
                        },
                    ],
                },
            ).body
            _, queried_data_value = envelope(queried_body, "RO memory.query", "complete")
            queried_data = mapping(queried_data_value, "RO query data")
            check(isinstance(queried_data.get("projection"), dict), "RO query lacks projection", queried_data)
            items = sequence(queried_data.get("items"), "RO query items")
            check(len(items) == 2, "RO query batch changed shape", queried_data)
            for item in items:
                results = sequence(item.get("results"), f"{item.get('id')} results")
                check(results, f"{item.get('id')} returned no result", item)
                check(
                    any(marker in str(result.get("content", "")) for result in results),
                    f"{item.get('id')} lost the source marker",
                    item,
                )

            after = mapping(self.ro.request("GET", "/v1/status").body, "RO status after")
            check(
                same_ref(before.get("corpus_revision"), after.get("corpus_revision")),
                "read-only retrieval advanced the corpus revision",
                {"before": before, "after": after},
            )

    def _canonical_request(self) -> tuple[dict[str, Any], str, str]:
        source_path = f"tests/live-api-smoke/{self.marker}.md"
        source_logical_ref = f"live-api-smoke:{self.marker}"
        object_handle = f"live-api-smoke-object:{self.marker}"
        verification_claim = f"The live API marker {self.marker} is supported by canonical source evidence."
        source_content = (
            f"# Straylight live API smoke {self.marker}\n\n"
            f"{verification_claim}\n\n"
            "This document is uniquely generated by tests/live_api_smoke.py.\n"
        )
        content_hash = sha256_prefixed(source_content)
        request = {
            "intent": "exercise canonical source and object persistence",
            "scope": self.scope_ref,
            "root_refs": [object_handle],
            "source_refs": [
                {
                    "ref": source_logical_ref,
                    "span": [0, len(source_content)],
                    "content_hash": content_hash,
                }
            ],
            "items": [
                {
                    "action": "create",
                    "kind": "source",
                    "ref": source_logical_ref,
                    "payload": {
                        "path": source_path,
                        "source_ref": source_logical_ref,
                        "source_kind": "live_api_smoke",
                        "source_version": content_hash,
                        "title": f"Live API smoke {self.marker}",
                        "media_type": "text/markdown",
                        "content": source_content,
                        "metadata": {"smoke_marker": self.marker, "run_id": self.run_id},
                    },
                },
                {
                    "action": "create",
                    "kind": "object",
                    "ref": object_handle,
                    "payload": {
                        "type_profiles": ["core.artifact"],
                        "label": f"Live API smoke object {self.marker}",
                        "description": verification_claim,
                        "source_text": verification_claim,
                        "canonical_source_ref": source_logical_ref,
                        "smoke_marker": self.marker,
                        "run_id": self.run_id,
                    },
                },
            ],
            "idempotency_key": f"live-api-smoke:save:{self.run_id}",
        }
        return request, source_content, verification_claim

    def _ro_mutation_denial(self, save_request: dict[str, Any]) -> None:
        with reported_step("read-only denial across every mutation surface"):
            def require_denied(result: HttpResult, capability: str, operation: str) -> None:
                check(
                    error_code(result.body) == "capability_denied",
                    f"RO {operation} must be capability_denied",
                    result.body,
                )
                details = mapping(
                    mapping(result.body, f"RO {operation} denial").get("error"),
                    f"RO {operation} denial error",
                ).get("details")
                check(
                    isinstance(details, dict)
                    and details.get("required_capability") == capability,
                    f"RO {operation} denial must identify {capability}",
                    result.body,
                )

            denied_request = copy.deepcopy(save_request)
            denied_request["idempotency_key"] = f"live-api-smoke:ro-denied:{self.run_id}"
            require_denied(
                self.ro.request(
                    "POST",
                    "/v1/memory/save",
                    json_body=denied_request,
                    expected=403,
                ),
                "save",
                "save",
            )
            require_denied(
                self.ro.request(
                    "POST",
                    "/v1/memory/capture",
                    json_body={
                        "content": "This read-only capture must never be persisted.",
                        "source": {
                            "title": "Forbidden read-only capture",
                            "kind": "live_api_smoke",
                            "origin": "user",
                        },
                        "scope": self.scope_ref,
                        "root_refs": [],
                        "idempotency_key": f"live-api-smoke:ro-capture:{self.run_id}",
                    },
                    expected=403,
                ),
                "save",
                "capture",
            )
            require_denied(
                self.ro.request(
                    "POST",
                    "/v1/memory/checkpoint",
                    json_body={
                        "session_id": f"session:{uuid.uuid4()}",
                        "parent_checkpoint_id": None,
                        "state": {},
                        "source_refs": [],
                        "idempotency_key": f"live-api-smoke:ro-checkpoint:{self.run_id}",
                    },
                    expected=403,
                ),
                "checkpoint",
                "checkpoint",
            )
            media_type, multipart = encode_multipart(
                {
                    "scope": self.scope_ref,
                    "stable_import_id": f"live-api-smoke:ro-stage:{self.run_id}",
                },
                [("file", "forbidden.md", "text/markdown", b"must not persist")],
            )
            require_denied(
                self.ro.request(
                    "POST",
                    "/v1/memory/stage",
                    raw_body=multipart,
                    headers={"Content-Type": media_type},
                    expected=403,
                ),
                "stage",
                "stage",
            )
            require_denied(
                self.ro.request(
                    "POST",
                    "/v1/dreams",
                    json_body={
                        "scope": self.scope_ref,
                        "root_refs": [],
                        "trigger": ["read_only_denial_test"],
                        "job_type": "deterministic_maintenance",
                        "repair_only": False,
                        "idempotency_key": f"live-api-smoke:ro-dream:{self.run_id}",
                    },
                    expected=403,
                ),
                "dream",
                "dream",
            )
            require_denied(
                self.ro.request(
                    "POST",
                    "/v1/credentials",
                    json_body={
                        "name": f"Forbidden RO manager {self.run_id}",
                        "access": "read_only",
                        "scope_ids": [self.scope_ref],
                    },
                    expected=403,
                ),
                "credential:manage",
                "credential management",
            )

    def _automatic_capture_flow(self) -> None:
        with reported_step("automatic source capture, replay, provenance, and retrieval"):
            marker = f"slcapture{uuid.uuid4().hex}"
            content = (
                f"Decision record {marker}: The backend implementation language for "
                f"project {marker} is Rust. This is an explicit current decision, not a proposal."
            )
            request = {
                "content": content,
                "source": {
                    "external_ref": f"live-api-smoke-message:{marker}",
                    "title": f"Automatic capture {marker}",
                    "kind": "user_statement",
                    "origin": "user",
                    "media_type": "text/plain",
                },
                "scope": self.scope_ref,
                "root_refs": [],
                "intent": "remember an explicit project decision",
                "idempotency_key": f"live-api-smoke:capture:{self.run_id}",
                "mode": "auto",
            }

            first_body = self.rw.request(
                "POST", "/v1/memory/capture", json_body=request
            ).body
            first_envelope, first_data_value = envelope(
                first_body, "automatic capture", "committed"
            )
            first_data = mapping(first_data_value, "automatic capture data")
            revision = first_envelope.get("corpus_revision") or first_data.get("corpus_revision")
            source_ref = first_data.get("source_ref")
            check(isinstance(revision, str), "automatic capture lacks corpus revision", first_body)
            check(
                isinstance(source_ref, str) and source_ref.startswith("source:"),
                "automatic capture must return its canonical source ref",
                first_data,
            )
            check(first_data.get("capture_replayed") is False, "first capture was marked replayed", first_data)
            model = mapping(first_data.get("model"), "automatic capture model receipt")
            check(model.get("provider") == "openai", "capture model provider is not receipted", model)
            check(bool(model.get("requested_model")), "capture receipt lacks requested model", model)
            check(bool(model.get("returned_model")), "capture receipt lacks returned model", model)
            item_kinds = {
                item.get("kind") for item in sequence(first_data.get("items"), "capture items")
            }
            check(
                {"source", "evidence", "object", "claim"} <= item_kinds,
                "automatic capture did not compile source, evidence, object, and claim",
                first_data,
            )

            replay_body = self.rw.request(
                "POST", "/v1/memory/capture", json_body=request
            ).body
            replay_envelope, replay_data_value = envelope(
                replay_body, "automatic capture replay", "committed"
            )
            replay_data = mapping(replay_data_value, "automatic capture replay data")
            check(replay_data.get("capture_replayed") is True, "capture replay was not identified", replay_data)
            check(
                same_ref(replay_envelope.get("corpus_revision"), revision),
                "capture replay advanced the corpus revision",
                {"first": first_body, "replay": replay_body},
            )
            check(
                same_ref(replay_data.get("source_ref"), source_ref),
                "capture replay changed its canonical source ref",
                replay_data,
            )

            conflicting = copy.deepcopy(request)
            conflicting["content"] = content + " The idempotency key was improperly reused."
            conflict = self.rw.request(
                "POST",
                "/v1/memory/capture",
                json_body=conflicting,
                expected=409,
            )
            check(
                error_code(conflict.body) == "idempotency_conflict",
                "capture idempotency reuse must fail closed",
                conflict.body,
            )

            mismatch = self.rw.request(
                "POST",
                "/v1/memory/capture",
                json_body={
                    "content": f"Fabricated source text {uuid.uuid4().hex}",
                    "source": {
                        "ref": source_ref,
                        "kind": "user_statement",
                        "origin": "user",
                    },
                    "scope": self.scope_ref,
                    "root_refs": [],
                    "idempotency_key": f"live-api-smoke:capture-mismatch:{self.run_id}",
                    "mode": "draft",
                },
                expected=409,
            )
            check(
                error_code(mismatch.body) == "source_content_mismatch",
                "existing-source capture accepted content outside the source",
                mismatch.body,
            )

            opened_body = self.rw.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Find the captured backend decision {marker}",
                    "hints": {
                        "authorization_scope": self.scope_ref,
                        "root_refs": [source_ref],
                        "open_object_refs": [],
                    },
                    "as_of": canonical_ref(revision),
                    "mode": "continuation",
                    "token_budget": 4_000,
                },
            ).body
            opened_envelope, opened_data_value = envelope(
                opened_body,
                "capture retrieval open",
                "complete",
            )
            opened_data = mapping(opened_data_value, "capture retrieval open data")
            session_id = opened_envelope.get("session_id")
            check(isinstance(session_id, str), "capture retrieval open lacks session", opened_data)
            queried_body = self.rw.request(
                "POST",
                "/v1/memory/query",
                json_body={
                    "session_id": session_id,
                    "queries": [
                        {
                            "id": "captured-decision",
                            "goal": "Recover the explicit backend decision",
                            "query": marker,
                            "scope": {
                                "authorization_scope": self.scope_ref,
                                "root_refs": [source_ref],
                            },
                            "modes": ["exact", "structured", "lexical", "semantic"],
                            "limit": 8,
                        }
                    ],
                },
            ).body
            _, queried_data_value = envelope(queried_body, "capture retrieval query", "complete")
            queried_data = mapping(queried_data_value, "capture retrieval query data")
            results = sequence(
                sequence(queried_data.get("items"), "capture retrieval items")[0].get("results"),
                "capture retrieval results",
            )
            check(
                any(marker in str(result.get("content", "")) for result in results),
                "captured source was not retrievable by its unique marker",
                queried_data,
            )

    def _save_replay_and_conflict(self, save_request: dict[str, Any]) -> str:
        with reported_step("canonical save, exact replay, and idempotency conflict"):
            first_body = self.rw.request("POST", "/v1/memory/save", json_body=save_request).body
            first_envelope, first_data_value = envelope(first_body, "initial save", "committed")
            first_data = mapping(first_data_value, "initial save data")
            check(first_data.get("status") == "committed", "save receipt must be committed", first_data)
            check(first_data.get("replayed") is False, "initial save must not be a replay", first_data)
            revision = first_data.get("corpus_revision")
            check(isinstance(revision, str) and revision.startswith("revision:"), "save lacks corpus revision", first_data)
            check(first_envelope.get("corpus_revision") == revision, "save envelope revision mismatch", first_body)

            items = sequence(first_data.get("items"), "save receipt items")
            check(len(items) == 2, "canonical save must commit exactly source and object items", items)
            source_item = next((item for item in items if item.get("kind") == "source"), None)
            object_item = next((item for item in items if item.get("kind") == "object"), None)
            check(source_item is not None and object_item is not None, "save receipt lacks source/object items", items)
            self.source_ref = str(source_item.get("ref"))
            self.object_ref = str(object_item.get("ref"))
            check(self.source_ref.startswith("source:"), "source receipt has invalid ref", source_item)
            check(self.object_ref.startswith("object:"), "object receipt has invalid ref", object_item)
            check(source_item.get("disposition") == "created", "source was not created", source_item)
            check(object_item.get("disposition") == "created", "object was not created", object_item)
            check(object_item.get("resulting_version") == 1, "object must begin at version 1", object_item)

            created_refs = sequence(mapping(source_item.get("details"), "source details").get("created_refs"), "source created refs")
            self.document_ref = next((ref for ref in created_refs if str(ref).startswith("document:")), "")
            self.evidence_ref = next((ref for ref in created_refs if str(ref).startswith("evidence:")), "")
            check(self.document_ref, "source save must materialize a document", source_item)
            check(self.evidence_ref, "source save must materialize native evidence", source_item)

            replay_body = self.rw.request("POST", "/v1/memory/save", json_body=save_request).body
            replay_envelope, replay_data_value = envelope(replay_body, "save replay", "committed")
            replay_data = mapping(replay_data_value, "save replay data")
            expected_replay = copy.deepcopy(first_data)
            expected_replay["replayed"] = True
            check(
                replay_data == expected_replay,
                "save replay must exactly reproduce the original receipt except replayed=true",
                {"initial": first_data, "replay": replay_data},
            )
            check(
                replay_envelope.get("corpus_revision") == revision,
                "save replay must retain the original corpus revision",
                replay_body,
            )

            conflict_request = copy.deepcopy(save_request)
            conflict_request["intent"] = "different request using the same idempotency key"
            conflict = self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body=conflict_request,
                expected=409,
            )
            check(
                error_code(conflict.body) == "idempotency_conflict",
                "changed save with reused key must produce idempotency_conflict",
                conflict.body,
            )
            return str(revision)

    def _pinned_operations(
        self,
        save_revision: str,
        source_content: str,
        verification_claim: str,
    ) -> str:
        with reported_step("pinned open, query, read, compute, and verify"):
            pinned_revision = canonical_ref(save_revision)
            opened_body = self.rw.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Inspect the live API smoke fixture {self.marker}",
                    "hints": {
                        "authorization_scope": self.scope_ref,
                        "root_refs": [self.object_ref],
                        "open_object_refs": [self.object_ref],
                    },
                    "as_of": pinned_revision,
                    "mode": "continuation",
                    "token_budget": 8_000,
                },
            ).body
            opened_envelope, opened_data_value = envelope(opened_body, "memory.open", "complete")
            opened_data = mapping(opened_data_value, "memory.open data")
            session_id = opened_envelope.get("session_id")
            check(isinstance(session_id, str) and session_id.startswith("session:"), "open lacks session_id", opened_body)
            check(same_ref(opened_envelope.get("corpus_revision"), save_revision), "open envelope revision mismatch", opened_body)
            check(
                "session_id" not in opened_data and "corpus_revision" not in opened_data,
                "open repeats envelope metadata inside data",
                opened_data,
            )

            query_body = self.rw.request(
                "POST",
                "/v1/memory/query",
                json_body={
                    "session_id": session_id,
                    "queries": [
                        {
                            "id": "smoke-marker",
                            "goal": "Find the uniquely saved canonical source",
                            "query": self.marker,
                            "scope": {
                                "authorization_scope": self.scope_ref,
                                "root_refs": [self.object_ref],
                            },
                            "modes": ["lexical"],
                            "limit": 10,
                        }
                    ],
                },
            ).body
            query_envelope, query_data_value = envelope(query_body, "memory.query", "complete")
            query_data = mapping(query_data_value, "memory.query data")
            check(same_ref(query_envelope.get("corpus_revision"), save_revision), "query escaped pinned revision", query_body)
            results = sequence(query_data.get("results"), "query results")
            check(results, "query returned no result for unique marker", query_body)
            check(
                any(self.marker in str(item.get("content", "")) for item in results),
                "query results do not contain the unique marker",
                results,
            )
            check(
                any(
                    item.get("source_ref") == f"live-api-smoke:{self.marker}"
                    and item.get("path") == f"tests/live-api-smoke/{self.marker}.md"
                    for item in results
                ),
                "query did not return the canonical source",
                results,
            )

            read_body = self.rw.request(
                "POST",
                "/v1/memory/read",
                json_body={
                    "session_id": session_id,
                    "requests": [{"ref": self.source_ref, "view": "full", "max_chars": 20_000}],
                },
            ).body
            read_envelope, read_data_value = envelope(read_body, "memory.read", "complete")
            read_data = mapping(read_data_value, "memory.read data")
            check(same_ref(read_envelope.get("corpus_revision"), save_revision), "read escaped pinned revision", read_body)
            read_items = sequence(read_data.get("items"), "read items")
            check(len(read_items) == 1 and read_items[0].get("status") == "complete", "source read was not complete", read_body)
            read_text = mapping(read_items[0].get("data"), "read item data").get("text")
            check(isinstance(read_text, str), "read item lacks text", read_items[0])
            check(self.marker in read_text and verification_claim in read_text, "read text lost canonical source content", read_items[0])
            check(source_content.strip() == read_text.strip(), "full read did not reproduce source text", read_items[0])

            compute_body = self.rw.request(
                "POST",
                "/v1/memory/compute",
                json_body={
                    "session_id": session_id,
                    "steps": [{"id": "catalog", "op": "catalog", "input": {}, "depends_on": []}],
                    "max_rows": 10_000,
                    "token_budget": 8_000,
                },
            ).body
            compute_envelope, compute_data_value = envelope(compute_body, "memory.compute", "complete")
            compute_data = mapping(compute_data_value, "memory.compute data")
            check(same_ref(compute_envelope.get("corpus_revision"), save_revision), "compute escaped pinned revision", compute_body)
            steps = sequence(compute_data.get("steps"), "compute steps")
            check(len(steps) == 1 and steps[0].get("status") == "complete", "catalog compute was not complete", compute_body)
            records = mapping(mapping(steps[0].get("output"), "catalog output").get("records"), "catalog records")
            object_records = sequence(records.get("object"), "catalog object records")
            check(
                any(same_ref(item.get("ref"), self.object_ref) for item in object_records),
                "catalog compute does not contain saved object",
                steps[0],
            )

            verify_body = self.rw.request(
                "POST",
                "/v1/memory/verify",
                json_body={
                    "session_id": session_id,
                    "claims": [
                        {
                            "id": "canonical-source-claim",
                            "claim": verification_claim,
                            "evidence_refs": [self.evidence_ref],
                            "about_ref": self.object_ref,
                        }
                    ],
                    "check_for": ["source_lineage", "contradictions", "supersession"],
                },
            ).body
            verify_envelope, verify_data_value = envelope(verify_body, "memory.verify", "complete")
            verify_data = mapping(verify_data_value, "memory.verify data")
            check(same_ref(verify_envelope.get("corpus_revision"), save_revision), "verify escaped pinned revision", verify_body)
            verify_results = sequence(verify_data.get("results"), "verify results")
            check(len(verify_results) == 1, "verify must return one claim result", verify_body)
            result = mapping(verify_results[0], "verify result")
            check(result.get("classification") == "supported", "explicit source claim was not supported", result)
            evidence = sequence(result.get("evidence"), "verify evidence")
            check(
                any(same_ref(item.get("evidence_ref"), self.evidence_ref) for item in evidence),
                "verify response lost explicit evidence lineage",
                result,
            )
            return str(session_id)

    def _checkpoint_resume_lineage(self, session_id: str) -> None:
        with reported_step("checkpoint and fresh-session resume lineage"):
            checkpoint_body = self.rw.request(
                "POST",
                "/v1/memory/checkpoint",
                json_body={
                    "session_id": session_id,
                    "parent_checkpoint_id": None,
                    "state": {
                        "objective": f"Resume live API smoke work for {self.marker}",
                        "ordered_goals": [f"Preserve source and object lineage for {self.marker}"],
                        "state_refs": [],
                        "acceptance_gates": ["Fresh session preserves the parent checkpoint"],
                        "next_actions": [
                            {
                                "text": f"Continue from checkpoint {self.marker}",
                                "preconditions": [self.source_ref, self.object_ref],
                            }
                        ],
                        "source_refs": [self.source_ref, self.object_ref],
                        "focus_links": [{"ref": self.object_ref, "lens": "artifact"}],
                    },
                    "source_refs": [self.source_ref, self.object_ref],
                    "idempotency_key": f"live-api-smoke:checkpoint-parent:{self.run_id}",
                },
            ).body
            _, checkpoint_data_value = envelope(checkpoint_body, "parent checkpoint", "committed")
            checkpoint_data = mapping(checkpoint_data_value, "parent checkpoint data")
            parent_revision = checkpoint_data.get("corpus_revision")
            items = sequence(checkpoint_data.get("items"), "parent checkpoint items")
            check(len(items) == 1 and items[0].get("kind") == "checkpoint", "checkpoint receipt is malformed", checkpoint_body)
            parent_checkpoint = items[0].get("ref")
            check(
                isinstance(parent_checkpoint, str) and parent_checkpoint.startswith("checkpoint:"),
                "checkpoint receipt lacks checkpoint ref",
                checkpoint_body,
            )

            parent_detail = mapping(
                self.rw.request("GET", f"/v1/checkpoints/{ref_path(parent_checkpoint)}").body,
                "parent checkpoint detail",
            )
            check(same_ref(parent_detail.get("session_id"), session_id), "checkpoint is linked to wrong session", parent_detail)
            check(same_ref(parent_detail.get("corpus_revision"), parent_revision), "checkpoint revision mismatch", parent_detail)
            parent_sources = {
                canonical_ref(reference)
                for reference in sequence(parent_detail.get("source_refs"), "parent checkpoint sources")
            }
            check(
                {canonical_ref(self.source_ref), canonical_ref(self.object_ref)} <= parent_sources,
                "checkpoint did not preserve requested source refs",
                parent_detail,
            )

            resume_body = self.rw.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Fresh agent resumes {self.marker}",
                    "hints": {
                        "authorization_scope": self.scope_ref,
                        "root_refs": [self.object_ref],
                        "open_object_refs": [self.object_ref],
                    },
                    "as_of": canonical_ref(parent_revision),
                    "mode": "continuation",
                    "resume_checkpoint_ref": parent_checkpoint,
                    "token_budget": 8_000,
                },
            ).body
            resume_envelope, resume_data_value = envelope(resume_body, "resume open", "complete")
            resume_data = mapping(resume_data_value, "resume open data")
            resumed_session = resume_envelope.get("session_id")
            check(
                isinstance(resumed_session, str)
                and resumed_session.startswith("session:")
                and resumed_session != session_id,
                "resume must create a fresh session",
                resume_body,
            )
            check(
                same_ref(resume_envelope.get("corpus_revision"), parent_revision),
                "resume revision is not pinned",
                resume_body,
            )
            resume_checkpoint = mapping(resume_data.get("resume_checkpoint"), "resume checkpoint")
            check(
                same_ref(resume_checkpoint.get("checkpoint_ref"), parent_checkpoint),
                "resume payload does not contain requested checkpoint",
                resume_checkpoint,
            )

            resumed_detail = mapping(
                self.rw.request("GET", f"/v1/sessions/{ref_path(str(resumed_session))}").body,
                "resumed session detail",
            )
            check(same_ref(resumed_detail.get("parent_session_id"), session_id), "resumed session lost parent session", resumed_detail)
            check(
                same_ref(resumed_detail.get("resumed_checkpoint_id"), parent_checkpoint),
                "resumed session lost checkpoint lineage",
                resumed_detail,
            )

            child_body = self.rw.request(
                "POST",
                "/v1/memory/checkpoint",
                json_body={
                    "session_id": resumed_session,
                    "parent_checkpoint_id": parent_checkpoint,
                    "state": {
                        "objective": f"Advance resumed live API smoke work for {self.marker}",
                        "ordered_goals": [f"Confirm child lineage for {self.marker}"],
                        "state_refs": [],
                        "acceptance_gates": ["Child checkpoint names the resumed parent"],
                        "next_actions": [f"Close live API smoke {self.marker}"],
                        "source_refs": [self.source_ref, self.object_ref],
                    },
                    "source_refs": [self.source_ref, self.object_ref],
                    "idempotency_key": f"live-api-smoke:checkpoint-child:{self.run_id}",
                },
            ).body
            _, child_data_value = envelope(child_body, "child checkpoint", "committed")
            child_data = mapping(child_data_value, "child checkpoint data")
            child_items = sequence(child_data.get("items"), "child checkpoint items")
            child_checkpoint = child_items[0].get("ref") if len(child_items) == 1 else None
            check(isinstance(child_checkpoint, str), "child checkpoint receipt is malformed", child_body)
            child_detail = mapping(
                self.rw.request("GET", f"/v1/checkpoints/{ref_path(child_checkpoint)}").body,
                "child checkpoint detail",
            )
            check(same_ref(child_detail.get("parent_checkpoint_id"), parent_checkpoint), "child lost parent checkpoint", child_detail)
            check(same_ref(child_detail.get("session_id"), resumed_session), "child is linked to wrong session", child_detail)
            self.final_smoke_revision = str(child_data.get("corpus_revision"))

    def _stage_and_promote(self) -> None:
        with reported_step("stage and promotion"):
            filename = f"live-api-smoke-stage-{self.marker}.md"
            stage_content = (
                f"# Staged Straylight smoke {self.marker}\n\n"
                f"Promotion evidence for {self.marker}.\n"
            ).encode("utf-8")
            media_type, multipart = encode_multipart(
                {"scope": self.scope_ref, "stable_import_id": f"live-api-smoke-stage:{self.run_id}"},
                [("file", filename, "text/markdown", stage_content)],
            )
            stage_body = self.rw.request(
                "POST",
                "/v1/memory/stage",
                raw_body=multipart,
                headers={"Content-Type": media_type},
            ).body
            _, stage_data_value = envelope(stage_body, "memory.stage", "complete")
            stage_data = mapping(stage_data_value, "memory.stage data")
            stage_ref = stage_data.get("stage_ref")
            check(isinstance(stage_ref, str) and stage_ref.startswith("stage:"), "stage lacks stage_ref", stage_body)
            check(stage_data.get("status") == "ready", "stage is not ready", stage_data)
            entries = sequence(stage_data.get("entries"), "stage entries")
            check(
                len(entries) == 1
                and entries[0].get("path") == filename
                and entries[0].get("status") == "readable",
                "staged UTF-8 file was not readable",
                stage_data,
            )

            promotion_body = self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body={
                    "intent": "promote staged live API smoke source",
                    "scope": self.scope_ref,
                    "root_refs": [],
                    "source_refs": [],
                    "items": [
                        {
                            "action": "create",
                            "kind": "import_receipt",
                            "ref": None,
                            "payload": {"stage_ref": stage_ref, "selected_entries": [filename]},
                        }
                    ],
                    "idempotency_key": f"live-api-smoke:stage-promote:{self.run_id}",
                },
            ).body
            _, promotion_data_value = envelope(promotion_body, "stage promotion", "committed")
            promotion_data = mapping(promotion_data_value, "stage promotion data")
            check(
                isinstance(promotion_data.get("import_receipt"), str)
                and promotion_data["import_receipt"].startswith("import:"),
                "stage promotion lacks import receipt",
                promotion_data,
            )
            promoted_items = sequence(promotion_data.get("items"), "promoted items")
            check(
                len(promoted_items) == 1
                and promoted_items[0].get("kind") == "source"
                and promoted_items[0].get("disposition") in {"created", "deduplicated"},
                "stage promotion did not produce a source receipt",
                promotion_data,
            )
            self.promoted_source_ref = str(promoted_items[0].get("ref", ""))
            check(
                self.promoted_source_ref.startswith("source:"),
                "stage promotion returned an invalid source ref",
                promotion_data,
            )
            self.final_smoke_revision = str(promotion_data.get("corpus_revision"))

    def _recurrence_flow(self) -> None:
        with reported_step("atomic recurrence save and DST-safe expansion"):
            temporal_ref = f"temporal:{uuid.uuid4()}"
            recurrence_ref = f"recurrence:{uuid.uuid4()}"
            series_ref = f"object:{uuid.uuid4()}"
            saved_body = self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body={
                    "intent": "exercise recurrence expansion across a daylight-saving boundary",
                    "scope": self.scope_ref,
                    "root_refs": [series_ref],
                    "source_refs": [],
                    "items": [
                        {
                            "action": "create",
                            "kind": "temporal_spec",
                            "ref": temporal_ref,
                            "payload": {
                                "kind": "local_datetime",
                                "local": "2026-03-01T09:00:00",
                                "time_zone": "America/Los_Angeles",
                            },
                        },
                        {
                            "action": "create",
                            "kind": "object",
                            "ref": series_ref,
                            "payload": {
                                "type_profiles": ["core.event", "core.event_series"],
                                "label": f"DST recurrence smoke {self.marker}",
                                "source_text": f"Weekly recurrence smoke {self.marker}",
                                "event_series": {
                                    "source_uid": f"live-api-smoke-recurrence-{self.run_id}",
                                    "original_temporal_ref": temporal_ref,
                                    "current_temporal_ref": temporal_ref,
                                    "recurrence_spec_ref": recurrence_ref,
                                    "scheduling_sequence": 1,
                                },
                            },
                        },
                        {
                            "action": "create",
                            "kind": "recurrence_spec",
                            "ref": recurrence_ref,
                            "payload": {
                                "schema": "recurrence-spec@v1",
                                "start": {
                                    "kind": "local_datetime",
                                    "local": "2026-03-01T09:00:00",
                                    "time_zone": "America/Los_Angeles",
                                },
                                "duration": "PT90M",
                                "rules": ["FREQ=WEEKLY;COUNT=5"],
                                "excluded_recurrence_ids": [
                                    {
                                        "kind": "local_datetime",
                                        "local": "2026-03-15T09:00:00",
                                        "time_zone": "America/Los_Angeles",
                                    }
                                ],
                                "added_occurrence_refs": [],
                                "split_from_series_ref": None,
                            },
                        },
                    ],
                    "idempotency_key": f"live-api-smoke:recurrence:{self.run_id}",
                },
            ).body
            _, saved_data_value = envelope(saved_body, "recurrence save", "committed")
            saved_data = mapping(saved_data_value, "recurrence save data")
            revision = saved_data.get("corpus_revision")
            check(isinstance(revision, str), "recurrence save lacks revision", saved_data)
            receipts = sequence(saved_data.get("items"), "recurrence save items")
            check(
                any(same_ref(item.get("ref"), recurrence_ref) for item in receipts),
                "recurrence save lacks recurrence receipt",
                saved_data,
            )
            check(
                any(same_ref(item.get("ref"), series_ref) for item in receipts),
                "recurrence save lacks event-series receipt",
                saved_data,
            )

            opened_body = self.rw.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Expand DST recurrence {self.marker}",
                    "hints": {
                        "authorization_scope": self.scope_ref,
                        "root_refs": [series_ref],
                        "open_object_refs": [series_ref],
                    },
                    "as_of": canonical_ref(revision),
                    "mode": "continuation",
                    "token_budget": 4_000,
                },
            ).body
            opened_envelope, opened_data_value = envelope(
                opened_body,
                "recurrence open",
                "complete",
            )
            mapping(opened_data_value, "recurrence open data")
            session_id = opened_envelope.get("session_id")
            check(isinstance(session_id, str), "recurrence open lacks session", opened_body)

            computed_body = self.rw.request(
                "POST",
                "/v1/memory/compute",
                json_body={
                    "session_id": session_id,
                    "steps": [
                        {
                            "id": "expand",
                            "op": "expandRecurrence",
                            "input": {
                                "series_ref": series_ref,
                                "window_start": "2026-03-01T00:00:00Z",
                                "window_end": "2026-04-01T00:00:00Z",
                            },
                            "depends_on": [],
                        }
                    ],
                    "max_rows": 100,
                    "token_budget": 4_000,
                },
            ).body
            _, computed_data_value = envelope(computed_body, "recurrence compute", "complete")
            computed_data = mapping(computed_data_value, "recurrence compute data")
            check(
                isinstance(computed_data.get("projection"), dict),
                "recurrence compute lacks projection receipt",
                computed_data,
            )
            steps = sequence(computed_data.get("steps"), "recurrence compute steps")
            check(
                len(steps) == 1 and steps[0].get("status") == "complete",
                "recurrence expansion failed",
                computed_body,
            )
            output = mapping(steps[0].get("output"), "recurrence output")
            generated = sequence(output.get("generated_occurrences"), "generated recurrences")
            check(len(generated) == 4, "recurrence exclusion or COUNT semantics changed", output)
            instants = [
                mapping(item.get("generated_time"), "generated time").get("instant")
                for item in generated
            ]
            check(
                instants
                == [
                    "2026-03-01T17:00:00Z",
                    "2026-03-08T16:00:00Z",
                    "2026-03-22T16:00:00Z",
                    "2026-03-29T16:00:00Z",
                ],
                "recurrence expansion did not preserve 09:00 civil time across DST",
                output,
            )
            check(
                output.get("limited") is False,
                "bounded recurrence was unexpectedly limited",
                output,
            )
            self.final_smoke_revision = str(revision)

    def _deletion_flow(self) -> None:
        with reported_step("deletion propagation and per-surface receipts"):
            check(
                self.promoted_source_ref.startswith("source:"),
                "deletion flow requires the promoted staged source",
            )
            deleted_body = self.rw.request(
                "POST",
                "/v1/memory/save",
                json_body={
                    "intent": "exercise authorized deletion propagation",
                    "scope": self.scope_ref,
                    "root_refs": [self.promoted_source_ref],
                    "source_refs": [],
                    "items": [
                        {
                            "action": "tombstone",
                            "kind": "source",
                            "ref": self.promoted_source_ref,
                            "payload": {
                                "reason": "authorized live API deletion propagation test",
                                "source_text": "Delete the disposable staged smoke source and its derivatives.",
                            },
                        }
                    ],
                    "idempotency_key": f"live-api-smoke:delete:{self.run_id}",
                },
            ).body
            _, deleted_data_value = envelope(deleted_body, "deletion save", "committed")
            deleted_data = mapping(deleted_data_value, "deletion save data")
            receipts = sequence(deleted_data.get("items"), "deletion receipts")
            check(len(receipts) == 1, "deletion save must return one tombstone receipt", deleted_data)
            receipt = mapping(receipts[0], "tombstone receipt")
            check(receipt.get("action") == "tombstone", "deletion receipt action changed", receipt)
            details = mapping(receipt.get("details"), "tombstone details")
            deletion_ref = details.get("deletion_job_id")
            check(
                isinstance(deletion_ref, str) and deletion_ref.startswith("deletion:"),
                "tombstone receipt lacks deletion job",
                receipt,
            )

            terminal = {"completed", "blocked", "failed"}
            deadline = time.monotonic() + self.poll_timeout
            status_body: dict[str, Any] = {}
            while True:
                status_body = mapping(
                    self.rw.request(
                        "GET",
                        f"/v1/deletions/{ref_path(deletion_ref)}",
                    ).body,
                    "deletion status",
                )
                if status_body.get("status") in terminal:
                    break
                check(
                    status_body.get("status") in {"queued", "propagating"},
                    "deletion entered an unknown status",
                    status_body,
                )
                check(time.monotonic() < deadline, "deletion polling timed out", status_body)
                time.sleep(self.poll_interval)

            check(status_body.get("status") == "completed", "deletion did not complete", status_body)
            check(
                same_ref(status_body.get("target_ref"), self.promoted_source_ref),
                "deletion status identifies the wrong target",
                status_body,
            )
            targets = sequence(status_body.get("targets"), "deletion targets")
            expected_kinds = {
                "source",
                "chunk",
                "lexical_index",
                "embedding",
                "cache",
                "export",
                "replica",
                "asset",
                "derived_view",
            }
            seen_kinds = {item.get("kind") for item in targets}
            check(expected_kinds <= seen_kinds, "deletion omitted required cleanup surfaces", status_body)
            check(
                all(item.get("status") in {"removed", "retained_by_policy"} for item in targets),
                "completed deletion retains a pending or failed target",
                status_body,
            )
            events = sequence(status_body.get("events"), "deletion events")
            check(
                any(event.get("kind") == "completed" for event in events),
                "deletion lacks a completion event",
                status_body,
            )
            check(isinstance(status_body.get("result"), dict), "deletion lacks terminal result", status_body)

            content = self.rw.request(
                "GET",
                f"/v1/sources/{ref_path(self.promoted_source_ref)}/content",
                expected={200, 404},
            )
            if content.status == 200:
                check(
                    self.marker not in content.text,
                    "deleted source content remains readable",
                    content.text,
                )
            self.final_smoke_revision = str(deleted_data.get("corpus_revision"))

    def _credential_lifecycle(self) -> None:
        credential_id: str | None = None
        revoked = False
        try:
            with reported_step("credential authority, create, list, use, and revoke"):
                denied = self.rw.request(
                    "POST",
                    "/v1/credentials",
                    json_body={
                        "name": f"Forbidden nested manager {self.run_id}",
                        "access": "read_only",
                        "scope_ids": [self.scope_ref],
                    },
                    expected=403,
                )
                check(
                    error_code(denied.body) == "capability_denied",
                    "ordinary read/write credential can manage credentials",
                    denied.body,
                )

                created = mapping(
                    self.owner_rw.request(
                        "POST",
                        "/v1/credentials",
                        json_body={
                            "name": f"Live API smoke RO {self.run_id}",
                            "access": "read_only",
                            "scope_ids": [self.owner_scope_ref],
                        },
                    ).body,
                    "credential create",
                )
                credential_id = created.get("id")
                credential_token = created.get("token")
                self.sanitizer.register(credential_token)
                check(
                    isinstance(credential_id, str) and credential_id.startswith("credential:"),
                    "credential create lacks credential id",
                    created,
                )
                check(isinstance(credential_token, str) and credential_token, "credential create lacks one-time token")
                check(created.get("access") == "read_only", "created credential is not read-only", created)

                temporary = ApiClient(
                    self.base_url,
                    self.sanitizer,
                    token=credential_token,
                    timeout=self.timeout,
                )
                temporary_me = mapping(temporary.request("GET", "/v1/me").body, "temporary credential identity")
                check(temporary_me.get("read_only") is True, "temporary credential is not read-only", temporary_me)
                check(
                    same_ref(
                        mapping(temporary_me.get("user"), "temporary user").get("id"),
                        self.dev_user_id,
                    ),
                    "temporary credential belongs to wrong user",
                )

                listed = mapping(self.owner_rw.request("GET", "/v1/credentials").body, "credential list")
                listed_items = sequence(listed.get("items"), "credential list items")
                listed_credential = next((item for item in listed_items if item.get("id") == credential_id), None)
                check(listed_credential is not None, "created credential is absent from list", listed)
                check(listed_credential.get("status") == "active", "created credential is not active", listed_credential)

                revoked_body = mapping(
                    self.owner_rw.request("DELETE", f"/v1/credentials/{ref_path(credential_id)}").body,
                    "credential revoke",
                )
                revoked = True
                check(revoked_body.get("status") == "revoked", "credential revoke did not report revoked", revoked_body)
                check(revoked_body.get("id") == credential_id, "credential revoke returned wrong id", revoked_body)

                denied = temporary.request("GET", "/v1/me", expected=401)
                check(error_code(denied.body) == "unauthenticated", "revoked token remains authenticated", denied.body)
        finally:
            if credential_id and not revoked:
                try:
                    self.owner_rw.request(
                        "DELETE",
                        f"/v1/credentials/{ref_path(credential_id)}",
                        expected={200, 404},
                    )
                except Exception as cleanup_error:
                    print(
                        "[smoke] credential cleanup also failed: "
                        + self.sanitizer.text(cleanup_error),
                        file=sys.stderr,
                    )

    def _dream_shadow_flow(self) -> None:
        dream_id: str | None = None
        reviewed = False
        try:
            with reported_step("Phase-0 dream create, poll, list, and review"):
                created = mapping(
                    self.rw.request(
                        "POST",
                        "/v1/dreams",
                        json_body={
                            "scope": self.scope_ref,
                            "root_refs": [self.object_ref],
                            "trigger": ["live_api_smoke"],
                            "job_type": "deterministic_maintenance",
                            "repair_only": False,
                            "compute_budget": {"max_records": 10_000, "lock_ttl_seconds": 120},
                            "expected_query_families": ["live_api_smoke"],
                            "idempotency_key": f"live-api-smoke:dream:{self.run_id}",
                        },
                    ).body,
                    "dream create",
                )
                dream_id = str(created.get("id", ""))
                check(dream_id and dream_id != "None", "dream create lacks id", created)
                check(created.get("status") in {"queued", "selecting"}, "new dream has unexpected status", created)
                phase0 = mapping(created.get("phase0"), "dream phase0")
                check(phase0.get("shadow_only") is True, "dream is not Phase-0 shadow-only", created)
                check(phase0.get("automatic_promotion") is False, "dream permits automatic promotion", created)
                active_manifest_before = mapping(created.get("active_manifest"), "dream active manifest")

                allowed_in_progress = {
                    "queued",
                    "selecting",
                    "opening_snapshot",
                    "generating",
                    "gating",
                    "evaluating",
                }
                reviewable = {"awaiting_review", "quarantined"}
                failed = {"failed", "canceled", "stale", "discarded", "promoted", "rolled_back"}
                deadline = time.monotonic() + self.poll_timeout
                detail = created
                while True:
                    status = detail.get("status")
                    if status in reviewable:
                        break
                    check(status not in failed, f"dream entered non-reviewable terminal status {status}", detail)
                    check(status in allowed_in_progress, f"dream entered unknown status {status}", detail)
                    check(time.monotonic() < deadline, "dream polling timed out", detail)
                    time.sleep(self.poll_interval)
                    detail = mapping(
                        self.rw.request("GET", f"/v1/dreams/{ref_path(dream_id)}").body,
                        "dream poll",
                    )

                candidate_id = detail.get("candidate_id")
                candidate_revision = detail.get("candidate_revision")
                check(candidate_id and candidate_revision, "reviewable dream lacks candidate lineage", detail)
                check(
                    detail.get("status") == "awaiting_review",
                    "deterministic shadow candidate did not pass its hard gates",
                    detail,
                )
                check(detail.get("active_manifest") == active_manifest_before, "dream mutated active manifest", detail)
                final_phase0 = mapping(detail.get("phase0"), "completed dream phase0")
                check(final_phase0.get("active_manifest_mutation_allowed") is False, "Phase-0 allows manifest mutation", detail)
                check(final_phase0.get("promotion_operation_exposed") is False, "Phase-0 exposes promotion", detail)

                query = urllib.parse.urlencode({"scope": self.scope_ref, "limit": 100})
                dream_list = mapping(self.rw.request("GET", f"/v1/dreams?{query}").body, "dream list")
                check(dream_list.get("phase") == "phase_0_shadow", "dream list is not Phase-0", dream_list)
                dream_items = sequence(dream_list.get("items"), "dream list items")
                check(any(str(item.get("id")) == dream_id for item in dream_items), "created dream is absent from list", dream_list)

                learned_open_body = self.rw.request(
                    "POST",
                    "/v1/memory/open",
                    json_body={
                        "task": "Use the latest safe learned context for this smoke corpus",
                        "hints": {
                            "authorization_scope": self.scope_ref,
                            "root_refs": [self.object_ref],
                            "open_object_refs": [],
                        },
                        "as_of": canonical_ref(f"revision:{detail.get('base_revision')}"),
                        "mode": "continuation",
                        "token_budget": 4_000,
                    },
                ).body
                learned_open_envelope, learned_open_value = envelope(
                    learned_open_body,
                    "automatic learned-context open",
                    "complete",
                )
                learned_open = mapping(learned_open_value, "automatic learned-context data")
                learned = mapping(
                    learned_open.get("learned_context"),
                    "automatic learned context",
                )
                check(learned.get("status") == "fresh", "fresh dream view was not selected", learned)
                check(
                    learned.get("authority") == "derived_non_authoritative",
                    "learned context crossed the evidence authority boundary",
                    learned,
                )
                check(learned.get("hard_gates_passed") is True, "learned context lacks hard-gate proof", learned)
                check(
                    str(learned.get("candidate_ref", "")).startswith("candidate:"),
                    "learned context lacks candidate lineage",
                    learned,
                )
                learned_items = sequence(learned.get("items"), "automatic learned items")
                check(
                    learned.get("included_by_default") is bool(learned_items),
                    "learned-context inclusion flag disagrees with returned items",
                    learned,
                )
                for item in learned_items:
                    check(
                        bool(sequence(item.get("source_refs"), "learned item source refs")),
                        "learned item lacks direct source links",
                        item,
                    )
                coverage = mapping(
                    learned_open_envelope.get("coverage"),
                    "learned open coverage",
                )
                searched = sequence(coverage.get("searched"), "learned open searched coverage")
                check(
                    any(
                        item.get("partition") == "automatic_learned_context"
                        and item.get("completeness") == "fresh"
                        for item in searched
                    ),
                    "open coverage did not receipt automatic learned context",
                    coverage,
                )

                reviewed_detail = mapping(
                    self.rw.request(
                        "POST",
                        f"/v1/dreams/{ref_path(dream_id)}/review",
                        json_body={
                            "decision": "reject",
                            "comment": f"Live API smoke review {self.run_id}",
                            "expected_candidate_revision": f"candidate:{candidate_id}",
                        },
                    ).body,
                    "dream review",
                )
                reviewed = True
                check(reviewed_detail.get("status") == "discarded", "rejected dream was not discarded", reviewed_detail)
                review = mapping(reviewed_detail.get("review"), "dream review result")
                check(review.get("decision") == "rejected", "dream review decision mismatch", reviewed_detail)
                check(reviewed_detail.get("active_manifest") == active_manifest_before, "dream review mutated active manifest", reviewed_detail)
                check(mapping(reviewed_detail.get("phase0"), "reviewed phase0").get("shadow_only") is True, "reviewed dream lost Phase-0 state")
        finally:
            if dream_id and not reviewed:
                try:
                    self.rw.request(
                        "POST",
                        f"/v1/dreams/{ref_path(dream_id)}/cancel",
                        json_body={},
                        expected={200, 409},
                    )
                except Exception as cleanup_error:
                    print(
                        "[smoke] dream cleanup also failed: " + self.sanitizer.text(cleanup_error),
                        file=sys.stderr,
                    )

    def _deep_dream_shadow_flow(self) -> None:
        dream_id: str | None = None
        reviewed = False
        with reported_step("GPT-5.6 deep shadow dream and model receipt"):
            try:
                created = mapping(
                    self.rw.request(
                        "POST",
                        "/v1/dreams",
                        json_body={
                            "scope": self.scope_ref,
                            "root_refs": [self.object_ref],
                            "trigger": ["live_api_smoke_deep"],
                            "job_type": "deep_consolidation",
                            "repair_only": False,
                            "compute_budget": {
                                "max_records": 256,
                                "lock_ttl_seconds": 180,
                            },
                            "expected_query_families": ["live_api_smoke"],
                            "idempotency_key": f"live-api-smoke:deep-dream:{self.run_id}",
                        },
                    ).body,
                    "deep dream create",
                )
                dream_id = str(created.get("id", ""))
                check(dream_id and dream_id != "None", "deep dream create lacks id", created)
                active_manifest_before = mapping(
                    created.get("active_manifest"),
                    "deep dream active manifest",
                )
                allowed_in_progress = {
                    "queued",
                    "selecting",
                    "opening_snapshot",
                    "generating",
                    "gating",
                    "evaluating",
                }
                reviewable = {"awaiting_review", "quarantined"}
                failed = {
                    "failed",
                    "canceled",
                    "stale",
                    "discarded",
                    "promoted",
                    "rolled_back",
                }
                deadline = time.monotonic() + self.poll_timeout
                detail = created
                while True:
                    status = detail.get("status")
                    if status in reviewable:
                        break
                    check(
                        status not in failed,
                        f"deep dream entered non-reviewable terminal status {status}",
                        detail,
                    )
                    check(status in allowed_in_progress, f"deep dream entered unknown status {status}", detail)
                    check(time.monotonic() < deadline, "deep dream polling timed out", detail)
                    time.sleep(self.poll_interval)
                    detail = mapping(
                        self.rw.request(
                            "GET",
                            f"/v1/dreams/{ref_path(dream_id)}",
                        ).body,
                        "deep dream poll",
                    )

                check(detail.get("job_type") == "deep_consolidation", "deep dream job type changed", detail)
                check(
                    detail.get("active_manifest") == active_manifest_before,
                    "deep dream mutated the active manifest",
                    detail,
                )
                receipt = mapping(detail.get("model_receipt"), "deep dream model receipt")
                check(receipt.get("provider") == "openai", "deep dream used the wrong provider", receipt)
                check(
                    str(receipt.get("requested_model", "")).startswith("gpt-5.6"),
                    "deep dream did not request GPT-5.6",
                    receipt,
                )
                check(receipt.get("status") == "completed", "deep dream model call did not complete", receipt)
                check(isinstance(receipt.get("response_id"), str), "deep dream lacks OpenAI response id", receipt)
                check(isinstance(receipt.get("usage"), dict), "deep dream lacks model usage", receipt)
                check(isinstance(receipt.get("validation"), dict), "deep dream lacks validation receipt", receipt)
                check(isinstance(receipt.get("output"), dict), "deep dream lacks bounded model output", receipt)
                candidate_id = detail.get("candidate_id")
                check(candidate_id and detail.get("candidate_revision"), "deep dream lacks candidate lineage", detail)

                reviewed_detail = mapping(
                    self.rw.request(
                        "POST",
                        f"/v1/dreams/{ref_path(dream_id)}/review",
                        json_body={
                            "decision": "reject",
                            "comment": f"Deep live smoke review {self.run_id}",
                            "expected_candidate_revision": f"candidate:{candidate_id}",
                        },
                    ).body,
                    "deep dream review",
                )
                reviewed = True
                check(reviewed_detail.get("status") == "discarded", "deep dream rejection failed", reviewed_detail)
                check(
                    reviewed_detail.get("active_manifest") == active_manifest_before,
                    "deep dream review mutated the active manifest",
                    reviewed_detail,
                )
            finally:
                if dream_id and not reviewed:
                    try:
                        self.rw.request(
                            "POST",
                            f"/v1/dreams/{ref_path(dream_id)}/cancel",
                            json_body={},
                            expected={200, 409},
                        )
                    except Exception as cleanup_error:
                        print(
                            "[smoke] deep dream cleanup also failed: "
                            + self.sanitizer.text(cleanup_error),
                            file=sys.stderr,
                        )

    def _audit_visibility(self) -> None:
        with reported_step("audit visibility"):
            query = urllib.parse.urlencode({"scope": self.scope_ref, "limit": 100})
            audit = mapping(self.rw.request("GET", f"/v1/audit?{query}").body, "audit list")
            items = sequence(audit.get("items"), "audit items")
            actions = {item.get("action") for item in items}
            required = {
                "memory.save",
                "memory.stage",
                "session.open",
                "dream.created",
                "dream.shadow_completed",
                "dream.reviewed",
            }
            check(required <= actions, "audit list is missing exercised operations", {"required": sorted(required), "seen": sorted(str(action) for action in actions)})
            for item in items:
                if item.get("action") in required:
                    check(item.get("status") == "recorded", "audit item is not recorded", item)
                    check(item.get("scope_id") == self.scope_ref, "audit item is in the wrong scope", item)

    def _evaluation_isolation(self) -> None:
        with reported_step("isolated evaluation import and cross-user denial"):
            provisioner = self.rw
            eval_marker = f"sleval{uuid.uuid4().hex}"
            eval_claim = f"The isolated evaluation marker {eval_marker} belongs only to its evaluation user."
            eval_content = f"# Isolated evaluation smoke\n\n{eval_claim}\n"
            eval_path = f"eval/{eval_marker}.md"
            import_request = {
                "schema": "straylight-eval-import@v1",
                "run_id": f"live-api-smoke-{self.run_id}",
                "case_id": f"isolated-{self.marker}",
                "authorization_scope": f"scope:live-api-smoke:{self.run_id}",
                "display_scope": f"Live API smoke {self.run_id}",
                "access_mode": "read_write",
                "documents": [
                    {
                        "path": eval_path,
                        "content": eval_content,
                        "content_sha256": sha256_prefixed(eval_content),
                        "media_type": "text/markdown",
                    }
                ],
                "delta_documents": [],
                "seed_checkpoint": None,
                "idempotency_key": f"live-api-smoke:eval:{self.run_id}",
            }
            imported = mapping(
                provisioner.request("POST", "/v1/admin/eval/import", json_body=import_request).body,
                "evaluation import",
            )
            eval_token = imported.get("credential_token")
            self.sanitizer.register(eval_token)
            check(isinstance(eval_token, str) and eval_token, "evaluation import did not issue its one-time token")
            check(imported.get("status") == "ready", "evaluation import is not ready", imported)
            check(imported.get("ready_for_evaluation") is True, "evaluation import is not evaluation-ready", imported)
            check(imported.get("access_mode") == "read_write", "evaluation import access mode mismatch", imported)
            check(imported.get("token_status") == "issued_once", "evaluation token was not issued once", imported)
            check(imported.get("replayed") is False, "unique evaluation import unexpectedly replayed", imported)
            self.eval_user_id = str(imported.get("user_id", ""))
            check(self.eval_user_id.startswith("user:"), "evaluation import lacks isolated user id", imported)
            check(not same_ref(self.eval_user_id, self.dev_user_id), "evaluation import reused the dev user", imported)

            index_status = mapping(imported.get("index_status"), "evaluation index status")
            check(
                index_status == {"exact": "ready", "lexical": "ready", "semantic": "ready"},
                "evaluation import indexes are not fully ready",
                imported,
            )
            index_counts = mapping(imported.get("index_counts"), "evaluation index counts")
            check(index_counts.get("documents") == 1, "evaluation import document count mismatch", imported)
            check(
                isinstance(index_counts.get("chunks"), int)
                and index_counts["chunks"] > 0
                and index_counts.get("embeddings") == index_counts.get("chunks"),
                "evaluation import chunk/embedding counts are inconsistent",
                imported,
            )

            status_url = imported.get("status_url")
            check(isinstance(status_url, str) and status_url.startswith("/v1/admin/eval/imports/"), "evaluation import lacks status URL", imported)
            import_status = mapping(provisioner.request("GET", status_url).body, "evaluation import status")
            check(import_status.get("status") == "ready", "evaluation status is not ready", import_status)
            check(import_status.get("credential_token") is None, "evaluation status reissued a token", import_status)
            check(import_status.get("token_status") == "not_available_from_status", "evaluation status token policy mismatch", import_status)

            isolated = ApiClient(
                self.base_url,
                self.sanitizer,
                token=eval_token,
                timeout=self.timeout,
            )
            isolated_me = mapping(isolated.request("GET", "/v1/me").body, "isolated identity")
            check(
                same_ref(
                    mapping(isolated_me.get("user"), "isolated user").get("id"),
                    self.eval_user_id,
                ),
                "isolated token resolves to wrong user",
            )
            check(isolated_me.get("read_only") is False, "isolated evaluation token is not read/write", isolated_me)
            isolated_scope = imported.get("authorization_scope")
            check(isolated_scope == "scope:root", "evaluation import effective scope contract changed", imported)

            isolated_open_body = isolated.request(
                "POST",
                "/v1/memory/open",
                json_body={
                    "task": f"Find isolated evaluation marker {eval_marker}",
                    "hints": {"authorization_scope": isolated_scope, "root_refs": []},
                    "as_of": imported.get("corpus_revision"),
                    "mode": "continuation",
                    "token_budget": 4_000,
                },
            ).body
            isolated_open_envelope, isolated_open_data_value = envelope(
                isolated_open_body,
                "isolated memory.open",
                "complete",
            )
            mapping(isolated_open_data_value, "isolated open data")
            isolated_session = isolated_open_envelope.get("session_id")
            check(isinstance(isolated_session, str), "isolated open lacks session", isolated_open_body)

            isolated_query_body = isolated.request(
                "POST",
                "/v1/memory/query",
                json_body={
                    "session_id": isolated_session,
                    "query": eval_marker,
                    "scope": isolated_scope,
                    "modes": ["lexical"],
                    "limit": 10,
                },
            ).body
            _, isolated_query_data_value = envelope(isolated_query_body, "isolated query", "complete")
            isolated_results = sequence(mapping(isolated_query_data_value, "isolated query data").get("results"), "isolated query results")
            check(
                any(eval_marker in str(item.get("content", "")) for item in isolated_results),
                "isolated user cannot retrieve imported document",
                isolated_query_body,
            )

            source_query = urllib.parse.urlencode({"query": eval_marker, "limit": 100})
            isolated_sources = mapping(isolated.request("GET", f"/v1/sources?{source_query}").body, "isolated sources")
            isolated_source_items = sequence(isolated_sources.get("items"), "isolated source items")
            check(len(isolated_source_items) == 1, "isolated source list did not find exactly one import source", isolated_sources)
            isolated_source_ref = isolated_source_items[0].get("id")
            check(isinstance(isolated_source_ref, str) and isolated_source_ref.startswith("source:"), "isolated source lacks ref", isolated_source_items[0])

            isolated_audit = mapping(isolated.request("GET", "/v1/audit?limit=100").body, "isolated audit")
            isolated_actions = {item.get("action") for item in sequence(isolated_audit.get("items"), "isolated audit items")}
            check("eval.import.provision" in isolated_actions, "isolated audit lacks eval import event", isolated_audit)

            dev_sources = mapping(provisioner.request("GET", f"/v1/sources?{source_query}").body, "dev source isolation query")
            check(
                sequence(dev_sources.get("items"), "dev source isolation items") == [],
                "dev user can list isolated evaluation source",
                dev_sources,
            )
            denied = provisioner.request(
                "GET",
                f"/v1/sources/{ref_path(isolated_source_ref)}",
                expected=404,
            )
            check(
                error_code(denied.body) == "source_not_found",
                "dev direct read of isolated source must look not-found",
                denied.body,
            )
            self.rw = isolated
            self.scope_ref = str(isolated_scope)


def build_parser() -> argparse.ArgumentParser:
    default_env = Path(__file__).resolve().parents[1] / ".env"
    parser = argparse.ArgumentParser(
        description=(
            "Run the destructive Straylight live API integration smoke against a Docker API. "
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
        help=f"dream worker poll timeout in seconds (default: {DEFAULT_POLL_TIMEOUT_SECONDS:g})",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=0.5,
        help="dream worker poll interval in seconds (default: 0.5)",
    )
    parser.add_argument(
        "--deep-dream",
        action="store_true",
        help="also require one real GPT-5.6 Phase-0 shadow dream",
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
            deep_dream=args.deep_dream,
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

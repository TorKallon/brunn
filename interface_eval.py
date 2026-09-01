#!/usr/bin/env python3

from __future__ import annotations

import argparse
import asyncio
import hashlib
import hmac
import http.client
import itertools
import json
import math
import mimetypes
import os
import random
import re
import secrets
import shlex
import shutil
import signal
import ssl
import sqlite3
import stat
import statistics
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable, Sequence

from agent_work_eval import (
    DEFAULT_CODEX,
    attach_workspace_metrics,
    grade_answer,
    parse_event_metrics,
    parse_json_answer,
    recompute_grade,
    require_codex_subscription,
    sha256_file,
    validate as validate_work_manifest,
)
from native_eval import (
    NativeApiClient,
    TEXT_SUFFIXES,
    file_document,
    provision_evaluation,
    provision_binary_assets,
    provisioning_matches_run_case,
    public_provisioning,
    text_documents,
    write_native_memory_wrapper,
)
from native_memory import response_payload_metrics
from transition_eval import (
    attach_native_lineage,
    grade_transition_answer,
    seed_sources,
    validate as validate_transition_manifest,
)


ROOT = Path(__file__).resolve().parent
EXECUTION_ROOT = Path(
    os.environ.get(
        "BRUNN_STATE_EVAL_RUN_ROOT",
        "/Users/Shared/brunn-state-eval-runs",
    )
).expanduser().resolve()
SCHEMA = ROOT / "eval" / "work_answer_schema.json"
MCP_SERVER = ROOT / "apps" / "mcp" / "dist" / "index.js"
OPENCLAW = Path.home() / ".npm-global" / "bin" / "openclaw"
OPENCLAW_NPM = Path.home() / ".openclaw" / "npm"
PROVISIONING_STATE = ".interface-provisioning.json"
RUN_MANIFEST = ".run-manifest.json"
PARENT_CUSTODY = ".parent-custody"
RECORD_KEY = ".record-hmac-key"
MAX_INSPECTION_BYTES = 512 * 1024 * 1024
MAX_MODEL_BODY_BYTES = 64 * 1024 * 1024
AGENTS = ("codex", "openclaw")
FILESYSTEM_INTERFACE = "filesystem"
INTERFACES = ("cli", "mcp", "api", FILESYSTEM_INTERFACE)
INTERFACE_CHOICES = INTERFACES
SUITE_MANIFESTS = {
    "work": ROOT / "eval" / "work_cases.json",
    "personal": ROOT / "eval" / "personal_coordination_cases.json",
    "rupture": ROOT / "eval" / "rupture_ops_cases.json",
    "transition": ROOT / "eval" / "transition_cases.json",
    "binary": ROOT / "eval" / "binary_asset_reasoning_cards.json",
}
MINIMAL_OPENCLAW_FILES = (
    "AGENTS.md",
    "SOUL.md",
    "TOOLS.md",
    "IDENTITY.md",
    "USER.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
)
MODEL_PRICING = {
    "gpt-5.6-sol": {
        "checked_at": "2026-07-24",
        "source": "https://developers.openai.com/api/docs/models/gpt-5.6-sol",
        "long_context_threshold_tokens": 272_000,
        "short_context_per_million": {
            "input": 5.0,
            "cached_input": 0.5,
            "cache_write": 6.25,
            "output": 30.0,
        },
    },
}


@dataclass(frozen=True)
class EvalCase:
    suite: str
    case: dict[str, Any]
    corpus_root: Path
    corpus_paths: frozenset[str]
    transition: bool = False
    seed_state: dict[str, Any] | None = None
    seed_source_refs: tuple[str, ...] = ()
    binary_asset_paths: tuple[str, ...] = ()
    provision_asset_paths: tuple[str, ...] = ()

    @property
    def qualified_id(self) -> str:
        return f"{self.suite}:{self.case['id']}"


@dataclass(frozen=True)
class Job:
    eval_case: EvalCase
    agent: str
    interface: str
    repetition: int = 1

    @property
    def key(self) -> str:
        return (
            f"{self.eval_case.suite}__{self.agent}__{self.interface}"
            f"__{self.eval_case.case['id']}__rep{self.repetition:02d}"
        )

    @property
    def pair_key(self) -> tuple[str, int]:
        return self.eval_case.qualified_id, self.repetition

    def run_dir(self, run_root: Path) -> Path:
        return (
            run_root
            / self.eval_case.suite
            / self.agent
            / self.interface
            / self.eval_case.case["id"]
            / f"rep{self.repetition:02d}"
        )


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"Expected an object in {path}")
    return value


def sha256_tree(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def regular_file_under(path: Path, roots: Sequence[Path]) -> Path:
    lexical = Path(os.path.abspath(path))
    root = next(
        (
            candidate.resolve()
            for candidate in roots
            if lexical == candidate.resolve()
            or lexical.is_relative_to(candidate.resolve())
        ),
        None,
    )
    if root is None:
        raise ValueError("inspection target is outside the allowed evidence roots")
    current = root
    for part in lexical.relative_to(root).parts:
        current = current / part
        file_stat = current.lstat()
        if stat.S_ISLNK(file_stat.st_mode):
            raise ValueError("inspection target may not contain symlinks")
    resolved = lexical.resolve(strict=True)
    if not resolved.is_relative_to(root):
        raise ValueError("inspection target escaped the allowed evidence root")
    file_stat = resolved.lstat()
    if not stat.S_ISREG(file_stat.st_mode):
        raise ValueError("inspection target is not a regular file")
    return resolved


def hash_regular_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    file_stat = os.fstat(descriptor)
    if not stat.S_ISREG(file_stat.st_mode):
        os.close(descriptor)
        raise ValueError("inspection target is not a regular file")
    with os.fdopen(descriptor, "rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return f"sha256:{digest.hexdigest()}", size


def snapshot_regular_file_under(
    path: Path,
    roots: Sequence[Path],
    *,
    max_bytes: int = MAX_INSPECTION_BYTES,
) -> tuple[Path, bytes, str, int]:
    resolved = regular_file_under(path, roots)
    root = next(
        candidate.resolve()
        for candidate in roots
        if resolved == candidate.resolve()
        or resolved.is_relative_to(candidate.resolve())
    )
    parts = resolved.relative_to(root).parts
    if not parts:
        raise ValueError("inspection target must be below an evidence root")
    directory_flags = (
        os.O_RDONLY
        | getattr(os, "O_DIRECTORY", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    descriptor = os.open(root, directory_flags)
    try:
        for part in parts[:-1]:
            child = os.open(part, directory_flags, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        file_descriptor = os.open(
            parts[-1],
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=descriptor,
        )
        try:
            file_stat = os.fstat(file_descriptor)
            if not stat.S_ISREG(file_stat.st_mode):
                raise ValueError("inspection target is not a regular file")
            if file_stat.st_size > max_bytes:
                raise ValueError("inspection target exceeds the trusted snapshot limit")
            chunks = []
            remaining = file_stat.st_size
            while remaining:
                chunk = os.read(file_descriptor, min(1024 * 1024, remaining))
                if not chunk:
                    raise ValueError("inspection target changed while being snapshotted")
                chunks.append(chunk)
                remaining -= len(chunk)
            if os.read(file_descriptor, 1):
                raise ValueError("inspection target changed while being snapshotted")
        finally:
            os.close(file_descriptor)
    finally:
        os.close(descriptor)
    content = b"".join(chunks)
    digest = f"sha256:{hashlib.sha256(content).hexdigest()}"
    return resolved, content, digest, len(content)


def query_sqlite_read_only_bytes(content: bytes, query: str) -> dict[str, Any]:
    normalized = query.strip()
    if (
        not normalized
        or len(normalized) > 4096
        or ";" in normalized.rstrip(";")
        or not re.match(r"^(?:select|with)\b", normalized, re.IGNORECASE)
    ):
        raise ValueError("SQLite inspection accepts one bounded SELECT or WITH query")
    connection = sqlite3.connect(":memory:", timeout=5)
    try:
        connection.deserialize(content)
        connection.execute("PRAGMA query_only = ON")
        cursor = connection.execute(normalized)
        columns = [str(item[0]) for item in cursor.description or ()]
        rows = cursor.fetchmany(1001)
        if len(rows) > 1000:
            raise ValueError("SQLite inspection result exceeds 1000 rows")
        return {
            "columns": columns,
            "rows": [list(row) for row in rows],
            "row_count": len(rows),
        }
    finally:
        connection.close()


def query_sqlite_read_only(path: Path, query: str) -> dict[str, Any]:
    _, content, _, _ = snapshot_regular_file_under(path, (path.parent,))
    return query_sqlite_read_only_bytes(content, query)


def atomic_write_bytes(
    path: Path,
    content: bytes,
    *,
    mode: int,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    parent_stat = path.parent.lstat()
    if not stat.S_ISDIR(parent_stat.st_mode) or stat.S_ISLNK(parent_stat.st_mode):
        raise ValueError(f"trusted output parent is not a real directory: {path.parent}")
    directory_flags = (
        os.O_RDONLY
        | getattr(os, "O_DIRECTORY", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    directory = os.open(path.parent, directory_flags)
    temporary_name = f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    try:
        descriptor = os.open(
            temporary_name,
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | getattr(os, "O_NOFOLLOW", 0),
            mode,
            dir_fd=directory,
        )
        try:
            offset = 0
            while offset < len(content):
                offset += os.write(descriptor, content[offset:])
            os.fchmod(descriptor, mode)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.replace(
            temporary_name,
            path.name,
            src_dir_fd=directory,
            dst_dir_fd=directory,
        )
        os.chmod(path.name, mode, dir_fd=directory, follow_symlinks=False)
        os.fsync(directory)
    except BaseException:
        try:
            os.unlink(temporary_name, dir_fd=directory)
        except OSError:
            pass
        raise
    finally:
        os.close(directory)


def atomic_write_text(path: Path, content: str, *, mode: int) -> None:
    atomic_write_bytes(path, content.encode("utf-8"), mode=mode)


def read_regular_file_bytes(path: Path, *, max_bytes: int = MAX_MODEL_BODY_BYTES) -> bytes:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        file_stat = os.fstat(descriptor)
        if not stat.S_ISREG(file_stat.st_mode):
            raise ValueError(f"trusted input is not a regular file: {path}")
        if file_stat.st_size > max_bytes:
            raise ValueError(f"trusted input is too large: {path}")
        content = bytearray()
        while chunk := os.read(descriptor, min(1024 * 1024, max_bytes + 1 - len(content))):
            content.extend(chunk)
            if len(content) > max_bytes:
                raise ValueError(f"trusted input is too large: {path}")
        return bytes(content)
    finally:
        os.close(descriptor)


class TrustedInspectionServer:
    def __init__(self, job: Job, run_dir: Path) -> None:
        self.job = job
        self.run_dir = run_dir.resolve()
        self.capability = secrets.token_urlsafe(32)
        self.records: list[dict[str, Any]] = []
        self.server: asyncio.Server | None = None
        self.url: str | None = None
        self.expected = binary_asset_specs(job.eval_case)

    async def start(self) -> None:
        self.server = await asyncio.start_server(
            self._handle,
            host="127.0.0.1",
            port=0,
            limit=16 * 1024,
        )
        socket = self.server.sockets[0]
        port = int(socket.getsockname()[1])
        self.url = f"http://127.0.0.1:{port}/inspect"

    async def close(self) -> None:
        if self.server is None:
            return
        self.server.close()
        await self.server.wait_closed()

    async def _handle(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        status = 500
        response: dict[str, Any]
        try:
            header = await asyncio.wait_for(
                reader.readuntil(b"\r\n\r\n"),
                timeout=10,
            )
            header_text = header.decode("iso-8859-1")
            request_line, *header_lines = header_text.split("\r\n")
            method, request_path, _ = request_line.split(" ", 2)
            headers = {
                key.strip().casefold(): value.strip()
                for line in header_lines
                if ":" in line
                for key, value in [line.split(":", 1)]
            }
            content_length = int(headers.get("content-length", "0"))
            if (
                method != "POST"
                or request_path != "/inspect"
                or content_length < 1
                or content_length > 16 * 1024
            ):
                raise ValueError("invalid inspection request")
            body = await asyncio.wait_for(
                reader.readexactly(content_length),
                timeout=10,
            )
            request = json.loads(body)
            if not isinstance(request, dict):
                raise ValueError("inspection request must be an object")
            supplied = str(request.get("capability") or "")
            if not hmac.compare_digest(supplied, self.capability):
                raise PermissionError("invalid inspection capability")
            requested_path = request.get("path")
            if not isinstance(requested_path, str):
                raise ValueError("inspection path is required")
            requested_operation = str(request.get("operation") or "inspect")
            if requested_operation not in {
                "metadata",
                "fetch",
                "inspect",
                "sqlite_query",
            }:
                raise ValueError("unsupported trusted inspection operation")
            roots = (
                (self.run_dir / "corpus",)
                if self.job.interface == FILESYSTEM_INTERFACE
                else (self.run_dir / "assets",)
            )
            resolved, content, content_hash, size = await asyncio.to_thread(
                snapshot_regular_file_under,
                Path(requested_path),
                roots,
            )
            matches = [
                expected
                for expected in self.expected
                if expected["content_hash"] == content_hash
                and expected["size_bytes"] == size
            ]
            if self.job.interface == FILESYSTEM_INTERFACE:
                logical_path = resolved.relative_to(
                    (self.run_dir / "corpus").resolve()
                ).as_posix()
                matches = [
                    expected
                    for expected in matches
                    if expected["path"] == logical_path
                ]
            if not matches:
                raise ValueError("inspection target is not an expected card asset")
            sqlite_result = None
            if requested_operation == "sqlite_query":
                if not any(
                    item["media_type"] == "application/vnd.sqlite3"
                    for item in matches
                ):
                    raise ValueError("SQLite query requested for a non-SQLite asset")
                query = request.get("query")
                if not isinstance(query, str):
                    raise ValueError("SQLite inspection query is required")
                sqlite_result = await asyncio.to_thread(
                    query_sqlite_read_only_bytes,
                    content,
                    query,
                )
            bytes_read = (
                size
                if requested_operation in {"fetch", "inspect", "sqlite_query"}
                else 0
            )
            record = {
                "at": datetime.now().astimezone().isoformat(
                    timespec="milliseconds"
                ),
                "operation": {
                    "metadata": "filesystem_asset.metadata",
                    "fetch": "filesystem_asset.fetch",
                    "inspect": "trusted_local_inspection",
                    "sqlite_query": "trusted_sqlite_query",
                }[requested_operation],
                "logical_paths": sorted(item["path"] for item in matches),
                "content_hash": content_hash,
                "size_bytes": size,
                "bytes_read": bytes_read,
                "local_path": str(resolved),
                "verified": requested_operation != "metadata",
                "snapshot_sha256": content_hash,
            }
            if sqlite_result is not None:
                record["query_sha256"] = (
                    "sha256:"
                    + hashlib.sha256(request["query"].encode("utf-8")).hexdigest()
                )
                record["sqlite_result"] = sqlite_result
            self.records.append(record)
            response = record
            status = 200
        except PermissionError as exc:
            response = {"error": str(exc)}
            status = 403
        except (
            asyncio.IncompleteReadError,
            asyncio.LimitOverrunError,
            asyncio.TimeoutError,
            FileNotFoundError,
            json.JSONDecodeError,
            OSError,
            ValueError,
        ) as exc:
            response = {"error": str(exc)}
            status = 400
        payload = json.dumps(response, separators=(",", ":")).encode("utf-8")
        reason = {200: "OK", 400: "Bad Request", 403: "Forbidden"}.get(
            status,
            "Internal Server Error",
        )
        writer.write(
            f"HTTP/1.1 {status} {reason}\r\n".encode("ascii")
            + b"Content-Type: application/json\r\n"
            + f"Content-Length: {len(payload)}\r\n".encode("ascii")
            + b"Connection: close\r\n\r\n"
            + payload
        )
        await writer.drain()
        writer.close()
        await writer.wait_closed()


def nested_string(value: Any, names: set[str]) -> str | None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key in names and isinstance(child, str) and child:
                return child
        for child in value.values():
            if result := nested_string(child, names):
                return result
    elif isinstance(value, list):
        for child in value:
            if result := nested_string(child, names):
                return result
    return None


def complete_source_paths(value: Any, *, force_complete: bool = False) -> list[str]:
    paths: set[str] = set()

    def visit(node: Any, inherited_complete: bool = False) -> None:
        if isinstance(node, dict):
            scope = str(node.get("content_scope") or "")
            complete = (
                inherited_complete
                or force_complete
                or scope in {"complete_source", "selected_source_sections"}
            )
            path = node.get("path")
            has_source_text = any(
                isinstance(node.get(key), str) and bool(node.get(key))
                for key in ("content", "text")
            )
            if (
                complete
                and has_source_text
                and isinstance(path, str)
                and path
                and not Path(path).is_absolute()
            ):
                paths.add(path.removeprefix("./").removeprefix("corpus/"))
            for child in node.values():
                visit(child, complete)
        elif isinstance(node, list):
            for child in node:
                visit(child, inherited_complete)

    visit(value)
    return sorted(paths)


class ServiceBroker:
    MAX_REQUEST_BYTES = 4 * 1024 * 1024
    MAX_RESPONSE_BYTES = 512 * 1024 * 1024
    WRITE_ROUTES = {
        ("POST", "/v1/memory/checkpoint"),
    }

    def __init__(
        self,
        job: Job,
        metadata: dict[str, Any],
        *,
        run_id: str,
        service_api_url: str,
    ) -> None:
        self.job = job
        self.real_token = str(metadata["token"])
        self.run_id = run_id
        self.capability = secrets.token_urlsafe(32)
        self.upstream = urllib.parse.urlsplit(service_api_url.rstrip("/"))
        if self.upstream.scheme not in {"http", "https"} or not self.upstream.hostname:
            raise ValueError("evaluation service URL must be HTTP or HTTPS")
        self.server: asyncio.Server | None = None
        self.base_url: str | None = None
        self.records: list[dict[str, Any]] = []
        self.session_id: str | None = None
        self.corpus_revision: str | None = None
        self.checkpoint: dict[str, Any] | None = None

    def request_allowed(self, method: str, path: str) -> bool:
        route = urllib.parse.urlsplit(path).path
        decoded = urllib.parse.unquote(route)
        if ".." in decoded.split("/"):
            return False
        allowed = {
            ("POST", "/v1/memory/open"),
            ("POST", "/v1/memory/query"),
            ("POST", "/v1/memory/read"),
            ("POST", "/v1/memory/compute"),
            ("POST", "/v1/memory/verify"),
            ("POST", "/v1/memory/checkpoint"),
            ("GET", "/v1/assets"),
        }
        if (method, route) in allowed:
            if (
                (method, route) in self.WRITE_ROUTES
                and self.job.eval_case.case.get("workspace_access") == "read_only"
            ):
                return False
            return True
        return bool(
            method == "GET"
            and (
                re.fullmatch(r"/v1/assets/[^/]+", route)
                or re.fullmatch(
                    r"/v1/assets/[^/]+/versions/[1-9][0-9]*/content",
                    route,
                )
            )
        )

    async def start(self) -> None:
        self.server = await asyncio.start_server(
            self._handle,
            host="127.0.0.1",
            port=0,
            limit=64 * 1024,
        )
        socket = self.server.sockets[0]
        port = int(socket.getsockname()[1])
        self.base_url = f"http://127.0.0.1:{port}"

    async def close(self) -> None:
        if self.server is None:
            return
        self.server.close()
        await self.server.wait_closed()

    @staticmethod
    def operation_name(method: str, route: str) -> str:
        memory = re.fullmatch(r"/v1/memory/([^/]+)", route)
        if memory:
            return f"memory.{memory.group(1)}"
        if route == "/v1/assets":
            return "asset.list"
        if re.fullmatch(r"/v1/assets/[^/]+", route):
            return "asset.metadata"
        if re.fullmatch(
            r"/v1/assets/[^/]+/versions/[1-9][0-9]*/content",
            route,
        ):
            return "asset.fetch"
        return f"http.{method.casefold()}.unknown"

    def _forward(
        self,
        method: str,
        path: str,
        headers: dict[str, str],
        body: bytes,
    ) -> tuple[int, str, list[tuple[str, str]], bytes]:
        connection_class = (
            http.client.HTTPSConnection
            if self.upstream.scheme == "https"
            else http.client.HTTPConnection
        )
        connection = connection_class(
            self.upstream.hostname,
            self.upstream.port,
            timeout=180,
        )
        forwarded_headers = {
            key: value
            for key, value in headers.items()
            if key.casefold()
            not in {
                "authorization",
                "connection",
                "content-length",
                "host",
                "proxy-authorization",
                "proxy-connection",
                "transfer-encoding",
            }
        }
        forwarded_headers.update({
            "Authorization": f"Bearer {self.real_token}",
            "Connection": "close",
            "Content-Length": str(len(body)),
            "Host": self.upstream.netloc,
            "X-Brunn-Eval-Run": self.run_id,
            "X-Brunn-Eval-Case": self.job.key,
        })
        upstream_path = (
            self.upstream.path.rstrip("/") + path
            if self.upstream.path
            else path
        )
        try:
            connection.request(
                method,
                upstream_path,
                body=body,
                headers=forwarded_headers,
            )
            response = connection.getresponse()
            response_body = response.read(self.MAX_RESPONSE_BYTES + 1)
            if len(response_body) > self.MAX_RESPONSE_BYTES:
                raise ValueError("evaluation service response exceeds broker limit")
            return (
                response.status,
                response.reason,
                list(response.getheaders()),
                response_body,
            )
        finally:
            connection.close()

    def _record_response(
        self,
        *,
        method: str,
        path: str,
        status: int,
        response_headers: list[tuple[str, str]],
        body: bytes,
        elapsed_ms: float,
    ) -> None:
        route = urllib.parse.urlsplit(path).path
        response_header_map = {
            key.casefold(): value for key, value in response_headers
        }
        content_type = response_header_map.get("content-type", "")
        parsed: dict[str, Any] | None = None
        if "json" in content_type:
            try:
                candidate = json.loads(body or b"{}")
            except json.JSONDecodeError:
                candidate = None
            if isinstance(candidate, dict):
                parsed = candidate
        record: dict[str, Any] = {
            "at": datetime.now().astimezone().isoformat(timespec="milliseconds"),
            "operation": self.operation_name(method, route),
            "method": method,
            "route": route,
            "route_class": (
                "write" if (method, route) in self.WRITE_ROUTES else "read"
            ),
            "http_status": status,
            "request_id": response_header_map.get("x-request-id"),
            "elapsed_ms": round(elapsed_ms, 3),
            "result_chars": len(body.decode("utf-8", errors="replace")),
            "http_calls": 1,
            "trusted_broker_receipt": True,
        }
        if parsed is not None:
            rendered = json.dumps(parsed, ensure_ascii=False, separators=(",", ":"))
            metrics = response_payload_metrics(rendered)
            metrics["source_paths"] = complete_source_paths(
                parsed,
                force_complete=route == "/v1/memory/read",
            )
            record.update(metrics)
            record["service_status"] = nested_string(parsed, {"status"})
            session_id = nested_string(parsed, {"session_id"})
            revision = nested_string(parsed, {"corpus_revision", "revision_id"})
            checkpoint_id = nested_string(parsed, {"checkpoint_id"})
            if session_id:
                record["session_id"] = session_id
                self.session_id = self.session_id or session_id
            if revision:
                record["corpus_revision"] = revision
                if route == "/v1/memory/open":
                    self.corpus_revision = revision
            if checkpoint_id and route == "/v1/memory/checkpoint":
                record["checkpoint_id"] = checkpoint_id
                checkpoint = parsed.get("data", parsed)
                if isinstance(checkpoint, dict):
                    self.checkpoint = checkpoint
        content_match = re.fullmatch(
            r"/v1/assets/([^/]+)/versions/([1-9][0-9]*)/content",
            route,
        )
        if content_match:
            record.update({
                "asset_ref": urllib.parse.unquote(content_match.group(1)),
                "asset_version": int(content_match.group(2)),
                "asset_content_hash": (
                    "sha256:" + hashlib.sha256(body).hexdigest()
                ),
                "asset_size_bytes": len(body),
                "binary_bytes": len(body),
                "service_status": (
                    "complete" if status in range(200, 300) else "failed"
                ),
            })
        self.records.append(record)

    async def verify_checkpoint(self) -> dict[str, Any] | None:
        checkpoint_id = nested_string(self.checkpoint, {"checkpoint_id", "id"})
        if not checkpoint_id:
            return None
        client = NativeApiClient(
            base_url=urllib.parse.urlunsplit(self.upstream),
            token=self.real_token,
            run_id=self.run_id,
            case_id=f"{self.job.key}:supervisor",
        )
        response = await asyncio.to_thread(
            client.get_checkpoint,
            checkpoint_id,
            self.session_id,
        )
        confirmed = nested_string(response.body, {"checkpoint_id", "id"})
        return {
            "checkpoint_id": checkpoint_id,
            "http_status": response.http_status,
            "persisted": confirmed == checkpoint_id,
            "response_sha256": canonical_sha256(response.body),
        }

    async def _handle(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        status = 502
        reason = "Bad Gateway"
        response_headers: list[tuple[str, str]] = []
        response_body = b""
        try:
            raw_header = await asyncio.wait_for(
                reader.readuntil(b"\r\n\r\n"),
                timeout=30,
            )
            header_text = raw_header.decode("iso-8859-1")
            request_line, *header_lines = header_text.split("\r\n")
            method, path, _ = request_line.split(" ", 2)
            headers = {
                key.strip(): value.strip()
                for line in header_lines
                if ":" in line
                for key, value in [line.split(":", 1)]
            }
            folded = {key.casefold(): value for key, value in headers.items()}
            supplied = folded.get("authorization", "")
            expected = f"Bearer {self.capability}"
            if not hmac.compare_digest(supplied, expected):
                raise PermissionError("invalid evaluation broker capability")
            if not self.request_allowed(method, path):
                route = urllib.parse.urlsplit(path).path
                self.records.append({
                    "at": datetime.now().astimezone().isoformat(
                        timespec="milliseconds"
                    ),
                    "operation": self.operation_name(method, route),
                    "method": method,
                    "route": route,
                    "route_class": (
                        "write"
                        if (method, route) in self.WRITE_ROUTES
                        else "disallowed"
                    ),
                    "http_status": 403,
                    "service_status": "failed",
                    "trusted_broker_receipt": True,
                    "allowed": False,
                    "result_chars": 0,
                    "http_calls": 0,
                })
                raise PermissionError(
                    "service request is outside the assigned evaluation allowlist"
                )
            if "transfer-encoding" in folded:
                raise ValueError("chunked evaluation requests are not supported")
            content_length = int(folded.get("content-length", "0"))
            if content_length < 0 or content_length > self.MAX_REQUEST_BYTES:
                raise ValueError("evaluation request body is too large")
            body = (
                await asyncio.wait_for(
                    reader.readexactly(content_length),
                    timeout=30,
                )
                if content_length
                else b""
            )
            started = time.monotonic()
            status, reason, response_headers, response_body = (
                await asyncio.to_thread(
                    self._forward,
                    method,
                    path,
                    headers,
                    body,
                )
            )
            self._record_response(
                method=method,
                path=path,
                status=status,
                response_headers=response_headers,
                body=response_body,
                elapsed_ms=(time.monotonic() - started) * 1000,
            )
        except PermissionError as exc:
            status = 403
            reason = "Forbidden"
            response_body = json.dumps({"error": str(exc)}).encode("utf-8")
            response_headers = [("Content-Type", "application/json")]
        except (
            asyncio.IncompleteReadError,
            asyncio.LimitOverrunError,
            asyncio.TimeoutError,
            ConnectionError,
            http.client.HTTPException,
            OSError,
            UnicodeError,
            ValueError,
        ) as exc:
            response_body = json.dumps({"error": str(exc)}).encode("utf-8")
            response_headers = [("Content-Type", "application/json")]
        excluded = {
            "connection",
            "content-length",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
        }
        forwarded_response_headers = [
            (key, value)
            for key, value in response_headers
            if key.casefold() not in excluded
        ]
        response_head = (
            f"HTTP/1.1 {status} {reason}\r\n"
            + "".join(
                f"{key}: {value}\r\n"
                for key, value in forwarded_response_headers
            )
            + f"Content-Length: {len(response_body)}\r\n"
            + "Connection: close\r\n\r\n"
        ).encode("iso-8859-1")
        writer.write(response_head + response_body)
        try:
            await writer.drain()
        except ConnectionError:
            pass
        writer.close()
        try:
            await writer.wait_closed()
        except ConnectionError:
            pass


def parent_model_credentials(*, direct_openai: bool) -> dict[str, str]:
    if direct_openai:
        raise ValueError(
            "Direct OpenAI reasoning is forbidden for Brunn evaluations. "
            "Use the Codex plan, switch ChatGPT accounts, or wait for its reset."
        )
    auth_path = Path.home() / ".codex" / "auth.json"
    auth = load_json(auth_path)
    if auth.get("auth_mode") != "chatgpt":
        raise ValueError(
            "Brunn reasoning evaluations require ChatGPT-authenticated Codex."
        )
    tokens = auth.get("tokens")
    if not isinstance(tokens, dict):
        raise ValueError("Codex subscription auth file has no token object")
    token = tokens.get("access_token")
    account_id = tokens.get("account_id")
    if not isinstance(token, str) or not token:
        raise ValueError("Codex subscription auth file has no access token")
    result = {"authorization": f"Bearer {token}"}
    if isinstance(account_id, str) and account_id:
        result["chatgpt-account-id"] = account_id
    return result


def response_api_usage(raw: bytes) -> dict[str, int] | None:
    candidates: list[dict[str, Any]] = []
    for line in raw.decode("utf-8", errors="replace").splitlines():
        payload = line.removeprefix("data:").strip()
        if not payload or payload == "[DONE]":
            continue
        try:
            value = json.loads(payload)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            candidates.append(value)
    try:
        value = json.loads(raw)
    except json.JSONDecodeError:
        value = None
    if isinstance(value, dict):
        candidates.append(value)

    usages: list[dict[str, Any]] = []

    def visit(node: Any) -> None:
        if isinstance(node, dict):
            usage = node.get("usage")
            if isinstance(usage, dict):
                usages.append(usage)
            for child in node.values():
                visit(child)
        elif isinstance(node, list):
            for child in node:
                visit(child)

    for candidate in candidates:
        visit(candidate)
    for usage in reversed(usages):
        input_tokens = usage.get("input_tokens")
        output_tokens = usage.get("output_tokens")
        if isinstance(input_tokens, int) and isinstance(output_tokens, int):
            details = usage.get("input_tokens_details")
            cached = (
                details.get("cached_tokens", 0)
                if isinstance(details, dict)
                else 0
            )
            return {
                "input_tokens": input_tokens,
                "cached_input_tokens": int(cached or 0),
                "cache_write_input_tokens": 0,
                "output_tokens": output_tokens,
                "uncached_input_tokens": max(0, input_tokens - int(cached or 0)),
            }
    return None


class LocalModelGateway:
    SERVER_SIDE_TOOL_TYPES = frozenset({
        "code_interpreter",
        "computer_use",
        "file_search",
        "image_generation",
        "mcp",
        "web_search",
        "web_search_preview",
    })

    def __init__(
        self,
        *,
        direct_openai: bool,
        expected_model: str = "gpt-5.6-sol",
        expected_reasoning_effort: str = "xhigh",
    ) -> None:
        self.direct_openai = direct_openai
        self.expected_model = expected_model
        self.expected_reasoning_effort = expected_reasoning_effort
        self.upstream_host = "api.openai.com" if direct_openai else "chatgpt.com"
        self.path_prefix = "/v1" if direct_openai else "/backend-api/codex"
        self.capability = secrets.token_urlsafe(32)
        self.parent_headers: dict[str, str] = {}
        self.server: asyncio.Server | None = None
        self.base_url: str | None = None
        self.requests: list[dict[str, Any]] = []

    def request_allowed(self, method: str, path: str) -> bool:
        route = path.split("?", 1)[0]
        allowed = {
            ("GET", f"{self.path_prefix}/models"),
            ("POST", f"{self.path_prefix}/responses"),
            ("POST", f"{self.path_prefix}/responses/compact"),
        }
        return (method, route) in allowed

    def validate_child_request(
        self,
        method: str,
        path: str,
        headers: dict[str, str],
        body: bytes,
    ) -> dict[str, Any]:
        if not self.request_allowed(method, path):
            raise PermissionError(
                "model gateway request is outside the evaluation allowlist"
            )
        supplied = headers.get("authorization", "")
        expected = f"Bearer {self.capability}"
        if not hmac.compare_digest(supplied, expected):
            raise PermissionError("invalid model gateway capability")
        route = path.split("?", 1)[0]
        details: dict[str, Any] = {
            "method": method,
            "path": route,
            "request_bytes": len(body),
            "request_sha256": f"sha256:{hashlib.sha256(body).hexdigest()}",
            "model": None,
            "reasoning_effort": None,
            "tool_types": [],
            "contract_valid": True,
        }
        if method == "GET":
            if body:
                raise ValueError("model-list requests may not contain a body")
            return details
        try:
            payload = json.loads(body)
        except json.JSONDecodeError as exc:
            raise ValueError("model gateway request body must be JSON") from exc
        if not isinstance(payload, dict):
            raise ValueError("model gateway request body must be an object")
        request_model = payload.get("model")
        if request_model != self.expected_model:
            raise PermissionError(
                "model gateway request does not match the predeclared model"
            )
        reasoning = payload.get("reasoning")
        effort = (
            reasoning.get("effort")
            if isinstance(reasoning, dict)
            else payload.get("reasoning_effort")
        )
        if effort != self.expected_reasoning_effort:
            raise PermissionError(
                "model gateway request does not match the predeclared reasoning effort"
            )
        if payload.get("store") is True:
            raise PermissionError("model gateway requests may not enable server storage")
        if payload.get("previous_response_id") not in (None, ""):
            raise PermissionError(
                "model gateway requests may not depend on prior server-side responses"
            )
        tools = payload.get("tools", [])
        if not isinstance(tools, list):
            raise ValueError("model gateway tools must be an array")
        tool_types = sorted({
            str(tool.get("type") or "")
            for tool in tools
            if isinstance(tool, dict)
        })
        if any(tool_type in self.SERVER_SIDE_TOOL_TYPES for tool_type in tool_types):
            raise PermissionError(
                "model gateway request contains a server-side evidence tool"
            )
        details.update({
            "model": request_model,
            "reasoning_effort": effort,
            "tool_types": tool_types,
            "store": bool(payload.get("store")),
            "has_previous_response": bool(payload.get("previous_response_id")),
        })
        return details

    async def start(self) -> None:
        self.parent_headers = parent_model_credentials(
            direct_openai=self.direct_openai,
        )
        self.server = await asyncio.start_server(
            self._handle,
            host="127.0.0.1",
            port=0,
            limit=64 * 1024,
        )
        socket = self.server.sockets[0]
        port = int(socket.getsockname()[1])
        self.base_url = f"http://127.0.0.1:{port}{self.path_prefix}"

    async def close(self) -> None:
        if self.server is None:
            return
        self.server.close()
        await self.server.wait_closed()

    async def _relay_upstream_response(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> tuple[int, int, bytes, bool]:
        raw_header = await asyncio.wait_for(
            reader.readuntil(b"\r\n\r\n"),
            timeout=900,
        )
        header_text = raw_header.decode("iso-8859-1")
        status_line, *header_lines = header_text.split("\r\n")
        _, status_text, _ = status_line.split(" ", 2)
        status = int(status_text)
        headers = {
            key.strip().casefold(): value.strip()
            for line in header_lines
            if ":" in line
            for key, value in [line.split(":", 1)]
        }
        writer.write(raw_header)
        await writer.drain()
        captured = bytearray()
        response_bytes = len(raw_header)
        truncated = False

        def capture(chunk: bytes) -> None:
            nonlocal truncated
            available = MAX_MODEL_BODY_BYTES - len(captured)
            if available > 0:
                captured.extend(chunk[:available])
            if len(chunk) > available:
                truncated = True

        if "chunked" in headers.get("transfer-encoding", "").casefold():
            while True:
                chunk_header = await asyncio.wait_for(reader.readline(), timeout=900)
                if not chunk_header:
                    raise ConnectionError("upstream chunked response ended early")
                writer.write(chunk_header)
                response_bytes += len(chunk_header)
                size_text = chunk_header.split(b";", 1)[0].strip()
                size = int(size_text, 16)
                if size == 0:
                    while True:
                        trailer = await asyncio.wait_for(reader.readline(), timeout=900)
                        if not trailer:
                            raise ConnectionError(
                                "upstream response trailers ended early"
                            )
                        writer.write(trailer)
                        response_bytes += len(trailer)
                        if trailer == b"\r\n":
                            break
                    await writer.drain()
                    break
                payload = await asyncio.wait_for(
                    reader.readexactly(size),
                    timeout=900,
                )
                ending = await asyncio.wait_for(reader.readexactly(2), timeout=900)
                if ending != b"\r\n":
                    raise ValueError("invalid upstream chunk delimiter")
                writer.write(payload + ending)
                await writer.drain()
                response_bytes += len(payload) + len(ending)
                capture(payload)
        elif "content-length" in headers:
            remaining = int(headers["content-length"])
            if remaining < 0:
                raise ValueError("invalid upstream content length")
            while remaining:
                chunk = await asyncio.wait_for(
                    reader.read(min(64 * 1024, remaining)),
                    timeout=900,
                )
                if not chunk:
                    raise ConnectionError("upstream response ended early")
                writer.write(chunk)
                await writer.drain()
                remaining -= len(chunk)
                response_bytes += len(chunk)
                capture(chunk)
        else:
            while chunk := await asyncio.wait_for(reader.read(64 * 1024), timeout=900):
                writer.write(chunk)
                await writer.drain()
                response_bytes += len(chunk)
                capture(chunk)
        return status, response_bytes, bytes(captured), truncated

    async def _handle(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        upstream_writer: asyncio.StreamWriter | None = None
        response_started = False
        request_record: dict[str, Any] = {
            "at": datetime.now().astimezone().isoformat(timespec="milliseconds"),
            "upstream_host": self.upstream_host,
        }
        self.requests.append(request_record)
        try:
            raw_header = await asyncio.wait_for(
                reader.readuntil(b"\r\n\r\n"),
                timeout=30,
            )
            header_text = raw_header.decode("iso-8859-1")
            request_line, *header_lines = header_text.split("\r\n")
            method, path, _ = request_line.split(" ", 2)
            parsed_headers = [
                (key.strip(), value.strip())
                for line in header_lines
                if ":" in line
                for key, value in [line.split(":", 1)]
            ]
            headers = {
                key.casefold(): value
                for key, value in parsed_headers
            }
            if "transfer-encoding" in headers:
                raise ValueError("chunked model gateway requests are not supported")
            content_length = int(headers.get("content-length", "0"))
            if content_length < 0 or content_length > 32 * 1024 * 1024:
                raise ValueError("model gateway request body is too large")
            body = (
                await asyncio.wait_for(
                    reader.readexactly(content_length),
                    timeout=30,
                )
                if content_length
                else b""
            )
            request_record.update({
                "method": method,
                "path": path.split("?", 1)[0],
                "request_bytes": len(body),
                "request_sha256": f"sha256:{hashlib.sha256(body).hexdigest()}",
            })
            contract = self.validate_child_request(method, path, headers, body)
            request_record.update(contract)
            context = ssl.create_default_context()
            upstream_reader, upstream_writer = await asyncio.wait_for(
                asyncio.open_connection(
                    self.upstream_host,
                    443,
                    ssl=context,
                    server_hostname=self.upstream_host,
                ),
                timeout=30,
            )
            forwarded_headers = [
                (key, value)
                for key, value in parsed_headers
                if key.casefold()
                not in {
                    "authorization",
                    "chatgpt-account-id",
                    "connection",
                    "host",
                    "proxy-authorization",
                    "proxy-connection",
                }
            ]
            forwarded = (
                f"{method} {path} HTTP/1.1\r\n"
                f"Host: {self.upstream_host}\r\n"
                "Connection: close\r\n"
                + "".join(
                    f"{key}: {value}\r\n"
                    for key, value in forwarded_headers
                )
                + "".join(
                    f"{key}: {value}\r\n"
                    for key, value in self.parent_headers.items()
                )
                + "\r\n"
            ).encode("iso-8859-1")
            upstream_writer.write(forwarded)
            upstream_writer.write(body)
            await upstream_writer.drain()
            response_started = True
            status, response_bytes, captured, truncated = (
                await self._relay_upstream_response(upstream_reader, writer)
            )
            request_record.update({
                "allowed": True,
                "upstream_http_status": status,
                "response_bytes": response_bytes,
                "response_body_bytes": len(captured),
                "response_body_complete": not truncated,
                "usage": (
                    response_api_usage(captured)
                    if not truncated
                    else None
                ),
            })
        except PermissionError as exc:
            request_record.update({
                "allowed": False,
                "contract_valid": False,
                "error": str(exc),
            })
            if not response_started:
                writer.write(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n"
                    b"Connection: close\r\n\r\n"
                )
                await writer.drain()
        except (
            asyncio.IncompleteReadError,
            asyncio.LimitOverrunError,
            asyncio.TimeoutError,
            ConnectionError,
            OSError,
            UnicodeError,
            ValueError,
        ) as exc:
            request_record.update({
                "allowed": False,
                "contract_valid": False,
                "error": str(exc),
            })
            if not response_started:
                writer.write(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n"
                    b"Connection: close\r\n\r\n"
                )
                try:
                    await writer.drain()
                except ConnectionError:
                    pass
        finally:
            if upstream_writer is not None:
                upstream_writer.close()
                try:
                    await upstream_writer.wait_closed()
                except ConnectionError:
                    pass
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass


def canonical_sha256(value: Any) -> str:
    encoded = json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def file_fingerprint(path: Path) -> dict[str, Any]:
    requested = str(path)
    try:
        resolved = path.resolve(strict=True)
    except OSError:
        return {
            "path": requested,
            "resolved_path": None,
            "sha256": None,
            "missing": True,
        }
    if not resolved.is_file():
        return {
            "path": requested,
            "resolved_path": str(resolved),
            "sha256": None,
            "missing": True,
        }
    return {
        "path": requested,
        "resolved_path": str(resolved),
        "sha256": sha256_file(resolved),
        "missing": False,
    }


def http_json_fingerprint(url: str) -> dict[str, Any]:
    request = urllib.request.Request(
        url,
        headers={"Accept": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        body = response.read()
        status = response.status
    parsed = json.loads(body)
    return {
        "url": url,
        "http_status": status,
        "sha256": hashlib.sha256(body).hexdigest(),
        "canonical_sha256": canonical_sha256(parsed),
        "body": parsed if url.rstrip("/").endswith("/ready") else None,
    }


def local_service_container_identity(service_api_url: str) -> dict[str, Any] | None:
    parsed = urllib.parse.urlsplit(service_api_url)
    if parsed.hostname not in {"127.0.0.1", "localhost"}:
        return None
    compose = subprocess.run(
        ["docker", "compose", "ps", "-q", "api"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    container_id = compose.stdout.strip()
    if compose.returncode != 0 or not container_id:
        return None
    inspected = subprocess.run(
        ["docker", "inspect", container_id],
        text=True,
        capture_output=True,
        check=False,
    )
    if inspected.returncode != 0:
        return None
    values = json.loads(inspected.stdout)
    if not isinstance(values, list) or not values:
        return None
    value = values[0]
    config = value.get("Config") if isinstance(value.get("Config"), dict) else {}
    labels = config.get("Labels") if isinstance(config.get("Labels"), dict) else {}
    return {
        "container_id": str(value.get("Id") or ""),
        "image_id": str(value.get("Image") or ""),
        "image_reference": str(config.get("Image") or ""),
        "created": str(value.get("Created") or ""),
        "revision_label": labels.get("org.opencontainers.image.revision"),
    }


def eval_case_fixture_sha256(eval_case: EvalCase) -> str:
    digest = hashlib.sha256()
    for logical_path in sorted(eval_case.corpus_paths):
        source = eval_case.corpus_root / logical_path
        if not source.is_file():
            source = ROOT / logical_path
        if not source.is_file():
            raise ValueError(
                f"{eval_case.qualified_id}: missing fixture path {logical_path}"
            )
        digest.update(logical_path.encode("utf-8"))
        digest.update(b"\0")
        digest.update(source.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def build_run_manifest(
    run_id: str,
    jobs: Sequence[Job],
    *,
    model: str,
    codex: Path,
    timeout_seconds: int,
    native_index_timeout: float,
    service_api_url: str,
    direct_openai: bool,
    repetitions: int = 1,
    randomization_seed: int = 0,
) -> dict[str, Any]:
    selected_agents = sorted({job.agent for job in jobs})
    selected_interfaces = sorted({job.interface for job in jobs})
    selected_suites = sorted({job.eval_case.suite for job in jobs})
    harness = {
        name: file_fingerprint(ROOT / filename)
        for name, filename in {
            "interface_eval": "interface_eval.py",
            "agent_work_eval": "agent_work_eval.py",
            "transition_eval": "transition_eval.py",
            "native_eval": "native_eval.py",
        }.items()
    }
    helpers: dict[str, dict[str, Any]] = {}
    if "cli" in selected_interfaces:
        helpers["native_memory"] = file_fingerprint(ROOT / "native_memory.py")
    if "api" in selected_interfaces:
        helpers["native_http"] = file_fingerprint(ROOT / "native_http.py")
    if "mcp" in selected_interfaces:
        helpers["mcp_server"] = file_fingerprint(MCP_SERVER)
    if any(job.eval_case.binary_asset_paths for job in jobs):
        helpers["trusted_asset_inspect"] = file_fingerprint(
            ROOT / "trusted_asset_inspect.py"
        )
    if any(
        job.interface == FILESYSTEM_INTERFACE
        and job.eval_case.binary_asset_paths
        for job in jobs
    ):
        helpers["filesystem_asset"] = file_fingerprint(
            ROOT / "filesystem_asset.py"
        )

    runtimes: dict[str, Any] = {}
    if "codex" in selected_agents:
        runtimes["codex"] = {
            "mode": "codex_cli",
            "executable": file_fingerprint(codex),
        }
    if "openclaw" in selected_agents:
        runtimes["openclaw"] = {
            "mode": "direct_openai_pi" if direct_openai else "codex_runtime",
            "executable": file_fingerprint(OPENCLAW),
            "package": file_fingerprint(OPENCLAW_NPM / "package.json"),
            "package_lock": file_fingerprint(OPENCLAW_NPM / "package-lock.json"),
        }

    fixtures = {
        "schema": file_fingerprint(SCHEMA),
        "suite_manifests": {
            suite: file_fingerprint(SUITE_MANIFESTS[suite])
            for suite in selected_suites
        },
        "cases": {
            eval_case.qualified_id: {
                "corpus_root": str(eval_case.corpus_root.resolve()),
                "sha256": eval_case_fixture_sha256(eval_case),
            }
            for eval_case in {
                job.eval_case.qualified_id: job.eval_case for job in jobs
            }.values()
        },
    }
    manifest = {
        "format": "brunn-state-interface-run-manifest-v3",
        "run_id": run_id,
        "selection": {
            "jobs": sorted(job.key for job in jobs),
            "execution_order": [job.key for job in jobs],
            "agents": selected_agents,
            "interfaces": selected_interfaces,
            "suites": selected_suites,
            "repetitions": repetitions,
            "randomization_seed": randomization_seed,
        },
        "model": {
            "id": model,
            "reasoning_effort": "xhigh",
        },
        "reasoning_billing": {
            "route": "chatgpt_subscription",
            "api_fallback": "forbidden",
        },
        "runtime": {
            "execution_root": str(EXECUTION_ROOT),
            "service_api_url": (
                service_api_url
                if any(job.interface != FILESYSTEM_INTERFACE for job in jobs)
                else None
            ),
            "timeout_seconds": timeout_seconds,
            "native_index_timeout_seconds": native_index_timeout,
            "direct_openai": direct_openai,
            "agents": runtimes,
            "service": (
                {
                    "ready": http_json_fingerprint(
                        f"{service_api_url.rstrip('/')}/ready"
                    ),
                    "openapi": http_json_fingerprint(
                        f"{service_api_url.rstrip('/')}/openapi.json"
                    ),
                    "container": local_service_container_identity(
                        service_api_url
                    ),
                    "migrations_sha256": sha256_tree(
                        ROOT / "apps" / "api" / "migrations"
                    ),
                    "retrieval": file_fingerprint(
                        ROOT / "apps" / "api" / "src" / "retrieval.rs"
                    ),
                    "configuration_contract": file_fingerprint(
                        ROOT / "apps" / "api" / "src" / "config.rs"
                    ),
                }
                if any(job.interface != FILESYSTEM_INTERFACE for job in jobs)
                else None
            ),
        },
        "harness": harness,
        "fixtures": fixtures,
        "helpers": helpers,
    }
    manifest["section_sha256"] = {
        section: canonical_sha256(manifest[section])
        for section in ("selection", "model", "runtime", "harness", "fixtures", "helpers")
    }
    return manifest


def write_run_manifest(path: Path, manifest: dict[str, Any]) -> None:
    payload = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
    try:
        os.fchmod(descriptor, 0o400)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        try:
            os.close(descriptor)
        except OSError:
            pass
        raise


def manifest_differences(
    expected: Any,
    actual: Any,
    *,
    prefix: str = "",
) -> list[str]:
    if type(expected) is not type(actual):
        return [prefix or "<root>"]
    if isinstance(expected, dict):
        differences = []
        for key in sorted(set(expected) | set(actual)):
            child = f"{prefix}.{key}" if prefix else key
            if key not in expected or key not in actual:
                differences.append(child)
            else:
                differences.extend(
                    manifest_differences(expected[key], actual[key], prefix=child)
                )
        return differences
    if isinstance(expected, list):
        if len(expected) != len(actual):
            return [prefix or "<root>"]
        differences = []
        for index, (expected_item, actual_item) in enumerate(zip(expected, actual)):
            differences.extend(
                manifest_differences(
                    expected_item,
                    actual_item,
                    prefix=f"{prefix}[{index}]",
                )
            )
        return differences
    return [] if expected == actual else [prefix or "<root>"]


def assert_resume_compatible(path: Path, expected: dict[str, Any]) -> None:
    try:
        file_stat = path.lstat()
    except FileNotFoundError as exc:
        raise ValueError(
            f"Run cannot be resumed without immutable manifest {path}"
        ) from exc
    if not stat.S_ISREG(file_stat.st_mode) or stat.S_ISLNK(file_stat.st_mode):
        raise ValueError(f"Run manifest is not a regular file: {path}")
    if stat.S_IMODE(file_stat.st_mode) & 0o222:
        raise ValueError(f"Run manifest is writable and cannot be trusted: {path}")
    actual = load_json(path)
    differences = manifest_differences(expected, actual)
    if differences:
        rendered = ", ".join(differences[:8])
        suffix = "" if len(differences) <= 8 else f" (+{len(differences) - 8} more)"
        raise ValueError(
            f"Run manifest is incompatible with this resume: {rendered}{suffix}"
        )


def post_run_integrity(
    jobs: Sequence[Job],
    manifest: dict[str, Any],
    *,
    service_api_url: str,
) -> dict[str, Any]:
    errors: list[str] = []
    unique_cases = {
        job.eval_case.qualified_id: job.eval_case
        for job in jobs
    }
    for qualified_id, eval_case in unique_cases.items():
        expected = (
            manifest.get("fixtures", {})
            .get("cases", {})
            .get(qualified_id, {})
            .get("sha256")
        )
        actual = eval_case_fixture_sha256(eval_case)
        if actual != expected:
            errors.append(f"fixture changed during run: {qualified_id}")

    def verify_files(value: Any, prefix: str) -> None:
        if not isinstance(value, dict):
            return
        if {"path", "sha256", "missing"}.issubset(value):
            current = file_fingerprint(Path(str(value["path"])))
            if (
                current.get("sha256") != value.get("sha256")
                or current.get("missing") != value.get("missing")
            ):
                errors.append(f"file changed during run: {prefix}")
            return
        for key, child in value.items():
            verify_files(child, f"{prefix}.{key}" if prefix else str(key))

    verify_files(manifest.get("harness"), "harness")
    verify_files(manifest.get("helpers"), "helpers")
    verify_files(manifest.get("runtime", {}).get("agents"), "runtime.agents")
    expected_service = manifest.get("runtime", {}).get("service")
    if isinstance(expected_service, dict):
        for name, suffix in (("ready", "ready"), ("openapi", "openapi.json")):
            try:
                actual = http_json_fingerprint(
                    f"{service_api_url.rstrip('/')}/{suffix}"
                )
            except (OSError, ValueError, json.JSONDecodeError) as exc:
                errors.append(f"service {name} unavailable after run: {exc}")
                continue
            expected = expected_service.get(name, {})
            if actual.get("canonical_sha256") != expected.get("canonical_sha256"):
                errors.append(f"service {name} contract changed during run")
    return {
        "pass": not errors,
        "errors": errors,
    }


def ensure_binary_fixture() -> tuple[Path, dict[str, Any]]:
    generator = ROOT / "eval" / "binary_asset_fixture.py"
    cards = SUITE_MANIFESTS["binary"]
    fixture_key = hashlib.sha256(
        generator.read_bytes() + b"\0" + cards.read_bytes()
    ).hexdigest()
    root = ROOT / "runs" / ".fixtures" / f"binary-{fixture_key[:20]}"
    manifest_path = root / "fixture-manifest.json"
    if not manifest_path.exists():
        from eval.binary_asset_fixture import generate_fixture

        if root.exists():
            shutil.rmtree(root)
        generate_fixture(root)
    manifest = load_json(manifest_path)
    if manifest.get("format") != "brunn-state-binary-fixture-v1":
        raise ValueError("binary fixture has an unexpected format")
    errors = verify_binary_fixture(root, manifest)
    if errors:
        shutil.rmtree(root)
        from eval.binary_asset_fixture import generate_fixture

        generate_fixture(root)
        manifest = load_json(manifest_path)
        errors = verify_binary_fixture(root, manifest)
        if errors:
            raise ValueError(
                "binary fixture failed its own hash manifest: "
                + "; ".join(errors[:8])
            )
    return root, manifest


def verify_binary_fixture(root: Path, manifest: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    snapshots = manifest.get("snapshots")
    if not isinstance(snapshots, dict):
        return ["snapshots is not an object"]
    for snapshot_name, snapshot in snapshots.items():
        if not isinstance(snapshot, dict):
            errors.append(f"{snapshot_name}: snapshot is not an object")
            continue
        scopes = snapshot.get("scopes")
        if not isinstance(scopes, dict):
            errors.append(f"{snapshot_name}: scopes is not an object")
            continue
        for scope_name, scope in scopes.items():
            if not isinstance(scope, dict):
                errors.append(f"{snapshot_name}/{scope_name}: invalid scope")
                continue
            snapshot_root = root / str(scope.get("root") or "")
            for item in scope.get("files", []):
                if not isinstance(item, dict) or not isinstance(item.get("path"), str):
                    errors.append(
                        f"{snapshot_name}/{scope_name}: invalid file manifest row"
                    )
                    continue
                path = snapshot_root / item["path"]
                if not path.is_file():
                    errors.append(f"{snapshot_name}/{scope_name}: missing {item['path']}")
                    continue
                if path.stat().st_size != item.get("size_bytes"):
                    errors.append(
                        f"{snapshot_name}/{scope_name}: size mismatch {item['path']}"
                    )
                if sha256_file(path) != item.get("sha256"):
                    errors.append(
                        f"{snapshot_name}/{scope_name}: hash mismatch {item['path']}"
                    )
    return errors


def link_or_copy(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.link(source, destination)
    except OSError:
        shutil.copy2(source, destination)


def copy_frozen_tree(source: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"frozen evidence destination already exists: {destination}")
    for path in source.rglob("*"):
        file_stat = path.lstat()
        if stat.S_ISLNK(file_stat.st_mode):
            raise ValueError(f"frozen evidence may not contain symlinks: {path}")
        if not stat.S_ISDIR(file_stat.st_mode) and not stat.S_ISREG(file_stat.st_mode):
            raise ValueError(f"unsupported frozen evidence entry: {path}")
    shutil.copytree(source, destination, copy_function=shutil.copy2)
    for path in sorted(destination.rglob("*"), reverse=True):
        path.chmod(0o500 if path.is_dir() else 0o400)
    destination.chmod(0o500)


def remove_owned_run_tree(path: Path) -> None:
    if path.is_symlink():
        path.unlink()
        return
    if not path.exists():
        return
    for child in path.rglob("*"):
        if child.is_symlink():
            continue
        try:
            child.chmod(0o700 if child.is_dir() else 0o600)
        except OSError:
            pass
    path.chmod(0o700)
    shutil.rmtree(path)


def materialize_binary_case_root(
    fixture_root: Path,
    scope_record: dict[str, Any],
    *,
    case_id: str,
    asset_paths: tuple[str, ...],
    restricted_asset_paths: frozenset[str],
) -> tuple[Path, dict[str, str]]:
    case_root = fixture_root / "cases" / case_id
    marker = case_root / ".complete"
    selected_files = [
        item
        for item in scope_record.get("files", [])
        if isinstance(item, dict)
        and isinstance(item.get("path"), str)
        and (
            (
                item.get("role") not in {"native_binary", "generated_description"}
                and (
                    item["path"] not in restricted_asset_paths
                    or item["path"] in asset_paths
                )
            )
            or (
                item.get("role") == "native_binary"
                and item["path"] in asset_paths
            )
            or (
                item.get("role") == "generated_description"
                and item.get("describes_path") in asset_paths
            )
        )
    ]
    descriptions = {
        str(item["describes_path"]): str(item["path"])
        for item in selected_files
        if isinstance(item, dict)
        and item.get("role") == "generated_description"
        and isinstance(item.get("describes_path"), str)
        and isinstance(item.get("path"), str)
    }
    if marker.exists():
        expected = {
            str(item["path"]): (str(item["sha256"]), int(item["size_bytes"]))
            for item in selected_files
            if isinstance(item, dict)
            and isinstance(item.get("path"), str)
            and isinstance(item.get("sha256"), str)
            and isinstance(item.get("size_bytes"), int)
        }
        actual = {
            path.relative_to(case_root).as_posix(): (
                sha256_file(path),
                path.stat().st_size,
            )
            for path in case_root.rglob("*")
            if path.is_file() and path.name != ".complete"
        }
        if actual == expected:
            return case_root, descriptions
    if case_root.exists():
        shutil.rmtree(case_root)
    evaluation_root = fixture_root / str(scope_record["evaluation_root"])
    snapshot_root = fixture_root / str(scope_record["root"])
    selected_paths = {
        str(item["path"])
        for item in selected_files
    }
    for source in sorted(path for path in evaluation_root.rglob("*") if path.is_file()):
        relative = source.relative_to(evaluation_root).as_posix()
        if relative in selected_paths:
            link_or_copy(source, case_root / relative)
    for description_path in descriptions.values():
        link_or_copy(
            snapshot_root / description_path,
            case_root / description_path,
        )
    marker.touch(mode=0o600)
    return case_root, descriptions


def validate_binary_leakage(
    eval_case: EvalCase,
    card: dict[str, Any],
) -> list[str]:
    errors: list[str] = []
    fake_metadata = {
        "authorization_scope": "scope:prompt-leakage-check",
        "checkpoint_id": None,
    }
    prompts = "\n".join(
        render_prompt(Job(eval_case, agent, interface), fake_metadata)
        for agent in AGENTS
        for interface in INTERFACES
    ).casefold()
    for value in card.get("withheld_values", []):
        if str(value).casefold() in prompts:
            errors.append(
                f"{card.get('id')}: withheld value leaked into agent prompt: {value}"
            )
    documents = text_documents(eval_case.corpus_root)
    searchable = "\n".join(
        str(document.get("content") or "") for document in documents
    ).casefold()
    described_paths = [
        str(item.get("path"))
        for item in card.get("evidence", [])
        if isinstance(item, dict)
        and item.get("kind") == "asset_path"
        and isinstance(item.get("path"), str)
    ]
    generated_descriptions = "\n".join(
        content
        for document in documents
        if (
            "/generated/descriptions/" in str(document.get("path") or "")
            or str(document.get("path") or "").startswith(
                ".brunn-state/generated/descriptions/"
            )
        )
        and any(
            f'"describes_path": {json.dumps(path)}'
            in (content := str(document.get("content") or ""))
            for path in described_paths
        )
    ).casefold()
    for value in card.get("native_only_values", []):
        if str(value).casefold() in searchable:
            errors.append(
                f"{card.get('id')}: native-only value leaked into searchable text: "
                f"{value}"
            )
    for value in card.get("description_forbidden_values", []):
        rendered_value = str(value).casefold()
        leaked = (
            re.search(
                rf"(?<![a-z0-9]){re.escape(rendered_value)}(?![a-z0-9])",
                generated_descriptions,
            )
            is not None
        )
        if leaked:
            errors.append(
                f"{card.get('id')}: withheld value leaked into generated description: "
                f"{value}"
            )
    return errors


def load_binary_cases() -> tuple[list[EvalCase], dict[str, Any]]:
    fixture_root, fixture = ensure_binary_fixture()
    manifest = load_json(SUITE_MANIFESTS["binary"])
    cases: list[EvalCase] = []
    errors: list[str] = []
    restricted_assets_by_scope: dict[str, frozenset[str]] = {
        scope: frozenset(
            str(item["path"])
            for card in manifest.get("cards", [])
            if isinstance(card, dict)
            and str(card.get("scope") or "personal") == scope
            for item in card.get("evidence", [])
            if isinstance(item, dict)
            and item.get("kind") == "asset_path"
            and isinstance(item.get("path"), str)
        )
        for scope in {
            str(card.get("scope") or "personal")
            for card in manifest.get("cards", [])
            if isinstance(card, dict)
        }
    }
    for source in manifest.get("cards", []):
        if not isinstance(source, dict) or source.get("active", True) is False:
            continue
        scope = str(source.get("scope") or "personal")
        scope_record = (
            fixture.get("snapshots", {})
            .get("r2", {})
            .get("scopes", {})
            .get(scope, {})
        )
        relative_root = scope_record.get("evaluation_root")
        if not isinstance(relative_root, str):
            errors.append(f"{source.get('id')}: fixture has no r2 {scope} evaluation root")
            continue
        scope_evaluation_root = fixture_root / relative_root
        evidence_rows = source.get("evidence")
        claims = source.get("claims")
        if not isinstance(evidence_rows, list) or not isinstance(claims, list):
            errors.append(f"{source.get('id')}: evidence and claims must be arrays")
            continue
        evidence = {
            str(item.get("id")): item
            for item in evidence_rows
            if isinstance(item, dict) and item.get("id")
        }
        asset_paths = tuple(sorted({
            str(item.get("path"))
            for item in evidence_rows
            if isinstance(item, dict)
            and item.get("kind") == "asset_path"
            and isinstance(item.get("path"), str)
        }))
        provision_asset_paths = asset_paths
        for path in asset_paths:
            if not (scope_evaluation_root / path).is_file():
                errors.append(f"{source.get('id')}: missing binary asset {path}")
        corpus_root, description_paths = materialize_binary_case_root(
            fixture_root,
            scope_record,
            case_id=str(source.get("id")),
            asset_paths=asset_paths,
            restricted_asset_paths=restricted_assets_by_scope.get(
                scope,
                frozenset(),
            ),
        )
        converted = dict(source)
        converted["workspace_access"] = "read_write"
        converted["grading_mode"] = "concept_tokens_v1"
        converted["claim_slots"] = {
            str(claim["id"]): str(claim["prompt"])
            for claim in claims
            if isinstance(claim, dict)
            and claim.get("id")
            and claim.get("prompt")
        }
        converted["rubric"] = []
        for claim in claims:
            if not isinstance(claim, dict) or not claim.get("id"):
                continue
            source_paths = sorted({
                (
                    description_paths.get(
                        str(evidence[evidence_id]["path"]),
                        str(evidence[evidence_id]["path"]),
                    )
                    if evidence[evidence_id].get("kind") == "generated_description"
                    else str(evidence[evidence_id]["path"])
                )
                for evidence_id in claim.get("evidence_ids", [])
                if evidence_id in evidence
                and isinstance(evidence[evidence_id].get("path"), str)
            })
            converted["rubric"].append({
                "id": claim["id"],
                "checks": claim.get("checks", []),
                "sources_any": source_paths,
                "sources_all": source_paths,
                "native_paths": sorted({
                    str(evidence[evidence_id]["path"])
                    for evidence_id in claim.get("evidence_ids", [])
                    if evidence_id in evidence
                    and evidence[evidence_id].get("kind") == "asset_path"
                    and isinstance(evidence[evidence_id].get("path"), str)
                }),
            })
        corpus_paths = frozenset(
            path.relative_to(corpus_root).as_posix()
            for path in corpus_root.rglob("*")
            if path.is_file()
        )
        eval_case = EvalCase(
            "binary",
            converted,
            corpus_root,
            corpus_paths,
            binary_asset_paths=asset_paths,
            provision_asset_paths=provision_asset_paths,
        )
        errors.extend(validate_binary_leakage(eval_case, source))
        cases.append(eval_case)
    return cases, {
        "cases": len(cases),
        "claims": sum(len(case.case["rubric"]) for case in cases),
        "corpus_root": str(fixture_root / "evaluation" / "r2"),
        "corpus_sha256": sha256_tree(fixture_root / "evaluation" / "r2"),
        "artifact_tree_sha256": sha256_file(SUITE_MANIFESTS["binary"]),
        "documents": sum(len(case.corpus_paths) for case in cases),
        "chunks": None,
        "errors": errors,
        "native_assets": sum(len(case.binary_asset_paths) for case in cases),
    }


def load_cases() -> tuple[list[EvalCase], dict[str, Any]]:
    cases: list[EvalCase] = []
    validation: dict[str, Any] = {"errors": [], "suites": {}}
    for suite in ("work", "personal", "rupture"):
        result = validate_work_manifest(SUITE_MANIFESTS[suite], SCHEMA)
        validation["errors"].extend(f"{suite}: {error}" for error in result["errors"])
        corpus_root = Path(result["corpus_root"])
        corpus_paths = frozenset(document.path for document in result["documents"])
        active = [
            case
            for case in result["manifest"]["cases"]
            if case.get("active", True)
        ]
        for case in active:
            case.setdefault(
                "grading_mode",
                result["manifest"].get("grading_mode", "exact_substring_v1"),
            )
            cases.append(EvalCase(suite, case, corpus_root, corpus_paths))
        validation["suites"][suite] = {
            "cases": len(active),
            "claims": sum(len(case["rubric"]) for case in active),
            "corpus_root": str(corpus_root),
            "corpus_sha256": result["corpus_sha256"],
            "artifact_tree_sha256": result["artifact_tree_sha256"],
            "documents": len(result["documents"]),
            "chunks": len(result["chunks"]),
        }

    transition = validate_transition_manifest(SUITE_MANIFESTS["transition"])
    validation["errors"].extend(
        f"transition: {error}" for error in transition["errors"]
    )
    work_manifest = load_json(SUITE_MANIFESTS["work"])
    transition_paths = frozenset(
        set(transition["base_paths"])
        | {case["delta_path"] for case in transition["manifest"]["cases"]}
    )
    for case in transition["manifest"]["cases"]:
        source_refs = seed_sources(case, work_manifest, transition["base_paths"])
        seed_record = transition["seed_records"][case["seed_case_id"]]
        cases.append(EvalCase(
            "transition",
            case,
            Path(transition["base_root"]),
            transition_paths,
            transition=True,
            seed_state=seed_record["workspace_checkpoint"],
            seed_source_refs=tuple(source_refs),
        ))
    validation["suites"]["transition"] = {
        "cases": len(transition["manifest"]["cases"]),
        "claims": sum(
            len(case["rubric"])
            for case in transition["manifest"]["cases"]
        ),
        "corpus_root": str(transition["base_root"]),
        "corpus_sha256": sha256_tree(Path(transition["base_root"])),
        "artifact_tree_sha256": sha256_tree(ROOT / "eval" / "transition-deltas"),
        "documents": len(transition["base_paths"]),
        "chunks": None,
        "workspace_max_completed_calls": transition["manifest"][
            "workspace_max_completed_calls"
        ],
    }
    binary_cases, binary_validation = load_binary_cases()
    cases.extend(binary_cases)
    validation["errors"].extend(
        f"binary: {error}" for error in binary_validation.pop("errors")
    )
    validation["suites"]["binary"] = binary_validation
    validation["case_count"] = len(cases)
    validation["claim_count"] = sum(
        len(eval_case.case["rubric"]) for eval_case in cases
    )
    return cases, validation


def select_cases(
    cases: Sequence[EvalCase],
    suites: Sequence[str] | None,
    requested: Sequence[str] | None,
) -> list[EvalCase]:
    selected_suites = set(suites or SUITE_MANIFESTS)
    result = [case for case in cases if case.suite in selected_suites]
    if not requested:
        return result
    requested_ids = set(requested)
    selected = [
        case
        for case in result
        if case.qualified_id in requested_ids or case.case["id"] in requested_ids
    ]
    missing = requested_ids - {
        name
        for case in selected
        for name in (case.qualified_id, case.case["id"])
    }
    if missing:
        raise ValueError(f"Unknown selected cases: {sorted(missing)}")
    return selected


def create_jobs(
    cases: Sequence[EvalCase],
    agents: Sequence[str] | None,
    interfaces: Sequence[str] | None,
    *,
    repetitions: int = 1,
    randomization_seed: int = 0,
) -> list[Job]:
    if repetitions < 1:
        raise ValueError("repetitions must be at least 1")
    selected_agents = tuple(agents or AGENTS)
    selected_interfaces = tuple(interfaces or INTERFACES)
    jobs = [
        Job(case, agent, interface, repetition)
        for case in cases
        for agent in selected_agents
        for repetition in range(1, repetitions + 1)
        for interface in selected_interfaces
    ]
    random.Random(randomization_seed).shuffle(jobs)
    return jobs


def load_provisioning(path: Path, run_id: str) -> dict[str, dict[str, Any]]:
    if not path.exists():
        return {}
    payload = json.loads(read_regular_file_bytes(path))
    if not isinstance(payload, dict):
        raise ValueError(f"Invalid provisioning state at {path}")
    if payload.get("version") != 1 or payload.get("run_id") != run_id:
        raise ValueError(f"Provisioning state does not belong to {run_id}")
    jobs = payload.get("jobs")
    if not isinstance(jobs, dict):
        raise ValueError(f"Invalid provisioning state at {path}")
    path.chmod(0o600)
    return jobs


def save_provisioning(
    path: Path,
    run_id: str,
    jobs: dict[str, dict[str, Any]],
) -> None:
    atomic_write_text(
        path,
        json.dumps({
            "version": 1,
            "run_id": run_id,
            "jobs": jobs,
        }, indent=2) + "\n",
        mode=0o600,
    )


def parent_custody_dir(run_root: Path) -> Path:
    return run_root / PARENT_CUSTODY


def ensure_parent_custody(run_root: Path) -> Path:
    custody = parent_custody_dir(run_root)
    custody.mkdir(parents=True, exist_ok=True, mode=0o700)
    file_stat = custody.lstat()
    if not stat.S_ISDIR(file_stat.st_mode) or stat.S_ISLNK(file_stat.st_mode):
        raise ValueError(f"parent custody path is not a real directory: {custody}")
    custody.chmod(0o700)
    return custody


def provision_one(
    job: Job,
    run_id: str,
    documents_by_root: dict[str, list[dict[str, Any]]],
    timeout_seconds: float,
) -> dict[str, Any]:
    case = job.eval_case.case
    seed_checkpoint = None
    delta_documents = None
    if job.eval_case.transition:
        seed_checkpoint = {
            "state": job.eval_case.seed_state,
            "source_refs": list(job.eval_case.seed_source_refs),
        }
        delta_documents = [
            file_document(ROOT / case["delta_path"], case["delta_path"])
        ]
    client = NativeApiClient(run_id=run_id, case_id=job.key)
    metadata = provision_evaluation(
        client,
        run_id=run_id,
        case_id=job.key,
        display_scope=case.get("scope", case["workload"]),
        access_mode=case.get("workspace_access", "read_write"),
        documents=documents_by_root[str(job.eval_case.corpus_root)],
        delta_documents=delta_documents,
        seed_checkpoint=seed_checkpoint,
        timeout_seconds=timeout_seconds,
    )
    if job.eval_case.transition:
        if not metadata.get("checkpoint_id"):
            raise RuntimeError(f"{job.key}: seed checkpoint was not created")
        metadata["seed_source_refs"] = list(job.eval_case.seed_source_refs)
    if job.eval_case.provision_asset_paths or job.eval_case.binary_asset_paths:
        metadata["binary_import"] = provision_binary_assets(
            metadata,
            corpus_root=job.eval_case.corpus_root,
            logical_paths=(
                job.eval_case.provision_asset_paths
                or job.eval_case.binary_asset_paths
            ),
            run_id=run_id,
            case_id=job.key,
            timeout_seconds=timeout_seconds,
            describe_binaries=False,
        )
    return metadata


async def provision_jobs(
    jobs: Sequence[Job],
    *,
    run_id: str,
    run_root: Path,
    concurrency: int,
    timeout_seconds: float,
) -> dict[str, dict[str, Any]]:
    state_path = ensure_parent_custody(run_root) / PROVISIONING_STATE
    persisted = load_provisioning(state_path, run_id)
    corpus_roots = {
        str(job.eval_case.corpus_root): job.eval_case.corpus_root
        for job in jobs
    }
    documents_by_root = {
        key: text_documents(root)
        for key, root in corpus_roots.items()
    }
    pending = []
    for job in jobs:
        existing = persisted.get(job.key)
        if existing is not None:
            if not provisioning_matches_run_case(
                existing,
                run_id=run_id,
                case_id=job.key,
                require_checkpoint=job.eval_case.transition,
            ):
                raise ValueError(f"Invalid persisted provisioning for {job.key}")
            continue
        pending.append(job)
    if not pending:
        print(f"provision: all {len(jobs)} isolated users already ready", flush=True)
        return persisted

    semaphore = asyncio.Semaphore(concurrency)
    write_lock = asyncio.Lock()
    completed = 0

    async def provision(job: Job) -> None:
        nonlocal completed
        async with semaphore:
            metadata = await asyncio.to_thread(
                provision_one,
                job,
                run_id,
                documents_by_root,
                timeout_seconds,
            )
        async with write_lock:
            persisted[job.key] = metadata
            save_provisioning(state_path, run_id, persisted)
            completed += 1
            print(
                f"provision: {completed}/{len(pending)} {job.key}",
                flush=True,
            )

    await asyncio.gather(*(provision(job) for job in pending))
    return persisted


def service_call_budget(job: Job) -> int:
    return 8 if job.eval_case.binary_asset_paths else 4


def binary_asset_specs(eval_case: EvalCase) -> list[dict[str, Any]]:
    specs = []
    for logical_path in eval_case.binary_asset_paths:
        path = eval_case.corpus_root / logical_path
        media_type = (
            "application/vnd.sqlite3"
            if path.suffix.casefold() in {".db", ".sqlite", ".sqlite3"}
            else mimetypes.guess_type(path.name)[0]
            or "application/octet-stream"
        )
        specs.append({
            "path": logical_path,
            "content_hash": f"sha256:{sha256_file(path)}",
            "size_bytes": path.stat().st_size,
            "media_type": media_type,
        })
    return specs


def common_retrieval_policy(
    read_only: bool,
    transition: bool,
    *,
    binary_assets: bool,
) -> str:
    checkpoint = (
        "This credential is read-only. Do not call checkpoint or any write operation."
        if read_only
        else (
            "Before answering, persist one child checkpoint. It must cite both a "
            "prior source and the delta source, and it must retain the parent checkpoint."
            if transition
            else "Before answering, persist one checkpoint that is useful to the next fresh agent."
        )
    )
    transition_claims = (
        "For any claim whose conclusion changes at N+1, state the changed rule in that "
        "claim and cite the delta source there. Historical parent sources do not replace "
        "the delta citation, and another claim's text or citations do not satisfy the slot."
        if transition
        else ""
    )
    native_policy = (
        """Native files are authoritative retained evidence, not model-context payloads. Locate
the exact asset reference through Brunn State, fetch it through the supplied verified asset
operation, and use local image, SQLite, archive, or hashing tools only on the returned private
path. Run `./inspect-asset "returned/private/path"` before the specialized local analysis.
A SQLite claim must come from `./inspect-asset "returned/private/path" --sqlite-query
"SELECT ..."` so the supervisor records the actual read-only query and result.
A successful fetch and inspection must match the session-pinned size and SHA-256. Do not
inspect any other local files."""
        if binary_assets
        else ""
    )
    call_budget = (
        "eight Brunn State operations"
        if binary_assets
        else "four Brunn State calls"
    )
    sequence = (
        "open, a focused query/read as needed, asset list or metadata, verified asset fetch, "
        "local analysis of that fetched file, then checkpoint"
        if binary_assets
        else "open, at most one batched query, at most one batched read, then checkpoint"
    )
    filesystem_boundary = (
        "Do not browse, search the filesystem for additional evidence, inspect an adapter, "
        "or use any evidence outside Brunn State."
        if binary_assets
        else "Do not browse, search the filesystem, inspect an adapter, or use any evidence "
        "outside Brunn State."
    )
    return f"""Treat the initial open response as the first answer packet. A `complete_source`
item already contains the full source and must not be read again. Pointer-only evidence leads
are candidates, not proof. Build a checklist from every task facet and claim slot. Query only
unresolved gaps. If a candidate path clearly owns an unresolved claim, read that exact path
before searching globally again. Copy every read `path` or `ref` verbatim from the current
Brunn State response; never synthesize a filename from a title, heading, or topic. If no exact
locator was returned, do not guess one. Use batch query or read operations for independent gaps.
Copy identifiers, timestamps, hashes, quantities, and measurements exactly. Use no more than
{call_budget} when the evidence permits. Prefer this sequence: {sequence}.
Do not alternate repeated query and read calls
when the unresolved questions or candidate paths can be batched. {filesystem_boundary}

{native_policy}

{transition_claims}

{checkpoint}"""


def filesystem_access(job: Job) -> str:
    if job.eval_case.transition:
        return """Use only `./checkpoint.json`, `./delta.json`, the exact delta source under
`./eval/transition-deltas`, and the frozen `./corpus`. The checkpoint is prior durable state,
not an infallible source; reconcile it with the changed evidence. Use ordinary filesystem
search and read tools. Cite original relative source paths, not the local `corpus/` prefix."""
    binary = (
        """ For native files, first use `./asset metadata "exact/path"`, then
`./asset fetch "exact/path"`, then `./inspect-asset "returned/local/path"` before using a
specialized local image, archive, PDF, calendar, or hashing tool. For SQLite, use
`./inspect-asset "returned/local/path" --sqlite-query "SELECT ..."`."""
        if job.eval_case.binary_asset_paths
        else ""
    )
    return f"""Use only the frozen evidence under `./corpus`. Use ordinary filesystem search,
read, and local analysis tools.{binary} Cite source paths relative to `./corpus`, without the
local `corpus/` prefix."""


def filesystem_policy() -> str:
    return """Build a checklist from every task facet and claim slot before searching. Read the
sources that own each unresolved claim, preserve exact identifiers, timestamps, hashes,
quantities, measurements, conditions, and provenance boundaries, and reconcile conflicts by
authority and freshness. Do not browse or read outside this run directory. The checkpoint
object in the final answer is a handoff projection; do not call Brunn State or any network
service."""


def cli_access(job: Job) -> str:
    case = job.eval_case.case
    initial = "resume" if job.eval_case.transition else "open"
    native_step = (
        """4. For native evidence, run `./memory assets`, then
   `./memory asset-metadata "asset:..."`, then `./memory asset-fetch "asset:..."`.
   The fetch command returns a private local path only after streaming size and SHA-256 checks.
5."""
        if job.eval_case.binary_asset_paths
        else "4."
    )
    return f"""Use only the thin Brunn State CLI at `./memory`.

1. Start with `./memory {initial}`.
2. For an unresolved gap, use a focused query such as:
   `./memory query --scope {shlex.quote(case.get('scope', case['workload']))} "question"`
3. Read exact sources with `./memory read --path "relative/path"` or batch paths in one call.
{native_step} For a writable case, persist the final state with:
   `./memory checkpoint '{{"state":{{"objective":"...","current_state":[],"decisions":[],
   "open_questions":[],"next_actions":[],"artifacts":[]}},"source_refs":["evidence:..."]}}'`

Use `evidence_refs` returned with source content as checkpoint `source_refs`; file paths are
for final-answer citations. The CLI remembers the session ID and adds checkpoint lineage
automatically."""


def mcp_access(job: Job, metadata: dict[str, Any]) -> str:
    case = job.eval_case.case
    resume = (
        f" Set `resume_checkpoint_ref` to `{metadata['checkpoint_id']}`."
        if job.eval_case.transition
        else (
            " Omit `resume_checkpoint_ref`; no exact checkpoint reference was "
            "supplied, and you must not invent one."
        )
    )
    native_tools = (
        ", `asset.list`, `asset.metadata`, and `asset.fetch`"
        if job.eval_case.binary_asset_paths
        else ""
    )
    native_step = (
        """4. For native evidence, use `asset.list` to locate an exact reference, then
   `asset.metadata` if needed and `asset.fetch`. The fetch result is a private local path whose
   bytes have been checked against session-pinned size and SHA-256. You may use local analysis
   tools only on a path returned by `asset.fetch`; do not search for other evidence on disk.
5."""
        if job.eval_case.binary_asset_paths
        else "4."
    )
    return f"""Use only the Brunn State MCP tools named `memory.open`, `memory.query`,
`memory.read`, `memory.compute`, `memory.verify`, and `memory.checkpoint`{native_tools}.
Do not use shell commands for memory or asset access.

1. Start with `memory.open`: task is the task below; hints.authorization_scope is
   `{metadata['authorization_scope']}`; as_of is `latest`; mode is `continuation`.{resume}
2. Pass the returned session ID to every query, read, compute, verify, or checkpoint call.
3. Query only focused unresolved gaps. Set query `scope` to the canonical
   `{metadata['authorization_scope']}` returned by open, or omit it. Never put a human display
   label such as `{case.get('scope', case['workload'])}` in an API scope field.
{native_step} For a writable case, call `memory.checkpoint` with idempotency key
   `interface-eval:{job.key}`, the final checkpoint state, and exact source-bearing IDs:
   `evidence_refs` returned by open/query or a `source_episode:...` returned by read as
   `source.reference`. File paths are for final-answer citations, not checkpoint source refs."""


def api_access(job: Job, metadata: dict[str, Any]) -> str:
    case = job.eval_case.case
    parent_field = (
        ',\"parent_checkpoint_id\":\"checkpoint:...\"'
        if job.eval_case.transition
        else ""
    )
    native_step = (
        """5. Native asset discovery uses
   `./api get "/v1/assets?session_id=session%3A...&limit=100"` and metadata uses
   `./api get "/v1/assets/asset%3A...?session_id=session%3A..."`. Download only the exact
   session-pinned version with:
   `./api download "/v1/assets/asset%3A.../versions/1/content?session_id=session%3A..."
   "assets/file.bin" --sha256 "sha256:..." --size 123`.
   Copy the hash, size, version, and reference from metadata. The helper rejects bytes that do
   not match and returns a private local path. Analyze only that returned path locally.
6."""
        if job.eval_case.binary_asset_paths
        else "5."
    )
    return f"""Use only the generic authenticated HTTP helper at `./api`. It performs no
memory-specific argument shaping and returns the service's raw JSON.

1. Start with `./api post /v1/memory/open @open-request.json`.
2. Copy the returned session ID into later request bodies. A focused query is:
   `./api post /v1/memory/query '{{"session_id":"session:...","queries":[{{"id":"q1",
   "query":"question","scope":{json.dumps(metadata['authorization_scope'])},"limit":8}}]}}'`
3. Read exact evidence with `/v1/memory/read` and a body containing `session_id` plus
   `requests`, for example `{{"path":"relative/path","view":"full","max_chars":20000}}`.
4. Batch unresolved questions into one query request and exact sources into one read request.
{native_step} For a writable case, use this checkpoint body shape exactly:
   `{{"session_id":"session:..."{parent_field},"state":{{"objective":"...",
   "current_state":[],"decisions":[],"open_questions":[],"next_actions":[],"artifacts":[]}},
   "source_refs":["evidence:..."],"idempotency_key":"interface-eval:{job.key}"}}`
   Replace placeholders with returned values. Use exact `source:` or `evidence:` references
   returned by source content; file paths are final-answer citations only.

You must manage the raw session and checkpoint identifiers yourself."""


def answer_contract(case: dict[str, Any]) -> str:
    slots = "\n".join(
        f"- {claim_id}: {label}"
        for claim_id, label in case["claim_slots"].items()
    )
    return f"""Return only one JSON object with this exact shape:
{{
  "answer": "concise integrated answer",
  "claims": [
    {{"id": "exact slot ID", "value": "self-contained factual answer",
      "source_paths": ["exact/relative/source/path"], "confidence": "high"}}
  ],
  "checkpoint": {{
    "objective": "...",
    "current_state": ["..."],
    "decisions": ["..."],
    "open_questions": ["..."],
    "next_actions": ["..."],
    "artifacts": ["..."]
  }}
}}

The claims array must contain exactly one self-contained entry for each slot:
{slots}

Each slot label is abbreviated. Preserve every concrete name, stable or source ID, quantity,
condition, exception, status, and provenance distinction needed to answer that task facet;
do not summarize those details away. Every claim must cite the exact path or paths that support
that claim itself, even when another claim already cites them. Do not rely on an adjacent
claim's citation or cite a merely related source. A claim about source authority must cite the
original source. Distinguish current fact from proposal, historical evidence from superseding
state, and verified results from incomplete work. Before checkpointing and answering, audit every
claim against its slot label and the task: put every decision-relevant condition and next-action
gate in that claim itself, even when the integrated answer or another claim already mentions it."""


def render_prompt(job: Job, metadata: dict[str, Any]) -> str:
    case = job.eval_case.case
    read_only = case.get("workspace_access") == "read_only"
    if job.interface == FILESYSTEM_INTERFACE:
        access = filesystem_access(job)
        policy = filesystem_policy()
    else:
        access = {
            "cli": cli_access(job),
            "mcp": mcp_access(job, metadata),
            "api": api_access(job, metadata),
        }[job.interface]
        policy = common_retrieval_policy(
            read_only,
            job.eval_case.transition,
            binary_assets=bool(job.eval_case.binary_asset_paths),
        )
    transition = (
        "You are a genuinely fresh agent continuing work at corpus revision N+1."
        if job.eval_case.transition
        else "You are a genuinely fresh agent taking over durable work from prior agents."
    )
    return f"""{transition}

{access}

{policy}

Task:
{case['task']}

{answer_contract(case)}
"""


def write_api_wrapper(
    run_dir: Path,
    *,
    run_id: str,
    case_id: str,
) -> None:
    arguments = [
        "python3",
        str(run_dir / ".helpers" / "native_http.py"),
        "--state",
        str(run_dir / "native-session.json"),
        "--run-id",
        run_id,
        "--case-id",
        case_id,
    ]
    rendered = " ".join(shlex.quote(value) for value in arguments)
    wrapper = run_dir / "api"
    wrapper.write_text(
        f"#!/bin/sh\nexec {rendered} \"$@\"\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)


def open_request(job: Job, metadata: dict[str, Any]) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "task": job.eval_case.case["task"],
        "hints": {
            "authorization_scope": metadata["authorization_scope"],
            "root_refs": [],
            "open_object_refs": [],
        },
        "as_of": "latest",
        "mode": "continuation",
    }
    if job.eval_case.transition:
        payload["resume_checkpoint_ref"] = metadata["checkpoint_id"]
    return payload


def prepare_run_dir(
    job: Job,
    metadata: dict[str, Any],
    *,
    run_id: str,
    run_root: Path,
) -> Path:
    run_dir = job.run_dir(run_root)
    if run_dir.exists() or run_dir.is_symlink():
        remove_owned_run_tree(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    helpers = run_dir / ".helpers"
    helpers.mkdir(mode=0o700)
    for helper_name in ("native_eval.py", "native_memory.py", "native_http.py"):
        helper = helpers / helper_name
        shutil.copy2(ROOT / helper_name, helper)
        helper.chmod(0o500)
    shutil.copy2(SCHEMA, run_dir / "answer-schema.json")
    (run_dir / "answer-schema.json").chmod(0o400)
    (run_dir / "prompt.txt").write_text(
        render_prompt(job, metadata),
        encoding="utf-8",
    )
    if job.interface == FILESYSTEM_INTERFACE:
        copy_frozen_tree(job.eval_case.corpus_root, run_dir / "corpus")
        if job.eval_case.transition:
            delta_path = job.eval_case.case["delta_path"]
            delta_target = run_dir / delta_path
            delta_target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / delta_path, delta_target)
            delta_target.chmod(0o400)
            (run_dir / "checkpoint.json").write_text(
                json.dumps({
                    "state": job.eval_case.seed_state,
                    "source_refs": list(job.eval_case.seed_source_refs),
                }, indent=2) + "\n",
                encoding="utf-8",
            )
            (run_dir / "delta.json").write_text(
                json.dumps({
                    "from_revision": "N",
                    "to_revision": "N+1",
                    "changed_paths": [delta_path],
                }, indent=2) + "\n",
                encoding="utf-8",
            )
    elif job.interface == "cli":
        write_native_memory_wrapper(
            run_dir,
            task=job.eval_case.case["task"],
            display_scope=job.eval_case.case.get(
                "scope",
                job.eval_case.case["workload"],
            ),
            authorization_scope=metadata["authorization_scope"],
            run_id=run_id,
            case_id=job.key,
            checkpoint_id=metadata.get("checkpoint_id"),
            script_path=helpers / "native_memory.py",
        )
    elif job.interface == "api":
        write_api_wrapper(run_dir, run_id=run_id, case_id=job.key)
        (run_dir / "open-request.json").write_text(
            json.dumps(open_request(job, metadata), indent=2) + "\n",
            encoding="utf-8",
        )
    if job.eval_case.binary_asset_paths:
        write_binary_tools(run_dir, job)
    return run_dir


def write_binary_tools(run_dir: Path, job: Job) -> None:
    (run_dir / "assets").mkdir(mode=0o700, exist_ok=True)
    inspect_helper = run_dir / ".trusted_asset_inspect.py"
    shutil.copy2(ROOT / "trusted_asset_inspect.py", inspect_helper)
    inspect_helper.chmod(0o700)
    inspect_wrapper = run_dir / "inspect-asset"
    inspect_wrapper.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        'ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
        'exec python3 "$ROOT/.trusted_asset_inspect.py" "$@"\n',
        encoding="utf-8",
    )
    inspect_wrapper.chmod(0o700)
    if job.interface != FILESYSTEM_INTERFACE:
        return
    manifest = run_dir / ".binary-assets.json"
    manifest.write_text(
        json.dumps({
            "corpus_root": str((run_dir / "corpus").resolve()),
            "assets": binary_asset_specs(job.eval_case),
        }, indent=2) + "\n",
        encoding="utf-8",
    )
    manifest.chmod(0o400)
    filesystem_helper = run_dir / ".filesystem_asset.py"
    shutil.copy2(ROOT / "filesystem_asset.py", filesystem_helper)
    filesystem_helper.chmod(0o700)
    filesystem_wrapper = run_dir / "asset"
    filesystem_wrapper.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        'ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
        'exec python3 "$ROOT/.filesystem_asset.py" '
        '--manifest "$ROOT/.binary-assets.json" '
        '--trace "$ROOT/filesystem-asset-trace.jsonl" "$@"\n',
        encoding="utf-8",
    )
    filesystem_wrapper.chmod(0o700)


def codex_gateway_config_values(
    model_base_url: str,
    *,
    direct_openai: bool,
) -> list[str]:
    provider = "evaluation_gateway"
    values = [
        f'model_provider="{provider}"',
        f'model_providers.{provider}.name="Brunn State evaluation gateway"',
        f"model_providers.{provider}.base_url={json.dumps(model_base_url)}",
        f'model_providers.{provider}.wire_api="responses"',
        f'model_providers.{provider}.env_key="BRUNN_STATE_MODEL_GATEWAY_TOKEN"',
        f"model_providers.{provider}.requires_openai_auth=true",
        f"model_providers.{provider}.supports_websockets=false",
    ]
    return values


def codex_gateway_config_args(
    model_base_url: str,
    *,
    direct_openai: bool,
) -> list[str]:
    return [
        argument
        for value in codex_gateway_config_values(
            model_base_url,
            direct_openai=direct_openai,
        )
        for argument in ("--config", value)
    ]


def codex_gateway_app_server_args(
    model_base_url: str,
    *,
    direct_openai: bool,
) -> list[str]:
    return [
        "app-server",
        "--listen",
        "stdio://",
        "--disable",
        "apps",
        "--disable",
        "plugins",
        "--disable",
        "remote_plugin",
        "--disable",
        "plugin_sharing",
        *codex_gateway_config_args(
            model_base_url,
            direct_openai=direct_openai,
        ),
    ]


def openclaw_config(
    job: Job,
    metadata: dict[str, Any],
    run_dir: Path,
    state_dir: Path,
    run_id: str,
    model_base_url: str | None = None,
    model_gateway_token: str | None = None,
    *,
    model: str = "gpt-5.6-sol",
    service_api_url: str | None = None,
    service_token: str | None = None,
) -> dict[str, Any]:
    direct_openai = False
    provider_name = "evaluation_gateway" if model_base_url else "openai"
    model_ref = f"{provider_name}/{model}"
    provider: dict[str, Any] = {
        "baseUrl": model_base_url or "https://api.openai.com/v1",
        "api": "openai-responses",
        "models": [{
            "id": model,
            "name": model,
            "api": "openai-responses",
            "reasoning": True,
            "input": ["text", "image"],
            "contextWindow": 1_050_000,
            "contextTokens": 1_000_000,
            "maxTokens": 128_000,
            "compat": {
                "supportsReasoningEffort": True,
                "supportsUsageInStreaming": True,
                "supportsTools": True,
                "supportedReasoningEfforts": [
                    "none", "low", "medium", "high", "xhigh", "max",
                ],
            },
            "cost": {
                "input": 0,
                "output": 0,
                "cacheRead": 0,
                "cacheWrite": 0,
            },
        }],
    }
    if model_base_url:
        if not model_gateway_token:
            raise ValueError("OpenClaw model gateway requires an ephemeral capability")
        provider["apiKey"] = model_gateway_token
    agent_model_config: dict[str, Any] = {
        "params": {"fastMode": False},
    }
    runtime_id = "pi" if direct_openai else "codex"
    provider["agentRuntime"] = {"id": runtime_id}
    provider["models"][0]["agentRuntime"] = {"id": runtime_id}
    agent_model_config["agentRuntime"] = {"id": runtime_id}
    config: dict[str, Any] = {
        "plugins": {
            "allow": ["codex", "openai"],
            "entries": {
                "codex": {
                    "enabled": True,
                    "config": {
                        "codexDynamicToolsLoading": "searchable",
                        "codexDynamicToolsExclude": [],
                        **(
                            {
                                "appServer": {
                                    "args": codex_gateway_app_server_args(
                                        model_base_url,
                                        direct_openai=direct_openai,
                                    ),
                                    "mode": "yolo",
                                    "sandbox": "danger-full-access",
                                    "approvalPolicy": "never",
                                },
                            }
                            if model_base_url
                            else {}
                        ),
                    },
                },
                "openai": {"enabled": True, "config": {}},
            },
            "bundledDiscovery": "compat",
        },
        "models": {
            "mode": "merge",
            "providers": {
                provider_name: provider,
            },
        },
        "agents": {
            "defaults": {
                "model": {"primary": model_ref},
                "workspace": str(run_dir),
                "thinkingDefault": "xhigh",
                "contextTokens": 1_000_000,
                "models": {
                    model_ref: agent_model_config,
                },
            },
            "list": [{
                "id": "main",
                "tools": {
                    "profile": "coding",
                    "exec": {"security": "full", "ask": "off"},
                },
            }],
        },
        "tools": {"profile": "coding"},
    }
    if job.interface == "mcp":
        config["mcp"] = {
            "servers": {
                "brunn-state": {
                    "command": "node",
                    "args": [str(MCP_SERVER)],
                    "env": {
                        "BRUNN_API_URL": (
                            service_api_url or os.environ["BRUNN_API_URL"]
                        ),
                        "BRUNN_API_TOKEN": (
                            service_token or metadata["token"]
                        ),
                        "BRUNN_EVAL_RUN": run_id,
                        "BRUNN_EVAL_CASE": job.key,
                        "BRUNN_MCP_TRACE_PATH": str(
                            run_dir / "mcp-trace.jsonl"
                        ),
                        "BRUNN_STATE_MCP_ASSET_ROOT": str(run_dir / "assets"),
                    },
                },
            },
        }
    return config


def prepare_openclaw(
    job: Job,
    metadata: dict[str, Any],
    run_dir: Path,
    run_id: str,
    model_base_url: str | None = None,
    model_gateway_token: str | None = None,
    *,
    model: str = "gpt-5.6-sol",
    service_api_url: str | None = None,
    service_token: str | None = None,
) -> tuple[Path, Path]:
    for filename in MINIMAL_OPENCLAW_FILES:
        path = run_dir / filename
        if not path.exists():
            content = (
                "Follow only the current evaluation prompt. Use no outside evidence.\n"
                if filename == "AGENTS.md"
                else "\n"
            )
            path.write_text(content, encoding="utf-8")
    state_dir = run_dir / ".openclaw-state"
    state_dir.mkdir(parents=True, exist_ok=True)
    state_dir.chmod(0o700)
    npm_link = state_dir / "npm"
    if not npm_link.exists():
        npm_link.symlink_to(OPENCLAW_NPM, target_is_directory=True)
    config_path = state_dir / "openclaw.json"
    config_path.write_text(
        json.dumps(
            openclaw_config(
                job,
                metadata,
                run_dir,
                state_dir,
                run_id,
                model_base_url,
                model_gateway_token,
                model=model,
                service_api_url=service_api_url,
                service_token=service_token,
            ),
            indent=2,
        ) + "\n",
        encoding="utf-8",
    )
    config_path.chmod(0o600)
    return state_dir, config_path


def cleanup_openclaw_state(run_dir: Path) -> None:
    if os.environ.get("BRUNN_STATE_EVAL_KEEP_AGENT_STATE") == "1":
        return
    state_dir = run_dir / ".openclaw-state"
    if state_dir.is_symlink():
        state_dir.unlink()
    elif state_dir.is_dir():
        shutil.rmtree(state_dir)


def cleanup_agent_state(run_dir: Path) -> None:
    for name in (".agent-home", ".agent-tmp"):
        path = run_dir / name
        if path.is_symlink():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)


def agent_environment(
    job: Job,
    metadata: dict[str, Any],
    *,
    run_id: str,
    run_dir: Path,
    service_api_url: str | None = None,
    service_token: str | None = None,
) -> dict[str, str]:
    allowed_host_keys = (
        "PATH",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TERM",
        "COLORTERM",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
    )
    env = {
        key: value
        for key in allowed_host_keys
        if (value := os.environ.get(key))
    }
    env.setdefault("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    agent_home = run_dir / ".agent-home"
    codex_home = agent_home / ".codex"
    agent_tmp = run_dir / ".agent-tmp"
    for path in (agent_home, codex_home, agent_tmp):
        path.mkdir(parents=True, exist_ok=True)
        path.chmod(0o700)
    env["HOME"] = str(agent_home)
    env["CODEX_HOME"] = str(codex_home)
    env["TMPDIR"] = str(agent_tmp)
    env["NO_PROXY"] = "127.0.0.1,localhost"
    env["no_proxy"] = env["NO_PROXY"]
    if job.interface != FILESYSTEM_INTERFACE:
        if not service_api_url or not service_token:
            raise ValueError("service evaluation requires a trusted broker")
        env.update({
            "BRUNN_API_URL": service_api_url,
            "BRUNN_EVAL_TOKEN": service_token,
            "BRUNN_API_TOKEN": service_token,
            "BRUNN_EVAL_RUN": run_id,
            "BRUNN_EVAL_CASE": job.key,
            "BRUNN_MCP_TRACE_PATH": str(run_dir / "mcp-trace.jsonl"),
            "BRUNN_STATE_MCP_ASSET_ROOT": str(run_dir / "assets"),
        })
    return env


def codex_command(
    job: Job,
    run_dir: Path,
    *,
    codex: Path,
    model: str,
    model_base_url: str | None = None,
    direct_openai: bool = False,
) -> list[str]:
    command = [
        str(codex.resolve()),
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--disable",
        "apps",
        "--disable",
        "plugins",
        "--disable",
        "remote_plugin",
        "--disable",
        "plugin_sharing",
        "--skip-git-repo-check",
        "--model",
        model,
        "--config",
        'model_reasoning_effort="xhigh"',
    ]
    if model_base_url:
        command.extend(codex_gateway_config_args(
            model_base_url,
            direct_openai=direct_openai,
        ))
    if job.interface == "mcp":
        forwarded = [
            "BRUNN_API_URL",
            "BRUNN_API_TOKEN",
            "BRUNN_EVAL_RUN",
            "BRUNN_EVAL_CASE",
            "BRUNN_MCP_TRACE_PATH",
            "BRUNN_STATE_MCP_ASSET_ROOT",
        ]
        command.extend([
            "--config",
            'mcp_servers.brunn-state.command="node"',
            "--config",
            f"mcp_servers.brunn-state.args={json.dumps([str(MCP_SERVER)])}",
            "--config",
            f"mcp_servers.brunn-state.env_vars={json.dumps(forwarded)}",
            "--config",
            "mcp_servers.brunn-state.startup_timeout_sec=30",
            "--config",
            'mcp_servers.brunn-state.default_tools_approval_mode="approve"',
        ])
    command.extend([
        "--dangerously-bypass-approvals-and-sandbox",
        "--cd",
        str(run_dir),
        "--output-schema",
        str(run_dir / "answer-schema.json"),
        "--output-last-message",
        str(run_dir / "answer.json"),
        "--json",
        "-",
    ])
    return command


def seatbelt_quote(value: str) -> str:
    return json.dumps(value)


def write_evidence_sandbox_profile(
    run_dir: Path,
    *,
    allowed_urls: Sequence[str] = (),
) -> Path:
    resolved_run_dir = run_dir.resolve()
    denied_data_roots = (
        Path.home(),
        Path("/Users/Shared"),
        Path("/Volumes"),
        Path("/private/tmp"),
        Path("/tmp"),
    )
    readable_exceptions = (
        DEFAULT_CODEX,
        DEFAULT_CODEX.resolve(),
        Path("/Applications/ChatGPT.app"),
        Path("/Applications/Codex.app"),
        OPENCLAW,
        OPENCLAW.resolve(),
        OPENCLAW.resolve().parent,
        OPENCLAW.parent,
        OPENCLAW_NPM,
        MCP_SERVER.parent,
        ROOT / "apps" / "mcp" / "node_modules",
    )
    readable_traversal_directories = tuple(sorted({
        parent
        for path in readable_exceptions
        if path.exists()
        for parent in path.resolve().parents
        if parent != Path("/")
    }, key=lambda path: (len(path.parts), str(path))))
    dangerous_executables = tuple({
        path.resolve() if path.exists() else path
        for path in (
            Path("/bin/launchctl"),
            Path("/usr/bin/open"),
            Path("/usr/bin/osascript"),
            Path("/usr/bin/security"),
            Path("/usr/bin/shortcuts"),
            Path("/usr/bin/sandbox-exec"),
            Path("/usr/bin/lldb"),
            Path("/usr/bin/dtrace"),
            Path("/usr/bin/sample"),
            Path("/usr/bin/vmmap"),
            Path("/usr/bin/leaks"),
            Path("/usr/bin/ps"),
            Path("/bin/ps"),
            Path("/usr/sbin/lsof"),
            Path("/usr/local/bin/docker"),
            Path("/opt/homebrew/bin/docker"),
            Path.home() / ".docker" / "bin" / "docker",
            Path.home() / ".local" / "bin" / "docker",
            Path("/usr/local/bin/colima"),
            Path("/opt/homebrew/bin/colima"),
            Path("/usr/local/bin/limactl"),
            Path("/opt/homebrew/bin/limactl"),
        )
    })
    allowed_ports = sorted({
        parsed.port
        for url in allowed_urls
        if (parsed := urllib.parse.urlsplit(url)).hostname
        in {"127.0.0.1", "localhost"}
        and parsed.port is not None
    })
    traversal_directories = tuple(
        parent
        for parent in resolved_run_dir.parents
        if parent != Path("/")
    )
    lines = [
        "(version 1)",
        "(allow default)",
        "(deny network-outbound)",
        *[
            f'(allow network-outbound (remote ip "localhost:{port}"))'
            for port in allowed_ports
        ],
        *[
            (
                "(deny file-read* file-write* (subpath "
                + f"{seatbelt_quote(str(path.resolve()))}))"
            )
            for path in denied_data_roots
            if path.exists()
        ],
        *[
            (
                "(deny process-exec (literal "
                f"{seatbelt_quote(str(path))}))"
            )
            for path in dangerous_executables
        ],
        '(deny mach-lookup (global-name "com.apple.securityd"))',
        '(deny mach-lookup (global-name "com.apple.securityd.xpc"))',
        '(deny mach-lookup (global-name "com.apple.trustd.agent"))',
        '(deny mach-lookup (global-name "com.apple.coreservices.launchservicesd"))',
        '(deny mach-lookup (global-name-regex #"^com\\.apple\\.pasteboard\\."))',
        "(deny mach-task-name)",
        "(deny appleevent-send)",
        *[
            (
                "(allow file-read* (literal "
                + f"{seatbelt_quote(str(path))}))"
            )
            for path in traversal_directories
        ],
        *[
            (
                "(allow file-read-metadata (literal "
                + f"{seatbelt_quote(str(path))}))"
            )
            for path in readable_traversal_directories
        ],
        (
            "(allow file-read* file-write* (subpath "
            f"{seatbelt_quote(str(run_dir.resolve()))}))"
        ),
        (
            "(deny file-write* (subpath "
            f"{seatbelt_quote(str((run_dir / 'corpus').resolve()))}))"
        ),
        *[
            (
                "(allow file-read* ("
                + ("subpath " if path.is_dir() else "literal ")
                + f"{seatbelt_quote(str(path.resolve()))}))"
            )
            for path in readable_exceptions
            if path.exists()
        ],
    ]
    profile = run_dir / ".evidence-sandbox.sb"
    profile.write_text("\n".join(lines) + "\n", encoding="utf-8")
    profile.chmod(0o400)
    return profile


def verify_evidence_sandbox_profile(
    profile: Path,
    run_dir: Path,
    *,
    protected_paths: Sequence[Path],
) -> dict[str, Any]:
    sandbox = Path("/usr/bin/sandbox-exec")
    if not sandbox.is_file():
        raise RuntimeError(
            "sandbox-exec is required for evidence-isolated evaluation runs"
        )

    def probe(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(sandbox), "-f", str(profile), *command],
            cwd=run_dir,
            env={
                "PATH": "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                "HOME": str(run_dir / ".agent-home"),
                "TMPDIR": str(run_dir / ".agent-tmp"),
            },
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )

    readable = run_dir / "prompt.txt"
    allow_probe = probe(("/bin/cat", str(readable)))
    checks: list[dict[str, Any]] = [{
        "name": "run_copy_readable",
        "pass": allow_probe.returncode == 0,
        "returncode": allow_probe.returncode,
    }]
    for index, path in enumerate(protected_paths):
        if not path.is_file():
            continue
        result = probe(("/bin/cat", str(path)))
        checks.append({
            "name": f"protected_path_{index}_denied",
            "pass": result.returncode != 0
            and "Operation not permitted" in result.stderr,
            "returncode": result.returncode,
        })
    for name, command in (
        ("launchd_escape_denied", ("/bin/launchctl", "print", "system")),
        ("keychain_escape_denied", ("/usr/bin/security", "list-keychains")),
    ):
        if not Path(command[0]).exists():
            continue
        result = probe(command)
        checks.append({
            "name": name,
            "pass": result.returncode != 0
            and "Operation not permitted" in result.stderr,
            "returncode": result.returncode,
        })
    docker = next(
        (
            path
            for path in (
                Path("/usr/local/bin/docker"),
                Path("/opt/homebrew/bin/docker"),
                Path.home() / ".docker" / "bin" / "docker",
            )
            if path.exists()
        ),
        None,
    )
    if docker is not None:
        result = probe((str(docker), "version"))
        checks.append({
            "name": "container_daemon_escape_denied",
            "pass": result.returncode != 0
            and "Operation not permitted" in result.stderr,
            "returncode": result.returncode,
        })
    result = {
        "mode": "macos_seatbelt_screening",
        "pass": bool(checks) and all(item["pass"] for item in checks),
        "checks": checks,
        "publishable_containment": False,
        "limitation": (
            "Seatbelt is a same-user screening boundary, not a separately "
            "credentialed VM or OS account."
        ),
    }
    if not result["pass"]:
        failed = ", ".join(
            item["name"] for item in checks if not item["pass"]
        )
        raise RuntimeError(f"evidence sandbox preflight failed closed: {failed}")
    return result


def sandboxed_command(
    command: Sequence[str],
    run_dir: Path,
    *,
    allowed_urls: Sequence[str] = (),
    profile: Path | None = None,
) -> list[str]:
    executable = Path("/usr/bin/sandbox-exec")
    if not executable.is_file():
        raise RuntimeError(
            "sandbox-exec is required for evidence-isolated evaluation runs"
        )
    profile = profile or write_evidence_sandbox_profile(
        run_dir,
        allowed_urls=allowed_urls,
    )
    return [str(executable), "-f", str(profile), *command]


def openclaw_command(
    run_dir: Path,
    *,
    session_id: str,
    model: str,
    timeout_seconds: int,
    provider: str = "openai",
) -> list[str]:
    return [
        str(OPENCLAW.resolve()),
        "agent",
        "--local",
        "--session-id",
        session_id,
        "--agent",
        "main",
        "--model",
        f"{provider}/{model}",
        "--thinking",
        "xhigh",
        "--timeout",
        str(timeout_seconds),
        "--json",
        "--message",
        (run_dir / "prompt.txt").read_text(encoding="utf-8"),
    ]


async def communicate(
    command: Sequence[str],
    *,
    cwd: Path,
    env: dict[str, str],
    stdin: bytes | None,
    timeout_seconds: int,
) -> tuple[int, bool, float, bytes, bytes]:
    started = time.monotonic()
    process = await asyncio.create_subprocess_exec(
        *command,
        cwd=cwd,
        env=env,
        stdin=asyncio.subprocess.PIPE if stdin is not None else None,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = await asyncio.wait_for(
            process.communicate(stdin),
            timeout=timeout_seconds,
        )
        timed_out = False
    except asyncio.TimeoutError:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stdout, stderr = await process.communicate()
        timed_out = True
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    else:
        await asyncio.sleep(0.1)
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    return (
        process.returncode,
        timed_out,
        time.monotonic() - started,
        stdout,
        stderr,
    )


def parse_json_stdout(raw: bytes) -> dict[str, Any]:
    text = raw.decode("utf-8", errors="replace").strip()
    try:
        parsed = json.loads(text)
        if isinstance(parsed, dict):
            return parsed
    except json.JSONDecodeError:
        pass
    decoder = json.JSONDecoder()
    candidates: list[dict[str, Any]] = []
    for match in re.finditer(r"\{", text):
        try:
            value, _ = decoder.raw_decode(text[match.start():])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            candidates.append(value)
    if not candidates:
        raise ValueError("OpenClaw did not emit a JSON result")
    return candidates[-1]


def openclaw_answer(result: dict[str, Any]) -> str:
    meta = result.get("meta") if isinstance(result.get("meta"), dict) else {}
    for key in ("finalAssistantVisibleText", "finalAssistantRawText"):
        value = meta.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    payloads = result.get("payloads")
    if isinstance(payloads, list):
        for payload in reversed(payloads):
            if isinstance(payload, dict):
                text = payload.get("text")
                if isinstance(text, str) and text.strip():
                    return text.strip()
    raise ValueError("OpenClaw result did not contain assistant text")


def nested_number(value: Any, names: set[str]) -> int:
    if isinstance(value, dict):
        for key, child in value.items():
            if key in names and isinstance(child, int):
                return child
        return max(
            (nested_number(child, names) for child in value.values()),
            default=0,
        )
    if isinstance(value, list):
        return max((nested_number(child, names) for child in value), default=0)
    return 0


def openclaw_metrics(result: dict[str, Any]) -> dict[str, Any]:
    meta = result.get("meta") if isinstance(result.get("meta"), dict) else {}
    agent_meta = (
        meta.get("agentMeta")
        if isinstance(meta.get("agentMeta"), dict)
        else {}
    )
    usage = (
        agent_meta.get("usage")
        if isinstance(agent_meta.get("usage"), dict)
        else {}
    )
    uncached = int(usage.get("input") or 0)
    cached = int(usage.get("cacheRead") or 0)
    cache_write = int(usage.get("cacheWrite") or 0)
    output = int(usage.get("output") or 0)
    prompt_report = (
        meta.get("systemPromptReport")
        if isinstance(meta.get("systemPromptReport"), dict)
        else {}
    )
    system_prompt = (
        prompt_report.get("systemPrompt")
        if isinstance(prompt_report.get("systemPrompt"), dict)
        else {}
    )
    tool_summary = (
        meta.get("toolSummary")
        if isinstance(meta.get("toolSummary"), dict)
        else {}
    )
    calls = nested_number(tool_summary, {"calls", "callCount", "count"})
    return {
        "events": 1,
        "commands": calls,
        "command_output_chars": 0,
        "tokens": {
            "input_tokens": uncached + cached + cache_write,
            "cached_input_tokens": cached,
            "cache_write_input_tokens": cache_write,
            "output_tokens": output,
            "uncached_input_tokens": uncached,
        },
        "system_prompt_chars": int(system_prompt.get("chars") or 0),
        "project_context_chars": int(
            system_prompt.get("projectContextChars") or 0
        ),
        "non_project_context_chars": int(
            system_prompt.get("nonProjectContextChars") or 0
        ),
        "reported_prompt_tokens": int(agent_meta.get("promptTokens") or 0),
        "duration_ms": int(meta.get("durationMs") or 0),
        "tool_summary": tool_summary,
    }


def model_gateway_contract(
    requests: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    invalid_requests = [
        index
        for index, request in enumerate(requests)
        if request.get("allowed") is not True
        or request.get("contract_valid") is not True
    ]
    generation_requests = [
        request
        for request in requests
        if request.get("method") == "POST"
        and request.get("path") in {
            "/v1/responses",
            "/v1/responses/compact",
            "/backend-api/codex/responses",
            "/backend-api/codex/responses/compact",
        }
    ]
    failed = [
        index
        for index, request in enumerate(generation_requests)
        if request.get("allowed") is not True
        or request.get("contract_valid") is not True
        or int(request.get("upstream_http_status") or 0) not in range(200, 300)
        or request.get("response_body_complete") is not True
    ]
    missing_usage = [
        index
        for index, request in enumerate(generation_requests)
        if not isinstance(request.get("usage"), dict)
    ]
    return {
        "generation_requests": len(generation_requests),
        "complete": (
            bool(generation_requests)
            and not invalid_requests
            and not failed
            and not missing_usage
        ),
        "invalid_request_indexes": invalid_requests,
        "failed_request_indexes": failed,
        "missing_usage_indexes": missing_usage,
        "token_accounting_complete": (
            bool(generation_requests)
            and not invalid_requests
            and not failed
            and not missing_usage
        ),
        "proof_source": "parent_model_gateway_dechunked_response_receipts",
    }


def trusted_model_usage(requests: Sequence[dict[str, Any]]) -> dict[str, int] | None:
    contract = model_gateway_contract(requests)
    if not contract["token_accounting_complete"]:
        return None
    usages = [
        request["usage"]
        for request in requests
        if request.get("method") == "POST"
        and isinstance(request.get("usage"), dict)
    ]
    keys = (
        "input_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "output_tokens",
        "uncached_input_tokens",
    )
    return {
        key: sum(int(usage.get(key, 0)) for usage in usages)
        for key in keys
    }


def normalize_mcp_trace(run_dir: Path) -> None:
    trace_path = run_dir / "mcp-trace.jsonl"
    if not trace_path.exists():
        return
    operations: list[dict[str, Any]] = []
    state: dict[str, Any] = {
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "session_id": None,
        "corpus_revision": None,
        "checkpoint_id": None,
        "checkpoint": None,
        "operations": operations,
    }
    for line in trace_path.read_text(
        encoding="utf-8",
        errors="replace",
    ).splitlines():
        try:
            operation = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(operation, dict):
            continue
        operations.append(operation)
        for key in ("session_id", "corpus_revision", "checkpoint_id"):
            value = operation.get(key)
            if isinstance(value, str) and value:
                state[key] = value
    if state["checkpoint_id"]:
        state["checkpoint"] = {"checkpoint_id": state["checkpoint_id"]}
    state_path = run_dir / "native-session.json"
    atomic_write_text(
        state_path,
        json.dumps(state, indent=2) + "\n",
        mode=0o600,
    )


def write_trusted_service_state(run_dir: Path, broker: ServiceBroker) -> None:
    checkpoint_id = nested_string(
        broker.checkpoint,
        {"checkpoint_id", "id"},
    )
    state = {
        "format": "brunn-state-supervisor-service-receipts-v1",
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "session_id": broker.session_id,
        "corpus_revision": broker.corpus_revision,
        "checkpoint_id": checkpoint_id,
        "checkpoint": broker.checkpoint,
        "operations": broker.records,
    }
    state_path = run_dir / "native-session.json"
    atomic_write_text(
        state_path,
        json.dumps(state, indent=2) + "\n",
        mode=0o400,
    )


def attach_trusted_service_metrics(record: dict[str, Any]) -> None:
    operations = [
        operation
        for operation in record.get("trusted_service_operations", [])
        if isinstance(operation, dict)
        and operation.get("trusted_broker_receipt") is True
    ]
    record["service_operations"] = operations
    record["service_calls"] = len(operations)
    record["service_http_calls"] = sum(
        int(operation.get("http_calls", 1)) for operation in operations
    )
    record["service_binary_bytes"] = sum(
        int(operation.get("binary_bytes", 0)) for operation in operations
    )
    record["service_result_chars"] = sum(
        int(operation.get("result_chars", 0)) for operation in operations
    )
    record["service_source_text_chars"] = sum(
        int(operation.get("source_text_chars", 0)) for operation in operations
    )
    record["service_metadata_chars"] = sum(
        int(operation.get("metadata_chars", 0)) for operation in operations
    )
    record["service_replay_weighted_chars"] = sum(
        int(operation.get("result_chars", 0)) * (len(operations) - index)
        for index, operation in enumerate(operations)
    )
    record["service_latency_ms"] = round(
        sum(float(operation.get("elapsed_ms", 0)) for operation in operations),
        3,
    )
    record["service_checkpoint"] = record.get("trusted_service_checkpoint")
    record["service_session_id"] = record.get("trusted_service_session_id")
    record["service_corpus_revision"] = record.get(
        "trusted_service_corpus_revision"
    )
    record["workspace_operations"] = operations
    record["workspace_result_chars"] = record["service_result_chars"]
    record["workspace_checkpoint"] = record["service_checkpoint"]
    record["workspace_state_error"] = None


def is_write_operation(operation: dict[str, Any]) -> bool:
    return operation.get("route_class") == "write"


def redact_value(value: Any, secret: str) -> Any:
    if isinstance(value, dict):
        return {
            key: redact_value(child, secret)
            for key, child in value.items()
            if key != "token"
        }
    if isinstance(value, list):
        return [redact_value(child, secret) for child in value]
    if isinstance(value, str) and secret:
        return value.replace(secret, "[REDACTED]")
    return value


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    values = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            values.append(value)
    return values


def successful_operation(operation: dict[str, Any]) -> bool:
    status = int(operation.get("http_status", 200) or 0)
    service_status = str(operation.get("service_status") or "complete")
    return status in range(200, 300) and service_status not in {
        "failed", "error", "asset_download_verification_failed",
    }


def exact_asset_match(
    operation: dict[str, Any],
    expected: dict[str, Any],
    *,
    service: bool,
) -> bool:
    content_hash = operation.get(
        "asset_content_hash" if service else "content_hash"
    )
    size = operation.get(
        "asset_size_bytes" if service else "size_bytes"
    )
    if content_hash != expected["content_hash"] or size != expected["size_bytes"]:
        return False
    if not service:
        return operation.get("path") == expected["path"]
    return (
        operation.get("asset_ref") == expected.get("asset_ref")
        and operation.get("asset_version") == expected.get("version")
    )


def sqlite_receipts_support_values(
    inspections: Sequence[dict[str, Any]],
    required_values: Sequence[Any],
) -> bool:
    if not required_values:
        return False
    rendered = "\n".join(
        json.dumps(
            operation.get("sqlite_result"),
            ensure_ascii=False,
            sort_keys=True,
        )
        for operation in inspections
        if operation.get("operation") == "trusted_sqlite_query"
        and isinstance(operation.get("sqlite_result"), dict)
    ).casefold()
    return all(
        re.search(
            rf"(?<![a-z0-9]){re.escape(str(value).casefold())}(?![a-z0-9])",
            rendered,
        )
        is not None
        for value in required_values
    )


def grade_binary_access(
    job: Job,
    metadata: dict[str, Any],
    record: dict[str, Any],
    run_dir: Path,
) -> dict[str, Any]:
    local_specs = {
        item["path"]: item for item in binary_asset_specs(job.eval_case)
    }
    service = job.interface != FILESYSTEM_INTERFACE
    if service:
        imported = metadata.get("binary_import", {}).get("assets", [])
        expected_assets = []
        for item in imported:
            if not isinstance(item, dict) or item.get("path") not in local_specs:
                continue
            expected = {**local_specs[item["path"]], **item}
            expected_assets.append(expected)
    else:
        expected_assets = list(local_specs.values())
    operations = record.get("service_operations", []) if service else []
    inspections = record.get("trusted_asset_inspections", [])
    required_native_values = job.eval_case.case.get(
        "description_forbidden_values",
        [],
    )
    results = []
    for expected in expected_assets:
        matching_inspections = [
            operation
            for operation in inspections
            if operation.get("operation")
            in {"trusted_local_inspection", "trusted_sqlite_query"}
            and operation.get("verified") is True
            and expected["path"] in operation.get("logical_paths", [])
            and operation.get("content_hash") == expected["content_hash"]
            and operation.get("size_bytes") == expected["size_bytes"]
            and int(operation.get("bytes_read", 0)) == expected["size_bytes"]
        ]
        if service:
            metadata_hit = any(
                successful_operation(operation)
                and exact_asset_match(operation, expected, service=True)
                and str(operation.get("operation") or "") == "asset.metadata"
                and operation.get("trusted_broker_receipt") is True
                for operation in operations
            )
            fetch_hit = any(
                successful_operation(operation)
                and exact_asset_match(operation, expected, service=True)
                and str(operation.get("operation") or "") == "asset.fetch"
                and operation.get("trusted_broker_receipt") is True
                and int(operation.get("binary_bytes", 0))
                == expected["size_bytes"]
                for operation in operations
            )
        else:
            metadata_hit = any(
                operation.get("operation") == "filesystem_asset.metadata"
                and expected["path"] in operation.get("logical_paths", [])
                for operation in inspections
            )
            fetch_hit = any(
                operation.get("operation") == "filesystem_asset.fetch"
                and operation.get("verified") is True
                and expected["path"] in operation.get("logical_paths", [])
                and operation.get("content_hash") == expected["content_hash"]
                and operation.get("size_bytes") == expected["size_bytes"]
                and int(operation.get("bytes_read", 0))
                == expected["size_bytes"]
                for operation in inspections
            )
        required_inspection = (
            "trusted_sqlite_query"
            if expected["media_type"] == "application/vnd.sqlite3"
            else "trusted_local_inspection"
        )
        byte_inspection_hit = any(
            operation.get("operation") == required_inspection
            for operation in matching_inspections
        )
        semantic_inspection_hit = (
            sqlite_receipts_support_values(
                matching_inspections,
                required_native_values,
            )
            if expected["media_type"] == "application/vnd.sqlite3"
            else False
        )
        inspection_hit = (
            semantic_inspection_hit
            if expected["media_type"] == "application/vnd.sqlite3"
            else byte_inspection_hit
        )
        results.append({
            "path": expected["path"],
            "asset_ref": expected.get("asset_ref"),
            "version": expected.get("version"),
            "content_hash": expected["content_hash"],
            "size_bytes": expected["size_bytes"],
            "metadata": metadata_hit,
            "verified_fetch": fetch_hit,
            "verified_byte_inspection": byte_inspection_hit,
            "local_inspection": inspection_hit,
            "semantic_inspection": semantic_inspection_hit,
            "semantic_evidence_complete": semantic_inspection_hit,
            "pass": metadata_hit and fetch_hit and inspection_hit,
        })
    return {
        "pass": bool(expected_assets) and all(item["pass"] for item in results),
        "semantic_evidence_complete": (
            bool(expected_assets)
            and all(item["semantic_evidence_complete"] for item in results)
        ),
        "assets": results,
        "proof_source": (
            "parent_exact_byte_receipts_with_claim_bound_sqlite_results; "
            "non-SQLite semantic use remains screening-only"
        ),
    }


def apply_native_access_gate(grade: dict[str, Any], binary_access: dict[str, Any]) -> None:
    failed_paths = {
        str(asset["path"])
        for asset in binary_access.get("assets", [])
        if not asset.get("pass")
    }
    for claim in grade.get("claims", []):
        native_paths = set(claim.get("native_paths", []))
        missing = sorted(native_paths & failed_paths)
        claim["native_access_correlated"] = not missing
        claim["missing_native_paths"] = missing
        if missing:
            claim["pre_native_access_score"] = claim.get("score")
            claim["score"] = 0.0
            claim["pass"] = False
    recompute_grade(grade)


def correlate_service_citations(record: dict[str, Any]) -> None:
    grade = record.get("grade")
    if not isinstance(grade, dict):
        return
    accessed = {
        str(path)
        for operation in record.get("service_operations", [])
        for path in operation.get("source_paths", [])
        if isinstance(path, str)
    }
    grade["accessed_source_paths"] = sorted(accessed)
    for claim in grade.get("claims", []):
        citations = [
            normalize_path
            for citation in claim.get("citations", [])
            if (normalize_path := citation.strip().removeprefix("./").removeprefix("corpus/"))
        ]
        claim["access_correlated"] = all(
            citation in accessed for citation in citations
        )
        if not claim["access_correlated"]:
            claim["pre_access_score"] = claim.get("score")
            claim["score"] = 0.0
            claim["pass"] = False
    recompute_grade(grade)


def finalize_record(
    record: dict[str, Any],
    job: Job,
    metadata: dict[str, Any],
    *,
    run_id: str,
    run_dir: Path,
) -> dict[str, Any]:
    answer_path = run_dir / "answer.json"
    if answer_path.exists():
        try:
            raw_answer = read_regular_file_bytes(answer_path).decode("utf-8").strip()
            if raw_answer.startswith("```"):
                raw_answer = re.sub(r"^```(?:json)?\s*", "", raw_answer)
                raw_answer = re.sub(r"\s*```$", "", raw_answer)
            answer = json.loads(raw_answer)
            if not isinstance(answer, dict):
                raise ValueError("answer must be a JSON object")
            record["answer"] = answer
            if job.eval_case.transition:
                record["grade"] = grade_transition_answer(
                    job.eval_case.case,
                    answer,
                    set(job.eval_case.corpus_paths),
                )
            else:
                record["grade"] = grade_answer(
                    job.eval_case.case,
                    answer,
                    set(job.eval_case.corpus_paths),
                )
        except (json.JSONDecodeError, OSError, TypeError, ValueError) as exc:
            record["error"] = f"Invalid answer JSON: {exc}"
            record["grade"] = None
    else:
        record["grade"] = None
        if not record.get("error"):
            record["error"] = "Agent produced no answer"

    attach_trusted_service_metrics(record)
    if job.interface != FILESYSTEM_INTERFACE:
        expected_revision = metadata.get("corpus_revision")
        actual_revision = record.get("service_corpus_revision")
        record["expected_corpus_revision"] = expected_revision
        record["service_corpus_revision_matched"] = bool(
            expected_revision
            and actual_revision
            and expected_revision == actual_revision
        )
    if job.interface != FILESYSTEM_INTERFACE:
        correlate_service_citations(record)
    if job.eval_case.binary_asset_paths:
        binary_access = grade_binary_access(
            job,
            metadata,
            record,
            run_dir,
        )
        record["binary_access"] = binary_access
        record["binary_semantic_evidence_complete"] = binary_access[
            "semantic_evidence_complete"
        ]
        if isinstance(record.get("grade"), dict):
            record["grade"]["content_pass_before_native_gate"] = bool(
                record["grade"].get("pass")
            )
            record["grade"]["native_access"] = binary_access
            apply_native_access_gate(record["grade"], binary_access)
    record["run_id"] = run_id
    record["job_key"] = job.key
    record["answer_pass"] = bool((record.get("grade") or {}).get("pass"))
    if job.interface == FILESYSTEM_INTERFACE:
        record["checkpoint_required"] = False
        record["checkpoint_persisted"] = False
        record["read_only_write_attempted"] = False
        record["workflow_pass"] = record["answer_pass"]
    elif job.eval_case.transition and record.get("grade"):
        attach_native_lineage(
            record,
            job.eval_case.case,
            metadata,
            4,
        )
        record["checkpoint_required"] = True
        record["checkpoint_persisted"] = bool(
            (record.get("trusted_checkpoint_verification") or {}).get(
                "persisted"
            )
        )
        record["workflow_pass"] = bool(
            record.get("transition_pass")
            and record["checkpoint_persisted"]
        )
    else:
        read_only = (
            job.eval_case.case.get("workspace_access") == "read_only"
        )
        write_attempted = any(
            is_write_operation(operation)
            for operation in record.get("service_operations", [])
        )
        checkpoint_persisted = bool(record.get("service_checkpoint"))
        checkpoint_verified = bool(
            (record.get("trusted_checkpoint_verification") or {}).get(
                "persisted"
            )
        )
        record["checkpoint_required"] = not read_only
        record["checkpoint_persisted"] = (
            checkpoint_persisted and checkpoint_verified
        )
        record["read_only_write_attempted"] = read_only and write_attempted
        record["workflow_pass"] = bool(
            record["answer_pass"]
            and (read_only or record["checkpoint_persisted"])
            and not record["read_only_write_attempted"]
        )
    if (
        job.interface == FILESYSTEM_INTERFACE
        and record.get("frozen_corpus_unchanged") is not True
    ):
        record["workflow_pass"] = False
        record["evidence_isolation_error"] = (
            "frozen filesystem evidence changed during the run"
        )
    if (
        job.interface != FILESYSTEM_INTERFACE
        and record.get("service_corpus_revision_matched") is not True
    ):
        record["workflow_pass"] = False
        record["evidence_isolation_error"] = (
            "agent session did not remain pinned to the provisioned corpus revision"
        )
    if (
        job.interface != FILESYSTEM_INTERFACE
        and record.get("checkpoint_required")
        and record.get("checkpoint_persisted") is not True
    ):
        record["workflow_pass"] = False
        record["checkpoint_verification_error"] = (
            "checkpoint was not confirmed through the parent-owned service client"
        )
    if not (record.get("trusted_model_gateway_contract") or {}).get("complete"):
        record["workflow_pass"] = False
        record["model_contract_error"] = (
            "one or more model requests or responses lacked a complete "
            "parent-validated contract"
        )
    if record.get("token_accounting_complete") is not True:
        record["workflow_pass"] = False
        record["token_accounting_error"] = (
            "parent-owned usage receipts were incomplete"
        )
    if not (record.get("containment") or {}).get("pass"):
        record["workflow_pass"] = False
        record["containment_error"] = (
            "the evidence sandbox did not pass its fail-closed preflight"
        )
    if record.get("credential_scope_complete") is not True:
        record["workflow_pass"] = False
        record["credential_scope_error"] = (
            "the service credential was not proven to be scoped to this exact job"
        )
    call_budget = service_call_budget(job)
    record["service_call_budget"] = call_budget
    record["within_four_calls"] = (
        True
        if job.interface == FILESYSTEM_INTERFACE
        else record.get("service_calls", 0) <= 4
    )
    record["within_call_budget"] = (
        True
        if job.interface == FILESYSTEM_INTERFACE
        else record.get("service_calls", 0) <= call_budget
    )
    sanitized = redact_value(record, metadata["token"])
    record_path = run_dir / "record.json"
    atomic_write_text(
        record_path,
        json.dumps(sanitized, indent=2) + "\n",
        mode=0o400,
    )
    return sanitized


def seal_run_dir(run_dir: Path) -> None:
    for path in sorted(run_dir.rglob("*"), reverse=True):
        if path.is_symlink():
            raise ValueError(f"evaluation run contains an unexpected symlink: {path}")
        path.chmod(0o500 if path.is_dir() else 0o400)
    run_dir.chmod(0o500)


def record_seal_path(run_root: Path, job: Job) -> Path:
    key = hashlib.sha256(job.key.encode("utf-8")).hexdigest()
    return ensure_parent_custody(run_root) / "record-seals" / f"{key}.json"


def canonical_record_path(run_root: Path, job: Job) -> Path:
    key = hashlib.sha256(job.key.encode("utf-8")).hexdigest()
    return ensure_parent_custody(run_root) / "records" / f"{key}.json"


def record_hmac_key(run_root: Path) -> bytes:
    key_path = ensure_parent_custody(run_root) / RECORD_KEY
    if not key_path.exists():
        atomic_write_bytes(key_path, secrets.token_bytes(32), mode=0o400)
    key = read_regular_file_bytes(key_path, max_bytes=64)
    if len(key) != 32:
        raise ValueError("parent record HMAC key has an invalid length")
    return key


def record_hmac(
    key: bytes,
    *,
    run_id: str,
    job_key: str,
    record_bytes: bytes,
) -> str:
    payload = (
        run_id.encode("utf-8")
        + b"\0"
        + job_key.encode("utf-8")
        + b"\0"
        + record_bytes
    )
    return f"hmac-sha256:{hmac.new(key, payload, hashlib.sha256).hexdigest()}"


def write_record_seal(
    run_root: Path,
    job: Job,
    *,
    run_id: str,
    record_path: Path,
) -> None:
    record_bytes = read_regular_file_bytes(record_path)
    canonical_path = canonical_record_path(run_root, job)
    atomic_write_bytes(canonical_path, record_bytes, mode=0o400)
    key = record_hmac_key(run_root)
    seal_path = record_seal_path(run_root, job)
    payload = {
        "format": "brunn-state-parent-record-seal-v2",
        "run_id": run_id,
        "job_key": job.key,
        "record_sha256": f"sha256:{hashlib.sha256(record_bytes).hexdigest()}",
        "record_hmac": record_hmac(
            key,
            run_id=run_id,
            job_key=job.key,
            record_bytes=record_bytes,
        ),
    }
    atomic_write_text(
        seal_path,
        json.dumps(payload, sort_keys=True) + "\n",
        mode=0o400,
    )


async def run_job(
    semaphore: asyncio.Semaphore,
    job: Job,
    metadata: dict[str, Any],
    *,
    run_id: str,
    run_root: Path,
    codex: Path,
    model: str,
    timeout_seconds: int,
) -> dict[str, Any]:
    async with semaphore:
        run_dir = prepare_run_dir(
            job,
            metadata,
            run_id=run_id,
            run_root=run_root,
        )
        stale_seal = record_seal_path(run_root, job)
        if stale_seal.exists():
            stale_seal.unlink()
        stale_record = canonical_record_path(run_root, job)
        if stale_record.exists():
            stale_record.unlink()
        frozen_before = (
            sha256_tree(run_dir / "corpus")
            if job.interface == FILESYSTEM_INTERFACE
            else None
        )
        prompt = (run_dir / "prompt.txt").read_bytes()
        service_broker = (
            ServiceBroker(
                job,
                metadata,
                run_id=run_id,
                service_api_url=os.environ["BRUNN_API_URL"],
            )
            if job.interface != FILESYSTEM_INTERFACE
            else None
        )
        if service_broker is not None:
            await service_broker.start()
        env = agent_environment(
            job,
            metadata,
            run_id=run_id,
            run_dir=run_dir,
            service_api_url=(
                service_broker.base_url
                if service_broker is not None
                else None
            ),
            service_token=(
                service_broker.capability
                if service_broker is not None
                else None
            ),
        )
        direct_openai = False
        model_gateway = LocalModelGateway(
            direct_openai=direct_openai,
            expected_model=model,
            expected_reasoning_effort="xhigh",
        )
        await model_gateway.start()
        env["BRUNN_STATE_MODEL_GATEWAY_TOKEN"] = model_gateway.capability
        inspector = (
            TrustedInspectionServer(job, run_dir)
            if job.eval_case.binary_asset_paths
            else None
        )
        if inspector is not None:
            await inspector.start()
            env["BRUNN_STATE_INSPECT_URL"] = str(inspector.url)
            env["BRUNN_STATE_INSPECT_CAPABILITY"] = inspector.capability
        allowed_urls = [
            str(model_gateway.base_url),
            *(
                [str(service_broker.base_url)]
                if service_broker is not None
                else []
            ),
            *([str(inspector.url)] if inspector is not None else []),
        ]
        checkpoint_verification: dict[str, Any] | None = None
        containment: dict[str, Any] | None = None
        try:
            profile = write_evidence_sandbox_profile(
                run_dir,
                allowed_urls=allowed_urls,
            )
            containment = await asyncio.to_thread(
                verify_evidence_sandbox_profile,
                profile,
                run_dir,
                protected_paths=(
                    run_root / RUN_MANIFEST,
                    ensure_parent_custody(run_root) / PROVISIONING_STATE,
                    ensure_parent_custody(run_root) / RECORD_KEY,
                ),
            )
            if job.agent == "codex":
                command = codex_command(
                    job,
                    run_dir,
                    codex=codex,
                    model=model,
                    model_base_url=model_gateway.base_url,
                    direct_openai=direct_openai,
                )
                command = sandboxed_command(
                    command,
                    run_dir,
                    allowed_urls=allowed_urls,
                    profile=profile,
                )
                exit_code, timed_out, elapsed, stdout, stderr = await communicate(
                    command,
                    cwd=run_dir,
                    env=env,
                    stdin=prompt,
                    timeout_seconds=timeout_seconds,
                )
                atomic_write_bytes(
                    run_dir / "events.jsonl",
                    stdout,
                    mode=0o600,
                )
                events = parse_event_metrics(run_dir / "events.jsonl")
            else:
                state_dir, config_path = prepare_openclaw(
                    job,
                    metadata,
                    run_dir,
                    run_id,
                    model_gateway.base_url,
                    model_gateway.capability,
                    model=model,
                    service_api_url=(
                        service_broker.base_url
                        if service_broker is not None
                        else None
                    ),
                    service_token=(
                        service_broker.capability
                        if service_broker is not None
                        else None
                    ),
                )
                env.update({
                    "OPENCLAW_STATE_DIR": str(state_dir),
                    "OPENCLAW_CONFIG_PATH": str(config_path),
                })
                session_id = hashlib.sha256(
                    f"{run_id}:{job.key}".encode()
                ).hexdigest()[:32]
                command = openclaw_command(
                    run_dir,
                    session_id=session_id,
                    model=model,
                    provider="evaluation_gateway",
                    timeout_seconds=timeout_seconds,
                )
                command = sandboxed_command(
                    command,
                    run_dir,
                    allowed_urls=allowed_urls,
                    profile=profile,
                )
                exit_code, timed_out, elapsed, stdout, stderr = await communicate(
                    command,
                    cwd=run_dir,
                    env=env,
                    stdin=None,
                    timeout_seconds=timeout_seconds + 30,
                )
                atomic_write_bytes(
                    run_dir / "openclaw-output.json",
                    stdout,
                    mode=0o600,
                )
                result: dict[str, Any] | None = None
                try:
                    result = parse_json_stdout(stdout)
                    events = openclaw_metrics(result)
                    answer_text = openclaw_answer(result)
                    atomic_write_text(
                        run_dir / "answer.json",
                        answer_text.rstrip() + "\n",
                        mode=0o600,
                    )
                except ValueError as exc:
                    events = (
                        openclaw_metrics(result)
                        if result is not None
                        else {
                            "events": 0,
                            "commands": 0,
                            "command_output_chars": 0,
                            "tokens": {},
                        }
                    )
                    stderr += f"\n{exc}".encode()
            if service_broker is not None and service_broker.checkpoint is not None:
                try:
                    checkpoint_verification = (
                        await service_broker.verify_checkpoint()
                    )
                except Exception as exc:
                    checkpoint_verification = {
                        "persisted": False,
                        "error": str(exc),
                    }
        finally:
            if inspector is not None:
                await inspector.close()
            if service_broker is not None:
                await service_broker.close()
            await model_gateway.close()

        gateway_contract = model_gateway_contract(model_gateway.requests)
        if gateway_tokens := trusted_model_usage(model_gateway.requests):
            events["runtime_reported_tokens"] = events.get("tokens", {})
            events["tokens"] = gateway_tokens
            events["token_provenance"] = "parent_model_gateway_response_usage"
        else:
            events["token_provenance"] = (
                "runtime_reported_unverified_model_gateway_incomplete"
            )
        events["token_accounting_complete"] = gateway_contract[
            "token_accounting_complete"
        ]
        if service_broker is not None:
            write_trusted_service_state(run_dir, service_broker)
        atomic_write_bytes(run_dir / "stderr.log", stderr, mode=0o600)
        frozen_after = (
            sha256_tree(run_dir / "corpus")
            if job.interface == FILESYSTEM_INTERFACE
            else None
        )
        record = {
            "suite": job.eval_case.suite,
            "qualified_case_id": job.eval_case.qualified_id,
            "case_id": job.eval_case.case["id"],
            "workload": job.eval_case.case["workload"],
            "capability": job.eval_case.case.get("capability"),
            "agent": job.agent,
            "interface": job.interface,
            "repetition": job.repetition,
            "cell": f"{job.agent}/{job.interface}",
            "transition": job.eval_case.transition,
            "expected_claims": len(job.eval_case.case["rubric"]),
            "exit_code": exit_code,
            "timed_out": timed_out,
            "elapsed_seconds": round(elapsed, 3),
            "prompt_chars": len(prompt),
            "answer_path": str(run_dir / "answer.json"),
            "error": (
                stderr.decode("utf-8", errors="replace")[-4000:]
                if exit_code != 0 or timed_out
                else None
            ),
            "events": events,
            "trusted_asset_inspections": (
                list(inspector.records) if inspector is not None else []
            ),
            "trusted_service_operations": (
                list(service_broker.records)
                if service_broker is not None
                else []
            ),
            "trusted_service_session_id": (
                service_broker.session_id
                if service_broker is not None
                else None
            ),
            "trusted_service_corpus_revision": (
                service_broker.corpus_revision
                if service_broker is not None
                else None
            ),
            "trusted_service_checkpoint": (
                service_broker.checkpoint
                if service_broker is not None
                else None
            ),
            "trusted_checkpoint_verification": checkpoint_verification,
            "trusted_model_gateway_requests": list(model_gateway.requests),
            "trusted_model_gateway_contract": gateway_contract,
            "token_accounting_complete": gateway_contract[
                "token_accounting_complete"
            ],
            "containment": containment,
            "credential_scope_complete": (
                True
                if job.interface == FILESYSTEM_INTERFACE
                else provisioning_matches_run_case(
                    metadata,
                    run_id=run_id,
                    case_id=job.key,
                    require_checkpoint=job.eval_case.transition,
                )
            ),
            "parent_evidence_custody": "hmac_canonical_record_v2",
            "frozen_corpus_sha256_before": frozen_before,
            "frozen_corpus_sha256_after": frozen_after,
            "frozen_corpus_unchanged": frozen_before == frozen_after,
        }
        try:
            finalized = finalize_record(
                record,
                job,
                metadata,
                run_id=run_id,
                run_dir=run_dir,
            )
        finally:
            if job.agent == "openclaw":
                cleanup_openclaw_state(run_dir)
            cleanup_agent_state(run_dir)
        write_record_seal(
            run_root,
            job,
            run_id=run_id,
            record_path=run_dir / "record.json",
        )
        seal_run_dir(run_dir)
        return finalized


def load_existing_record(
    job: Job,
    metadata: dict[str, Any],
    *,
    run_id: str,
    run_root: Path,
    run_dir: Path,
) -> dict[str, Any]:
    record_path = canonical_record_path(run_root, job)
    record_bytes = read_regular_file_bytes(record_path)
    record = json.loads(record_bytes)
    if not isinstance(record, dict):
        raise ValueError(f"existing canonical record is invalid: {record_path}")
    if record.get("run_id") != run_id or record.get("job_key") != job.key:
        raise ValueError(f"existing record identity mismatch: {record_path}")
    seal_path = record_seal_path(run_root, job)
    seal = json.loads(read_regular_file_bytes(seal_path))
    if not isinstance(seal, dict):
        raise ValueError(f"existing record seal is invalid: {seal_path}")
    expected_sha256 = f"sha256:{hashlib.sha256(record_bytes).hexdigest()}"
    expected_hmac = record_hmac(
        record_hmac_key(run_root),
        run_id=run_id,
        job_key=job.key,
        record_bytes=record_bytes,
    )
    if (
        seal.get("format") != "brunn-state-parent-record-seal-v2"
        or seal.get("run_id") != run_id
        or seal.get("job_key") != job.key
        or seal.get("record_sha256") != expected_sha256
        or not hmac.compare_digest(
            str(seal.get("record_hmac") or ""),
            expected_hmac,
        )
    ):
        raise ValueError(f"existing record seal mismatch: {record_path}")
    return record


def token_value(record: dict[str, Any], key: str) -> int:
    if record.get("token_accounting_complete") is False:
        return 0
    tokens = record.get("events", {}).get("tokens", {})
    if key == "uncached_input_tokens":
        if isinstance(tokens.get(key), int):
            return tokens[key]
        return max(
            0,
            int(tokens.get("input_tokens") or 0)
            - int(tokens.get("cached_input_tokens") or 0),
        )
    return int(tokens.get(key) or 0)


def mean(values: Iterable[float | int]) -> float:
    materialized = list(values)
    return statistics.fmean(materialized) if materialized else 0.0


def percentile(values: Iterable[float | int], fraction: float) -> float:
    materialized = sorted(values)
    if not materialized:
        return 0.0
    index = max(0, min(len(materialized) - 1, int(
        math.ceil(fraction * len(materialized)) - 1
    )))
    return float(materialized[index])


def mean_confidence_interval_95(
    values: Sequence[float | int],
) -> list[float] | None:
    if len(values) < 2:
        return None
    center = statistics.fmean(values)
    margin = 1.96 * statistics.stdev(values) / math.sqrt(len(values))
    return [round(center - margin, 4), round(center + margin, 4)]


def token_cost_equivalent(
    token_totals: dict[str, int],
    pricing: dict[str, Any],
) -> dict[str, float]:
    rates = pricing["short_context_per_million"]
    uncached = token_totals["uncached_input_tokens"]
    cached = token_totals["cached_input_tokens"]
    cache_write = token_totals["cache_write_input_tokens"]
    output = token_totals["output_tokens"]
    standard = (
        uncached * rates["input"]
        + cached * rates["cached_input"]
        + cache_write * rates["cache_write"]
        + output * rates["output"]
    ) / 1_000_000
    cache_write_bound = (
        (uncached + cache_write) * rates["cache_write"]
        + cached * rates["cached_input"]
        + output * rates["output"]
    ) / 1_000_000
    return {
        "standard_input_usd": round(standard, 4),
        "all_uncached_as_cache_write_usd": round(cache_write_bound, 4),
    }


def group_summary(rows: Sequence[dict[str, Any]]) -> dict[str, Any]:
    evaluated_rows = [row for row in rows if row.get("grade")]
    expected_claims = sum(int(row.get("expected_claims") or 0) for row in rows)
    evaluated_claims = sum(
        int(row.get("expected_claims") or 0)
        for row in evaluated_rows
    )
    passed_claims = sum(
        int((row.get("grade") or {}).get("claims_passed") or 0)
        for row in rows
    )
    token_keys = (
        "input_tokens",
        "uncached_input_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "output_tokens",
    )
    token_totals = {
        key: sum(token_value(row, key) for row in rows)
        for key in token_keys
    }
    token_means = {
        key: round(mean(token_value(row, key) for row in rows), 1)
        for key in token_keys
    }
    token_medians = {
        key: round(statistics.median(
            token_value(row, key) for row in rows
        ), 1) if rows else 0.0
        for key in token_keys
    }
    token_p95 = {
        key: round(percentile(
            (token_value(row, key) for row in rows),
            0.95,
        ), 1)
        for key in token_keys
    }
    service_calls = [int(row.get("service_calls") or 0) for row in rows]
    service_http_calls = [
        int(row.get("service_http_calls") or 0) for row in rows
    ]
    service_binary_bytes = [
        int(row.get("service_binary_bytes") or 0) for row in rows
    ]
    agent_tool_calls = [
        int(row.get("events", {}).get("commands") or 0)
        for row in rows
    ]
    elapsed_seconds = [
        float(row.get("elapsed_seconds") or 0)
        for row in rows
    ]
    return {
        "runs": len(rows),
        "successful_processes": sum(
            row.get("exit_code") == 0 and not row.get("timed_out")
            for row in rows
        ),
        "process_failures": sum(
            row.get("exit_code") != 0 or bool(row.get("timed_out"))
            for row in rows
        ),
        "valid_answers": len(evaluated_rows),
        "answer_cases_passed": sum(bool(row.get("answer_pass")) for row in rows),
        "workflow_cases_passed": sum(
            bool(row.get("workflow_pass")) for row in rows
        ),
        "evaluated_answer_case_pass_rate": round(
            mean(bool(row.get("answer_pass")) for row in evaluated_rows),
            4,
        ),
        "evaluated_workflow_case_pass_rate": round(
            mean(bool(row.get("workflow_pass")) for row in evaluated_rows),
            4,
        ),
        "answer_case_pass_rate": round(
            mean(bool(row.get("answer_pass")) for row in rows),
            4,
        ),
        "workflow_case_pass_rate": round(
            mean(bool(row.get("workflow_pass")) for row in rows),
            4,
        ),
        "claims_passed": passed_claims,
        "claims_total": expected_claims,
        "evaluated_claims_total": evaluated_claims,
        "evaluated_claim_pass_rate": round(
            passed_claims / max(1, evaluated_claims),
            4,
        ),
        "claim_pass_rate": round(
            passed_claims / max(1, expected_claims),
            4,
        ),
        "mean_score": round(
            mean(float((row.get("grade") or {}).get("score") or 0) for row in rows),
            4,
        ),
        "checkpoints_required": sum(
            (
                bool(row.get("checkpoint_required"))
                if "checkpoint_required" in row
                else bool(row.get("transition"))
            )
            for row in rows
        ),
        "checkpoints_persisted": sum(
            bool(row.get("checkpoint_persisted"))
            or bool((row.get("lineage") or {}).get("child_checkpoint"))
            for row in rows
        ),
        "within_four_calls": sum(bool(row.get("within_four_calls")) for row in rows),
        "within_call_budget": sum(
            bool(row.get("within_call_budget")) for row in rows
        ),
        "mean_service_calls": round(
            mean(service_calls),
            2,
        ),
        "median_service_calls": round(
            statistics.median(service_calls) if service_calls else 0,
            2,
        ),
        "p95_service_calls": round(percentile(service_calls, 0.95), 2),
        "mean_service_http_calls": round(mean(service_http_calls), 2),
        "total_service_binary_bytes": sum(service_binary_bytes),
        "mean_service_binary_bytes": round(mean(service_binary_bytes), 1),
        "mean_agent_tool_calls": round(mean(agent_tool_calls), 2),
        "median_agent_tool_calls": round(
            statistics.median(agent_tool_calls) if agent_tool_calls else 0,
            2,
        ),
        "p95_agent_tool_calls": round(
            percentile(agent_tool_calls, 0.95),
            2,
        ),
        "mean_service_result_chars": round(
            mean(int(row.get("service_result_chars") or 0) for row in rows),
            1,
        ),
        "mean_service_source_text_chars": round(
            mean(int(row.get("service_source_text_chars") or 0) for row in rows),
            1,
        ),
        "mean_service_metadata_chars": round(
            mean(int(row.get("service_metadata_chars") or 0) for row in rows),
            1,
        ),
        "mean_service_latency_ms": round(
            mean(float(row.get("service_latency_ms") or 0) for row in rows),
            3,
        ),
        "mean_elapsed_seconds": round(
            mean(elapsed_seconds),
            2,
        ),
        "median_elapsed_seconds": round(
            statistics.median(elapsed_seconds) if elapsed_seconds else 0,
            2,
        ),
        "p95_elapsed_seconds": round(
            percentile(elapsed_seconds, 0.95),
            2,
        ),
        "mean_prompt_chars": round(
            mean(int(row.get("prompt_chars") or 0) for row in rows),
            1,
        ),
        "mean_system_prompt_chars": round(
            mean(
                int(row.get("events", {}).get("system_prompt_chars") or 0)
                for row in rows
            ),
            1,
        ),
        "tokens_total": token_totals,
        "tokens_mean": token_means,
        "tokens_median": token_medians,
        "tokens_p95": token_p95,
    }


def add_pricing_to_summary(
    summary: dict[str, Any],
    pricing: dict[str, Any],
) -> None:
    for group_name in ("cells", "by_agent", "by_interface"):
        for result in summary[group_name].values():
            result["api_equivalent_cost"] = token_cost_equivalent(
                result["tokens_total"],
                pricing,
            )
    for suite in summary["by_suite"].values():
        for result in suite.values():
            result["api_equivalent_cost"] = token_cost_equivalent(
                result["tokens_total"],
                pricing,
            )


def paired_comparisons(
    records: Sequence[dict[str, Any]],
) -> dict[str, dict[str, Any]]:
    comparisons: dict[str, dict[str, Any]] = {}
    pairs = tuple(itertools.combinations(INTERFACE_CHOICES, 2))
    for agent in AGENTS:
        for left, right in pairs:
            left_rows = {
                (
                    row["qualified_case_id"],
                    int(row.get("repetition") or 1),
                ): row
                for row in records
                if row["agent"] == agent and row["interface"] == left
            }
            right_rows = {
                (
                    row["qualified_case_id"],
                    int(row.get("repetition") or 1),
                ): row
                for row in records
                if row["agent"] == agent and row["interface"] == right
            }
            ids = sorted(left_rows.keys() & right_rows.keys())
            if not ids:
                continue
            score_differences = [
                float((right_rows[case_id].get("grade") or {}).get("score") or 0)
                - float((left_rows[case_id].get("grade") or {}).get("score") or 0)
                for case_id in ids
            ]
            token_differences = {
                key: [
                    token_value(right_rows[case_id], key)
                    - token_value(left_rows[case_id], key)
                    for case_id in ids
                ]
                for key in (
                    "input_tokens",
                    "uncached_input_tokens",
                    "cached_input_tokens",
                    "cache_write_input_tokens",
                    "output_tokens",
                )
            }
            comparisons[f"{agent}:{right}-minus-{left}"] = {
                "paired_cases": len(ids),
                "workflow_wins": sum(
                    bool(right_rows[case_id].get("workflow_pass"))
                    and not bool(left_rows[case_id].get("workflow_pass"))
                    for case_id in ids
                ),
                "workflow_losses": sum(
                    bool(left_rows[case_id].get("workflow_pass"))
                    and not bool(right_rows[case_id].get("workflow_pass"))
                    for case_id in ids
                ),
                "workflow_ties": sum(
                    bool(left_rows[case_id].get("workflow_pass"))
                    == bool(right_rows[case_id].get("workflow_pass"))
                    for case_id in ids
                ),
                "mean_score_delta": round(mean(score_differences), 4),
                "mean_score_delta_ci95": mean_confidence_interval_95(
                    score_differences
                ),
                "mean_token_delta": {
                    key: round(mean(values), 1)
                    for key, values in token_differences.items()
                },
                "mean_token_delta_ci95": {
                    key: mean_confidence_interval_95(values)
                    for key, values in token_differences.items()
                },
                "discordant_cases": [
                    {
                        "qualified_case_id": pair_id[0],
                        "repetition": pair_id[1],
                    }
                    for pair_id in ids
                    if bool(left_rows[pair_id].get("workflow_pass"))
                    != bool(right_rows[pair_id].get("workflow_pass"))
                ],
            }
    return comparisons


def summarize(records: Sequence[dict[str, Any]]) -> dict[str, Any]:
    cells = {
        f"{agent}/{interface}": group_summary([
            row
            for row in records
            if row["agent"] == agent and row["interface"] == interface
        ])
        for agent in AGENTS
        for interface in INTERFACE_CHOICES
        if any(
            row["agent"] == agent and row["interface"] == interface
            for row in records
        )
    }
    by_suite = {
        suite: {
            cell: group_summary([
                row
                for row in records
                if row["suite"] == suite
                and row["cell"] == cell
            ])
            for cell in cells
        }
        for suite in SUITE_MANIFESTS
        if any(row["suite"] == suite for row in records)
    }
    by_agent = {
        agent: group_summary([
            row for row in records if row["agent"] == agent
        ])
        for agent in AGENTS
        if any(row["agent"] == agent for row in records)
    }
    by_interface = {
        interface: group_summary([
            row for row in records if row["interface"] == interface
        ])
        for interface in INTERFACE_CHOICES
        if any(row["interface"] == interface for row in records)
    }
    return {
        "cells": cells,
        "by_suite": by_suite,
        "by_agent": by_agent,
        "by_interface": by_interface,
        "paired_comparisons": paired_comparisons(records),
    }


def quality_publishability(
    run: dict[str, Any],
    *,
    all_case_count: int,
    process_failures: int,
) -> dict[str, Any]:
    protocol = run["protocol"]
    records = run["records"]
    agents = set(protocol["agents"])
    interfaces = set(protocol["interfaces"])
    suites = set(protocol["suites"])
    repetitions = int(protocol["repetitions"])
    expected_pairs = protocol["case_count"] * repetitions
    comparisons = run["summary"]["paired_comparisons"]
    comparison_checks: dict[str, Any] = {}
    for agent in agents:
        for interface in interfaces - {FILESYSTEM_INTERFACE}:
            label = f"{agent}:{FILESYSTEM_INTERFACE}-minus-{interface}"
            comparison = comparisons.get(label)
            comparison_checks[label] = {
                "paired_runs": (
                    comparison.get("paired_cases") if comparison else 0
                ),
                "expected_paired_runs": expected_pairs,
                "complete": bool(
                    comparison
                    and comparison.get("paired_cases") == expected_pairs
                ),
                "screening_noninferiority_margin": 0.02,
                "screening_noninferior": bool(
                    comparison
                    and comparison.get("mean_score_delta_ci95")
                    and comparison["mean_score_delta_ci95"][1] <= 0.02
                ),
            }
    full_profile = bool(
        protocol["case_count"] == all_case_count
        and agents == set(AGENTS)
        and interfaces == set(INTERFACES)
        and suites == set(SUITE_MANIFESTS)
    )
    trusted_receipts = all(
        record["interface"] == FILESYSTEM_INTERFACE
        or (
            bool(record.get("service_operations"))
            and all(
                operation.get("trusted_broker_receipt") is True
                for operation in record.get("service_operations", [])
            )
        )
        for record in records
    )
    parent_custody = all(
        record.get("parent_evidence_custody") == "hmac_canonical_record_v2"
        for record in records
    )
    containment_complete = all(
        (record.get("containment") or {}).get("pass") is True
        and (record.get("containment") or {}).get("publishable_containment") is True
        for record in records
    )
    model_contracts_complete = all(
        (record.get("trusted_model_gateway_contract") or {}).get("complete") is True
        for record in records
    )
    token_accounting_complete = all(
        record.get("token_accounting_complete") is True
        for record in records
    )
    credential_scopes_complete = all(
        record.get("credential_scope_complete") is True
        for record in records
    )
    binary_semantics_complete = all(
        "binary_access" not in record
        or record.get("binary_semantic_evidence_complete") is True
        for record in records
    )
    blockers = []
    if len(records) != protocol["fresh_agent_runs"]:
        blockers.append("not every predeclared run has a record")
    if not run["integrity"]["pass"]:
        blockers.append("run integrity checks failed")
    if process_failures:
        blockers.append("one or more agent processes failed")
    if not full_profile:
        blockers.append("run is not the full predeclared matrix")
    if repetitions < 5:
        blockers.append("fewer than five randomized repetitions were run")
    if not comparison_checks or not all(
        item["complete"] for item in comparison_checks.values()
    ):
        blockers.append("filesystem and Brunn State matched pairs are incomplete")
    if not trusted_receipts:
        blockers.append("one or more service operations lacks a parent receipt")
    if not parent_custody:
        blockers.append("one or more records lacks keyed parent evidence custody")
    if not containment_complete:
        blockers.append(
            "same-user Seatbelt containment is screening-only; publishable "
            "containment requires a separate OS identity or VM"
        )
    if not model_contracts_complete:
        blockers.append("one or more model request/response contracts is incomplete")
    if not token_accounting_complete:
        blockers.append("parent-owned token accounting is incomplete")
    if not credential_scopes_complete:
        blockers.append("one or more service credentials lacks exact scope proof")
    if not binary_semantics_complete:
        blockers.append(
            "one or more native-file claims lacks parent-verifiable semantic evidence"
        )
    blockers.append(
        "deterministic keyword rubrics are screening evidence; blind semantic "
        "or human adjudication is required for a quality claim"
    )
    return {
        "all_attempts_recorded": len(records) == protocol["fresh_agent_runs"],
        "integrity_pass": bool(run["integrity"]["pass"]),
        "stochastic_confidence_available": repetitions >= 5,
        "full_predeclared_profile": full_profile,
        "trusted_parent_receipts": trusted_receipts,
        "keyed_parent_evidence_custody": parent_custody,
        "containment_complete": containment_complete,
        "model_contracts_complete": model_contracts_complete,
        "token_accounting_complete": token_accounting_complete,
        "credential_scopes_complete": credential_scopes_complete,
        "binary_semantics_complete": binary_semantics_complete,
        "cross_agent_quality_comparable": False,
        "process_failures": process_failures,
        "quality_grader": "deterministic_screening_only",
        "comparison_checks": comparison_checks,
        "blockers": blockers,
        "publishable_for_quality_comparison": not blockers,
    }


def compact_public_provisioning(metadata: dict[str, Any]) -> dict[str, Any]:
    public = public_provisioning(metadata)
    provisioning = public.get("provisioning", {})
    return {
        key: public.get(key)
        for key in (
            "authorization_scope",
            "display_scope",
            "access_mode",
            "import_id",
            "checkpoint_id",
            "base_corpus_revision",
            "corpus_revision",
        )
    } | {
        "provisioning": {
            key: provisioning.get(key)
            for key in (
                "documents",
                "delta_documents",
                "characters",
                "import_http_status",
                "import_elapsed_ms",
                "ready_elapsed_ms",
            )
        },
    }


def render_report(run: dict[str, Any]) -> str:
    pricing = run.get("pricing") or {}
    pricing_complete = bool(pricing) and pricing.get("complete", True) is not False
    lines = [
        "# Brunn State interface evaluation",
        "",
        f"- Run: `{run['run_id']}`",
        f"- Model: `{run['protocol']['model']}` at `xhigh` reasoning",
        f"- Fresh-agent runs: {len(run['records'])}",
        f"- Cases per complete cell: {run['protocol']['case_count']}",
        f"- Claims per complete cell: {run['protocol']['claim_count']}",
        f"- Repetitions: {run['protocol']['repetitions']}",
        f"- Randomization seed: `{run['protocol']['randomization_seed']}`",
        (
            "- Quality-comparison status: "
            + (
                "**publishable**"
                if run["evaluation_status"]["publishable_for_quality_comparison"]
                else "**not publishable**"
            )
        ),
        *[
            f"- Quality blocker: {blocker}"
            for blocker in run["evaluation_status"].get("blockers", [])
        ],
        "",
        "## Overall matrix",
        "",
        "| Agent | Interface | Workflow pass | Claims | Valid answers | Process failures | Mean score | Mean input | Mean uncached | Mean output | Mean service calls | Mean seconds |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for cell, result in run["summary"]["cells"].items():
        agent, interface = cell.split("/", 1)
        tokens = result["tokens_mean"]
        lines.append(
            f"| {agent} | {interface} | "
            f"{result['workflow_cases_passed']}/{result['runs']} | "
            f"{result['claims_passed']}/{result['claims_total']} | "
            f"{result['valid_answers']} | "
            f"{result['process_failures']} | "
            f"{result['mean_score']:.4f} | "
            f"{tokens['input_tokens']:.1f} | "
            f"{tokens['uncached_input_tokens']:.1f} | "
            f"{tokens['output_tokens']:.1f} | "
            f"{result['mean_service_calls']:.2f} | "
            f"{result['mean_elapsed_seconds']:.2f} |"
        )

    if pricing_complete:
        lines.extend([
            "",
            "## Distribution and API-equivalent cost",
            "",
            "| Agent | Interface | Input median / p95 | Uncached median / p95 | Seconds median / p95 | Calls median / p95 | Estimated cost per run |",
            "|---|---|---:|---:|---:|---:|---:|",
        ])
        for cell, result in run["summary"]["cells"].items():
            agent, interface = cell.split("/", 1)
            median_tokens = result["tokens_median"]
            p95_tokens = result["tokens_p95"]
            cost = result["api_equivalent_cost"]
            runs = max(1, result["runs"])
            lines.append(
                f"| {agent} | {interface} | "
                f"{median_tokens['input_tokens']:.0f} / "
                f"{p95_tokens['input_tokens']:.0f} | "
                f"{median_tokens['uncached_input_tokens']:.0f} / "
                f"{p95_tokens['uncached_input_tokens']:.0f} | "
                f"{result['median_elapsed_seconds']:.1f} / "
                f"{result['p95_elapsed_seconds']:.1f} | "
                f"{result['median_service_calls']:.0f} / "
                f"{result['p95_service_calls']:.0f} | "
                f"${cost['standard_input_usd'] / runs:.3f} to "
                f"${cost['all_uncached_as_cache_write_usd'] / runs:.3f} |"
            )

    lines.extend([
        "",
        "## Paired differences",
        "",
        "Token deltas are right interface minus left interface on the same task and agent.",
        "",
        "| Comparison | Paired runs | Wins | Losses | Score delta (95% CI) | Input delta | Uncached delta | Output delta |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ])
    for label, result in run["summary"]["paired_comparisons"].items():
        deltas = result["mean_token_delta"]
        score_ci = result.get("mean_score_delta_ci95")
        score_rendered = (
            f"{result['mean_score_delta']:+.4f} "
            f"({score_ci[0]:+.4f}, {score_ci[1]:+.4f})"
            if score_ci
            else f"{result['mean_score_delta']:+.4f} (insufficient repeats)"
        )
        lines.append(
            f"| {label} | {result['paired_cases']} | "
            f"{result['workflow_wins']} | {result['workflow_losses']} | "
            f"{score_rendered} | "
            f"{deltas['input_tokens']:+.1f} | "
            f"{deltas['uncached_input_tokens']:+.1f} | "
            f"{deltas['output_tokens']:+.1f} |"
        )

    lines.extend([
        "",
        "## Suite detail",
        "",
        "| Suite | Cell | Workflow pass | Claims | Process failures | Mean input | Mean uncached |",
        "|---|---|---:|---:|---:|---:|---:|",
    ])
    for suite, cells in run["summary"]["by_suite"].items():
        for cell, result in cells.items():
            tokens = result["tokens_mean"]
            lines.append(
                f"| {suite} | {cell} | "
                f"{result['workflow_cases_passed']}/{result['runs']} | "
                f"{result['claims_passed']}/{result['claims_total']} | "
                f"{result['process_failures']} | "
                f"{tokens['input_tokens']:.1f} | "
                f"{tokens['uncached_input_tokens']:.1f} |"
            )
    lines.extend([
        "",
        "## Protocol",
        "",
        "- Within each agent, every interface received the same task, claim slots, evidence discipline, model ID, and reasoning effort. Codex and OpenClaw are not treated as interchangeable runtimes: their system prompts, tool plumbing, and structured-output enforcement differ.",
        "- Primary workflow and claim rates include every attempted run. Provider, authentication, timeout, malformed-output, and runner failures therefore count as misses; evaluated-only rates remain secondary diagnostics in the JSON.",
        (
            "- Every Brunn State run used a separate service user, credential, corpus, "
            "session history, and writable checkpoint history. Each filesystem control "
            "used a private read-only copy of the same frozen operational corpus."
            if FILESYSTEM_INTERFACE in run["protocol"]["interfaces"]
            else "- Every run used a separate Brunn State user, credential, corpus, "
            "session history, and writable checkpoint history."
        ),
        "- CLI retained session IDs and compacted service responses. MCP supplied typed tools and the same compact reasoning view. HTTP returned raw JSON and required the agent to manage identifiers and payloads. Filesystem controls used ordinary local search and reads without Brunn State.",
        "- The evaluator gives each agent an isolated home and temporary directory. Network access is restricted to exact parent-owned model, Brunn State, and native-inspection ports. The parent injects real credentials and records service, checkpoint, download, and inspection receipts; agent-written traces are diagnostic only.",
        "- Deterministic claim rubrics are a screening signal, not final quality adjudication. A result cannot be published as parity evidence without a complete matched matrix, at least five randomized repetitions, and blind semantic or human review.",
        (
            "- Brunn State workflow passes require a passing answer and the expected "
            "durable checkpoint. Brunn State transition cases additionally require exact "
            "parent, corpus revision, old-source, delta-source, and four-call lineage "
            "gates. Filesystem workflow passes grade the answer only."
        ),
        "- Input includes cached input. Uncached input is the newly billed or newly processed portion reported by the runtime; output includes visible and hidden reasoning tokens when the provider reports them together.",
        (
            "- The dollar range is an API-price equivalent, not a claim about "
            "the user's actual Codex or OpenClaw bill. Its low end charges "
            "reported uncached input at the standard rate; its high end "
            "conservatively treats all reported uncached input as cache writes."
            if pricing_complete
            else (
                "- API-equivalent cost is unavailable because parent-owned token "
                "receipts are incomplete."
                if pricing
                else "- No public model price was attached; raw token totals are preserved."
            )
        ),
        (
            f"- Pricing was checked {pricing['checked_at']} against "
            f"{pricing['source']}; every complete-run token total was "
            f"{'below' if pricing['all_record_totals_below_long_context_threshold'] else 'not guaranteed below'} "
            f"the {pricing['long_context_threshold_tokens']:,}-token "
            "long-context threshold."
            if pricing_complete
            else ""
        ),
        "",
        "## Reproduce",
        "",
        "```bash",
        "cd /Users/Shared/projects/brunn",
        "python3 interface_eval.py validate",
        f"python3 interface_eval.py run --resume-run-id {run['run_id']} "
        f"--repetitions {run['protocol']['repetitions']} "
        f"--randomization-seed {run['protocol']['randomization_seed']} "
        f"--out {run['output_path']} --report {run['report_path']}",
        "```",
        "",
    ])
    return "\n".join(lines)


def validate_environment() -> dict[str, Any]:
    cases, validation = load_cases()
    checks = {
        "codex": {
            "path": str(DEFAULT_CODEX),
            "ready": DEFAULT_CODEX.is_file() and os.access(DEFAULT_CODEX, os.X_OK),
        },
        "openclaw": {
            "path": str(OPENCLAW),
            "ready": OPENCLAW.is_file() and os.access(OPENCLAW, os.X_OK),
        },
        "openclaw_codex_runtime": {
            "path": str(OPENCLAW_NPM),
            "ready": OPENCLAW_NPM.is_dir(),
        },
        "mcp_server": {
            "path": str(MCP_SERVER),
            "ready": MCP_SERVER.is_file(),
        },
        "schema": {
            "path": str(SCHEMA),
            "ready": SCHEMA.is_file(),
        },
    }
    for name, check in checks.items():
        if not check["ready"]:
            validation["errors"].append(f"{name} is not ready: {check['path']}")
    validation["checks"] = checks
    validation["agents"] = list(AGENTS)
    validation["interfaces"] = list(INTERFACES)
    validation["runs_per_repetition"] = (
        len(cases) * len(AGENTS) * len(INTERFACES)
    )
    validation["default_repetitions"] = 5
    validation["expected_full_runs"] = validation["runs_per_repetition"] * 5
    return validation


async def run_all(args: argparse.Namespace) -> dict[str, Any]:
    require_codex_subscription(args.codex)
    all_cases, validation = load_cases()
    if validation["errors"]:
        raise ValueError("\n".join(validation["errors"]))
    cases = select_cases(all_cases, args.suite, args.case)
    run_id = (
        args.resume_run_id
        or args.run_id
        or datetime.now().astimezone().strftime("interface-%Y%m%dT%H%M%S%z")
    )
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,127}", run_id):
        raise ValueError("run ID must be a safe single path component")
    EXECUTION_ROOT.mkdir(parents=True, exist_ok=True, mode=0o700)
    EXECUTION_ROOT.chmod(0o700)
    run_root = EXECUTION_ROOT / run_id
    if args.resume_run_id:
        if not run_root.is_dir():
            raise ValueError(f"Unknown run directory: {run_root}")
        prior_manifest = load_json(run_root / RUN_MANIFEST)
        prior_selection = prior_manifest.get("selection", {})
        repetitions = (
            args.repetitions
            if args.repetitions is not None
            else int(prior_selection.get("repetitions") or 1)
        )
        randomization_seed = (
            args.randomization_seed
            if args.randomization_seed is not None
            else int(prior_selection.get("randomization_seed") or 0)
        )
    else:
        repetitions = args.repetitions if args.repetitions is not None else 5
        randomization_seed = (
            args.randomization_seed
            if args.randomization_seed is not None
            else secrets.randbits(63)
        )
    jobs = create_jobs(
        cases,
        args.agent,
        args.interface,
        repetitions=repetitions,
        randomization_seed=randomization_seed,
    )
    service_jobs = [
        job for job in jobs
        if job.interface != FILESYSTEM_INTERFACE
    ]
    if service_jobs and not os.environ.get("BRUNN_API_URL"):
        raise ValueError("BRUNN_API_URL is required for Brunn State interfaces")
    if service_jobs and not os.environ.get("BRUNN_EVAL_TOKEN"):
        raise ValueError("BRUNN_EVAL_TOKEN is required for Brunn State interfaces")
    direct_openai = False
    service_api_url = os.environ.get("BRUNN_API_URL", "")
    run_manifest = build_run_manifest(
        run_id,
        jobs,
        model=args.model,
        codex=args.codex,
        timeout_seconds=args.timeout,
        native_index_timeout=args.native_index_timeout,
        service_api_url=service_api_url,
        direct_openai=direct_openai,
        repetitions=repetitions,
        randomization_seed=randomization_seed,
    )
    if args.resume_run_id:
        assert_resume_compatible(run_root / RUN_MANIFEST, run_manifest)
    else:
        run_root.mkdir(parents=True, exist_ok=False)
        write_run_manifest(run_root / RUN_MANIFEST, run_manifest)
    custody = ensure_parent_custody(run_root)
    if args.resume_run_id and not (custody / RECORD_KEY).is_file():
        raise ValueError("Run cannot be resumed without its parent record HMAC key")
    record_hmac_key(run_root)

    if service_jobs:
        provisioning = await provision_jobs(
            service_jobs,
            run_id=run_id,
            run_root=run_root,
            concurrency=args.provision_concurrency,
            timeout_seconds=args.native_index_timeout,
        )
    else:
        provisioning = {}
        print("provision: filesystem control needs no service users", flush=True)
    metadata_by_job = {
        **provisioning,
        **{
            job.key: {
                "token": "",
                "authorization_scope": "filesystem",
                "display_scope": job.eval_case.case.get(
                    "scope",
                    job.eval_case.case["workload"],
                ),
                "access_mode": "read_only",
                "checkpoint_id": None,
            }
            for job in jobs
            if job.interface == FILESYSTEM_INTERFACE
        },
    }
    records: list[dict[str, Any]] = []
    pending: list[Job] = []
    for job in jobs:
        run_dir = job.run_dir(run_root)
        canonical_exists = canonical_record_path(run_root, job).exists()
        seal_exists = record_seal_path(run_root, job).exists()
        if canonical_exists or seal_exists:
            if not canonical_exists or not seal_exists:
                raise ValueError(
                    f"Incomplete parent-custodied record for {job.key}"
                )
            records.append(load_existing_record(
                job,
                metadata_by_job[job.key],
                run_id=run_id,
                run_root=run_root,
                run_dir=run_dir,
            ))
        else:
            pending.append(job)
    print(
        f"agents: {len(records)} existing, {len(pending)} pending",
        flush=True,
    )

    semaphore = asyncio.Semaphore(args.concurrency)

    async def execute(job: Job) -> tuple[Job, dict[str, Any]]:
        record = await run_job(
            semaphore,
            job,
            metadata_by_job[job.key],
            run_id=run_id,
            run_root=run_root,
            codex=args.codex,
            model=args.model,
            timeout_seconds=args.timeout,
        )
        return job, record

    completed = 0
    tasks = [asyncio.create_task(execute(job)) for job in pending]
    for future in asyncio.as_completed(tasks):
        job, record = await future
        records.append(record)
        completed += 1
        result = "pass" if record.get("workflow_pass") else "FAIL"
        print(
            f"agents: {completed}/{len(pending)} {result} {job.key} "
            f"tokens={token_value(record, 'input_tokens')}",
            flush=True,
        )

    records.sort(
        key=lambda row: (
            row["suite"],
            row["case_id"],
            int(row.get("repetition") or 1),
            row["agent"],
            row["interface"],
        )
    )
    run = {
        "experiment_version": "brunn-state-interface-v3-screening",
        "run_id": run_id,
        "run_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "output_path": str(args.out),
        "report_path": str(args.report) if args.report else None,
        "protocol": {
            "model": args.model,
            "reasoning_effort": "xhigh",
            "agents": list(dict.fromkeys(job.agent for job in jobs)),
            "interfaces": list(dict.fromkeys(job.interface for job in jobs)),
            "suites": list(dict.fromkeys(job.eval_case.suite for job in jobs)),
            "case_count": len(cases),
            "claim_count": sum(
                len(eval_case.case["rubric"]) for eval_case in cases
            ),
            "repetitions": repetitions,
            "randomization_seed": randomization_seed,
            "fresh_agent_runs": len(jobs),
            "isolation": (
                "one service user and credential per Brunn State cell; "
                "one frozen run directory per filesystem control"
                if any(
                    job.interface == FILESYSTEM_INTERFACE
                    for job in jobs
                )
                else "one service user and credential per agent-interface-case"
            ),
            "max_service_calls": {
                "ordinary": 4,
                "binary_asset": 8,
            },
            "runtime_comparability": (
                "The task and model ID are held constant. Codex and OpenClaw "
                "remain different agent runtimes with different system prompts, "
                "tool plumbing, and structured-output enforcement; interface "
                "comparisons are primary only within the same agent."
            ),
            "network_isolation": (
                "macOS Seatbelt screening boundary with denied user data, known "
                "same-user escape tools, and non-allowlisted network destinations; "
                "parent-owned gateways inject credentials and record receipts. "
                "A separate OS identity or VM remains required for publishable "
                "hostile-agent containment."
            ),
            "quality_grading": (
                "Deterministic rubric scores are screening-only and cannot by "
                "themselves support a public parity claim."
            ),
        },
        "validation": validation,
        "run_manifest": run_manifest,
        "hashes": {
            "run_manifest_sha256": canonical_sha256(run_manifest),
            "harness_sha256": sha256_file(Path(__file__)),
            "cli_sha256": sha256_file(ROOT / "native_memory.py"),
            "api_sha256": sha256_file(ROOT / "native_http.py"),
            "mcp_sha256": sha256_file(MCP_SERVER),
            "schema_sha256": sha256_file(SCHEMA),
            "manifests": {
                suite: sha256_file(path)
                for suite, path in SUITE_MANIFESTS.items()
            },
        },
        "provisioning": {
            key: compact_public_provisioning(metadata)
            for key, metadata in provisioning.items()
            if key in {job.key for job in service_jobs}
        },
        "records": records,
        "integrity": post_run_integrity(
            jobs,
            run_manifest,
            service_api_url=service_api_url,
        ),
    }
    run["summary"] = summarize(records)
    process_failures = sum(
        result["process_failures"]
        for result in run["summary"]["cells"].values()
    )
    run["evaluation_status"] = quality_publishability(
        run,
        all_case_count=len(all_cases),
        process_failures=process_failures,
    )
    pricing = MODEL_PRICING.get(args.model)
    if pricing and records and all(
        record.get("token_accounting_complete") is True
        for record in records
    ):
        max_record_input = max(
            (token_value(record, "input_tokens") for record in records),
            default=0,
        )
        run["pricing"] = {
            **pricing,
            "basis": "API-equivalent only; runtime billing may differ",
            "max_record_input_tokens": max_record_input,
            "all_record_totals_below_long_context_threshold": (
                max_record_input
                <= pricing["long_context_threshold_tokens"]
            ),
        }
        add_pricing_to_summary(run["summary"], pricing)
    elif pricing:
        run["pricing"] = {
            **pricing,
            "basis": "unavailable: parent-owned token receipts are incomplete",
            "complete": False,
        }
    return run


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Compare Brunn State CLI, MCP, raw HTTP, and a frozen operational "
            "filesystem corpus "
            "from Codex and OpenClaw"
        ),
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("validate")

    run = subparsers.add_parser("run")
    run.add_argument("--codex", type=Path, default=DEFAULT_CODEX)
    run.add_argument("--model", default="gpt-5.6-sol")
    run.add_argument("--concurrency", type=int, default=3)
    run.add_argument("--provision-concurrency", type=int, default=3)
    run.add_argument("--timeout", type=int, default=600)
    run.add_argument("--native-index-timeout", type=float, default=600.0)
    run.add_argument("--run-id")
    run.add_argument("--resume-run-id")
    run.add_argument(
        "--repetitions",
        type=int,
        help="Independent repetitions per agent/interface/case (default: 5)",
    )
    run.add_argument(
        "--randomization-seed",
        type=int,
        help="Deterministic job-order seed (generated and recorded by default)",
    )
    run.add_argument("--suite", action="append", choices=tuple(SUITE_MANIFESTS))
    run.add_argument("--agent", action="append", choices=AGENTS)
    run.add_argument(
        "--interface",
        action="append",
        choices=INTERFACE_CHOICES,
    )
    run.add_argument("--case", action="append")
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--report", type=Path)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    if args.command == "validate":
        result = validate_environment()
        print(json.dumps({
            "status": "ok" if not result["errors"] else "error",
            **result,
        }, indent=2))
        if result["errors"]:
            raise SystemExit(1)
        return

    run = asyncio.run(run_all(args))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(render_report(run), encoding="utf-8")
    print(json.dumps({
        "status": "ok",
        "run_id": run["run_id"],
        "out": str(args.out),
        "report": str(args.report) if args.report else None,
        "summary": run["summary"]["cells"],
    }, indent=2))


if __name__ == "__main__":
    main()

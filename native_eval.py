#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import os
import shlex
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


PROJECT_ROOT = Path(__file__).resolve().parent
TEXT_SUFFIXES = {
    ".md", ".txt", ".csv", ".tsv", ".sql", ".json", ".jsonl",
    ".py", ".rs", ".go", ".swift", ".ts", ".tsx", ".js", ".jsx",
    ".sh", ".toml", ".yaml", ".yml",
}
READY_VALUES = {"ready", "complete", "completed", "current"}


class NativeApiError(RuntimeError):
    def __init__(
        self,
        status: int,
        body: dict[str, Any],
        *,
        elapsed_ms: float = 0.0,
    ) -> None:
        message = body.get("message") or body.get("error") or f"HTTP {status}"
        super().__init__(str(message))
        self.status = status
        self.body = body
        self.elapsed_ms = elapsed_ms


@dataclass(frozen=True)
class NativeResponse:
    body: dict[str, Any]
    http_status: int
    elapsed_ms: float
    headers: dict[str, str]

    @property
    def data(self) -> dict[str, Any]:
        value = self.body.get("data", self.body)
        return value if isinstance(value, dict) else {"value": value}


class NativeApiClient:
    def __init__(
        self,
        base_url: str | None = None,
        token: str | None = None,
        *,
        run_id: str | None = None,
        case_id: str | None = None,
        timeout: float = 120.0,
    ) -> None:
        self.base_url = (base_url or os.environ.get("STRAYLIGHT_API_URL") or "").rstrip("/")
        self.token = token or os.environ.get("STRAYLIGHT_EVAL_TOKEN") or ""
        self.run_id = run_id
        self.case_id = case_id
        self.timeout = timeout
        if not self.base_url:
            raise ValueError("STRAYLIGHT_API_URL is required for native evaluation")
        if not self.token:
            raise ValueError("STRAYLIGHT_EVAL_TOKEN is required for native evaluation")

    def request(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
    ) -> NativeResponse:
        url = path if path.startswith("http://") or path.startswith("https://") else f"{self.base_url}/{path.lstrip('/')}"
        data = None
        headers = {
            "Accept": "application/json",
            "Authorization": f"Bearer {self.token}",
        }
        if payload is not None:
            data = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if self.run_id:
            headers["X-Straylight-Eval-Run"] = self.run_id
        if self.case_id:
            headers["X-Straylight-Eval-Case"] = self.case_id
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        started = time.monotonic()
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                raw = response.read()
                elapsed_ms = (time.monotonic() - started) * 1000
                body = json.loads(raw or b"{}")
                if not isinstance(body, dict):
                    body = {"data": body}
                return NativeResponse(
                    body=body,
                    http_status=response.status,
                    elapsed_ms=elapsed_ms,
                    headers={key.casefold(): value for key, value in response.headers.items()},
                )
        except urllib.error.HTTPError as exc:
            elapsed_ms = (time.monotonic() - started) * 1000
            try:
                raw = exc.read()
            finally:
                exc.close()
            try:
                body = json.loads(raw or b"{}")
            except json.JSONDecodeError:
                body = {"error": "http_error", "message": raw.decode("utf-8", errors="replace")}
            if not isinstance(body, dict):
                body = {"error": "http_error", "data": body}
            raise NativeApiError(exc.code, body, elapsed_ms=elapsed_ms) from exc
        except urllib.error.URLError as exc:
            elapsed_ms = (time.monotonic() - started) * 1000
            raise NativeApiError(
                503,
                {"error": "dependency_unavailable", "message": str(exc.reason)},
                elapsed_ms=elapsed_ms,
            ) from exc

    def post(self, path: str, payload: dict[str, Any]) -> NativeResponse:
        return self.request("POST", path, payload)

    def get(self, path: str) -> NativeResponse:
        return self.request("GET", path)

    def get_session(self, session_id: str) -> NativeResponse:
        return self.get(f"/v1/sessions/{urllib.parse.quote(session_id, safe=':_-')}")

    def get_checkpoint(self, checkpoint_id: str, session_id: str | None = None) -> NativeResponse:
        if session_id:
            session = self.get_session(session_id)
            checkpoint = find_checkpoint(session.body, checkpoint_id)
            if checkpoint is not None:
                return NativeResponse(
                    body={"status": "complete", "data": checkpoint},
                    http_status=session.http_status,
                    elapsed_ms=session.elapsed_ms,
                    headers=session.headers,
                )
        return self.get(f"/v1/checkpoints/{urllib.parse.quote(checkpoint_id, safe=':_-')}")


def find_checkpoint(value: Any, checkpoint_id: str) -> dict[str, Any] | None:
    if isinstance(value, dict):
        if value.get("checkpoint_id") == checkpoint_id:
            return value
        for nested in value.values():
            match = find_checkpoint(nested, checkpoint_id)
            if match is not None:
                return match
    elif isinstance(value, list):
        for nested in value:
            match = find_checkpoint(nested, checkpoint_id)
            if match is not None:
                return match
    return None


def text_documents(root: Path) -> list[dict[str, Any]]:
    documents = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix.casefold() not in TEXT_SUFFIXES:
            continue
        content = path.read_text(encoding="utf-8", errors="replace")
        documents.append({
            "path": path.relative_to(root).as_posix(),
            "content": content,
            "content_sha256": hashlib.sha256(content.encode("utf-8")).hexdigest(),
            "media_type": media_type(path),
        })
    return documents


def file_document(path: Path, logical_path: str) -> dict[str, Any]:
    content = path.read_text(encoding="utf-8", errors="replace")
    return {
        "path": logical_path,
        "content": content,
        "content_sha256": hashlib.sha256(content.encode("utf-8")).hexdigest(),
        "media_type": media_type(path),
    }


def media_type(path: Path) -> str:
    return {
        ".md": "text/markdown",
        ".json": "application/json",
        ".jsonl": "application/x-ndjson",
        ".csv": "text/csv",
        ".tsv": "text/tab-separated-values",
    }.get(path.suffix.casefold(), "text/plain")


def stable_scope(run_id: str, case_id: str) -> str:
    def clean(value: str) -> str:
        cleaned = "".join(character if character.isalnum() or character in "._-" else "-" for character in value)
        return cleaned.strip("-") or "run"

    return f"eval:{clean(run_id)}/{clean(case_id)}"


def stable_import_key(
    run_id: str,
    case_id: str,
    documents: Iterable[dict[str, Any]],
    delta_documents: Iterable[dict[str, Any]],
) -> str:
    digest = hashlib.sha256()
    digest.update(run_id.encode())
    digest.update(b"\0")
    digest.update(case_id.encode())
    for group in (documents, delta_documents):
        digest.update(b"\0")
        for document in group:
            digest.update(document["path"].encode())
            digest.update(b"\0")
            digest.update(document["content_sha256"].encode())
    return f"eval-import:{digest.hexdigest()}"


def recursively_redact_secrets(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            key: recursively_redact_secrets(item)
            for key, item in value.items()
            if key.casefold() not in {"token", "credential_token", "api_token", "secret"}
        }
    if isinstance(value, list):
        return [recursively_redact_secrets(item) for item in value]
    return value


def response_field(response: NativeResponse, *names: str) -> Any:
    for source in (response.data, response.body):
        for name in names:
            value = source.get(name)
            if value is not None:
                return value
    return None


def native_error_code(error: NativeApiError) -> str | None:
    value = error.body.get("error")
    if isinstance(value, dict):
        code = value.get("code")
        return str(code) if code is not None else None
    if isinstance(value, str):
        return value
    code = error.body.get("code")
    return str(code) if code is not None else None


def post_import_with_retry(
    client: NativeApiClient,
    payload: dict[str, Any],
    *,
    timeout_seconds: float,
    initial_delay_seconds: float,
) -> NativeResponse:
    deadline = time.monotonic() + timeout_seconds
    delay = max(0.01, initial_delay_seconds)
    while True:
        try:
            return client.post("/v1/admin/eval/import", payload)
        except NativeApiError as error:
            retryable = error.status in {429, 503} and native_error_code(error) == "dependency_unavailable"
            remaining = deadline - time.monotonic()
            if not retryable or remaining <= 0:
                raise
            time.sleep(min(delay, remaining))
            delay = min(delay * 2, 15.0)


def indexes_ready(response: NativeResponse) -> bool:
    data = response.data
    if data.get("ready_for_evaluation") is True:
        return True
    indexes = data.get("index_status") or data.get("indexes") or data.get("retrieval")
    if isinstance(indexes, dict):
        required = [indexes.get("exact"), indexes.get("lexical"), indexes.get("semantic")]
        if all(value is not None for value in required):
            return all(str(value).casefold() in READY_VALUES for value in required)
    status = str(data.get("status") or response.body.get("status") or "").casefold()
    return status in READY_VALUES


def wait_for_indexes(
    client: NativeApiClient,
    imported: NativeResponse,
    *,
    timeout_seconds: float = 300.0,
    poll_seconds: float = 0.25,
) -> NativeResponse:
    if indexes_ready(imported):
        return imported
    data = imported.data
    status_url = data.get("status_url")
    import_id = response_field(imported, "import_id")
    if not status_url and import_id:
        status_url = f"/v1/admin/eval/imports/{urllib.parse.quote(str(import_id), safe=':_-')}"
    if not status_url:
        raise RuntimeError("eval import response did not include ready indexes, status_url, or import_id")
    deadline = time.monotonic() + timeout_seconds
    latest = imported
    while time.monotonic() < deadline:
        latest = client.get(str(status_url))
        if indexes_ready(latest):
            return latest
        time.sleep(poll_seconds)
    raise TimeoutError(
        f"Straylight indexes were not ready after {timeout_seconds:.1f}s: "
        f"{json.dumps(recursively_redact_secrets(latest.body), sort_keys=True)[:1000]}"
    )


def provision_evaluation(
    client: NativeApiClient,
    *,
    run_id: str,
    case_id: str,
    display_scope: str,
    access_mode: str,
    documents: list[dict[str, Any]],
    delta_documents: list[dict[str, Any]] | None = None,
    seed_checkpoint: dict[str, Any] | None = None,
    timeout_seconds: float = 300.0,
    dependency_retry_seconds: float = 1.0,
) -> dict[str, Any]:
    deltas = delta_documents or []
    authorization_scope = stable_scope(run_id, case_id)
    payload = {
        "schema": "straylight-eval-import@v1",
        "run_id": run_id,
        "case_id": case_id,
        "authorization_scope": authorization_scope,
        "display_scope": display_scope,
        "access_mode": access_mode,
        "documents": documents,
        "delta_documents": deltas,
        "seed_checkpoint": seed_checkpoint,
        "idempotency_key": stable_import_key(run_id, case_id, documents, deltas),
    }
    imported = post_import_with_retry(
        client,
        payload,
        timeout_seconds=timeout_seconds,
        initial_delay_seconds=dependency_retry_seconds,
    )
    ready = wait_for_indexes(client, imported, timeout_seconds=timeout_seconds)
    issued_token = response_field(imported, "credential_token", "token") or client.token
    checkpoint_id = response_field(imported, "checkpoint_id", "seed_checkpoint_id")
    corpus_revision = response_field(ready, "corpus_revision", "revision_id") or response_field(
        imported, "corpus_revision", "revision_id"
    )
    base_revision = response_field(imported, "base_corpus_revision", "base_revision")
    return {
        "authorization_scope": response_field(imported, "authorization_scope") or authorization_scope,
        "requested_authorization_scope": response_field(
            imported, "requested_authorization_scope"
        ) or authorization_scope,
        "display_scope": display_scope,
        "access_mode": access_mode,
        "import_id": response_field(imported, "import_id"),
        "checkpoint_id": checkpoint_id,
        "base_corpus_revision": base_revision,
        "corpus_revision": corpus_revision,
        "token": issued_token,
        "provisioning": {
            "documents": len(documents),
            "delta_documents": len(deltas),
            "characters": sum(len(item["content"]) for item in [*documents, *deltas]),
            "import_http_status": imported.http_status,
            "import_elapsed_ms": round(imported.elapsed_ms, 3),
            "ready_elapsed_ms": round(ready.elapsed_ms, 3),
            "import_response": recursively_redact_secrets(imported.body),
            "ready_response": recursively_redact_secrets(ready.body),
        },
    }


def provisioning_matches_run_case(
    metadata: dict[str, Any],
    *,
    run_id: str,
    case_id: str,
    require_checkpoint: bool = False,
) -> bool:
    import_response = metadata.get("provisioning", {}).get("import_response", {})
    requested_scope = (
        metadata.get("requested_authorization_scope")
        or import_response.get("requested_authorization_scope")
    )
    return bool(
        requested_scope == stable_scope(run_id, case_id)
        and metadata.get("authorization_scope")
        and metadata.get("token")
        and (not require_checkpoint or metadata.get("checkpoint_id"))
    )


def write_native_memory_wrapper(
    run_dir: Path,
    *,
    task: str,
    display_scope: str,
    authorization_scope: str,
    run_id: str,
    case_id: str,
    checkpoint_id: str | None = None,
) -> None:
    task_file = run_dir / "task.txt"
    task_file.write_text(task.rstrip() + "\n", encoding="utf-8")
    arguments = [
        "python3",
        str(PROJECT_ROOT / "native_memory.py"),
        "--state",
        str(run_dir / "native-session.json"),
        "--task-file",
        str(task_file),
        "--scope",
        display_scope,
        "--authorization-scope",
        authorization_scope,
        "--run-id",
        run_id,
        "--case-id",
        case_id,
    ]
    if checkpoint_id:
        arguments.extend(["--checkpoint-id", checkpoint_id])
    rendered = " ".join(shlex.quote(value) for value in arguments)
    wrapper = run_dir / "memory"
    wrapper.write_text(f"#!/bin/sh\nexec {rendered} \"$@\"\n", encoding="utf-8")
    wrapper.chmod(0o700)


def public_provisioning(metadata: dict[str, Any]) -> dict[str, Any]:
    return {
        key: recursively_redact_secrets(value)
        for key, value in metadata.items()
        if key != "token"
    }

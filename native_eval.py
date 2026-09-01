#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import hmac
import json
import mimetypes
import os
import shlex
import stat
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


@dataclass(frozen=True)
class NativeDownload:
    path: Path
    content_hash: str
    size_bytes: int
    media_type: str
    elapsed_ms: float


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
        self.base_url = (base_url or os.environ.get("BRUNN_API_URL") or "").rstrip("/")
        self.token = token or os.environ.get("BRUNN_EVAL_TOKEN") or ""
        self.run_id = run_id
        self.case_id = case_id
        self.timeout = timeout
        if not self.base_url:
            raise ValueError("BRUNN_API_URL is required for native evaluation")
        if not self.token:
            raise ValueError("BRUNN_EVAL_TOKEN is required for native evaluation")

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
            headers["X-Brunn-Eval-Run"] = self.run_id
        if self.case_id:
            headers["X-Brunn-Eval-Case"] = self.case_id
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

    def request_multipart(
        self,
        path: str,
        *,
        fields: list[tuple[str, str]],
        files: list[tuple[str, Path]],
    ) -> NativeResponse:
        content_type, body = encode_multipart(fields, files)
        url = f"{self.base_url}/{path.lstrip('/')}"
        headers = {
            "Accept": "application/json",
            "Authorization": f"Bearer {self.token}",
            "Content-Type": content_type,
        }
        if self.run_id:
            headers["X-Brunn-Eval-Run"] = self.run_id
        if self.case_id:
            headers["X-Brunn-Eval-Case"] = self.case_id
        request = urllib.request.Request(url, data=body, headers=headers, method="POST")
        started = time.monotonic()
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                raw = response.read()
                parsed = json.loads(raw or b"{}")
                if not isinstance(parsed, dict):
                    parsed = {"data": parsed}
                return NativeResponse(
                    body=parsed,
                    http_status=response.status,
                    elapsed_ms=(time.monotonic() - started) * 1000,
                    headers={
                        key.casefold(): value
                        for key, value in response.headers.items()
                    },
                )
        except urllib.error.HTTPError as exc:
            elapsed_ms = (time.monotonic() - started) * 1000
            try:
                raw = exc.read()
            finally:
                exc.close()
            try:
                parsed = json.loads(raw or b"{}")
            except json.JSONDecodeError:
                parsed = {
                    "error": "http_error",
                    "message": raw.decode("utf-8", errors="replace"),
                }
            if not isinstance(parsed, dict):
                parsed = {"error": "http_error", "data": parsed}
            raise NativeApiError(exc.code, parsed, elapsed_ms=elapsed_ms) from exc

    def download_verified(
        self,
        path: str,
        destination: Path,
        *,
        expected_hash: str,
        expected_size: int,
    ) -> NativeDownload:
        expected_digest = expected_hash.removeprefix("sha256:").casefold()
        if not re_full_sha256(expected_digest):
            raise ValueError("expected_hash must be a SHA-256 digest")
        if expected_size < 0:
            raise ValueError("expected_size must not be negative")
        destination.parent.mkdir(parents=True, exist_ok=True)
        started = time.monotonic()
        if destination.exists() or destination.is_symlink():
            existing = destination.lstat()
            if (
                not stat.S_ISREG(existing.st_mode)
                or existing.st_uid != os.getuid()
                or stat.S_IMODE(existing.st_mode) != 0o600
            ):
                raise PermissionError(
                    f"refusing to reuse non-private asset download {destination}"
                )
            if existing.st_size != expected_size:
                raise FileExistsError(
                    f"existing asset download does not match metadata: {destination}"
                )
            digest = hashlib.sha256()
            size = 0
            with destination.open("rb") as source:
                while chunk := source.read(1024 * 1024):
                    size += len(chunk)
                    digest.update(chunk)
            actual_digest = digest.hexdigest()
            if size != expected_size or actual_digest != expected_digest:
                raise FileExistsError(
                    "existing asset download failed size or SHA-256 verification: "
                    f"{destination}"
                )
            return NativeDownload(
                path=destination,
                content_hash=f"sha256:{actual_digest}",
                size_bytes=size,
                media_type=mimetypes.guess_type(destination.name)[0]
                or "application/octet-stream",
                elapsed_ms=(time.monotonic() - started) * 1000,
            )
        temporary = destination.with_name(
            f".{destination.name}.{os.getpid()}.{time.time_ns()}.tmp"
        )
        url = path if path.startswith(("http://", "https://")) else (
            f"{self.base_url}/{path.lstrip('/')}"
        )
        headers = {
            "Accept": "*/*",
            "Accept-Encoding": "identity",
            "Authorization": f"Bearer {self.token}",
        }
        if self.run_id:
            headers["X-Brunn-Eval-Run"] = self.run_id
        if self.case_id:
            headers["X-Brunn-Eval-Case"] = self.case_id
        request = urllib.request.Request(url, headers=headers, method="GET")
        digest = hashlib.sha256()
        size = 0
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                response_hash = (
                    response.headers.get("X-Brunn-State-SHA256") or ""
                ).removeprefix("sha256:").casefold()
                content_length = response.headers.get("Content-Length")
                if response_hash != expected_digest:
                    raise RuntimeError("asset hash header does not match metadata")
                if content_length is None or int(content_length) != expected_size:
                    raise RuntimeError("asset content length does not match metadata")
                with temporary.open("xb", buffering=0) as output:
                    os.chmod(temporary, 0o600)
                    while True:
                        chunk = response.read(1024 * 1024)
                        if not chunk:
                            break
                        size += len(chunk)
                        if size > expected_size:
                            raise RuntimeError("asset download exceeded its expected size")
                        digest.update(chunk)
                        output.write(chunk)
                    output.flush()
                    os.fsync(output.fileno())
                media = response.headers.get_content_type()
            actual_digest = digest.hexdigest()
            if size != expected_size or actual_digest != expected_digest:
                raise RuntimeError("asset download failed size or SHA-256 verification")
            temporary.replace(destination)
            return NativeDownload(
                path=destination,
                content_hash=f"sha256:{actual_digest}",
                size_bytes=size,
                media_type=media,
                elapsed_ms=(time.monotonic() - started) * 1000,
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
                body = {"error": "http_error", "message": "asset download failed"}
            if not isinstance(body, dict):
                body = {"error": "http_error"}
            raise NativeApiError(exc.code, body, elapsed_ms=elapsed_ms) from exc
        finally:
            if temporary.exists():
                temporary.unlink()


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


def encode_multipart(
    fields: list[tuple[str, str]],
    files: list[tuple[str, Path]],
) -> tuple[str, bytes]:
    seed = "\0".join(
        [*(f"{key}={value}" for key, value in fields), *(str(path) for _, path in files)]
    )
    boundary = f"brunn-state-eval-{hashlib.sha256(seed.encode()).hexdigest()[:32]}"
    output = bytearray()
    for name, value in fields:
        output.extend(f"--{boundary}\r\n".encode())
        output.extend(
            f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode()
        )
        output.extend(value.encode())
        output.extend(b"\r\n")
    for logical_path, path in files:
        media = mimetypes.guess_type(logical_path)[0] or "application/octet-stream"
        output.extend(f"--{boundary}\r\n".encode())
        output.extend(b'Content-Disposition: form-data; name="path"\r\n\r\n')
        output.extend(logical_path.encode("utf-8"))
        output.extend(b"\r\n")
        output.extend(f"--{boundary}\r\n".encode())
        output.extend(
            (
                'Content-Disposition: form-data; name="file"; '
                f'filename="{path.name}"\r\n'
            ).encode()
        )
        output.extend(f"Content-Type: {media}\r\n\r\n".encode())
        output.extend(path.read_bytes())
        output.extend(b"\r\n")
    output.extend(f"--{boundary}--\r\n".encode())
    return f"multipart/form-data; boundary={boundary}", bytes(output)


def re_full_sha256(value: str) -> bool:
    return len(value) == 64 and all(character in "0123456789abcdef" for character in value)


def provision_binary_assets(
    metadata: dict[str, Any],
    *,
    corpus_root: Path,
    logical_paths: Iterable[str],
    run_id: str,
    case_id: str,
    timeout_seconds: float,
    describe_binaries: bool = True,
) -> dict[str, Any]:
    paths = tuple(sorted(set(logical_paths)))
    if not paths:
        return {"files": 0, "bytes": 0, "stage_ref": None}
    token = str(metadata.get("token") or "")
    scope = str(metadata.get("authorization_scope") or "")
    if not token or not scope:
        raise RuntimeError("binary provisioning requires an issued token and scope")
    files: list[tuple[str, Path]] = []
    total_bytes = 0
    for logical_path in paths:
        path = corpus_root / logical_path
        if not path.is_file():
            raise FileNotFoundError(f"binary evaluation asset is missing: {logical_path}")
        size = path.stat().st_size
        if size > 64 * 1024 * 1024:
            raise ValueError(
                f"binary evaluation asset requires resumable provisioning: {logical_path}"
            )
        total_bytes += size
        if total_bytes > 64 * 1024 * 1024:
            raise ValueError("binary evaluation stage exceeds 64 MiB")
        files.append((logical_path, path))
    digest = hashlib.sha256()
    for logical_path, path in files:
        digest.update(logical_path.encode())
        digest.update(b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    stable_import_id = f"eval-binary:{digest.hexdigest()}"
    client = NativeApiClient(
        base_url=os.environ.get("BRUNN_API_URL"),
        token=token,
        run_id=run_id,
        case_id=case_id,
        timeout=max(120.0, timeout_seconds),
    )
    staged = client.request_multipart(
        "/v1/memory/stage",
        fields=[
            ("scope", scope),
            ("stable_import_id", stable_import_id),
            ("describe_binaries", str(describe_binaries).lower()),
        ],
        files=files,
    )
    stage = staged.data
    stage_ref = str(stage.get("stage_ref") or stage.get("id") or "")
    if not stage_ref:
        raise RuntimeError("binary stage response did not include stage_ref")
    deadline = time.monotonic() + timeout_seconds
    while str(stage.get("status") or "") not in {"ready", "promoted"}:
        if str(stage.get("status") or "") in {"failed", "expired", "quarantined"}:
            raise RuntimeError(f"binary stage ended in {stage.get('status')}: {stage_ref}")
        if time.monotonic() >= deadline:
            raise TimeoutError(f"binary stage did not become ready: {stage_ref}")
        time.sleep(0.25)
        stage = client.get(
            f"/v1/stages/{urllib.parse.quote(stage_ref, safe=':')}"
        ).data
    entries = stage.get("entries")
    if not isinstance(entries, list):
        stage = client.get(
            f"/v1/stages/{urllib.parse.quote(stage_ref, safe=':')}"
        ).data
        entries = stage.get("entries")
    if not isinstance(entries, list) or not entries:
        raise RuntimeError(f"binary stage has no entries: {stage_ref}")
    selected = [
        item["path"]
        for item in entries
        if isinstance(item, dict) and isinstance(item.get("path"), str)
    ]
    expected_assets = []
    for item in entries:
        if not isinstance(item, dict) or item.get("path") not in paths:
            continue
        expected_assets.append({
            "path": item.get("path"),
            "asset_ref": item.get("asset_ref"),
            "version": item.get("asset_version"),
            "content_hash": item.get("content_hash") or item.get("hash"),
            "size_bytes": item.get("size_bytes"),
        })
    if len(expected_assets) != len(paths) or any(
        not isinstance(item.get("asset_ref"), str)
        or not isinstance(item.get("version"), int)
        or not isinstance(item.get("content_hash"), str)
        or not isinstance(item.get("size_bytes"), int)
        for item in expected_assets
    ):
        raise RuntimeError(
            f"binary stage omitted immutable asset identity: {stage_ref}"
        )
    promoted = client.post(
        "/v1/memory/save",
        {
            "intent": "binary_evaluation_import",
            "scope": scope,
            "root_refs": [],
            "source_refs": [],
            "base_corpus_revision": stage.get("base_corpus_revision"),
            "idempotency_key": f"{stable_import_id}:{case_id}",
            "items": [
                {
                    "action": "create",
                    "kind": "import_receipt",
                    "payload": {
                        "stage_ref": stage_ref,
                        "selected_entries": selected,
                    },
                }
            ],
        },
    )
    return {
        "files": len(files),
        "bytes": total_bytes,
        "stage_ref": stage_ref,
        "selected_entries": len(selected),
        "assets": expected_assets,
        "descriptions_requested": describe_binaries,
        "corpus_revision": response_field(
            promoted,
            "corpus_revision",
            "resulting_corpus_revision",
        ),
    }


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
    import_path: str = "/v1/admin/eval/import",
) -> NativeResponse:
    deadline = time.monotonic() + timeout_seconds
    delay = max(0.01, initial_delay_seconds)
    while True:
        try:
            return client.post(import_path, payload)
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
    poll_seconds: float = 0.5,
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
    issued_token = response_field(imported, "credential_token", "token")
    if not isinstance(issued_token, str) or not issued_token:
        raise RuntimeError("eval import did not issue a credential for index readiness")
    status_client = NativeApiClient(
        base_url=client.base_url,
        token=issued_token,
        run_id=client.run_id,
        case_id=client.case_id,
        timeout=client.timeout,
    )
    deadline = time.monotonic() + timeout_seconds
    latest = imported
    while time.monotonic() < deadline:
        latest = status_client.get(str(status_url))
        if indexes_ready(latest):
            return latest
        time.sleep(poll_seconds)
    raise TimeoutError(
        f"Brunn indexes were not ready after {timeout_seconds:.1f}s: "
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
    import_path: str = "/v1/admin/eval/import",
    wait_for_semantic: bool = True,
    batch_size: int | None = None,
) -> dict[str, Any]:
    deltas = delta_documents or []
    authorization_scope = stable_scope(run_id, case_id)
    if batch_size is not None and batch_size <= 0:
        raise ValueError("batch_size must be positive")
    document_batches = (
        [
            documents[offset:offset + batch_size]
            for offset in range(0, len(documents), batch_size)
        ]
        if batch_size is not None and len(documents) > batch_size
        else [documents]
    )
    import_key = stable_import_key(run_id, case_id, documents, deltas)
    imported_batches: list[NativeResponse] = []
    for batch_index, batch_documents in enumerate(document_batches):
        final_batch = batch_index + 1 == len(document_batches)
        payload = {
            "schema": "brunn-eval-import@v1",
            "run_id": run_id,
            "case_id": case_id,
            "authorization_scope": authorization_scope,
            "display_scope": display_scope,
            "access_mode": access_mode,
            "documents": batch_documents,
            "delta_documents": deltas if final_batch else [],
            "seed_checkpoint": seed_checkpoint if final_batch else None,
            "idempotency_key": import_key,
        }
        if len(document_batches) > 1:
            payload["batch_index"] = batch_index
            payload["batch_count"] = len(document_batches)
        imported_batches.append(post_import_with_retry(
            client,
            payload,
            timeout_seconds=timeout_seconds,
            initial_delay_seconds=dependency_retry_seconds,
            import_path=import_path,
        ))
    imported = imported_batches[-1]
    issued_token = response_field(imported, "credential_token", "token")
    issued_scope = response_field(imported, "authorization_scope")
    requested_scope = response_field(imported, "requested_authorization_scope")
    if not isinstance(issued_token, str) or not issued_token:
        raise RuntimeError(
            "eval import did not issue a case-scoped credential"
        )
    if hmac.compare_digest(issued_token, client.token):
        raise RuntimeError(
            "eval import returned the parent/admin credential instead of a scoped credential"
        )
    for batch in imported_batches[:-1]:
        batch_token = response_field(batch, "credential_token", "token")
        if not isinstance(batch_token, str) or not hmac.compare_digest(
            batch_token,
            issued_token,
        ):
            raise RuntimeError(
                "batched eval import did not preserve one isolated credential"
            )
    if not isinstance(issued_scope, str) or not issued_scope:
        raise RuntimeError(
            "eval import did not return the credential's effective scope"
        )
    if not isinstance(requested_scope, str) or requested_scope != authorization_scope:
        raise RuntimeError(
            "eval import did not preserve the exact requested isolation scope"
        )
    imported_indexes = imported.data.get("index_status")
    if not wait_for_semantic:
        if not isinstance(imported_indexes, dict) or any(
            str(imported_indexes.get(lane, "")).casefold() not in READY_VALUES
            for lane in ("exact", "lexical")
        ):
            raise RuntimeError(
                "eval import did not make exact and lexical retrieval ready immediately"
            )
        ready = imported
    else:
        ready = wait_for_indexes(client, imported, timeout_seconds=timeout_seconds)
    checkpoint_id = response_field(imported, "checkpoint_id", "seed_checkpoint_id")
    corpus_revision = response_field(ready, "corpus_revision", "revision_id") or response_field(
        imported, "corpus_revision", "revision_id"
    )
    base_revision = response_field(
        imported_batches[0],
        "base_corpus_revision",
        "base_revision",
    )
    return {
        "authorization_scope": issued_scope,
        "requested_authorization_scope": requested_scope,
        "display_scope": display_scope,
        "access_mode": access_mode,
        "import_id": response_field(imported, "import_id"),
        "checkpoint_id": checkpoint_id,
        "base_corpus_revision": base_revision,
        "corpus_revision": corpus_revision,
        "status_url": response_field(imported, "status_url"),
        "token": issued_token,
        "credential_provenance": "service_issued_case_scope",
        "provisioning": {
            "documents": len(documents),
            "delta_documents": len(deltas),
            "characters": sum(len(item["content"]) for item in [*documents, *deltas]),
            "import_http_status": imported.http_status,
            "import_elapsed_ms": round(
                sum(batch.elapsed_ms for batch in imported_batches),
                3,
            ),
            "import_batch_count": len(imported_batches),
            "max_batch_documents": max(len(batch) for batch in document_batches),
            "ready_elapsed_ms": round(ready.elapsed_ms, 3),
            "semantic_waited": wait_for_semantic,
            "semantic_ready_at_start": (
                isinstance(imported_indexes, dict)
                and str(imported_indexes.get("semantic", "")).casefold()
                in READY_VALUES
            ),
            "index_status_at_start": recursively_redact_secrets(
                imported_indexes if isinstance(imported_indexes, dict) else {}
            ),
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
    expected_scope = stable_scope(run_id, case_id)
    return bool(
        requested_scope == expected_scope
        and isinstance(metadata.get("authorization_scope"), str)
        and metadata.get("authorization_scope")
        and metadata.get("token")
        and metadata.get("credential_provenance") == "service_issued_case_scope"
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
    script_path: Path | None = None,
    protocol: str = "legacy",
) -> None:
    if protocol not in {"legacy", "simple"}:
        raise ValueError("native memory protocol must be legacy or simple")
    task_file = run_dir / "task.txt"
    task_file.write_text(task.rstrip() + "\n", encoding="utf-8")
    arguments = [
        "python3",
        str(script_path or PROJECT_ROOT / "native_memory.py"),
        "--state",
        str(run_dir / "native-session.json"),
        "--task-file",
        str(task_file),
        "--scope",
        display_scope,
        "--authorization-scope",
        authorization_scope,
        "--protocol",
        protocol,
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

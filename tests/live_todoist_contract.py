#!/usr/bin/env python3

"""Real-stack gate-12e contract for the one-way Todoist integration.

The runner owns only disposable state. It starts a loopback recorded API
fixture plus the real Straylight API and worker binaries, provisions an owner
Web identity, stores a canary token through the vault API, and drives every
mutation through the owner Web session with CSRF protection. Evidence is
content-free and never contains the canary token or an Authorization header.
"""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import http.cookiejar
import http.server
import json
import os
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable
from xml.etree import ElementTree


SCHEMA = "straylight-todoist-gate12@v1"
PASSWORD = "not-a-real-password"
PASSWORD_HASH = (
    "$argon2id$v=19$m=19456,t=2,p=1$"
    "c3RyYXlsaWdodC1kdW1teSE$G4cTCRccjpoNV+1tywKuUd5bq/LsW4NT7Sq0Wt9H1Hw"
)
SHIP_ID = "9QwErTyUiOpAsDfG"
RECURRING_ID = "A1b2C3d4E5f6G7h8"
DELETED_ID = "H4rDNoDue0000001"


class ContractFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractFailure(message)


def run_command(
    command: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 180.0,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if completed.returncode != 0:
        label = " ".join(command[:4])
        detail = (completed.stderr or completed.stdout)[-1500:]
        raise ContractFailure(f"{label} failed with exit {completed.returncode}: {detail}")
    return completed


def docker_env(container: str, name: str) -> str:
    value = run_command(["docker", "exec", container, "printenv", name]).stdout.strip()
    require(bool(value), f"{container} did not expose required {name}")
    return value


def docker_config_env(container: str, name: str) -> str:
    lines = run_command(
        [
            "docker",
            "inspect",
            container,
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
        ]
    ).stdout.splitlines()
    prefix = f"{name}="
    matches = [line.removeprefix(prefix) for line in lines if line.startswith(prefix)]
    require(len(matches) == 1 and bool(matches[0]), f"{container} omitted required {name}")
    return matches[0]


def port_is_free(host: str, port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.2)
        return probe.connect_ex((host, port)) != 0


def uuid7() -> uuid.UUID:
    timestamp_ms = int(time.time() * 1000) & ((1 << 48) - 1)
    randomness = int.from_bytes(os.urandom(10), "big")
    value = (
        (timestamp_ms << 80)
        | (0x7 << 76)
        | (((randomness >> 68) & ((1 << 12) - 1)) << 64)
        | (0b10 << 62)
        | (randomness & ((1 << 62) - 1))
    )
    result = uuid.UUID(int=value)
    require(result.version == 7 and str(result) == str(result).lower(), "UUIDv7 failed")
    return result


@dataclass
class Check:
    name: str
    elapsed_ms: float
    detail: dict[str, Any]


class Recorder:
    def __init__(self) -> None:
        self.checks: list[Check] = []

    def record(
        self,
        name: str,
        action: Callable[[], Any],
        detail: Callable[[Any], dict[str, Any]] | None = None,
    ) -> Any:
        started = time.monotonic()
        result = action()
        elapsed = round((time.monotonic() - started) * 1000, 3)
        self.checks.append(Check(name, elapsed, {} if detail is None else detail(result)))
        return result


@dataclass
class FixtureResponse:
    expected_cursor: str | None
    status: int
    body: dict[str, Any]


class FixturePlan:
    def __init__(self, token: str) -> None:
        self._token = token
        self._lock = threading.Lock()
        self.sync: deque[FixtureResponse] = deque()
        self.completed: deque[FixtureResponse] = deque()
        self.requests: list[dict[str, Any]] = []
        self.errors: list[str] = []

    def enqueue_sync(
        self,
        expected_cursor: str,
        body: dict[str, Any],
        *,
        status: int = 200,
    ) -> None:
        with self._lock:
            self.sync.append(FixtureResponse(expected_cursor, status, copy.deepcopy(body)))

    def enqueue_completed(
        self,
        body: dict[str, Any],
        *,
        status: int = 200,
    ) -> None:
        with self._lock:
            self.completed.append(FixtureResponse(None, status, copy.deepcopy(body)))

    def consume_sync(
        self, authorization: str | None, body: bytes
    ) -> tuple[int, dict[str, Any]]:
        with self._lock:
            self._validate_authorization(authorization)
            values = urllib.parse.parse_qs(body.decode("ascii"), keep_blank_values=True)
            keys = set(values)
            if keys != {"sync_token", "resource_types"}:
                self._fail(f"sync form fields were {sorted(keys)!r}")
            if "commands" in values:
                self._fail("read-only Sync request contained commands")
            if values.get("resource_types") != ['["projects","items"]']:
                self._fail("Sync resource_types was not the exact bounded set")
            if not self.sync:
                self._fail("unexpected Sync request")
            response = self.sync.popleft()
            cursor = values.get("sync_token", [None])[0]
            if cursor != response.expected_cursor:
                self._fail(
                    f"Sync cursor mismatch: expected {response.expected_cursor!r}, got {cursor!r}"
                )
            self.requests.append({"kind": "sync", "cursor": cursor, "status": response.status})
            return response.status, response.body

    def consume_completed(
        self, authorization: str | None, query: str
    ) -> tuple[int, dict[str, Any]]:
        with self._lock:
            self._validate_authorization(authorization)
            values = urllib.parse.parse_qs(query, keep_blank_values=True)
            required = {"since", "until", "limit"}
            if not required.issubset(values) or set(values) - (required | {"cursor"}):
                self._fail(f"completed query fields were {sorted(values)!r}")
            if values.get("limit") != ["200"]:
                self._fail("completed request did not use limit=200")
            if not self.completed:
                self._fail("unexpected completed-tasks request")
            response = self.completed.popleft()
            self.requests.append(
                {
                    "kind": "completed",
                    "paginated": "cursor" in values,
                    "status": response.status,
                }
            )
            return response.status, response.body

    def _validate_authorization(self, authorization: str | None) -> None:
        if authorization != f"Bearer {self._token}":
            self._fail("fixture received missing or incorrect bearer authentication")

    def _fail(self, message: str) -> None:
        self.errors.append(message)
        raise ContractFailure(message)

    def snapshot(self) -> dict[str, Any]:
        with self._lock:
            return {
                "request_count": len(self.requests),
                "sync_count": sum(item["kind"] == "sync" for item in self.requests),
                "completed_count": sum(item["kind"] == "completed" for item in self.requests),
                "queued_sync": len(self.sync),
                "queued_completed": len(self.completed),
                "errors": list(self.errors),
            }


class FixtureHandler(http.server.BaseHTTPRequestHandler):
    server: "FixtureHttpServer"

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/api/v1/sync":
            self._reply(404, {})
            return
        try:
            length = int(self.headers.get("content-length", "0"))
            status, body = self.server.plan.consume_sync(
                self.headers.get("authorization"), self.rfile.read(length)
            )
        except Exception:
            self._reply(400, {})
            return
        self._reply(status, body)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != "/api/v1/tasks/completed/by_completion_date":
            self._reply(404, {})
            return
        try:
            status, body = self.server.plan.consume_completed(
                self.headers.get("authorization"), parsed.query
            )
        except Exception:
            self._reply(400, {})
            return
        self._reply(status, body)

    def _reply(self, status: int, body: dict[str, Any]) -> None:
        encoded = json.dumps(body, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, _format: str, *_args: Any) -> None:
        return


class FixtureHttpServer(http.server.ThreadingHTTPServer):
    allow_reuse_address = True

    def __init__(self, plan: FixturePlan) -> None:
        super().__init__(("127.0.0.1", 0), FixtureHandler)
        self.plan = plan


class RunningFixture:
    def __init__(self, plan: FixturePlan) -> None:
        self.server = FixtureHttpServer(plan)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def origin(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}"

    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class ProcessLog:
    def __init__(self, process: subprocess.Popen[str]) -> None:
        self.process = process
        self.lines: deque[str] = deque(maxlen=20_000)
        self.thread = threading.Thread(target=self._drain, daemon=True)
        self.thread.start()

    def _drain(self) -> None:
        stream = self.process.stdout
        if stream is None:
            return
        for line in stream:
            self.lines.append(line)

    def text(self) -> str:
        return "".join(self.lines)

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.thread.join(timeout=5)


class WebClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        expected: int | tuple[int, ...] = 200,
        csrf: bool = False,
        csrf_token_override: str | None = None,
    ) -> dict[str, Any]:
        payload = None if body is None else json.dumps(body).encode("utf-8")
        headers = {"accept": "application/json"}
        if payload is not None:
            headers["content-type"] = "application/json"
        if csrf:
            headers["x-csrf-token"] = self.csrf_token()
        elif csrf_token_override is not None:
            headers["x-csrf-token"] = csrf_token_override
        request = urllib.request.Request(
            f"{self.base_url}{path}", data=payload, headers=headers, method=method
        )
        try:
            with self.opener.open(request, timeout=30) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        allowed = (expected,) if isinstance(expected, int) else expected
        parsed: Any = json.loads(raw) if raw else {}
        if status not in allowed:
            raise ContractFailure(
                f"{method} {path} returned HTTP {status}, expected {allowed}: "
                f"{json.dumps(parsed, sort_keys=True)[:1000]}"
            )
        require(isinstance(parsed, dict), f"{method} {path} returned non-object JSON")
        return parsed

    def csrf_token(self) -> str:
        for cookie in self.jar:
            if cookie.name == "straylight_csrf":
                return cookie.value
        raise ContractFailure("Web session omitted the CSRF cookie")


class BearerClient:
    def __init__(self, base_url: str, token: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        expected: int | tuple[int, ...] = 200,
    ) -> dict[str, Any]:
        payload = None if body is None else json.dumps(body).encode("utf-8")
        headers = {"accept": "application/json", "authorization": f"Bearer {self.token}"}
        if payload is not None:
            headers["content-type"] = "application/json"
        request = urllib.request.Request(
            f"{self.base_url}{path}", data=payload, headers=headers, method=method
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        allowed = (expected,) if isinstance(expected, int) else expected
        parsed: Any = json.loads(raw) if raw else {}
        if status not in allowed:
            raise ContractFailure(
                f"{method} {path} returned HTTP {status}, expected {allowed}: "
                f"{json.dumps(parsed, sort_keys=True)[:1000]}"
            )
        require(isinstance(parsed, dict), f"{method} {path} returned non-object JSON")
        return parsed


def response_data(response: dict[str, Any]) -> dict[str, Any]:
    value = response.get("data", response)
    require(isinstance(value, dict), "response data was not an object")
    return value


class Database:
    def __init__(self, container: str, name: str, user: str) -> None:
        self.container = container
        self.name = name
        self.user = user

    def scalar(self, sql: str) -> str:
        completed = run_command(
            [
                "docker",
                "exec",
                self.container,
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-U",
                self.user,
                "-d",
                self.name,
                "-At",
                "-c",
                sql,
            ]
        )
        return completed.stdout.strip()

    def json(self, sql: str) -> dict[str, Any]:
        raw = self.scalar(sql)
        require(bool(raw), "database query returned no JSON row")
        parsed = json.loads(raw)
        require(isinstance(parsed, dict), "database query did not return an object")
        return parsed


def sql_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def sync_state(database: Database, user_id: str) -> dict[str, Any]:
    return database.json(
        "SELECT json_build_object("
        "'cursor',cursor,'last_outcome',last_outcome,'last_error_code',last_error_code,"
        "'next_run_at',next_run_at,'manual_requested_at',manual_requested_at,"
        "'lease_owner',lease_owner,'configuration_generation',configuration_generation) "
        "FROM straylight.task_sync_state WHERE user_id="
        f"{sql_literal(user_id)}::uuid AND system='todoist'"
    )


def external_task(database: Database, user_id: str, external_id: str) -> dict[str, Any]:
    return database.json(
        "SELECT json_build_object("
        "'task_id',ref.task_id,'entry_id',ref.entry_id,'series_id',ref.series_id,"
        "'occurrence_key',ref.occurrence_key,'ref_metadata',ref.metadata,"
        "'version',task.entry_version,'title',task.title,'status',task.status,"
        "'project',task.project_slug,'task',task.task) "
        "FROM straylight.task_external_refs AS ref "
        "JOIN straylight.task_index AS task ON task.user_id=ref.user_id AND task.task_id=ref.task_id "
        f"WHERE ref.user_id={sql_literal(user_id)}::uuid AND ref.system='todoist' "
        f"AND ref.external_id={sql_literal(external_id)}"
    )


def occurrence_rows(database: Database, user_id: str, series_id: str) -> list[dict[str, Any]]:
    raw = database.scalar(
        "SELECT coalesce(json_agg(json_build_object("
        "'occurrence_key',occ.occurrence_key,'task_id',occ.task_id,'status',task.status) "
        "ORDER BY occ.occurrence_key),'[]'::json) "
        "FROM straylight.task_todoist_occurrences AS occ "
        "JOIN straylight.task_index AS task ON task.user_id=occ.user_id AND task.task_id=occ.task_id "
        f"WHERE occ.user_id={sql_literal(user_id)}::uuid AND occ.series_id={sql_literal(series_id)}"
    )
    parsed = json.loads(raw)
    require(isinstance(parsed, list), "occurrence query did not return a list")
    return parsed


def task_count(database: Database, user_id: str) -> int:
    return int(
        database.scalar(
            "SELECT count(*) FROM straylight.task_index WHERE user_id="
            f"{sql_literal(user_id)}::uuid"
        )
    )


def wait_until(
    predicate: Callable[[], Any],
    description: str,
    *,
    timeout: float = 45.0,
) -> Any:
    deadline = time.monotonic() + timeout
    last: Any = None
    while time.monotonic() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(0.2)
    raise ContractFailure(f"timed out waiting for {description}; last={last!r}")


def wait_for_http(base_url: str, process: ProcessLog) -> None:
    def ready() -> bool:
        if process.process.poll() is not None:
            raise ContractFailure(f"API exited before readiness: {process.text()[-1500:]}")
        try:
            with urllib.request.urlopen(f"{base_url}/ready", timeout=2) as response:
                return response.status == 200
        except (urllib.error.URLError, TimeoutError):
            return False

    wait_until(ready, "API readiness", timeout=60)


def start_process(binary: Path, command: str, env: dict[str, str]) -> ProcessLog:
    process = subprocess.Popen(
        [str(binary), command],
        cwd=binary.parents[4],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        bufsize=1,
    )
    return ProcessLog(process)


def task_cell(task: dict[str, Any], field: str) -> dict[str, Any]:
    cells = task.get("task")
    require(isinstance(cells, dict), "task projection omitted sourced cells")
    cell = cells.get(field)
    require(isinstance(cell, dict), f"task projection omitted sourced {field}")
    return cell


def completed_item(external_id: str, occurrence_key: str, completed_at: str) -> dict[str, Any]:
    return {
        "id": external_id,
        "completed_at": completed_at,
        "due": {
            "date": occurrence_key,
            "string": "every Monday at 9am",
            "lang": "en",
            "is_recurring": True,
            "timezone": "America/Los_Angeles",
        },
    }


def recurring_item(template: dict[str, Any], due: str) -> dict[str, Any]:
    item = copy.deepcopy(template)
    item["due"]["date"] = due
    item["checked"] = False
    item["is_deleted"] = False
    item["completed_at"] = None
    return item


def sync_payload(cursor: str, items: list[dict[str, Any]], *, full: bool = False) -> dict[str, Any]:
    return {
        "sync_token": cursor,
        "full_sync": full,
        "projects": [],
        "items": items,
    }


def completed_payload(items: list[dict[str, Any]]) -> dict[str, Any]:
    return {"items": items, "next_cursor": None}


def manual_pull(web: WebClient, key: str) -> dict[str, Any]:
    result = response_data(
        web.request(
            "POST",
            "/v1/workspace/integrations/todoist/pull",
            body={"idempotency_key": key},
            csrf=True,
        )
    )
    require(isinstance(result.get("queued"), bool), "pull response omitted queued")
    return result


def configure_mode(
    web: WebClient, key: str, mode: str, expected_generation: int
) -> dict[str, Any]:
    result = response_data(
        web.request(
            "PUT",
            "/v1/workspace/integrations/todoist/config",
            body={
                "expected_generation": expected_generation,
                "idempotency_key": key,
                "mode": mode,
            },
            csrf=True,
        )
    )
    status = result.get("status")
    require(isinstance(status, dict), "Todoist config response omitted status")
    return result


def wait_for_cursor(database: Database, user_id: str, cursor: str) -> dict[str, Any]:
    def observed() -> dict[str, Any] | bool:
        state = sync_state(database, user_id)
        if state.get("cursor") == cursor and state.get("last_outcome") == "success":
            return state
        return False

    return wait_until(observed, f"successful Todoist cursor {cursor}")


def wait_for_error(database: Database, user_id: str, code: str) -> dict[str, Any]:
    def observed() -> dict[str, Any] | bool:
        state = sync_state(database, user_id)
        if state.get("last_outcome") == "error" and state.get("last_error_code") == code:
            return state
        return False

    return wait_until(observed, f"content-free Todoist error {code}")


def build_runtime_env(
    args: argparse.Namespace, fixture_origin: str, secret_key: str
) -> tuple[dict[str, str], str]:
    admin_password = docker_env(args.database_container, "POSTGRES_PASSWORD")
    rw_password = docker_env(args.database_container, "APP_RW_PASSWORD")
    ro_password = docker_env(args.database_container, "APP_RO_PASSWORD")
    minio_access = docker_config_env(
        args.minio_init_container, "STRAYLIGHT_MINIO_ACCESS_KEY"
    )
    minio_secret = docker_config_env(
        args.minio_init_container, "STRAYLIGHT_MINIO_SECRET_KEY"
    )
    minio_bucket = docker_config_env(
        args.minio_init_container, "STRAYLIGHT_MINIO_BUCKET"
    )
    admin_url = (
        f"postgresql://admin:{admin_password}@127.0.0.1:{args.database_port}/"
        f"{args.database_name}"
    )
    env = {
        **os.environ,
        "STRAYLIGHT_ENV": "development",
        "STRAYLIGHT_BIND": f"127.0.0.1:{args.api_port}",
        "DATABASE_URL_ADMIN": admin_url,
        "DATABASE_URL_RW": (
            f"postgresql://app_rw:{rw_password}@127.0.0.1:{args.database_port}/"
            f"{args.database_name}"
        ),
        "DATABASE_URL_RO": (
            f"postgresql://app_ro:{ro_password}@127.0.0.1:{args.database_port}/"
            f"{args.database_name}"
        ),
        "STRAYLIGHT_CONTINUATION_SECRET": "gate12-continuation-secret-32-bytes-minimum",
        "STRAYLIGHT_SECRET_ENCRYPTION_KEY": secret_key,
        "STRAYLIGHT_S3_ENDPOINT": f"http://127.0.0.1:{args.minio_port}",
        "STRAYLIGHT_S3_REGION": "us-east-1",
        "STRAYLIGHT_S3_BUCKET": minio_bucket,
        "STRAYLIGHT_S3_ACCESS_KEY": minio_access,
        "STRAYLIGHT_S3_SECRET_KEY": minio_secret,
        "STRAYLIGHT_S3_FORCE_PATH_STYLE": "true",
        "STRAYLIGHT_S3_CREATE_BUCKET": "false",
        "STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS": "true",
        "STRAYLIGHT_LEGACY_API_ENABLED": "false",
        "STRAYLIGHT_EVALUATION_API_ENABLED": "false",
        "STRAYLIGHT_DREAM_SCHEDULER_ENABLED": "false",
        "STRAYLIGHT_APNS_DELIVERY_ENABLED": "false",
        "STRAYLIGHT_TODOIST_SYNC_ENABLED": "true",
        "STRAYLIGHT_TODOIST_FIXTURE_ORIGIN": fixture_origin,
        "RUST_LOG": "info,straylight=debug",
    }
    return env, admin_url


def provision_owner(binary: Path, env: dict[str, str], suffix: str) -> tuple[str, str]:
    created = json.loads(
        run_command(
            [
                str(binary),
                "operator",
                "provision-user",
                "--external-ref",
                f"contract:todoist-gate12:{suffix}",
                "--display-name",
                "Todoist Gate 12 Owner",
                "--credential-name",
                "Todoist Gate 12 owner",
            ],
            env=env,
        ).stdout
    )
    user = created.get("user")
    credential = created.get("credential")
    require(isinstance(user, dict) and isinstance(credential, dict), "owner provisioning failed")
    user_ref = user.get("id")
    token = credential.get("token")
    require(isinstance(user_ref, str) and user_ref.startswith("user:"), "user ref missing")
    require(isinstance(token, str) and token, "owner bearer token missing")
    run_command(
        [
            str(binary),
            "operator",
            "configure-web-identity",
            "--user-id",
            user_ref,
            "--username",
            f"gate12owner-{suffix}",
            "--email",
            f"gate12owner-{suffix}@example.test",
        ],
        env=env,
    )
    return user_ref.removeprefix("user:"), token


def install_test_password(database: Database, user_id: str) -> None:
    database.scalar(
        "UPDATE straylight.web_identities SET password_hash="
        f"{sql_literal(PASSWORD_HASH)},updated_at=clock_timestamp() "
        f"WHERE user_id={sql_literal(user_id)}::uuid RETURNING user_id"
    )


def issue_narrow_bearer(database: Database, user_id: str, suffix: str) -> str:
    token = f"straylight_gate12_read_only_{uuid7().hex}_{uuid7().hex}"
    token_hash = hashlib.sha256(token.encode()).hexdigest()
    credential_id = str(uuid7())
    database.scalar(
        "WITH created AS ("
        "INSERT INTO straylight.api_credentials(id,user_id,label,token_hash,capabilities) VALUES ("
        f"{sql_literal(credential_id)}::uuid,{sql_literal(user_id)}::uuid,"
        f"{sql_literal(f'Todoist gate read only {suffix}')},{sql_literal(token_hash)},"
        "ARRAY['read','task.read']::text[]) RETURNING id,user_id), "
        "granted AS (INSERT INTO straylight.credential_scope_grants(credential_id,user_id,scope_id) "
        "SELECT created.id,created.user_id,scope.id FROM created JOIN straylight.scopes AS scope "
        "ON scope.user_id=created.user_id ORDER BY scope.created_at LIMIT 1 RETURNING credential_id) "
        "SELECT credential_id FROM granted"
    )
    return token


def login_owner(web: WebClient, email: str) -> None:
    web.request(
        "POST", "/v1/auth/login", body={"email": email, "password": PASSWORD}, expected=200
    )
    require(bool(web.csrf_token()), "owner login did not establish CSRF state")


def write_evidence(output: Path, evidence: dict[str, Any]) -> None:
    output.mkdir(parents=True, exist_ok=True)
    json_path = output / "todoist.json"
    xml_path = output / "todoist.xml"
    json_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    suite = ElementTree.Element(
        "testsuite",
        {
            "name": "straylight.todoist.gate12",
            "tests": str(len(evidence.get("checks", []))),
            "failures": "0" if evidence.get("status") == "pass" else "1",
            "time": f"{float(evidence.get('elapsed_ms', 0)) / 1000:.6f}",
        },
    )
    for check in evidence.get("checks", []):
        case = ElementTree.SubElement(
            suite,
            "testcase",
            {
                "name": str(check.get("name")),
                "time": f"{float(check.get('elapsed_ms', 0)) / 1000:.6f}",
            },
        )
        if check.get("detail"):
            output_node = ElementTree.SubElement(case, "system-out")
            output_node.text = json.dumps(check["detail"], sort_keys=True)
    if evidence.get("status") != "pass":
        case = ElementTree.SubElement(suite, "testcase", {"name": "scenario"})
        failure = ElementTree.SubElement(case, "failure", {"message": "gate-12e failed"})
        failure.text = str(evidence.get("error", "unknown failure"))
    ElementTree.ElementTree(suite).write(xml_path, encoding="utf-8", xml_declaration=True)


def scan_export(carrystate: Path, base_url: str, token: str, canary: str) -> int:
    with tempfile.TemporaryDirectory(prefix="straylight-todoist-export-") as temporary:
        export = Path(temporary) / "workspace"
        run_command(
            [
                str(carrystate),
                "workspace",
                "export",
                "--output",
                str(export),
                "--history",
            ],
            env={
                **os.environ,
                "CARRYSTATE_API_URL": base_url,
                "CARRYSTATE_API_TOKEN": token,
            },
        )
        files = 0
        for path in export.rglob("*"):
            if path.is_file():
                files += 1
                require(canary.encode() not in path.read_bytes(), f"canary leaked into {path.name}")
        return files


def scan_database(database: Database, canary: str) -> str:
    completed = run_command(
        [
            "docker",
            "exec",
            database.container,
            "pg_dump",
            "-U",
            database.user,
            "-d",
            database.name,
            "--data-only",
            "--no-owner",
            "--no-privileges",
        ],
        timeout=180,
    )
    require(canary not in completed.stdout, "vault canary appeared in database plaintext dump")
    return hashlib.sha256(completed.stdout.encode()).hexdigest()


def scan_object_store(
    binary: Path, env: dict[str, str], canary: str
) -> tuple[int, str]:
    with tempfile.TemporaryDirectory(prefix="straylight-todoist-objects-") as temporary:
        output = Path(temporary) / "backup"
        run_command(
            [str(binary), "object-store-backup", "export", "--output", str(output)],
            env=env,
            timeout=180,
        )
        digest = hashlib.sha256()
        files = 0
        for path in sorted(output.rglob("*")):
            if not path.is_file():
                continue
            body = path.read_bytes()
            require(canary.encode() not in body, f"canary leaked into object backup {path.name}")
            digest.update(path.relative_to(output).as_posix().encode())
            digest.update(b"\0")
            digest.update(hashlib.sha256(body).digest())
            files += 1
        return files, digest.hexdigest()


def run(args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    recorder = Recorder()
    require(port_is_free("127.0.0.1", args.api_port), f"API port {args.api_port} is occupied")
    require(args.binary.exists(), f"missing Straylight binary: {args.binary}")
    require(args.carrystate.exists(), f"missing carrystate binary: {args.carrystate}")
    base_fixture = json.loads(args.full_fixture.read_text(encoding="utf-8"))
    recurring_template = next(
        item for item in base_fixture["items"] if item.get("id") == RECURRING_ID
    )
    suffix = uuid7().hex[-10:]
    canary = f"todoist-gate12-{uuid7().hex}-{uuid7().hex}"
    apply_canary_id = f"ApplyRejectCanary{suffix}"
    apply_canary_occurrence = "2026-10-12T16:00:00Z"
    secret_key = base64.b64encode(os.urandom(32)).decode("ascii")
    plan = FixturePlan(canary)
    fixture = RunningFixture(plan)
    fixture.start()
    env, _admin_url = build_runtime_env(args, fixture.origin, secret_key)
    database = Database(args.database_container, args.database_name, args.database_user)
    api: ProcessLog | None = None
    worker: ProcessLog | None = None
    owner_token = ""
    captured_logs = ""
    try:
        user_id, owner_token = recorder.record(
            "owner.provision",
            lambda: provision_owner(args.binary, env, suffix),
            lambda value: {"user_id": value[0]},
        )
        secondary_id, secondary_token = provision_owner(args.binary, env, f"{suffix}b")
        secondary_token = ""
        recorder.checks.append(
            Check("tenant_b.provision", 0, {"user_id": secondary_id})
        )
        email = f"gate12owner-{suffix}@example.test"
        recorder.record("owner.password_fixture", lambda: install_test_password(database, user_id))

        api = start_process(args.binary, "serve", env)
        recorder.record("stack.ready", lambda: wait_for_http(args.base_url, api))
        web = WebClient(args.base_url)
        recorder.record("web.login_csrf", lambda: login_owner(web, email))

        recorder.record(
            "vault.put_canary",
            lambda: web.request(
                "POST",
                "/v1/workspace/secrets/put",
                body={
                    "name": "todoist-api-token",
                    "value": canary,
                    "description": "Disposable Todoist gate token",
                },
                csrf=True,
            ),
            lambda value: {
                "name": value.get("name"),
                "version": value.get("version"),
                "status": value.get("status"),
            },
        )
        status = response_data(
            recorder.record(
                "todoist.status.default_off",
                lambda: web.request("GET", "/v1/workspace/integrations/todoist/status"),
            )
        )
        require(status.get("saved_mode") == "off", "Todoist was not saved off by default")
        require(status.get("environment_enabled") is True, "environment gate was not enabled")
        require(status.get("token_configured") is True, "status did not observe vault token")
        generation = status.get("configuration_generation")
        require(isinstance(generation, int) and generation >= 1, "status omitted generation")

        denied_config = {
            "expected_generation": generation,
            "idempotency_key": f"todoist-gate:{suffix}:denied-config",
            "mode": "pull",
        }
        denied_pull = {"idempotency_key": f"todoist-gate:{suffix}:denied-pull"}
        owner_bearer = BearerClient(args.base_url, owner_token)
        read_only_token = issue_narrow_bearer(database, user_id, suffix)
        read_only = BearerClient(args.base_url, read_only_token)
        recorder.record(
            "trust.owner_bearer_not_web",
            lambda: (
                owner_bearer.request(
                    "PUT",
                    "/v1/workspace/integrations/todoist/config",
                    body=denied_config,
                    expected=403,
                ),
                owner_bearer.request(
                    "POST",
                    "/v1/workspace/integrations/todoist/pull",
                    body=denied_pull,
                    expected=403,
                ),
            ),
        )
        recorder.record(
            "trust.task_read_bearer_denied",
            lambda: (
                read_only.request(
                    "PUT",
                    "/v1/workspace/integrations/todoist/config",
                    body={**denied_config, "idempotency_key": f"todoist-gate:{suffix}:ro-config"},
                    expected=403,
                ),
                read_only.request(
                    "POST",
                    "/v1/workspace/integrations/todoist/pull",
                    body={"idempotency_key": f"todoist-gate:{suffix}:ro-pull"},
                    expected=403,
                ),
            ),
        )
        recorder.record(
            "trust.csrf_denial",
            lambda: (
                web.request(
                    "PUT",
                    "/v1/workspace/integrations/todoist/config",
                    body={**denied_config, "idempotency_key": f"todoist-gate:{suffix}:no-csrf"},
                    expected=403,
                ),
                web.request(
                    "POST",
                    "/v1/workspace/integrations/todoist/pull",
                    body={"idempotency_key": f"todoist-gate:{suffix}:wrong-csrf"},
                    expected=403,
                    csrf_token_override="definitely-not-the-session-token",
                ),
            ),
        )
        denied_state = sync_state(database, user_id)
        require(
            denied_state.get("cursor") is None
            and denied_state.get("configuration_generation") == generation,
            "denied integration mutations changed durable state",
        )
        read_only_token = ""

        recorder.record(
            "project.register",
            lambda: web.request(
                "PUT",
                "/v1/workspace/projects/straylight",
                body={
                    "title": "Straylight",
                    "description": "Project mapping fixture",
                    "aliases": [],
                    "source": "owner",
                    "idempotency_key": f"todoist-gate:{suffix}:project",
                },
                csrf=True,
            ),
        )
        configured = response_data(
            recorder.record(
                "todoist.configure_pull",
                lambda: web.request(
                    "PUT",
                    "/v1/workspace/integrations/todoist/config",
                    body={
                        "expected_generation": generation,
                        "idempotency_key": f"todoist-gate:{suffix}:configure",
                        "mode": "pull",
                    },
                    csrf=True,
                ),
            )
        )
        require(configured.get("changed") is True, "pull mode did not commit")

        initial = copy.deepcopy(base_fixture)
        initial["sync_token"] = "gate12-cursor-1"
        active_0907 = recurring_item(recurring_template, "2026-09-07T16:00:00Z")
        plan.enqueue_sync("*", initial)
        plan.enqueue_sync(
            "gate12-cursor-1",
            sync_payload("gate12-cursor-2", [active_0907], full=False),
        )
        plan.enqueue_completed(
            completed_payload(
                [
                    completed_item(
                        RECURRING_ID,
                        "2026-08-31T16:00:00Z",
                        "2026-08-31T18:00:00Z",
                    )
                ]
            )
        )
        initial_pull = manual_pull(web, f"todoist-gate:{suffix}:pull-initial")
        require(initial_pull.get("queued") is True, "initial pull was not queued")
        worker = start_process(args.binary, "worker", env)
        recorder.record(
            "todoist.initial_pull",
            lambda: wait_for_cursor(database, user_id, "gate12-cursor-2"),
        )
        require(task_count(database, user_id) == 4, "stale-full catch-up did not create exactly 4 tasks")
        ship = external_task(database, user_id, SHIP_ID)
        require(task_cell(ship, "soft_due").get("value") == "2026-08-30", "due did not map")
        hard_due = task_cell(ship, "hard_due")
        require(str(hard_due.get("value", "")).startswith("2026-09-01"), "deadline did not map")
        require(hard_due.get("note") == "todoist_deadline", "deadline provenance marker missing")
        contexts = task_cell(ship, "required_contexts").get("value")
        require(isinstance(contexts, list) and {"online", "release"}.issubset(contexts), "labels did not map")
        require(ship.get("project") == "straylight", "Todoist project did not map by registry name")
        recorder.checks.append(
            Check(
                "todoist.mapping",
                0,
                {
                    "deadline": "hard_due",
                    "due": "soft_due",
                    "labels": sorted(contexts),
                    "project": ship.get("project"),
                },
            )
        )
        initial_occurrences = occurrence_rows(database, user_id, RECURRING_ID)
        require(
            [row["occurrence_key"] for row in initial_occurrences]
            == ["2026-08-31T16:00:00Z", "2026-09-07T16:00:00Z"],
            f"stale-full completion catch-up lost an occurrence: {initial_occurrences!r}",
        )
        require(
            [row["status"] for row in initial_occurrences] == ["done", "open"],
            "stale-full catch-up did not produce done/open",
        )
        recorder.checks.append(Check("recurrence.stale_full_catchup", 0, {}))

        plan.enqueue_sync("gate12-cursor-2", sync_payload("gate12-cursor-3", []))
        plan.enqueue_completed(completed_payload([]))
        recorder.record(
            "todoist.idempotent_second_import",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:pull-replay"),
                wait_for_cursor(database, user_id, "gate12-cursor-3"),
            ),
        )
        require(task_count(database, user_id) == 4, "second import created duplicates")

        ship_detail = response_data(
            web.request("GET", f"/v1/workspace/tasks/{ship['task_id']}")
        )["task"]
        recorder.record(
            "owner.title_correction",
            lambda: web.request(
                "PATCH",
                f"/v1/workspace/tasks/{ship['task_id']}",
                body={
                    "expected_version": ship_detail["version"],
                    "idempotency_key": f"todoist-gate:{suffix}:owner-title",
                    "operation": {
                        "type": "correct",
                        "field": "title",
                        "value": "Owner-set launch title",
                        "source": "owner",
                        "reason": "Gate 12 owner precedence proof",
                    },
                },
                csrf=True,
            ),
        )

        recurring = external_task(database, user_id, RECURRING_ID)
        recurring_detail = response_data(
            web.request("GET", f"/v1/workspace/tasks/{recurring['task_id']}")
        )["task"]
        completed = response_data(
            recorder.record(
                "recurrence.owner_completion",
                lambda: web.request(
                    "PATCH",
                    f"/v1/workspace/tasks/{recurring['task_id']}",
                    body={
                        "expected_version": recurring_detail["version"],
                        "idempotency_key": f"todoist-gate:{suffix}:owner-complete",
                        "operation": {
                            "type": "complete",
                            "source": "owner",
                            "completed_via": "web",
                        },
                    },
                    csrf=True,
                ),
            )
        )
        require(completed.get("next_occurrence_task_ref"), "owner completion lost next occurrence")

        active_0914 = recurring_item(recurring_template, "2026-09-14T16:00:00Z")
        plan.enqueue_sync(
            "gate12-cursor-3", sync_payload("gate12-cursor-4", [active_0914])
        )
        plan.enqueue_completed(completed_payload([]))
        recorder.record(
            "recurrence.owner_path_upstream_dedupe",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:pull-owner-dedupe"),
                wait_for_cursor(database, user_id, "gate12-cursor-4"),
            ),
        )
        owner_path_rows = occurrence_rows(database, user_id, RECURRING_ID)
        require(len(owner_path_rows) == 3, f"owner path occurrence count was {owner_path_rows!r}")

        active_0921 = recurring_item(recurring_template, "2026-09-21T16:00:00Z")
        completed_0914 = completed_item(
            RECURRING_ID,
            "2026-09-14T16:00:00Z",
            "2026-09-14T18:00:00Z",
        )
        terminal_ship = copy.deepcopy(next(item for item in initial["items"] if item["id"] == SHIP_ID))
        terminal_ship.update(
            {
                "content": "Todoist tried to replace owner title",
                "checked": True,
                "completed_at": "2026-09-07T18:01:00Z",
            }
        )
        terminal_deleted = copy.deepcopy(
            next(item for item in initial["items"] if item["id"] == DELETED_ID)
        )
        terminal_deleted["is_deleted"] = True
        plan.enqueue_sync(
            "gate12-cursor-4",
            sync_payload(
                "gate12-cursor-5", [active_0921, terminal_ship, terminal_deleted]
            ),
        )
        plan.enqueue_completed(completed_payload([completed_0914]))
        recorder.record(
            "recurrence.todoist_completion_and_terminals",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:pull-remote-complete"),
                wait_for_cursor(database, user_id, "gate12-cursor-5"),
            ),
        )
        remote_rows = occurrence_rows(database, user_id, RECURRING_ID)
        require(len(remote_rows) == 4, f"Todoist completion did not produce four occurrences: {remote_rows!r}")
        require([row["status"] for row in remote_rows] == ["done", "done", "done", "open"], "recurrence statuses were not done/done/done/open")
        ship_after = external_task(database, user_id, SHIP_ID)
        deleted_after = external_task(database, user_id, DELETED_ID)
        require(ship_after.get("title") == "Owner-set launch title", "Todoist overwrote owner title")
        require(ship_after.get("status") == "done", "Todoist completion did not propagate")
        require(deleted_after.get("status") == "dropped", "Todoist deletion did not propagate")
        require(task_cell(deleted_after, "dropped_reason").get("value") == "todoist_deleted", "deletion reason missing")

        plan.enqueue_sync(
            "gate12-cursor-5", sync_payload("gate12-cursor-6", [active_0921])
        )
        plan.enqueue_completed(completed_payload([completed_0914]))
        recorder.record(
            "recurrence.remote_replay_dedupe",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:pull-remote-replay"),
                wait_for_cursor(database, user_id, "gate12-cursor-6"),
            ),
        )
        require(len(occurrence_rows(database, user_id, RECURRING_ID)) == 4, "recurrence replay duplicated an occurrence")

        before_failure_count = task_count(database, user_id)
        plan.enqueue_sync(
            "gate12-cursor-6",
            {},
            status=401,
        )
        recorder.record(
            "todoist.failure_fail_closed",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:pull-error"),
                wait_for_error(database, user_id, "todoist_auth_rejected"),
            ),
        )
        failed_state = sync_state(database, user_id)
        require(failed_state.get("cursor") == "gate12-cursor-6", "failed pull advanced cursor")
        require(task_count(database, user_id) == before_failure_count, "failed pull changed tasks")

        apply_canary_item = recurring_item(
            recurring_template, "2026-10-05T16:00:00Z"
        )
        apply_canary_item["id"] = apply_canary_id
        apply_canary_item["content"] = "Apply rejection log boundary"
        plan.enqueue_sync(
            "gate12-cursor-6", sync_payload("gate12-cursor-7", [apply_canary_item])
        )
        plan.enqueue_completed(completed_payload([]))
        recorder.record(
            "todoist.failure_recovery",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:pull-recovery"),
                wait_for_cursor(database, user_id, "gate12-cursor-7"),
            ),
        )

        before_apply_rejection = task_count(database, user_id)
        plan.enqueue_sync(
            "gate12-cursor-7",
            sync_payload("gate12-cursor-must-not-commit", [apply_canary_item]),
        )
        plan.enqueue_completed(
            completed_payload(
                [
                    completed_item(
                        apply_canary_id,
                        apply_canary_occurrence,
                        "2026-10-12T18:00:00Z",
                    )
                ]
            )
        )
        recorder.record(
            "todoist.apply_rejection_content_free",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:apply-rejection"),
                wait_for_error(database, user_id, "todoist_apply_rejected"),
            ),
        )
        rejected_state = sync_state(database, user_id)
        require(rejected_state.get("cursor") == "gate12-cursor-7", "apply rejection advanced cursor")
        require(task_count(database, user_id) == before_apply_rejection, "apply rejection changed tasks")
        plan.enqueue_sync("gate12-cursor-7", sync_payload("gate12-cursor-8", []))
        plan.enqueue_completed(completed_payload([]))
        recorder.record(
            "todoist.apply_rejection_recovery",
            lambda: (
                manual_pull(web, f"todoist-gate:{suffix}:apply-rejection-recovery"),
                wait_for_cursor(database, user_id, "gate12-cursor-8"),
            ),
        )

        post_recovery_status = response_data(
            web.request("GET", "/v1/workspace/integrations/todoist/status")
        )
        pull_generation = post_recovery_status.get("configuration_generation")
        require(isinstance(pull_generation, int), "pull status omitted generation")
        to_off = recorder.record(
            "generation.pull_to_off",
            lambda: configure_mode(
                web,
                f"todoist-gate:{suffix}:generation-off",
                "off",
                pull_generation,
            ),
        )
        off_generation = to_off["status"].get("configuration_generation")
        require(off_generation == pull_generation + 1, "pull→off did not advance generation")
        require(sync_state(database, user_id).get("cursor") == "gate12-cursor-8", "pull→off reset cursor")
        plan.enqueue_sync("gate12-cursor-8", sync_payload("gate12-cursor-9", []))
        plan.enqueue_completed(completed_payload([]))
        to_once = recorder.record(
            "generation.off_to_import_once",
            lambda: configure_mode(
                web,
                f"todoist-gate:{suffix}:generation-import-once",
                "import_once",
                off_generation,
            ),
        )
        once_generation = to_once["status"].get("configuration_generation")
        require(once_generation == off_generation + 1, "off→import_once did not advance generation")
        recorder.record(
            "generation.import_once_exactly_once",
            lambda: wait_for_cursor(database, user_id, "gate12-cursor-9"),
        )
        requests_after_once = plan.snapshot()["request_count"]
        no_op_pull = manual_pull(web, f"todoist-gate:{suffix}:import-once-noop")
        require(no_op_pull.get("queued") is False, "completed import_once queued twice")
        time.sleep(1)
        require(plan.snapshot()["request_count"] == requests_after_once, "completed import_once ran twice")

        fixture_before_off = plan.snapshot()["request_count"]
        worker.stop()
        api.stop()
        captured_logs += worker.text() + api.text()
        worker = None
        api = None
        off_env = {**env, "STRAYLIGHT_TODOIST_SYNC_ENABLED": "false"}
        api = start_process(args.binary, "serve", off_env)
        worker = start_process(args.binary, "worker", off_env)
        recorder.record("kill_switch.stack_ready", lambda: wait_for_http(args.base_url, api))
        off_status = response_data(
            web.request("GET", "/v1/workspace/integrations/todoist/status")
        )
        require(off_status.get("environment_enabled") is False, "kill switch status stayed enabled")
        require(off_status.get("effective_mode") == "off", "kill switch effective mode was not off")
        off_pull = recorder.record(
            "kill_switch.no_queue",
            lambda: manual_pull(web, f"todoist-gate:{suffix}:pull-disabled"),
        )
        require(off_pull.get("queued") is False, "kill switch queued a pull")
        time.sleep(6)
        require(plan.snapshot()["request_count"] == fixture_before_off, "kill switch contacted Todoist")
        off_state = sync_state(database, user_id)
        require(
            off_state.get("next_run_at") is None
            and off_state.get("manual_requested_at") is None
            and off_state.get("lease_owner") is None,
            "kill switch left durable backlog or lease state",
        )

        disabled_status = response_data(
            web.request("GET", "/v1/workspace/integrations/todoist/status")
        )
        disabled_generation = disabled_status.get("configuration_generation")
        require(disabled_generation == once_generation, "disabled restart changed generation")
        disabled_off = configure_mode(
            web,
            f"todoist-gate:{suffix}:disabled-off",
            "off",
            disabled_generation,
        )
        disabled_off_generation = disabled_off["status"].get("configuration_generation")
        disabled_once = configure_mode(
            web,
            f"todoist-gate:{suffix}:disabled-import-once",
            "import_once",
            disabled_off_generation,
        )
        disabled_once_generation = disabled_once["status"].get("configuration_generation")
        require(
            disabled_once_generation == disabled_off_generation + 1,
            "disabled import_once did not advance generation",
        )
        time.sleep(6)
        require(plan.snapshot()["request_count"] == fixture_before_off, "disabled configuration contacted Todoist")
        disabled_state = sync_state(database, user_id)
        require(
            disabled_state.get("next_run_at") is None
            and disabled_state.get("manual_requested_at") is None
            and disabled_state.get("lease_owner") is None,
            "disabled import_once accumulated backlog",
        )

        plan.enqueue_sync("gate12-cursor-9", sync_payload("gate12-cursor-10", []))
        plan.enqueue_completed(completed_payload([]))
        worker.stop()
        api.stop()
        captured_logs += worker.text() + api.text()
        worker = None
        api = None
        api = start_process(args.binary, "serve", env)
        worker = start_process(args.binary, "worker", env)
        recorder.record("generation.reenabled_stack_ready", lambda: wait_for_http(args.base_url, api))
        recorder.record(
            "generation.disabled_then_enabled_once",
            lambda: wait_for_cursor(database, user_id, "gate12-cursor-10"),
        )
        reenabled_requests = plan.snapshot()["request_count"]
        reenabled_noop = manual_pull(web, f"todoist-gate:{suffix}:reenabled-noop")
        require(reenabled_noop.get("queued") is False, "reenabled import_once queued twice")
        time.sleep(1)
        require(plan.snapshot()["request_count"] == reenabled_requests, "reenabled import_once ran twice")

        export_files = recorder.record(
            "canary.workspace_export_scan",
            lambda: scan_export(args.carrystate, args.base_url, owner_token, canary),
            lambda count: {"files_scanned": count},
        )
        require(export_files > 0, "workspace export contained no files")
        dump_hash = recorder.record(
            "canary.database_plaintext_scan",
            lambda: scan_database(database, canary),
            lambda digest: {"dump_sha256": digest},
        )
        require(bool(dump_hash), "database scan omitted digest")
        object_scan = recorder.record(
            "canary.object_store_scan",
            lambda: scan_object_store(args.binary, env, canary),
            lambda value: {"files_scanned": value[0], "aggregate_sha256": value[1]},
        )
        require(object_scan[0] > 0, "object-store backup contained no files")
        logs = captured_logs + (api.text() if api else "") + (worker.text() if worker else "")
        require(canary not in logs, "vault canary appeared in API/worker logs")
        require(
            apply_canary_id not in logs and apply_canary_occurrence not in logs,
            "apply rejection leaked external identity into API/worker logs",
        )
        recorder.checks.append(Check("canary.log_scan", 0, {"bytes_scanned": len(logs)}))
        fixture_state = plan.snapshot()
        require(not fixture_state["errors"], f"fixture protocol errors: {fixture_state['errors']!r}")
        require(fixture_state["queued_sync"] == 0, "unused Sync fixture responses remain")
        require(fixture_state["queued_completed"] == 0, "unused completion fixture responses remain")
        recorder.checks.append(Check("fixture.read_only_protocol", 0, fixture_state))
        secondary_state = sync_state(database, secondary_id)
        require(
            secondary_state.get("cursor") is None
            and secondary_state.get("configuration_generation") == 1
            and task_count(database, secondary_id) == 0,
            "tenant B state changed during tenant A Todoist scenario",
        )
        recorder.checks.append(Check("tenant_b.unchanged", 0, {}))

        return {
            "schema": SCHEMA,
            "status": "pass",
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
            "checks": [
                {"name": check.name, "elapsed_ms": check.elapsed_ms, "detail": check.detail}
                for check in recorder.checks
            ],
            "summary": {
                "initial_tasks": 4,
                "recurring_occurrences": 4,
                "final_cursor": "gate12-cursor-10",
                "fixture_requests": fixture_state["request_count"],
                "kill_switch": "suppressed_without_backlog_then_one_run_on_enable",
                "canary": "absent_from_scanned_sinks",
            },
        }
    finally:
        if worker is not None:
            worker.stop()
        if api is not None:
            api.stop()
        fixture.stop()
        owner_token = ""
        canary = ""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    root = Path(__file__).resolve().parents[1]
    parser.add_argument("--repo", type=Path, default=root)
    parser.add_argument(
        "--binary", type=Path, default=root / "apps/api/target/debug/straylight"
    )
    parser.add_argument(
        "--carrystate", type=Path, default=root / "apps/api/target/debug/carrystate"
    )
    parser.add_argument(
        "--full-fixture",
        type=Path,
        default=root / "apps/api/tests/fixtures/todoist/v1/full_sync.json",
    )
    parser.add_argument("--database-container", default="straylight-task-todoist-db")
    parser.add_argument("--database-port", type=int, default=15111)
    parser.add_argument("--database-name", default="straylight")
    parser.add_argument("--database-user", default="admin")
    parser.add_argument("--minio-init-container", default="straylight-task-m2-minio-init-1")
    parser.add_argument("--minio-port", type=int, default=19112)
    parser.add_argument("--api-port", type=int, default=18111)
    parser.add_argument("--artifact-dir", type=Path, default=root / "release-artifacts/task-gate12")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args.repo = args.repo.resolve()
    args.binary = args.binary.resolve()
    args.carrystate = args.carrystate.resolve()
    args.full_fixture = args.full_fixture.resolve()
    args.artifact_dir = args.artifact_dir.resolve()
    args.base_url = f"http://127.0.0.1:{args.api_port}"
    evidence: dict[str, Any]
    try:
        evidence = run(args)
    except Exception as error:  # the artifact is required even on a red run
        evidence = {
            "schema": SCHEMA,
            "status": "fail",
            "elapsed_ms": 0,
            "checks": [],
            "error": f"{type(error).__name__}: {error}",
        }
    write_evidence(args.artifact_dir, evidence)
    print(json.dumps(evidence, indent=2, sort_keys=True))
    return 0 if evidence.get("status") == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())

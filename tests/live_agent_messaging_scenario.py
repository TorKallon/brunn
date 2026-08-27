#!/usr/bin/env python3
"""Real-interface Gate 12a/12b harness for agent messaging.

The normal mode talks to the Straylight HTTP API and to two fresh stdio MCP
adapter processes. Credentials are supplied only by references (``env:NAME``
or ``file:/path``), are redacted from diagnostics, and are never accepted as
literal command-line values. Every run uses unique conversation data and
soft-closes the conversations it creates.

``--preflight`` is intentionally static so the initial red test does not need
or disturb a shared stack. It requires the gated HTTP route registration and
all five MCP tool registrations to exist in this worktree.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import selectors
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence


DEFAULT_BASE_URL = "http://127.0.0.1:18112"
REQUIRED_MCP_TOOLS = {
    "agent.list",
    "message.list",
    "message.read",
    "message.send",
    "message.wait",
}
TOKEN_PATTERN = re.compile(r"\b(?:sl|seval)_[A-Za-z0-9_-]{12,}\b")
BEARER_PATTERN = re.compile(r"(?i)(bearer\s+)[^\s,;\"']+")
CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
AGENT_ID_PATTERN = re.compile(r"[a-z0-9]+(?:[._-][a-z0-9]+)*")
DOCKER_NAME_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}")
POSTGRES_NAME_PATTERN = re.compile(r"[A-Za-z_][A-Za-z0-9_]{0,62}")


class ScenarioFailure(AssertionError):
    """A contract failure whose text is safe to show after sanitization."""


class Sanitizer:
    def __init__(self) -> None:
        self._secrets: set[str] = set()

    def register(self, value: str | None) -> None:
        if value:
            self._secrets.add(value)

    def text(self, value: Any) -> str:
        rendered = str(value)
        for secret in sorted(self._secrets, key=len, reverse=True):
            rendered = rendered.replace(secret, "<redacted>")
        rendered = BEARER_PATTERN.sub(r"\1<redacted>", rendered)
        return TOKEN_PATTERN.sub("<redacted>", rendered)


def credential_from_reference(reference: str | None, label: str, sanitizer: Sanitizer) -> str:
    if not reference:
        raise ScenarioFailure(f"missing {label} credential reference")
    if reference.startswith("env:"):
        name = reference[4:]
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            raise ScenarioFailure(f"invalid {label} env credential reference")
        value = os.environ.get(name, "")
    elif reference.startswith("file:"):
        path = Path(reference[5:]).expanduser()
        try:
            value = path.read_text(encoding="utf-8").strip()
        except OSError as error:
            raise ScenarioFailure(f"could not read {label} credential file: {error}") from error
    else:
        raise ScenarioFailure(f"{label} credential must use env:NAME or file:/path")
    if not value or "\n" in value or "\r" in value:
        raise ScenarioFailure(f"{label} credential reference resolved to invalid content")
    sanitizer.register(value)
    return value


def ulid() -> str:
    value = int.from_bytes(secrets.token_bytes(16), "big")
    encoded = ["0"] * 26
    for index in range(25, -1, -1):
        encoded[index] = CROCKFORD[value & 31]
        value >>= 5
    return "".join(encoded)


def find_field(value: Any, name: str) -> Any | None:
    if isinstance(value, dict):
        if name in value:
            return value[name]
        for child in value.values():
            found = find_field(child, name)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_field(child, name)
            if found is not None:
                return found
    return None


def require_field(value: Any, name: str, expected_type: type) -> Any:
    found = find_field(value, name)
    if not isinstance(found, expected_type):
        raise ScenarioFailure(f"response is missing typed {name}")
    return found


def response_data(value: Any) -> Mapping[str, Any]:
    if not isinstance(value, dict):
        raise ScenarioFailure("response is not a JSON object")
    data = value.get("data", value)
    if not isinstance(data, dict):
        raise ScenarioFailure("response data is not a JSON object")
    return data


def response_array(value: Any, name: str) -> list[Mapping[str, Any]]:
    found = response_data(value).get(name)
    if not isinstance(found, list) or not all(isinstance(item, dict) for item in found):
        raise ScenarioFailure(f"response is missing typed {name} array")
    return found


def canonical_uuid(value: str, label: str) -> str:
    try:
        parsed = uuid.UUID(value)
    except (ValueError, AttributeError) as error:
        raise ScenarioFailure(f"{label} is not a UUID") from error
    if str(parsed) != value or parsed.version != 7:
        raise ScenarioFailure(f"{label} is not a canonical lowercase UUIDv7")
    return value


def require_agent_id(value: str, label: str) -> str:
    if not AGENT_ID_PATTERN.fullmatch(value) or len(value) > 80:
        raise ScenarioFailure(f"{label} is not a canonical messaging principal id")
    return value


@dataclass(frozen=True)
class HttpResult:
    status: int
    body: Any


class HttpClient:
    def __init__(self, base_url: str, token: str, sanitizer: Sanitizer, timeout: float) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.sanitizer = sanitizer
        self.timeout = timeout

    def request(
        self,
        method: str,
        path: str,
        body: Any | None = None,
        *,
        expected_status: int | None = None,
    ) -> HttpResult:
        headers = {"accept": "application/json", "user-agent": "straylight-messaging-gate12/1"}
        headers["authorization"] = f"Bearer {self.token}"
        data = None
        if body is not None:
            data = json.dumps(body, separators=(",", ":")).encode("utf-8")
            headers["content-type"] = "application/json"
        request = urllib.request.Request(
            self.base_url + path,
            data=data,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        except (urllib.error.URLError, TimeoutError) as error:
            raise ScenarioFailure(f"{method} {path} could not reach API: {error}") from error
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            parsed = {"unparseable_response": raw.decode("utf-8", errors="replace")[:500]}
        expected = 200 <= status < 300 if expected_status is None else status == expected_status
        if not expected:
            detail = self.sanitizer.text(json.dumps(parsed, sort_keys=True))
            expectation = "a success" if expected_status is None else f"HTTP {expected_status}"
            raise ScenarioFailure(
                f"{method} {path} returned HTTP {status}, expected {expectation}: {detail}"
            )
        return HttpResult(status=status, body=parsed)

    def get(self, path: str) -> Any:
        return self.request("GET", path).body

    def post(self, path: str, body: Any) -> Any:
        return self.request("POST", path, body).body

    def post_error(self, path: str, body: Any, status: int) -> Any:
        return self.request("POST", path, body, expected_status=status).body


class McpClient:
    def __init__(
        self,
        command: Sequence[str],
        cwd: Path,
        base_url: str,
        token: str,
        sanitizer: Sanitizer,
        timeout: float,
    ) -> None:
        environment = {
            key: value
            for key, value in os.environ.items()
            if key in {"HOME", "LANG", "LC_ALL", "PATH", "TMPDIR"}
        }
        environment.update(
            {
                "STRAYLIGHT_API_TOKEN": token,
                "STRAYLIGHT_API_URL": base_url,
                "STRAYLIGHT_MESSAGING_ENABLED": "true",
                "STRAYLIGHT_MCP_RETRY_BACKOFF_MS": "1,1,1,1,1,1",
            }
        )
        try:
            self.process = subprocess.Popen(
                list(command),
                cwd=cwd,
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                bufsize=1,
            )
        except OSError as error:
            raise ScenarioFailure(f"could not launch MCP command: {error}") from error
        self.sanitizer = sanitizer
        self.timeout = timeout
        self.next_id = 1
        self.request(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "straylight-gate12", "version": "1"},
            },
        )
        self.notify("notifications/initialized", {})

    def _write(self, message: Mapping[str, Any]) -> None:
        if self.process.stdin is None:
            raise ScenarioFailure("MCP stdin is unavailable")
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def request(self, method: str, params: Mapping[str, Any]) -> Any:
        request_id = self.next_id
        self.next_id += 1
        self._write({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        if self.process.stdout is None:
            raise ScenarioFailure("MCP stdout is unavailable")
        selector = selectors.DefaultSelector()
        selector.register(self.process.stdout, selectors.EVENT_READ)
        try:
            while selector.select(self.timeout):
                line = self.process.stdout.readline()
                if not line:
                    break
                try:
                    message = json.loads(line)
                except json.JSONDecodeError as error:
                    raise ScenarioFailure("MCP wrote non-JSON protocol output") from error
                if message.get("id") != request_id:
                    continue
                if "error" in message:
                    raise ScenarioFailure(
                        f"MCP {method} failed: {self.sanitizer.text(json.dumps(message['error']))}"
                    )
                return message.get("result", {})
        finally:
            selector.close()
        raise ScenarioFailure(f"MCP {method} did not return a response")

    def notify(self, method: str, params: Mapping[str, Any]) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def list_tools(self) -> set[str]:
        result = self.request("tools/list", {})
        tools = result.get("tools") if isinstance(result, dict) else None
        if not isinstance(tools, list):
            raise ScenarioFailure("MCP tools/list returned no tools array")
        return {str(tool.get("name")) for tool in tools if isinstance(tool, dict)}

    def call_result(self, name: str, arguments: Mapping[str, Any]) -> tuple[bool, Any]:
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if not isinstance(result, dict):
            raise ScenarioFailure(f"MCP {name} returned an invalid result")
        content = result.get("content")
        if not isinstance(content, list) or not content or not isinstance(content[0], dict):
            raise ScenarioFailure(f"MCP {name} returned no content")
        text = content[0].get("text")
        if not isinstance(text, str):
            raise ScenarioFailure(f"MCP {name} returned non-text content")
        try:
            return result.get("isError") is True, json.loads(text)
        except json.JSONDecodeError as error:
            raise ScenarioFailure(f"MCP {name} returned non-JSON content") from error

    def call(self, name: str, arguments: Mapping[str, Any]) -> Any:
        is_error, body = self.call_result(name, arguments)
        if is_error:
            detail = self.sanitizer.text(json.dumps(body, sort_keys=True))
            raise ScenarioFailure(f"MCP {name} returned a tool error: {detail}")
        return body

    def call_error(self, name: str, arguments: Mapping[str, Any], code: str) -> Any:
        is_error, body = self.call_result(name, arguments)
        if not is_error:
            raise ScenarioFailure(f"MCP {name} unexpectedly succeeded")
        if find_field(body, "code") != code:
            detail = self.sanitizer.text(json.dumps(body, sort_keys=True))
            raise ScenarioFailure(f"MCP {name} returned the wrong error: {detail}")
        return body

    def close(self) -> None:
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=2)


def validate_database_target(args: argparse.Namespace, sanitizer: Sanitizer) -> None:
    for value, pattern, label in (
        (args.database_container, DOCKER_NAME_PATTERN, "database container"),
        (args.database_compose_project, DOCKER_NAME_PATTERN, "database Compose project"),
        (args.database_user, POSTGRES_NAME_PATTERN, "database user"),
        (args.database_name, POSTGRES_NAME_PATTERN, "database name"),
    ):
        if not pattern.fullmatch(value):
            raise ScenarioFailure(f"invalid {label}")
    completed = subprocess.run(
        [
            "docker",
            "inspect",
            "--format",
            "{{.State.Running}}\t{{index .Config.Labels \"com.docker.compose.project\"}}\t"
            "{{index .Config.Labels \"com.docker.compose.service\"}}",
            args.database_container,
        ],
        text=True,
        capture_output=True,
        check=False,
        timeout=args.timeout,
    )
    if completed.returncode != 0:
        detail = sanitizer.text(completed.stderr[-500:])
        raise ScenarioFailure(f"could not inspect disposable database container: {detail}")
    fields = completed.stdout.strip().split("\t")
    if fields != ["true", args.database_compose_project, "db"]:
        raise ScenarioFailure(
            "database target is not the running db service in the explicitly selected "
            "disposable Compose project"
        )
    current = docker_psql(args, "SELECT current_database()", sanitizer)
    if current != args.database_name:
        raise ScenarioFailure("database target did not confirm the selected disposable database")


def docker_psql(args: argparse.Namespace, sql: str, sanitizer: Sanitizer) -> str:
    completed = subprocess.run(
        [
            "docker",
            "exec",
            args.database_container,
            "psql",
            "-X",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            args.database_user,
            "-d",
            args.database_name,
            "-At",
            "-c",
            sql,
        ],
        text=True,
        capture_output=True,
        check=False,
        timeout=args.timeout,
    )
    if completed.returncode != 0:
        detail = sanitizer.text(completed.stderr[-1000:])
        raise ScenarioFailure(f"disposable database contract query failed: {detail}")
    return completed.stdout.strip()


class EchoProcess:
    def __init__(self, args: argparse.Namespace, sanitizer: Sanitizer) -> None:
        script = args.source_root / "scripts" / "agent_messaging_echo.py"
        if not script.is_file():
            raise ScenarioFailure("echo resident script is missing")
        self._temporary = tempfile.TemporaryDirectory(prefix="straylight-gate12-echo-")
        environment = {
            key: value
            for key, value in os.environ.items()
            if key in {"HOME", "LANG", "LC_ALL", "PATH", "TMPDIR"}
        }
        if args.resident_credential_ref.startswith("env:"):
            variable = args.resident_credential_ref[4:]
            environment[variable] = os.environ[variable]
        try:
            self.process = subprocess.Popen(
                [
                    sys.executable,
                    str(script),
                    "--base-url",
                    args.base_url,
                    "--credential-ref",
                    args.resident_credential_ref,
                    "--state-file",
                    str(Path(self._temporary.name) / "state.json"),
                    "--request-timeout",
                    str(args.timeout),
                ],
                cwd=args.source_root,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
            )
        except OSError as error:
            self._temporary.cleanup()
            raise ScenarioFailure(f"could not launch echo resident: {error}") from error
        if self.process.stderr is None:
            self.close()
            raise ScenarioFailure("echo resident stderr is unavailable")
        selector = selectors.DefaultSelector()
        selector.register(self.process.stderr, selectors.EVENT_READ)
        try:
            events = selector.select(min(args.timeout, 5.0))
            line = self.process.stderr.readline() if events else ""
        finally:
            selector.close()
        if "[echo] resident started" not in line:
            detail = sanitizer.text(line.strip() or f"exit {self.process.poll()}")
            self.close()
            raise ScenarioFailure(f"echo resident did not start: {detail}")

    def assert_running(self, sanitizer: Sanitizer) -> None:
        code = self.process.poll()
        if code is None:
            return
        detail = ""
        if self.process.stderr is not None:
            detail = sanitizer.text(self.process.stderr.read()[-1000:])
        raise ScenarioFailure(f"echo resident exited with {code}: {detail}")

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=2)
        if self.process.stderr is not None:
            self.process.stderr.close()
        self._temporary.cleanup()


class MessagingScenario:
    def __init__(self, args: argparse.Namespace, sanitizer: Sanitizer) -> None:
        self.args = args
        self.sanitizer = sanitizer
        self.owner = HttpClient(args.base_url, args.owner_token, sanitizer, args.timeout)
        self.resident = HttpClient(args.base_url, args.resident_token, sanitizer, args.timeout)
        self.created_conversations: list[str] = []

    def run(self) -> None:
        self.scenario_12a_mcp_agent_to_agent()
        self.scenario_12b_owner_resident_http()

    def scenario_12a_mcp_agent_to_agent(self) -> None:
        """Full MCP exchange budget, owner resume, and worker expiry contract."""
        command = shlex.split(self.args.mcp_command)
        if not command:
            raise ScenarioFailure("MCP command is empty")
        agent_a = McpClient(
            command, self.args.source_root, self.args.base_url, self.args.agent_a_token,
            self.sanitizer, self.args.timeout,
        )
        agent_b = McpClient(
            command, self.args.source_root, self.args.base_url, self.args.agent_b_token,
            self.sanitizer, self.args.timeout,
        )
        try:
            missing = REQUIRED_MCP_TOOLS - agent_b.list_tools()
            if missing:
                raise ScenarioFailure(f"gate-on MCP is missing tools: {', '.join(sorted(missing))}")
            agent_b.call("agent.list", {})
            question_key = ulid()
            question = agent_b.call(
                "message.send",
                {
                    "to": self.args.agent_a_id,
                    "client_key": question_key,
                    "kind": "question",
                    "body_md": "Gate 12a: can you acknowledge this deadline question?",
                    "expects_reply": True,
                    "reply_by": (
                        datetime.now(timezone.utc) + timedelta(minutes=5)
                    ).isoformat().replace("+00:00", "Z"),
                },
            )
            conversation_id = require_field(question, "conversation_id", str)
            canonical_uuid(conversation_id, "Gate 12a conversation id")
            question_seq = require_field(question, "seq", int)
            self.created_conversations.append(conversation_id)
            received = agent_a.call(
                "message.wait",
                {"conversation_id": conversation_id, "after_seq": 0, "timeout_s": 2},
            )
            if not any(
                message.get("seq") == question_seq
                for message in response_array(received, "messages")
            ):
                raise ScenarioFailure("Gate 12a resident wait did not receive the question")
            reply = agent_a.call(
                "message.send",
                {
                    "conversation_id": conversation_id,
                    "client_key": ulid(),
                    "body_md": "Gate 12a acknowledgement.",
                    "in_reply_to": question_seq,
                },
            )
            reply_seq = require_field(reply, "seq", int)
            observed = agent_b.call(
                "message.wait",
                {
                    "conversation_id": conversation_id,
                    "after_seq": question_seq,
                    "timeout_s": 2,
                },
            )
            if not any(
                message.get("seq") == reply_seq
                for message in response_array(observed, "messages")
            ):
                raise ScenarioFailure("Gate 12a reply was not observed after the question sequence")

            before_pause_notifications = self.notification_items()
            last_agent_seq = reply_seq
            for logical_number in range(3, 21):
                sender = agent_b if logical_number % 2 else agent_a
                sent = sender.call(
                    "message.send",
                    {
                        "conversation_id": conversation_id,
                        "client_key": ulid(),
                        "body_md": f"Gate 12a exchange message {logical_number}.",
                    },
                )
                last_agent_seq = require_field(sent, "seq", int)

            history = agent_b.call(
                "message.list",
                {"conversation_id": conversation_id, "after_seq": 0, "limit": 100},
            )
            messages = response_array(history, "messages")
            agent_messages = [message for message in messages if message.get("kind") != "system"]
            systems = [message for message in messages if message.get("kind") == "system"]
            if len(agent_messages) != 20 or len(systems) != 1:
                raise ScenarioFailure(
                    "Gate 12a did not pause with exactly 20 agent messages and one system message"
                )
            pause_system_seq = require_field(systems[0], "seq", int)
            if pause_system_seq != last_agent_seq + 1:
                raise ScenarioFailure("Gate 12a pause system message was not gapless")
            conversations = response_array(history, "conversations")
            matching = [
                item for item in conversations if item.get("conversation_id") == conversation_id
            ]
            if len(matching) != 1 or matching[0].get("status") != "paused_for_human":
                raise ScenarioFailure("Gate 12a conversation was not paused at the exact budget")
            if matching[0].get("needs_human") is not True:
                raise ScenarioFailure("Gate 12a pause did not surface needs_human")
            self.assert_one_new_notification(
                before_pause_notifications,
                conversation_id,
                pause_system_seq,
                ["Gate 12a exchange message", "Agent exchange paused"],
            )
            self.assert_notification_event_count(
                f"needs-human:{conversation_id}:{pause_system_seq}", conversation_id, 1
            )
            agent_a.call_error(
                "message.send",
                {
                    "conversation_id": conversation_id,
                    "client_key": ulid(),
                    "body_md": "Gate 12a blocked send must not commit.",
                },
                "conversation_paused",
            )

            route = (
                "/v1/workspace/messaging/conversations/"
                f"{urllib.parse.quote(conversation_id)}/messages"
            )
            owner_resume = self.owner.post(
                route,
                {
                    "client_key": ulid(),
                    "kind": "text",
                    "body_md": "Gate 12a owner response resumes this conversation.",
                },
            )
            owner_seq = require_field(owner_resume, "seq", int)
            resumed = agent_a.call(
                "message.send",
                {
                    "conversation_id": conversation_id,
                    "client_key": ulid(),
                    "body_md": "Gate 12a agent send after owner resume.",
                    "in_reply_to": owner_seq,
                },
            )
            if require_field(resumed, "seq", int) != owner_seq + 1:
                raise ScenarioFailure("Gate 12a owner message did not resume gapless agent sends")

            deadline = agent_b.call(
                "message.send",
                {
                    "conversation_id": conversation_id,
                    "client_key": ulid(),
                    "kind": "question",
                    "body_md": "Gate 12a unanswered worker deadline.",
                    "expects_reply": True,
                    "reply_by": (
                        datetime.now(timezone.utc) + timedelta(seconds=2)
                    ).isoformat().replace("+00:00", "Z"),
                },
            )
            deadline_seq = require_field(deadline, "seq", int)
            before_expiry_notifications = self.notification_items()
            expiry_system = self.wait_for_worker_system(agent_b, conversation_id, deadline_seq)
            expiry_system_seq = require_field(expiry_system, "seq", int)
            if expiry_system_seq != deadline_seq + 1:
                raise ScenarioFailure("Gate 12a reply_by system message was not gapless")
            no_second = agent_b.call(
                "message.wait",
                {
                    "conversation_id": conversation_id,
                    "after_seq": expiry_system_seq,
                    "timeout_s": 2,
                },
            )
            if response_array(no_second, "messages"):
                raise ScenarioFailure("Gate 12a reply_by worker emitted more than one message")
            post_expiry = agent_b.call(
                "message.list",
                {
                    "conversation_id": conversation_id,
                    "after_seq": deadline_seq,
                    "limit": 10,
                },
            )
            expired_systems = [
                item
                for item in response_array(post_expiry, "messages")
                if item.get("kind") == "system"
            ]
            if len(expired_systems) != 1:
                raise ScenarioFailure("Gate 12a reply_by worker did not remain exactly-once")
            self.assert_notification_event_count(
                f"reply-by:{conversation_id}:{deadline_seq}", conversation_id, 1
            )
            self.assert_one_new_notification(
                before_expiry_notifications,
                conversation_id,
                expiry_system_seq,
                ["Gate 12a unanswered worker deadline", "reply window expired"],
            )
            agent_b.call(
                "message.read",
                {"conversation_id": conversation_id, "last_read_seq": expiry_system_seq},
            )
            print(
                "[messaging-scenario] 12a MCP exact-20 pause/owner resume/worker expiry passed"
            )
        finally:
            agent_a.close()
            agent_b.close()

    def scenario_12b_owner_resident_http(self) -> None:
        """HTTP replay/delta plus the real echo process and lease time travel."""
        echo = EchoProcess(self.args, self.sanitizer)
        try:
            created = self.owner.post(
                "/v1/workspace/messaging/conversations",
                {
                    "participants": [self.args.resident_id],
                    "subject": f"Gate 12b {ulid()}",
                },
            )
            conversation_id = require_field(created, "conversation_id", str)
            canonical_uuid(conversation_id, "Gate 12b conversation id")
            self.created_conversations.append(conversation_id)
            baseline = self.owner.get("/v1/workspace/messaging/sync?cursor=0&wait=0")
            cursor = require_field(baseline, "cursor", int)
            client_key = ulid()
            payload = {
                "client_key": client_key,
                "body_md": "Gate 12b echo request.",
                "kind": "text",
            }
            route = (
                "/v1/workspace/messaging/conversations/"
                f"{urllib.parse.quote(conversation_id)}/messages"
            )
            first = self.owner.post(route, payload)
            replay = self.owner.post(route, payload)
            first_seq = require_field(first, "seq", int)
            if (
                require_field(replay, "seq", int) != first_seq
                or find_field(replay, "duplicate") is not True
            ):
                raise ScenarioFailure("Gate 12b duplicate client key did not replay one message")
            delta = self.owner.get(f"/v1/workspace/messaging/sync?cursor={cursor}&wait=0")
            next_cursor = require_field(delta, "cursor", int)
            messages = response_array(delta, "messages")
            if sum(1 for message in messages if message.get("client_key") == client_key) != 1:
                raise ScenarioFailure(
                    "Gate 12b cursor delta did not contain exactly one idempotent send"
                )
            echoed = self.wait_for_http_echo(
                echo, conversation_id, first_seq, next_cursor
            )
            if echoed.get("body_md") != payload["body_md"]:
                raise ScenarioFailure("Gate 12b real echo resident did not echo the text")
            online = self.agent_view(self.args.resident_id)
            if online.get("online") is not True or not isinstance(
                online.get("last_seen_at"), str
            ):
                raise ScenarioFailure("Gate 12b resident wait did not renew its online lease")
            last_seen_at = online["last_seen_at"]
        finally:
            echo.close()

        self.expire_presence(conversation_id, self.args.resident_id)
        offline = self.agent_view(self.args.resident_id)
        if offline.get("online") is not False or offline.get("last_seen_at") != last_seen_at:
            raise ScenarioFailure("Gate 12b injected lease expiry did not preserve last-seen state")
        print(
            "[messaging-scenario] 12b HTTP replay/delta/real echo/presence expiry passed"
        )

    def wait_for_worker_system(
        self, agent: McpClient, conversation_id: str, after_seq: int
    ) -> Mapping[str, Any]:
        deadline = time.monotonic() + self.args.worker_observation_timeout
        cursor = after_seq
        while time.monotonic() < deadline:
            page = agent.call(
                "message.wait",
                {
                    "conversation_id": conversation_id,
                    "after_seq": cursor,
                    "timeout_s": 2,
                },
            )
            messages = response_array(page, "messages")
            for message in messages:
                seq = message.get("seq")
                if isinstance(seq, int):
                    cursor = max(cursor, seq)
                if message.get("kind") == "system":
                    return message
        raise ScenarioFailure(
            "Gate 12a existing worker did not expose the due reply_by system message over MCP"
        )

    def wait_for_http_echo(
        self,
        echo: EchoProcess,
        conversation_id: str,
        in_reply_to: int,
        cursor: int,
    ) -> Mapping[str, Any]:
        deadline = time.monotonic() + self.args.worker_observation_timeout
        while time.monotonic() < deadline:
            echo.assert_running(self.sanitizer)
            page = self.owner.get(
                "/v1/workspace/messaging/sync?"
                + urllib.parse.urlencode({"cursor": cursor, "wait": 2})
            )
            cursor = require_field(page, "cursor", int)
            for message in response_array(page, "messages"):
                if (
                    message.get("conversation_id") == conversation_id
                    and message.get("from_agent_id") == self.args.resident_id
                    and message.get("in_reply_to") == in_reply_to
                ):
                    return message
        raise ScenarioFailure("Gate 12b real echo resident did not reply before the timeout")

    def notification_items(self) -> list[Mapping[str, Any]]:
        payload = self.owner.get("/v1/workspace/notifications?limit=100")
        items = payload.get("items") if isinstance(payload, dict) else None
        if not isinstance(items, list) or not all(isinstance(item, dict) for item in items):
            raise ScenarioFailure("notification API returned no typed items array")
        return items

    def assert_one_new_notification(
        self,
        before: Sequence[Mapping[str, Any]],
        conversation_id: str,
        seq: int,
        forbidden_content: Sequence[str],
    ) -> None:
        known = {item.get("notification_ref") for item in before}
        added = [
            item for item in self.notification_items() if item.get("notification_ref") not in known
        ]
        matching = [
            item
            for item in added
            if isinstance(item.get("target"), dict)
            and item["target"].get("type") == "conversation"
            and item["target"].get("conversation_id") == conversation_id
            and item["target"].get("seq") == seq
        ]
        if len(matching) != 1:
            raise ScenarioFailure("messaging event did not expose exactly one typed notification")
        visible = f"{matching[0].get('title', '')}\n{matching[0].get('body', '')}"
        if not visible.strip() or any(marker in visible for marker in forbidden_content):
            raise ScenarioFailure("messaging notification body was not generic")

    def agent_view(self, agent_id: str) -> Mapping[str, Any]:
        agents = response_array(
            self.owner.get("/v1/workspace/messaging/agents"), "agents"
        )
        matches = [item for item in agents if item.get("agent_id") == agent_id]
        if len(matches) != 1:
            raise ScenarioFailure("agent registry did not return the expected resident")
        return matches[0]

    def expire_presence(self, conversation_id: str, agent_id: str) -> None:
        canonical_uuid(conversation_id, "presence time-travel conversation id")
        require_agent_id(agent_id, "presence time-travel principal")
        changed = docker_psql(
            self.args,
            f"""
            WITH target AS (
              SELECT participant.user_id
              FROM straylight.messaging_participants AS participant
              WHERE participant.conversation_id='{conversation_id}'::uuid
                AND participant.agent_id='{agent_id}'
            ), changed AS (
              UPDATE straylight.messaging_agents AS agent
              SET lease_expires_at=agent.last_seen_at
              FROM target
              WHERE agent.user_id=target.user_id
                AND agent.agent_id='{agent_id}'
                AND agent.last_seen_at IS NOT NULL
                AND agent.last_seen_at < clock_timestamp()
                AND agent.lease_expires_at > clock_timestamp()
              RETURNING 1
            ) SELECT count(*) FROM changed
            """,
            self.sanitizer,
        )
        if changed != "1":
            raise ScenarioFailure("presence time travel did not expire exactly one resident lease")

    def assert_notification_event_count(
        self, event_key: str, conversation_id: str, expected: int
    ) -> None:
        canonical_uuid(conversation_id, "notification count conversation id")
        if not re.fullmatch(r"(?:needs-human|reply-by):[0-9a-f-]{36}:[1-9][0-9]*", event_key):
            raise ScenarioFailure("notification event key is invalid")
        count = docker_psql(
            self.args,
            f"""
            SELECT count(*)
            FROM straylight.notifications AS notification
            WHERE notification.event_key='{event_key}'
              AND notification.user_id=(
                SELECT conversation.user_id
                FROM straylight.messaging_conversations AS conversation
                WHERE conversation.conversation_id='{conversation_id}'::uuid
              )
            """,
            self.sanitizer,
        )
        if count != str(expected):
            raise ScenarioFailure(
                f"notification event count was {count or 'missing'}, expected {expected}"
            )

    def cleanup(self) -> None:
        for conversation_id in reversed(self.created_conversations):
            path = (
                "/v1/workspace/messaging/conversations/"
                f"{urllib.parse.quote(conversation_id)}/close"
            )
            try:
                self.owner.post(path, {})
            except Exception:
                pass


def static_preflight(source_root: Path) -> None:
    mcp_source = "\n".join(
        (
            (source_root / "apps/mcp/src/index.ts").read_text(encoding="utf-8"),
            (source_root / "apps/mcp/src/messaging-tools.ts").read_text(encoding="utf-8"),
        )
    )
    api_source = (source_root / "apps/api/src/api.rs").read_text(encoding="utf-8")
    worker_source = (source_root / "apps/api/src/worker.rs").read_text(encoding="utf-8")
    echo_source = (source_root / "scripts/agent_messaging_echo.py").read_text(encoding="utf-8")
    missing_tools = {name for name in REQUIRED_MCP_TOOLS if f'"{name}"' not in mcp_source}
    missing_route = not (
        "messaging_service::router()" in api_source
        and "messaging_enabled" in api_source
    )
    failures: list[str] = []
    if missing_route:
        failures.append("HTTP route /v1/workspace/messaging")
    if missing_tools:
        failures.append("MCP tools " + ", ".join(sorted(missing_tools)))
    if "messaging_service::process_due_reply_by" not in worker_source:
        failures.append("existing worker reply_by invocation")
    if "class EchoResident" not in echo_source or "run_forever" not in echo_source:
        failures.append("real echo resident harness")
    if failures:
        raise ScenarioFailure("gate-on messaging preflight missing " + " and ".join(failures))
    print("[messaging-scenario] gate-on route/tool registrations are present")


def parser() -> argparse.ArgumentParser:
    source_root = Path(__file__).resolve().parents[1]
    result = argparse.ArgumentParser(
        description="Run the real-interface Straylight agent messaging Gate 12a/12b scenario.",
    )
    result.add_argument("--preflight", action="store_true", help="check route/tool registrations only")
    result.add_argument("--source-root", type=Path, default=source_root)
    result.add_argument(
        "--base-url",
        default=os.environ.get("STRAYLIGHT_MESSAGING_SCENARIO_BASE_URL", DEFAULT_BASE_URL),
    )
    result.add_argument(
        "--mcp-command",
        default=os.environ.get(
            "STRAYLIGHT_MESSAGING_SCENARIO_MCP_COMMAND", "node apps/mcp/dist/index.js"
        ),
    )
    result.add_argument("--agent-a-id", default=os.environ.get("STRAYLIGHT_GATE12_AGENT_A_ID", "gate12-a"))
    result.add_argument("--agent-b-id", default=os.environ.get("STRAYLIGHT_GATE12_AGENT_B_ID", "gate12-b"))
    result.add_argument("--resident-id", default=os.environ.get("STRAYLIGHT_GATE12_RESIDENT_ID", "echo"))
    result.add_argument(
        "--agent-a-credential-ref",
        default=os.environ.get("STRAYLIGHT_GATE12_AGENT_A_CREDENTIAL_REF"),
    )
    result.add_argument(
        "--agent-b-credential-ref",
        default=os.environ.get("STRAYLIGHT_GATE12_AGENT_B_CREDENTIAL_REF"),
    )
    result.add_argument(
        "--owner-credential-ref",
        default=os.environ.get("STRAYLIGHT_GATE12_OWNER_CREDENTIAL_REF"),
    )
    result.add_argument(
        "--resident-credential-ref",
        default=os.environ.get("STRAYLIGHT_GATE12_RESIDENT_CREDENTIAL_REF"),
    )
    result.add_argument("--timeout", type=float, default=10.0)
    result.add_argument("--worker-observation-timeout", type=float, default=20.0)
    result.add_argument(
        "--database-container", default="straylight_agent_messaging-db-1"
    )
    result.add_argument(
        "--database-compose-project", default="straylight_agent_messaging"
    )
    result.add_argument("--database-user", default="admin")
    result.add_argument("--database-name", default="straylight_agent_messaging")
    result.add_argument(
        "--allow-nonlocal",
        action="store_true",
        help="allow a disposable non-loopback stack; production is covered by Gate 12g instead",
    )
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    sanitizer = Sanitizer()
    try:
        args.source_root = args.source_root.expanduser().resolve()
        if args.preflight:
            static_preflight(args.source_root)
            return 0
        parsed_url = urllib.parse.urlsplit(args.base_url)
        if parsed_url.scheme not in {"http", "https"} or not parsed_url.hostname:
            raise ScenarioFailure("base URL must be an HTTP(S) origin")
        if parsed_url.hostname not in {"127.0.0.1", "localhost", "::1"} and not args.allow_nonlocal:
            raise ScenarioFailure("refusing a non-local stack without --allow-nonlocal")
        if args.timeout <= 0:
            raise ScenarioFailure("timeout must be positive")
        if args.worker_observation_timeout < 2:
            raise ScenarioFailure("worker observation timeout must be at least two seconds")
        require_agent_id(args.agent_a_id, "agent A id")
        require_agent_id(args.agent_b_id, "agent B id")
        require_agent_id(args.resident_id, "resident id")
        validate_database_target(args, sanitizer)
        args.agent_a_token = credential_from_reference(
            args.agent_a_credential_ref, "agent A", sanitizer
        )
        args.agent_b_token = credential_from_reference(
            args.agent_b_credential_ref, "agent B", sanitizer
        )
        args.owner_token = credential_from_reference(args.owner_credential_ref, "owner", sanitizer)
        args.resident_token = credential_from_reference(
            args.resident_credential_ref, "resident", sanitizer
        )
        scenario = MessagingScenario(args, sanitizer)
        try:
            scenario.run()
        finally:
            scenario.cleanup()
        return 0
    except ScenarioFailure as error:
        print(f"[messaging-scenario] FAIL: {sanitizer.text(error)}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("[messaging-scenario] INTERRUPTED", file=sys.stderr)
        return 130
    except Exception as error:
        print(
            f"[messaging-scenario] FAIL: unexpected {type(error).__name__}: "
            f"{sanitizer.text(error)}",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

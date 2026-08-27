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
import urllib.error
import urllib.parse
import urllib.request
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

    def request(self, method: str, path: str, body: Any | None = None) -> HttpResult:
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
        if not 200 <= status < 300:
            detail = self.sanitizer.text(json.dumps(parsed, sort_keys=True))
            raise ScenarioFailure(f"{method} {path} returned HTTP {status}: {detail}")
        return HttpResult(status=status, body=parsed)

    def get(self, path: str) -> Any:
        return self.request("GET", path).body

    def post(self, path: str, body: Any) -> Any:
        return self.request("POST", path, body).body


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

    def call(self, name: str, arguments: Mapping[str, Any]) -> Any:
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if not isinstance(result, dict):
            raise ScenarioFailure(f"MCP {name} returned an invalid result")
        if result.get("isError") is True:
            raise ScenarioFailure(f"MCP {name} returned a tool error")
        content = result.get("content")
        if not isinstance(content, list) or not content or not isinstance(content[0], dict):
            raise ScenarioFailure(f"MCP {name} returned no content")
        text = content[0].get("text")
        if not isinstance(text, str):
            raise ScenarioFailure(f"MCP {name} returned non-text content")
        try:
            return json.loads(text)
        except json.JSONDecodeError as error:
            raise ScenarioFailure(f"MCP {name} returned non-JSON content") from error

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
        """Question/reply/wait core; guard and expiry slices extend this scenario."""
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
            question_seq = require_field(question, "seq", int)
            self.created_conversations.append(conversation_id)
            agent_a.call(
                "message.wait",
                {"conversation_id": conversation_id, "after_seq": 0, "timeout_s": 2},
            )
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
            if find_field(observed, "seq") != reply_seq:
                raise ScenarioFailure("Gate 12a reply was not observed after the question sequence")
            agent_b.call(
                "message.list",
                {"conversation_id": conversation_id, "after_seq": 0, "limit": 100},
            )
            agent_b.call(
                "message.read",
                {"conversation_id": conversation_id, "last_read_seq": reply_seq},
            )
            print("[messaging-scenario] 12a core MCP question/reply/wait passed")
        finally:
            agent_a.close()
            agent_b.close()

    def scenario_12b_owner_resident_http(self) -> None:
        """Idempotent owner send, cursor delta, resident echo, and next delta."""
        created = self.owner.post(
            "/v1/workspace/messaging/conversations",
            {
                "participants": [self.args.resident_id],
                "subject": f"Gate 12b {ulid()}",
            },
        )
        conversation_id = require_field(created, "conversation_id", str)
        self.created_conversations.append(conversation_id)
        baseline = self.owner.get("/v1/workspace/messaging/sync?cursor=0&wait=0")
        cursor = require_field(baseline, "cursor", int)
        client_key = ulid()
        payload = {
            "client_key": client_key,
            "body_md": "Gate 12b echo request.",
            "kind": "text",
        }
        route = f"/v1/workspace/messaging/conversations/{urllib.parse.quote(conversation_id)}/messages"
        first = self.owner.post(route, payload)
        replay = self.owner.post(route, payload)
        first_seq = require_field(first, "seq", int)
        if require_field(replay, "seq", int) != first_seq or find_field(replay, "duplicate") is not True:
            raise ScenarioFailure("Gate 12b duplicate client key did not replay one message")
        delta = self.owner.get(f"/v1/workspace/messaging/sync?cursor={cursor}&wait=0")
        next_cursor = require_field(delta, "cursor", int)
        messages = find_field(delta, "messages")
        if not isinstance(messages, list) or sum(
            1 for message in messages if find_field(message, "client_key") == client_key
        ) != 1:
            raise ScenarioFailure("Gate 12b cursor delta did not contain exactly one idempotent send")
        self.resident.get(
            "/v1/workspace/messaging/sync?"
            + urllib.parse.urlencode(
                {"cursor": 0, "wait": 2, "conversation_id": conversation_id, "after_seq": 0}
            )
        )
        reply = self.resident.post(
            route,
            {
                "client_key": ulid(),
                "body_md": "Gate 12b echo request.",
                "in_reply_to": first_seq,
            },
        )
        reply_seq = require_field(reply, "seq", int)
        echoed = self.owner.get(f"/v1/workspace/messaging/sync?cursor={next_cursor}&wait=2")
        if not any(
            find_field(message, "seq") == reply_seq
            for message in (find_field(echoed, "messages") or [])
            if isinstance(message, dict)
        ):
            raise ScenarioFailure("Gate 12b echo reply was not present in the next cursor delta")
        print("[messaging-scenario] 12b core HTTP replay/sync/echo passed")

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
    mcp_source = (source_root / "apps/mcp/src/index.ts").read_text(encoding="utf-8")
    api_source = (source_root / "apps/api/src/api.rs").read_text(encoding="utf-8")
    missing_tools = {name for name in REQUIRED_MCP_TOOLS if f'"{name}"' not in mcp_source}
    missing_route = '"/workspace/messaging' not in api_source
    failures: list[str] = []
    if missing_route:
        failures.append("HTTP route /v1/workspace/messaging")
    if missing_tools:
        failures.append("MCP tools " + ", ".join(sorted(missing_tools)))
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

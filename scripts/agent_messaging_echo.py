#!/usr/bin/env python3
"""Run a small durable echo resident against Straylight agent messaging.

The resident long-polls the authenticated principal's inbox, atomically saves
its cursor and pending replies, and sends each reply with one stable ULID. A
transient or otherwise ambiguous send retries the identical body and key.
Credentials are accepted only by ``env:NAME`` or ``file:/path`` reference.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence


STATE_SCHEMA = "straylight.echo-resident-state.v1"
SYNC_WAIT_SECONDS = 25
MAX_RESPONSE_BYTES = 4 * 1024 * 1024
DEFAULT_REQUEST_TIMEOUT_SECONDS = 35.0
DEFAULT_RETRY_BACKOFF_SECONDS = (0.25, 1.0, 2.0, 4.0)
OUTER_RETRY_SECONDS = 2.0
TRANSIENT_STATUSES = {502, 503, 504}
CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
TOKEN_PATTERN = re.compile(r"\b(?:sl|seval)_[A-Za-z0-9_-]{12,}\b")
BEARER_PATTERN = re.compile(r"(?i)(bearer\s+)[^\s,;\"']+")


class EchoFailure(RuntimeError):
    """A fail-closed resident or protocol error safe for sanitized output."""


class TransientHttpFailure(EchoFailure):
    """A network result whose mutation outcome may be ambiguous."""


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


def credential_from_reference(reference: str | None, sanitizer: Sanitizer) -> str:
    if not reference:
        raise EchoFailure("missing credential reference")
    if reference.startswith("env:"):
        name = reference[4:]
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            raise EchoFailure("invalid env credential reference")
        value = os.environ.get(name, "")
    elif reference.startswith("file:"):
        path = Path(reference[5:]).expanduser()
        try:
            value = path.read_text(encoding="utf-8").strip()
        except OSError as error:
            raise EchoFailure(f"could not read credential file: {error}") from error
    else:
        raise EchoFailure("credential must use env:NAME or file:/path")
    if not value or "\n" in value or "\r" in value:
        raise EchoFailure("credential reference resolved to invalid content")
    sanitizer.register(value)
    return value


def normalize_base_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if (
        parsed.scheme not in {"http", "https"}
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise EchoFailure("base URL must be an HTTP(S) origin without credentials or query data")
    path = parsed.path.rstrip("/")
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, path, "", ""))


def new_ulid() -> str:
    timestamp_ms = int(time.time() * 1_000) & ((1 << 48) - 1)
    value = (timestamp_ms << 80) | int.from_bytes(secrets.token_bytes(10), "big")
    encoded = ["0"] * 26
    for index in range(25, -1, -1):
        encoded[index] = CROCKFORD[value & 31]
        value >>= 5
    return "".join(encoded)


@dataclass(frozen=True)
class PendingReply:
    conversation_id: str
    in_reply_to: int
    client_key: str
    body_md: str

    def as_json(self) -> dict[str, Any]:
        return {
            "conversation_id": self.conversation_id,
            "in_reply_to": self.in_reply_to,
            "client_key": self.client_key,
            "body_md": self.body_md,
        }

    @classmethod
    def from_json(cls, value: Any) -> "PendingReply":
        if not isinstance(value, dict) or set(value) != {
            "conversation_id",
            "in_reply_to",
            "client_key",
            "body_md",
        }:
            raise EchoFailure("cursor state contains an invalid pending reply")
        conversation_id = value.get("conversation_id")
        in_reply_to = value.get("in_reply_to")
        client_key = value.get("client_key")
        body_md = value.get("body_md")
        if (
            not isinstance(conversation_id, str)
            or not conversation_id
            or not isinstance(in_reply_to, int)
            or isinstance(in_reply_to, bool)
            or in_reply_to <= 0
            or not isinstance(client_key, str)
            or not re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", client_key)
            or not isinstance(body_md, str)
        ):
            raise EchoFailure("cursor state contains malformed pending reply fields")
        return cls(conversation_id, in_reply_to, client_key, body_md)


@dataclass(frozen=True)
class EchoState:
    cursor: int
    pending: tuple[PendingReply, ...]
    principal_id: str | None = None


class StateStore:
    def __init__(self, path: Path) -> None:
        self.path = path

    def load(self) -> EchoState:
        if not self.path.exists():
            return EchoState(cursor=0, pending=(), principal_id=None)
        try:
            value = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise EchoFailure("cursor state file is unreadable or invalid") from error
        if not isinstance(value, dict) or set(value) != {
            "schema",
            "cursor",
            "pending",
            "principal_id",
        }:
            raise EchoFailure("cursor state file has an invalid shape")
        if value.get("schema") != STATE_SCHEMA:
            raise EchoFailure("cursor state file has an unsupported schema")
        cursor = value.get("cursor")
        pending = value.get("pending")
        principal_id = value.get("principal_id")
        if (
            not isinstance(cursor, int)
            or isinstance(cursor, bool)
            or cursor < 0
            or not isinstance(pending, list)
            or (
                principal_id is not None
                and (
                    not isinstance(principal_id, str)
                    or not re.fullmatch(r"[a-z0-9]+(?:[._-][a-z0-9]+)*", principal_id)
                    or len(principal_id) > 80
                )
            )
        ):
            raise EchoFailure("cursor state file contains invalid state")
        parsed_pending = tuple(PendingReply.from_json(item) for item in pending)
        identities = {(item.conversation_id, item.in_reply_to) for item in parsed_pending}
        if len(identities) != len(parsed_pending):
            raise EchoFailure("cursor state file contains duplicate pending replies")
        return EchoState(cursor=cursor, pending=parsed_pending, principal_id=principal_id)

    def save(self, state: EchoState) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        document = {
            "schema": STATE_SCHEMA,
            "cursor": state.cursor,
            "pending": [reply.as_json() for reply in state.pending],
            "principal_id": state.principal_id,
        }
        encoded = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode(
            "utf-8"
        )
        descriptor, temporary_name = tempfile.mkstemp(
            dir=self.path.parent,
            prefix=f".{self.path.name}.",
        )
        temporary = Path(temporary_name)
        try:
            os.fchmod(descriptor, 0o600)
            with os.fdopen(descriptor, "wb") as handle:
                descriptor = -1
                handle.write(encoded)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.path)
            os.chmod(self.path, 0o600)
            try:
                directory_descriptor = os.open(self.path.parent, os.O_RDONLY)
            except OSError:
                directory_descriptor = -1
            if directory_descriptor >= 0:
                try:
                    os.fsync(directory_descriptor)
                finally:
                    os.close(directory_descriptor)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass


class JsonHttpClient:
    def __init__(self, base_url: str, token: str, timeout: float) -> None:
        self.base_url = normalize_base_url(base_url).rstrip("/")
        self.token = token
        self.timeout = timeout

    def request(self, method: str, path: str, body: Mapping[str, Any] | None = None) -> Any:
        headers = {
            "accept": "application/json",
            "authorization": f"Bearer {self.token}",
            "user-agent": "straylight-agent-messaging-echo/1",
        }
        encoded_body = None
        if body is not None:
            encoded_body = json.dumps(body, separators=(",", ":")).encode("utf-8")
            headers["content-type"] = "application/json"
        request = urllib.request.Request(
            self.base_url + path,
            data=encoded_body,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                status = response.status
                raw = response.read(MAX_RESPONSE_BYTES + 1)
        except urllib.error.HTTPError as error:
            try:
                status = error.code
                raw = error.read(MAX_RESPONSE_BYTES + 1)
            finally:
                error.close()
        except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
            raise TransientHttpFailure("network outcome was ambiguous") from error
        if len(raw) > MAX_RESPONSE_BYTES:
            raise EchoFailure("API response exceeded the resident safety limit")
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError as error:
            if status in TRANSIENT_STATUSES:
                raise TransientHttpFailure("transient API response was not valid JSON") from error
            raise EchoFailure("API returned an invalid JSON response") from error
        if status in TRANSIENT_STATUSES:
            raise TransientHttpFailure(f"transient HTTP {status}")
        if not 200 <= status < 300:
            raise EchoFailure(f"API rejected the request with HTTP {status}")
        if not isinstance(parsed, dict):
            raise EchoFailure("API response must be a JSON object")
        return parsed


def response_data(response: Mapping[str, Any]) -> Mapping[str, Any]:
    data = response.get("data")
    return data if isinstance(data, dict) else response


def parse_sync(response: Mapping[str, Any], prior_cursor: int) -> tuple[int, list[Any], str | None]:
    data = response_data(response)
    cursor = data.get("cursor", data.get("resume_cursor"))
    if not isinstance(cursor, int) or isinstance(cursor, bool) or cursor < prior_cursor:
        raise EchoFailure("sync response contains an invalid or regressed cursor")
    messages = data.get("messages", [])
    if not isinstance(messages, list):
        raise EchoFailure("sync response messages must be an array")
    principal_id = data.get("principal_id")
    if not isinstance(principal_id, str):
        principal = data.get("principal")
        principal_id = principal.get("agent_id") if isinstance(principal, dict) else None
    return cursor, messages, principal_id if isinstance(principal_id, str) else None


def response_sender(response: Mapping[str, Any]) -> str | None:
    data = response_data(response)
    message = data.get("message")
    if not isinstance(message, dict):
        message = response.get("message")
    if not isinstance(message, dict):
        return None
    sender = message.get("from_agent_id")
    return sender if isinstance(sender, str) and sender else None


def reply_for_message(
    message: Any,
    principal_id: str | None,
    ulid_factory: Callable[[], str] = new_ulid,
) -> PendingReply | None:
    if not isinstance(message, dict):
        raise EchoFailure("sync returned a malformed message")
    kind = message.get("kind")
    if kind == "system":
        return None
    if kind not in {"text", "question"}:
        raise EchoFailure("sync returned an unsupported message kind")
    conversation_id = message.get("conversation_id")
    seq = message.get("seq")
    sender = message.get("from_agent_id", message.get("from"))
    body_md = message.get("body_md")
    if principal_id is not None and sender == principal_id:
        return None
    if (
        not isinstance(conversation_id, str)
        or not conversation_id
        or not isinstance(seq, int)
        or isinstance(seq, bool)
        or seq <= 0
        or not isinstance(sender, str)
        or not sender
        or not isinstance(body_md, str)
    ):
        raise EchoFailure("sync returned incomplete message fields")
    reply_body = "Acknowledged." if kind == "question" else body_md
    return PendingReply(
        conversation_id=conversation_id,
        in_reply_to=seq,
        client_key=ulid_factory(),
        body_md=reply_body,
    )


class EchoResident:
    def __init__(
        self,
        client: JsonHttpClient,
        state_store: StateStore,
        *,
        slow_seconds: float = 0.0,
        offline_seconds: float = 0.0,
        retry_backoff_seconds: Sequence[float] = DEFAULT_RETRY_BACKOFF_SECONDS,
        sleep: Callable[[float], None] = time.sleep,
        logger: Callable[[str], None] | None = None,
        ulid_factory: Callable[[], str] = new_ulid,
    ) -> None:
        self.client = client
        self.state_store = state_store
        self.state = state_store.load()
        self.slow_seconds = slow_seconds
        self.offline_seconds = offline_seconds
        self.retry_backoff_seconds = tuple(retry_backoff_seconds)
        self.sleep = sleep
        self.logger = logger if logger is not None else (
            lambda message: print(message, file=sys.stderr)
        )
        self.ulid_factory = ulid_factory

    def run_forever(self) -> None:
        if self.offline_seconds > 0:
            self.logger("[echo] offline interval started")
            self.sleep(self.offline_seconds)
            self.logger("[echo] offline interval ended")
        while True:
            progressed = self.run_cycle()
            if not progressed:
                self.sleep(OUTER_RETRY_SECONDS)

    def run_cycle(self) -> bool:
        if not self._flush_pending():
            return False
        sync = self._request_with_retry(
            "GET",
            "/v1/workspace/messaging/sync?"
            + urllib.parse.urlencode({"cursor": self.state.cursor, "wait": SYNC_WAIT_SECONDS}),
            None,
        )
        if sync is None:
            self.logger("[echo] sync temporarily unavailable; cursor unchanged")
            return False
        cursor, messages, sync_principal_id = parse_sync(sync, self.state.cursor)
        principal_id = self.state.principal_id or sync_principal_id
        if (
            self.state.principal_id is not None
            and sync_principal_id is not None
            and sync_principal_id != self.state.principal_id
        ):
            raise EchoFailure("sync principal does not match the persisted credential binding")
        pending = list(self.state.pending)
        queued = {(reply.conversation_id, reply.in_reply_to) for reply in pending}
        for message in messages:
            reply = reply_for_message(message, principal_id, self.ulid_factory)
            if reply is None:
                continue
            identity = (reply.conversation_id, reply.in_reply_to)
            if identity in queued:
                continue
            pending.append(reply)
            queued.add(identity)
        next_state = EchoState(
            cursor=cursor,
            pending=tuple(pending),
            principal_id=principal_id,
        )
        if next_state != self.state:
            self.state_store.save(next_state)
            self.state = next_state
        return self._flush_pending()

    def _flush_pending(self) -> bool:
        while self.state.pending:
            reply = self.state.pending[0]
            if self.slow_seconds > 0:
                self.sleep(self.slow_seconds)
            path = (
                "/v1/workspace/messaging/conversations/"
                f"{urllib.parse.quote(reply.conversation_id, safe='')}/messages"
            )
            body = {
                "client_key": reply.client_key,
                "kind": "text",
                "body_md": reply.body_md,
                "in_reply_to": reply.in_reply_to,
            }
            response = self._request_with_retry("POST", path, body)
            if response is None:
                self.logger("[echo] reply outcome remains ambiguous; reply kept queued")
                return False
            sender = response_sender(response)
            if (
                sender is not None
                and self.state.principal_id is not None
                and sender != self.state.principal_id
            ):
                raise EchoFailure("reply receipt sender does not match the persisted credential binding")
            self.state = EchoState(
                cursor=self.state.cursor,
                pending=self.state.pending[1:],
                principal_id=self.state.principal_id or sender,
            )
            self.state_store.save(self.state)
        return True

    def _request_with_retry(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None,
    ) -> Mapping[str, Any] | None:
        attempts = len(self.retry_backoff_seconds) + 1
        for attempt in range(attempts):
            try:
                response = self.client.request(method, path, body)
                if not isinstance(response, dict):
                    raise EchoFailure("API response must be a JSON object")
                return response
            except TransientHttpFailure:
                if attempt >= len(self.retry_backoff_seconds):
                    return None
                self.sleep(self.retry_backoff_seconds[attempt])
        return None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run the Straylight agent-messaging echo resident.")
    parser.add_argument(
        "--base-url",
        default=os.environ.get("STRAYLIGHT_ECHO_BASE_URL"),
        help="Straylight API origin (or STRAYLIGHT_ECHO_BASE_URL)",
    )
    parser.add_argument(
        "--credential-ref",
        default=os.environ.get("STRAYLIGHT_ECHO_CREDENTIAL_REF"),
        help="env:NAME or file:/path (or STRAYLIGHT_ECHO_CREDENTIAL_REF)",
    )
    parser.add_argument(
        "--state-file",
        type=Path,
        default=(
            Path(os.environ["STRAYLIGHT_ECHO_STATE_FILE"])
            if "STRAYLIGHT_ECHO_STATE_FILE" in os.environ
            else None
        ),
        help="caller-selected durable cursor/outbox file",
    )
    parser.add_argument("--slow", type=float, default=0.0, help="delay each logical reply")
    parser.add_argument("--offline", type=float, default=0.0, help="stay offline before polling")
    parser.add_argument("--request-timeout", type=float, default=DEFAULT_REQUEST_TIMEOUT_SECONDS)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    sanitizer = Sanitizer()
    try:
        if not args.base_url:
            raise EchoFailure("--base-url or STRAYLIGHT_ECHO_BASE_URL is required")
        if args.state_file is None:
            raise EchoFailure("--state-file or STRAYLIGHT_ECHO_STATE_FILE is required")
        if args.slow < 0 or args.offline < 0 or args.request_timeout <= 0:
            raise EchoFailure("slow/offline must be nonnegative and request timeout must be positive")
        token = credential_from_reference(args.credential_ref, sanitizer)
        client = JsonHttpClient(args.base_url, token, args.request_timeout)
        resident = EchoResident(
            client,
            StateStore(args.state_file.expanduser().resolve()),
            slow_seconds=args.slow,
            offline_seconds=args.offline,
        )
        print("[echo] resident started", file=sys.stderr)
        resident.run_forever()
        return 0
    except EchoFailure as error:
        print(f"[echo] FAIL: {sanitizer.text(error)}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("[echo] stopped", file=sys.stderr)
        return 130
    except Exception as error:
        print(
            f"[echo] FAIL: unexpected {type(error).__name__}: {sanitizer.text(error)}",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

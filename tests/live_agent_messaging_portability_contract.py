#!/usr/bin/env python3
"""Gate 12e: file-native portability contract for agent messaging.

The live mode provisions two disposable workspaces through the production API,
seeds only their pre-existing messaging principals through the disposable
database, and exercises the normal workspace write/change/export/import paths.
It never copies projection rows between users.

``--preflight`` is deliberately read-only.  It keeps the contract red until the
shared simple-core and carrystate dispatchers understand managed conversation
entries, including their narrow 12 MiB exception and continuation ordering.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
SIMPLE_CORE = ROOT / "apps" / "api" / "src" / "simple_core.rs"
CARRYSTATE_EXPORT = ROOT / "apps" / "api" / "src" / "carrystate_export.rs"
CARRYSTATE_IMPORT = ROOT / "apps" / "api" / "src" / "carrystate_import.rs"

IMPORT_FORMAT = "straylight-workspace-import-manifest@v1"
CONVERSATION_PREFIX = ".straylight/conversations/"
CONTINUATION_BODY = (
    "This conversation continues from the preceding 500-message entry."
)
ORDINARY_LIMIT = 4 * 1024 * 1024
MANAGED_LIMIT = 12 * 1024 * 1024
MESSAGE_LIMIT = 16 * 1024
MESSAGE_COUNT = 500

# The child intentionally sorts before its parent.  A generic path-sorted
# carrystate import therefore fails unless the messaging-aware importer orders
# the continuation graph parent-first.
CHILD_ID = uuid.UUID("00000000-0000-7000-8000-000000000001")
PARENT_ID = uuid.UUID("ffffffff-ffff-7fff-bfff-ffffffffffff")
MALFORMED_ID = uuid.UUID("10000000-0000-7000-8000-000000000001")
UNMARKED_ID = uuid.UUID("10000000-0000-7000-8000-000000000002")
MANAGED_OVERSIZE_ID = uuid.UUID("10000000-0000-7000-8000-000000000003")
PARTICIPANTS = [
    {"agent_id": "agent-a", "role": "participant"},
    {"agent_id": "owner", "role": "participant"},
]
CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


class ContractFailure(RuntimeError):
    pass


class Sanitizer:
    def __init__(self) -> None:
        self.secrets: set[str] = set()

    def register(self, value: str | None) -> None:
        if value:
            self.secrets.add(value)

    def text(self, value: Any) -> str:
        rendered = str(value)
        for secret in sorted(self.secrets, key=len, reverse=True):
            rendered = rendered.replace(secret, "<redacted>")
        return rendered


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractFailure(message)


def data(response: dict[str, Any]) -> dict[str, Any]:
    value = response.get("data", response)
    if not isinstance(value, dict):
        raise ContractFailure("API response data was not an object")
    return value


def parse_ref(value: Any, prefix: str) -> uuid.UUID:
    require(isinstance(value, str), f"{prefix} reference was omitted")
    raw = value.removeprefix(f"{prefix}:")
    try:
        return uuid.UUID(raw)
    except (ValueError, AttributeError) as error:
        raise ContractFailure(f"invalid {prefix} reference") from error


def credential_from_reference(
    reference: str | None, sanitizer: Sanitizer
) -> str:
    if not reference:
        raise ContractFailure("--admin-token-ref is required outside preflight")
    if reference.startswith("env:"):
        name = reference[4:]
        if not name or not name.replace("_", "a").isalnum() or name[0].isdigit():
            raise ContractFailure("invalid admin credential environment reference")
        token = os.environ.get(name, "")
    elif reference.startswith("file:"):
        try:
            token = Path(reference[5:]).expanduser().read_text(encoding="utf-8").strip()
        except OSError as error:
            raise ContractFailure(f"could not read admin credential file: {error}") from error
    else:
        raise ContractFailure("admin credential must use env:NAME or file:/path")
    if not token or "\n" in token or "\r" in token:
        raise ContractFailure("admin credential reference resolved to invalid content")
    sanitizer.register(token)
    return token


@dataclass(frozen=True)
class HttpResult:
    status: int
    body: dict[str, Any]


class Client:
    def __init__(
        self,
        base_url: str,
        token: str,
        sanitizer: Sanitizer,
        timeout: float,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.sanitizer = sanitizer
        self.timeout = timeout

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        expected: int = 200,
    ) -> HttpResult:
        payload = None
        headers = {
            "Accept": "application/json",
            "Authorization": f"Bearer {self.token}",
            "User-Agent": "straylight-messaging-portability-gate12e/1",
        }
        if body is not None:
            payload = json.dumps(
                body, ensure_ascii=False, separators=(",", ":")
            ).encode("utf-8")
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(
            self.base_url + path,
            data=payload,
            method=method,
            headers=headers,
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        except (TimeoutError, urllib.error.URLError) as error:
            raise ContractFailure(f"{method} {path} could not reach API: {error}") from error
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError as error:
            raise ContractFailure(f"{method} {path} returned non-JSON") from error
        if not isinstance(parsed, dict):
            raise ContractFailure(f"{method} {path} returned a non-object JSON response")
        if status != expected:
            detail = self.sanitizer.text(json.dumps(parsed, sort_keys=True))
            raise ContractFailure(
                f"{method} {path} returned HTTP {status}, expected {expected}: "
                f"{detail[:1000]}"
            )
        return HttpResult(status, parsed)

    def get(self, path: str, *, expected: int = 200) -> dict[str, Any]:
        return self.request("GET", path, expected=expected).body

    def post(
        self, path: str, body: Any, *, expected: int = 200
    ) -> dict[str, Any]:
        return self.request("POST", path, body=body, expected=expected).body


@dataclass(frozen=True)
class Workspace:
    user_id: uuid.UUID
    credential_id: uuid.UUID
    client: Client


def provision_workspace(
    admin: Client,
    run_id: str,
    label: str,
    sanitizer: Sanitizer,
    timeout: float,
) -> Workspace:
    response = admin.post(
        "/v1/admin/users",
        {
            "external_ref": f"messaging-portability:{run_id}:{label}",
            "display_name": f"Messaging portability {label}",
            "credential_name": f"Messaging portability {label} owner",
        },
    )
    user = response.get("user")
    credential = response.get("credential")
    require(isinstance(user, dict), "provisioning omitted user")
    require(isinstance(credential, dict), "provisioning omitted credential")
    token = credential.get("token")
    require(isinstance(token, str) and bool(token), "provisioning omitted owner token")
    sanitizer.register(token)
    return Workspace(
        user_id=parse_ref(user.get("id"), "user"),
        credential_id=parse_ref(credential.get("id"), "credential"),
        client=Client(admin.base_url, token, sanitizer, timeout),
    )


def docker_psql(
    args: argparse.Namespace,
    sql: str,
    *,
    tuples_only: bool = True,
) -> str:
    command = [
        "docker",
        "exec",
        args.database_container,
        "psql",
        "-v",
        "ON_ERROR_STOP=1",
        "-U",
        args.database_user,
        "-d",
        args.database_name,
    ]
    if tuples_only:
        command.extend(["-At"])
    command.extend(["-c", sql])
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    if completed.returncode != 0:
        raise ContractFailure(f"database contract query failed: {completed.stderr[-1000:]}")
    return completed.stdout.strip()


def seed_principals(args: argparse.Namespace, workspace: Workspace) -> None:
    user_id = workspace.user_id
    credential_id = workspace.credential_id
    docker_psql(
        args,
        f"""
        BEGIN;
        INSERT INTO straylight.messaging_agents (
          user_id,agent_id,display_name,principal_kind,delivery_mode,
          created_by_credential_id
        ) VALUES
          ('{user_id}'::uuid,'agent-a','Agent A','resident','pull','{credential_id}'::uuid),
          ('{user_id}'::uuid,'owner','Owner','owner','pull','{credential_id}'::uuid);
        INSERT INTO straylight.messaging_credential_bindings (
          user_id,credential_id,agent_id,bound_by_credential_id
        ) VALUES (
          '{user_id}'::uuid,'{credential_id}'::uuid,'owner','{credential_id}'::uuid
        );
        COMMIT;
        """,
        tuples_only=False,
    )


def crockford(value: int) -> str:
    encoded = ["0"] * 26
    for index in range(25, -1, -1):
        encoded[index] = CROCKFORD[value & 31]
        value >>= 5
    return "".join(encoded)


def request_hash(
    conversation_id: uuid.UUID,
    client_key: str,
    body_md: str,
) -> str:
    payload = {
        "conversation_id": str(conversation_id),
        "client_key": client_key,
        "kind": "text",
        "body_md": body_md,
        "refs": [],
        "in_reply_to_conversation_id": None,
        "in_reply_to": None,
        "correlation_id": None,
        "expects_reply": False,
        "reply_by": None,
    }
    serialized = json.dumps(
        payload, ensure_ascii=False, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(serialized).hexdigest()


def canonical_header(
    conversation_id: uuid.UUID,
    *,
    status: str,
    continues_from: uuid.UUID | None,
    latest_sync_cursor: int,
    created_at: str,
    closed_at: str | None,
) -> dict[str, Any]:
    return {
        "schema": "conversation.v1",
        "conversation_id": str(conversation_id),
        "conversation_kind": "direct",
        "direct_key": "agent-a|owner",
        "subject": None,
        "status": status,
        "participants": PARTICIPANTS,
        "created_by_agent_id": "owner",
        "continues_from": str(continues_from) if continues_from else None,
        "agent_streak": 0,
        "needs_human": False,
        "latest_sync_cursor": latest_sync_cursor,
        "created_at": created_at,
        "closed_at": closed_at,
    }


def text_message(
    conversation_id: uuid.UUID,
    seq: int,
    body_md: str,
    created_at: str,
) -> dict[str, Any]:
    client_key = crockford(seq)
    sender = "owner" if seq % 2 else "agent-a"
    return {
        "seq": seq,
        "message_id": str(uuid.uuid5(conversation_id, f"message:{seq}")),
        "from_agent_id": sender,
        "client_key": client_key,
        "system_key": None,
        "request_hash": request_hash(conversation_id, client_key, body_md),
        "kind": "text",
        "refs": [],
        "in_reply_to_conversation_id": None,
        "in_reply_to": None,
        "correlation_id": None,
        "expects_reply": False,
        "reply_by": None,
        "reply_by_handled_at": None,
        "sync_cursor": seq,
        "created_at": created_at,
        "body_bytes": len(body_md.encode("utf-8")),
    }


def system_message(
    conversation_id: uuid.UUID,
    parent_id: uuid.UUID,
    created_at: str,
) -> dict[str, Any]:
    return {
        "seq": 1,
        "message_id": str(uuid.uuid5(conversation_id, "continuation")),
        "from_agent_id": None,
        "client_key": None,
        "system_key": f"continuation:{parent_id}",
        "request_hash": None,
        "kind": "system",
        "refs": [],
        "in_reply_to_conversation_id": None,
        "in_reply_to": None,
        "correlation_id": None,
        "expects_reply": False,
        "reply_by": None,
        "reply_by_handled_at": None,
        "sync_cursor": MESSAGE_COUNT + 1,
        "created_at": created_at,
        "body_bytes": len(CONTINUATION_BODY.encode("utf-8")),
    }


def render_conversation(
    header: dict[str, Any], messages: Iterable[tuple[dict[str, Any], str]]
) -> bytes:
    output = bytearray()
    header_json = json.dumps(header, ensure_ascii=False, separators=(",", ":")).replace(
        ">", "\\u003e"
    )
    output.extend(f"<!-- straylight-conversation-v1 {header_json} -->\n".encode())
    for envelope, body_md in messages:
        envelope_json = json.dumps(
            envelope, ensure_ascii=False, separators=(",", ":")
        ).replace(">", "\\u003e")
        output.extend(f"<!-- straylight-message-v1 {envelope_json} -->\n".encode())
        output.extend(body_md.encode("utf-8"))
        output.extend(b"\n<!-- /straylight-message-v1 -->\n")
    return bytes(output)


def conversation_metadata(header: dict[str, Any], *, imported: bool) -> dict[str, Any]:
    metadata = {
        "kind": "conversation",
        "schema": "conversation.v1",
        "conversation": {
            "id": header["conversation_id"],
            "conversation_kind": header["conversation_kind"],
            "direct_key": header["direct_key"],
            "subject": header["subject"],
            "status": header["status"],
            "participants": header["participants"],
            "created_by_agent_id": header["created_by_agent_id"],
            "continues_from": header["continues_from"],
            "agent_streak": header["agent_streak"],
            "needs_human": header["needs_human"],
            "latest_sync_cursor": header["latest_sync_cursor"],
            "created_at": header["created_at"],
            "closed_at": header["closed_at"],
        },
    }
    if imported:
        metadata["_straylight_import"] = {"format": IMPORT_FORMAT}
    return metadata


def fixtures() -> dict[str, Any]:
    created_at = "2026-08-27T12:00:00Z"
    continued_at = "2026-08-27T12:00:01Z"
    parent_header = canonical_header(
        PARENT_ID,
        status="closed",
        continues_from=None,
        latest_sync_cursor=MESSAGE_COUNT,
        created_at=created_at,
        closed_at=created_at,
    )
    parent_messages: list[tuple[dict[str, Any], str]] = []
    for seq in range(1, MESSAGE_COUNT + 1):
        marker = f"gate-12e-message-{seq:03d}:"
        body = marker + ("x" * (MESSAGE_LIMIT - len(marker)))
        parent_messages.append(
            (text_message(PARENT_ID, seq, body, created_at), body)
        )
    parent = render_conversation(parent_header, parent_messages)
    require(len(parent) > ORDINARY_LIMIT, "managed boundary fixture did not exceed 4 MiB")
    require(len(parent) <= MANAGED_LIMIT, "managed boundary fixture exceeded 12 MiB")

    child_header = canonical_header(
        CHILD_ID,
        status="open",
        continues_from=PARENT_ID,
        latest_sync_cursor=MESSAGE_COUNT + 1,
        created_at=continued_at,
        closed_at=None,
    )
    child = render_conversation(
        child_header,
        [(system_message(CHILD_ID, PARENT_ID, continued_at), CONTINUATION_BODY)],
    )
    return {
        "parent_header": parent_header,
        "parent": parent,
        "parent_metadata": conversation_metadata(parent_header, imported=True),
        "child_header": child_header,
        "child": child,
        "child_metadata": conversation_metadata(child_header, imported=True),
    }


def workspace_generation(client: Client) -> int:
    response = data(client.get("/v1/workspace/manifest?limit=1&offset=0"))
    generation = response.get("workspace_generation")
    require(isinstance(generation, int), "workspace manifest omitted generation")
    return generation


def changed_paths(client: Client, since_generation: int) -> list[str]:
    query = urllib.parse.urlencode(
        {"since_generation": since_generation, "limit": 200}
    )
    response = data(client.get(f"/v1/workspace/changes?{query}"))
    changes = response.get("changes")
    require(isinstance(changes, list), "workspace changes omitted change list")
    return [
        str(change["path"])
        for change in changes
        if isinstance(change, dict) and isinstance(change.get("path"), str)
    ]


def write_entry(
    client: Client,
    conversation_id: uuid.UUID,
    content: bytes,
    metadata: dict[str, Any],
    *,
    path: str | None = None,
    expected: int = 200,
) -> dict[str, Any]:
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ContractFailure("canonical fixture was not UTF-8") from error
    return client.post(
        "/v1/workspace/write",
        {
            "path": path or f"{CONVERSATION_PREFIX}{conversation_id}.md",
            "content": text,
            "media_type": "text/markdown",
            "metadata": metadata,
            "expected_version": 0,
        },
        expected=expected,
    )


def assert_failed_candidate_absent(
    args: argparse.Namespace,
    user_id: uuid.UUID,
    conversation_ids: Iterable[uuid.UUID],
    ordinary_path: str,
) -> None:
    ids = ",".join(f"'{value}'::uuid" for value in conversation_ids)
    result = docker_psql(
        args,
        f"""
        SELECT jsonb_build_object(
          'entries',(
            SELECT count(*) FROM straylight.entries
            WHERE user_id='{user_id}'::uuid
              AND (path='{ordinary_path}' OR id IN ({ids}))
          ),
          'conversations',(
            SELECT count(*) FROM straylight.messaging_conversations
            WHERE user_id='{user_id}'::uuid AND conversation_id IN ({ids})
          )
        )::text
        """,
    )
    counts = json.loads(result)
    require(counts == {"entries": 0, "conversations": 0}, f"failed candidate persisted: {counts}")


def projection_snapshot(
    args: argparse.Namespace, user_id: uuid.UUID
) -> dict[str, Any]:
    conversation_ids = f"ARRAY['{PARENT_ID}'::uuid,'{CHILD_ID}'::uuid]"
    queries = {
        "conversations": f"""
          SELECT coalesce(jsonb_agg(to_jsonb(item) ORDER BY item.conversation_id),'[]')::text
          FROM (
            SELECT conversation_id,entry_id,path,conversation_kind,direct_key,subject,status,
                   created_by_agent_id,last_seq,last_message_at,agent_streak,needs_human,
                   continues_from,latest_sync_cursor,closed_at,created_at
            FROM straylight.messaging_conversations
            WHERE user_id='{user_id}'::uuid AND conversation_id=ANY({conversation_ids})
          ) AS item
        """,
        "participants": f"""
          SELECT coalesce(jsonb_agg(to_jsonb(item)
                   ORDER BY item.conversation_id,item.agent_id),'[]')::text
          FROM (
            SELECT conversation_id,agent_id,role,last_read_seq,joined_at,updated_at
            FROM straylight.messaging_participants
            WHERE user_id='{user_id}'::uuid AND conversation_id=ANY({conversation_ids})
          ) AS item
        """,
        "messages": f"""
          SELECT coalesce(jsonb_agg(to_jsonb(item)
                   ORDER BY item.conversation_id,item.seq),'[]')::text
          FROM (
            SELECT conversation_id,seq,message_id,from_agent_id,client_key,system_key,
                   request_hash,kind,octet_length(body_md) AS body_bytes,
                   encode(digest(body_md,'sha256'),'hex') AS body_sha256,
                   refs,in_reply_to_conversation_id,in_reply_to,correlation_id,
                   expects_reply,reply_by,reply_by_handled_at,sync_cursor,created_at
            FROM straylight.messaging_message_index
            WHERE user_id='{user_id}'::uuid AND conversation_id=ANY({conversation_ids})
          ) AS item
        """,
        "sync": f"""
          SELECT jsonb_build_object('current_cursor',coalesce((
            SELECT current_cursor FROM straylight.messaging_sync_state
            WHERE user_id='{user_id}'::uuid
          ),0))::text
        """,
    }
    return {name: json.loads(docker_psql(args, sql)) for name, sql in queries.items()}


def assert_no_search_work(args: argparse.Namespace, user_id: uuid.UUID) -> None:
    result = docker_psql(
        args,
        f"""
        WITH managed_entries AS (
          SELECT id FROM straylight.entries
          WHERE user_id='{user_id}'::uuid
            AND path IN (
              '{CONVERSATION_PREFIX}{PARENT_ID}.md',
              '{CONVERSATION_PREFIX}{CHILD_ID}.md'
            )
        )
        SELECT jsonb_build_object(
          'chunks',(
            SELECT count(*) FROM straylight.search_chunks AS chunk
            WHERE chunk.user_id='{user_id}'::uuid
              AND chunk.entry_id IN (SELECT id FROM managed_entries)
          ),
          'embed_jobs',(
            SELECT count(*) FROM straylight.jobs AS job
            WHERE job.user_id='{user_id}'::uuid
              AND job.kind='embed_entry'
              AND (job.payload->>'entry_id')::uuid IN (SELECT id FROM managed_entries)
          )
        )::text
        """,
    )
    require(
        json.loads(result) == {"chunks": 0, "embed_jobs": 0},
        f"managed conversation entered search/index work: {result}",
    )


def run_carrystate(
    args: argparse.Namespace,
    sanitizer: Sanitizer,
    token: str,
    arguments: list[str],
) -> None:
    completed = subprocess.run(
        [str(args.carrystate), "workspace", *arguments],
        env={
            **os.environ,
            "CARRYSTATE_API_URL": args.base_url,
            "CARRYSTATE_API_TOKEN": token,
        },
        text=True,
        capture_output=True,
        check=False,
        timeout=args.cli_timeout,
    )
    if completed.returncode != 0:
        detail = sanitizer.text(completed.stderr[-1500:] or completed.stdout[-1500:])
        raise ContractFailure(f"carrystate {' '.join(arguments[:2])} failed: {detail}")


def preflight() -> dict[str, Any]:
    sources = {
        "simple_core": SIMPLE_CORE.read_text(encoding="utf-8"),
        "carrystate_export": CARRYSTATE_EXPORT.read_text(encoding="utf-8"),
        "carrystate_import": CARRYSTATE_IMPORT.read_text(encoding="utf-8"),
    }
    required = {
        "simple_core messaging projection dispatcher": (
            "simple_core",
            "messaging_service::sync_managed_entry_in_tx",
        ),
        "simple_core managed 12 MiB boundary": (
            "simple_core",
            "MAX_CANONICAL_CONVERSATION_BYTES",
        ),
        "simple_core conversation candidate recognition": (
            "simple_core",
            "is_conversation_candidate",
        ),
        "carrystate managed exact-read boundary": (
            "carrystate_export",
            "MAX_CANONICAL_CONVERSATION_BYTES",
        ),
        "carrystate managed import boundary": (
            "carrystate_import",
            "MAX_CANONICAL_CONVERSATION_BYTES",
        ),
        "carrystate continuation parent-first ordering": (
            "carrystate_import",
            "continues_from",
        ),
    }
    missing = [
        label
        for label, (source, marker) in required.items()
        if marker not in sources[source]
    ]
    if missing:
        raise ContractFailure(
            "unwired shared messaging portability seams: " + "; ".join(missing)
        )
    return {
        "schema": "straylight-agent-messaging-portability-preflight@v1",
        "status": "pass",
        "checks": sorted(required),
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    sanitizer = Sanitizer()
    admin_token = credential_from_reference(args.admin_token_ref, sanitizer)
    admin = Client(args.base_url, admin_token, sanitizer, args.timeout)
    run_id = f"{int(time.time())}-{uuid.uuid4().hex[:12]}"
    source = provision_workspace(admin, run_id, "source", sanitizer, args.timeout)
    target = provision_workspace(admin, run_id, "target", sanitizer, args.timeout)
    seed_principals(args, source)
    seed_principals(args, target)
    fixture = fixtures()

    source_generation = workspace_generation(source.client)
    malformed_id = MALFORMED_ID
    malformed_header = canonical_header(
        malformed_id,
        status="closed",
        continues_from=None,
        latest_sync_cursor=1,
        created_at="2026-08-27T12:00:00Z",
        closed_at="2026-08-27T12:00:00Z",
    )
    write_entry(
        source.client,
        malformed_id,
        b"not a canonical conversation",
        conversation_metadata(malformed_header, imported=True),
        expected=400,
    )

    unmarked_id = UNMARKED_ID
    unmarked_header = canonical_header(
        unmarked_id,
        status="closed",
        continues_from=None,
        latest_sync_cursor=1,
        created_at="2026-08-27T12:00:00Z",
        closed_at="2026-08-27T12:00:00Z",
    )
    unmarked_body = "unmarked conversation-shaped payload"
    unmarked_content = render_conversation(
        unmarked_header,
        [
            (
                text_message(
                    unmarked_id,
                    1,
                    unmarked_body,
                    "2026-08-27T12:00:00Z",
                ),
                unmarked_body,
            )
        ],
    )
    ordinary_unmarked_path = f"Contracts/{run_id}/unmarked-conversation.md"
    write_entry(
        source.client,
        unmarked_id,
        unmarked_content,
        conversation_metadata(unmarked_header, imported=False),
        path=ordinary_unmarked_path,
        expected=400,
    )

    ordinary_oversize = ("ordinary-boundary:" + "o" * ORDINARY_LIMIT).encode()
    write_entry(
        source.client,
        uuid.uuid4(),
        ordinary_oversize,
        {},
        path=f"Contracts/{run_id}/ordinary-over-4mib.md",
        expected=413,
    )

    managed_oversize_id = MANAGED_OVERSIZE_ID
    managed_oversize_header = canonical_header(
        managed_oversize_id,
        status="closed",
        continues_from=None,
        latest_sync_cursor=1,
        created_at="2026-08-27T12:00:00Z",
        closed_at="2026-08-27T12:00:00Z",
    )
    write_entry(
        source.client,
        managed_oversize_id,
        b"m" * (MANAGED_LIMIT + 1),
        conversation_metadata(managed_oversize_header, imported=True),
        expected=413,
    )

    assert_failed_candidate_absent(
        args,
        source.user_id,
        [malformed_id, unmarked_id, managed_oversize_id],
        ordinary_unmarked_path,
    )
    require(
        workspace_generation(source.client) == source_generation,
        "failed or oversized candidates changed memory.changes generation",
    )

    # The child is valid but cannot precede the parent at the projection boundary.
    write_entry(
        source.client,
        CHILD_ID,
        fixture["child"],
        fixture["child_metadata"],
        expected=400,
    )
    require(
        workspace_generation(source.client) == source_generation,
        "rejected continuation child changed memory.changes generation",
    )

    parent_receipt = data(
        write_entry(
            source.client,
            PARENT_ID,
            fixture["parent"],
            fixture["parent_metadata"],
        )
    )
    child_receipt = data(
        write_entry(
            source.client,
            CHILD_ID,
            fixture["child"],
            fixture["child_metadata"],
        )
    )
    require(
        parent_receipt.get("entry_ref") == f"entry:{PARENT_ID}",
        "managed parent did not preserve path-derived entry identity",
    )
    require(
        child_receipt.get("entry_ref") == f"entry:{CHILD_ID}",
        "managed child did not preserve path-derived entry identity",
    )
    expected_paths = {
        f"{CONVERSATION_PREFIX}{PARENT_ID}.md",
        f"{CONVERSATION_PREFIX}{CHILD_ID}.md",
    }
    require(
        set(changed_paths(source.client, source_generation)) == expected_paths,
        "source memory.changes did not contain exactly the canonical conversation entries",
    )
    assert_no_search_work(args, source.user_id)
    source_projection = projection_snapshot(args, source.user_id)
    require(
        len(source_projection["conversations"]) == 2
        and len(source_projection["participants"]) == 4
        and len(source_projection["messages"]) == MESSAGE_COUNT + 1,
        "source messaging projection has the wrong cardinality",
    )

    target_generation = workspace_generation(target.client)
    with tempfile.TemporaryDirectory(prefix="straylight-messaging-portability-") as temporary:
        temporary_root = Path(temporary)
        source_export = temporary_root / "source-export"
        target_export = temporary_root / "target-export"
        import_state = temporary_root / "target-state"
        run_carrystate(
            args,
            sanitizer,
            source.client.token,
            ["export", "--output", str(source_export)],
        )
        manifest = json.loads((source_export / "manifest.json").read_text(encoding="utf-8"))
        managed_entries = [
            entry
            for entry in manifest.get("entries", [])
            if isinstance(entry, dict) and entry.get("path") in expected_paths
        ]
        require(len(managed_entries) == 2, "source export omitted a managed conversation")
        require(
            [entry["path"] for entry in managed_entries]
            == [
                f"{CONVERSATION_PREFIX}{CHILD_ID}.md",
                f"{CONVERSATION_PREFIX}{PARENT_ID}.md",
            ],
            "fixture no longer presents continuation child before parent to importer",
        )
        require(
            (source_export / "workspace" / f"{CONVERSATION_PREFIX}{PARENT_ID}.md").read_bytes()
            == fixture["parent"],
            "source export changed parent bytes",
        )
        require(
            (source_export / "workspace" / f"{CONVERSATION_PREFIX}{CHILD_ID}.md").read_bytes()
            == fixture["child"],
            "source export changed continuation bytes",
        )

        run_carrystate(
            args,
            sanitizer,
            target.client.token,
            [
                "import",
                "--root",
                str(source_export),
                "--state-dir",
                str(import_state),
                "--describe-binaries",
                "false",
            ],
        )
        run_carrystate(
            args,
            sanitizer,
            target.client.token,
            ["export", "--output", str(target_export)],
        )
        for conversation_id, expected_bytes in [
            (PARENT_ID, fixture["parent"]),
            (CHILD_ID, fixture["child"]),
        ]:
            target_bytes = (
                target_export
                / "workspace"
                / f"{CONVERSATION_PREFIX}{conversation_id}.md"
            ).read_bytes()
            require(
                target_bytes == expected_bytes,
                f"target export changed canonical bytes for {conversation_id}",
            )

    require(
        set(changed_paths(target.client, target_generation)) == expected_paths,
        "target memory.changes did not contain exactly the imported conversations",
    )
    assert_no_search_work(args, target.user_id)
    target_projection = projection_snapshot(args, target.user_id)
    require(
        target_projection == source_projection,
        "rebuilt target conversation/message projection differs from source",
    )
    require(
        all(
            participant.get("last_read_seq") == 0
            for participant in target_projection["participants"]
        ),
        "portable import did not reset delivery-only read positions",
    )

    return {
        "schema": "straylight-agent-messaging-portability-contract@v1",
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "managed_parent_bytes": len(fixture["parent"]),
        "conversation_ids": [str(PARENT_ID), str(CHILD_ID)],
        "checks": {
            "byte_exact_export_import": True,
            "memory_changes_source_and_target": True,
            "identical_conversation_message_projection": True,
            "no_search_chunks_or_embed_jobs": True,
            "parent_first_continuation_import": True,
            "malformed_and_unmarked_fail_closed": True,
            "ordinary_4mib_managed_12mib_boundary": True,
            "delivery_read_positions_reset": True,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--preflight", action="store_true")
    parser.add_argument("--base-url", default="http://127.0.0.1:18112")
    parser.add_argument("--admin-token-ref")
    parser.add_argument("--database-container", default="straylight_agent_messaging-db-1")
    parser.add_argument("--database-user", default="admin")
    parser.add_argument("--database-name", default="straylight")
    parser.add_argument("--timeout", type=float, default=180.0)
    parser.add_argument("--cli-timeout", type=float, default=300.0)
    parser.add_argument(
        "--carrystate",
        type=Path,
        default=ROOT / "apps" / "api" / "target" / "debug" / "carrystate",
    )
    args = parser.parse_args()
    try:
        result = preflight() if args.preflight else run(args)
    except (ContractFailure, KeyError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        result = {
            "schema": "straylight-agent-messaging-portability-contract@v1",
            "status": "fail",
            "error": str(error),
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

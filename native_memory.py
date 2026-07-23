#!/usr/bin/env python3

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import sys
import time
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from typing import Any, Iterator

from native_eval import NativeApiClient, NativeApiError, NativeResponse, response_field


def load_state(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {
            "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
            "session_id": None,
            "corpus_revision": None,
            "checkpoint_id": None,
            "checkpoint": None,
            "operations": [],
        }
    return json.loads(path.read_text(encoding="utf-8"))


def save_state(path: Path, state: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


@contextmanager
def locked_state(path: Path) -> Iterator[dict[str, Any]]:
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.with_name(path.name + ".lock")
    with lock_path.open("a+", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            yield load_state(path)
        finally:
            fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


def record_state(
    path: Path,
    state: dict[str, Any],
    operation: str,
    *,
    result_chars: int,
    elapsed_ms: float,
    http_status: int,
    request_id: str | None,
    service_status: str | None,
    response: NativeResponse | None = None,
) -> dict[str, Any]:
    state["operations"].append({
        "at": datetime.now().astimezone().isoformat(timespec="milliseconds"),
        "operation": operation,
        "http_status": http_status,
        "service_status": service_status,
        "request_id": request_id,
        "elapsed_ms": round(elapsed_ms, 3),
        "result_chars": result_chars,
    })
    if response is not None:
        session_id = response_field(response, "session_id")
        corpus_revision = response_field(response, "corpus_revision", "revision_id")
        checkpoint_id = response_field(response, "checkpoint_id")
        if session_id:
            state["session_id"] = session_id
        if corpus_revision:
            state["corpus_revision"] = corpus_revision
        if checkpoint_id:
            state["checkpoint_id"] = checkpoint_id
            state["checkpoint"] = response.data
    save_state(path, state)
    return state


def initialization_marker(path: Path) -> Path:
    return path.with_name(path.name + ".initializing")


def initializer_is_active(marker: Path) -> bool:
    try:
        pid = int(marker.read_text(encoding="ascii").strip())
        os.kill(pid, 0)
        return True
    except (FileNotFoundError, ProcessLookupError, PermissionError, ValueError):
        return False


def wait_for_initializing_peer(path: Path, grace_seconds: float = 0.5) -> None:
    if load_state(path).get("session_id"):
        return
    marker = initialization_marker(path)
    deadline = time.monotonic() + grace_seconds
    while time.monotonic() < deadline and not initializer_is_active(marker):
        time.sleep(0.025)
    while initializer_is_active(marker) and not load_state(path).get("session_id"):
        time.sleep(0.025)


def load_json_argument(value: str) -> dict[str, Any]:
    raw = Path(value[1:]).read_text(encoding="utf-8") if value.startswith("@") else value
    parsed = json.loads(raw)
    if not isinstance(parsed, dict):
        raise ValueError("operation payload must be a JSON object")
    return parsed


def optional_json(value: str | None) -> dict[str, Any] | None:
    if value is None:
        return None
    stripped = value.lstrip()
    if stripped.startswith("{") or stripped.startswith("@"):
        return load_json_argument(value)
    return None


def require_session(state: dict[str, Any]) -> str:
    session_id = state.get("session_id")
    if not session_id:
        raise ValueError("Run ./memory open or ./memory resume first")
    return str(session_id)


def parse_read_range(value: str | None) -> tuple[int | None, int | None]:
    if value is None:
        return None, None
    match = re.fullmatch(r"\s*(\d+)\s*[:-]\s*(\d+)\s*", value)
    if not match:
        raise ValueError("read --range must be START:END or START-END")
    start, end = (int(part) for part in match.groups())
    if start < 1 or end < start:
        raise ValueError("read --range requires 1 <= START <= END")
    return start, end


def display_scope_query(args: argparse.Namespace, query: str) -> str:
    scope = args.query_scope or args.scope
    if not scope or scope == args.authorization_scope or scope.startswith("scope:"):
        return query
    normalized_scope = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", scope)
    normalized_scope = re.sub(r"(?<=[A-Z])(?=[A-Z][a-z])", " ", normalized_scope)
    scope_terms = {
        term.casefold()
        for term in re.split(r"[^A-Za-z0-9]+", normalized_scope)
        if len(term) > 1
    }
    normalized_query = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", query)
    normalized_query = re.sub(r"(?<=[A-Z])(?=[A-Z][a-z])", " ", normalized_query)
    query_terms = {
        term.casefold()
        for term in re.split(r"[^A-Za-z0-9]+", normalized_query)
        if len(term) > 1
    }
    if scope_terms and scope_terms.issubset(query_terms):
        return query
    return f"{normalized_scope}: {query}"


def query_scope(args: argparse.Namespace) -> str | dict[str, Any]:
    requested = args.query_scope
    if not requested or requested == args.scope:
        return {
            "authorization_scope": args.authorization_scope,
            "root_refs": [],
        }
    return requested


def render_native_response(command: str, response: NativeResponse) -> str:
    if command not in {"open", "resume", "query", "read", "compute", "verify"}:
        return json.dumps(response.body, indent=2, ensure_ascii=False)
    return json.dumps(
        compact_reasoning_response(command, response.body),
        indent=2,
        ensure_ascii=False,
    )


def compact_reasoning_response(command: str, body: dict[str, Any]) -> dict[str, Any]:
    data = body.get("data")
    data = data if isinstance(data, dict) else {}
    compact: dict[str, Any] = {
        key: body[key]
        for key in ("request_id", "session_id", "corpus_revision", "status")
        if body.get(key) is not None
    }
    if command in {"open", "resume"}:
        freshness = compact_freshness(body.get("freshness"))
        coverage = compact_coverage(body.get("coverage"))
        if freshness:
            compact["freshness"] = freshness
        if coverage:
            compact["coverage"] = coverage
    for key in ("conflicts", "gaps", "ambiguities"):
        if body.get(key):
            compact[key] = body[key]
    truncation = body.get("truncation")
    if isinstance(truncation, dict) and truncation.get("truncated"):
        compact["truncation"] = truncation

    if command in {"open", "resume"}:
        compact_data = compact_open_data(data)
    elif command == "query":
        compact_data = compact_query_data(data)
    elif command == "read":
        compact_data = compact_read_data(data)
    elif command == "verify":
        compact_data = compact_verify_data(data)
    else:
        compact_data = compact_generic_data(data, ("steps", "rows_returned", "estimated_tokens"))
    compact["data"] = compact_data
    return compact


def compact_open_data(data: dict[str, Any]) -> dict[str, Any]:
    compact = compact_generic_data(
        data,
        (
            "resolved_scope",
            "resume_checkpoint",
            "revision_delta",
            "learned_context",
            "initial_case_file",
        ),
    )
    evidence = data.get("initial_evidence")
    if isinstance(evidence, list) and evidence:
        compact["initial_evidence"] = [
            compact_candidate(item) for item in evidence if isinstance(item, dict)
        ]
    corpus_map = data.get("corpus_map")
    if isinstance(corpus_map, dict):
        compact["corpus_map"] = {
            key: corpus_map[key]
            for key in ("record_counts", "profile_counts", "available_views", "truncated")
            if key in corpus_map
        }
    return compact


def compact_query_data(data: dict[str, Any]) -> dict[str, Any]:
    compact = compact_generic_data(data, ())
    items = data.get("items")
    if isinstance(items, list):
        compact["items"] = [
            compact_query_item(item) for item in items if isinstance(item, dict)
        ]
    elif isinstance(data.get("results"), list):
        compact["results"] = [
            compact_candidate(item)
            for item in data["results"]
            if isinstance(item, dict)
        ]
    return compact


def compact_query_item(item: dict[str, Any]) -> dict[str, Any]:
    compact = {
        key: item[key]
        for key in ("id", "status")
        if item.get(key) is not None
    }
    results = item.get("results")
    if isinstance(results, list):
        compact["results"] = [
            compact_candidate(candidate)
            for candidate in results
            if isinstance(candidate, dict)
        ]
    for key in ("conflicts", "gaps", "ambiguities"):
        if item.get(key):
            compact[key] = item[key]
    coverage = compact_coverage(item.get("coverage"))
    if coverage and (item.get("status") != "complete" or coverage.get("unsearched")):
        compact["coverage"] = coverage
    return compact


def compact_read_data(data: dict[str, Any]) -> dict[str, Any]:
    compact = compact_generic_data(data, ("items",))
    return compact


def compact_verify_data(data: dict[str, Any]) -> dict[str, Any]:
    keys = ("results",) if isinstance(data.get("results"), list) else ("claims",)
    return compact_generic_data(data, keys)


def compact_generic_data(data: dict[str, Any], keys: tuple[str, ...]) -> dict[str, Any]:
    compact = {
        key: data[key]
        for key in keys
        if key in data and data[key] not in (None, [], {})
    }
    projection = compact_projection(data.get("projection"))
    if projection:
        compact["projection"] = projection
    return compact


def compact_candidate(candidate: dict[str, Any]) -> dict[str, Any]:
    return {
        key: candidate[key]
        for key in (
            "reference",
            "source_ref",
            "path",
            "heading",
            "content",
            "source_version",
            "authority",
            "canonicality",
            "recorded_at",
            "valid_time",
            "why_selected",
        )
        if key in candidate and candidate[key] not in (None, [], {})
    }


def compact_projection(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    return {
        key: value[key]
        for key in (
            "policy_ref",
            "policy_version",
            "audience",
            "purpose",
            "output_hash",
            "audit_receipt",
            "withheld",
            "transforms",
        )
        if key in value and value[key] not in (None, [], {})
    }


def compact_freshness(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    return {
        key: value[key]
        for key in ("source_updated_at", "lexical_index_updated_at", "semantic_index_updated_at")
        if value.get(key) is not None
    }


def compact_coverage(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    compact: dict[str, Any] = {"absence_safe": bool(value.get("absence_safe", False))}
    for partition in ("searched", "unsearched"):
        rows = value.get(partition)
        if not isinstance(rows, list) or not rows:
            continue
        compact[partition] = [
            {
                key: row[key]
                for key in ("lane", "completeness", "candidate_count", "failure_reason")
                if key in row and row[key] is not None
            }
            for row in rows
            if isinstance(row, dict)
        ]
    return compact


def operation_request(args: argparse.Namespace, state: dict[str, Any]) -> tuple[str, str, dict[str, Any] | None]:
    if args.command in {"open", "resume"}:
        checkpoint_id = state.get("checkpoint_id") or args.checkpoint_id
        payload: dict[str, Any] = {
            "task": args.task_file.read_text(encoding="utf-8"),
            "hints": {
                "authorization_scope": args.authorization_scope,
                "root_refs": [],
                "open_object_refs": [],
            },
            "as_of": "latest",
            "mode": "continuation",
        }
        if checkpoint_id:
            payload["resume_checkpoint_ref"] = checkpoint_id
        return "POST", "/v1/memory/open", payload

    session_id = require_session(state)
    if args.command == "status":
        return "GET", f"/v1/sessions/{session_id}", None

    if args.command == "query":
        if args.batch:
            if args.payload:
                raise ValueError("query accepts either a payload or --batch, not both")
            queries = []
            for index, query in enumerate(args.batch):
                spec = {
                        "id": f"q{index}",
                        "goal": args.goal,
                        "query": display_scope_query(args, query),
                        "scope": query_scope(args),
                        "limit": args.limit,
                }
                if args.mode:
                    spec["modes"] = args.mode
                queries.append(spec)
            payload = {"queries": queries}
        else:
            payload = optional_json(args.payload)
        if payload is None:
            if not args.payload:
                raise ValueError("query requires text or a JSON payload")
            spec = {
                    "id": "q0",
                    "goal": args.goal,
                    "query": display_scope_query(args, args.payload),
                    "scope": query_scope(args),
                    "limit": args.limit,
            }
            if args.mode:
                spec["modes"] = args.mode
            payload = {"queries": [spec]}
        payload["session_id"] = session_id
        return "POST", "/v1/memory/query", payload

    if args.command == "read":
        payload = optional_json(args.payload)
        if payload is None:
            references = [
                *((reference, False) for reference in (args.reference or [])),
                *((path, True) for path in (args.path or [])),
            ]
            if not references:
                raise ValueError("read requires JSON or --ref/--path")
            range_start, range_end = parse_read_range(args.range_spec)
            start = args.start if args.start is not None else range_start
            end = args.end if args.end is not None else range_end
            requests = []
            for reference, is_path in references:
                view = args.view
                if view is None:
                    if start is not None or end is not None:
                        view = "range"
                    elif args.neighbors is not None and not is_path:
                        view = "neighbors"
                    else:
                        view = "full"
                request: dict[str, Any] = {
                    "ref": reference,
                    "view": view,
                    "max_chars": args.max_chars,
                }
                if start is not None:
                    request["start"] = start
                if end is not None:
                    request["end"] = end
                if view == "neighbors":
                    before = args.before if args.before is not None else args.neighbors
                    after = args.after if args.after is not None else args.neighbors
                    if before is not None:
                        request["before"] = before
                    if after is not None:
                        request["after"] = after
                requests.append(request)
            payload = {"requests": requests}
        payload["session_id"] = session_id
        return "POST", "/v1/memory/read", payload

    if args.command == "compute":
        payload = optional_json(args.payload)
        if payload is None:
            if not args.payload:
                raise ValueError("compute requires a typed JSON plan or expression")
            payload = {
                "steps": [{
                    "id": "compute-1",
                    "op": "aggregate",
                    "input": {"expression": args.payload},
                }],
            }
        payload["session_id"] = session_id
        return "POST", "/v1/memory/compute", payload

    if args.command == "verify":
        payload = optional_json(args.payload)
        if payload is None:
            if not args.payload:
                raise ValueError("verify requires a claim or JSON payload")
            payload = {
                "claims": [{"id": "claim-1", "claim": args.payload}],
                "check_for": args.check_for,
            }
        payload["session_id"] = session_id
        return "POST", "/v1/memory/verify", payload

    if args.command == "checkpoint":
        if args.payload and args.json_payload:
            raise ValueError("checkpoint accepts either positional JSON or --json, not both")
        payload = optional_json(args.json_payload or args.payload)
        if payload is not None and "state" not in payload:
            state_keys = {
                "objective",
                "current_state",
                "decisions",
                "open_questions",
                "next_actions",
                "artifacts",
            }
            state_payload = {
                key: payload.pop(key)
                for key in list(payload)
                if key in state_keys
            }
            if state_payload:
                payload["state"] = state_payload
        if payload is None:
            if not args.objective:
                raise ValueError("checkpoint requires JSON or --objective")
            payload = {
                "state": {
                    "objective": args.objective,
                    "current_state": args.current_state,
                    "decisions": args.decision,
                    "open_questions": args.open_question,
                    "next_actions": args.next_action,
                    "artifacts": args.artifact,
                },
                "source_refs": args.source_ref,
            }
        payload["session_id"] = session_id
        payload.setdefault("parent_checkpoint_id", args.checkpoint_id or state.get("checkpoint_id"))
        payload.setdefault("idempotency_key", checkpoint_key(session_id, payload))
        return "POST", "/v1/memory/checkpoint", payload

    if args.command == "save":
        if not args.payload:
            raise ValueError("save requires one JSON payload")
        return "POST", "/v1/memory/save", load_json_argument(args.payload)

    raise ValueError(f"unsupported operation: {args.command}")


def checkpoint_key(session_id: str, payload: dict[str, Any]) -> str:
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return f"eval-checkpoint:{session_id}:{hashlib.sha256(canonical.encode()).hexdigest()}"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Native Straylight evaluation memory adapter")
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--task-file", type=Path, required=True)
    parser.add_argument("--scope", required=True)
    parser.add_argument("--authorization-scope", required=True)
    parser.add_argument("--checkpoint-id")
    parser.add_argument("--run-id")
    parser.add_argument("--case-id")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("open")
    subparsers.add_parser("resume")

    query = subparsers.add_parser("query")
    query.add_argument("payload", nargs="?")
    query.add_argument("--scope", dest="query_scope")
    query.add_argument("--goal")
    query.add_argument("--mode", action="append")
    query.add_argument("--limit", type=int, default=8)
    query.add_argument("--batch", nargs="+")

    read = subparsers.add_parser("read")
    read.add_argument("payload", nargs="?")
    read.add_argument("--ref", dest="reference", action="append")
    read.add_argument("--path", action="append")
    read.add_argument(
        "--view",
        choices=["full", "range", "outline", "neighbors", "structured"],
    )
    read.add_argument("--start", type=int)
    read.add_argument("--end", type=int)
    read.add_argument("--range", dest="range_spec")
    read.add_argument("--before", type=int)
    read.add_argument("--after", type=int)
    read.add_argument(
        "--neighbors",
        type=int,
        help="shortcut for symmetric neighbor reads on chunk refs; exact paths remain full reads",
    )
    read.add_argument("--max-chars", type=int, default=20_000)

    compute = subparsers.add_parser("compute")
    compute.add_argument("payload", nargs="?")

    verify = subparsers.add_parser("verify")
    verify.add_argument("payload", nargs="?")
    verify.add_argument(
        "--check-for",
        action="append",
        default=["contradictions", "superseded_sources", "unsupported_claims", "temporal_ambiguity"],
    )

    checkpoint = subparsers.add_parser("checkpoint")
    checkpoint.add_argument("payload", nargs="?")
    checkpoint.add_argument("--json", dest="json_payload")
    checkpoint.add_argument("--scope", dest="checkpoint_scope")
    checkpoint.add_argument("--objective")
    checkpoint.add_argument("--current-state", action="append", default=[])
    checkpoint.add_argument("--decision", action="append", default=[])
    checkpoint.add_argument("--open-question", action="append", default=[])
    checkpoint.add_argument("--next-action", action="append", default=[])
    checkpoint.add_argument("--artifact", action="append", default=[])
    checkpoint.add_argument("--source-ref", action="append", default=[])

    save = subparsers.add_parser("save")
    save.add_argument("payload")
    subparsers.add_parser("status")
    return parser


def error_code(body: dict[str, Any]) -> str | None:
    error = body.get("error")
    if isinstance(error, dict):
        return error.get("code")
    return str(error) if error is not None else None


def execute_locked(args: argparse.Namespace) -> tuple[str, int]:
    with locked_state(args.state) as state:
        return execute_with_state(args, state)


def execute_with_state(args: argparse.Namespace, state: dict[str, Any]) -> tuple[str, int]:
    if args.command in {"open", "resume"} and state.get("session_id"):
        return json.dumps({
            "status": "already_open",
            "session_id": state["session_id"],
            "corpus_revision": state.get("corpus_revision"),
            "message": "This adapter state is already open; no service call was made.",
        }, indent=2), 0
    try:
        method, path, payload = operation_request(args, state)
        client = NativeApiClient(run_id=args.run_id, case_id=args.case_id)
        response = client.request(method, path, payload)
        rendered = render_native_response(args.command, response)
        record_state(
            args.state,
            state,
            args.command,
            result_chars=len(rendered),
            elapsed_ms=response.elapsed_ms,
            http_status=response.http_status,
            request_id=str(response.body.get("request_id") or response.headers.get("x-request-id") or "") or None,
            service_status=str(response.body.get("status") or "") or None,
            response=response,
        )
        return rendered, 0
    except NativeApiError as exc:
        rendered = json.dumps(exc.body, indent=2, ensure_ascii=False)
        record_state(
            args.state,
            state,
            f"denied:{args.command}" if exc.status == 403 else f"failed:{args.command}",
            result_chars=len(rendered),
            elapsed_ms=exc.elapsed_ms,
            http_status=exc.status,
            request_id=None,
            service_status=error_code(exc.body),
        )
        exit_code = 77 if exc.status == 403 or error_code(exc.body) == "capability_denied" else 1
        return rendered, exit_code
    except (ValueError, json.JSONDecodeError, OSError) as exc:
        body = {"error": {"code": "invalid_request", "message": str(exc)}}
        rendered = json.dumps(body, indent=2)
        record_state(
            args.state,
            state,
            f"failed:{args.command}",
            result_chars=len(rendered),
            elapsed_ms=0,
            http_status=0,
            request_id=None,
            service_status="invalid_request",
        )
        return rendered, 2


def main(argv: list[str] | None = None) -> None:
    args = build_parser().parse_args(argv)
    marker = initialization_marker(args.state)
    is_initializer = args.command in {"open", "resume"}
    if is_initializer:
        marker.write_text(str(os.getpid()), encoding="ascii")
    else:
        wait_for_initializing_peer(args.state)
    try:
        rendered, exit_code = execute_locked(args)
    finally:
        if is_initializer:
            marker.unlink(missing_ok=True)
    print(rendered)
    if exit_code:
        raise SystemExit(exit_code)


if __name__ == "__main__":
    main(sys.argv[1:])

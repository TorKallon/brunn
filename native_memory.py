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
import urllib.parse
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
    result_metrics: dict[str, Any] | None = None,
) -> dict[str, Any]:
    operation_record = {
        "at": datetime.now().astimezone().isoformat(timespec="milliseconds"),
        "operation": operation,
        "http_status": http_status,
        "service_status": service_status,
        "request_id": request_id,
        "elapsed_ms": round(elapsed_ms, 3),
        "result_chars": result_chars,
    }
    if result_metrics:
        operation_record.update(result_metrics)
    state["operations"].append(operation_record)
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


def response_payload_metrics(rendered: str) -> dict[str, Any]:
    try:
        body = json.loads(rendered)
    except json.JSONDecodeError:
        return {}
    source_text_chars = 0
    evidence_items = 0
    complete_sources = 0
    pointer_sources = 0
    source_paths: set[str] = set()

    def visit(value: Any, parent_key: str | None = None) -> None:
        nonlocal source_text_chars, evidence_items, complete_sources, pointer_sources
        if isinstance(value, dict):
            scope = value.get("content_scope")
            if scope == "complete_source":
                complete_sources += 1
            elif scope == "source_lead":
                pointer_sources += 1
            if isinstance(value.get("content"), str):
                source_text_chars += len(value["content"])
                evidence_items += 1
            if isinstance(value.get("text"), str):
                source_text_chars += len(value["text"])
                evidence_items += 1
            for key, child in value.items():
                if (
                    key == "path"
                    and isinstance(child, str)
                    and child
                    and not Path(child).is_absolute()
                ):
                    source_paths.add(child.removeprefix("./"))
                if key not in {"content", "text"}:
                    visit(child, key)
        elif isinstance(value, list):
            for child in value:
                visit(child, parent_key)

    visit(body)
    sufficiency = (
        body.get("data", {}).get("retrieval_sufficiency")
        if isinstance(body, dict) and isinstance(body.get("data"), dict)
        else None
    )
    result = {
        "source_text_chars": source_text_chars,
        "metadata_chars": max(0, len(rendered) - source_text_chars),
        "evidence_items": evidence_items,
        "complete_sources": complete_sources,
        "pointer_sources": pointer_sources,
        "source_paths": sorted(source_paths),
        "sufficiency_status": (
            sufficiency.get("status") if isinstance(sufficiency, dict) else None
        ),
    }
    asset = body.get("data", body) if isinstance(body, dict) else {}
    if isinstance(asset, dict) and isinstance(asset.get("asset_ref"), str):
        result.update({
            "asset_ref": asset["asset_ref"],
            "asset_version": asset.get("version"),
            "asset_content_hash": asset.get("content_hash"),
            "asset_size_bytes": asset.get("size_bytes"),
        })
    return result


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


REASONING_COMMANDS = {
    "open",
    "resume",
    "query",
    "read",
    "compute",
    "verify",
    "checkpoint",
}
OPEN_TEXT_SOURCE_LIMIT = 12
OPEN_TEXT_TOTAL_CHARS = 32_000


def render_json(value: Any, *, pretty: bool) -> str:
    if pretty:
        return json.dumps(value, indent=2, ensure_ascii=False)
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def render_native_response(
    command: str,
    response: NativeResponse,
    *,
    pretty: bool = False,
) -> str:
    body = (
        compact_reasoning_response(command, response.body)
        if command in REASONING_COMMANDS
        else response.body
    )
    return render_json(body, pretty=pretty)


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
    elif command == "checkpoint":
        compact_data = compact_checkpoint_data(data)
    else:
        compact_data = compact_generic_data(data, ("steps", "rows_returned", "estimated_tokens"))
    compact["data"] = compact_data
    return compact


def compact_open_data(data: dict[str, Any]) -> dict[str, Any]:
    compact = compact_generic_data(
        data,
        (
            "resume_checkpoint",
            "revision_delta",
            "initial_case_file",
        ),
    )
    resolved_scope = compact_resolved_scope(data.get("resolved_scope"))
    if resolved_scope:
        compact["resolved_scope"] = resolved_scope
    learned_context = compact_learned_context(data.get("learned_context"))
    if learned_context:
        compact["learned_context"] = learned_context

    evidence = data.get("initial_evidence")
    evidence_refs_by_source = collect_evidence_refs_by_source(evidence)
    hydrated = data.get("hydrated_sources")
    represented_sources: set[str] = set()
    text_sources: list[dict[str, Any]] = []
    leads: list[dict[str, Any]] = []
    if isinstance(hydrated, list) and hydrated:
        source_rows = [item for item in hydrated if isinstance(item, dict)]
        text_sources, leads = partition_open_sources(source_rows)
        compact["initial_evidence"] = [
            compact_hydrated_source(
                item,
                include_text=True,
                evidence_refs=evidence_refs_for_source(
                    item,
                    evidence_refs_by_source,
                ),
            )
            for item in text_sources
        ]
        represented_sources = {
            str(item.get("source_ref") or item.get("path") or "")
            for item in source_rows
        }
        if leads:
            compact["evidence_leads"] = [
                compact_hydrated_source(
                    item,
                    include_text=False,
                    evidence_refs=evidence_refs_for_source(
                        item,
                        evidence_refs_by_source,
                    ),
                )
                for item in leads
            ]

    if isinstance(evidence, list) and evidence:
        remaining = [
            compact_candidate(item)
            for item in evidence
            if isinstance(item, dict)
            and str(item.get("source_ref") or item.get("path") or "") not in represented_sources
        ]
        if remaining:
            compact.setdefault("initial_evidence", []).extend(remaining)

    sufficiency = data.get("retrieval_sufficiency")
    if isinstance(sufficiency, dict):
        compact["retrieval_sufficiency"] = {
            **sufficiency,
            "returned_text_sources": len(text_sources) if isinstance(hydrated, list) else 0,
            "pointer_only_sources": len(leads) if isinstance(hydrated, list) else 0,
        }
    corpus_map = data.get("corpus_map")
    if isinstance(corpus_map, dict):
        compact["corpus_map"] = {
            key: corpus_map[key]
            for key in ("record_counts", "profile_counts", "available_views", "truncated")
            if key in corpus_map
        }
    return compact


def partition_open_sources(
    sources: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    text_sources: list[dict[str, Any]] = []
    leads: list[dict[str, Any]] = []
    text_chars = 0
    for source in sources:
        text = source.get("text")
        source_chars = len(text) if isinstance(text, str) else 0
        within_count = len(text_sources) < OPEN_TEXT_SOURCE_LIMIT
        within_budget = text_chars + source_chars <= OPEN_TEXT_TOTAL_CHARS
        if within_count and (within_budget or not text_sources):
            text_sources.append(source)
            text_chars += source_chars
        else:
            leads.append(source)
    return text_sources, leads


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
    compact = compact_generic_data(data, ())
    items = data.get("items")
    if isinstance(items, list):
        compact["items"] = [
            compact_read_item(item) for item in items if isinstance(item, dict)
        ]
    return compact


def compact_read_item(item: dict[str, Any]) -> dict[str, Any]:
    compact = {
        key: item[key]
        for key in ("reference", "view", "status")
        if item.get(key) is not None
    }
    error = item.get("error")
    if isinstance(error, dict) and error:
        compact["error"] = error
    data = item.get("data")
    if isinstance(data, dict) and "text" in data:
        metadata = data.get("metadata")
        metadata = metadata if isinstance(metadata, dict) else {}
        locator = metadata.get("native_locator")
        locator = locator if isinstance(locator, dict) else {}
        source = {
            key: value
            for key, value in {
                "reference": metadata.get("ref"),
                "path": locator.get("path") or metadata.get("source_ref"),
                "title": metadata.get("title"),
                "version": metadata.get("source_version") or metadata.get("version"),
                "media_type": metadata.get("media_type"),
                "representation": metadata.get("representation"),
            }.items()
            if value not in (None, "", [], {})
        }
        if source:
            compact["source"] = source
        range_value = data.get("range")
        if isinstance(range_value, dict):
            compact["lines"] = [
                range_value.get("start_line"),
                range_value.get("end_line"),
            ]
        compact["text"] = data.get("text", "")
    elif isinstance(data, dict) and isinstance(data.get("chunks"), list):
        compact["anchor_reference"] = data.get("anchor_ref")
        compact["chunks"] = [
            compact_neighbor_chunk(chunk)
            for chunk in data["chunks"]
            if isinstance(chunk, dict)
        ]
    elif data not in (None, {}, []):
        compact["data"] = data

    truncation = item.get("truncation")
    if isinstance(truncation, dict) and truncation.get("truncated"):
        compact["truncation"] = {
            key: truncation[key]
            for key in ("truncated", "continuation_token")
            if truncation.get(key) is not None
        }
    return compact


def compact_neighbor_chunk(chunk: dict[str, Any]) -> dict[str, Any]:
    locator = chunk.get("locator")
    locator = locator if isinstance(locator, dict) else {}
    compact = {
        key: chunk[key]
        for key in ("ref", "ordinal", "heading_path", "content", "is_anchor")
        if chunk.get(key) not in (None, [], {})
    }
    if locator:
        compact["lines"] = [locator.get("start_line"), locator.get("end_line")]
    return compact


def compact_verify_data(data: dict[str, Any]) -> dict[str, Any]:
    keys = ("results",) if isinstance(data.get("results"), list) else ("claims",)
    return compact_generic_data(data, keys)


def compact_checkpoint_data(data: dict[str, Any]) -> dict[str, Any]:
    compact = {
        key: data[key]
        for key in (
            "checkpoint_id",
            "id",
            "base_corpus_revision",
            "resulting_corpus_revision",
            "receipt",
            "replayed",
            "review_required",
            "search_status",
            "indexing",
        )
        if key in data and data[key] not in (None, [], {})
    }
    parent_checkpoint = None
    source_refs: list[str] = []
    for item in data.get("items", []) if isinstance(data.get("items"), list) else []:
        if not isinstance(item, dict):
            continue
        details = item.get("details")
        if not isinstance(details, dict):
            continue
        parent_checkpoint = parent_checkpoint or details.get("parent_checkpoint_id")
        for source_ref in details.get("source_refs", []):
            if isinstance(source_ref, str) and source_ref not in source_refs:
                source_refs.append(source_ref)
    if parent_checkpoint:
        compact["parent_checkpoint_id"] = parent_checkpoint
    if source_refs:
        compact["source_refs"] = source_refs
    return compact


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
    compact = {
        key: candidate[key]
        for key in (
            "reference",
            "path",
            "heading",
            "content",
            "authority",
            "canonicality",
            "recorded_at",
            "valid_time",
        )
        if key in candidate and candidate[key] not in (None, [], {})
    }
    source_ref = candidate.get("source_ref")
    if source_ref and source_ref != candidate.get("path"):
        compact["source_ref"] = source_ref
    evidence_refs = candidate.get("evidence_refs")
    if isinstance(evidence_refs, list):
        compact["evidence_refs"] = list(dict.fromkeys(
            value for value in evidence_refs if isinstance(value, str) and value
        ))
    return compact


def compact_hydrated_source(
    source: dict[str, Any],
    *,
    include_text: bool,
    evidence_refs: list[str] | None = None,
) -> dict[str, Any]:
    selected = source.get("selected_references")
    selected = selected if isinstance(selected, list) else []
    compact = {
        key: source[key]
        for key in (
            "path",
            "title",
            "authority",
            "canonicality",
            "recorded_at",
        )
        if key in source and source[key] not in (None, [], {})
    }
    source_ref = source.get("source_ref")
    if source_ref and source_ref != source.get("path"):
        compact["source_ref"] = source_ref
    if evidence_refs:
        compact["evidence_refs"] = evidence_refs
    if selected and not source.get("complete"):
        compact["reference"] = selected[0]
    ranges = source.get("ranges")
    if not source.get("complete") and isinstance(ranges, list) and ranges:
        compact["ranges"] = [
            compact_source_range(item) for item in ranges if isinstance(item, dict)
        ]
    if include_text:
        compact["content"] = source.get("text", "")
        compact["content_scope"] = (
            "complete_source" if source.get("complete") else "selected_source_sections"
        )
    else:
        compact["content_scope"] = "source_lead"
    return compact


def collect_evidence_refs_by_source(value: Any) -> dict[str, list[str]]:
    by_source: dict[str, list[str]] = {}
    if not isinstance(value, list):
        return by_source
    for candidate in value:
        if not isinstance(candidate, dict):
            continue
        keys = {
            str(candidate.get(key))
            for key in ("source_ref", "path")
            if candidate.get(key)
        }
        refs = [
            ref
            for ref in candidate.get("evidence_refs", [])
            if isinstance(ref, str) and ref
        ]
        for key in keys:
            existing = by_source.setdefault(key, [])
            existing.extend(ref for ref in refs if ref not in existing)
    return by_source


def evidence_refs_for_source(
    source: dict[str, Any],
    by_source: dict[str, list[str]],
) -> list[str]:
    refs: list[str] = []
    for key in ("source_ref", "path"):
        value = source.get(key)
        if not value:
            continue
        refs.extend(ref for ref in by_source.get(str(value), []) if ref not in refs)
    return refs


def compact_source_range(value: dict[str, Any]) -> list[int | None]:
    return [value.get("start_line"), value.get("end_line")]


def compact_resolved_scope(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    return {
        key: value[key]
        for key in (
            "authorization_scope",
            "root_objects",
            "requested_lenses",
            "ambiguities",
        )
        if value.get(key) not in (None, [], {})
    }


def compact_learned_context(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    items = value.get("items")
    if not isinstance(items, list) or not items:
        return {
            key: value[key]
            for key in ("status", "reason")
            if value.get(key) not in (None, "")
        }
    return {
        key: value[key]
        for key in (
            "status",
            "authority",
            "pinned_corpus_revision",
            "items",
            "latest_candidate",
        )
        if value.get(key) not in (None, [], {})
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
        compact_rows = [
            {
                key: row[key]
                for key in ("lane", "completeness", "candidate_count", "failure_reason")
                if key in row and row[key] is not None
            }
            for row in rows
            if isinstance(row, dict)
            and (
                row.get("candidate_count", 0) > 0
                or row.get("failure_reason")
                or row.get("completeness")
                in {"degraded", "failed", "partial", "unsearched"}
            )
        ]
        if compact_rows:
            compact[partition] = compact_rows
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

    if args.command == "assets":
        query = urllib.parse.urlencode({
            "session_id": session_id,
            "offset": args.offset,
            "limit": args.limit,
        })
        return "GET", f"/v1/assets?{query}", None

    if args.command == "asset-metadata":
        query = urllib.parse.urlencode({"session_id": session_id})
        asset_ref = urllib.parse.quote(args.asset_ref, safe="")
        return "GET", f"/v1/assets/{asset_ref}?{query}", None

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
    parser.add_argument(
        "--pretty",
        action="store_true",
        help="pretty-print responses for a human instead of the compact agent transport",
    )
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
    assets = subparsers.add_parser("assets")
    assets.add_argument("--offset", type=int, default=0)
    assets.add_argument("--limit", type=int, default=100)
    metadata = subparsers.add_parser("asset-metadata")
    metadata.add_argument("asset_ref")
    fetch = subparsers.add_parser("asset-fetch")
    fetch.add_argument("asset_ref")
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
        return render_json({
            "status": "already_open",
            "session_id": state["session_id"],
            "corpus_revision": state.get("corpus_revision"),
            "message": "This adapter state is already open; no service call was made.",
        }, pretty=args.pretty), 0
    try:
        if args.command == "asset-fetch":
            session_id = require_session(state)
            client = NativeApiClient(run_id=args.run_id, case_id=args.case_id)
            encoded_ref = urllib.parse.quote(args.asset_ref, safe="")
            query = urllib.parse.urlencode({"session_id": session_id})
            metadata = client.get(f"/v1/assets/{encoded_ref}?{query}")
            data = metadata.data
            if data.get("asset_ref") != args.asset_ref:
                raise RuntimeError("asset metadata returned an unexpected reference")
            version = int(data["version"])
            expected_hash = str(data["content_hash"])
            expected_size = int(data["size_bytes"])
            original_path = str(data.get("path") or "")
            suffix = Path(original_path).suffix
            if not re.fullmatch(r"\.[A-Za-z0-9]{1,12}", suffix):
                suffix = ""
            asset_id = args.asset_ref.removeprefix("asset:")
            destination = (
                args.state.parent
                / "assets"
                / f"{asset_id}.v{version}{suffix.casefold()}"
            )
            downloaded = client.download_verified(
                f"/v1/assets/{encoded_ref}/versions/{version}/content?{query}",
                destination,
                expected_hash=expected_hash,
                expected_size=expected_size,
            )
            body = {
                "status": "complete",
                "asset_ref": args.asset_ref,
                "version": version,
                "local_path": str(downloaded.path),
                "content_hash": downloaded.content_hash,
                "size_bytes": downloaded.size_bytes,
                "media_type": downloaded.media_type,
            }
            rendered = render_json(body, pretty=args.pretty)
            record_state(
                args.state,
                state,
                args.command,
                result_chars=len(rendered),
                elapsed_ms=metadata.elapsed_ms + downloaded.elapsed_ms,
                http_status=200,
                request_id=None,
                service_status="complete",
                response=metadata,
                result_metrics={
                    **response_payload_metrics(rendered),
                    "binary_bytes": downloaded.size_bytes,
                    "http_calls": 2,
                    "asset_ref": args.asset_ref,
                    "asset_version": version,
                    "asset_content_hash": downloaded.content_hash,
                    "asset_size_bytes": downloaded.size_bytes,
                    "asset_local_path": str(downloaded.path),
                },
            )
            return rendered, 0
        method, path, payload = operation_request(args, state)
        client = NativeApiClient(run_id=args.run_id, case_id=args.case_id)
        response = client.request(method, path, payload)
        rendered = render_native_response(args.command, response, pretty=args.pretty)
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
            result_metrics=response_payload_metrics(rendered),
        )
        return rendered, 0
    except NativeApiError as exc:
        rendered = render_json(exc.body, pretty=args.pretty)
        record_state(
            args.state,
            state,
            f"denied:{args.command}" if exc.status == 403 else f"failed:{args.command}",
            result_chars=len(rendered),
            elapsed_ms=exc.elapsed_ms,
            http_status=exc.status,
            request_id=None,
            service_status=error_code(exc.body),
            result_metrics=response_payload_metrics(rendered),
        )
        exit_code = 77 if exc.status == 403 or error_code(exc.body) == "capability_denied" else 1
        return rendered, exit_code
    except (ValueError, RuntimeError, json.JSONDecodeError, OSError) as exc:
        body = {"error": {"code": "invalid_request", "message": str(exc)}}
        rendered = render_json(body, pretty=args.pretty)
        record_state(
            args.state,
            state,
            f"failed:{args.command}",
            result_chars=len(rendered),
            elapsed_ms=0,
            http_status=0,
            request_id=None,
            service_status="invalid_request",
            result_metrics=response_payload_metrics(rendered),
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

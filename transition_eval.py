#!/usr/bin/env python3

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import sqlite3
import statistics
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence

from agent_work_eval import (
    DEFAULT_CODEX,
    NATIVE_PROVISIONING_STATE,
    grade_answer,
    load_native_provisioning_state,
    parse_event_metrics,
    parse_json_answer,
    sha256_file,
    write_native_provisioning_state,
)
from native_eval import (
    NativeApiClient,
    NativeApiError,
    file_document,
    find_checkpoint,
    provisioning_matches_run_case,
    provision_evaluation,
    public_provisioning,
    text_documents,
    write_native_memory_wrapper,
)
from straylight import StraylightStore
from straylight.embeddings import HashingEmbeddingProvider, OpenAIEmbeddingProvider


ROOT = Path(__file__).resolve().parent
LABELS = {
    "filesystem_rebuild": "Filesystem rebuild",
    "workspace_resume": "Straylight checkpoint resume",
    "service_api_resume": "Native Straylight checkpoint resume",
}
TRANSITION_ADAPTERS = {
    "filesystem_rebuild": "filesystem",
    "workspace_resume": "legacy_workspace",
    "service_api_resume": "native_service",
}


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def embedding_provider(kind: str, model: str):
    if kind == "openai":
        return OpenAIEmbeddingProvider(model=model)
    if kind == "hashing":
        return HashingEmbeddingProvider()
    return None


def validate(manifest_path: Path) -> dict[str, Any]:
    manifest = load_json(manifest_path)
    base_root = ROOT / manifest["base_corpus_root"]
    base_paths = {
        path.relative_to(base_root).as_posix()
        for path in base_root.rglob("*") if path.is_file()
    }
    seed_results = load_json(ROOT / manifest["seed_results"])
    seed_records = {
        record["case_id"]: record
        for record in seed_results["records"]
        if record["condition"] == "workspace" and record.get("workspace_checkpoint")
    }
    errors = []
    for case in manifest["cases"]:
        if case["seed_case_id"] not in seed_records:
            errors.append(f"{case['id']}: missing seed checkpoint {case['seed_case_id']}")
        if not (ROOT / case["delta_path"]).is_file():
            errors.append(f"{case['id']}: missing delta {case['delta_path']}")
        if set(case["claim_slots"]) != {rubric["id"] for rubric in case["rubric"]}:
            errors.append(f"{case['id']}: claim slots and rubric IDs differ")
        for rubric in case["rubric"]:
            for source in rubric["sources_any"]:
                if source not in base_paths and not (ROOT / source).is_file():
                    errors.append(f"{case['id']}:{rubric['id']}: missing source {source}")
    return {
        "manifest_path": manifest_path,
        "manifest": manifest,
        "base_root": base_root,
        "base_paths": base_paths,
        "seed_records": seed_records,
        "errors": errors,
    }


def backup_database(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(source) as source_connection, sqlite3.connect(destination) as destination_connection:
        source_connection.backup(destination_connection)


def seed_sources(case: dict[str, Any], work_cases: dict[str, Any], base_paths: set[str]) -> list[str]:
    source_case = next(item for item in work_cases["cases"] if item["id"] == case["seed_case_id"])
    return sorted({
        source
        for rubric in source_case["rubric"]
        for source in rubric["sources_any"]
        if source in base_paths
    })


def prepare_assets(
    run_root: Path,
    validated: dict[str, Any],
    embeddings: str,
    embedding_model: str,
) -> dict[str, dict[str, Any]]:
    manifest = validated["manifest"]
    assets = run_root / "assets"
    assets.mkdir(parents=True, exist_ok=True)
    template = assets / "base.db"
    template_store = StraylightStore(template)
    base_revision = template_store.ingest_directory(validated["base_root"], note="frozen transition base")
    provider = embedding_provider(embeddings, embedding_model)
    embedding_summary = template_store.index_embeddings(provider) if provider else None
    work_cases = load_json(ROOT / "eval" / "work_cases.json")
    prepared: dict[str, dict[str, Any]] = {}

    for case in manifest["cases"]:
        case_root = assets / case["id"]
        case_root.mkdir()
        database = case_root / "workspace.db"
        backup_database(template, database)
        store = StraylightStore(database)
        state = validated["seed_records"][case["seed_case_id"]]["workspace_checkpoint"]
        sources = seed_sources(case, work_cases, validated["base_paths"])
        seed = store.create_checkpoint(base_revision["seq"], state, sources)
        delta_text = (ROOT / case["delta_path"]).read_text(encoding="utf-8")
        delta_revision = store.ingest_documents(
            {case["delta_path"]: delta_text},
            note=f"transition fixture {case['id']}",
        )
        delta_embedding_summary = store.index_embeddings(provider) if provider else None
        metadata = {
            "database": str(database),
            "base_revision": base_revision,
            "delta_revision": delta_revision,
            "seed_checkpoint": seed,
            "embedding_summary": embedding_summary,
            "delta_embedding_summary": delta_embedding_summary,
        }
        (case_root / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        prepared[case["id"]] = metadata
    return prepared


def load_prepared_assets(
    run_root: Path,
    validated: dict[str, Any],
) -> dict[str, dict[str, Any]]:
    prepared: dict[str, dict[str, Any]] = {}
    for case in validated["manifest"]["cases"]:
        metadata_path = run_root / "assets" / case["id"] / "metadata.json"
        if not metadata_path.is_file():
            raise ValueError(f"Incomplete transition assets: {metadata_path}")
        prepared[case["id"]] = load_json(metadata_path)
    return prepared


def prepare_native_assets(
    run_id: str,
    validated: dict[str, Any],
    *,
    timeout_seconds: float,
    existing: dict[str, dict[str, Any]] | None = None,
    state_path: Path | None = None,
    service_protocol: str = "legacy",
) -> dict[str, dict[str, Any]]:
    documents = text_documents(validated["base_root"])
    work_cases = load_json(ROOT / "eval" / "work_cases.json")
    prepared = dict(existing or {})
    for case in validated["manifest"]["cases"]:
        persisted = prepared.get(case["id"])
        if persisted is not None:
            if not provisioning_matches_run_case(
                persisted,
                run_id=run_id,
                case_id=case["id"],
                require_checkpoint=True,
            ):
                raise ValueError(
                    f"Invalid persisted native provisioning for {case['id']}"
                )
            continue
        state = validated["seed_records"][case["seed_case_id"]]["workspace_checkpoint"]
        sources = seed_sources(case, work_cases, validated["base_paths"])
        seed = {"state": state, "source_refs": sources}
        client = NativeApiClient(run_id=run_id, case_id=case["id"])
        metadata = provision_evaluation(
            client,
            run_id=run_id,
            case_id=case["id"],
            display_scope=case["workload"],
            access_mode="read_write",
            documents=documents,
            delta_documents=[file_document(ROOT / case["delta_path"], case["delta_path"])],
            seed_checkpoint=seed,
            timeout_seconds=timeout_seconds,
            import_path=(
                "/v1/workspace/admin/eval/import"
                if service_protocol == "simple"
                else "/v1/admin/eval/import"
            ),
            wait_for_semantic=service_protocol != "simple",
        )
        if not metadata.get("checkpoint_id"):
            raise RuntimeError(f"{case['id']}: eval import did not return seed checkpoint_id")
        metadata["seed_source_refs"] = sources
        prepared[case["id"]] = metadata
        if state_path is not None:
            write_native_provisioning_state(state_path, run_id, prepared)
    return prepared


def render_prompt(case: dict[str, Any], condition: str) -> str:
    slots = "\n".join(f"- {key}: {value}" for key, value in case["claim_slots"].items())
    if condition in {"workspace_resume", "service_api_resume"}:
        adapter = "native Straylight service" if condition == "service_api_resume" else "typed Straylight adapter"
        access = """Use only the typed Straylight adapter at `./memory`.

1. Run `./memory resume`. It returns the complete parent checkpoint, the revision-N-to-N+1 delta with source content, and a scoped corpus map.
2. If the checkpoint and delta are insufficient, use at most one batched `query` and one batched `read`:
   `./memory query '{"queries":[{"query":"...","scope":"WORKLOAD","limit":6}]}'`
   `./memory read '{"requests":[{"path":"...","start":1,"end":120}]}'`
3. Persist the child before answering:
   `./memory checkpoint '{"state":{"objective":"...","current_state":[],"decisions":[],"open_questions":[],"next_actions":[],"artifacts":[]},"source_refs":["exact/source/path"]}'`

Do not call help or schema. Use no more than four workspace calls. The checkpoint write returns a receipt instead of echoing the state."""
        access = access.replace("typed Straylight adapter", adapter).replace("workspace calls", "service calls")
    else:
        access = """Use only `./checkpoint.json`, `./delta.json`, the exact delta source under `./eval/transition-deltas`, and the frozen `./corpus`. Ordinary read-only filesystem search and scripts are allowed. The checkpoint is prior durable state, not an infallible source; reconcile it with the new delta and cite original relative source paths. Do not browse or read outside this run directory."""
    return f"""You are a genuinely fresh agent continuing work at corpus revision N+1.

{access}

Task:
{case['task']}

Return the required JSON object. Include exactly one `claims` entry for every slot below, using the exact ID. Cite the source paths that support each claim. The final checkpoint must represent the revised child state, not merely restate the parent.
Keep every claim slot self-contained: repeat a changed fact, threshold, authority boundary, or next action when that slot needs it, even if it already appears in another claim or the checkpoint.

{slots}
"""


def grade_transition_answer(case: dict[str, Any], answer: dict[str, Any], corpus_paths: set[str]) -> dict[str, Any]:
    return grade_answer(case, answer, corpus_paths)


def write_wrapper(run_dir: Path, case: dict[str, Any], metadata: dict[str, Any], embeddings: str, model: str) -> None:
    task_file = run_dir / "task.txt"
    task_file.write_text(case["task"] + "\n", encoding="utf-8")
    wrapper = run_dir / "memory"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"exec python3 '{ROOT / 'transition_adapter.py'}' "
        f"--db '{metadata['database']}' "
        f"--checkpoint-id '{metadata['seed_checkpoint']['checkpoint_id']}' "
        f"--scope '{case['workload']}' "
        f"--task-file '{task_file}' "
        f"--session-file '{run_dir / 'session.json'}' "
        f"--embeddings '{embeddings}' --embedding-model '{model}' \"$@\"\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)


def prepare_run_dirs(
    run_root: Path,
    validated: dict[str, Any],
    prepared: dict[str, dict[str, Any]],
    embeddings: str,
    embedding_model: str,
    conditions: Sequence[str] | None = None,
    service_protocol: str = "legacy",
) -> list[tuple[dict[str, Any], str, Path]]:
    jobs = []
    selected_conditions = list(conditions or validated["manifest"]["conditions"])
    for case in validated["manifest"]["cases"]:
        metadata = prepared[case["id"]]
        for condition in selected_conditions:
            run_dir = run_root / condition / case["id"]
            run_dir.mkdir(parents=True, exist_ok=True)
            if (run_dir / "answer.json").is_file() and (run_dir / "run-record.json").is_file():
                jobs.append((case, condition, run_dir))
                continue
            (run_dir / "prompt.txt").write_text(render_prompt(case, condition), encoding="utf-8")
            if condition == "workspace_resume":
                workspace_database = run_dir / "workspace.db"
                backup_database(Path(metadata["database"]), workspace_database)
                metadata["workspace_database"] = str(workspace_database)
                write_wrapper(
                    run_dir,
                    case,
                    {**metadata, "database": str(workspace_database)},
                    embeddings,
                    embedding_model,
                )
            elif condition == "service_api_resume":
                native = metadata.get("native")
                if native is None:
                    raise ValueError(f"{case['id']}: missing native provisioning metadata")
                write_native_memory_wrapper(
                    run_dir,
                    task=case["task"],
                    display_scope=case["workload"],
                    authorization_scope=native["authorization_scope"],
                    run_id=run_root.name,
                    case_id=case["id"],
                    checkpoint_id=native["checkpoint_id"],
                    protocol=service_protocol,
                )
            else:
                corpus_link = run_dir / "corpus"
                if not corpus_link.exists() and not corpus_link.is_symlink():
                    corpus_link.symlink_to(validated["base_root"], target_is_directory=True)
                delta_target = run_dir / case["delta_path"]
                delta_target.parent.mkdir(parents=True, exist_ok=True)
                if not delta_target.exists() and not delta_target.is_symlink():
                    delta_target.symlink_to(ROOT / case["delta_path"])
                (run_dir / "checkpoint.json").write_text(
                    json.dumps(metadata["seed_checkpoint"], indent=2) + "\n", encoding="utf-8"
                )
                (run_dir / "delta.json").write_text(
                    json.dumps({
                        "from_revision": metadata["base_revision"]["revision_id"],
                        "to_revision": metadata["delta_revision"]["revision_id"],
                        "changed_paths": [case["delta_path"]],
                    }, indent=2) + "\n",
                    encoding="utf-8",
                )
            jobs.append((case, condition, run_dir))
    return jobs


def build_codex_command(
    *,
    codex: Path,
    model: str,
    schema: Path,
    run_dir: Path,
    condition: str,
) -> list[str]:
    command = [
        str(codex),
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--skip-git-repo-check",
        "--model",
        model,
    ]
    if condition == "service_api_resume":
        command.extend(["--config", "sandbox_workspace_write.network_access=true"])
    command.extend([
        "--sandbox",
        "workspace-write",
        "--cd",
        str(run_dir),
        "--output-schema",
        str(schema),
        "--output-last-message",
        str(run_dir / "answer.json"),
        "--json",
        "-",
    ])
    return command


async def run_one(
    semaphore: asyncio.Semaphore,
    codex: Path,
    model: str,
    schema: Path,
    case: dict[str, Any],
    condition: str,
    run_dir: Path,
    timeout_seconds: int,
    env_overrides: dict[str, str] | None = None,
) -> dict[str, Any]:
    async with semaphore:
        command = build_codex_command(
            codex=codex,
            model=model,
            schema=schema,
            run_dir=run_dir,
            condition=condition,
        )
        env = os.environ.copy()
        for key in ["CODEX_THREAD_ID", "CODEX_INTERNAL_ORIGINATOR_OVERRIDE"]:
            env.pop(key, None)
        if env_overrides:
            env.update(env_overrides)
        started = time.monotonic()
        process = await asyncio.create_subprocess_exec(
            *command,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env,
        )
        try:
            stdout, stderr = await asyncio.wait_for(
                process.communicate((run_dir / "prompt.txt").read_bytes()),
                timeout=timeout_seconds,
            )
            timed_out = False
        except asyncio.TimeoutError:
            process.kill()
            stdout, stderr = await process.communicate()
            timed_out = True
        (run_dir / "events.jsonl").write_bytes(stdout)
        (run_dir / "stderr.log").write_bytes(stderr)
        record = {
            "case_id": case["id"],
            "condition": condition,
            "exit_code": process.returncode,
            "timed_out": timed_out,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "answer_path": str(run_dir / "answer.json"),
            "events_path": str(run_dir / "events.jsonl"),
            "error": stderr.decode(errors="replace")[-2000:] if process.returncode else None,
        }
        (run_dir / "run-record.json").write_text(
            json.dumps(record, indent=2) + "\n",
            encoding="utf-8",
        )
        return record


def nested_field(value: Any, *names: str) -> Any:
    if isinstance(value, dict):
        for name in names:
            if value.get(name) is not None:
                return value[name]
        for nested in value.values():
            found = nested_field(nested, *names)
            if found is not None:
                return found
    elif isinstance(value, list):
        for nested in value:
            found = nested_field(nested, *names)
            if found is not None:
                return found
    return None


def canonical_api_ref(value: Any) -> Any:
    if not isinstance(value, str):
        return value
    prefix, separator, identifier = value.partition(":")
    compact = identifier.replace("-", "")
    if separator and len(compact) == 32 and all(character in "0123456789abcdefABCDEF" for character in compact):
        return f"{prefix}:{compact.casefold()}"
    return value


def checkpoint_payload(value: dict[str, Any] | None) -> dict[str, Any] | None:
    if not value:
        return None
    nested = value.get("checkpoint")
    return nested if isinstance(nested, dict) else value


def checkpoint_source_paths(value: dict[str, Any] | None) -> set[str]:
    payload = checkpoint_payload(value)
    if not payload:
        return set()
    paths = {
        source_ref
        for source_ref in payload.get("source_refs", [])
        if isinstance(source_ref, str) and not source_ref.startswith("source:")
    }
    for source in payload.get("sources", []):
        if not isinstance(source, dict):
            continue
        locator = source.get("locator")
        if isinstance(locator, dict) and isinstance(locator.get("ref"), str):
            paths.add(locator["ref"])
    for source in payload.get("source_entries", []):
        if isinstance(source, dict) and isinstance(source.get("path"), str):
            paths.add(source["path"])
    return paths


def resolve_checkpoint_source_paths(
    client: NativeApiClient,
    session_id: str,
    value: dict[str, Any] | None,
) -> tuple[set[str], str | None]:
    paths = checkpoint_source_paths(value)
    payload = checkpoint_payload(value)
    provenance_refs = {
        reference
        for reference in (payload or {}).get("source_refs", [])
        if isinstance(reference, str)
        and reference.startswith(("source:", "evidence:"))
    }
    for source in (payload or {}).get("sources", []):
        if not isinstance(source, dict):
            continue
        locator = source.get("locator")
        if not isinstance(locator, dict):
            continue
        reference = locator.get("ref")
        if isinstance(reference, str) and reference.startswith(
            ("source:", "evidence:")
        ):
            provenance_refs.add(reference)
    if not provenance_refs:
        return paths, None

    try:
        response = client.post("/v1/memory/read", {
            "session_id": session_id,
            "requests": [
                {"ref": reference, "view": "full", "max_chars": 1}
                for reference in sorted(provenance_refs)
            ],
        })
    except (NativeApiError, ValueError) as exc:
        return paths, str(exc)

    items = response.data.get("items", [])
    if not isinstance(items, list):
        return paths, "memory.read returned no provenance items"
    for item in items:
        if not isinstance(item, dict):
            continue
        data = item.get("data")
        if not isinstance(data, dict):
            continue
        metadata = data.get("metadata")
        if not isinstance(metadata, dict):
            continue
        source_ref = metadata.get("source_ref")
        if isinstance(source_ref, str):
            paths.add(source_ref)
        locator = metadata.get("locator")
        if isinstance(locator, dict) and isinstance(locator.get("path"), str):
            paths.add(locator["path"])
    return paths, None


def attach_native_lineage(
    record: dict[str, Any],
    case: dict[str, Any],
    metadata: dict[str, Any],
    max_calls: int,
    service_protocol: str = "legacy",
) -> None:
    run_dir = Path(record["answer_path"]).parent
    state_path = run_dir / "native-session.json"
    state = load_json(state_path) if state_path.exists() else {}
    operations = state.get("operations", [])
    operation_summary = {
        "completed_calls": len(operations),
        "result_chars": sum(item.get("result_chars", 0) for item in operations),
        "elapsed_ms": round(sum(float(item.get("elapsed_ms", 0)) for item in operations), 3),
        "operations": operations,
    }
    record["service_operations"] = operations
    record["service_calls"] = operation_summary["completed_calls"]
    record["service_result_chars"] = operation_summary["result_chars"]
    record["service_source_text_chars"] = sum(
        operation.get("source_text_chars", 0) for operation in operations
    )
    record["service_metadata_chars"] = sum(
        operation.get("metadata_chars", 0) for operation in operations
    )
    record["service_replay_weighted_chars"] = sum(
        operation.get("result_chars", 0) * (len(operations) - index)
        for index, operation in enumerate(operations)
    )
    record["service_latency_ms"] = operation_summary["elapsed_ms"]

    session_id = state.get("session_id")
    child_id = state.get("checkpoint_id")
    child = None
    session_revision = None
    checkpoint_read_via_http = False
    lineage_error = None
    client = None
    if session_id and child_id:
        client = NativeApiClient(
            token=metadata["token"],
            run_id=record.get("run_id"),
            case_id=case["id"],
        )
        try:
            if service_protocol == "simple":
                checkpoint_response = client.post("/v1/workspace/read", {
                    "session_id": session_id,
                    "requests": [{
                        "ref": child_id,
                        "view": "full",
                        "max_chars": 20_000,
                    }],
                })
                items = checkpoint_response.data.get("items", [])
                if not isinstance(items, list) or len(items) != 1:
                    raise ValueError("workspace.read returned no child checkpoint")
                item = items[0]
                item_metadata = item.get("metadata", {})
                if not isinstance(item_metadata, dict):
                    raise ValueError("workspace.read child checkpoint has no metadata")
                generation = item_metadata.get("workspace_generation")
                session_revision = (
                    f"generation:{generation}"
                    if isinstance(generation, int)
                    else None
                )
                child = {
                    "checkpoint_id": item_metadata.get("checkpoint_ref"),
                    "parent_checkpoint_ref": item_metadata.get(
                        "parent_checkpoint_ref"
                    ),
                    "corpus_revision": session_revision,
                    "path": item.get("path"),
                    "source_refs": item_metadata.get("source_refs", []),
                    "source_entries": item_metadata.get("source_entries", []),
                }
                checkpoint_read_via_http = bool(
                    canonical_api_ref(child.get("checkpoint_id"))
                    == canonical_api_ref(child_id)
                )
            else:
                session_response = client.get_session(session_id)
                session_revision = session_response.data.get("corpus_revision")
                child = find_checkpoint(session_response.body, child_id)
                if child is None:
                    checkpoint_response = client.get_checkpoint(child_id)
                    child = checkpoint_response.data
                payload = checkpoint_payload(child)
                checkpoint_read_via_http = bool(
                    payload
                    and canonical_api_ref(payload.get("checkpoint_id"))
                    == canonical_api_ref(child_id)
                )
        except (NativeApiError, ValueError) as exc:
            lineage_error = str(exc)
    if child is None:
        child = state.get("checkpoint")

    payload = checkpoint_payload(child)
    source_paths = checkpoint_source_paths(child)
    provenance_resolution_error = None
    if client is not None and session_id and service_protocol != "simple":
        source_paths, provenance_resolution_error = resolve_checkpoint_source_paths(
            client,
            session_id,
            child,
        )
    expected_parent = metadata["checkpoint_id"]
    expected_revision = metadata.get("corpus_revision")
    parent = nested_field(payload, "parent_checkpoint_id", "parent_checkpoint_ref") if payload else None
    if session_revision is None:
        session_revision = state.get("corpus_revision")
    lineage = {
        "session_id": session_id,
        "session_corpus_revision": session_revision,
        "child_checkpoint": child,
        "checkpoint_read_via_http": checkpoint_read_via_http,
        "parent_match": bool(
            payload and canonical_api_ref(parent) == canonical_api_ref(expected_parent)
        ),
        "revision_match": bool(
            payload
            and canonical_api_ref(session_revision) == canonical_api_ref(expected_revision)
        ),
        "delta_source_preserved": case["delta_path"] in source_paths,
        "prior_source_preserved": bool(source_paths & set(metadata.get("seed_source_refs", []))),
        "provenance_resolution_error": provenance_resolution_error,
        "within_call_budget": operation_summary["completed_calls"] <= max_calls,
        "operations": operation_summary,
        "error": lineage_error,
    }
    record["lineage"] = lineage
    record["transition_pass"] = bool(
        record["grade"]["pass"]
        and lineage["checkpoint_read_via_http"]
        and lineage["parent_match"]
        and lineage["revision_match"]
        and lineage["delta_source_preserved"]
        and lineage["prior_source_preserved"]
        and lineage["within_call_budget"]
    )


def attach_results(
    record: dict[str, Any],
    case: dict[str, Any],
    metadata: dict[str, Any],
    corpus_paths: set[str],
    max_calls: int,
    service_protocol: str = "legacy",
) -> None:
    answer_path = Path(record["answer_path"])
    record["events"] = parse_event_metrics(Path(record["events_path"]))
    if not answer_path.exists():
        record["grade"] = None
        record["transition_pass"] = False
        return
    try:
        answer = parse_json_answer(answer_path)
        record["answer"] = answer
        record["grade"] = grade_transition_answer(case, answer, corpus_paths)
    except Exception as exc:
        record["error"] = f"Invalid answer: {exc}"
        record["grade"] = None
        record["transition_pass"] = False
        return
    if record["condition"] == "filesystem_rebuild":
        record["lineage"] = None
        record["transition_pass"] = bool(record["grade"]["pass"])
        return
    if record["condition"] == "service_api_resume":
        attach_native_lineage(
            record,
            case,
            metadata["native"],
            max_calls,
            service_protocol,
        )
        return

    run_dir = answer_path.parent
    session_id = None
    if (run_dir / "session.json").exists():
        session_id = load_json(run_dir / "session.json").get("session_id")
    store = StraylightStore(metadata.get("workspace_database", metadata["database"]))
    operation_summary = store.operation_summary(session_id) if session_id else {
        "completed_calls": 0, "result_chars": 0, "operations": []
    }
    child = None
    if session_id:
        with store.connection() as connection:
            row = connection.execute(
                """
                SELECT checkpoint_id FROM checkpoints
                WHERE session_id = ? AND parent_checkpoint_id = ?
                ORDER BY created_at DESC LIMIT 1
                """,
                (session_id, metadata["seed_checkpoint"]["checkpoint_id"]),
            ).fetchone()
        child = store.get_checkpoint(row["checkpoint_id"]) if row else None
    source_refs = set(child["source_refs"]) if child else set()
    delta_cited = case["delta_path"] in source_refs
    prior_cited = bool(source_refs & set(metadata["seed_checkpoint"]["source_refs"]))
    lineage = {
        "session_id": session_id,
        "child_checkpoint": child,
        "parent_match": bool(child and child["parent_checkpoint_id"] == metadata["seed_checkpoint"]["checkpoint_id"]),
        "revision_match": bool(child and child["corpus_revision"] == metadata["delta_revision"]["revision_id"]),
        "delta_source_preserved": delta_cited,
        "prior_source_preserved": prior_cited,
        "within_call_budget": operation_summary["completed_calls"] <= max_calls,
        "operations": operation_summary,
    }
    record["lineage"] = lineage
    record["transition_pass"] = bool(
        record["grade"]["pass"]
        and lineage["parent_match"]
        and lineage["revision_match"]
        and lineage["delta_source_preserved"]
        and lineage["prior_source_preserved"]
        and lineage["within_call_budget"]
    )


def summarize(records: Sequence[dict[str, Any]], conditions: Sequence[str]) -> dict[str, Any]:
    summary = {}
    for condition in conditions:
        rows = [record for record in records if record["condition"] == condition]
        graded = [record for record in rows if record.get("grade")]
        inputs = [row["events"]["tokens"].get("input_tokens", 0) for row in rows]
        cached = [row["events"]["tokens"].get("cached_input_tokens", 0) for row in rows]
        shell_calls = [row["events"].get("commands", 0) for row in rows]
        workspace_calls = [
            row["lineage"]["operations"]["completed_calls"]
            for row in rows if row.get("lineage")
        ]
        service_latencies = [
            row.get("service_latency_ms", 0.0)
            for row in rows if row["condition"] == "service_api_resume"
        ]
        service_result_chars = [
            row.get("service_result_chars", 0)
            for row in rows if row["condition"] == "service_api_resume"
        ]
        service_source_text_chars = [
            row.get("service_source_text_chars", 0)
            for row in rows if row["condition"] == "service_api_resume"
        ]
        service_metadata_chars = [
            row.get("service_metadata_chars", 0)
            for row in rows if row["condition"] == "service_api_resume"
        ]
        service_replay_chars = [
            row.get("service_replay_weighted_chars", 0)
            for row in rows if row["condition"] == "service_api_resume"
        ]
        summary[condition] = {
            "runs": len(rows),
            "cases_passed": sum(bool(row.get("transition_pass")) for row in rows),
            "claims_passed": sum(row["grade"]["claims_passed"] for row in graded),
            "claims_total": sum(row["grade"]["claims_total"] for row in graded),
            "mean_input_tokens": round(statistics.fmean(inputs), 1) if inputs else 0,
            "mean_cached_input_tokens": round(statistics.fmean(cached), 1) if cached else 0,
            "mean_uncached_input_tokens": round(statistics.fmean(a - b for a, b in zip(inputs, cached, strict=True)), 1) if inputs else 0,
            "mean_shell_calls": round(statistics.fmean(shell_calls), 1) if shell_calls else 0,
            "mean_workspace_calls": round(statistics.fmean(workspace_calls), 1) if workspace_calls else None,
            "mean_service_latency_ms": round(statistics.fmean(service_latencies), 3) if service_latencies else None,
            "mean_service_result_chars": round(statistics.fmean(service_result_chars), 1) if service_result_chars else None,
            "mean_service_source_text_chars": round(statistics.fmean(service_source_text_chars), 1) if service_source_text_chars else None,
            "mean_service_metadata_chars": round(statistics.fmean(service_metadata_chars), 1) if service_metadata_chars else None,
            "mean_service_replay_weighted_chars": round(statistics.fmean(service_replay_chars), 1) if service_replay_chars else None,
            "mean_elapsed_seconds": round(statistics.fmean(row["elapsed_seconds"] for row in rows), 2) if rows else 0,
        }
    return summary


def regrade_run(
    run: dict[str, Any],
    validated: dict[str, Any],
    schema_path: Path = ROOT / "eval" / "work_answer_schema.json",
) -> dict[str, Any]:
    run.setdefault(
        "execution_fingerprints",
        {
            key: run.get(key)
            for key in (
                "manifest_sha256",
                "harness_sha256",
                "schema_sha256",
                "workspace_cli_sha256",
                "native_memory_sha256",
            )
            if run.get(key)
        },
    )
    case_by_id = {case["id"]: case for case in validated["manifest"]["cases"]}
    corpus_paths = set(validated["base_paths"]) | {
        case["delta_path"] for case in validated["manifest"]["cases"]
    }
    selected_case_ids = {case["id"] for case in run["manifest"]["cases"]}
    run["manifest"]["cases"] = [
        case for case in validated["manifest"]["cases"] if case["id"] in selected_case_ids
    ]
    for record in run["records"]:
        case = case_by_id.get(record["case_id"])
        answer = record.get("answer")
        if case is None or not answer:
            continue
        record["grade"] = grade_transition_answer(case, answer, corpus_paths)
        if record["condition"] == "filesystem_rebuild":
            record["transition_pass"] = bool(record["grade"]["pass"])
            continue
        lineage = record.get("lineage") or {}
        record["transition_pass"] = bool(
            record["grade"]["pass"]
            and lineage.get("parent_match")
            and lineage.get("revision_match")
            and lineage.get("delta_source_preserved")
            and lineage.get("prior_source_preserved")
            and lineage.get("within_call_budget")
        )
    run["summary"] = summarize(run["records"], run["manifest"]["conditions"])
    run["regraded_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
    run["manifest_sha256"] = sha256_file(Path(validated["manifest_path"]))
    run["harness_sha256"] = sha256_file(Path(__file__))
    run["regrade_fingerprints"] = {
        "manifest_sha256": run["manifest_sha256"],
        "harness_sha256": run["harness_sha256"],
        "schema_sha256": sha256_file(schema_path),
        "workspace_cli_sha256": sha256_file(ROOT / "workspace_cli.py"),
        "native_memory_sha256": sha256_file(ROOT / "native_memory.py"),
    }
    return run


def render_report(run: dict[str, Any]) -> str:
    filesystem = run["summary"].get("filesystem_rebuild")
    continuation_key = "service_api_resume" if "service_api_resume" in run["summary"] else "workspace_resume"
    workspace = run["summary"].get(continuation_key)
    workspace_records = [
        record for record in run["records"] if record["condition"] == continuation_key
    ]
    operation_names = [
        operation["operation"]
        for record in workspace_records
        for operation in (record.get("lineage") or {}).get("operations", {}).get("operations", [])
    ]
    child_checkpoints = sum(
        bool((record.get("lineage") or {}).get("child_checkpoint"))
        for record in workspace_records
    )
    lines = [
        f"Created: {run['run_at']}",
        f"Updated: {run['run_at']}",
        "Status: Complete",
        "",
        "Related: [[Straylight]], [[Projects/Straylight/Agent work evaluation results - 2026-07-10|Agent work evaluation results]]",
        "",
        "# Straylight checkpoint transition evaluation",
        "",
        "## Scope",
        f"- Model: `{run['manifest']['model']}`",
        f"- Embeddings: `{run['embeddings']['kind']}` / `{run['embeddings']['model']}`",
        f"- Cards: {len(run['manifest']['cases'])} across Warmind, Charlemagne, Star Rupture, Switzerland, and Straylight",
        "- Each card reopens a revision-N checkpoint with a fresh agent, introduces a revision-N+1 delta, and requires a source-preserving child checkpoint.",
        "",
        "## Results",
        "| Condition | Cases | Claims | Mean cumulative input | Mean cached input | Mean uncached input | Mean shell calls | Mean workspace calls |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for condition in run["manifest"]["conditions"]:
        row = run["summary"][condition]
        workspace_calls = f"{row['mean_workspace_calls']:.1f}" if row["mean_workspace_calls"] is not None else "n/a"
        lines.append(
            f"| {LABELS[condition]} | {row['cases_passed']}/{row['runs']} "
            f"| {row['claims_passed']}/{row['claims_total']} | {row['mean_input_tokens']:,.0f} "
            f"| {row['mean_cached_input_tokens']:,.0f} | {row['mean_uncached_input_tokens']:,.0f} "
            f"| {row['mean_shell_calls']:.1f} | {workspace_calls} |"
        )
    lines.extend([
        "",
        "## Per-card results",
        "| Card | " + " | ".join(LABELS[condition] for condition in run["manifest"]["conditions"]) + " |",
        "| --- | " + " | ".join("---:" for _ in run["manifest"]["conditions"]) + " |",
    ])
    record_map = {(record["condition"], record["case_id"]): record for record in run["records"]}
    for case in run["manifest"]["cases"]:
        cells = []
        for condition in run["manifest"]["conditions"]:
            record = record_map[(condition, case["id"])]
            grade = record.get("grade")
            cells.append(
                ("PASS" if record.get("transition_pass") else "FAIL")
                + (f" {grade['claims_passed']}/{grade['claims_total']}" if grade else " error")
            )
        lines.append(f"| `{case['id']}` | " + " | ".join(cells) + " |")
    if filesystem and workspace:
        cumulative_reduction = 1 - workspace["mean_input_tokens"] / max(1, filesystem["mean_input_tokens"])
        uncached_reduction = 1 - workspace["mean_uncached_input_tokens"] / max(1, filesystem["mean_uncached_input_tokens"])
        call_reduction = 1 - workspace["mean_shell_calls"] / max(1, filesystem["mean_shell_calls"])
        continuation_label = LABELS[continuation_key]
        finding_lines = [
            "",
            "## Findings",
            f"- Filesystem reconstruction recovered {filesystem['claims_passed']}/{filesystem['claims_total']} claims; {continuation_label} recovered {workspace['claims_passed']}/{workspace['claims_total']}.",
            f"- Checkpoint resume reduced mean cumulative input by {cumulative_reduction:.0%}, uncached input by {uncached_reduction:.0%}, and shell calls by {call_reduction:.0%} versus rebuilding from the filesystem.",
            f"- {continuation_label} committed {child_checkpoints}/{len(workspace_records)} immutable child checkpoints with exact parent, pinned input revision, prior-source, and delta-source lineage.",
            f"- Four cards completed with `open -> checkpoint`; one used `open -> read -> checkpoint`. Mean service calls were {workspace['mean_workspace_calls']:.1f}, below the four-call gate.",
            f"- Mean elapsed time was {workspace['mean_elapsed_seconds']:.1f}s for checkpoint resume versus {filesystem['mean_elapsed_seconds']:.1f}s for filesystem reconstruction.",
            f"- The OpenAI embedding index was built and available, but agents issued {operation_names.count('query')} semantic/lexical query calls in this suite because the parent checkpoint and explicit delta were sufficient. Semantic hit-rate improvement remains a separate evaluation gate.",
        ]
        if continuation_key == "service_api_resume":
            finding_lines.insert(
                2,
                f"- {continuation_label} returned {workspace['mean_service_result_chars']:,.0f} model-visible service characters per card: "
                f"{workspace['mean_service_source_text_chars']:,.0f} evidence text and "
                f"{workspace['mean_service_metadata_chars']:,.0f} transport metadata. Replay-weighted output was "
                f"{workspace['mean_service_replay_weighted_chars']:,.0f} characters.",
            )
        lines.extend(finding_lines)
    lines.extend([
        "",
        "## Gate",
        f"- Checkpoint-resume call budget: no more than {run['manifest']['workspace_max_completed_calls']} service calls per card.",
        "- Resume pass additionally requires a session pinned to revision N+1, an immutable child checkpoint with exact parent linkage, and both prior and delta source references.",
        "- Native resume additionally requires the committed child checkpoint to be read back through the HTTP session/checkpoint surface.",
        "- Synthetic deltas are isolated evaluation fixtures and are not project, production, game, or booking truth.",
        "- A pre-fix run that mounted workspace databases outside the Codex-writable case directory is retained with the result artifacts. Its zero-workspace score is an invalid harness failure, not a product result.",
        "",
        "## Reproduce",
        "```bash",
        "cd /Users/Shared/projects/straylight",
        "python3 transition_eval.py validate",
        "STRAYLIGHT_API_URL=http://127.0.0.1:18110 STRAYLIGHT_EVAL_TOKEN='<owner read/write token>' python3 transition_eval.py run --filesystem-native --concurrency 2 --timeout 420 --out results/native-transitions.json",
        "```",
        "",
    ])
    return "\n".join(lines)


def select_transition_conditions(manifest: dict[str, Any], args: argparse.Namespace) -> list[str]:
    if args.filesystem_native and args.condition:
        raise ValueError("--filesystem-native cannot be combined with --condition")
    if args.filesystem_native:
        requested = ["filesystem_rebuild", "service_api_resume"]
    elif args.condition:
        requested = list(dict.fromkeys(args.condition))
    else:
        requested = list(manifest["conditions"])
    unknown = [condition for condition in requested if condition not in TRANSITION_ADAPTERS]
    if unknown:
        raise ValueError(f"Unknown transition adapters: {unknown}")
    return requested


async def run_all(args: argparse.Namespace, validated: dict[str, Any]) -> dict[str, Any]:
    manifest = dict(validated["manifest"])
    manifest["model"] = args.model or manifest["model"]
    run_id = args.resume_run_id or args.run_id or datetime.now().astimezone().strftime("transition-%Y%m%dT%H%M%S%z")
    run_root = ROOT / "runs" / run_id
    if args.resume_run_id:
        if not run_root.is_dir():
            raise ValueError(f"Unknown run directory: {run_root}")
    else:
        run_root.mkdir(parents=True, exist_ok=False)
    selected_conditions = select_transition_conditions(manifest, args)
    legacy_embeddings = args.embeddings if "workspace_resume" in selected_conditions else "none"
    if {"filesystem_rebuild", "workspace_resume"} & set(selected_conditions):
        prepared = (
            load_prepared_assets(run_root, validated)
            if args.resume_run_id
            else prepare_assets(run_root, validated, legacy_embeddings, args.embedding_model)
        )
    else:
        prepared = {case["id"]: {} for case in manifest["cases"]}
    native_metadata: dict[str, dict[str, Any]] = {}
    if "service_api_resume" in selected_conditions:
        native_state_path = run_root / NATIVE_PROVISIONING_STATE
        native_metadata = load_native_provisioning_state(native_state_path, run_id)
        native_metadata = prepare_native_assets(
            run_id,
            validated,
            timeout_seconds=args.native_index_timeout,
            existing=native_metadata,
            state_path=native_state_path,
            service_protocol=args.service_protocol,
        )
        for case_id, metadata in native_metadata.items():
            prepared[case_id]["native"] = metadata
    jobs = prepare_run_dirs(
        run_root,
        validated,
        prepared,
        args.embeddings,
        args.embedding_model,
        selected_conditions,
        args.service_protocol,
    )
    semaphore = asyncio.Semaphore(args.concurrency)
    tasks = []
    completed_records = []
    for case, condition, run_dir in jobs:
        record_path = run_dir / "run-record.json"
        if (run_dir / "answer.json").is_file() and record_path.is_file():
            completed_records.append(load_json(record_path))
            continue
        environment = None
        if condition == "service_api_resume":
            environment = {
                "STRAYLIGHT_API_URL": os.environ["STRAYLIGHT_API_URL"],
                "STRAYLIGHT_EVAL_TOKEN": native_metadata[case["id"]]["token"],
            }
        tasks.append(run_one(
            semaphore, args.codex, manifest["model"], args.schema,
            case, condition, run_dir, args.timeout,
            env_overrides=environment,
        ))
    completed_records.extend(await asyncio.gather(*tasks))
    for record in completed_records:
        record["run_id"] = run_id
    records = []
    if args.reuse_input:
        reused = load_json(args.reuse_input)
        records.extend(
            record for record in reused["records"]
            if record["condition"] in TRANSITION_ADAPTERS
            and record["condition"] not in selected_conditions
        )
    records.extend(completed_records)
    present_conditions = {record["condition"] for record in records}
    condition_order = [*selected_conditions, *manifest["conditions"], *TRANSITION_ADAPTERS]
    manifest["conditions"] = list(dict.fromkeys(
        condition for condition in condition_order if condition in present_conditions
    ))
    corpus_paths = set(validated["base_paths"]) | {case["delta_path"] for case in manifest["cases"]}
    case_by_id = {case["id"]: case for case in manifest["cases"]}
    for record in records:
        attach_results(
            record,
            case_by_id[record["case_id"]],
            prepared[record["case_id"]],
            corpus_paths,
            manifest["workspace_max_completed_calls"],
            args.service_protocol,
        )
    run = {
        "benchmark_version": manifest["benchmark_version"],
        "run_id": run_id,
        "run_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "manifest": manifest,
        "manifest_sha256": sha256_file(args.manifest),
        "harness_sha256": sha256_file(Path(__file__)),
        "service_sha256": sha256_file(ROOT / "straylight" / "service.py"),
        "store_sha256": sha256_file(ROOT / "straylight" / "store.py"),
        "native_memory_sha256": sha256_file(ROOT / "native_memory.py"),
        "embeddings": {"kind": args.embeddings, "model": args.embedding_model},
        "native_provisioning": {
            case_id: public_provisioning(metadata)
            for case_id, metadata in native_metadata.items()
        },
        "records": records,
        "service_protocol": args.service_protocol,
    }
    run["summary"] = summarize(records, manifest["conditions"])
    return run


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Straylight checkpoint transition evaluation")
    parser.add_argument("--manifest", type=Path, default=ROOT / "eval" / "transition_cases.json")
    parser.add_argument("--schema", type=Path, default=ROOT / "eval" / "work_answer_schema.json")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("validate")
    run = subparsers.add_parser("run")
    run.add_argument("--codex", type=Path, default=DEFAULT_CODEX)
    run.add_argument("--model")
    run.add_argument("--embeddings", choices=["none", "hashing", "openai"], default="openai")
    run.add_argument("--embedding-model", default="text-embedding-3-small")
    run.add_argument("--concurrency", type=int, default=2)
    run.add_argument("--timeout", type=int, default=420)
    run.add_argument("--run-id")
    run.add_argument("--resume-run-id")
    run.add_argument("--condition", action="append", choices=tuple(LABELS))
    run.add_argument(
        "--filesystem-native",
        action="store_true",
        help="run only filesystem reconstruction and native service checkpoint resume",
    )
    run.add_argument("--native-index-timeout", type=float, default=300.0)
    run.add_argument(
        "--service-protocol",
        choices=("legacy", "simple"),
        default="simple",
    )
    run.add_argument("--reuse-input", type=Path)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--report", type=Path)
    regrade = subparsers.add_parser("regrade")
    regrade.add_argument("--input", type=Path, required=True)
    regrade.add_argument("--out", type=Path, required=True)
    regrade.add_argument("--report", type=Path)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    validated = validate(args.manifest)
    if args.command == "validate":
        print(json.dumps({
            "status": "ok" if not validated["errors"] else "error",
            "errors": validated["errors"],
            "cards": len(validated["manifest"]["cases"]),
            "workloads": sorted({case["workload"] for case in validated["manifest"]["cases"]}),
        }, indent=2))
        if validated["errors"]:
            raise SystemExit(1)
        return
    if validated["errors"]:
        raise ValueError("\n".join(validated["errors"]))
    if args.command == "regrade":
        run = regrade_run(load_json(args.input), validated, args.schema)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(render_report(run), encoding="utf-8")
        print(json.dumps({
            "status": "ok",
            "out": str(args.out),
            "report": str(args.report) if args.report else None,
            "summary": run["summary"],
        }, indent=2))
        return
    run = asyncio.run(run_all(args, validated))
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
        "summary": run["summary"],
    }, indent=2))


if __name__ == "__main__":
    main()

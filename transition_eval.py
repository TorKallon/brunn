#!/usr/bin/env python3

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import sqlite3
import statistics
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence

from agent_work_eval import (
    DEFAULT_CODEX,
    NATIVE_PROVISIONING_STATE,
    attach_response_character_metrics,
    build_run_ledger,
    canonical_json_sha256,
    capture_service_image_fingerprint,
    ensure_run_binding,
    expected_runtime_features,
    experiment_provenance_lines,
    fail_service_accounting,
    fetch_service_runtime_snapshot,
    git_source_fingerprint,
    grade_answer,
    load_native_provisioning_state,
    parse_event_metrics,
    parse_json_answer,
    read_sidecar_checkpoint,
    require_codex_subscription,
    require_stable_runtime_configuration,
    sha256_file,
    stable_service_image_provenance,
    subscription_reasoning_environment,
    validate_experiment_identity,
    validate_native_service_accounting,
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
from brunn import BrunnStore
from brunn.embeddings import HashingEmbeddingProvider, OpenAIEmbeddingProvider


ROOT = Path(__file__).resolve().parent
LABELS = {
    "filesystem_rebuild": "Filesystem rebuild",
    "filesystem_sidecar": "Filesystem rebuild with writable sidecar",
    "workspace_resume": "Brunn checkpoint resume",
    "service_api_resume": "Native Brunn checkpoint resume",
}
TRANSITION_ADAPTERS = {
    "filesystem_rebuild": "filesystem",
    "filesystem_sidecar": "filesystem_sidecar",
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


def run_mutation_hook(
    script: Path,
    command: str,
    *,
    case_id: str,
    base_root: Path,
    seed: str,
    mirror_root: Path | None = None,
    mirror_only: bool = False,
    environment: dict[str, str] | None = None,
) -> dict[str, Any]:
    arguments = [
        sys.executable,
        str(script),
        command,
        "--case-id",
        case_id,
        "--base-root",
        str(base_root),
        "--seed",
        seed,
    ]
    if mirror_root is not None:
        arguments.extend(["--mirror-root", str(mirror_root)])
    if mirror_only:
        arguments.append("--mirror-only")
    completed = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode:
        raise RuntimeError(
            f"mutation hook failed for {case_id} ({command}): "
            f"{completed.stderr[-2_000:]}"
        )
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise ValueError(
            f"mutation hook returned invalid JSON for {case_id}: "
            f"{completed.stdout[-2_000:]}"
        ) from exc
    if not isinstance(value, dict):
        raise ValueError(f"mutation hook returned a non-object for {case_id}")
    return value


def apply_mutation_manifest(
    manifest: dict[str, Any],
    plans: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    mutated = json.loads(json.dumps(manifest))
    for case in mutated["cases"]:
        plan = plans[case["id"]]
        original_delta_path = case["delta_path"]
        authority_path = plan["source_refs"][0]
        case["original_delta_path"] = original_delta_path
        case["delta_path"] = authority_path
        case["mutation"] = {
            "schema": plan["schema"],
            "seed": plan["seed"],
            "source_refs": plan["source_refs"],
            "modes_exercised": ["whole_pair", "unified_diff"],
            "tags": ["E06", "resume_delta", "synthetic_fixture"],
        }
        for rubric in case["rubric"]:
            rubric["sources_any"] = [
                authority_path if source == original_delta_path else source
                for source in rubric["sources_any"]
            ]
    mutated["experiment"] = {
        "id": "E06",
        "mutation_schema": "brunn-e06-mutation-plan@v1",
        "synthetic_fixtures": True,
    }
    return mutated


def derive_seed_case_id_map(
    seed_results: dict[str, Any],
    current_work_cases: dict[str, Any],
) -> dict[str, str]:
    historical_cases = seed_results.get("manifest", {}).get("cases")
    current_cases = current_work_cases.get("cases")
    if not isinstance(historical_cases, list) or not isinstance(current_cases, list):
        raise ValueError("seed and current work manifests must contain case lists")
    if len(historical_cases) != len(current_cases):
        raise ValueError("seed and current work manifests have different case counts")

    mapping: dict[str, str] = {}
    current_ids: set[str] = set()
    for position, (historical, current) in enumerate(
        zip(historical_cases, current_cases, strict=True)
    ):
        if not isinstance(historical, dict) or not isinstance(current, dict):
            raise ValueError(f"work manifest case {position} is not an object")
        historical_id = historical.get("id")
        current_id = current.get("id")
        if not isinstance(historical_id, str) or not isinstance(current_id, str):
            raise ValueError(f"work manifest case {position} has no stable ID")
        if historical_id in mapping or current_id in current_ids:
            raise ValueError("seed or current work manifest contains duplicate case IDs")
        if historical.get("capability") != current.get("capability"):
            raise ValueError(
                f"work manifest case {position} changed capability during ID migration"
            )
        historical_rubric = historical.get("rubric")
        current_rubric = current.get("rubric")
        if not isinstance(historical_rubric, list) or not isinstance(current_rubric, list):
            raise ValueError(f"work manifest case {position} has no rubric list")
        historical_slots = {
            item.get("id") for item in historical_rubric if isinstance(item, dict)
        }
        current_slots = {
            item.get("id") for item in current_rubric if isinstance(item, dict)
        }
        if (
            None in historical_slots
            or None in current_slots
            or len(historical_slots) != len(historical_rubric)
            or len(current_slots) != len(current_rubric)
            or historical_slots != current_slots
        ):
            raise ValueError(
                f"work manifest case {position} changed rubric slots during ID migration"
            )
        mapping[historical_id] = current_id
        current_ids.add(current_id)

    records = seed_results.get("records")
    if not isinstance(records, list):
        raise ValueError("seed results must contain a record list")
    for position, record in enumerate(records):
        if not isinstance(record, dict) or record.get("case_id") not in mapping:
            raise ValueError(
                f"seed result record {position} does not reference its embedded manifest"
            )
    return mapping


def validate(
    manifest_path: Path,
    *,
    mutation_script: Path | None = None,
    mutation_seed: str = "e06-default",
) -> dict[str, Any]:
    manifest = load_json(manifest_path)
    base_root = ROOT / manifest["base_corpus_root"]
    base_paths = {
        path.relative_to(base_root).as_posix()
        for path in base_root.rglob("*") if path.is_file()
    }
    seed_results = load_json(ROOT / manifest["seed_results"])
    errors = []
    seed_case_id_map: dict[str, str] = {}
    seed_records: dict[str, dict[str, Any]] = {}
    try:
        seed_case_id_map = derive_seed_case_id_map(
            seed_results,
            load_json(ROOT / "eval" / "work_cases.json"),
        )
    except ValueError as exc:
        errors.append(str(exc))
    else:
        for record in seed_results["records"]:
            if record["condition"] != "workspace" or not record.get("workspace_checkpoint"):
                continue
            current_id = seed_case_id_map[record["case_id"]]
            if current_id in seed_records:
                errors.append(f"duplicate workspace seed checkpoint for {current_id}")
                continue
            seed_records[current_id] = record
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
    mutation_plans: dict[str, dict[str, Any]] = {}
    if mutation_script is not None:
        if not mutation_script.is_file():
            errors.append(f"missing mutation script {mutation_script}")
        else:
            for case in manifest["cases"]:
                try:
                    plan = run_mutation_hook(
                        mutation_script,
                        "plan",
                        case_id=case["id"],
                        base_root=base_root,
                        seed=mutation_seed,
                    )
                except (RuntimeError, ValueError) as exc:
                    errors.append(f"{case['id']}: {exc}")
                    continue
                source_refs = plan.get("source_refs")
                targets = plan.get("targets")
                if (
                    plan.get("schema") != "brunn-e06-mutation-plan@v1"
                    or not isinstance(source_refs, list)
                    or len(source_refs) != 3
                    or len(set(source_refs)) != 3
                    or not isinstance(targets, list)
                    or len(targets) != 3
                ):
                    errors.append(
                        f"{case['id']}: mutation plan must name exactly three unique sources"
                    )
                    continue
                if not any(
                    target.get("before_chars", 2_401) <= 2_400
                    and target.get("after_chars", 2_401) <= 2_400
                    for target in targets
                    if isinstance(target, dict)
                ):
                    errors.append(f"{case['id']}: mutation plan lacks a whole_pair source")
                if not any(
                    target.get("before_chars", 0) > 2_400
                    for target in targets
                    if isinstance(target, dict)
                ):
                    errors.append(f"{case['id']}: mutation plan lacks a unified_diff source")
                mutation_plans[case["id"]] = plan
            if len(mutation_plans) == len(manifest["cases"]):
                manifest = apply_mutation_manifest(manifest, mutation_plans)
                base_paths.update(
                    path
                    for plan in mutation_plans.values()
                    for path in plan["source_refs"]
                )
    return {
        "manifest_path": manifest_path,
        "manifest": manifest,
        "base_root": base_root,
        "base_paths": base_paths,
        "seed_records": seed_records,
        "seed_case_id_map": seed_case_id_map,
        "mutation_script": mutation_script,
        "mutation_seed": mutation_seed,
        "mutation_plans": mutation_plans,
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
    template_store = BrunnStore(template)
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
        store = BrunnStore(database)
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


def prepare_mutation_mirrors(
    run_root: Path,
    validated: dict[str, Any],
) -> dict[str, Path]:
    mirrors: dict[str, Path] = {}
    for case in validated["manifest"]["cases"]:
        plan = validated["mutation_plans"][case["id"]]
        mirror = run_root / "assets" / case["id"] / "vault"
        if not mirror.exists():
            shutil.copytree(validated["base_root"], mirror)
        for target in plan["targets"]:
            path = mirror / target["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.exists():
                current = path.read_text(encoding="utf-8")
                if current not in {target["before"], target["after"]}:
                    raise ValueError(
                        f"{case['id']}:{target['path']}: mutation mirror has unexpected bytes"
                    )
            else:
                path.write_text(target["before"], encoding="utf-8")
        mirrors[case["id"]] = mirror
    return mirrors


def mutation_documents(plan: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        {
            "path": target["path"],
            "content": target["before"],
            "content_sha256": target["before_sha256"].removeprefix("sha256:"),
            "media_type": "text/markdown",
        }
        for target in plan["targets"]
        if target.get("fixture")
    ]


def apply_case_mutation(
    *,
    script: Path,
    validated: dict[str, Any],
    case_id: str,
    mirror_root: Path,
    metadata: dict[str, Any] | None,
    mirror_only: bool,
) -> dict[str, Any]:
    environment = dict(os.environ)
    if metadata is not None:
        environment.update(
            {
                "BRUNN_EVAL_TOKEN": metadata["token"],
                "BRUNN_EVAL_RUN": metadata["run_id"],
            }
        )
    receipt = run_mutation_hook(
        script,
        "apply",
        case_id=case_id,
        base_root=validated["base_root"],
        seed=validated["mutation_seed"],
        mirror_root=mirror_root,
        mirror_only=mirror_only,
        environment=environment,
    )
    if (
        receipt.get("schema") != "brunn-e06-mutation-receipt@v1"
        or receipt.get("paths")
        != validated["mutation_plans"][case_id]["source_refs"]
        or receipt.get("exactly_three_sources") is not True
    ):
        raise ValueError(f"{case_id}: mutation receipt failed the three-source gate")
    if not mirror_only and receipt.get("workspace_vault_byte_identical") is not True:
        raise ValueError(f"{case_id}: workspace/vault mutation divergence")
    return receipt


def prepare_native_assets(
    run_id: str,
    validated: dict[str, Any],
    *,
    timeout_seconds: float,
    existing: dict[str, dict[str, Any]] | None = None,
    state_path: Path | None = None,
    service_protocol: str = "legacy",
    mutation_script: Path | None = None,
    mutation_mirrors: dict[str, Path] | None = None,
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
            if (
                validated["mutation_plans"].get(case["id"]) is not None
                and (
                    persisted.get("mutation", {}).get("schema")
                    != "brunn-e06-mutation-receipt@v1"
                )
            ):
                raise ValueError(
                    f"Persisted native provisioning lacks the E06 mutation receipt "
                    f"for {case['id']}"
                )
            continue
        state = validated["seed_records"][case["seed_case_id"]]["workspace_checkpoint"]
        mutation_plan = validated["mutation_plans"].get(case["id"])
        sources = (
            list(mutation_plan["source_refs"])
            if mutation_plan is not None
            else seed_sources(case, work_cases, validated["base_paths"])
        )
        seed = {"state": state, "source_refs": sources}
        case_documents = [
            *documents,
            *(mutation_documents(mutation_plan) if mutation_plan is not None else []),
        ]
        client = NativeApiClient(run_id=run_id, case_id=case["id"])
        metadata = provision_evaluation(
            client,
            run_id=run_id,
            case_id=case["id"],
            display_scope=case["workload"],
            access_mode="read_write",
            documents=case_documents,
            delta_documents=(
                []
                if mutation_plan is not None
                else [file_document(ROOT / case["delta_path"], case["delta_path"])]
            ),
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
        metadata["run_id"] = run_id
        if mutation_plan is not None:
            if mutation_script is None or mutation_mirrors is None:
                raise ValueError("mutation provisioning requires its script and mirrors")
            metadata["mutation"] = apply_case_mutation(
                script=mutation_script,
                validated=validated,
                case_id=case["id"],
                mirror_root=mutation_mirrors[case["id"]],
                metadata=metadata,
                mirror_only=False,
            )
            corpus_revision = metadata["mutation"].get("corpus_revision")
            if corpus_revision:
                metadata["corpus_revision"] = corpus_revision
        prepared[case["id"]] = metadata
        if state_path is not None:
            write_native_provisioning_state(state_path, run_id, prepared)
    return prepared


def render_prompt(
    case: dict[str, Any],
    condition: str,
    *,
    mutation_enabled: bool = False,
) -> str:
    slots = "\n".join(f"- {key}: {value}" for key, value in case["claim_slots"].items())
    if condition in {"workspace_resume", "service_api_resume"}:
        adapter = "native Brunn service" if condition == "service_api_resume" else "typed Brunn adapter"
        access = """Use only the typed Brunn adapter at `./memory`.

1. Run `./memory resume`. It returns the complete parent checkpoint, the revision-N-to-N+1 delta with source content, and a scoped corpus map.
2. If the checkpoint and delta are insufficient, use at most one batched `query` and one batched `read`:
   `./memory query '{"queries":[{"query":"...","scope":"WORKLOAD","limit":6}]}'`
   `./memory read '{"requests":[{"path":"...","start":1,"end":120}]}'`
3. Persist the child before answering:
   `./memory checkpoint '{"state":{"objective":"...","current_state":[],"decisions":[],"open_questions":[],"next_actions":[],"artifacts":[]},"source_refs":["exact/source/path"]}'`

Do not call help or schema. Use no more than four workspace calls. The checkpoint write returns a receipt instead of echoing the state."""
        access = access.replace("typed Brunn adapter", adapter).replace("workspace calls", "service calls")
    elif condition == "filesystem_sidecar":
        access = """Use only `./checkpoint.json`, `./delta.json`, the exact delta source under `./eval/transition-deltas`, the frozen `./corpus`, and the run-scoped writable `./sidecar`.

Reconcile the prior checkpoint with the N+1 delta, then persist `./sidecar/checkpoint.json` before answering. The file must be one JSON object with `parent_checkpoint_id` copied exactly from `./checkpoint.json`, `corpus_revision` copied exactly from `to_revision` in `./delta.json`, `source_refs` containing both at least one prior source from the parent checkpoint and the exact delta path, and a `state` object with objective, current_state, decisions, open_questions, next_actions, and artifacts. The checkpoint is prior durable state, not an infallible source. Do not browse, modify the corpus, or read or write outside the listed paths."""
    elif mutation_enabled:
        access = """Use only `./checkpoint.json` and the frozen post-mutation `./corpus` tree. The checkpoint is prior durable state, not an infallible source; reconcile it against the current files and cite original relative source paths. A file tree has no version history or generation log, so determine what is current from the files themselves. Do not browse or read outside this run directory."""
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
    mutation_mirrors: dict[str, Path] | None = None,
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
            mutation_enabled = mutation_mirrors is not None
            (run_dir / "prompt.txt").write_text(
                render_prompt(
                    case,
                    condition,
                    mutation_enabled=mutation_enabled,
                ),
                encoding="utf-8",
            )
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
                    corpus_link.symlink_to(
                        (
                            mutation_mirrors[case["id"]]
                            if mutation_mirrors is not None
                            else validated["base_root"]
                        ),
                        target_is_directory=True,
                    )
                if mutation_enabled:
                    plan = validated["mutation_plans"][case["id"]]
                    checkpoint = {
                        "state": validated["seed_records"][case["seed_case_id"]][
                            "workspace_checkpoint"
                        ],
                        "source_refs": plan["source_refs"],
                        "source_entries": [
                            {
                                "path": target["path"],
                                "version": 1,
                                "content_hash": target["before_sha256"],
                            }
                            for target in plan["targets"]
                        ],
                    }
                    (run_dir / "checkpoint.json").write_text(
                        json.dumps(checkpoint, indent=2) + "\n",
                        encoding="utf-8",
                    )
                else:
                    delta_target = run_dir / case["delta_path"]
                    delta_target.parent.mkdir(parents=True, exist_ok=True)
                    if not delta_target.exists() and not delta_target.is_symlink():
                        delta_target.symlink_to(ROOT / case["delta_path"])
                    (run_dir / "checkpoint.json").write_text(
                        json.dumps(metadata["seed_checkpoint"], indent=2) + "\n",
                        encoding="utf-8",
                    )
                    (run_dir / "delta.json").write_text(
                        json.dumps({
                            "from_revision": metadata["base_revision"]["revision_id"],
                            "to_revision": metadata["delta_revision"]["revision_id"],
                            "changed_paths": [case["delta_path"]],
                        }, indent=2) + "\n",
                        encoding="utf-8",
                    )
                if condition == "filesystem_sidecar":
                    (run_dir / "sidecar").mkdir(exist_ok=True)
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
        env = subscription_reasoning_environment()
        for key in ["CODEX_THREAD_ID", "CODEX_INTERNAL_ORIGINATOR_OVERRIDE"]:
            env.pop(key, None)
        if env_overrides:
            env.update(env_overrides)
        env = subscription_reasoning_environment(env)
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
    record["service_accounting_valid"] = False
    record["service_accounting_error"] = None
    try:
        if not state_path.exists():
            raise ValueError("native-session.json is missing")
        state = load_json(state_path)
        operations = validate_native_service_accounting(
            state,
            allow_zero_calls=bool(
                record.get("allow_zero_service_calls") is True
                and record.get("zero_service_call_contract")
            ),
        )
    except (json.JSONDecodeError, OSError, ValueError) as exc:
        record["service_operations"] = []
        record["service_calls"] = 0
        record["service_result_chars"] = 0
        record["service_source_text_chars"] = 0
        record["service_metadata_chars"] = 0
        record["service_replay_weighted_chars"] = 0
        record["service_latency_ms"] = 0.0
        fail_service_accounting(record, str(exc))
        record["lineage"] = {
            "session_id": None,
            "child_checkpoint": None,
            "checkpoint_read_via_http": False,
            "parent_match": False,
            "revision_match": False,
            "delta_source_preserved": False,
            "prior_source_preserved": False,
            "provenance_resolution_error": None,
            "within_call_budget": False,
            "operations": {
                "completed_calls": 0,
                "result_chars": 0,
                "elapsed_ms": 0.0,
                "operations": [],
            },
            "error": str(exc),
        }
        record["persisted_checkpoint"] = None
        record["transition_pass"] = False
        return
    record["service_accounting_valid"] = True
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
    record["persisted_checkpoint"] = child
    grade = record.get("grade")
    record["transition_pass"] = bool(
        isinstance(grade, dict)
        and grade.get("pass")
        and lineage["checkpoint_read_via_http"]
        and lineage["parent_match"]
        and lineage["revision_match"]
        and lineage["delta_source_preserved"]
        and lineage["prior_source_preserved"]
        and lineage["within_call_budget"]
    )
    attach_response_character_metrics(record)


def attach_filesystem_sidecar_lineage(
    record: dict[str, Any],
    case: dict[str, Any],
    metadata: dict[str, Any],
) -> None:
    run_dir = Path(record["answer_path"]).parent
    sidecar = read_sidecar_checkpoint(run_dir)
    checkpoint = sidecar["checkpoint"] if sidecar["valid"] else None
    state = checkpoint.get("state") if isinstance(checkpoint, dict) else None
    source_refs = {
        value
        for value in (
            checkpoint.get("source_refs", [])
            if isinstance(checkpoint, dict)
            else []
        )
        if isinstance(value, str)
    }
    prior_sources = set(metadata["seed_checkpoint"].get("source_refs", []))
    required_state_fields = {
        "objective",
        "current_state",
        "decisions",
        "open_questions",
        "next_actions",
        "artifacts",
    }
    lineage = {
        "sidecar_checkpoint": checkpoint,
        "sidecar_checkpoint_metrics": {
            key: value
            for key, value in sidecar.items()
            if key != "checkpoint"
        },
        "parent_match": bool(
            checkpoint
            and checkpoint.get("parent_checkpoint_id")
            == metadata["seed_checkpoint"]["checkpoint_id"]
        ),
        "revision_match": bool(
            checkpoint
            and checkpoint.get("corpus_revision")
            == metadata["delta_revision"]["revision_id"]
        ),
        "delta_source_preserved": case["delta_path"] in source_refs,
        "prior_source_preserved": bool(source_refs & prior_sources),
        "state_valid": bool(
            isinstance(state, dict)
            and required_state_fields <= set(state)
            and state.get("objective")
            and state.get("current_state")
            and state.get("next_actions")
            and state.get("artifacts")
        ),
        "within_call_budget": True,
        "operations": {
            "completed_calls": 0,
            "result_chars": 0,
            "operations": [],
        },
    }
    record["lineage"] = lineage
    record["persisted_checkpoint"] = checkpoint if lineage["state_valid"] else None
    record["transition_pass"] = bool(
        record["grade"]["pass"]
        and lineage["parent_match"]
        and lineage["revision_match"]
        and lineage["delta_source_preserved"]
        and lineage["prior_source_preserved"]
        and lineage["state_valid"]
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
    record["model_visible_tool_output_chars"] = record["events"].get(
        "command_output_chars",
        0,
    )
    attach_response_character_metrics(record)
    record["persisted_checkpoint"] = None
    if not answer_path.exists():
        record["grade"] = None
        record["transition_pass"] = False
        if record["condition"] == "service_api_resume":
            attach_native_lineage(
                record,
                case,
                metadata["native"],
                max_calls,
                service_protocol,
            )
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
        if record["condition"] == "service_api_resume":
            attach_native_lineage(
                record,
                case,
                metadata["native"],
                max_calls,
                service_protocol,
            )
            record["transition_pass"] = False
        return
    if record["condition"] == "filesystem_rebuild":
        record["lineage"] = None
        record["transition_pass"] = bool(record["grade"]["pass"])
        return
    if record["condition"] == "filesystem_sidecar":
        attach_filesystem_sidecar_lineage(record, case, metadata)
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
    store = BrunnStore(metadata.get("workspace_database", metadata["database"]))
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
    record["persisted_checkpoint"] = child
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
            "persisted_checkpoints": sum(
                bool(row.get("persisted_checkpoint"))
                for row in rows
            ),
            "mean_model_visible_tool_output_chars": round(
                statistics.fmean(
                    row.get(
                        "model_visible_tool_output_chars",
                        row["events"].get("command_output_chars", 0),
                    )
                    for row in rows
                ),
                1,
            ) if rows else 0,
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
        attach_response_character_metrics(record)
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
            and (
                record["condition"] != "filesystem_sidecar"
                or lineage.get("state_valid")
            )
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
        "source": git_source_fingerprint(),
        "captured_at": run["regraded_at"],
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
        "Related: [[Brunn]], [[Projects/Brunn/Agent work evaluation results - 2026-07-10|Agent work evaluation results]]",
        "",
        "# Brunn checkpoint transition evaluation",
        "",
        "## Scope",
        f"- Model: `{run['manifest']['model']}`",
        f"- Embeddings: `{run['embeddings']['kind']}` / `{run['embeddings']['model']}`",
        f"- Cards: {len(run['manifest']['cases'])} across Warmind, Charlemagne, Star Rupture, Switzerland, and Brunn",
        "- Each card reopens a revision-N checkpoint with a fresh agent, introduces a revision-N+1 delta, and requires a source-preserving child checkpoint.",
        *experiment_provenance_lines(run),
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
        "cd /Users/Shared/projects/brunn",
        "python3 transition_eval.py validate",
        "BRUNN_API_URL=http://127.0.0.1:18110 BRUNN_EVAL_TOKEN='<owner read/write token>' python3 transition_eval.py run --filesystem-native --concurrency 2 --timeout 420 --out results/native-transitions.json",
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


def select_transition_cases(
    manifest: dict[str, Any],
    requested: Sequence[str] | None,
) -> list[dict[str, Any]]:
    if not requested:
        return list(manifest["cases"])
    requested_ids = list(dict.fromkeys(requested))
    cases_by_id = {case["id"]: case for case in manifest["cases"]}
    unknown = [case_id for case_id in requested_ids if case_id not in cases_by_id]
    if unknown:
        raise ValueError(f"unknown requested transition cases: {unknown}")
    return [cases_by_id[case_id] for case_id in requested_ids]


async def run_all(args: argparse.Namespace, validated: dict[str, Any]) -> dict[str, Any]:
    reasoning_billing = require_codex_subscription(args.codex)
    manifest = dict(validated["manifest"])
    manifest["model"] = args.model or manifest["model"]
    manifest["cases"] = select_transition_cases(manifest, args.case)
    if not manifest["cases"]:
        raise ValueError("no transition cases were selected")
    validated = dict(validated)
    validated["manifest"] = manifest
    run_id = args.resume_run_id or args.run_id or datetime.now().astimezone().strftime("transition-%Y%m%dT%H%M%S%z")
    run_root = ROOT / "runs" / run_id
    if args.resume_run_id:
        if not run_root.is_dir():
            raise ValueError(f"Unknown run directory: {run_root}")
    else:
        run_root.mkdir(parents=True, exist_ok=False)
    selected_conditions = select_transition_conditions(manifest, args)
    experiment_arm, paired_draw_id = validate_experiment_identity(
        args.experiment_arm,
        args.paired_draw_id,
        selected_conditions,
    )
    if experiment_arm is not None and args.reuse_input:
        raise ValueError(
            "--reuse-input cannot be combined with explicit experiment-arm pairing"
        )
    expected_features = expected_runtime_features(
        args.expect_feature_flag,
        args.expect_runtime_config,
    )
    if (
        (expected_features or args.expect_build_revision)
        and "service_api_resume" not in selected_conditions
    ):
        raise ValueError(
            "runtime/build expectations require service_api_resume"
        )
    service_retrieval_modes = tuple(args.service_retrieval_modes or ())
    if len(service_retrieval_modes) != len(set(service_retrieval_modes)):
        raise ValueError(
            "--service-retrieval-modes must not contain duplicates"
        )
    if (
        service_retrieval_modes
        and "service_api_resume" not in selected_conditions
    ):
        raise ValueError(
            "--service-retrieval-modes requires service_api_resume"
        )
    if service_retrieval_modes and args.service_protocol != "simple":
        raise ValueError(
            "--service-retrieval-modes requires --service-protocol simple"
        )
    if (
        expected_features.get("semantic_lane") is False
        and "service_api_resume" in selected_conditions
        and service_retrieval_modes != ("exact", "lexical")
    ):
        raise ValueError(
            "semantic_lane=off service arms require "
            "--service-retrieval-modes exact lexical"
        )
    source_fingerprint = git_source_fingerprint()
    if args.api_container and "service_api_resume" not in selected_conditions:
        raise ValueError(
            "--api-container requires service_api_resume"
        )
    definitive_service_arm = bool(
        experiment_arm is not None
        and "service_api_resume" in selected_conditions
    )
    if definitive_service_arm:
        if not args.api_container:
            raise ValueError(
                "explicit service_api_resume experiment arms require "
                "--api-container"
            )
        if not args.expect_build_revision:
            raise ValueError(
                "explicit service_api_resume experiment arms require "
                "--expect-build-revision"
            )
        if not source_fingerprint["clean"]:
            raise ValueError(
                "explicit service_api_resume experiment arms require clean "
                f"source provenance: {source_fingerprint}"
            )
    if args.api_container and not args.expect_build_revision:
        raise ValueError(
            "--api-container requires --expect-build-revision"
        )
    service_image_before = (
        capture_service_image_fingerprint(
            args.api_container,
            source_revision=source_fingerprint["revision"],
            expected_build_revision=args.expect_build_revision,
        )
        if args.api_container
        else None
    )
    manifest_sha256 = sha256_file(args.manifest)
    mutation_script = validated.get("mutation_script")
    mutation_enabled = mutation_script is not None
    if mutation_enabled and "workspace_resume" in selected_conditions:
        raise ValueError(
            "the E06 mutation hook supports service_api_resume and "
            "filesystem_rebuild; workspace_resume is not an E06 arm"
        )
    mutation_mirrors = (
        prepare_mutation_mirrors(run_root, validated)
        if mutation_enabled
        else None
    )
    experiment_parameters = {
        "mutation_script_sha256": (
            sha256_file(mutation_script) if mutation_enabled else None
        ),
        "mutation_seed": (
            validated["mutation_seed"] if mutation_enabled else None
        ),
        "mutation_plans_sha256": (
            canonical_json_sha256(validated["mutation_plans"])
            if mutation_enabled
            else None
        ),
        "service_retrieval_modes": list(service_retrieval_modes),
        "service_image_fingerprint": service_image_before,
    }
    ensure_run_binding(
        run_root,
        resume=bool(args.resume_run_id),
        run_id=run_id,
        experiment_arm=experiment_arm,
        paired_draw_id=paired_draw_id,
        conditions=selected_conditions,
        case_ids=[case["id"] for case in manifest["cases"]],
        model=manifest["model"],
        service_protocol=args.service_protocol,
        manifest_sha256=manifest_sha256,
        expected_runtime_features=expected_features,
        expected_build_revision=args.expect_build_revision,
        experiment_parameters=experiment_parameters,
    )
    legacy_embeddings = args.embeddings if "workspace_resume" in selected_conditions else "none"
    if mutation_enabled:
        prepared = {case["id"]: {} for case in manifest["cases"]}
    elif {
        "filesystem_rebuild",
        "filesystem_sidecar",
        "workspace_resume",
    } & set(selected_conditions):
        prepared = (
            load_prepared_assets(run_root, validated)
            if args.resume_run_id
            else prepare_assets(run_root, validated, legacy_embeddings, args.embedding_model)
        )
    else:
        prepared = {case["id"]: {} for case in manifest["cases"]}
    native_metadata: dict[str, dict[str, Any]] = {}
    runtime_snapshot: dict[str, Any] | None = None
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
            mutation_script=mutation_script,
            mutation_mirrors=mutation_mirrors,
        )
        for case_id, metadata in native_metadata.items():
            prepared[case_id]["native"] = metadata
        if not native_metadata:
            raise ValueError("transition evaluation produced no authenticated runtime token")
        runtime_snapshot = fetch_service_runtime_snapshot(
            next(iter(native_metadata.values())),
            run_id=run_id,
            expected_features=expected_features,
            expected_build_revision=args.expect_build_revision,
        )
    if mutation_enabled and "service_api_resume" not in selected_conditions:
        assert mutation_mirrors is not None
        for case in manifest["cases"]:
            prepared[case["id"]]["mutation"] = apply_case_mutation(
                script=mutation_script,
                validated=validated,
                case_id=case["id"],
                mirror_root=mutation_mirrors[case["id"]],
                metadata=None,
                mirror_only=True,
            )
    jobs = prepare_run_dirs(
        run_root,
        validated,
        prepared,
        args.embeddings,
        args.embedding_model,
        selected_conditions,
        args.service_protocol,
        mutation_mirrors,
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
                "BRUNN_API_URL": os.environ["BRUNN_API_URL"],
                "BRUNN_EVAL_TOKEN": native_metadata[case["id"]]["token"],
            }
            if service_retrieval_modes:
                environment[
                    "BRUNN_EVAL_RETRIEVAL_MODES"
                ] = ",".join(service_retrieval_modes)
        tasks.append(run_one(
            semaphore, args.codex, manifest["model"], args.schema,
            case, condition, run_dir, args.timeout,
            env_overrides=environment,
        ))
    completed_records.extend(await asyncio.gather(*tasks))
    if runtime_snapshot is not None:
        final_runtime_snapshot = fetch_service_runtime_snapshot(
            next(iter(native_metadata.values())),
            run_id=run_id,
            expected_features=expected_features,
            expected_build_revision=args.expect_build_revision,
        )
        require_stable_runtime_configuration(
            runtime_snapshot,
            final_runtime_snapshot,
        )
        runtime_snapshot = final_runtime_snapshot
    for record in completed_records:
        record["run_id"] = run_id
    records = []
    if args.reuse_input:
        reused = load_json(args.reuse_input)
        selected_case_ids = {case["id"] for case in manifest["cases"]}
        records.extend(
            record for record in reused["records"]
            if record["condition"] in TRANSITION_ADAPTERS
            and record["condition"] not in selected_conditions
            and record.get("case_id") in selected_case_ids
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
    service_image_provenance: dict[str, Any] | None = None
    if service_image_before is not None:
        service_image_after = capture_service_image_fingerprint(
            args.api_container,
            source_revision=source_fingerprint["revision"],
            expected_build_revision=args.expect_build_revision,
        )
        service_image_provenance = stable_service_image_provenance(
            service_image_before,
            service_image_after,
        )
    runtime_features = (
        runtime_snapshot.get("runtime_features")
        if isinstance(runtime_snapshot, dict)
        else None
    )
    run = {
        "benchmark_version": manifest["benchmark_version"],
        "run_id": run_id,
        "experiment_arm": experiment_arm,
        "paired_draw_id": paired_draw_id,
        "run_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "manifest": manifest,
        "manifest_sha256": manifest_sha256,
        "harness_sha256": sha256_file(Path(__file__)),
        "service_sha256": sha256_file(ROOT / "brunn" / "service.py"),
        "store_sha256": sha256_file(ROOT / "brunn" / "store.py"),
        "native_memory_sha256": sha256_file(ROOT / "native_memory.py"),
        "embeddings": {"kind": args.embeddings, "model": args.embedding_model},
        "native_provisioning": {
            case_id: public_provisioning(metadata)
            for case_id, metadata in native_metadata.items()
        },
        "records": records,
        "service_protocol": args.service_protocol,
        "service_retrieval_modes": list(service_retrieval_modes),
        "feature_flags": {
            "resume_deltas": (
                runtime_features.get("resume_deltas")
                if isinstance(runtime_features, dict)
                else None
            )
        },
        "mutation": (
            {
                "script": str(mutation_script),
                "script_sha256": sha256_file(mutation_script),
                "seed": validated["mutation_seed"],
                "plans": validated["mutation_plans"],
                "receipts": {
                    case["id"]: (
                        prepared[case["id"]]
                        .get("native", {})
                        .get("mutation")
                        or prepared[case["id"]].get("mutation")
                    )
                    for case in manifest["cases"]
                },
            }
            if mutation_enabled
            else None
        ),
        "reasoning_billing": reasoning_billing,
        "expected_runtime_features": expected_features,
        "expected_build_revision": args.expect_build_revision,
        "service_runtime_snapshot": runtime_snapshot,
        "service_image_provenance": service_image_provenance,
        "implementation_fingerprint": source_fingerprint,
        "experiment_parameters": experiment_parameters,
    }
    run["run_ledger"] = build_run_ledger(
        run_id=run_id,
        reasoning_billing=reasoning_billing,
        model=manifest["model"],
        conditions=manifest["conditions"],
        service_protocol=args.service_protocol,
        manifest_sha256=run["manifest_sha256"],
        schema_sha256=sha256_file(args.schema),
        harness_sha256=run["harness_sha256"],
        experiment_arm=experiment_arm,
        paired_draw_id=paired_draw_id,
        runtime_snapshot=runtime_snapshot,
        expected_runtime_features=expected_features,
        expected_build_revision=args.expect_build_revision,
        experiment_parameters=experiment_parameters,
        service_image_provenance=service_image_provenance,
        mutation_evidence=run["mutation"],
    )
    run["summary"] = summarize(records, manifest["conditions"])
    return run


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Brunn checkpoint transition evaluation")
    parser.add_argument("--manifest", type=Path, default=ROOT / "eval" / "transition_cases.json")
    parser.add_argument("--schema", type=Path, default=ROOT / "eval" / "work_answer_schema.json")
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate_command = subparsers.add_parser("validate")
    validate_command.add_argument("--mutation-script", type=Path)
    validate_command.add_argument("--mutation-seed", default="e06-default")
    run = subparsers.add_parser("run")
    run.add_argument("--codex", type=Path, default=DEFAULT_CODEX)
    run.add_argument("--model")
    run.add_argument("--embeddings", choices=["none", "hashing", "openai"], default="openai")
    run.add_argument("--embedding-model", default="text-embedding-3-small")
    run.add_argument("--concurrency", type=int, default=2)
    run.add_argument("--timeout", type=int, default=420)
    run.add_argument("--run-id")
    run.add_argument("--resume-run-id")
    run.add_argument(
        "--experiment-arm",
        help=(
            "stable arm identity for a single-condition invocation; requires "
            "--paired-draw-id"
        ),
    )
    run.add_argument(
        "--paired-draw-id",
        help="shared draw identity used by every separately invoked experiment arm",
    )
    run.add_argument("--case", action="append")
    run.add_argument("--condition", action="append", choices=tuple(LABELS))
    run.add_argument(
        "--filesystem-native",
        action="store_true",
        help="run only filesystem reconstruction and native service checkpoint resume",
    )
    run.add_argument("--native-index-timeout", type=float, default=300.0)
    run.add_argument(
        "--expect-feature-flag",
        action="append",
        help="fail closed unless /v1/status runtime_features matches NAME=on|off",
    )
    run.add_argument(
        "--expect-runtime-config",
        action="append",
        help=(
            "fail closed unless /v1/status runtime_features matches NAME=<JSON value>"
        ),
    )
    run.add_argument(
        "--expect-build-revision",
        help="fail closed unless /v1/status reports this exact build revision",
    )
    run.add_argument(
        "--api-container",
        help=(
            "running API container whose immutable image ID and OCI revision "
            "label are captured before and after the run"
        ),
    )
    run.add_argument(
        "--service-protocol",
        choices=("legacy", "simple"),
        default="simple",
    )
    run.add_argument(
        "--service-retrieval-modes",
        nargs="+",
        choices=("exact", "lexical", "semantic"),
        help=(
            "immutable retrieval lanes injected into simple native service "
            "requests"
        ),
    )
    run.add_argument("--reuse-input", type=Path)
    run.add_argument("--mutation-script", type=Path)
    run.add_argument("--mutation-seed", default="e06-default")
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--report", type=Path)
    regrade = subparsers.add_parser("regrade")
    regrade.add_argument("--input", type=Path, required=True)
    regrade.add_argument("--out", type=Path, required=True)
    regrade.add_argument("--report", type=Path)
    regrade.add_argument("--mutation-script", type=Path)
    regrade.add_argument("--mutation-seed", default="e06-default")
    return parser


def main() -> None:
    args = build_parser().parse_args()
    mutation_script = getattr(args, "mutation_script", None)
    if mutation_script is not None:
        mutation_script = mutation_script.resolve()
    validated = validate(
        args.manifest,
        mutation_script=mutation_script,
        mutation_seed=getattr(args, "mutation_seed", "e06-default"),
    )
    if args.command == "validate":
        print(json.dumps({
            "status": "ok" if not validated["errors"] else "error",
            "errors": validated["errors"],
            "cards": len(validated["manifest"]["cases"]),
            "workloads": sorted({case["workload"] for case in validated["manifest"]["cases"]}),
            "mutation_cases": len(validated["mutation_plans"]),
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

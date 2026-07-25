#!/usr/bin/env python3

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import statistics
import time
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence

from native_eval import (
    NativeApiClient,
    provisioning_matches_run_case,
    provision_evaluation,
    public_provisioning,
    text_documents,
    write_native_memory_wrapper,
)
from straylight_eval import BM25Index
from workspace_cli import corpus_hash, diverse_results, load_corpus


PROJECT_ROOT = Path(__file__).resolve().parent
DEFAULT_MANIFEST = PROJECT_ROOT / "eval" / "work_cases.json"
DEFAULT_SCHEMA = PROJECT_ROOT / "eval" / "work_answer_schema.json"


def resolve_codex_path(candidates: Sequence[Path] | None = None) -> Path:
    paths = list(candidates or (
        Path.home() / ".local" / "bin" / "codex",
        Path("/Applications/ChatGPT.app/Contents/Resources/codex"),
        Path("/Applications/Codex.app/Contents/Resources/codex"),
    ))
    for path in paths:
        if path.is_file() and os.access(path, os.X_OK):
            return path
    return paths[0]


DEFAULT_CODEX = resolve_codex_path()
NATIVE_PROVISIONING_STATE = ".native-provisioning.json"


def load_native_provisioning_state(path: Path, run_id: str) -> dict[str, dict[str, Any]]:
    if not path.exists():
        return {}
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("version") != 1 or payload.get("run_id") != run_id:
        raise ValueError(f"Invalid native provisioning state: {path}")
    cases = payload.get("cases")
    if not isinstance(cases, dict):
        raise ValueError(f"Invalid native provisioning cases: {path}")
    path.chmod(0o600)
    return cases


def write_native_provisioning_state(
    path: Path,
    run_id: str,
    cases: dict[str, dict[str, Any]],
) -> None:
    temporary = path.with_name(f"{path.name}.tmp")
    temporary.write_text(
        json.dumps({"version": 1, "run_id": run_id, "cases": cases}, indent=2) + "\n",
        encoding="utf-8",
    )
    temporary.chmod(0o600)
    temporary.replace(path)
    path.chmod(0o600)
CONDITION_ADAPTERS = {
    "fixed_pack": {"label": "Fixed handoff pack", "kind": "fixed_pack"},
    "filesystem": {"label": "Filesystem agent", "kind": "filesystem"},
    "workspace": {"label": "Straylight workspace agent", "kind": "legacy_workspace"},
    "service_api": {"label": "Native Straylight API agent", "kind": "native_service"},
}
CONDITION_LABELS = {key: value["label"] for key, value in CONDITION_ADAPTERS.items()}
WORKSPACE_CONDITIONS = {"workspace", "service_api"}
CONCEPT_STOPWORDS = {
    "a", "an", "and", "are", "as", "at", "be", "been", "being", "both",
    "by", "did", "do", "does", "each", "for", "from", "is", "it", "its",
    "itself", "must", "of", "or", "remain", "remaining", "remains", "still",
    "that", "the", "their", "these", "this", "those", "through", "to", "was",
    "were", "with",
}
CONCEPT_IRREGULAR = {
    "aliases": "alias",
    "addresses": "address",
    "changed": "change",
    "changes": "change",
    "claims": "claim",
    "dossiers": "dossier",
    "exclude": "remove",
    "excluded": "remove",
    "excludes": "remove",
    "excluding": "remove",
    "ids": "id",
    "moved": "move",
    "moves": "move",
    "names": "name",
    "not": "no",
    "none": "no",
    "rebuilt": "rebuild",
    "retained": "retain",
    "retaining": "retain",
    "retains": "retain",
    "rewritten": "rewrite",
    "sources": "source",
    "states": "state",
}


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_tree(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def normalize(value: str) -> str:
    value = value.casefold().replace("–", "-").replace("—", "-")
    value = re.sub(r"[`*_]", "", value)
    value = re.sub(r"\s+", " ", value)
    return value.strip()


def concept_stem(token: str) -> str:
    token = CONCEPT_IRREGULAR.get(token, token)
    if token in CONCEPT_IRREGULAR.values():
        return token
    if len(token) > 5 and token.endswith("ing"):
        token = token[:-3]
        if len(token) > 2 and token[-1] == token[-2]:
            token = token[:-1]
    elif len(token) > 4 and token.endswith("ed"):
        token = token[:-2]
    elif len(token) > 4 and token.endswith("s"):
        token = token[:-1]
    return CONCEPT_IRREGULAR.get(token, token)


def concept_tokens(value: str) -> set[str]:
    result: set[str] = set()
    for raw in re.findall(r"[a-z0-9]+(?:[._/+:-][a-z0-9]+)*", value.casefold()):
        rate = re.fullmatch(
            r"(?:\d+(?:\.\d+)?|[a-z]+)/"
            r"(?:s|sec|second|min|minute|h|hr|hour|day)",
            raw,
        )
        if rate:
            parts = raw.split("/")
        elif ":" in raw or "/" in raw or re.match(r"^\d{4}-\d{2}-", raw):
            parts = [raw]
        else:
            parts = re.split(r"[._+-]+", raw)
        for part in parts:
            normalized = concept_stem(part)
            if normalized and normalized not in CONCEPT_STOPWORDS:
                result.add(normalized)
    return result


def candidate_matches(candidate: str, value: str, mode: str) -> bool:
    if normalize(candidate) in normalize(value):
        return True
    if mode != "concept_tokens_v1":
        return False
    candidate_concepts = concept_tokens(candidate)
    return bool(candidate_concepts) and candidate_concepts <= concept_tokens(value)


def forbidden_is_asserted(forbidden: str, rendered: str) -> bool:
    phrase = normalize(forbidden)
    start = 0
    while (match_at := rendered.find(phrase, start)) >= 0:
        prefix = rendered[max(0, match_at - 240):match_at]
        clause = re.split(r"[.!?;]", prefix)[-1]
        negated = re.search(
            r"\b(?:cannot|can't|do not|does not|did not|must not|never|no|should not|without)\b",
            clause,
        )
        contrast = re.search(r"\b(?:but|except|however|instead)\b", clause)
        if not negated or (contrast and contrast.start() > negated.start()):
            return True
        start = match_at + len(phrase)
    return False


def load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_json_answer(path: Path) -> dict:
    raw = path.read_text(encoding="utf-8").strip()
    if raw.startswith("```"):
        raw = re.sub(r"^```(?:json)?\s*", "", raw)
        raw = re.sub(r"\s*```$", "", raw)
    return json.loads(raw)


def case_query(case: dict) -> str:
    slots = " ".join(case["claim_slots"].values())
    return f"{case['task']} {slots}"


def render_fixed_context(case: dict, index: BM25Index, manifest: dict) -> str:
    ranked = index.search(case_query(case))
    results = diverse_results(
        ranked,
        limit=manifest["fixed_pack_chunks"],
        max_chars=manifest["fixed_pack_chars"],
    )
    lines = [
        "# Fixed handoff context",
        "",
        "This context was assembled once from the task. It cannot be expanded during the run.",
    ]
    for item in results:
        lines.extend([
            "",
            f"## {item['path']} :: {item['heading']} :: lines {item['lines'][0]}-{item['lines'][1]}",
            item["text"],
        ])
    return "\n".join(lines) + "\n"


def render_prompt(case: dict, condition: str) -> str:
    scope = case.get("scope", case["workload"])
    if condition == "fixed_pack":
        access = (
            "Read ./context.md. It is your only evidence source. Do not browse, search elsewhere on the machine, "
            "or invent missing details. If the pack is incomplete, state the gap."
        )
    elif condition == "filesystem":
        access = (
            "The frozen evidence corpus is available at ./corpus. Use ordinary filesystem tools such as rg, sed, "
            "and read-only scripts. Do not browse, modify corpus files, or use paths outside ./corpus."
        )
    elif condition == "service_api" and case.get("workspace_access") == "read_only":
        access = (
            "Use the read-only native Straylight service through ./memory only. Do not inspect the wrapper or any corpus path. "
            "Run `./memory open` and treat its initial evidence, learned context, checkpoint, and revision delta as the first "
            "answer packet. A `complete_source` item is already the full source and must not be read again. Pointer-only "
            "`evidence_leads` identify additional sources to read only when a requested task facet is absent. Treat "
            "`retrieval_sufficiency.status=likely_sufficient` as evidence that the primary source is complete and task anchors "
            "are covered, not as proof that every requested output facet is established. Build a facet checklist from the task "
            "and claim slots; do not query again for facts that packet directly establishes. For unresolved checklist gaps, use one focused "
            f"`./memory query --scope {json.dumps(scope)} \"question\"`, or `--batch` only for independent gaps. Follow a "
            "candidate into its source with `./memory read --ref \"chunk:...\" --neighbors 4` or an exact line range. A candidate "
            "excerpt is a source lead, not proof that the displayed section is complete: if its path clearly owns an unresolved "
            "claim, read that exact path before searching globally again. A single read call can repeat `--path` or `--ref` to "
            "fetch several exact sources or ranges together. "
            "This credential cannot checkpoint, save, or mutate corpus or staged state. Keep the whole run to four service calls "
            "when the evidence permits. Do not browse or use filesystem search outside this service surface."
        )
    elif condition == "service_api":
        access = (
            "Use the native Straylight service through ./memory only. Do not inspect the wrapper or any corpus path. "
            "Run `./memory open` and treat its initial evidence, learned context, checkpoint, and revision delta as the first "
            "answer packet. A `complete_source` item is already the full source and must not be read again. Pointer-only "
            "`evidence_leads` identify additional sources to read only when a requested task facet is absent. Treat "
            "`retrieval_sufficiency.status=likely_sufficient` as evidence that the primary source is complete and task anchors "
            "are covered, not as proof that every requested output facet is established. Build a facet checklist from the task "
            "and claim slots; do not query again for facts that packet directly establishes. For unresolved checklist gaps, use one focused "
            f"`./memory query --scope {json.dumps(scope)} \"question\"`, or `--batch` only for independent gaps. Follow a "
            "candidate into its source with `./memory read --ref \"chunk:...\" --neighbors 4` or an exact line range. A candidate "
            "excerpt is a source lead, not proof that the displayed section is complete: if its path clearly owns an unresolved "
            "claim, read that exact path before searching globally again. A single read call can repeat `--path` or `--ref` to "
            "fetch several exact sources or ranges together. "
            "Use one typed compute or combined verify call only when useful. Before answering, persist one checkpoint through "
            "./memory checkpoint. Keep the whole run to four service calls when the evidence permits. Do not browse or use "
            "filesystem search outside this service surface."
        )
    elif case.get("workspace_access") == "read_only":
        access = (
            "Use the read-only Straylight workspace through ./memory only. Do not inspect the wrapper or corpus path directly. "
            f"Start with ./memory open --scope {json.dumps(scope)}, then use only targeted query, read, compute, or verify "
            "operations. This credential cannot checkpoint or mutate corpus or staged state. Do not browse or use filesystem "
            "search outside this workspace surface."
        )
    else:
        access = (
            "Use the Straylight workspace through ./memory only. Do not inspect the wrapper or corpus path directly. "
            f"Start with ./memory open --scope {json.dumps(scope)}, then use a small number of targeted query and read "
            "operations. Use compute for arithmetic and one combined verify when useful; do not verify every claim separately "
            "when the same sources cover them. Before answering, persist a checkpoint with ./memory checkpoint. Do not browse "
            "or use filesystem search outside this workspace surface."
        )

    slot_lines = "\n".join(f"- {claim_id}: {label}" for claim_id, label in case["claim_slots"].items())
    checkpoint_instruction = (
        "For this read-only workspace case, the required checkpoint-shaped response is an output proposal only. "
        "Do not persist it through ./memory."
        if condition in WORKSPACE_CONDITIONS and case.get("workspace_access") == "read_only"
        else "The checkpoint must be useful to the next fresh agent."
    )
    return f"""You are a fresh agent taking over durable work from prior agents.

{access}

Task:
{case['task']}

Return the required JSON object. The `claims` array must contain exactly one entry for each claim slot below, using the exact ID. Put the factual answer for that slot in `value` and cite relative corpus paths in `source_paths`.
For service retrieval, cite only exact `path` values returned with candidates, never a path merely mentioned inside candidate content.
When a claim characterizes a source's authority, cite that original source, not only an index or summary that discusses it.
Copy stable IDs, ISO-8601 timestamps, timezones, hashes, quantities, and measurements exactly from authoritative evidence. Do not silently normalize or reconstruct exact values from memory.
Before checkpointing, check every task facet against the claim slot responsible for it. Keep every claim slot self-contained; repeat a fact when it is needed in more than one slot, and do not rely on details stated only in another claim, the answer summary, or the checkpoint. When reporting an inventory, implementation state, public/private boundary, or operational behavior, preserve the concrete names, thresholds, state transitions, and failure or recovery rules that establish the conclusion rather than replacing them with counts, symbolic IDs, or broad categories. Cite the source that directly supports each slot's details, even when another source elsewhere in the answer covers adjacent context.
{slot_lines}

{checkpoint_instruction} Record the objective, current state, decisions, unresolved questions, concrete next actions, and the source or output artifacts that matter. Distinguish current fact from proposal, historical evidence from superseding state, and verified results from incomplete work. Use only evidence available under this condition.
"""


def write_memory_wrapper(run_dir: Path, corpus: Path, access_mode: str) -> None:
    wrapper = run_dir / "memory"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"exec python3 '{PROJECT_ROOT / 'workspace_cli.py'}' "
        f"--corpus '{corpus}' --session '{run_dir / 'workspace-session.json'}' "
        f"--access-mode '{access_mode}' \"$@\"\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)


def prepare_case_dir(
    root: Path,
    case: dict,
    condition: str,
    *,
    corpus: Path,
    index: BM25Index,
    manifest: dict,
    run_id: str | None = None,
    native_metadata: dict[str, Any] | None = None,
) -> tuple[Path, int]:
    run_dir = root / condition / case["id"]
    run_dir.mkdir(parents=True, exist_ok=False)
    context_chars = 0
    if condition == "fixed_pack":
        context = render_fixed_context(case, index, manifest)
        (run_dir / "context.md").write_text(context, encoding="utf-8")
        context_chars = len(context)
    elif condition == "filesystem":
        (run_dir / "corpus").symlink_to(corpus, target_is_directory=True)
    elif condition == "workspace":
        write_memory_wrapper(
            run_dir,
            corpus,
            case.get("workspace_access", "read_write"),
        )
    elif condition == "service_api":
        if not run_id or native_metadata is None:
            raise ValueError("service_api requires provisioned native metadata")
        write_native_memory_wrapper(
            run_dir,
            task=case["task"],
            display_scope=case.get("scope", case["workload"]),
            authorization_scope=native_metadata["authorization_scope"],
            run_id=run_id,
            case_id=case["id"],
            checkpoint_id=native_metadata.get("checkpoint_id"),
        )
    else:
        raise ValueError(f"Unknown condition adapter: {condition}")
    (run_dir / "prompt.txt").write_text(render_prompt(case, condition), encoding="utf-8")
    return run_dir, context_chars


def recursive_token_usage(value: Any, totals: dict[str, int]) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key in {"input_tokens", "output_tokens", "cached_input_tokens"} and isinstance(item, int):
                totals[key] = max(totals.get(key, 0), item)
            else:
                recursive_token_usage(item, totals)
    elif isinstance(value, list):
        for item in value:
            recursive_token_usage(item, totals)


def parse_event_metrics(path: Path) -> dict:
    totals: dict[str, int] = {}
    event_count = 0
    command_count = 0
    command_output_chars = 0
    if not path.exists():
        return {"events": 0, "commands": 0, "command_output_chars": 0, "tokens": totals}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        event_count += 1
        recursive_token_usage(event, totals)
        item = event.get("item", {})
        if event.get("type") == "item.completed" and item.get("type") == "command_execution":
            command_count += 1
            command_output_chars += len(item.get("aggregated_output") or "")
    return {
        "events": event_count,
        "commands": command_count,
        "command_output_chars": command_output_chars,
        "tokens": totals,
    }


def source_matches(cited: str, expected: Sequence[str]) -> bool:
    normalized = normalize_citation_path(cited)
    return normalized in expected


def normalize_citation_path(cited: str) -> str:
    normalized = cited.strip()
    if normalized.startswith("./"):
        normalized = normalized[2:]
    if normalized.startswith("corpus/"):
        normalized = normalized[len("corpus/"):]
    if (
        not normalized
        or normalized.startswith("/")
        or any(part in {"", ".", ".."} for part in normalized.split("/"))
    ):
        return ""
    return normalized


def recompute_grade(grade: dict) -> None:
    claims = grade.get("claims", [])
    claim_score = (
        statistics.fmean(float(item.get("score") or 0) for item in claims)
        if claims
        else 0.0
    )
    full_claims = sum(1 for item in claims if item.get("pass"))
    checkpoint_score = float(grade.get("checkpoint_score") or 0)
    citation_validity = float(grade.get("citation_validity") or 0)
    score = 0.85 * claim_score + 0.1 * checkpoint_score + 0.05 * citation_validity
    grade["score"] = round(score, 4)
    grade["claims_passed"] = full_claims
    grade["claims_total"] = len(claims)
    grade["claim_score"] = round(claim_score, 4)
    grade["pass"] = bool(
        score >= 0.8
        and full_claims == len(claims)
        and grade.get("claim_set_valid") is True
        and not grade.get("forbidden_hits")
    )


def grade_answer(case: dict, answer: dict, corpus_paths: set[str]) -> dict:
    answer_claims = answer.get("claims", [])
    if not isinstance(answer_claims, list):
        answer_claims = []
    actual_claim_ids = [
        str(claim.get("id"))
        for claim in answer_claims
        if isinstance(claim, dict) and isinstance(claim.get("id"), str)
    ]
    expected_claim_ids = [str(rubric["id"]) for rubric in case["rubric"]]
    duplicate_claim_ids = sorted({
        claim_id
        for claim_id in actual_claim_ids
        if actual_claim_ids.count(claim_id) > 1
    })
    missing_claim_ids = sorted(set(expected_claim_ids) - set(actual_claim_ids))
    extra_claim_ids = sorted(set(actual_claim_ids) - set(expected_claim_ids))
    malformed_claim_count = sum(
        not isinstance(claim, dict) or not isinstance(claim.get("id"), str)
        for claim in answer_claims
    )
    claim_set_valid = bool(
        not duplicate_claim_ids
        and not missing_claim_ids
        and not extra_claim_ids
        and malformed_claim_count == 0
        and len(actual_claim_ids) == len(expected_claim_ids)
    )
    claims_by_id = {
        claim["id"]: claim
        for claim in answer_claims
        if isinstance(claim, dict) and isinstance(claim.get("id"), str)
    }
    claim_results = []
    all_citations: list[str] = []
    for rubric in case["rubric"]:
        claim = claims_by_id.get(rubric["id"], {})
        value = str(claim.get("value", ""))
        grading_mode = case.get("grading_mode", "exact_substring_v1")
        checks = []
        for check in rubric["checks"]:
            matched = next(
                (
                    candidate
                    for candidate in check["any"]
                    if candidate_matches(candidate, value, grading_mode)
                ),
                None,
            )
            checks.append({"alternatives": check["any"], "matched": matched})
        citations = claim.get("source_paths", []) if isinstance(claim.get("source_paths", []), list) else []
        all_citations.extend(citations)
        expected_all = rubric.get("sources_all")
        if isinstance(expected_all, list):
            source_hit = bool(expected_all) and all(
                any(source_matches(citation, [expected]) for citation in citations)
                for expected in expected_all
            )
        else:
            source_hit = any(
                source_matches(citation, rubric["sources_any"])
                for citation in citations
            )
        content_fraction = sum(1 for check in checks if check["matched"]) / max(1, len(checks))
        score = 0.8 * content_fraction + 0.2 * int(source_hit)
        claim_results.append({
            "id": rubric["id"],
            "score": round(score, 4),
            "pass": content_fraction == 1.0 and source_hit,
            "content_fraction": round(content_fraction, 4),
            "source_hit": source_hit,
            "checks": checks,
            "citations": citations,
            "native_paths": list(rubric.get("native_paths", [])),
        })

    checkpoint = answer.get("checkpoint", {})
    required_checkpoint = ["objective", "current_state", "next_actions", "artifacts"]
    checkpoint_fields = {
        field: bool(checkpoint.get(field))
        for field in required_checkpoint
    }
    checkpoint_score = sum(checkpoint_fields.values()) / len(required_checkpoint)
    rendered = normalize(json.dumps(answer, ensure_ascii=False))
    forbidden_hits = [
        item for item in case.get("forbidden", [])
        if forbidden_is_asserted(item, rendered)
    ]
    valid_citations = [
        citation for citation in all_citations
        if normalize_citation_path(citation) in corpus_paths
    ]
    citation_validity = len(valid_citations) / max(1, len(all_citations))
    result = {
        "score": 0.0,
        "pass": False,
        "claims_passed": 0,
        "claims_total": len(claim_results),
        "claim_score": 0.0,
        "checkpoint_score": round(checkpoint_score, 4),
        "checkpoint_fields": checkpoint_fields,
        "citation_validity": round(citation_validity, 4),
        "citation_count": len(all_citations),
        "forbidden_hits": forbidden_hits,
        "claims": claim_results,
        "claim_set_valid": claim_set_valid,
        "expected_claim_ids": expected_claim_ids,
        "actual_claim_ids": actual_claim_ids,
        "duplicate_claim_ids": duplicate_claim_ids,
        "missing_claim_ids": missing_claim_ids,
        "extra_claim_ids": extra_claim_ids,
        "malformed_claim_count": malformed_claim_count,
    }
    recompute_grade(result)
    return result


def attach_workspace_metrics(record: dict, run_dir: Path) -> None:
    record["service_operations"] = []
    record["service_calls"] = 0
    record["service_result_chars"] = 0
    record["service_source_text_chars"] = 0
    record["service_metadata_chars"] = 0
    record["service_replay_weighted_chars"] = 0
    record["service_latency_ms"] = 0.0
    record["service_http_calls"] = 0
    record["service_binary_bytes"] = 0
    record["service_checkpoint"] = None
    record["service_session_id"] = None
    record["service_corpus_revision"] = None
    session_path = run_dir / "workspace-session.json"
    record["workspace_operations"] = []
    record["workspace_result_chars"] = 0
    record["workspace_checkpoint"] = None
    record["workspace_state_error"] = None
    if session_path.exists():
        try:
            session = load_json(session_path)
        except (json.JSONDecodeError, OSError) as exc:
            record["workspace_state_error"] = str(exc)
        else:
            record["workspace_operations"] = session.get("operations", [])
            record["workspace_result_chars"] = sum(
                operation.get("result_chars", 0) for operation in session.get("operations", [])
            )
            record["workspace_checkpoint"] = session.get("checkpoint")

    native_path = run_dir / "native-session.json"
    if not native_path.exists():
        return
    try:
        native = load_json(native_path)
    except (json.JSONDecodeError, OSError) as exc:
        record["workspace_state_error"] = str(exc)
        return
    operations = native.get("operations", [])
    record["service_operations"] = operations
    record["service_calls"] = len(operations)
    record["service_http_calls"] = sum(
        int(operation.get("http_calls", 1)) for operation in operations
    )
    record["service_binary_bytes"] = sum(
        int(operation.get("binary_bytes", 0)) for operation in operations
    )
    record["service_result_chars"] = sum(operation.get("result_chars", 0) for operation in operations)
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
    record["service_latency_ms"] = round(
        sum(float(operation.get("elapsed_ms", 0)) for operation in operations),
        3,
    )
    record["service_checkpoint"] = native.get("checkpoint")
    record["service_session_id"] = native.get("session_id")
    record["service_corpus_revision"] = native.get("corpus_revision")
    record["workspace_operations"] = operations
    record["workspace_result_chars"] = record["service_result_chars"]
    record["workspace_checkpoint"] = native.get("checkpoint")


def load_existing_record(
    *,
    run_dir: Path,
    case: dict,
    condition: str,
    corpus_paths: set[str],
) -> dict:
    answer_path = run_dir / "answer.json"
    context_path = run_dir / "context.md"
    record = {
        "case_id": case["id"],
        "condition": condition,
        "exit_code": 0,
        "timed_out": False,
        "elapsed_seconds": None,
        "fixed_context_chars": len(context_path.read_text(encoding="utf-8")) if context_path.exists() else 0,
        "answer_path": str(answer_path),
        "error": None,
    }
    try:
        answer = parse_json_answer(answer_path)
        record["answer"] = answer
        record["grade"] = grade_answer(case, answer, corpus_paths)
    except (json.JSONDecodeError, OSError, TypeError) as exc:
        record["error"] = f"Invalid existing answer JSON: {exc}"
        record["grade"] = None
    record["events"] = parse_event_metrics(run_dir / "events.jsonl")
    attach_workspace_metrics(record, run_dir)
    return record


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
    if condition == "service_api":
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
    *,
    codex: Path,
    model: str,
    schema: Path,
    run_dir: Path,
    case: dict,
    condition: str,
    context_chars: int,
    corpus_paths: set[str],
    timeout_seconds: int,
    env_overrides: dict[str, str] | None = None,
) -> dict:
    async with semaphore:
        prompt = (run_dir / "prompt.txt").read_text(encoding="utf-8")
        answer_path = run_dir / "answer.json"
        events_path = run_dir / "events.jsonl"
        stderr_path = run_dir / "stderr.log"
        command = build_codex_command(
            codex=codex,
            model=model,
            schema=schema,
            run_dir=run_dir,
            condition=condition,
        )
        started = time.monotonic()
        env = os.environ.copy()
        for key in ["CODEX_THREAD_ID", "CODEX_INTERNAL_ORIGINATOR_OVERRIDE"]:
            env.pop(key, None)
        if env_overrides:
            env.update(env_overrides)
        process = await asyncio.create_subprocess_exec(
            *command,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env,
        )
        try:
            stdout, stderr = await asyncio.wait_for(
                process.communicate(prompt.encode()),
                timeout=timeout_seconds,
            )
            timed_out = False
        except asyncio.TimeoutError:
            process.kill()
            stdout, stderr = await process.communicate()
            timed_out = True
        elapsed = time.monotonic() - started
        events_path.write_bytes(stdout)
        stderr_path.write_bytes(stderr)
        record = {
            "case_id": case["id"],
            "condition": condition,
            "exit_code": process.returncode,
            "timed_out": timed_out,
            "elapsed_seconds": round(elapsed, 3),
            "fixed_context_chars": context_chars,
            "answer_path": str(answer_path),
            "error": None,
        }
        if process.returncode != 0 or not answer_path.exists():
            record["error"] = stderr.decode(errors="replace")[-4000:] or "Codex produced no answer"
            record["grade"] = None
        else:
            try:
                answer = parse_json_answer(answer_path)
                record["answer"] = answer
                record["grade"] = grade_answer(case, answer, corpus_paths)
            except (json.JSONDecodeError, OSError, TypeError) as exc:
                record["error"] = f"Invalid answer JSON: {exc}"
                record["grade"] = None
        record["events"] = parse_event_metrics(events_path)
        attach_workspace_metrics(record, run_dir)
        (run_dir / "record.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        return record


def summarize(manifest: dict, records: Sequence[dict]) -> dict:
    case_by_id = {case["id"]: case for case in manifest["cases"]}

    def group_summary(rows: Sequence[dict]) -> dict:
        graded = [row for row in rows if row.get("grade")]
        elapsed_values = [row["elapsed_seconds"] for row in rows if row["elapsed_seconds"] is not None]
        claim_passed = sum(row["grade"]["claims_passed"] for row in graded)
        claim_total = sum(row["grade"]["claims_total"] for row in graded)
        token_keys = ["input_tokens", "output_tokens", "cached_input_tokens"]
        mean_tokens = {
            key: round(statistics.fmean(row["events"]["tokens"].get(key, 0) for row in rows), 1) if rows else 0
            for key in token_keys
        }
        mean_tokens["uncached_input_tokens"] = round(
            mean_tokens["input_tokens"] - mean_tokens["cached_input_tokens"], 1
        )
        checkpoint_eligible_runs = sum(
            1
            for row in rows
            if row["condition"] in WORKSPACE_CONDITIONS
            and case_by_id.get(row["case_id"], {}).get("workspace_access") != "read_only"
        )
        return {
            "runs": len(rows),
            "successful_runs": len(graded),
            "cases_passed": sum(1 for row in graded if row["grade"]["pass"]),
            "case_pass_rate": sum(1 for row in graded if row["grade"]["pass"]) / max(1, len(rows)),
            "claims_passed": claim_passed,
            "claims_total": claim_total,
            "claim_pass_rate": claim_passed / max(1, claim_total),
            "mean_score": round(statistics.fmean(row["grade"]["score"] for row in graded), 4) if graded else 0,
            "elapsed_samples": len(elapsed_values),
            "mean_elapsed_seconds": round(statistics.fmean(elapsed_values), 2) if elapsed_values else None,
            "mean_fixed_context_chars": round(statistics.fmean(row["fixed_context_chars"] for row in rows), 1) if rows else 0,
            "mean_workspace_result_chars": round(statistics.fmean(row["workspace_result_chars"] for row in rows), 1) if rows else 0,
            "persisted_checkpoints": sum(1 for row in rows if row.get("workspace_checkpoint")),
            "checkpoint_eligible_runs": checkpoint_eligible_runs,
            "mean_service_calls": round(
                statistics.fmean(row.get("service_calls", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_result_chars": round(
                statistics.fmean(row.get("service_result_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_source_text_chars": round(
                statistics.fmean(row.get("service_source_text_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_metadata_chars": round(
                statistics.fmean(row.get("service_metadata_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_replay_weighted_chars": round(
                statistics.fmean(row.get("service_replay_weighted_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_latency_ms": round(
                statistics.fmean(row.get("service_latency_ms", 0.0) for row in rows), 3
            ) if rows else 0,
            "mean_completed_commands": round(
                statistics.fmean(row["events"].get("commands", 0) for row in rows), 1
            ) if rows else 0,
            "mean_command_output_chars": round(
                statistics.fmean(row["events"].get("command_output_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_tokens": mean_tokens,
        }

    by_condition = {
        condition: group_summary([row for row in records if row["condition"] == condition])
        for condition in manifest["conditions"]
    }
    by_workload = {}
    for workload in sorted({case["workload"] for case in manifest["cases"]}):
        ids = {case["id"] for case in manifest["cases"] if case["workload"] == workload}
        by_workload[workload] = {
            condition: group_summary([
                row for row in records
                if row["condition"] == condition and row["case_id"] in ids
            ])
            for condition in manifest["conditions"]
        }
    by_capability = {}
    for capability in sorted({case["capability"] for case in manifest["cases"]}):
        ids = {case["id"] for case in manifest["cases"] if case["capability"] == capability}
        by_capability[capability] = {
            condition: group_summary([
                row for row in records
                if row["condition"] == condition and row["case_id"] in ids
            ])
            for condition in manifest["conditions"]
        }
    return {
        "by_condition": by_condition,
        "by_workload": by_workload,
        "by_capability": by_capability,
        "case_metadata": case_by_id,
    }


def render_report(run: dict) -> str:
    manifest = run["manifest"]
    summary = run["summary"]
    labels = CONDITION_LABELS
    report_title = manifest.get("report_title", manifest["name"])
    report_date = manifest.get("report_date", run["run_at"][:10])
    workloads = ", ".join(sorted({case["workload"] for case in manifest["cases"]}))
    has_read_only = any(
        case.get("workspace_access") == "read_only"
        for case in manifest["cases"]
    )
    benchmark_version = str(manifest.get("benchmark_version", ""))
    if benchmark_version.startswith("personal-coordination"):
        manifest_argument = "eval/personal_coordination_cases.json"
    elif benchmark_version.startswith("rupture-ops"):
        manifest_argument = "eval/rupture_ops_cases.json"
    else:
        manifest_argument = "eval/work_cases.json"
    native_flag = " --filesystem-native" if "service_api" in manifest["conditions"] else ""
    condition_descriptions = {
        "fixed_pack": "- **Fixed handoff pack:** a fresh agent receives one task-specific context file and cannot retrieve more.",
        "filesystem": "- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.",
        "workspace": (
            "- **Straylight workspace agent:** a fresh agent uses the initial BM25-backed shell CLI. "
            "This legacy condition does not test semantic retrieval or the native API."
        ),
        "service_api": (
            "- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through "
            "batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations."
        ),
    }
    if has_read_only:
        condition_descriptions["workspace"] += " The read-only authorization card cannot persist a checkpoint."
        condition_descriptions["service_api"] += " Read-only credentials cannot persist a checkpoint."
    lines = [
        f"Created: {run['run_at']}",
        f"Updated: {run.get('regraded_at', run['run_at'])}",
        "Status: Complete",
        "",
        "Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]",
        "",
        f"# {report_title} - {report_date}",
        "",
        "## Scope",
        f"- Model: `{manifest['model']}`",
        f"- Corpus: {run['corpus']['documents']} files, {run['corpus']['characters']:,} characters, {run['corpus']['chunks']} chunks",
        f"- Corpus SHA-256: `{run['corpus']['sha256']}`",
        f"- Cases: {len(manifest['cases'])} complex work tasks with {sum(len(case['rubric']) for case in manifest['cases'])} scored claims",
        f"- Workloads: {workloads}",
        "- This evaluates agent work and durable checkpoints, not retrieval recall alone.",
        "",
        "## Conditions",
        *(condition_descriptions[condition] for condition in manifest["conditions"]),
        "",
        "## Results",
        "| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for condition in manifest["conditions"]:
        row = summary["by_condition"][condition]
        checkpoints = (
            f"{row['persisted_checkpoints']}/{row['checkpoint_eligible_runs']} eligible"
            if condition in WORKSPACE_CONDITIONS
            else "output only"
        )
        lines.append(
            f"| {labels[condition]} | {row['cases_passed']}/{row['runs']} ({row['case_pass_rate']:.0%}) "
            f"| {row['claims_passed']}/{row['claims_total']} ({row['claim_pass_rate']:.0%}) "
            f"| {row['mean_score']:.3f} | {checkpoints} |"
        )

    lines.extend([
        "",
        "## Token and tool accounting",
        "`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.",
        "",
        "| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ])
    for condition in manifest["conditions"]:
        row = summary["by_condition"][condition]
        tokens = row["mean_tokens"]
        lines.append(
            f"| {labels[condition]} | {tokens['input_tokens']:,.0f} | {tokens['cached_input_tokens']:,.0f} "
            f"| {tokens['uncached_input_tokens']:,.0f} | {tokens['output_tokens']:,.0f} "
            f"| {row['mean_completed_commands']:.1f} | {row['mean_command_output_chars']:,.0f} |"
        )

    lines.extend([
        "",
        "## Workload results",
        "| Workload | " + " | ".join(labels[condition] for condition in manifest["conditions"]) + " |",
        "| --- | " + " | ".join("---:" for _ in manifest["conditions"]) + " |",
    ])
    for workload, conditions in summary["by_workload"].items():
        cells = []
        for condition in manifest["conditions"]:
            row = conditions[condition]
            cells.append(f"{row['cases_passed']}/{row['runs']} cases, {row['claims_passed']}/{row['claims_total']} claims")
        lines.append(f"| {workload} | " + " | ".join(cells) + " |")

    lines.extend([
        "",
        "## Per-case results",
        "| Case | Capability | " + " | ".join(labels[condition] for condition in manifest["conditions"]) + " |",
        "| --- | --- | " + " | ".join("---:" for _ in manifest["conditions"]) + " |",
    ])
    record_map = {(record["condition"], record["case_id"]): record for record in run["records"]}
    for case in manifest["cases"]:
        cells = []
        for condition in manifest["conditions"]:
            record = record_map.get((condition, case["id"]))
            if not record or not record.get("grade"):
                cells.append("ERROR")
            else:
                grade = record["grade"]
                cells.append(
                    ("PASS" if grade["pass"] else "FAIL")
                    + f" {grade['claims_passed']}/{grade['claims_total']}"
                )
        lines.append(f"| `{case['id']}` | {case['capability']} | " + " | ".join(cells) + " |")

    successful = {
        condition: summary["by_condition"][condition]["case_pass_rate"]
        for condition in manifest["conditions"]
    }
    best_rate = max(successful.values())
    winners = [labels[condition] for condition, rate in successful.items() if rate == best_rate]
    filesystem = summary["by_condition"].get("filesystem")
    workspace = summary["by_condition"].get("workspace")
    service = summary["by_condition"].get("service_api")
    lines.extend([
        "",
        "## Findings",
        f"- Highest complete-case rate: {', '.join(winners)} at {best_rate:.0%}.",
    ])
    if filesystem and workspace:
        token_ratio = workspace["mean_tokens"]["input_tokens"] / max(1, filesystem["mean_tokens"]["input_tokens"])
        uncached_ratio = (
            workspace["mean_tokens"]["uncached_input_tokens"]
            / max(1, filesystem["mean_tokens"]["uncached_input_tokens"])
        )
        call_ratio = workspace["mean_completed_commands"] / max(1, filesystem["mean_completed_commands"])
        finding_lines = [
            f"- Filesystem and workspace agents both recovered {filesystem['claims_passed']}/{filesystem['claims_total']} scored claims.",
            f"- The workspace recorded {token_ratio:.1f}x the cumulative input but only {uncached_ratio:.2f}x the uncached input of direct filesystem access.",
            f"- The difference tracks interaction shape: the shell workspace required {workspace['mean_completed_commands']:.1f} completed calls per case versus {filesystem['mean_completed_commands']:.1f} for filesystem access ({call_ratio:.1f}x). Repeated cached history, not a doubled evidence load, dominates the headline token ratio.",
            f"- Workspace commands returned {workspace['mean_command_output_chars']:,.0f} recorded characters per case versus {filesystem['mean_command_output_chars']:,.0f} from filesystem commands. These raw event-log outputs are not guaranteed to equal model-visible text, but they confirm that the current shell surface is chattier and returns more text overall.",
        ]
        if has_read_only:
            finding_lines.append(
                f"- The workspace persisted {workspace['persisted_checkpoints']}/{workspace['checkpoint_eligible_runs']} eligible checkpoints; the read-only authorization card intentionally could not write one."
            )
        else:
            finding_lines.append(
                f"- The workspace persisted {workspace['persisted_checkpoints']}/{workspace['checkpoint_eligible_runs']} eligible checkpoints."
            )
        fixed = summary["by_condition"].get("fixed_pack")
        if fixed and fixed["case_pass_rate"] < 1:
            finding_lines.append(
                "- Fixed handoff failures clustered around facts or constraints omitted during pack assembly."
            )
        lines.extend(finding_lines)
    if filesystem and service:
        service_uncached = service["mean_tokens"]["uncached_input_tokens"]
        filesystem_uncached = filesystem["mean_tokens"]["uncached_input_tokens"]
        lines.extend([
            f"- Native service calls averaged {service['mean_service_calls']:.1f} per case and returned "
            f"{service['mean_service_result_chars']:,.0f} characters in {service['mean_service_latency_ms']:,.1f} ms of measured API time.",
            f"- Of that model-visible service output, {service['mean_service_source_text_chars']:,.0f} characters were evidence text and "
            f"{service['mean_service_metadata_chars']:,.0f} were transport metadata; replay-weighted output was "
            f"{service['mean_service_replay_weighted_chars']:,.0f} characters per case.",
            f"- Native uncached model input was {service_uncached / max(1, filesystem_uncached):.2f}x the filesystem baseline.",
        ])

    is_personal = str(manifest.get("benchmark_version", "")).startswith("personal-coordination")
    if is_personal:
        durable = service or workspace or summary["by_condition"].get("filesystem")
        durable_label = "native service" if service else "workspace"
        lines.extend([
            "",
            "## Interpretation boundary",
            "This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.",
            "",
            "## Conclusions",
            "- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.",
            "- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.",
            f"- The {durable_label} scored {durable['mean_score']:.3f} and persisted {durable['persisted_checkpoints']}/{durable['checkpoint_eligible_runs']} eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.",
            (
                "- The native API improved complete-case and claim recall over filesystem access, but still used more uncached input on this compact suite. Compact projections and model tool policy remain optimization targets."
                if service
                else "- The shell prototype is too interactive: it used substantially more calls, cumulative input, uncached input, and returned text than direct filesystem access. The typed native API must preserve quality while collapsing those round trips."
            ),
            (
                "- The native condition exercised OpenAI embeddings, hybrid ranking, authority-aware retrieval, snapshot pinning, and capability-bound writes; the suite is not an isolated semantic hit-rate benchmark."
                if service
                else "- Lexical BM25 was sufficient on this compact synthetic corpus. OpenAI embeddings, hybrid ranking, authority-aware traversal, hit rate, and search latency remain untested target-architecture hypotheses."
            ),
            (
                "- The separate changed-evidence transition suite remains the decisive fresh-agent continuation and efficiency gate."
                if service
                else "- The next implementation gate is the Rust/Postgres service and native typed adapter, followed by untouched holdout tasks and the existing changed-evidence checkpoint-transition suite."
            ),
            "",
            "## Limitations",
            "- The corpus is synthetic and the rubrics were authored with knowledge of it; untouched holdout tasks are still required.",
            "- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.",
            "- The filesystem and workspace agents chose their own evidence paths, so token and latency differences include tool-policy behavior.",
            "- Cumulative input includes cached-history replay. Uncached input is a better comparison of newly processed context, but it is not a direct measure of unique evidence or cost.",
            "- Concept-token grading is deterministic and preserves explicit negation, identifiers, citations, and forbidden conclusions, but it is not a complete semantic judge. The final regrade was manually audited at the claim level.",
            "- The filesystem condition was instruction-restricted rather than OS-sandboxed.",
            (
                "- The native condition is the containerized Rust, Postgres/pgvector, MinIO, and OpenAI implementation; Python and SQLite remain evaluation controls only."
                if service
                else "- The workspace condition is a BM25-backed Python shell prototype, not the planned Rust, Postgres, OpenAI-embedding, and MinIO architecture."
            ),
            (
                "- Read-only denial is executable in this suite; the separate destructive live smoke covers cross-user isolation and every native mutation surface."
                if service
                else "- Read-only denial is executable across the prototype command surface; the production service still needs cross-user and capability tests for every native API operation."
            ),
            "- Live telemetry, external websites, and changing production state were intentionally unavailable.",
            "",
            "## Reproduce",
            "```bash",
            "cd /Users/Shared/projects/straylight",
            "python3 -m unittest discover -s tests -v",
            f"python3 agent_work_eval.py --manifest {manifest_argument} validate",
            f"python3 agent_work_eval.py --manifest {manifest_argument} run{native_flag} --concurrency 3 --timeout 420 --out results/native-personal-coordination.json",
            "```",
            "",
        ])
    else:
        lines.extend([
            "",
            "## Interpretation boundary",
            "This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.",
            "",
            "## Conclusions",
            "- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.",
            "- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.",
            (
                f"- The native API recovered {service['claims_passed']}/{service['claims_total']} claims versus {filesystem['claims_passed']}/{filesystem['claims_total']} for filesystem access and persisted every eligible checkpoint."
                if service and filesystem
                else "- The initial shell interface creates too many model/tool round trips. A native typed API, persistent index and session, batched retrieval, compact deltas, and non-echoing checkpoint writes should remove most of that overhead."
            ),
            (
                "- The native condition exercised exact, structured, lexical, semantic, temporal, and relation retrieval with source diversification and authority-preserving checkpoint writes."
                if service
                else "- The current workspace uses lexical BM25 retrieval. Semantic retrieval, reranking, authority and supersession signals, hit rate, and search latency remain untested product hypotheses."
            ),
            "",
            "## Limitations",
            "- The corpus and rubrics were authored from known project material, so future runs need untouched holdout tasks.",
            "- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.",
            "- Cumulative input includes cached-history replay and is not a measure of unique evidence.",
            "- Live telemetry, external websites, and production state were intentionally unavailable.",
            "",
            "## Reproduce",
            "```bash",
            "cd /Users/Shared/projects/straylight",
            "python3 -m unittest discover -s tests -v",
            f"python3 agent_work_eval.py --manifest {manifest_argument} validate",
            f"python3 agent_work_eval.py --manifest {manifest_argument} run{native_flag} --concurrency 3 --timeout 420 --out results/native-agent-work.json",
            "```",
            "",
        ])
    return "\n".join(lines)


def validate(manifest_path: Path, schema_path: Path) -> dict:
    manifest = load_json(manifest_path)
    for case in manifest["cases"]:
        case.setdefault(
            "grading_mode",
            manifest.get("grading_mode", "exact_substring_v1"),
        )
    schema = load_json(schema_path)
    corpus = (PROJECT_ROOT / manifest["corpus_root"]).resolve()
    documents, chunks = load_corpus(corpus)
    paths = {document.path for document in documents}
    errors = []
    ids = [case["id"] for case in manifest["cases"]]
    if len(ids) != len(set(ids)):
        errors.append("Duplicate case IDs")
    for case in manifest["cases"]:
        if case.get("workspace_access", "read_write") not in {"read_write", "read_only"}:
            errors.append(f"{case['id']}: invalid workspace_access")
        rubric_ids = {rubric["id"] for rubric in case["rubric"]}
        if rubric_ids != set(case["claim_slots"]):
            errors.append(f"{case['id']}: claim slots and rubric IDs differ")
        for rubric in case["rubric"]:
            missing = [path for path in rubric["sources_any"] if path not in paths]
            if missing:
                errors.append(f"{case['id']}:{rubric['id']}: missing sources {missing}")
    return {
        "errors": errors,
        "manifest": manifest,
        "schema": schema,
        "documents": documents,
        "chunks": chunks,
        "corpus_root": corpus,
        "corpus_sha256": corpus_hash(documents),
        "artifact_tree_sha256": sha256_tree(corpus),
    }


def select_conditions(manifest: dict, args: argparse.Namespace) -> list[str]:
    if args.filesystem_native and args.condition:
        raise ValueError("--filesystem-native cannot be combined with --condition")
    if args.filesystem_native:
        requested = ["filesystem", "service_api"]
    elif args.condition:
        requested = list(dict.fromkeys(args.condition))
    else:
        requested = list(manifest["conditions"])
    unknown = [condition for condition in requested if condition not in CONDITION_ADAPTERS]
    if unknown:
        raise ValueError(f"Unknown condition adapters: {unknown}")
    return requested


def select_cases(
    manifest: dict,
    requested: Sequence[str] | None,
    *,
    include_retired: bool,
) -> list[dict[str, Any]]:
    if requested:
        requested_ids = set(requested)
        return [case for case in manifest["cases"] if case["id"] in requested_ids]
    return [
        case
        for case in manifest["cases"]
        if include_retired or case.get("active", True)
    ]


async def run_all(args: argparse.Namespace) -> dict:
    validated = validate(args.manifest, args.schema)
    if validated["errors"]:
        raise ValueError("\n".join(validated["errors"]))
    manifest = validated["manifest"]
    corpus = validated["corpus_root"]
    documents = validated["documents"]
    chunks = validated["chunks"]
    index = BM25Index(chunks)
    run_id = args.resume_run_id or args.run_id or datetime.now().astimezone().strftime("%Y%m%dT%H%M%S%z")
    run_root = PROJECT_ROOT / "runs" / run_id
    if args.resume_run_id:
        if not run_root.is_dir():
            raise ValueError(f"Unknown run directory: {run_root}")
    else:
        run_root.mkdir(parents=True, exist_ok=False)
    corpus_paths = {document.path for document in documents}
    selected_cases = select_cases(
        manifest,
        args.case,
        include_retired=args.include_retired,
    )
    selected_conditions = select_conditions(manifest, args)
    native_metadata: dict[str, dict[str, Any]] = {}
    if "service_api" in selected_conditions:
        native_state_path = run_root / NATIVE_PROVISIONING_STATE
        native_metadata = load_native_provisioning_state(native_state_path, run_id)
        documents_payload = text_documents(corpus)
        for case in selected_cases:
            existing = native_metadata.get(case["id"])
            if existing is not None:
                if not provisioning_matches_run_case(
                    existing,
                    run_id=run_id,
                    case_id=case["id"],
                ):
                    raise ValueError(
                        f"Invalid persisted native provisioning for {case['id']}"
                    )
                continue
            client = NativeApiClient(run_id=run_id, case_id=case["id"])
            native_metadata[case["id"]] = provision_evaluation(
                client,
                run_id=run_id,
                case_id=case["id"],
                display_scope=case.get("scope", case["workload"]),
                access_mode=case.get("workspace_access", "read_write"),
                documents=documents_payload,
                timeout_seconds=args.native_index_timeout,
            )
            write_native_provisioning_state(native_state_path, run_id, native_metadata)
    semaphore = asyncio.Semaphore(args.concurrency)
    tasks = []
    records = []
    for case in selected_cases:
        for condition in selected_conditions:
            run_dir = run_root / condition / case["id"]
            if (run_dir / "answer.json").exists():
                records.append(load_existing_record(
                    run_dir=run_dir,
                    case=case,
                    condition=condition,
                    corpus_paths=corpus_paths,
                ))
                continue
            if run_dir.exists():
                context_path = run_dir / "context.md"
                context_chars = len(context_path.read_text(encoding="utf-8")) if context_path.exists() else 0
            else:
                run_dir, context_chars = prepare_case_dir(
                    run_root,
                    case,
                    condition,
                    corpus=corpus,
                    index=index,
                    manifest=manifest,
                    run_id=run_id,
                    native_metadata=native_metadata.get(case["id"]),
                )
            environment = None
            if condition == "service_api":
                metadata = native_metadata[case["id"]]
                environment = {
                    "STRAYLIGHT_API_URL": os.environ["STRAYLIGHT_API_URL"],
                    "STRAYLIGHT_EVAL_TOKEN": metadata["token"],
                }
            tasks.append(run_one(
                semaphore,
                codex=args.codex,
                model=args.model or manifest["model"],
                schema=args.schema,
                run_dir=run_dir,
                case=case,
                condition=condition,
                context_chars=context_chars,
                corpus_paths=corpus_paths,
                timeout_seconds=args.timeout,
                env_overrides=environment,
            ))
    completed = await asyncio.gather(*tasks, return_exceptions=True)
    for item in completed:
        if isinstance(item, Exception):
            records.append({
                "case_id": "unknown",
                "condition": "unknown",
                "error": f"Unhandled run error: {item}",
                "grade": None,
                "elapsed_seconds": None,
                "fixed_context_chars": 0,
                "workspace_result_chars": 0,
                "events": {"events": 0, "commands": 0, "tokens": {}},
            })
        else:
            records.append(item)
    selected_manifest = dict(manifest)
    selected_manifest["model"] = args.model or manifest["model"]
    selected_manifest["cases"] = selected_cases
    selected_manifest["conditions"] = selected_conditions
    run = {
        "benchmark_version": manifest["benchmark_version"],
        "run_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "run_id": run_id,
        "harness_sha256": sha256_file(Path(__file__)),
        "workspace_cli_sha256": sha256_file(PROJECT_ROOT / "workspace_cli.py"),
        "native_memory_sha256": sha256_file(PROJECT_ROOT / "native_memory.py"),
        "manifest_sha256": sha256_file(args.manifest),
        "schema_sha256": sha256_file(args.schema),
        "corpus": {
            "root": str(corpus),
            "sha256": validated["corpus_sha256"],
            "artifact_tree_sha256": validated["artifact_tree_sha256"],
            "documents": len(documents),
            "chunks": len(chunks),
            "characters": sum(len(document.text) for document in documents),
        },
        "manifest": selected_manifest,
        "native_provisioning": {
            case_id: public_provisioning(metadata)
            for case_id, metadata in native_metadata.items()
        },
        "records": records,
    }
    run["summary"] = summarize(selected_manifest, records)
    return run


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run the Straylight agent-work evaluation")
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("validate")

    run_parser = subparsers.add_parser("run")
    run_parser.add_argument("--codex", type=Path, default=DEFAULT_CODEX)
    run_parser.add_argument("--model")
    run_parser.add_argument("--concurrency", type=int, default=3)
    run_parser.add_argument("--timeout", type=int, default=360)
    run_parser.add_argument("--run-id")
    run_parser.add_argument("--resume-run-id")
    run_parser.add_argument("--case", action="append")
    run_parser.add_argument(
        "--include-retired",
        action="store_true",
        help="include cases retired because their product premise is no longer current",
    )
    run_parser.add_argument("--condition", action="append", choices=tuple(CONDITION_LABELS))
    run_parser.add_argument(
        "--filesystem-native",
        action="store_true",
        help="run only the unchanged filesystem baseline and native service_api condition",
    )
    run_parser.add_argument("--native-index-timeout", type=float, default=300.0)
    run_parser.add_argument("--out", type=Path, required=True)
    run_parser.add_argument("--report", type=Path)

    regrade_parser = subparsers.add_parser("regrade")
    regrade_parser.add_argument("--input", type=Path, required=True)
    regrade_parser.add_argument("--out", type=Path, required=True)
    regrade_parser.add_argument("--report", type=Path)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    validated = validate(args.manifest, args.schema)
    if args.command == "validate":
        print(json.dumps({
            "status": "ok" if not validated["errors"] else "error",
            "errors": validated["errors"],
            "cases": len(validated["manifest"]["cases"]),
            "claims": sum(len(case["rubric"]) for case in validated["manifest"]["cases"]),
            "documents": len(validated["documents"]),
            "chunks": len(validated["chunks"]),
            "corpus_sha256": validated["corpus_sha256"],
            "artifact_tree_sha256": validated["artifact_tree_sha256"],
            "manifest_sha256": sha256_file(args.manifest),
        }, indent=2))
        if validated["errors"]:
            raise SystemExit(1)
        return

    if args.command == "regrade":
        if validated["errors"]:
            raise ValueError("\n".join(validated["errors"]))
        run = load_json(args.input)
        case_by_id = {case["id"]: case for case in validated["manifest"]["cases"]}
        corpus_paths = {document.path for document in validated["documents"]}
        selected_cases = [case_by_id[case["id"]] for case in run["manifest"]["cases"]]
        run["manifest"]["cases"] = selected_cases
        for record in run["records"]:
            if record.get("answer") and record["case_id"] in case_by_id:
                record["grade"] = grade_answer(case_by_id[record["case_id"]], record["answer"], corpus_paths)
            run_dir = Path(record["answer_path"]).parent
            record["events"] = parse_event_metrics(run_dir / "events.jsonl")
            attach_workspace_metrics(record, run_dir)
        run["summary"] = summarize(run["manifest"], run["records"])
        run["regraded_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
        run["manifest_sha256"] = sha256_file(args.manifest)
        run["harness_sha256"] = sha256_file(Path(__file__))
        run["workspace_cli_sha256"] = sha256_file(PROJECT_ROOT / "workspace_cli.py")
        run["native_memory_sha256"] = sha256_file(PROJECT_ROOT / "native_memory.py")
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(render_report(run), encoding="utf-8")
        print(json.dumps({
            "status": "ok",
            "out": str(args.out),
            "report": str(args.report) if args.report else None,
            "summary": run["summary"]["by_condition"],
        }, indent=2))
        return

    run = asyncio.run(run_all(args))
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
        "summary": run["summary"]["by_condition"],
    }, indent=2))


if __name__ == "__main__":
    main()

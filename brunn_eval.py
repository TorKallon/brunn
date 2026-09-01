#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
from collections import Counter, defaultdict
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Iterable, Sequence


TOKEN_RE = re.compile(r"[a-z0-9]+(?:[._/+:-][a-z0-9]+)*", re.IGNORECASE)
HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")
STOPWORDS = {
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "can",
    "did", "do", "does", "for", "from", "had", "has", "have", "how",
    "i", "if", "in", "into", "is", "it", "its", "may", "must", "not",
    "of", "on", "or", "our", "should", "so", "than", "that", "the",
    "their", "then", "there", "these", "they", "this", "to", "was", "we",
    "were", "what", "when", "where", "which", "while", "who", "why", "with",
    "would", "yet", "you", "your",
}
POLICY_VERSION = "0.2"


def normalize_text(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip().casefold()


def tokenize(value: str) -> list[str]:
    tokens: list[str] = []
    for raw in TOKEN_RE.findall(value.casefold()):
        if raw not in STOPWORDS:
            tokens.append(raw)
        for part in re.split(r"[._/+:-]+", raw):
            if part and part != raw and part not in STOPWORDS:
                tokens.append(part)
    return tokens


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


@dataclass(frozen=True)
class Document:
    project: str
    path: str
    text: str
    headings: tuple[str, ...]


@dataclass(frozen=True)
class Chunk:
    id: str
    section_id: str
    project: str
    path: str
    heading: str
    start_line: int
    end_line: int
    text: str
    tokens: tuple[str, ...]


@dataclass(frozen=True)
class PackItem:
    id: str
    project: str
    path: str
    heading: str
    start_line: int
    end_line: int
    text: str
    reason: str
    score: float


class BM25Index:
    def __init__(self, chunks: Sequence[Chunk]):
        self.chunks = list(chunks)
        self.term_counts = [Counter(chunk.tokens) for chunk in self.chunks]
        self.lengths = [len(chunk.tokens) for chunk in self.chunks]
        self.avg_length = statistics.fmean(self.lengths) if self.lengths else 1.0
        self.df: Counter[str] = Counter()
        for counts in self.term_counts:
            self.df.update(counts.keys())
        self.idf = {
            term: math.log(1.0 + (len(self.chunks) - freq + 0.5) / (freq + 0.5))
            for term, freq in self.df.items()
        }

    def score_tokens(self, query_tokens: Sequence[str], index: int) -> float:
        if not query_tokens:
            return 0.0
        counts = self.term_counts[index]
        length = self.lengths[index]
        score = 0.0
        k1 = 1.5
        b = 0.75
        for term in query_tokens:
            frequency = counts.get(term, 0)
            if not frequency:
                continue
            denominator = frequency + k1 * (1 - b + b * length / self.avg_length)
            score += self.idf.get(term, 0.0) * frequency * (k1 + 1) / denominator

        chunk = self.chunks[index]
        metadata = set(tokenize(f"{chunk.path} {chunk.heading}"))
        score += 0.45 * sum(self.idf.get(term, 0.0) for term in set(query_tokens) & metadata)
        return score

    def search(
        self,
        query: str,
        *,
        project: str | None = None,
        paths: set[str] | None = None,
        limit: int | None = None,
    ) -> list[tuple[float, Chunk]]:
        query_tokens = tokenize(query)
        ranked: list[tuple[float, Chunk]] = []
        for index, chunk in enumerate(self.chunks):
            if project and chunk.project != project:
                continue
            if paths is not None and chunk.path not in paths:
                continue
            score = self.score_tokens(query_tokens, index)
            if score > 0:
                ranked.append((score, chunk))
        ranked.sort(key=lambda item: (-item[0], item[1].path, item[1].start_line))
        return ranked[:limit] if limit is not None else ranked


def split_sections(path: str, text: str) -> list[tuple[str, int, list[str]]]:
    lines = text.splitlines()
    sections: list[tuple[str, int, list[str]]] = []
    heading = "(preamble)"
    start_line = 1
    current: list[str] = []
    for line_number, line in enumerate(lines, 1):
        match = HEADING_RE.match(line)
        if match and current:
            sections.append((heading, start_line, current))
            heading = match.group(2).strip()
            start_line = line_number
            current = [line]
        elif match:
            heading = match.group(2).strip()
            start_line = line_number
            current = [line]
        else:
            current.append(line)
    if current:
        sections.append((heading, start_line, current))
    return sections


def split_section_lines(
    lines: Sequence[str],
    start_line: int,
    target_chars: int,
    overlap_chars: int,
) -> list[tuple[int, int, str]]:
    chunks: list[tuple[int, int, str]] = []
    cursor = 0
    while cursor < len(lines):
        end = cursor
        size = 0
        while end < len(lines):
            next_size = len(lines[end]) + 1
            if end > cursor and size + next_size > target_chars:
                break
            size += next_size
            end += 1
        if end == cursor:
            end += 1
        text = "\n".join(lines[cursor:end]).strip()
        chunks.append((start_line + cursor, start_line + end - 1, text))
        if end >= len(lines):
            break
        overlap = 0
        next_cursor = end
        while next_cursor > cursor and overlap < overlap_chars:
            next_cursor -= 1
            overlap += len(lines[next_cursor]) + 1
        cursor = max(cursor + 1, next_cursor)
    return chunks


def build_corpus(
    vault: Path,
    manifest: dict,
    excluded_paths: Iterable[Path] = (),
) -> tuple[list[Document], list[Chunk]]:
    documents: list[Document] = []
    chunks: list[Chunk] = []
    budget = manifest["budgets"]
    excluded = {path.resolve() for path in excluded_paths}
    for root in manifest["corpus_roots"]:
        base = vault / root["path"]
        for path in sorted(base.glob(root.get("glob", "*.md"))):
            if not path.is_file() or path.resolve() in excluded:
                continue
            relative = path.relative_to(vault).as_posix()
            text = path.read_text(encoding="utf-8", errors="replace")
            sections = split_sections(relative, text)
            headings = tuple(section[0] for section in sections if section[0] != "(preamble)")
            documents.append(Document(root["project"], relative, text, headings))
            for heading, section_start, section_lines in sections:
                section_id = f"{relative}#{section_start}:{heading}"
                for start, end, chunk_text in split_section_lines(
                    section_lines,
                    section_start,
                    budget["chunk_chars"],
                    budget["chunk_overlap_chars"],
                ):
                    chunk_id = f"{relative}:{start}-{end}"
                    chunks.append(
                        Chunk(
                            chunk_id,
                            section_id,
                            root["project"],
                            relative,
                            heading,
                            start,
                            end,
                            chunk_text,
                            tuple(tokenize(chunk_text)),
                        )
                    )
    return documents, chunks


def corpus_hash(documents: Sequence[Document]) -> str:
    digest = hashlib.sha256()
    for document in sorted(documents, key=lambda item: item.path):
        digest.update(document.path.encode())
        digest.update(b"\0")
        digest.update(document.text.encode())
        digest.update(b"\0")
    return digest.hexdigest()


def compact_map(documents: Sequence[Document], selected_project: str | None = None) -> str:
    by_project: dict[str, list[Document]] = defaultdict(list)
    for document in documents:
        by_project[document.project].append(document)
    lines = ["CORPUS MAP"]
    for project in sorted(by_project):
        project_documents = by_project[project]
        lines.append(
            f"- {project}: {len(project_documents)} documents, "
            f"{sum(len(document.text) for document in project_documents)} characters"
        )
    if selected_project and selected_project in by_project:
        lines.append(f"\nFILES IN {selected_project}:")
        for document in sorted(by_project[selected_project], key=lambda item: item.path):
            lines.append(f"- {document.path} ({len(document.text)} chars)")
    return "\n".join(lines)


def query_clauses(question: str) -> list[str]:
    clauses = [question.strip()]
    for part in re.split(r"\s+(?:and|while|including)\s+|[,;?]", question, flags=re.IGNORECASE):
        part = part.strip(" .")
        if len(set(tokenize(part))) >= 2 and part.casefold() != question.strip().casefold():
            clauses.append(part)
    return list(dict.fromkeys(clauses))


def chunk_item(chunk: Chunk, reason: str, score: float) -> PackItem:
    return PackItem(
        chunk.id,
        chunk.project,
        chunk.path,
        chunk.heading,
        chunk.start_line,
        chunk.end_line,
        chunk.text,
        reason,
        score,
    )


def document_item(document: Document, reason: str, score: float) -> PackItem:
    return PackItem(
        f"{document.path}:full",
        document.project,
        document.path,
        "(full file)",
        1,
        len(document.text.splitlines()),
        document.text,
        reason,
        score,
    )


def select_under_budget(
    candidates: Iterable[PackItem],
    budget_chars: int,
    *,
    initial: Sequence[PackItem] = (),
) -> list[PackItem]:
    selected = list(initial)
    seen = {item.id for item in selected}
    used = sum(len(item.text) for item in selected)
    for item in candidates:
        if item.id in seen:
            continue
        size = len(item.text)
        if selected and used + size > budget_chars:
            continue
        selected.append(item)
        seen.add(item.id)
        used += size
    return selected


def direct_pack(
    question: str,
    documents: Sequence[Document],
    index: BM25Index,
    budget: dict,
) -> tuple[list[PackItem], dict]:
    scores_by_path: dict[str, list[float]] = defaultdict(list)
    clauses = query_clauses(question)
    for clause_index, clause in enumerate(clauses):
        weight = 1.0 if clause_index == 0 else 0.8
        for score, chunk in index.search(clause):
            scores_by_path[chunk.path].append(score * weight)
    file_scores = []
    for path, scores in scores_by_path.items():
        ordered = sorted(scores, reverse=True)
        aggregate = ordered[0] + sum(weight * value for weight, value in zip((0.4, 0.2), ordered[1:3]))
        file_scores.append((aggregate, path))
    file_scores.sort(key=lambda item: (-item[0], item[1]))
    docs = {document.path: document for document in documents}
    candidates = [
        document_item(docs[path], "filesystem-search then full-file read", score)
        for score, path in file_scores[: budget["direct_files"]]
    ]
    selected = select_under_budget(candidates, budget["direct_chars"])
    return selected, {
        "mode": "filesystem",
        "query_clauses": clauses,
        "ranked_files": [path for _, path in file_scores[:10]],
    }


def one_shot_pack(question: str, index: BM25Index, budget: dict) -> tuple[list[PackItem], dict]:
    ranked = index.search(question, limit=budget["one_shot_chunks"] * 3)
    candidates = [chunk_item(chunk, "one-shot BM25 top-k", score) for score, chunk in ranked]
    selected = select_under_budget(candidates, budget["one_shot_chars"])
    selected = selected[: budget["one_shot_chunks"]]
    return selected, {"mode": "one_shot", "ranked_chunks": [chunk.id for _, chunk in ranked[:12]]}


def resolve_project(question: str, index: BM25Index) -> tuple[str | None, float, dict[str, float]]:
    raw_scores: dict[str, list[float]] = defaultdict(list)
    for score, chunk in index.search(question, limit=40):
        raw_scores[chunk.project].append(score)
    project_scores: dict[str, float] = {}
    query_terms = set(tokenize(question))
    for project, scores in raw_scores.items():
        ordered = sorted(scores, reverse=True)
        aggregate = sum(weight * value for weight, value in zip((1.0, 0.35, 0.15), ordered[:3]))
        project_terms = set(tokenize(project))
        aggregate += 2.0 * len(query_terms & project_terms)
        project_scores[project] = aggregate
    total = sum(project_scores.values())
    if not project_scores or total <= 0:
        return None, 0.0, {}
    project = max(project_scores, key=project_scores.get)
    return project, project_scores[project] / total, dict(project_scores)


def feedback_terms(question: str, chunks: Sequence[Chunk], index: BM25Index, limit: int = 8) -> list[str]:
    query_terms = set(tokenize(question))
    candidates: Counter[str] = Counter()
    for chunk in chunks[:3]:
        counts = Counter(chunk.tokens)
        for term, frequency in counts.items():
            if term in query_terms or term in STOPWORDS or len(term) < 3:
                continue
            candidates[term] += frequency * index.idf.get(term, 0.0)
    return [term for term, _ in candidates.most_common(limit)]


def resolve_wikilink(target: str, documents: Sequence[Document]) -> set[str]:
    target = target.split("|", 1)[0].split("#", 1)[0].strip()
    if not target:
        return set()
    target_no_ext = target[:-3] if target.endswith(".md") else target
    matches = set()
    for document in documents:
        path_no_ext = document.path[:-3] if document.path.endswith(".md") else document.path
        if path_no_ext == target_no_ext or Path(path_no_ext).name == Path(target_no_ext).name:
            matches.add(document.path)
    return matches


def workspace_pack(
    question: str,
    documents: Sequence[Document],
    chunks: Sequence[Chunk],
    index: BM25Index,
    budget: dict,
) -> tuple[list[PackItem], dict]:
    project, confidence, project_scores = resolve_project(question, index)
    project_documents = [document for document in documents if document.project == project]
    project_chars = sum(len(document.text) for document in project_documents)
    map_item = PackItem(
        "corpus-map",
        "(system)",
        "(corpus map)",
        "(map)",
        0,
        0,
        compact_map(documents, project),
        "memory.open corpus map",
        0.0,
    )

    if project and confidence >= 0.45 and project_chars <= budget["workspace_materialize_project_chars"]:
        materialized = [
            document_item(document, "memory.read materialize_project", 0.0)
            for document in sorted(project_documents, key=lambda item: item.path)
        ]
        return [map_item, *materialized], {
            "mode": "materialize_project",
            "resolved_project": project,
            "project_confidence": confidence,
            "project_scores": project_scores,
            "feedback_terms": [],
        }

    scope = project if project and confidence >= 0.35 else None
    initial_ranked = index.search(question, project=scope, limit=max(12, budget["workspace_initial_chunks"] * 4))
    initial_ranked = initial_ranked[: budget["workspace_initial_chunks"]]
    initial_chunks = [chunk for _, chunk in initial_ranked]

    by_section: dict[str, list[Chunk]] = defaultdict(list)
    by_path: dict[str, list[Chunk]] = defaultdict(list)
    chunk_position: dict[str, int] = {}
    for chunk in chunks:
        by_section[chunk.section_id].append(chunk)
        by_path[chunk.path].append(chunk)
    for path_chunks in by_path.values():
        path_chunks.sort(key=lambda item: (item.start_line, item.end_line))
        for position, chunk in enumerate(path_chunks):
            chunk_position[chunk.id] = position

    clauses = query_clauses(question)
    clause_ranked: list[tuple[float, Chunk]] = []
    for clause in clauses[1:]:
        clause_ranked.extend(index.search(clause, project=scope, limit=2))

    feedback_source = initial_chunks + [chunk for _, chunk in clause_ranked]
    expansion_terms = feedback_terms(question, feedback_source, index)
    expanded_query = question + " " + " ".join(expansion_terms)
    expanded_ranked = index.search(
        expanded_query,
        project=scope,
        limit=budget["workspace_expansion_chunks"] * 2,
    )
    primary: list[tuple[float, Chunk, str]] = []
    primary.extend((score, chunk, "memory.query initial") for score, chunk in initial_ranked)
    primary.extend((score, chunk, "memory.query clause follow-up") for score, chunk in clause_ranked)
    primary.extend(
        (score, chunk, "memory.query feedback expansion")
        for score, chunk in expanded_ranked[: budget["workspace_expansion_chunks"]]
    )
    deduped_primary: list[tuple[float, Chunk, str]] = []
    seen_primary: set[str] = set()
    for score, chunk, reason in primary:
        if chunk.id in seen_primary:
            continue
        seen_primary.add(chunk.id)
        deduped_primary.append((score, chunk, reason))

    candidates: list[PackItem] = [
        chunk_item(chunk, reason, score) for score, chunk, reason in deduped_primary
    ]
    for score, chunk, _ in deduped_primary:
        for sibling in by_section[chunk.section_id]:
            candidates.append(chunk_item(sibling, "memory.read full section", score * 0.9))
    for score, chunk, _ in deduped_primary:
        path_chunks = by_path[chunk.path]
        position = chunk_position[chunk.id]
        for neighbor_position in (position - 1, position + 1):
            if 0 <= neighbor_position < len(path_chunks):
                candidates.append(chunk_item(path_chunks[neighbor_position], "memory.read neighbor", score * 0.8))

    linked_paths: set[str] = set()
    for chunk in initial_chunks:
        for raw_target in WIKILINK_RE.findall(chunk.text):
            linked_paths.update(resolve_wikilink(raw_target, documents))
    for linked_path in sorted(linked_paths):
        linked_ranked = index.search(question, paths={linked_path}, limit=1)
        if linked_ranked:
            score, chunk = linked_ranked[0]
            candidates.append(chunk_item(chunk, "memory.read linked artifact", score))

    selected = select_under_budget(candidates, budget["workspace_chars"], initial=[map_item])
    return selected, {
        "mode": "progressive_workspace",
        "resolved_project": project,
        "project_confidence": confidence,
        "project_scores": project_scores,
        "feedback_terms": expansion_terms,
        "query_clauses": clauses,
        "initial_chunks": [chunk.id for chunk in initial_chunks],
    }


def score_pack(case: dict, items: Sequence[PackItem]) -> dict:
    texts_by_path: dict[str, list[str]] = defaultdict(list)
    for item in items:
        texts_by_path[item.path].append(item.text)
    evidence = []
    for gold in case["gold"]:
        needle = normalize_text(gold["contains"])
        matched_item = None
        for item in items:
            if item.path == gold["path"] and needle in normalize_text(item.text):
                matched_item = item.id
                break
        evidence.append({**gold, "found": matched_item is not None, "matched_item": matched_item})
    found = sum(1 for item in evidence if item["found"])
    chars = sum(len(item.text) for item in items)
    return {
        "case_pass": found == len(evidence),
        "evidence_found": found,
        "evidence_total": len(evidence),
        "evidence_recall": found / len(evidence) if evidence else 1.0,
        "chars": chars,
        "estimated_tokens": math.ceil(chars / 4),
        "files": len({item.path for item in items if not item.path.startswith("(")}),
        "items": len(items),
        "evidence": evidence,
        "pack": [
            {
                "id": item.id,
                "path": item.path,
                "heading": item.heading,
                "start_line": item.start_line,
                "end_line": item.end_line,
                "reason": item.reason,
                "score": round(item.score, 6),
            }
            for item in items
        ],
    }


def percentile(values: Sequence[int], fraction: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, math.ceil(len(ordered) * fraction) - 1))
    return ordered[index]


def summarize(cases: Sequence[dict], results: dict[str, dict[str, dict]]) -> dict:
    summary: dict[str, dict] = {}
    for method, method_results in results.items():
        rows = list(method_results.values())
        chars = [row["chars"] for row in rows]
        evidence_found = sum(row["evidence_found"] for row in rows)
        evidence_total = sum(row["evidence_total"] for row in rows)
        passed = sum(1 for row in rows if row["case_pass"])
        summary[method] = {
            "cases_passed": passed,
            "cases_total": len(rows),
            "case_pass_rate": passed / len(rows) if rows else 0.0,
            "evidence_found": evidence_found,
            "evidence_total": evidence_total,
            "evidence_recall": evidence_found / evidence_total if evidence_total else 0.0,
            "median_chars": int(statistics.median(chars)) if chars else 0,
            "p95_chars": percentile(chars, 0.95),
            "mean_chars": round(statistics.fmean(chars), 1) if chars else 0,
            "mean_estimated_tokens": round(statistics.fmean(row["estimated_tokens"] for row in rows), 1) if rows else 0,
        }

    case_by_id = {case["id"]: case for case in cases}
    categories = sorted({case["category"] for case in cases})
    by_category = {}
    for category in categories:
        ids = [case_id for case_id, case in case_by_id.items() if case["category"] == category]
        by_category[category] = {
            method: {
                "passed": sum(1 for case_id in ids if results[method][case_id]["case_pass"]),
                "total": len(ids),
                "evidence_recall": round(
                    sum(results[method][case_id]["evidence_found"] for case_id in ids)
                    / max(1, sum(results[method][case_id]["evidence_total"] for case_id in ids)),
                    4,
                ),
            }
            for method in results
        }
    return {"methods": summary, "by_category": by_category}


def render_markdown(run: dict, baseline: dict | None = None) -> str:
    summary = run["summary"]["methods"]
    lines = [
        "Created: " + run["run_at"],
        "Updated: " + run["run_at"],
        "Status: Complete",
        "",
        "Related: [[Brunn]], [[Retrieval API - Initial Design]], [[Projects/Brunn/Retrieval evaluation plan|Retrieval evaluation plan]]",
        "",
        "# Brunn retrieval evaluation results - 2026-07-10",
        "",
        "## Scope",
        f"- Corpus: {run['corpus']['documents']} Markdown documents, {run['corpus']['characters']:,} characters, {run['corpus']['chunks']} chunks",
        f"- Frozen corpus root: `{run['corpus']['root']}`",
        f"- Cases: {run['case_count']} frozen questions with {run['gold_item_count']} gold evidence items",
        f"- Manifest SHA-256: `{run['manifest_sha256']}`",
        f"- Corpus SHA-256: `{run['corpus']['sha256']}`",
        f"- Retrieval policy: `{run['retrieval_policy_version']}`",
        f"- Harness SHA-256: `{run['harness_sha256']}`",
        "- Corpus areas: Metis, N24 RaceWatch, Home Network Improvements, and Brunn",
        "- Private, health, finance, family, and work-record folders were excluded.",
        "",
        "## What this run measures",
        "This is a deterministic retrieval-readiness benchmark. A case passes when every frozen gold evidence item needed to answer the question is present in the returned material. It measures answerability and evidence coverage, not prose quality from a separately sampled language model.",
        "",
        "## Results",
        "| Method | Cases passed | Evidence recall | Median chars | P95 chars | Mean estimated tokens |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    labels = {"direct": "Direct filesystem", "one_shot": "One-shot top-k", "workspace": "Memory Workspace"}
    for method in ("direct", "one_shot", "workspace"):
        row = summary[method]
        lines.append(
            f"| {labels[method]} | {row['cases_passed']}/{row['cases_total']} ({row['case_pass_rate']:.0%}) "
            f"| {row['evidence_found']}/{row['evidence_total']} ({row['evidence_recall']:.0%}) "
            f"| {row['median_chars']:,} | {row['p95_chars']:,} | {row['mean_estimated_tokens']:,.0f} |"
        )

    if baseline:
        baseline_workspace = baseline["summary"]["methods"]["workspace"]
        current_workspace = summary["workspace"]
        hashes_match = (
            baseline.get("manifest_sha256") == run.get("manifest_sha256")
            and baseline.get("corpus", {}).get("sha256") == run.get("corpus", {}).get("sha256")
        )
        lines.extend([
            "",
            "## Tuning pass",
            "| Workspace run | Cases passed | Evidence recall | Median chars | P95 chars | Mean estimated tokens |",
            "| --- | ---: | ---: | ---: | ---: | ---: |",
            f"| Initial policy | {baseline_workspace['cases_passed']}/{baseline_workspace['cases_total']} "
            f"({baseline_workspace['case_pass_rate']:.0%}) | {baseline_workspace['evidence_found']}/{baseline_workspace['evidence_total']} "
            f"({baseline_workspace['evidence_recall']:.0%}) | {baseline_workspace['median_chars']:,} "
            f"| {baseline_workspace['p95_chars']:,} | {baseline_workspace['mean_estimated_tokens']:,.0f} |",
            f"| Policy {run['retrieval_policy_version']} | {current_workspace['cases_passed']}/{current_workspace['cases_total']} "
            f"({current_workspace['case_pass_rate']:.0%}) | {current_workspace['evidence_found']}/{current_workspace['evidence_total']} "
            f"({current_workspace['evidence_recall']:.0%}) | {current_workspace['median_chars']:,} "
            f"| {current_workspace['p95_chars']:,} | {current_workspace['mean_estimated_tokens']:,.0f} |",
            f"- Frozen-input check: {'PASS' if hashes_match else 'FAIL'}; manifest and corpus hashes "
            f"{'match' if hashes_match else 'do not match'} the initial run.",
            "- Policy changes: compact project maps, max-weight project routing, clause follow-up queries, and breadth-first admission before expansion.",
            "- This tuning used the initial failure set. The improved score is a regression result, not holdout evidence.",
        ])

    one_pass = {case_id for case_id, result in run["results"]["one_shot"].items() if result["case_pass"]}
    workspace_pass = {case_id for case_id, result in run["results"]["workspace"].items() if result["case_pass"]}
    gained = sorted(workspace_pass - one_pass)
    lost = sorted(one_pass - workspace_pass)
    lines.extend([
        "",
        "## Workspace delta",
        f"- Cases recovered beyond one-shot top-k: {', '.join(f'`{item}`' for item in gained) if gained else 'none'}",
        f"- Cases lost relative to one-shot top-k: {', '.join(f'`{item}`' for item in lost) if lost else 'none'}",
        "",
        "## Per-case results",
        "| Case | Category | Direct | One-shot | Workspace | One-shot chars | Workspace chars |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ])
    case_by_id = {case["id"]: case for case in run["cases"]}
    for case_id in sorted(case_by_id):
        case = case_by_id[case_id]
        row = []
        for method in ("direct", "one_shot", "workspace"):
            result = run["results"][method][case_id]
            row.append("PASS" if result["case_pass"] else f"{result['evidence_found']}/{result['evidence_total']}")
        lines.append(
            f"| `{case_id}` | {case['category']} | {row[0]} | {row[1]} | {row[2]} "
            f"| {run['results']['one_shot'][case_id]['chars']:,} | {run['results']['workspace'][case_id]['chars']:,} |"
        )

    failures = {
        method: [case_id for case_id, result in run["results"][method].items() if not result["case_pass"]]
        for method in ("direct", "one_shot", "workspace")
    }
    lines.extend([
        "",
        "## Failures to inspect",
        f"- Direct filesystem: {', '.join(f'`{item}`' for item in failures['direct']) if failures['direct'] else 'none'}",
        f"- One-shot top-k: {', '.join(f'`{item}`' for item in failures['one_shot']) if failures['one_shot'] else 'none'}",
        f"- Memory Workspace: {', '.join(f'`{item}`' for item in failures['workspace']) if failures['workspace'] else 'none'}",
        "",
        "## Interpretation",
    ])
    workspace = summary["workspace"]
    one_shot = summary["one_shot"]
    direct = summary["direct"]
    if (
        workspace["case_pass_rate"] > one_shot["case_pass_rate"]
        and workspace["evidence_recall"] > one_shot["evidence_recall"]
    ):
        lines.append(
            f"The workspace recovered {workspace['evidence_recall'] - one_shot['evidence_recall']:.0%} more gold evidence than one-shot retrieval and passed more complete cases. "
            "That supports the core claim that iterative section, neighbor, link, and project materialization can recover evidence hidden beyond an initial top-k boundary."
        )
    else:
        context_ratio = workspace["mean_estimated_tokens"] / max(1, one_shot["mean_estimated_tokens"])
        lines.append(
            f"The workspace tied one-shot retrieval at {workspace['cases_passed']}/{workspace['cases_total']} complete cases, "
            f"but recovered {one_shot['evidence_found'] - workspace['evidence_found']} fewer gold evidence items "
            f"while using {context_ratio:.1f}x the mean estimated context. One-shot top-k is the strongest default on this benchmark; "
            "the Memory Workspace hypothesis is not yet demonstrated."
        )
    if workspace["evidence_recall"] < direct["evidence_recall"]:
        lines.append("The workspace still trails direct filesystem evidence coverage, so the retrieval contract is not yet a replacement for direct source access.")
    else:
        lines.append("The workspace matched or exceeded direct filesystem evidence coverage under the tested deterministic policy.")
    lines.extend([
        "",
        "## Limitations",
        "- The same author selected the corpus and gold evidence, so this is an engineering regression test, not an independent scientific evaluation.",
        "- Policy 0.2 was tuned against the initial run's failures; the final score is not a holdout result.",
        "- Gold matching uses normalized exact evidence strings. It does not grade semantic paraphrases or generated-answer quality.",
        "- The workspace policy is deterministic pseudo-relevance feedback, not a model choosing its own follow-up queries.",
        "- The corpus is project documentation, not raw chat, email, images, or volatile live state.",
        "- Full project materialization is allowed below the frozen 45,000-character threshold and its cost is reported rather than hidden.",
        "",
        "## Next decision",
        "Freeze policy 0.2 and run a blinded model-answer evaluation with model-directed workspace calls plus an untouched holdout set. Do not choose the canonical storage engine from this retrieval-only run.",
        "",
        "## Reproduce",
        "```bash",
        "cd /Users/Shared/projects/brunn",
        "python3 -m unittest discover -s tests -v",
        "python3 brunn_eval.py validate --vault eval/corpus-v0.1",
        "python3 brunn_eval.py run --vault eval/corpus-v0.1 --baseline results/2026-07-10-v0.1.json --out results/2026-07-10-v0.2.json --report '/Users/aether/obsidian/notes/Projects/Brunn/Retrieval evaluation results - 2026-07-10.md'",
        "```",
        "",
    ])
    return "\n".join(lines)


def load_manifest(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def validate_manifest(vault: Path, manifest: dict) -> list[str]:
    errors = []
    for case in manifest["cases"]:
        for gold in case["gold"]:
            path = vault / gold["path"]
            if not path.exists():
                errors.append(f"{case['id']}: missing {gold['path']}")
                continue
            if normalize_text(gold["contains"]) not in normalize_text(path.read_text(encoding="utf-8", errors="replace")):
                errors.append(f"{case['id']}: evidence not found in {gold['path']}: {gold['contains']}")
    return errors


def run_benchmark(
    vault: Path,
    manifest_path: Path,
    excluded_paths: Iterable[Path] = (),
) -> dict:
    manifest = load_manifest(manifest_path)
    errors = validate_manifest(vault, manifest)
    if errors:
        raise ValueError("Invalid benchmark manifest:\n" + "\n".join(errors))
    documents, chunks = build_corpus(vault, manifest, excluded_paths)
    index = BM25Index(chunks)
    results: dict[str, dict[str, dict]] = {"direct": {}, "one_shot": {}, "workspace": {}}
    for case in manifest["cases"]:
        methods = {
            "direct": direct_pack(case["question"], documents, index, manifest["budgets"]),
            "one_shot": one_shot_pack(case["question"], index, manifest["budgets"]),
            "workspace": workspace_pack(case["question"], documents, chunks, index, manifest["budgets"]),
        }
        for method, (items, trace) in methods.items():
            results[method][case["id"]] = {**score_pack(case, items), "trace": trace}
    run_at = datetime.now().astimezone().isoformat(timespec="seconds")
    run = {
        "benchmark_version": manifest["benchmark_version"],
        "retrieval_policy_version": POLICY_VERSION,
        "run_at": run_at,
        "harness_sha256": sha256_bytes(Path(__file__).read_bytes()),
        "manifest_sha256": sha256_bytes(manifest_path.read_bytes()),
        "corpus": {
            "root": str(vault.resolve()),
            "sha256": corpus_hash(documents),
            "documents": len(documents),
            "chunks": len(chunks),
            "characters": sum(len(document.text) for document in documents),
            "projects": sorted({document.project for document in documents}),
        },
        "case_count": len(manifest["cases"]),
        "gold_item_count": sum(len(case["gold"]) for case in manifest["cases"]),
        "budgets": manifest["budgets"],
        "cases": manifest["cases"],
        "results": results,
    }
    run["summary"] = summarize(manifest["cases"], results)
    return run


def main() -> None:
    parser = argparse.ArgumentParser(description="Run the Brunn retrieval benchmark.")
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path(__file__).resolve().parent / "eval" / "cases.json",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate_parser = subparsers.add_parser("validate", help="Validate all frozen gold evidence against the vault.")
    validate_parser.add_argument("--vault", type=Path, required=True)

    run_parser = subparsers.add_parser("run", help="Run all retrieval methods and write results.")
    run_parser.add_argument("--vault", type=Path, required=True)
    run_parser.add_argument("--out", type=Path, required=True)
    run_parser.add_argument("--report", type=Path)
    run_parser.add_argument("--baseline", type=Path)

    args = parser.parse_args()
    manifest = load_manifest(args.manifest)
    if args.command == "validate":
        errors = validate_manifest(args.vault, manifest)
        if errors:
            for error in errors:
                print(error)
            raise SystemExit(1)
        print(json.dumps({
            "status": "ok",
            "cases": len(manifest["cases"]),
            "gold_items": sum(len(case["gold"]) for case in manifest["cases"]),
            "manifest_sha256": sha256_bytes(args.manifest.read_bytes()),
        }, indent=2))
        return

    excluded_paths = [args.report] if args.report else []
    run = run_benchmark(args.vault, args.manifest, excluded_paths)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
    if args.report:
        baseline = json.loads(args.baseline.read_text(encoding="utf-8")) if args.baseline else None
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(render_markdown(run, baseline), encoding="utf-8")
    print(json.dumps({
        "status": "ok",
        "out": str(args.out),
        "report": str(args.report) if args.report else None,
        "summary": run["summary"]["methods"],
    }, indent=2))


if __name__ == "__main__":
    main()

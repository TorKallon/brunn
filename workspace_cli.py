#!/usr/bin/env python3

from __future__ import annotations

import argparse
import ast
import fcntl
import hashlib
import json
import operator
import os
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Sequence

from brunn_eval import (
    BM25Index,
    Chunk,
    Document,
    split_section_lines,
    split_sections,
    tokenize,
)


CHUNK_CHARS = 1600
CHUNK_OVERLAP_CHARS = 220
DEFAULT_QUERY_CHARS = 7000
TEXT_SUFFIXES = {
    ".md", ".txt", ".csv", ".tsv", ".sql", ".json", ".jsonl",
    ".py", ".rs", ".go", ".swift", ".ts", ".tsx", ".js", ".jsx",
    ".sh", ".toml", ".yaml", ".yml",
}
READ_ONLY_COMMANDS = {"open", "query", "read", "compute", "verify"}
MUTATION_COMMANDS = {"checkpoint", "save", "stage", "correct", "delete", "dream"}


def infer_project(relative: str) -> str:
    parts = Path(relative).parts
    if len(parts) >= 2 and parts[0] in {"Projects", "Topics", "Trips"}:
        return parts[1]
    return parts[0] if parts else "Corpus"


def load_corpus(root: Path) -> tuple[list[Document], list[Chunk]]:
    documents: list[Document] = []
    chunks: list[Chunk] = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix.casefold() not in TEXT_SUFFIXES:
            continue
        relative = path.relative_to(root).as_posix()
        project = infer_project(relative)
        text = path.read_text(encoding="utf-8", errors="replace")
        if path.suffix.casefold() == ".md":
            sections = split_sections(relative, text)
        else:
            sections = [(f"({path.suffix.lstrip('.')} artifact)", 1, text.splitlines())]
        headings = tuple(section[0] for section in sections if section[0] != "(preamble)")
        documents.append(Document(project, relative, text, headings))
        for heading, section_start, section_lines in sections:
            section_id = f"{relative}#{section_start}:{heading}"
            for start, end, chunk_text in split_section_lines(
                section_lines,
                section_start,
                CHUNK_CHARS,
                CHUNK_OVERLAP_CHARS,
            ):
                chunk_id = f"{relative}:{start}-{end}"
                chunks.append(
                    Chunk(
                        chunk_id,
                        section_id,
                        project,
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


def chunk_ref(chunk: Chunk) -> str:
    return "chunk-" + hashlib.sha256(chunk.id.encode()).hexdigest()[:12]


def load_state(path: Path) -> dict:
    if not path.exists():
        return {
            "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
            "operations": [],
            "checkpoint": None,
        }
    return json.loads(path.read_text(encoding="utf-8"))


def save_state(path: Path, state: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def record_operation(state: dict, operation: str, arguments: dict, result_chars: int) -> None:
    state["operations"].append({
        "at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "operation": operation,
        "arguments": arguments,
        "result_chars": result_chars,
    })


def commit_operation(
    path: Path,
    operation: str,
    arguments: dict,
    result_chars: int,
    checkpoint: dict | None = None,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.with_name(path.name + ".lock")
    with lock_path.open("a+", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        state = load_state(path)
        record_operation(state, operation, arguments, result_chars)
        if checkpoint is not None:
            state["checkpoint"] = checkpoint
        save_state(path, state)
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


def compact_map(documents: Sequence[Document], scope: str | None = None) -> dict:
    by_project: dict[str, list[Document]] = defaultdict(list)
    for document in documents:
        by_project[document.project].append(document)
    projects = [
        {
            "project": project,
            "documents": len(items),
            "characters": sum(len(item.text) for item in items),
        }
        for project, items in sorted(by_project.items())
    ]
    result: dict = {"projects": projects}
    if scope:
        selected = next((name for name in by_project if name.casefold() == scope.casefold()), None)
        if selected:
            result["scope"] = selected
            result["files"] = [
                {
                    "path": document.path,
                    "characters": len(document.text),
                    "headings": list(document.headings[:24]),
                }
                for document in sorted(by_project[selected], key=lambda item: item.path)
            ]
        else:
            result["scope_error"] = f"Unknown scope: {scope}"
    return result


def diverse_results(
    ranked: Sequence[tuple[float, Chunk]],
    *,
    limit: int,
    max_chars: int,
) -> list[dict]:
    results: list[dict] = []
    seen_chunks: set[str] = set()
    used = 0
    for score, chunk in ranked:
        if chunk.id in seen_chunks:
            continue
        snippet = chunk.text[:1500]
        if results and used + len(snippet) > max_chars:
            continue
        results.append({
            "ref": chunk_ref(chunk),
            "path": chunk.path,
            "project": chunk.project,
            "heading": chunk.heading,
            "lines": [chunk.start_line, chunk.end_line],
            "score": round(score, 6),
            "text": snippet,
        })
        used += len(snippet)
        seen_chunks.add(chunk.id)
        if len(results) >= limit:
            break
    return results


ALLOWED_BINARY = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
}
ALLOWED_UNARY = {ast.UAdd: operator.pos, ast.USub: operator.neg}


def safe_compute(expression: str) -> int | float:
    def evaluate(node: ast.AST) -> int | float:
        if isinstance(node, ast.Expression):
            return evaluate(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
            return node.value
        if isinstance(node, ast.BinOp) and type(node.op) in ALLOWED_BINARY:
            return ALLOWED_BINARY[type(node.op)](evaluate(node.left), evaluate(node.right))
        if isinstance(node, ast.UnaryOp) and type(node.op) in ALLOWED_UNARY:
            return ALLOWED_UNARY[type(node.op)](evaluate(node.operand))
        raise ValueError("Only numeric arithmetic expressions are allowed")

    return evaluate(ast.parse(expression, mode="eval"))


def resolve_chunk(ref: str, chunks: Sequence[Chunk]) -> Chunk | None:
    return next((chunk for chunk in chunks if chunk_ref(chunk) == ref), None)


def read_path(
    root: Path,
    relative: str,
    *,
    start: int | None,
    end: int | None,
    max_chars: int,
) -> dict:
    path = (root / relative).resolve()
    try:
        path.relative_to(root.resolve())
    except ValueError as exc:
        raise ValueError("Path escapes the corpus") from exc
    if not path.is_file():
        raise ValueError(f"Unknown corpus path: {relative}")
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    first = max(1, start or 1)
    last = min(len(lines), end or len(lines))
    text = "\n".join(lines[first - 1:last])
    truncated = len(text) > max_chars
    if truncated:
        text = text[:max_chars]
    return {
        "path": relative,
        "lines": [first, last],
        "text": text,
        "truncated": truncated,
        "next_start": first + text.count("\n") + 1 if truncated else None,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Minimal Brunn agent workspace CLI")
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument(
        "--access-mode",
        choices=("read_write", "read_only"),
        default="read_write",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    open_parser = subparsers.add_parser("open")
    open_parser.add_argument("--scope")

    query_parser = subparsers.add_parser("query")
    query_parser.add_argument("query")
    query_parser.add_argument("--scope")
    query_parser.add_argument("--limit", type=int, default=8)
    query_parser.add_argument("--max-chars", type=int, default=DEFAULT_QUERY_CHARS)

    read_parser = subparsers.add_parser("read")
    read_group = read_parser.add_mutually_exclusive_group(required=True)
    read_group.add_argument("--ref")
    read_group.add_argument("--path")
    read_parser.add_argument("--start", type=int)
    read_parser.add_argument("--end", type=int)
    read_parser.add_argument("--max-chars", type=int, default=20000)

    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("claim")
    verify_parser.add_argument("--scope")
    verify_parser.add_argument("--limit", type=int, default=12)

    compute_parser = subparsers.add_parser("compute")
    compute_parser.add_argument("expression")

    checkpoint_parser = subparsers.add_parser("checkpoint")
    checkpoint_parser.add_argument("--objective", required=True)
    checkpoint_parser.add_argument("--current-state", action="append", default=[])
    checkpoint_parser.add_argument("--decision", action="append", default=[])
    checkpoint_parser.add_argument("--open-question", action="append", default=[])
    checkpoint_parser.add_argument("--next-action", action="append", default=[])
    checkpoint_parser.add_argument("--artifact", action="append", default=[])

    for command in sorted(MUTATION_COMMANDS - {"checkpoint"}):
        mutation_parser = subparsers.add_parser(command)
        mutation_parser.add_argument("payload", nargs="?")

    subparsers.add_parser("status")
    return parser


def main() -> None:
    args = build_parser().parse_args()
    documents, chunks = load_corpus(args.corpus)
    index = BM25Index(chunks)

    if args.access_mode == "read_only" and args.command not in READ_ONLY_COMMANDS:
        result = {
            "error": {
                "code": "capability_denied",
                "operation": f"memory.{args.command}",
                "access_mode": args.access_mode,
            }
        }
        rendered = json.dumps(result, indent=2)
        commit_operation(
            args.session,
            f"denied:{args.command}",
            {
                key: value
                for key, value in vars(args).items()
                if key not in {"corpus", "session"}
            },
            len(rendered),
        )
        print(rendered)
        raise SystemExit(77)

    if args.command in MUTATION_COMMANDS - {"checkpoint"}:
        result = {
            "error": {
                "code": "not_implemented",
                "operation": f"memory.{args.command}",
                "access_mode": args.access_mode,
            }
        }
        rendered = json.dumps(result, indent=2)
        commit_operation(
            args.session,
            f"unsupported:{args.command}",
            {
                key: value
                for key, value in vars(args).items()
                if key not in {"corpus", "session"}
            },
            len(rendered),
        )
        print(rendered)
        raise SystemExit(78)

    if args.command == "open":
        result = compact_map(documents, args.scope)
    elif args.command in {"query", "verify"}:
        query = args.query if args.command == "query" else args.claim
        scope = args.scope
        project = next(
            (document.project for document in documents if scope and document.project.casefold() == scope.casefold()),
            None,
        )
        ranked = index.search(query, project=project)
        if args.command == "verify":
            counter_query = f"{query} current superseded rejected correction not verify evidence"
            ranked = sorted(
                [*ranked[:40], *index.search(counter_query, project=project, limit=40)],
                key=lambda item: (-item[0], item[1].path, item[1].start_line),
            )
        result = {
            "query": query,
            "scope": project,
            "results": diverse_results(
                ranked,
                limit=args.limit,
                max_chars=min(args.max_chars, 10000) if args.command == "query" else DEFAULT_QUERY_CHARS,
            ),
        }
    elif args.command == "read":
        if args.ref:
            chunk = resolve_chunk(args.ref, chunks)
            if not chunk:
                raise SystemExit(f"Unknown chunk ref: {args.ref}")
            result = read_path(
                args.corpus,
                chunk.path,
                start=args.start or chunk.start_line,
                end=args.end or chunk.end_line,
                max_chars=args.max_chars,
            )
            result["ref"] = args.ref
            result["heading"] = chunk.heading
        else:
            result = read_path(
                args.corpus,
                args.path,
                start=args.start,
                end=args.end,
                max_chars=args.max_chars,
            )
    elif args.command == "compute":
        result = {"expression": args.expression, "result": safe_compute(args.expression)}
    elif args.command == "checkpoint":
        result = {
            "objective": args.objective,
            "current_state": args.current_state,
            "decisions": args.decision,
            "open_questions": args.open_question,
            "next_actions": args.next_action,
            "artifacts": args.artifact,
        }
    else:
        state = load_state(args.session)
        result = {
            "corpus_sha256": corpus_hash(documents),
            "documents": len(documents),
            "chunks": len(chunks),
            "operations": state["operations"],
            "checkpoint": state["checkpoint"],
        }

    rendered = json.dumps(result, indent=2)
    commit_operation(
        args.session,
        args.command,
        {key: value for key, value in vars(args).items() if key not in {"corpus", "session"}},
        len(rendered),
        checkpoint=result if args.command == "checkpoint" else None,
    )
    print(rendered)


if __name__ == "__main__":
    main()

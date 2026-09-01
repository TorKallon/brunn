#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from brunn import BrunnService, BrunnStore
from brunn.embeddings import HashingEmbeddingProvider, OpenAIEmbeddingProvider


def save_session(path: Path, session_id: str) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps({"session_id": session_id}) + "\n", encoding="utf-8")
    temporary.replace(path)


def load_session(path: Path) -> str:
    if not path.exists():
        raise SystemExit("Run ./memory resume first")
    return json.loads(path.read_text(encoding="utf-8"))["session_id"]


def provider_from_args(args: argparse.Namespace):
    if args.embeddings == "openai":
        return OpenAIEmbeddingProvider(model=args.embedding_model)
    if args.embeddings == "hashing":
        return HashingEmbeddingProvider()
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description="Transition evaluation adapter")
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--checkpoint-id", required=True)
    parser.add_argument("--scope", required=True)
    parser.add_argument("--task-file", type=Path, required=True)
    parser.add_argument("--session-file", type=Path, required=True)
    parser.add_argument("--embeddings", choices=["none", "hashing", "openai"], default="none")
    parser.add_argument("--embedding-model", default="text-embedding-3-small")
    parser.add_argument("command", choices=["resume", "query", "read", "checkpoint", "status", "schema"])
    parser.add_argument("payload", nargs="?")
    args = parser.parse_args()

    service = BrunnService(BrunnStore(args.db), provider_from_args(args))
    if args.command == "schema":
        result = service.schema()
    elif args.command == "resume":
        if args.session_file.exists():
            result = service.status(load_session(args.session_file))
            result["status"] = "already_open"
        else:
            result = service.open({
                "task": args.task_file.read_text(encoding="utf-8"),
                "scopes": [args.scope],
                "checkpoint_id": args.checkpoint_id,
                "as_of": "latest",
            })
            save_session(args.session_file, result["session_id"])
    elif args.command == "status":
        result = service.status(load_session(args.session_file))
    else:
        if not args.payload:
            raise SystemExit(f"{args.command} requires one JSON payload argument")
        request = json.loads(args.payload)
        request["session_id"] = load_session(args.session_file)
        if args.command == "checkpoint":
            request["parent_checkpoint_id"] = args.checkpoint_id
        result = getattr(service, args.command)(request)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()

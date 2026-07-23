#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from straylight import StraylightService, StraylightStore
from straylight.embeddings import HashingEmbeddingProvider, OpenAIEmbeddingProvider


def provider_from_args(args: argparse.Namespace):
    if args.embeddings == "openai":
        return OpenAIEmbeddingProvider(model=args.embedding_model)
    if args.embeddings == "hashing":
        return HashingEmbeddingProvider()
    return None


def load_request(value: str | None) -> dict:
    raw = value if value is not None else sys.stdin.read()
    if raw.startswith("@"):
        raw = Path(raw[1:]).read_text(encoding="utf-8")
    parsed = json.loads(raw)
    if not isinstance(parsed, dict):
        raise ValueError("request must be a JSON object")
    return parsed


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Straylight persistent workspace API adapter")
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--embeddings", choices=["none", "hashing", "openai"], default="none")
    parser.add_argument("--embedding-model", default="text-embedding-3-small")
    subparsers = parser.add_subparsers(dest="command", required=True)

    ingest = subparsers.add_parser("ingest")
    ingest.add_argument("--root", type=Path, required=True)
    ingest.add_argument("--note", default="directory ingestion")

    ingest_file = subparsers.add_parser("ingest-file")
    ingest_file.add_argument("--path", type=Path, required=True)
    ingest_file.add_argument("--as-path", required=True)
    ingest_file.add_argument("--note", default="file ingestion")

    subparsers.add_parser("index")
    subparsers.add_parser("schema")
    for name in ["open", "query", "read", "checkpoint"]:
        operation = subparsers.add_parser(name)
        operation.add_argument("--request")
    status = subparsers.add_parser("status")
    status.add_argument("--session-id", required=True)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    store = StraylightStore(args.db)
    provider = provider_from_args(args)
    service = StraylightService(store, provider)

    if args.command == "ingest":
        result = store.ingest_directory(args.root, note=args.note)
    elif args.command == "ingest-file":
        result = store.ingest_documents(
            {args.as_path: args.path.read_text(encoding="utf-8", errors="replace")},
            note=args.note,
        )
    elif args.command == "index":
        if not provider:
            raise SystemExit("--embeddings hashing or --embeddings openai is required for indexing")
        result = store.index_embeddings(provider)
    elif args.command == "schema":
        result = service.schema()
    elif args.command == "status":
        result = service.status(args.session_id)
    else:
        request = load_request(args.request)
        result = getattr(service, args.command)(request)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()

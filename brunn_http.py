#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from brunn import BrunnService, BrunnStore
from brunn.embeddings import HashingEmbeddingProvider, OpenAIEmbeddingProvider


class BrunnHandler(BaseHTTPRequestHandler):
    service: BrunnService
    server_version = "Brunn/0.1"

    def log_message(self, format: str, *args: Any) -> None:
        return

    def send_json(self, status: int, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(HTTPStatus.OK, {"status": "ok"})
        elif self.path == "/v1/schema":
            self.send_json(HTTPStatus.OK, self.service.schema())
        else:
            self.send_json(HTTPStatus.NOT_FOUND, {"error": "not_found"})

    def do_POST(self) -> None:
        routes = {
            "/v1/memory/open": self.service.open,
            "/v1/memory/query": self.service.query,
            "/v1/memory/read": self.service.read,
            "/v1/memory/checkpoint": self.service.checkpoint,
        }
        operation = routes.get(self.path)
        if not operation:
            self.send_json(HTTPStatus.NOT_FOUND, {"error": "not_found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            request = json.loads(self.rfile.read(length) or b"{}")
            self.send_json(HTTPStatus.OK, operation(request))
        except (ValueError, KeyError, json.JSONDecodeError) as exc:
            self.send_json(HTTPStatus.BAD_REQUEST, {"error": type(exc).__name__, "message": str(exc)})
        except Exception as exc:
            self.send_json(HTTPStatus.INTERNAL_SERVER_ERROR, {"error": type(exc).__name__, "message": str(exc)})


def build_server(
    db: Path,
    host: str = "127.0.0.1",
    port: int = 0,
    embeddings: str = "none",
    embedding_model: str = "text-embedding-3-small",
) -> ThreadingHTTPServer:
    if embeddings == "openai":
        provider = OpenAIEmbeddingProvider(model=embedding_model)
    elif embeddings == "hashing":
        provider = HashingEmbeddingProvider()
    else:
        provider = None
    handler = type("ConfiguredBrunnHandler", (BrunnHandler,), {})
    handler.service = BrunnService(BrunnStore(db), provider)
    return ThreadingHTTPServer((host, port), handler)


def main() -> None:
    parser = argparse.ArgumentParser(description="Brunn HTTP API")
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8787)
    parser.add_argument("--embeddings", choices=["none", "hashing", "openai"], default="none")
    parser.add_argument("--embedding-model", default="text-embedding-3-small")
    args = parser.parse_args()
    server = build_server(args.db, args.host, args.port, args.embeddings, args.embedding_model)
    print(json.dumps({"status": "listening", "host": args.host, "port": server.server_port}))
    server.serve_forever()


if __name__ == "__main__":
    main()

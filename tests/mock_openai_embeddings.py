#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import signal
import subprocess
import sys
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


TOKEN = re.compile(r"[A-Za-z0-9_-]+")
DEFAULT_HOST = "0.0.0.0"
DEFAULT_PORT = 55200
DEFAULT_DIMENSIONS = 1536
DEFAULT_STATE = Path("/tmp/straylight-embedding-mock-55200.pid")
DEFAULT_LOG = Path("/tmp/straylight-embedding-mock-55200.log")


def embedding(text: str, dimensions: int) -> list[float]:
    vector = [0.0] * dimensions
    for token in TOKEN.findall(text.casefold()):
        digest = hashlib.sha256(token.encode("utf-8")).digest()
        index = int.from_bytes(digest[:4], "big") % dimensions
        vector[index] += 1.0 if digest[4] & 1 else -1.0
    norm = math.sqrt(sum(value * value for value in vector))
    if norm:
        vector = [value / norm for value in vector]
    return vector


class Handler(BaseHTTPRequestHandler):
    server_version = "StraylightEmbeddingMock/1"

    def do_GET(self) -> None:
        if self.path == "/health":
            self._json(200, {"status": "ok"})
        else:
            self._json(404, {"error": "not_found"})

    def do_POST(self) -> None:
        if not self.path.endswith("/embeddings"):
            self._json(404, {"error": "not_found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            payload = json.loads(self.rfile.read(length))
            inputs = payload.get("input", [])
            if isinstance(inputs, str):
                inputs = [inputs]
            if not isinstance(inputs, list) or not all(
                isinstance(value, str) for value in inputs
            ):
                raise ValueError("input must be a string or string array")
            dimensions = int(payload.get("dimensions") or DEFAULT_DIMENSIONS)
            if dimensions < 1 or dimensions > 4096:
                raise ValueError("dimensions are out of range")
        except (ValueError, TypeError, json.JSONDecodeError) as error:
            self._json(400, {"error": {"message": str(error)}})
            return
        self._json(
            200,
            {
                "object": "list",
                "model": payload.get("model", "mock-embedding"),
                "data": [
                    {
                        "object": "embedding",
                        "index": index,
                        "embedding": embedding(text, dimensions),
                    }
                    for index, text in enumerate(inputs)
                ],
                "usage": {
                    "prompt_tokens": sum(max(1, len(text) // 4) for text in inputs),
                    "total_tokens": sum(max(1, len(text) // 4) for text in inputs),
                },
            },
        )

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _json(self, status: int, value: Any) -> None:
        body = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def serve(host: str, port: int) -> int:
    server = ThreadingHTTPServer((host, port), Handler)
    server.serve_forever()
    return 0


def read_pid(state: Path) -> int | None:
    try:
        return int(state.read_text(encoding="ascii").strip())
    except (FileNotFoundError, ValueError):
        return None


def process_is_live(pid: int | None) -> bool:
    if pid is None:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def health(port: int) -> bool:
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/health",
            timeout=0.25,
        ) as response:
            return response.status == 200
    except OSError:
        return False


def start(state: Path, log: Path, port: int) -> int:
    pid = read_pid(state)
    if process_is_live(pid) and health(port):
        return 0
    state.unlink(missing_ok=True)
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("ab") as output:
        process = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "serve",
                "--port",
                str(port),
            ],
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=output,
            start_new_session=True,
        )
    state.write_text(f"{process.pid}\n", encoding="ascii")
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if health(port):
            return 0
        if process.poll() is not None:
            break
        time.sleep(0.05)
    stop(state)
    return 1


def stop(state: Path) -> int:
    pid = read_pid(state)
    state.unlink(missing_ok=True)
    if not process_is_live(pid):
        return 0
    assert pid is not None
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if not process_is_live(pid):
            return 0
        time.sleep(0.05)
    os.kill(pid, signal.SIGKILL)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("serve", "start", "stop", "status"))
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--state", type=Path, default=DEFAULT_STATE)
    parser.add_argument("--log", type=Path, default=DEFAULT_LOG)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "serve":
        return serve(args.host, args.port)
    if args.command == "start":
        return start(args.state, args.log, args.port)
    if args.command == "stop":
        return stop(args.state)
    return 0 if process_is_live(read_pid(args.state)) and health(args.port) else 1


if __name__ == "__main__":
    raise SystemExit(main())

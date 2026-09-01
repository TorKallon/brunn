#!/usr/bin/env python3

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import stat
import urllib.request
from datetime import datetime
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any


def load_manifest(path: Path) -> tuple[Path, dict[str, dict[str, Any]]]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or not isinstance(value.get("assets"), list):
        raise ValueError("invalid filesystem asset manifest")
    corpus_root = Path(value["corpus_root"]).resolve()
    assets = {
        str(item["path"]): item
        for item in value["assets"]
        if isinstance(item, dict) and isinstance(item.get("path"), str)
    }
    return corpus_root, assets


def asset_file(
    corpus_root: Path,
    assets: dict[str, dict[str, Any]],
    logical_path: str,
) -> tuple[Path, dict[str, Any]]:
    metadata = assets.get(logical_path)
    if metadata is None:
        raise ValueError("asset path is not part of this evaluation card")
    relative = PurePosixPath(logical_path)
    if relative.is_absolute() or any(part in {"", ".", ".."} for part in relative.parts):
        raise ValueError("asset path is not a safe relative path")
    path = corpus_root.joinpath(*relative.parts)
    lexical = Path(os.path.abspath(path))
    try:
        parts = lexical.relative_to(corpus_root).parts
    except ValueError as exc:
        raise ValueError("asset path is outside the evaluation corpus") from exc
    current = corpus_root
    for part in parts:
        current = current / part
        file_stat = current.lstat()
        if stat.S_ISLNK(file_stat.st_mode):
            raise ValueError("asset path may not contain symlinks")
    resolved = lexical.resolve(strict=True)
    resolved.relative_to(corpus_root)
    file_stat = resolved.lstat()
    if not stat.S_ISREG(file_stat.st_mode):
        raise ValueError("asset is not a regular file")
    return resolved, metadata


def verify(path: Path, metadata: dict[str, Any]) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    file_stat = os.fstat(descriptor)
    if not stat.S_ISREG(file_stat.st_mode):
        os.close(descriptor)
        raise ValueError("asset is not a regular file")
    with os.fdopen(descriptor, "rb") as source:
        while chunk := source.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    expected_hash = str(metadata["content_hash"]).removeprefix("sha256:")
    if size != int(metadata["size_bytes"]) or digest.hexdigest() != expected_hash:
        raise RuntimeError("filesystem asset failed size or SHA-256 verification")
    return f"sha256:{digest.hexdigest()}", size


def append_trace(path: Path, record: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "a", encoding="utf-8") as output:
            fcntl.flock(output.fileno(), fcntl.LOCK_EX)
            output.write(json.dumps(record, separators=(",", ":")) + "\n")
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        try:
            os.close(descriptor)
        except OSError:
            pass
        raise


def supervised_operation(path: Path, operation: str) -> dict[str, Any]:
    endpoint = os.environ.get("BRUNN_STATE_INSPECT_URL")
    capability = os.environ.get("BRUNN_STATE_INSPECT_CAPABILITY")
    if not endpoint or not capability:
        raise RuntimeError("trusted inspection supervisor is not available")
    request = urllib.request.Request(
        endpoint,
        data=json.dumps({
            "capability": capability,
            "path": str(path),
            "operation": operation,
        }).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        value = json.loads(response.read())
    if not isinstance(value, dict):
        raise RuntimeError("trusted inspection supervisor returned invalid data")
    return value


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Audited verifier for native files in a matched filesystem evaluation",
    )
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("operation", choices=("list", "metadata", "fetch", "inspect"))
    parser.add_argument("path", nargs="?")
    args = parser.parse_args()
    corpus_root, assets = load_manifest(args.manifest)
    if args.operation == "list":
        print(json.dumps({"assets": list(assets.values())}, indent=2))
        return
    if not args.path:
        parser.error(f"{args.operation} requires an exact logical path")
    resolved, metadata = asset_file(corpus_root, assets, args.path)
    content_hash = str(metadata["content_hash"])
    size = int(metadata["size_bytes"])
    bytes_read = 0
    if args.operation in {"fetch", "inspect"}:
        content_hash, size = verify(resolved, metadata)
        bytes_read = size
    trusted = supervised_operation(
        resolved,
        "inspect" if args.operation == "inspect" else args.operation,
    )
    record = {
        "at": datetime.now().astimezone().isoformat(timespec="milliseconds"),
        "operation": args.operation,
        "path": args.path,
        "content_hash": content_hash,
        "size_bytes": size,
        "bytes_read": bytes_read,
        "local_path": str(resolved),
        "verified": args.operation in {"fetch", "inspect"},
        "trusted_receipt": trusted,
        "untrusted_client_trace": True,
    }
    append_trace(args.trace, record)
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()

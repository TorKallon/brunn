#!/usr/bin/env python3

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import stat
from datetime import datetime
from pathlib import Path
from typing import Any


def load_assets(
    path: Path,
) -> tuple[Path, dict[tuple[str, int], dict[str, Any]]]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or not isinstance(value.get("assets"), list):
        raise ValueError("invalid local inspection manifest")
    return Path(value["corpus_root"]).resolve(), {
        (
            str(item["content_hash"]).removeprefix("sha256:"),
            int(item["size_bytes"]),
        ): item
        for item in value["assets"]
        if isinstance(item, dict)
    }


def hash_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    file_stat = os.fstat(descriptor)
    if not stat.S_ISREG(file_stat.st_mode):
        os.close(descriptor)
        raise ValueError("inspection target is not a regular file")
    with os.fdopen(descriptor, "rb") as source:
        while chunk := source.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    return digest.hexdigest(), size


def append_trace(path: Path, record: dict[str, Any]) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    with os.fdopen(descriptor, "a", encoding="utf-8") as output:
        os.fchmod(output.fileno(), 0o600)
        fcntl.flock(output.fileno(), fcntl.LOCK_EX)
        output.write(json.dumps(record, separators=(",", ":")) + "\n")
        output.flush()
        os.fsync(output.fileno())


def _is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def resolve_regular_file(path: Path, roots: tuple[Path, ...]) -> Path:
    lexical = Path(os.path.abspath(path))
    root = next((candidate for candidate in roots if _is_within(lexical, candidate)), None)
    if root is None:
        raise ValueError("inspection target is outside the evaluation roots")
    current = root
    for part in lexical.relative_to(root).parts:
        current = current / part
        file_stat = current.lstat()
        if stat.S_ISLNK(file_stat.st_mode):
            raise ValueError("inspection target may not contain symlinks")
    resolved = lexical.resolve(strict=True)
    if not _is_within(resolved, root):
        raise ValueError("inspection target is outside the evaluation roots")
    file_stat = resolved.lstat()
    if not stat.S_ISREG(file_stat.st_mode):
        raise ValueError("inspection target is not a regular file")
    return resolved


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Verify and record local inspection of a fetched evaluation asset",
    )
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--run-root", type=Path, required=True)
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    run_root = args.run_root.resolve()
    local_path = args.path if args.path.is_absolute() else Path.cwd() / args.path
    corpus_root, assets = load_assets(args.manifest)
    local_path = resolve_regular_file(local_path, (run_root, corpus_root))
    digest, size = hash_file(local_path)
    metadata = assets.get((digest, size))
    if metadata is None:
        raise RuntimeError("inspection target is not an expected card asset")
    record = {
        "at": datetime.now().astimezone().isoformat(timespec="milliseconds"),
        "operation": "local_inspection",
        "path": metadata["path"],
        "content_hash": f"sha256:{digest}",
        "size_bytes": size,
        "bytes_read": size,
        "media_type": metadata.get("media_type"),
        "local_path": str(local_path),
        "verified": True,
    }
    append_trace(args.trace, record)
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3

"""Read-only owner-vault snapshot import and scoped fidelity-audit tooling.

The simplified evaluation import route accepts UTF-8 text only.  This module
therefore inventories the complete filesystem snapshot, imports only the
explicitly supported text subset into an isolated read-only evaluation user,
and keeps the unsupported/binary/history boundaries visible in every artifact.
It never follows symlinks and never writes to the source vault.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import mimetypes
import os
import re
import stat
import sys
import unicodedata
import urllib.parse
from collections import defaultdict
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any, Iterator, Mapping, Sequence

from native_eval import (
    NativeApiClient,
    provision_evaluation,
    recursively_redact_secrets,
)


INVENTORY_SCHEMA = "straylight-owner-snapshot-inventory@v1"
AUDIT_SCHEMA = "straylight-owner-snapshot-audit@v1"
LINK_CANDIDATES_SCHEMA = "straylight-owner-link-candidates@v1"
LEAK_REQUEST_SCHEMA = "straylight-owner-link-leak-request@v1"
LEAK_REPORT_SCHEMA = "straylight-owner-link-leak-report@v1"
CREDENTIAL_SCHEMA = "straylight-owner-eval-credential@v1"

MAX_TEXT_BYTES = 4 * 1024 * 1024
DEFAULT_BATCH_SIZE = 1_000
MAX_BATCH_DOCUMENTS = 10_000
MAX_SAFE_BATCH_BYTES = 384 * 1024 * 1024

BINARY_EXTENSIONS = {
    ".7z",
    ".avif",
    ".bz2",
    ".db",
    ".dmg",
    ".doc",
    ".docx",
    ".gif",
    ".gz",
    ".heic",
    ".ico",
    ".jpeg",
    ".jpg",
    ".mov",
    ".mp3",
    ".mp4",
    ".otf",
    ".parquet",
    ".pdf",
    ".png",
    ".ppt",
    ".pptx",
    ".rar",
    ".sqlite",
    ".sqlite3",
    ".tif",
    ".tiff",
    ".ttf",
    ".wav",
    ".webp",
    ".woff",
    ".woff2",
    ".xls",
    ".xlsx",
    ".xz",
    ".zip",
}

BINARY_SIGNATURES = (
    (b"\x89PNG\r\n\x1a\n", "png_signature"),
    (b"\xff\xd8\xff", "jpeg_signature"),
    (b"GIF87a", "gif_signature"),
    (b"GIF89a", "gif_signature"),
    (b"%PDF-", "pdf_signature"),
    (b"PK\x03\x04", "zip_signature"),
    (b"PK\x05\x06", "zip_signature"),
    (b"\x1f\x8b", "gzip_signature"),
    (b"SQLite format 3\x00", "sqlite_signature"),
    (b"PAR1", "parquet_signature"),
)

WIKI_LINK_RE = re.compile(r"!?\[\[([^\]\r\n]+)\]\]")
CHECKPOINT_PATH_RE = re.compile(
    r"^\.straylight/checkpoints/([0-9a-fA-F-]{32,36})\.md$"
)
CHECKPOINT_ID_RE = re.compile(
    r"^(?:checkpoint:)?([0-9a-fA-F]{32}|[0-9a-fA-F-]{36})$"
)


class SnapshotError(RuntimeError):
    """A deterministic preflight or fidelity failure."""


def utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return "sha256:" + hashlib.sha256(value).hexdigest()


def strip_sha256(value: str) -> str:
    return value.removeprefix("sha256:").lower()


def portable_path_key(value: str) -> str:
    return unicodedata.normalize("NFC", value).casefold()


def validate_relative_path(value: str) -> str | None:
    if (
        not value
        or len(value) > 1_024
        or value.startswith("/")
        or "\\" in value
        or any(unicodedata.category(character).startswith("C") for character in value)
    ):
        return "unsupported_workspace_path"
    if any(part in {"", ".", ".."} for part in value.split("/")):
        return "unsupported_workspace_path"
    return None


def source_stat_signature(value: os.stat_result) -> tuple[int, int, int, int]:
    return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns)


def read_stable_bytes(path: Path) -> bytes:
    before = path.stat(follow_symlinks=False)
    if not stat.S_ISREG(before.st_mode):
        raise SnapshotError(f"source path is no longer a regular file: {path}")
    with path.open("rb") as handle:
        opened = os.fstat(handle.fileno())
        if source_stat_signature(before) != source_stat_signature(opened):
            raise SnapshotError(f"source path changed before it was read: {path}")
        payload = handle.read()
        closed = os.fstat(handle.fileno())
    after = path.stat(follow_symlinks=False)
    expected = source_stat_signature(before)
    if (
        source_stat_signature(closed) != expected
        or source_stat_signature(after) != expected
        or len(payload) != before.st_size
    ):
        raise SnapshotError(f"source path changed while it was read: {path}")
    return payload


def walk_snapshot(root: Path) -> Iterator[tuple[str, Path, str]]:
    """Yield sorted (relative path, local path, type) without following links."""

    pending: list[tuple[str, Path]] = [("", root)]
    while pending:
        prefix, directory = pending.pop()
        try:
            children = sorted(
                os.scandir(directory),
                key=lambda item: unicodedata.normalize("NFC", item.name),
                reverse=True,
            )
        except OSError as error:
            raise SnapshotError(f"could not inventory {directory}: {error}") from error
        for child in children:
            relative = f"{prefix}/{child.name}".lstrip("/")
            path = Path(child.path)
            if child.is_symlink():
                yield relative, path, "symlink"
            elif child.is_dir(follow_symlinks=False):
                pending.append((relative, path))
            elif child.is_file(follow_symlinks=False):
                yield relative, path, "file"
            else:
                yield relative, path, "special"


def detected_binary_reason(path: Path, payload: bytes) -> str | None:
    if len(payload) > MAX_TEXT_BYTES:
        return "larger_than_4_mib_text_limit"
    if b"\x00" in payload:
        return "contains_nul"
    if path.suffix.casefold() in BINARY_EXTENSIONS:
        return "known_binary_extension"
    for signature, reason in BINARY_SIGNATURES:
        if payload.startswith(signature):
            return reason
    try:
        payload.decode("utf-8", errors="strict")
    except UnicodeDecodeError:
        return "invalid_utf8"
    return None


def text_media_type(path: Path) -> str:
    suffix = path.suffix.casefold()
    if suffix in {".md", ".markdown"}:
        return "text/markdown"
    if suffix == ".json":
        return "application/json"
    if suffix == ".jsonl":
        return "application/x-ndjson"
    if suffix == ".csv":
        return "text/csv"
    if suffix == ".tsv":
        return "text/tab-separated-values"
    guessed = mimetypes.guess_type(path.name)[0]
    if guessed and (
        guessed.startswith("text/")
        or guessed
        in {
            "application/javascript",
            "application/json",
            "application/sql",
            "application/toml",
            "application/xml",
            "application/yaml",
        }
        or guessed.endswith("+json")
        or guessed.endswith("+xml")
    ):
        return guessed
    return "text/plain"


def normalize_checkpoint_ref(value: str) -> str | None:
    raw = value.strip().strip("\"'")
    match = CHECKPOINT_ID_RE.fullmatch(raw)
    if match is None:
        return None
    compact = match.group(1).replace("-", "").lower()
    if len(compact) != 32:
        return None
    return "checkpoint:" + compact


def frontmatter_scalars(text: str) -> dict[str, list[str]]:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    closing = next(
        (index for index, line in enumerate(lines[1:], start=1) if line.strip() == "---"),
        None,
    )
    if closing is None:
        return {}
    values: dict[str, list[str]] = defaultdict(list)
    for line in lines[1:closing]:
        match = re.match(r"^\s*([A-Za-z0-9_-]+)\s*:\s*(.*?)\s*$", line)
        if match is not None:
            values[match.group(1).casefold()].append(match.group(2))
    return dict(values)


def walk_json_checkpoint_fields(
    value: Any,
    *,
    definitions: list[str],
    parents: list[str],
) -> None:
    if isinstance(value, Mapping):
        for key, child in value.items():
            folded = str(key).casefold()
            if folded in {"checkpoint_id", "checkpoint_ref"} and isinstance(child, str):
                definitions.append(child)
            elif folded in {"parent_checkpoint_id", "parent_checkpoint_ref"} and isinstance(
                child, str
            ):
                parents.append(child)
            walk_json_checkpoint_fields(
                child,
                definitions=definitions,
                parents=parents,
            )
    elif isinstance(value, list):
        for child in value:
            walk_json_checkpoint_fields(
                child,
                definitions=definitions,
                parents=parents,
            )


def checkpoint_values(path: str, text: str) -> tuple[list[str], list[str]]:
    definitions: list[str] = []
    parents: list[str] = []
    path_match = CHECKPOINT_PATH_RE.fullmatch(path)
    if path_match is not None:
        definitions.append(path_match.group(1))
    frontmatter = frontmatter_scalars(text)
    for key in ("checkpoint_id", "checkpoint_ref"):
        definitions.extend(
            value
            for value in frontmatter.get(key, [])
            if value.strip().casefold() not in {"", "null", "~"}
        )
    for key in ("parent_checkpoint_id", "parent_checkpoint_ref"):
        parents.extend(
            value
            for value in frontmatter.get(key, [])
            if value.strip().casefold() not in {"", "null", "~"}
        )
    if path.casefold().endswith((".json", ".jsonl")):
        json_values: list[Any] = []
        try:
            if path.casefold().endswith(".jsonl"):
                json_values = [
                    json.loads(line)
                    for line in text.splitlines()
                    if line.strip()
                ]
            else:
                json_values = [json.loads(text)]
        except json.JSONDecodeError:
            json_values = []
        for value in json_values:
            walk_json_checkpoint_fields(
                value,
                definitions=definitions,
                parents=parents,
            )
    return definitions, parents


def checkpoint_lineage(
    root: Path,
    entries: Sequence[Mapping[str, Any]],
) -> dict[str, Any]:
    definitions: dict[str, set[str]] = defaultdict(set)
    raw_parents: list[dict[str, str | None]] = []
    for entry in entries:
        if entry.get("classification") != "supported_text":
            continue
        path = str(entry["path"])
        payload = read_stable_bytes(root / PurePosixPath(path))
        text = payload.decode("utf-8", errors="strict")
        raw_definitions, parents = checkpoint_values(path, text)
        for raw in raw_definitions:
            normalized = normalize_checkpoint_ref(raw)
            if normalized is not None:
                definitions[normalized].add(path)
        for raw in parents:
            raw_parents.append(
                {
                    "source_path": path,
                    "raw_sha256": sha256_bytes(raw.encode("utf-8")),
                    "normalized_parent": normalize_checkpoint_ref(raw),
                }
            )
    references: list[dict[str, Any]] = []
    for parent in raw_parents:
        normalized = parent["normalized_parent"]
        if normalized is None:
            status = "invalid"
        elif len(definitions.get(normalized, [])) == 1:
            status = "resolved"
        elif len(definitions.get(normalized, [])) > 1:
            status = "ambiguous"
        else:
            status = "unresolved"
        references.append(
            {
                **parent,
                "status": status,
                "definition_paths": sorted(definitions.get(normalized or "", [])),
            }
        )
    duplicate_definitions = [
        {"checkpoint_ref": reference, "paths": sorted(paths)}
        for reference, paths in sorted(definitions.items())
        if len(paths) > 1
    ]
    counts = {
        status: sum(1 for reference in references if reference["status"] == status)
        for status in ("resolved", "unresolved", "ambiguous", "invalid")
    }
    return {
        "extraction": "checkpoint_path_plus_frontmatter_and_json_fields",
        "service_representation": "source_text_only_on_evaluation_import",
        "definitions": len(definitions),
        "duplicate_definitions": duplicate_definitions,
        "references": references,
        "counts": counts,
        "source_resolution_pass": (
            not duplicate_definitions
            and counts["unresolved"] == 0
            and counts["ambiguous"] == 0
            and counts["invalid"] == 0
        ),
    }


def inventory_identity(artifact: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "schema": artifact["schema"],
        "snapshot": artifact["snapshot"],
        "boundaries": artifact["boundaries"],
        "entries": artifact["entries"],
        "checkpoint_lineage": artifact["checkpoint_lineage"],
    }


def build_inventory(root: Path) -> dict[str, Any]:
    root = root.expanduser().resolve(strict=True)
    if not root.is_dir():
        raise SnapshotError(f"snapshot root is not a directory: {root}")
    provisional: list[dict[str, Any]] = []
    collision_paths: dict[str, list[str]] = defaultdict(list)
    for relative, path, entry_type in walk_snapshot(root):
        invalid_path = validate_relative_path(relative)
        if entry_type == "symlink":
            provisional.append(
                {
                    "path": relative,
                    "classification": "ignored_symlink",
                    "reason": "symbolic_link_not_followed",
                    "size_bytes": None,
                    "sha256": None,
                    "media_type": None,
                }
            )
            continue
        if entry_type == "special":
            provisional.append(
                {
                    "path": relative,
                    "classification": "unsupported_special",
                    "reason": "not_a_regular_file",
                    "size_bytes": None,
                    "sha256": None,
                    "media_type": None,
                }
            )
            continue
        payload = read_stable_bytes(path)
        reason = invalid_path or detected_binary_reason(path, payload)
        classification = "supported_text" if reason is None else "unsupported_binary"
        media_type = (
            text_media_type(path)
            if classification == "supported_text"
            else (mimetypes.guess_type(path.name)[0] or "application/octet-stream")
        )
        provisional.append(
            {
                "path": relative,
                "classification": classification,
                "reason": reason or "valid_utf8_text",
                "size_bytes": len(payload),
                "sha256": sha256_bytes(payload),
                "media_type": media_type,
            }
        )
        collision_paths[portable_path_key(relative)].append(relative)
    collisions = {
        path
        for paths in collision_paths.values()
        if len(paths) > 1
        for path in paths
    }
    entries: list[dict[str, Any]] = []
    for entry in sorted(provisional, key=lambda item: str(item["path"])):
        if entry["path"] in collisions and entry["classification"] == "supported_text":
            entry = {
                **entry,
                "classification": "unsupported_collision",
                "reason": "case_or_unicode_normalized_path_collision",
            }
        entries.append(entry)
    lineage = checkpoint_lineage(root, entries)
    classifications = sorted({str(entry["classification"]) for entry in entries})
    totals_by_classification = {
        classification: {
            "files": sum(
                1 for entry in entries if entry["classification"] == classification
            ),
            "bytes": sum(
                int(entry["size_bytes"] or 0)
                for entry in entries
                if entry["classification"] == classification
            ),
        }
        for classification in classifications
    }
    artifact: dict[str, Any] = {
        "schema": INVENTORY_SCHEMA,
        "created_at": utc_now(),
        "snapshot": {
            "root": str(root),
            "kind": "current_filesystem_snapshot",
            "authority": "owner_obsidian_vault",
            "symlinks_followed": False,
        },
        "boundaries": {
            "supported_text": (
                "regular files no larger than 4 MiB with safe workspace-relative "
                "paths, valid UTF-8, no NUL byte, and no known binary signature or extension"
            ),
            "binary": "inventoried_with_exact_size_and_sha256_but_not_imported",
            "history": {
                "included": False,
                "version_lineage": "not_available_from_current_filesystem_snapshot",
                "deletion_history": "not_represented",
            },
            "evaluation_import": {
                "access_mode": "read_only",
                "entry_kind": "markdown",
                "binary_transport": False,
            },
        },
        "entries": entries,
        "checkpoint_lineage": lineage,
        "totals": {
            "entries": len(entries),
            "regular_files": sum(
                1 for entry in entries if entry["size_bytes"] is not None
            ),
            "bytes": sum(int(entry["size_bytes"] or 0) for entry in entries),
            "by_classification": totals_by_classification,
        },
    }
    artifact["inventory_sha256"] = sha256_bytes(
        canonical_json(inventory_identity(artifact))
    )
    return artifact


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SnapshotError(f"could not read JSON from {path}: {error}") from error
    if not isinstance(value, dict):
        raise SnapshotError(f"expected a JSON object in {path}")
    return value


def load_inventory(path: Path) -> dict[str, Any]:
    artifact = load_json(path)
    if artifact.get("schema") != INVENTORY_SCHEMA:
        raise SnapshotError(f"{path} is not a {INVENTORY_SCHEMA} artifact")
    expected = sha256_bytes(canonical_json(inventory_identity(artifact)))
    if artifact.get("inventory_sha256") != expected:
        raise SnapshotError(f"inventory fingerprint mismatch: {path}")
    return artifact


def snapshot_root(
    inventory: Mapping[str, Any],
    override: Path | None = None,
) -> Path:
    root = override or Path(str(inventory["snapshot"]["root"]))
    resolved = root.expanduser().resolve(strict=True)
    if not resolved.is_dir():
        raise SnapshotError(f"snapshot root is not a directory: {resolved}")
    return resolved


def supported_entries(inventory: Mapping[str, Any]) -> list[dict[str, Any]]:
    return [
        dict(entry)
        for entry in inventory["entries"]
        if entry.get("classification") == "supported_text"
    ]


def verify_snapshot_unchanged(
    inventory: Mapping[str, Any],
    root: Path,
    *,
    all_regular_files: bool = True,
) -> None:
    for entry in inventory["entries"]:
        if entry.get("size_bytes") is None:
            continue
        if not all_regular_files and entry.get("classification") != "supported_text":
            continue
        path = root / PurePosixPath(str(entry["path"]))
        payload = read_stable_bytes(path)
        if len(payload) != entry["size_bytes"] or sha256_bytes(payload) != entry["sha256"]:
            raise SnapshotError(
                f"source snapshot changed after inventory: {entry['path']}"
            )


def import_documents(
    inventory: Mapping[str, Any],
    root: Path,
    *,
    batch_size: int,
) -> list[dict[str, Any]]:
    if batch_size <= 0 or batch_size > MAX_BATCH_DOCUMENTS:
        raise SnapshotError(
            f"batch size must be between 1 and {MAX_BATCH_DOCUMENTS}"
        )
    documents: list[dict[str, Any]] = []
    batch_bytes = 0
    batch_count = 0
    for entry in supported_entries(inventory):
        payload = read_stable_bytes(root / PurePosixPath(entry["path"]))
        if len(payload) != entry["size_bytes"] or sha256_bytes(payload) != entry["sha256"]:
            raise SnapshotError(
                f"source snapshot changed after inventory: {entry['path']}"
            )
        text = payload.decode("utf-8", errors="strict")
        documents.append(
            {
                "path": entry["path"],
                "content": text,
                "content_sha256": strip_sha256(entry["sha256"]),
                "media_type": entry["media_type"],
            }
        )
        batch_bytes += len(payload)
        batch_count += 1
        if batch_count == batch_size:
            if batch_bytes > MAX_SAFE_BATCH_BYTES:
                raise SnapshotError(
                    "an evaluation import batch exceeds the 384 MiB safety limit; "
                    "rerun with a smaller --batch-size"
                )
            batch_bytes = 0
            batch_count = 0
    if batch_bytes > MAX_SAFE_BATCH_BYTES:
        raise SnapshotError(
            "an evaluation import batch exceeds the 384 MiB safety limit; "
            "rerun with a smaller --batch-size"
        )
    if not documents:
        raise SnapshotError("snapshot has no supported text documents to import")
    return documents


def manifest_entries(client: NativeApiClient) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    cursor: dict[str, Any] = {}
    while True:
        query = {"limit": 5_000, "history": "false", **cursor}
        response = client.get(
            "/v1/workspace/manifest?" + urllib.parse.urlencode(query)
        )
        data = response.data
        page = data.get("entries")
        if not isinstance(page, list) or any(not isinstance(item, dict) for item in page):
            raise SnapshotError("workspace manifest omitted its entries array")
        entries.extend(dict(item) for item in page)
        next_cursor = data.get("next")
        if next_cursor is None:
            break
        if not isinstance(next_cursor, dict):
            raise SnapshotError("workspace manifest returned an invalid cursor")
        if not {
            "after_path",
            "after_entry_ref",
        }.issubset(next_cursor):
            raise SnapshotError("workspace manifest cursor is incomplete")
        cursor = {
            "after_path": next_cursor["after_path"],
            "after_entry_ref": next_cursor["after_entry_ref"],
        }
    return entries


def fidelity_differences(
    inventory: Mapping[str, Any],
    actual_entries: Sequence[Mapping[str, Any]],
) -> dict[str, Any]:
    expected = {entry["path"]: entry for entry in supported_entries(inventory)}
    actual_by_path: dict[str, Mapping[str, Any]] = {}
    duplicate_actual: list[str] = []
    for entry in actual_entries:
        path = str(entry.get("path") or "")
        if path in actual_by_path:
            duplicate_actual.append(path)
        actual_by_path[path] = entry
    missing = sorted(set(expected) - set(actual_by_path))
    unexpected = sorted(set(actual_by_path) - set(expected))
    mismatches: list[dict[str, Any]] = []
    for path in sorted(set(expected) & set(actual_by_path)):
        wanted = expected[path]
        observed = actual_by_path[path]
        fields: dict[str, Any] = {}
        observed_hash = str(observed.get("content_hash") or "")
        if observed_hash != wanted["sha256"]:
            fields["sha256"] = {
                "expected": wanted["sha256"],
                "actual": observed_hash or None,
            }
        observed_size = observed.get("size_bytes")
        if observed_size != wanted["size_bytes"]:
            fields["size_bytes"] = {
                "expected": wanted["size_bytes"],
                "actual": observed_size,
            }
        if observed.get("kind") != "markdown":
            fields["kind"] = {
                "expected": "markdown",
                "actual": observed.get("kind"),
            }
        if fields:
            mismatches.append({"path": path, "fields": fields})
    return {
        "expected_entries": len(expected),
        "actual_entries": len(actual_entries),
        "matched_entries": len(expected) - len(missing) - len(mismatches),
        "missing_paths": missing,
        "unexpected_paths": unexpected,
        "duplicate_actual_paths": sorted(set(duplicate_actual)),
        "mismatches": mismatches,
        "pass": not missing
        and not unexpected
        and not duplicate_actual
        and not mismatches
        and len(actual_entries) == len(expected),
    }


def d14_blockers(inventory: Mapping[str, Any]) -> list[str]:
    blockers = [
        "history=true version lineage is unavailable from a current filesystem snapshot",
        (
            "evaluation import stores checkpoint-looking documents as ordinary Markdown; "
            "service parent_checkpoint_id foreign-key resolution is not representable"
        ),
    ]
    unsupported = sum(
        1
        for entry in inventory["entries"]
        if entry.get("classification") == "unsupported_binary"
    )
    if unsupported:
        blockers.append(
            f"{unsupported} binary or unsupported regular files are inventoried but not imported"
        )
    collisions = sum(
        1
        for entry in inventory["entries"]
        if entry.get("classification") == "unsupported_collision"
    )
    if collisions:
        blockers.append(
            f"{collisions} paths collide under workspace case/Unicode normalization"
        )
    lineage = inventory["checkpoint_lineage"]
    if not lineage["source_resolution_pass"]:
        blockers.append("source-level checkpoint references do not all resolve uniquely")
    return blockers


def build_audit(
    inventory: Mapping[str, Any],
    actual_entries: Sequence[Mapping[str, Any]],
    *,
    api_url: str,
    import_metadata: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    fidelity = fidelity_differences(inventory, actual_entries)
    blockers = d14_blockers(inventory)
    return {
        "schema": AUDIT_SCHEMA,
        "created_at": utc_now(),
        "source_inventory_sha256": inventory["inventory_sha256"],
        "target": {
            "kind": "disposable_simplified_core_evaluation_user",
            "api_url": api_url,
            "access_mode": "read_only",
        },
        "supported_text_fidelity": fidelity,
        "checkpoint_lineage": {
            **inventory["checkpoint_lineage"],
            "service_resolution_verified": False,
            "service_resolution_reason": (
                "the evaluation import route does not create checkpoint table rows "
                "or preserve source checkpoint identifiers"
            ),
        },
        "boundaries": inventory["boundaries"],
        "tier_a_assessment": {
            "supported_text_scope": "pass" if fidelity["pass"] else "fail",
            "full_d14_zero_diff_gate": "blocked" if blockers else "pass",
            "blockers": blockers,
        },
        "import": recursively_redact_secrets(dict(import_metadata or {})),
    }


def write_json(path: Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    rendered = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    path.write_text(rendered, encoding="utf-8")


def write_credential(path: Path, *, api_url: str, token: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(
        {
            "schema": CREDENTIAL_SCHEMA,
            "api_url": api_url,
            "token": token,
        },
        separators=(",", ":"),
    ).encode("utf-8")
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError as error:
        raise SnapshotError(f"credential output already exists: {path}") from error
    try:
        os.write(descriptor, payload)
    finally:
        os.close(descriptor)
    os.chmod(path, 0o600)


def load_credential(path: Path) -> tuple[str, str]:
    mode = stat.S_IMODE(path.stat().st_mode)
    if mode & 0o077:
        raise SnapshotError(f"credential file must not be group/world accessible: {path}")
    value = load_json(path)
    if value.get("schema") != CREDENTIAL_SCHEMA:
        raise SnapshotError(f"{path} is not a {CREDENTIAL_SCHEMA} artifact")
    api_url = value.get("api_url")
    token = value.get("token")
    if not isinstance(api_url, str) or not api_url:
        raise SnapshotError("credential file is missing api_url")
    if not isinstance(token, str) or not token:
        raise SnapshotError("credential file is missing token")
    return api_url, token


def import_and_audit(
    inventory: Mapping[str, Any],
    *,
    root: Path,
    api_url: str,
    admin_token: str,
    run_id: str,
    case_id: str,
    batch_size: int,
    credential_out: Path,
    timeout_seconds: float,
) -> dict[str, Any]:
    verify_snapshot_unchanged(inventory, root)
    documents = import_documents(
        inventory,
        root,
        batch_size=batch_size,
    )
    admin = NativeApiClient(
        base_url=api_url,
        token=admin_token,
        run_id=run_id,
        case_id=case_id,
        timeout=timeout_seconds,
    )
    metadata = provision_evaluation(
        admin,
        run_id=run_id,
        case_id=case_id,
        display_scope="owner-vault-current-snapshot",
        access_mode="read_only",
        documents=documents,
        timeout_seconds=timeout_seconds,
        import_path="/v1/workspace/admin/eval/import",
        wait_for_semantic=False,
        batch_size=batch_size,
    )
    token = str(metadata.pop("token"))
    write_credential(credential_out, api_url=api_url, token=token)
    scoped = NativeApiClient(
        base_url=api_url,
        token=token,
        run_id=run_id,
        case_id=case_id,
        timeout=timeout_seconds,
    )
    actual = manifest_entries(scoped)
    return build_audit(
        inventory,
        actual,
        api_url=api_url,
        import_metadata=metadata,
    )


def verify_import(
    inventory: Mapping[str, Any],
    *,
    api_url: str,
    token: str,
    timeout_seconds: float,
) -> dict[str, Any]:
    client = NativeApiClient(
        base_url=api_url,
        token=token,
        run_id="owner-snapshot-verify",
        case_id="owner-snapshot",
        timeout=timeout_seconds,
    )
    return build_audit(
        inventory,
        manifest_entries(client),
        api_url=api_url,
    )


def markdown_documents(
    inventory: Mapping[str, Any],
    root: Path,
) -> dict[str, str]:
    documents: dict[str, str] = {}
    for entry in supported_entries(inventory):
        path = str(entry["path"])
        if not path.casefold().endswith((".md", ".markdown")):
            continue
        payload = read_stable_bytes(root / PurePosixPath(path))
        if len(payload) != entry["size_bytes"] or sha256_bytes(payload) != entry["sha256"]:
            raise SnapshotError(f"source snapshot changed after inventory: {path}")
        documents[path] = payload.decode("utf-8", errors="strict")
    return documents


def wiki_target(raw: str) -> str:
    value = raw.split("|", 1)[0].split("#", 1)[0].split("^", 1)[0].strip()
    return urllib.parse.unquote(value)


def resolve_wiki_target(
    source_path: str,
    raw_target: str,
    *,
    exact_index: Mapping[str, list[str]],
    basename_index: Mapping[str, list[str]],
    stem_index: Mapping[str, list[str]],
) -> tuple[str, list[str]]:
    target = wiki_target(raw_target)
    if not target:
        return "invalid", []
    target_path = PurePosixPath(target)
    explicit_candidates: list[str] = []
    if "/" in target or target.casefold().endswith((".md", ".markdown")):
        with_suffix = target if PurePosixPath(target).suffix else target + ".md"
        source_relative = str(PurePosixPath(source_path).parent / with_suffix)
        explicit_candidates.extend([source_relative, with_suffix])
    else:
        explicit_candidates.extend([target, target + ".md"])
    exact_matches: set[str] = set()
    for candidate in explicit_candidates:
        normalized_parts: list[str] = []
        invalid = False
        for part in PurePosixPath(candidate).parts:
            if part in {"", "."}:
                continue
            if part == "..":
                if not normalized_parts:
                    invalid = True
                    break
                normalized_parts.pop()
            else:
                normalized_parts.append(part)
        if invalid:
            continue
        normalized = "/".join(normalized_parts)
        exact_matches.update(exact_index.get(portable_path_key(normalized), []))
    if len(exact_matches) == 1:
        return "resolved", sorted(exact_matches)
    if len(exact_matches) > 1:
        return "ambiguous", sorted(exact_matches)
    basename = PurePosixPath(target).name
    lookup = (
        basename_index.get(portable_path_key(basename), [])
        if PurePosixPath(basename).suffix
        else stem_index.get(portable_path_key(basename), [])
    )
    unique = sorted(set(lookup))
    if len(unique) == 1:
        return "resolved", unique
    if len(unique) > 1:
        return "ambiguous", unique
    return "unresolved", []


def build_link_candidates(
    inventory: Mapping[str, Any],
    *,
    root: Path,
    min_resolved_links: int = 3,
) -> dict[str, Any]:
    if min_resolved_links <= 0:
        raise SnapshotError("minimum resolved-link count must be positive")
    documents = markdown_documents(inventory, root)
    paths = sorted(documents)
    exact_index: dict[str, list[str]] = defaultdict(list)
    basename_index: dict[str, list[str]] = defaultdict(list)
    stem_index: dict[str, list[str]] = defaultdict(list)
    for path in paths:
        pure = PurePosixPath(path)
        exact_index[portable_path_key(path)].append(path)
        basename_index[portable_path_key(pure.name)].append(path)
        stem_index[portable_path_key(pure.stem)].append(path)
    notes: list[dict[str, Any]] = []
    resolution_totals = defaultdict(int)
    for source_path, content in sorted(documents.items()):
        resolved: set[str] = set()
        unresolved_hashes: set[str] = set()
        ambiguous_hashes: set[str] = set()
        raw_links = WIKI_LINK_RE.findall(content)
        for raw in raw_links:
            status, matches = resolve_wiki_target(
                source_path,
                raw,
                exact_index=exact_index,
                basename_index=basename_index,
                stem_index=stem_index,
            )
            resolution_totals[status] += 1
            if status == "resolved":
                resolved.update(matches)
            elif status == "ambiguous":
                ambiguous_hashes.add(sha256_bytes(wiki_target(raw).encode("utf-8")))
            elif status in {"unresolved", "invalid"}:
                unresolved_hashes.add(sha256_bytes(wiki_target(raw).encode("utf-8")))
        resolved.discard(source_path)
        if len(resolved) >= min_resolved_links:
            notes.append(
                {
                    "path": source_path,
                    "resolved_link_count": len(resolved),
                    "resolved_paths": sorted(resolved),
                    "raw_wiki_link_count": len(raw_links),
                    "unresolved_target_hashes": sorted(unresolved_hashes),
                    "ambiguous_target_hashes": sorted(ambiguous_hashes),
                }
            )
    notes.sort(key=lambda item: (-item["resolved_link_count"], item["path"]))
    return {
        "schema": LINK_CANDIDATES_SCHEMA,
        "created_at": utc_now(),
        "source_inventory_sha256": inventory["inventory_sha256"],
        "selection_policy": {
            "minimum_unique_resolved_outgoing_wiki_links": min_resolved_links,
            "resolution_order": (
                "source-relative exact, vault-root exact, then unique basename/stem"
            ),
            "content_or_excerpts_in_artifact": False,
            "questions_or_rubric_answers_in_artifact": False,
        },
        "counts": {
            "markdown_notes": len(documents),
            "candidate_notes": len(notes),
            "raw_link_resolution": dict(sorted(resolution_totals.items())),
        },
        "candidates": notes,
        "next_human_gate": (
            "owner authors final multi-note questions and rubric claims outside the vault, "
            "then signs off before draw 1"
        ),
    }


def build_leak_report(
    inventory: Mapping[str, Any],
    *,
    root: Path,
    request: Mapping[str, Any],
) -> dict[str, Any]:
    if request.get("schema") != LEAK_REQUEST_SCHEMA:
        raise SnapshotError(f"leak request must use {LEAK_REQUEST_SCHEMA}")
    checks = request.get("checks")
    if not isinstance(checks, list) or not checks:
        raise SnapshotError("leak request requires a non-empty checks array")
    documents: dict[str, str] = {}
    for entry in supported_entries(inventory):
        path = str(entry["path"])
        payload = read_stable_bytes(root / PurePosixPath(path))
        if len(payload) != entry["size_bytes"] or sha256_bytes(payload) != entry["sha256"]:
            raise SnapshotError(f"source snapshot changed after inventory: {path}")
        documents[path] = payload.decode("utf-8", errors="strict")
    records: list[dict[str, Any]] = []
    seen_ids: set[tuple[str, str]] = set()
    for item in checks:
        if not isinstance(item, dict):
            raise SnapshotError("every leak check must be an object")
        case_id = item.get("case_id")
        claim_id = item.get("claim_id")
        text = item.get("text")
        if not all(isinstance(value, str) and value for value in (case_id, claim_id, text)):
            raise SnapshotError(
                "every leak check requires non-empty case_id, claim_id, and text"
            )
        key = (case_id, claim_id)
        if key in seen_ids:
            raise SnapshotError(f"duplicate leak check identity: {case_id}/{claim_id}")
        seen_ids.add(key)
        matches = sorted(path for path, content in documents.items() if text in content)
        records.append(
            {
                "case_id": case_id,
                "claim_id": claim_id,
                "claim_sha256": sha256_bytes(text.encode("utf-8")),
                "verbatim_match": bool(matches),
                "matching_paths": matches,
            }
        )
    return {
        "schema": LEAK_REPORT_SCHEMA,
        "created_at": utc_now(),
        "source_inventory_sha256": inventory["inventory_sha256"],
        "comparison": "case-sensitive exact Unicode substring",
        "claims_echoed": False,
        "checks": records,
        "summary": {
            "checks": len(records),
            "leaks": sum(1 for record in records if record["verbatim_match"]),
            "pass": not any(record["verbatim_match"] for record in records),
        },
    }


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Inventory, import, and audit an owner-vault current snapshot.",
    )
    subparsers = value.add_subparsers(dest="command", required=True)

    inventory = subparsers.add_parser("inventory")
    inventory.add_argument("--root", type=Path, required=True)
    inventory.add_argument("--out", type=Path, required=True)

    candidates = subparsers.add_parser("link-candidates")
    candidates.add_argument("--inventory", type=Path, required=True)
    candidates.add_argument("--root", type=Path)
    candidates.add_argument("--min-resolved-links", type=int, default=3)
    candidates.add_argument("--out", type=Path, required=True)

    leak = subparsers.add_parser("leak-check")
    leak.add_argument("--inventory", type=Path, required=True)
    leak.add_argument("--root", type=Path)
    leak.add_argument("--request", type=Path, required=True)
    leak.add_argument("--out", type=Path, required=True)

    importer = subparsers.add_parser("import")
    importer.add_argument("--inventory", type=Path, required=True)
    importer.add_argument("--root", type=Path)
    importer.add_argument(
        "--api-url",
        default=os.environ.get("STRAYLIGHT_API_URL"),
    )
    importer.add_argument(
        "--admin-token-env",
        default="STRAYLIGHT_EVAL_TOKEN",
    )
    importer.add_argument("--run-id", required=True)
    importer.add_argument("--case-id", default="owner-snapshot")
    importer.add_argument("--batch-size", type=int, default=DEFAULT_BATCH_SIZE)
    importer.add_argument("--credential-out", type=Path, required=True)
    importer.add_argument("--timeout", type=float, default=7_200.0)
    importer.add_argument("--out", type=Path, required=True)

    verifier = subparsers.add_parser("verify")
    verifier.add_argument("--inventory", type=Path, required=True)
    verifier.add_argument("--credential", type=Path, required=True)
    verifier.add_argument("--timeout", type=float, default=300.0)
    verifier.add_argument("--out", type=Path, required=True)
    return value


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "inventory":
            artifact = build_inventory(args.root)
        elif args.command == "link-candidates":
            inventory = load_inventory(args.inventory)
            artifact = build_link_candidates(
                inventory,
                root=snapshot_root(inventory, args.root),
                min_resolved_links=args.min_resolved_links,
            )
        elif args.command == "leak-check":
            inventory = load_inventory(args.inventory)
            artifact = build_leak_report(
                inventory,
                root=snapshot_root(inventory, args.root),
                request=load_json(args.request),
            )
        elif args.command == "import":
            inventory = load_inventory(args.inventory)
            if not args.api_url:
                raise SnapshotError("--api-url or STRAYLIGHT_API_URL is required")
            admin_token = os.environ.get(args.admin_token_env, "")
            if not admin_token:
                raise SnapshotError(
                    f"administrative token environment variable is empty: "
                    f"{args.admin_token_env}"
                )
            artifact = import_and_audit(
                inventory,
                root=snapshot_root(inventory, args.root),
                api_url=args.api_url.rstrip("/"),
                admin_token=admin_token,
                run_id=args.run_id,
                case_id=args.case_id,
                batch_size=args.batch_size,
                credential_out=args.credential_out,
                timeout_seconds=args.timeout,
            )
        elif args.command == "verify":
            inventory = load_inventory(args.inventory)
            api_url, token = load_credential(args.credential)
            artifact = verify_import(
                inventory,
                api_url=api_url,
                token=token,
                timeout_seconds=args.timeout,
            )
        else:
            raise AssertionError(args.command)
        write_json(args.out, artifact)
        if args.command in {"import", "verify"}:
            return 0 if artifact["supported_text_fidelity"]["pass"] else 2
        if args.command == "leak-check":
            return 0 if artifact["summary"]["pass"] else 2
        return 0
    except (OSError, SnapshotError, ValueError) as error:
        print(f"owner snapshot error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

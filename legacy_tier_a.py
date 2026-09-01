#!/usr/bin/env python3

"""Fail-closed legacy Tier-A export recovery, replay, and fidelity audit.

The hosted legacy vault export is split into three already-verified inputs:

* a complete current portable export,
* a ``history=true`` manifest,
* a small exact-byte delta containing inactive versions and current drift.

This tool composes those inputs without downloading or regenerating content,
materializes bounded replay stages for the simplified workspace importer, and
audits the resulting service history.  Legacy binary descriptions are paired
to their binary and must be uploaded through the exact portable-companion
contract; no description model or embedding provider is used.

Owner paths and record payloads are private.  Live artifacts belong under the
ignored ``operator-output/`` tree.  Tests use synthetic fixtures only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import tempfile
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
import uuid
from collections import Counter, defaultdict
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any, Iterable, Iterator, Mapping, Sequence


COMPOSITE_SCHEMA = "brunn-tier-a-legacy-composite@v1"
AUDIT_SCHEMA = "brunn-tier-a-legacy-audit@v1"
CHECKPOINT_IMPORT_SCHEMA = "brunn-tier-a-checkpoint-import@v1"
CREDENTIAL_SCHEMA = "brunn-tier-a-isolated-credential@v1"
LEGACY_MANIFEST_FORMAT = "brunn-state-vault-manifest@v1"
LEGACY_CURRENT_FORMAT = "brunn-state-portable-vault@v1"
LEGACY_DELTA_FORMAT = "brunn-legacy-vault-delta@v1"
LEGACY_NATIVE_FORMAT = "brunn-legacy-native-record-export@v1"
WORKSPACE_EXPORT_FORMAT = "brunn-workspace-export@v1"
WORKSPACE_IMPORT_FORMAT = "brunn-workspace-import-manifest@v1"
PORTABLE_COMPANION_FORMAT = "brunn-tier-a-portable-companion@v1"
TIER_A_HISTORY_STAGE_FORMAT = "brunn-tier-a-history-stage@v1"
ORDINARY_HISTORY_SEMANTICS = "ordinary_content_transition"
EXACT_HISTORY_SEMANTICS = "preserve_intentional_exact_bytes_version"

NATIVE_ARCHIVE_PATH = "Brunn Migration/Native/legacy-native-records.json"
NATIVE_INDEX_PATH = "Brunn Migration/Native/legacy-native-records.index.json"
NATIVE_DESCRIPTION_PATH = (
    "Brunn Migration/Native/legacy-native-records.description.md"
)
NATIVE_MATERIALIZED_ARCHIVE_PREFIX = "native/materialized"

SHA256_RE = re.compile(r"^(?:sha256:)?([0-9a-fA-F]{64})$")
UUID_REF_RE = re.compile(
    r"^(?:(?P<kind>[a-z_]+):)?"
    r"(?P<uuid>[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-"
    r"[89aAbB][0-9a-fA-F]{3}-[0-9a-fA-F]{12})$"
)
HISTORY_ARCHIVE_RE = re.compile(
    r"^history/(?P<path>.+)/v(?P<version>[1-9][0-9]*)-"
    r"(?P<hash>[0-9a-f]{16})$"
)
SUPPORTED_NATIVE_KINDS = {
    "checkpoint",
    "claim",
    "evidence",
    "object",
    "relation",
    "recurrence_spec",
    "temporal_spec",
}
IMPORT_BINARY_EXTENSIONS = {
    ".bz2",
    ".db",
    ".gif",
    ".gz",
    ".heic",
    ".jpeg",
    ".jpg",
    ".parquet",
    ".pdf",
    ".png",
    ".sqlite",
    ".webp",
    ".woff",
    ".woff2",
    ".xz",
    ".zip",
}
IMPORT_BINARY_SIGNATURES = (
    b"\x89PNG\r\n\x1a\n",
    b"\xff\xd8\xff",
    b"GIF87a",
    b"GIF89a",
    b"%PDF-",
    b"PK\x03\x04",
    b"PK\x05\x06",
    b"\x1f\x8b",
    b"SQLite format 3\x00",
    b"PAR1",
)


class TierAError(RuntimeError):
    """A deterministic preflight, composition, import, or fidelity failure."""


def utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


def pretty_json(value: Any) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode(
        "utf-8"
    )


def sha256_hex(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def sha256_value(payload: bytes) -> str:
    return "sha256:" + sha256_hex(payload)


def normalize_sha256(value: Any, field: str = "sha256") -> str:
    match = SHA256_RE.fullmatch(str(value or "").strip())
    if match is None:
        raise TierAError(f"{field} is not a SHA-256 value")
    return "sha256:" + match.group(1).lower()


def load_json_object(path: Path) -> dict[str, Any]:
    try:
        payload = path.read_bytes()
        value = json.loads(payload)
    except (OSError, json.JSONDecodeError) as error:
        raise TierAError(f"could not read JSON object from {path}: {error}") from error
    if not isinstance(value, dict):
        raise TierAError(f"{path} must contain a JSON object")
    return value


def safe_relative_path(value: Any, *, field: str = "path") -> str:
    path = str(value or "")
    if (
        not path
        or len(path) > 4_096
        or path.startswith("/")
        or "\\" in path
        or any(unicodedata.category(character).startswith("C") for character in path)
        or any(part in {"", ".", ".."} for part in path.split("/"))
    ):
        raise TierAError(f"{field} is not a safe portable relative path")
    return path


def portable_path_key(path: str) -> str:
    return unicodedata.normalize("NFC", path).casefold()


def safe_file(root: Path, relative: str) -> Path:
    relative = safe_relative_path(relative)
    path = root.joinpath(*PurePosixPath(relative).parts)
    try:
        metadata = path.lstat()
    except OSError as error:
        raise TierAError(f"portable artifact is missing {relative}: {error}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise TierAError(f"portable artifact is not a regular non-symlink file: {relative}")
    resolved_root = root.resolve(strict=True)
    resolved = path.resolve(strict=True)
    if not resolved.is_relative_to(resolved_root):
        raise TierAError(f"portable artifact escaped its root: {relative}")
    return path


def hash_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return "sha256:" + digest.hexdigest(), size


def raw_manifest_sha256(path: Path) -> str:
    return sha256_value(path.read_bytes())


def parse_checksum_ledger(root: Path) -> dict[str, str]:
    ledger_path = safe_file(root, "CHECKSUMS.sha256")
    try:
        lines = ledger_path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise TierAError(f"could not read {ledger_path}: {error}") from error
    entries: dict[str, str] = {}
    prior: str | None = None
    for line in lines:
        if not line or "  " not in line:
            raise TierAError("CHECKSUMS.sha256 has an invalid line")
        digest, relative = line.split("  ", 1)
        relative = safe_relative_path(relative, field="checksum path")
        normalized = normalize_sha256(digest, "checksum")
        if relative in entries:
            raise TierAError(f"CHECKSUMS.sha256 repeats {relative}")
        if prior is not None and relative <= prior:
            raise TierAError("CHECKSUMS.sha256 must be strictly path-sorted")
        prior = relative
        entries[relative] = normalized
    if not entries:
        raise TierAError("CHECKSUMS.sha256 is empty")
    return entries


def verify_checksum_tree(root: Path) -> dict[str, str]:
    root = root.resolve(strict=True)
    if not root.is_dir():
        raise TierAError(f"portable artifact root is not a directory: {root}")
    ledger = parse_checksum_ledger(root)
    observed: set[str] = set()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode):
            raise TierAError(f"portable artifact contains a symlink: {relative}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise TierAError(f"portable artifact contains a special file: {relative}")
        if relative == "CHECKSUMS.sha256":
            continue
        observed.add(relative)
    expected = set(ledger)
    missing = sorted(expected - observed)
    unexpected = sorted(observed - expected)
    if missing or unexpected:
        raise TierAError(
            "portable checksum coverage differs "
            f"(missing={len(missing)}, unexpected={len(unexpected)})"
        )
    for relative, expected_hash in ledger.items():
        actual_hash, _ = hash_file(safe_file(root, relative))
        if actual_hash != expected_hash:
            raise TierAError(f"portable checksum mismatch: {relative}")
    return ledger


def write_private(path: Path, payload: bytes, *, overwrite: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    flags = os.O_WRONLY | os.O_CREAT | (os.O_TRUNC if overwrite else os.O_EXCL)
    descriptor = os.open(path, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        try:
            path.unlink()
        except OSError:
            pass
        raise


def write_json_private(path: Path, value: Any, *, overwrite: bool = False) -> None:
    write_private(path, pretty_json(value), overwrite=overwrite)


def hardlink_new(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if destination.exists() or destination.is_symlink():
        raise TierAError(f"refusing to overwrite {destination}")
    source_metadata = source.lstat()
    if stat.S_ISLNK(source_metadata.st_mode) or not stat.S_ISREG(source_metadata.st_mode):
        raise TierAError(f"hardlink source is not a regular file: {source}")
    try:
        os.link(source, destination, follow_symlinks=False)
    except OSError as error:
        raise TierAError(
            "portable composition requires same-filesystem hardlinks; "
            f"could not link {source} to {destination}: {error}"
        ) from error


def write_checksums(root: Path, *, excluded: Iterable[str] = ()) -> None:
    excluded_set = set(excluded) | {"CHECKSUMS.sha256"}
    paths = [
        path
        for path in root.rglob("*")
        if not path.is_dir()
        and path.relative_to(root).as_posix() not in excluded_set
    ]
    paths.sort(key=lambda path: path.relative_to(root).as_posix())
    rows: list[tuple[str, str]] = []
    for path in paths:
        relative = path.relative_to(root).as_posix()
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise TierAError(f"refusing to checksum non-regular path {relative}")
        digest, _ = hash_file(path)
        rows.append((relative, digest.removeprefix("sha256:")))
    payload = "".join(f"{digest}  {relative}\n" for relative, digest in rows).encode()
    write_private(root / "CHECKSUMS.sha256", payload)


def require_format(value: Mapping[str, Any], expected: str, path: Path) -> None:
    actual = value.get("format") or value.get("schema")
    if actual != expected:
        raise TierAError(f"{path} is {actual!r}, expected {expected!r}")


def require_entries(value: Mapping[str, Any], path: Path) -> list[dict[str, Any]]:
    entries = value.get("entries")
    if not isinstance(entries, list) or any(not isinstance(item, dict) for item in entries):
        raise TierAError(f"{path} has no valid entries array")
    return [dict(item) for item in entries]


def current_entry_identity(entry: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "path": safe_relative_path(entry.get("path")),
        "asset_ref": str(entry.get("asset_ref") or ""),
        "asset_version": int(entry.get("asset_version") or entry.get("version") or 0),
        "content_hash": normalize_sha256(entry.get("content_hash"), "content_hash"),
        "size_bytes": int(entry.get("size_bytes")),
        "media_type": str(entry.get("media_type") or ""),
        "source_kind": str(entry.get("source_kind") or ""),
        "source_ref": str(entry.get("source_ref") or ""),
    }


def legacy_portable_path(entry: Mapping[str, Any]) -> str:
    source_path = safe_relative_path(entry.get("path"), field="legacy source path")
    if entry.get("category") == "generated_description":
        described_path = safe_relative_path(
            entry.get("original_path") or source_path,
            field="legacy description original_path",
        )
        return safe_relative_path(f"workspace/assets/{described_path}.md")
    return safe_relative_path(f"sources/{source_path}")


def history_entry_identity(entry: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "path": legacy_portable_path(entry),
        "asset_ref": str(entry.get("asset_ref") or ""),
        "asset_version": int(entry.get("version") or 0),
        "content_hash": normalize_sha256(entry.get("content_hash"), "content_hash"),
        "size_bytes": int(entry.get("size_bytes")),
        "media_type": str(entry.get("media_type") or ""),
        "source_kind": str(entry.get("source_kind") or ""),
        "source_ref": str(entry.get("source_ref") or ""),
    }


def delta_logical_path(entry: Mapping[str, Any]) -> str:
    path = safe_relative_path(entry.get("path"))
    if entry.get("reason") == "active_drift":
        return path
    if entry.get("reason") != "inactive_history":
        raise TierAError(f"unsupported delta reason for {path}")
    match = HISTORY_ARCHIVE_RE.fullmatch(path)
    if match is None:
        raise TierAError("inactive history delta path does not encode its logical path")
    expected_hash = normalize_sha256(entry.get("content_hash")).removeprefix("sha256:")[:16]
    if match.group("hash") != expected_hash:
        raise TierAError("inactive history archive suffix does not match its content hash")
    if int(match.group("version")) != int(entry.get("asset_version") or 0):
        raise TierAError("inactive history archive suffix does not match its asset version")
    return safe_relative_path(match.group("path"))


def exact_content_source(
    *,
    history_entry: Mapping[str, Any],
    base_root: Path,
    base_by_path: Mapping[str, Mapping[str, Any]],
    delta_root: Path,
    delta_by_identity: Mapping[tuple[str, str, str], Mapping[str, Any]],
) -> tuple[Path, dict[str, Any], str]:
    identity = history_entry_identity(history_entry)
    logical_path = identity["path"]
    content_hash = identity["content_hash"]
    active = bool(history_entry.get("active"))
    key = (logical_path, content_hash, "active_drift" if active else "inactive_history")
    if key in delta_by_identity:
        delta_entry = delta_by_identity[key]
        archive_path = safe_relative_path(delta_entry.get("path"))
        return (
            safe_file(delta_root, archive_path),
            {
                "modified_unix_ns": delta_entry.get("modified_unix_ns"),
                "mode": delta_entry.get("mode"),
            },
            "delta",
        )
    if not active:
        raise TierAError(
            "the exact delta omits an inactive history version; refusing partial lineage"
        )
    base_entry = base_by_path.get(logical_path)
    if base_entry is None:
        raise TierAError("the current portable export omits an active history path")
    if current_entry_identity(base_entry) != identity:
        raise TierAError(
            "an active history row differs from both the current export and exact delta"
        )
    return (
        safe_file(base_root, logical_path),
        {
            "modified_unix_ns": base_entry.get("modified_unix_ns"),
            "mode": base_entry.get("mode"),
        },
        "base_current",
    )


def validate_content(path: Path, entry: Mapping[str, Any]) -> None:
    content_hash, size_bytes = hash_file(path)
    expected_hash = normalize_sha256(entry.get("content_hash"), "content_hash")
    expected_size = int(entry.get("size_bytes"))
    if content_hash != expected_hash or size_bytes != expected_size:
        raise TierAError(
            "portable bytes do not match their manifest identity "
            f"(hash_match={content_hash == expected_hash}, "
            f"size_match={size_bytes == expected_size})"
        )


def importer_kind(path: str, source: Path, size_bytes: int) -> str:
    """Mirror brunn_state_import's bounded UTF-8/binary classification."""

    if size_bytes > 4 * 1024 * 1024:
        return "binary"
    payload = source.read_bytes()
    if len(payload) != size_bytes:
        raise TierAError("portable source changed during import classification")
    if b"\x00" in payload:
        return "binary"
    try:
        payload.decode("utf-8", errors="strict")
    except UnicodeDecodeError:
        return "binary"
    if any(payload.startswith(signature) for signature in IMPORT_BINARY_SIGNATURES):
        return "binary"
    if PurePosixPath(path).suffix.casefold() in IMPORT_BINARY_EXTENSIONS:
        return "binary"
    return "markdown"


def native_checkpoint_plan(record: Mapping[str, Any]) -> dict[str, Any]:
    record_ref = str(record.get("record_ref") or "")
    match = UUID_REF_RE.fullmatch(record_ref)
    if match is None or match.group("kind") != "checkpoint":
        raise TierAError("native checkpoint record_ref is invalid")
    checkpoint_uuid = str(uuid.UUID(match.group("uuid")))
    payload = record.get("payload")
    if not isinstance(payload, dict) or not isinstance(payload.get("record"), dict):
        raise TierAError("native checkpoint payload omits its record")
    checkpoint = payload["record"]
    payload_id = str(checkpoint.get("id") or "")
    if payload_id != checkpoint_uuid:
        raise TierAError("native checkpoint payload ID differs from record_ref")
    parent_id = checkpoint.get("parent_checkpoint_id")
    parent_ref: str | None
    if parent_id is None:
        parent_ref = None
    else:
        try:
            parent_ref = "checkpoint:" + str(uuid.UUID(str(parent_id)))
        except ValueError as error:
            raise TierAError("native checkpoint has an invalid parent ID") from error
    summary = checkpoint.get("summary")
    state: Any = summary
    if isinstance(summary, str):
        try:
            state = json.loads(summary)
        except json.JSONDecodeError:
            state = {"legacy_summary": summary}
    if not isinstance(state, dict):
        state = {"legacy_summary": state}
    source_refs = []
    for item in payload.get("source_refs") or []:
        if not isinstance(item, dict):
            raise TierAError("native checkpoint source_refs contains a non-object")
        locator = item.get("locator")
        source_ref = locator.get("ref") if isinstance(locator, dict) else None
        if isinstance(source_ref, str) and source_ref:
            source_refs.append(source_ref)
    content = (
        "---\n"
        "brunn_kind: checkpoint\n"
        f"checkpoint_id: checkpoint:{checkpoint_uuid}\n"
        f"parent_checkpoint_id: {parent_ref or ''}\n"
        f"legacy_corpus_revision: {checkpoint.get('corpus_revision_id') or ''}\n"
        "---\n\n"
        f"# {str(record.get('title') or 'Imported legacy checkpoint').strip()}\n\n"
        "## Legacy state\n\n```json\n"
        + json.dumps(state, ensure_ascii=False, indent=2, sort_keys=True)
        + "\n```\n"
    )
    metadata = {
        "kind": "checkpoint",
        "checkpoint_ref": f"checkpoint:{checkpoint_uuid}",
        "parent_checkpoint_ref": parent_ref,
        "legacy_record_ref": record_ref,
        "legacy_corpus_revision": checkpoint.get("corpus_revision_id"),
        "legacy_session_id": checkpoint.get("session_id"),
        "legacy_source_refs": source_refs,
        "source_entries": [],
        "workspace_generation": 0,
        "_brunn_import": {"format": WORKSPACE_IMPORT_FORMAT},
    }
    return {
        "record_ref": record_ref,
        "checkpoint_ref": f"checkpoint:{checkpoint_uuid}",
        "parent_checkpoint_ref": parent_ref,
        "path": f".brunn/checkpoints/{checkpoint_uuid}.md",
        "content": content,
        "content_hash": sha256_value(content.encode("utf-8")),
        "size_bytes": len(content.encode("utf-8")),
        "metadata": metadata,
    }


def validate_checkpoint_graph(plans: Sequence[Mapping[str, Any]]) -> list[dict[str, Any]]:
    by_ref: dict[str, dict[str, Any]] = {}
    for raw in plans:
        plan = dict(raw)
        reference = str(plan["checkpoint_ref"])
        if reference in by_ref:
            raise TierAError("native records repeat a checkpoint reference")
        by_ref[reference] = plan
    for plan in by_ref.values():
        parent = plan.get("parent_checkpoint_ref")
        if parent is not None and parent not in by_ref:
            raise TierAError("native checkpoint parent is absent from the native-record capture")
        if parent == plan["checkpoint_ref"]:
            raise TierAError("native checkpoint cannot parent itself")
    ordered: list[dict[str, Any]] = []
    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(reference: str) -> None:
        if reference in visited:
            return
        if reference in visiting:
            raise TierAError("native checkpoint graph contains a cycle")
        visiting.add(reference)
        parent = by_ref[reference].get("parent_checkpoint_ref")
        if isinstance(parent, str):
            visit(parent)
        visiting.remove(reference)
        visited.add(reference)
        ordered.append(by_ref[reference])

    for reference in sorted(by_ref):
        visit(reference)
    return ordered


def longest_backtick_run(value: str) -> int:
    longest = 0
    current = 0
    for character in value:
        if character == "`":
            current += 1
            longest = max(longest, current)
        else:
            current = 0
    return longest


def render_native_record(
    record: Mapping[str, Any],
    *,
    corpus_revision: str,
) -> bytes:
    """Reproduce the deterministic Markdown emitted by the legacy exporter."""

    payload = record.get("payload")
    if not isinstance(payload, dict):
        raise TierAError("native record payload is not a JSON object")
    payload_text = json.dumps(
        payload,
        ensure_ascii=False,
        indent=2,
        sort_keys=True,
    )
    fence = "`" * max(3, longest_backtick_run(payload_text) + 1)
    title = str(record.get("title") or "").replace("\r", " ").replace("\n", " ")[:240]
    version = record.get("record_version")
    if version is not None and (isinstance(version, bool) or not isinstance(version, int)):
        raise TierAError("native record version is neither an integer nor null")
    version_text = "null" if version is None else str(version)
    return (
        "---\n"
        "brunn_state_format: native-record@v1\n"
        f'record_ref: "{record["record_ref"]}"\n'
        f'record_kind: "{record["record_kind"]}"\n'
        f"record_version: {version_text}\n"
        f'corpus_revision: "{corpus_revision}"\n'
        "---\n"
        f"# {title or 'Untitled record'}\n\n"
        "This is a deterministic export of the complete structured Brunn State "
        "record at the pinned corpus revision.\n\n"
        f"{fence}json\n{payload_text}\n{fence}\n"
    ).encode("utf-8")


def native_materialization(
    native_path: Path,
    *,
    expected_stable_import_id: str,
    output_root: Path,
) -> dict[str, Any]:
    value = load_json_object(native_path)
    require_format(value, LEGACY_NATIVE_FORMAT, native_path)
    if value.get("stable_import_id") != expected_stable_import_id:
        raise TierAError("native records stable_import_id differs from the vault manifest")
    records = value.get("records")
    if not isinstance(records, list) or any(not isinstance(item, dict) for item in records):
        raise TierAError("native record capture has no valid records array")
    references: set[str] = set()
    index_rows: list[dict[str, Any]] = []
    materialized: list[dict[str, Any]] = []
    checkpoints: list[dict[str, Any]] = []
    kinds: Counter[str] = Counter()
    corpus_revision = str(value.get("corpus_revision") or "")
    if not corpus_revision:
        raise TierAError("native record capture omits its corpus revision")
    for record in records:
        reference = str(record.get("record_ref") or "")
        kind = str(record.get("record_kind") or "")
        if kind not in SUPPORTED_NATIVE_KINDS:
            raise TierAError(f"unsupported native record kind: {kind!r}")
        match = UUID_REF_RE.fullmatch(reference)
        if match is None or match.group("kind") != kind:
            raise TierAError("native record reference does not match its kind")
        if reference in references:
            raise TierAError("native record capture repeats a record reference")
        references.add(reference)
        kinds[kind] += 1
        encoded = canonical_json(record)
        record_uuid = str(uuid.UUID(match.group("uuid")))
        logical_path = f"memory/{kind}/{record_uuid}.md"
        archive_path = f"{NATIVE_MATERIALIZED_ARCHIVE_PREFIX}/{logical_path}"
        rendered = render_native_record(record, corpus_revision=corpus_revision)
        write_private(
            output_root.joinpath(*PurePosixPath(archive_path).parts),
            rendered,
        )
        materialized_entry = {
            "path": logical_path,
            "archive_path": archive_path,
            "record_ref": reference,
            "record_kind": kind,
            "record_version": record.get("record_version"),
            "content_hash": sha256_value(rendered),
            "size_bytes": len(rendered),
            "media_type": "text/markdown",
            "kind": "markdown",
        }
        materialized.append(materialized_entry)
        index_rows.append(
            {
                "path": logical_path,
                "record_ref": reference,
                "record_kind": kind,
                "record_version": record.get("record_version"),
                "record_sha256": sha256_value(encoded),
                "materialized_content_hash": materialized_entry["content_hash"],
                "materialized_size_bytes": materialized_entry["size_bytes"],
            }
        )
        if kind == "checkpoint":
            checkpoints.append(native_checkpoint_plan(record))
    ordered_checkpoints = validate_checkpoint_graph(checkpoints)
    archive_hash, archive_size = hash_file(native_path)
    archive_destination = output_root.joinpath(*PurePosixPath(NATIVE_ARCHIVE_PATH).parts)
    hardlink_new(native_path, archive_destination)
    index = {
        "schema": "brunn-tier-a-native-record-index@v1",
        "source_schema": LEGACY_NATIVE_FORMAT,
        "source_sha256": archive_hash,
        "stable_import_id": value["stable_import_id"],
        "corpus_revision": value.get("corpus_revision"),
        "record_count": len(records),
        "record_kind_counts": dict(sorted(kinds.items())),
        "records": sorted(index_rows, key=lambda item: str(item["record_ref"])),
    }
    index_bytes = pretty_json(index)
    index_destination = output_root.joinpath(*PurePosixPath(NATIVE_INDEX_PATH).parts)
    write_private(index_destination, index_bytes)
    description = (
        "# Legacy native-record archive\n\n"
        "Exact byte-copied legacy native records retained for migration audit. "
        "The JSON archive is authoritative; this deterministic companion does "
        "not reinterpret record payloads.\n"
    ).encode("utf-8")
    description_destination = output_root.joinpath(
        *PurePosixPath(NATIVE_DESCRIPTION_PATH).parts
    )
    write_private(description_destination, description)
    return {
        "archive": {
            "path": NATIVE_ARCHIVE_PATH,
            "content_hash": archive_hash,
            "size_bytes": archive_size,
            "media_type": "application/json",
            "kind": "binary",
            "description_path": NATIVE_DESCRIPTION_PATH,
            "description_hash": sha256_value(description),
            "description_size_bytes": len(description),
        },
        "index": {
            "path": NATIVE_INDEX_PATH,
            "content_hash": sha256_value(index_bytes),
            "size_bytes": len(index_bytes),
            "media_type": "text/plain",
            "kind": "markdown",
        },
        "records": {
            "count": len(records),
            "kind_counts": dict(sorted(kinds.items())),
            "capture_sha256": archive_hash,
        },
        "materialized": sorted(materialized, key=lambda item: str(item["path"])),
        "checkpoints": ordered_checkpoints,
    }


def compose(
    *,
    current_root: Path,
    history_manifest_path: Path,
    delta_root: Path,
    native_records_path: Path,
    output: Path,
) -> dict[str, Any]:
    if output.exists():
        raise TierAError(f"refusing to overwrite {output}")
    current_root = current_root.resolve(strict=True)
    delta_root = delta_root.resolve(strict=True)
    verify_checksum_tree(current_root)
    verify_checksum_tree(delta_root)
    current_manifest_path = safe_file(current_root, "manifest.json")
    delta_manifest_path = safe_file(delta_root, "manifest.json")
    current = load_json_object(current_manifest_path)
    history = load_json_object(history_manifest_path)
    delta = load_json_object(delta_manifest_path)
    require_format(current, LEGACY_CURRENT_FORMAT, current_manifest_path)
    require_format(history, LEGACY_MANIFEST_FORMAT, history_manifest_path)
    require_format(delta, LEGACY_DELTA_FORMAT, delta_manifest_path)
    if history.get("include_history") is not True:
        raise TierAError("legacy source manifest is not history=true")
    if history.get("stable_import_id") != delta.get("stable_import_id"):
        raise TierAError("history and delta stable_import_id values differ")
    if raw_manifest_sha256(history_manifest_path) != normalize_sha256(
        delta.get("source_manifest_sha256"), "source_manifest_sha256"
    ):
        raise TierAError("history manifest differs from the manifest pinned by the delta")
    if raw_manifest_sha256(current_manifest_path) != normalize_sha256(
        delta.get("base_current_manifest_sha256"), "base_current_manifest_sha256"
    ):
        raise TierAError("current manifest differs from the manifest pinned by the delta")

    current_entries = require_entries(current, current_manifest_path)
    history_entries = require_entries(history, history_manifest_path)
    delta_entries = require_entries(delta, delta_manifest_path)
    if len(current_entries) != len({portable_path_key(str(item.get("path"))) for item in current_entries}):
        raise TierAError("current portable export has a path collision")
    base_by_path = {
        safe_relative_path(entry.get("path")): entry for entry in current_entries
    }
    delta_by_identity: dict[tuple[str, str, str], dict[str, Any]] = {}
    for entry in delta_entries:
        reason = str(entry.get("reason") or "")
        logical_path = delta_logical_path(entry)
        key = (
            logical_path,
            normalize_sha256(entry.get("content_hash"), "content_hash"),
            reason,
        )
        if key in delta_by_identity:
            raise TierAError("delta repeats a logical-path/content/reason identity")
        delta_by_identity[key] = entry

    output.mkdir(parents=True, mode=0o700)
    output.chmod(0o700)
    provisional: list[dict[str, Any]] = []
    logical_collision: dict[str, str] = {}
    for manifest_ordinal, entry in enumerate(history_entries, start=1):
        identity = history_entry_identity(entry)
        path_key = portable_path_key(identity["path"])
        previous = logical_collision.setdefault(path_key, identity["path"])
        if previous != identity["path"]:
            raise TierAError("history manifest has a portable logical-path collision")
        source, portable, source_tree = exact_content_source(
            history_entry=entry,
            base_root=current_root,
            base_by_path=base_by_path,
            delta_root=delta_root,
            delta_by_identity=delta_by_identity,
        )
        validate_content(source, entry)
        provisional.append(
            {
                **identity,
                "legacy_manifest_ordinal": manifest_ordinal,
                "active": bool(entry.get("active")),
                "category": str(entry.get("category") or ""),
                "original_path": entry.get("original_path"),
                "captured_at": entry.get("captured_at"),
                "stored_at": entry.get("stored_at"),
                "previous_asset_version": entry.get("previous_version"),
                "description_status": entry.get("description_status"),
                "description_method": entry.get("description_method"),
                "portable": portable,
                "workspace_kind": importer_kind(
                    identity["path"], source, int(identity["size_bytes"])
                ),
                "_source_file": source,
                "_source_tree": source_tree,
            }
        )
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for entry in provisional:
        grouped[str(entry["path"])].append(entry)
    if set(grouped) != set(base_by_path):
        raise TierAError(
            "history=true active path universe differs from the complete current export"
        )

    description_by_binary: dict[str, dict[str, Any]] = {}
    for entry in provisional:
        if entry["source_kind"] != "derived_asset_description":
            continue
        original_path = safe_relative_path(
            entry.get("original_path"), field="description original_path"
        )
        original_path = f"sources/{original_path}"
        if not entry["active"]:
            raise TierAError(
                "historical binary descriptions require an explicit paired-version contract"
            )
        if original_path in description_by_binary:
            raise TierAError("multiple active binary descriptions target one binary path")
        description_by_binary[original_path] = entry
    for binary_path, description in description_by_binary.items():
        sources = grouped.get(binary_path)
        if not sources or len(sources) != 1 or not sources[0]["active"]:
            raise TierAError(
                "binary description does not pair one-to-one with one active binary"
            )
        if description["media_type"] != "text/markdown":
            raise TierAError("legacy binary description is not Markdown")
        sources[0]["binary_description_path"] = description["path"]
        sources[0]["binary_description_hash"] = description["content_hash"]
        description["describes_binary_path"] = binary_path
    binary_entries = [entry for entry in provisional if entry["workspace_kind"] == "binary"]
    unpaired_binaries = [
        entry for entry in binary_entries if not entry.get("binary_description_path")
    ]
    if unpaired_binaries:
        raise TierAError(
            "one or more legacy binaries have no exact byte-copied description; "
            "the simplified binary API would create an unexpected companion"
        )
    non_markdown_descriptions = [
        entry
        for entry in provisional
        if entry.get("describes_binary_path") and entry["workspace_kind"] != "markdown"
    ]
    if non_markdown_descriptions:
        raise TierAError("a legacy binary description is not importable as exact Markdown")

    entries: list[dict[str, Any]] = []
    for logical_path in sorted(grouped):
        versions = grouped[logical_path]
        active_positions = [index for index, row in enumerate(versions) if row["active"]]
        if active_positions != [len(versions) - 1]:
            raise TierAError(
                "history manifest order must end in exactly one active version per path"
            )
        prior_order: tuple[str, int] | None = None
        for lineage_index, row in enumerate(versions, start=1):
            captured_at = str(row.get("captured_at") or "")
            order_key = (captured_at, int(row["legacy_manifest_ordinal"]))
            if prior_order is not None and order_key <= prior_order:
                raise TierAError(
                    "legacy history order is not strictly captured-at/manifest-ordinal ordered"
                )
            prior_order = order_key
            archive_path = (
                "archive/"
                + sha256_hex(logical_path.encode("utf-8"))[:32]
                + f"/v{lineage_index:020d}-"
                + row["content_hash"].removeprefix("sha256:")[:16]
            )
            destination = output.joinpath(*PurePosixPath(archive_path).parts)
            hardlink_new(Path(row.pop("_source_file")), destination)
            row["content_origin"] = row.pop("_source_tree")
            row["lineage_ordinal"] = lineage_index
            row["lineage_length"] = len(versions)
            row["archive_path"] = archive_path
            entries.append(row)
            if row["active"]:
                workspace_path = f"workspace/{logical_path}"
                hardlink_new(destination, output.joinpath(*PurePosixPath(workspace_path).parts))
                row["workspace_archive_path"] = workspace_path

    native = native_materialization(
        native_records_path,
        expected_stable_import_id=str(history["stable_import_id"]),
        output_root=output,
    )
    legacy_paths = {str(entry["path"]) for entry in entries}
    native_paths = {
        native["archive"]["path"],
        native["archive"]["description_path"],
        native["index"]["path"],
        *(item["path"] for item in native["materialized"]),
        *(plan["path"] for plan in native["checkpoints"]),
    }
    collision = legacy_paths & native_paths
    if collision:
        raise TierAError("native materialization collides with a legacy vault path")

    totals = {
        "logical_paths": len(grouped),
        "versions": len(entries),
        "active_versions": sum(bool(entry["active"]) for entry in entries),
        "inactive_versions": sum(not bool(entry["active"]) for entry in entries),
        "bytes_across_versions": sum(int(entry["size_bytes"]) for entry in entries),
        "max_versions_per_path": max((len(rows) for rows in grouped.values()), default=0),
        "binary_descriptions": len(description_by_binary),
        "content_origins": dict(
            sorted(Counter(str(entry["content_origin"]) for entry in entries).items())
        ),
    }
    artifact: dict[str, Any] = {
        "schema": COMPOSITE_SCHEMA,
        "created_at": utc_now(),
        "source": {
            "stable_import_id": history["stable_import_id"],
            "corpus_revision": history.get("corpus_revision"),
            "history_manifest_sha256": raw_manifest_sha256(history_manifest_path),
            "current_manifest_sha256": raw_manifest_sha256(current_manifest_path),
            "delta_manifest_sha256": raw_manifest_sha256(delta_manifest_path),
            "native_records_sha256": native["records"]["capture_sha256"],
            "history_included": True,
            "lineage_order": (
                "legacy endpoint ORDER BY logical path, captured_at, source_id, "
                "asset_version; manifest ordinal is retained"
            ),
        },
        "requirements": {
            "binary_description_copy": "exact_legacy_bytes",
            "description_regeneration_allowed": False,
            "semantic_index_required": False,
            "checkpoint_parent_policy": "all_parents_present_and_service_resolved",
            "unsupported_condition_policy": "fail_closed",
        },
        "totals": totals,
        "entries": entries,
        "native": native,
    }
    identity = {
        "schema": artifact["schema"],
        "source": artifact["source"],
        "requirements": artifact["requirements"],
        "totals": artifact["totals"],
        "entries": artifact["entries"],
        "native": artifact["native"],
    }
    artifact["composite_sha256"] = sha256_value(canonical_json(identity))
    write_json_private(output / "tier-a-manifest.json", artifact)
    write_checksums(output)
    return artifact


def composite_identity(artifact: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "schema": artifact["schema"],
        "source": artifact["source"],
        "requirements": artifact["requirements"],
        "totals": artifact["totals"],
        "entries": artifact["entries"],
        "native": artifact["native"],
    }


def load_composite(root: Path, *, verify_files: bool = True) -> dict[str, Any]:
    root = root.resolve(strict=True)
    if verify_files:
        verify_checksum_tree(root)
    artifact = load_json_object(safe_file(root, "tier-a-manifest.json"))
    if artifact.get("schema") != COMPOSITE_SCHEMA:
        raise TierAError(f"tier-a-manifest.json is not {COMPOSITE_SCHEMA}")
    expected = sha256_value(canonical_json(composite_identity(artifact)))
    if artifact.get("composite_sha256") != expected:
        raise TierAError("Tier-A composite fingerprint mismatch")
    return artifact


def rebuild_native_materialization(source_root: Path, *, output: Path) -> dict[str, Any]:
    """Upgrade a verified predecessor composite without touching legacy bytes."""

    source_root = source_root.resolve(strict=True)
    predecessor = load_composite(source_root)
    if output.exists():
        raise TierAError(f"refusing to overwrite {output}")
    output.mkdir(parents=True, mode=0o700)
    output.chmod(0o700)
    entries = [dict(entry) for entry in predecessor["entries"]]
    for entry in entries:
        source = safe_file(source_root, str(entry["archive_path"]))
        hardlink_new(
            source,
            output.joinpath(*PurePosixPath(str(entry["archive_path"])).parts),
        )
        if entry["active"]:
            workspace_path = str(
                entry.get("workspace_archive_path") or f"workspace/{entry['path']}"
            )
            hardlink_new(
                source,
                output.joinpath(*PurePosixPath(workspace_path).parts),
            )
    predecessor_native = predecessor["native"]
    native = native_materialization(
        safe_file(source_root, str(predecessor_native["archive"]["path"])),
        expected_stable_import_id=str(predecessor["source"]["stable_import_id"]),
        output_root=output,
    )
    if native["records"]["count"] != predecessor_native["records"]["count"]:
        raise TierAError("native materialization rebuild changed the record count")
    legacy_paths = {str(entry["path"]) for entry in entries}
    native_paths = {
        native["archive"]["path"],
        native["archive"]["description_path"],
        native["index"]["path"],
        *(item["path"] for item in native["materialized"]),
        *(plan["path"] for plan in native["checkpoints"]),
    }
    if legacy_paths & native_paths:
        raise TierAError("rebuilt native materialization collides with a legacy vault path")
    source = {
        **predecessor["source"],
        "predecessor_composite_sha256": predecessor["composite_sha256"],
    }
    artifact: dict[str, Any] = {
        "schema": COMPOSITE_SCHEMA,
        "created_at": utc_now(),
        "source": source,
        "requirements": predecessor["requirements"],
        "totals": predecessor["totals"],
        "entries": entries,
        "native": native,
    }
    artifact["composite_sha256"] = sha256_value(
        canonical_json(composite_identity(artifact))
    )
    write_json_private(output / "tier-a-manifest.json", artifact)
    write_checksums(output)
    return artifact


def workspace_kind(entry: Mapping[str, Any]) -> str:
    kind = entry.get("workspace_kind")
    if kind not in {"markdown", "binary"}:
        raise TierAError("composite entry omits its importer-compatible workspace kind")
    return str(kind)


def portable_metadata_for_entry(
    entry: Mapping[str, Any],
    *,
    descriptions: Mapping[str, Mapping[str, Any]],
    history_semantics: str | None,
) -> dict[str, Any]:
    metadata: dict[str, Any] = {
        "legacy": {
            "source_ref": entry["source_ref"],
            "source_kind": entry["source_kind"],
            "asset_ref": entry["asset_ref"],
            "asset_version": entry["asset_version"],
            "legacy_manifest_ordinal": entry["legacy_manifest_ordinal"],
            "lineage_ordinal": entry["lineage_ordinal"],
            "captured_at": entry.get("captured_at"),
            "stored_at": entry.get("stored_at"),
        }
    }
    if history_semantics is not None:
        metadata["_brunn_tier_a_history"] = {
            "format": TIER_A_HISTORY_STAGE_FORMAT,
            "target_lineage_ordinal": entry["lineage_ordinal"],
            "semantics": history_semantics,
        }
    if entry.get("describes_binary_path"):
        binary_path = str(entry["describes_binary_path"])
        metadata.update(
            {
                "kind": "binary_description",
                "binary_path": binary_path,
                "description_status": entry.get("description_status"),
                "description_method": entry.get("description_method"),
                "_brunn_tier_a": {
                    "format": PORTABLE_COMPANION_FORMAT,
                    "copy_method": "exact_legacy_bytes",
                },
            }
        )
    if entry.get("binary_description_path"):
        description_path = str(entry["binary_description_path"])
        description = descriptions.get(description_path)
        if description is None:
            raise TierAError("binary description metadata target is absent")
        metadata["_brunn_tier_a"] = {
            "format": PORTABLE_COMPANION_FORMAT,
            "binary_description_path": description_path,
            "binary_description_hash": description["content_hash"],
            "copy_method": "exact_legacy_bytes",
        }
    return metadata


def stage_history_semantics(
    entry: Mapping[str, Any],
    predecessor: Mapping[str, Any] | None,
    *,
    kind: str,
) -> str | None:
    same_bytes_as_predecessor = predecessor is not None and (
        predecessor["content_hash"] == entry["content_hash"]
        and int(predecessor["size_bytes"]) == int(entry["size_bytes"])
    )
    if same_bytes_as_predecessor and kind == "binary":
        raise TierAError(
            "same-byte binary history is unsupported; refusing to collapse "
            "or synthesize a binary version"
        )
    if same_bytes_as_predecessor and entry.get("describes_binary_path"):
        raise TierAError(
            "same-byte binary companion history is unsupported; refusing to "
            "collapse or synthesize a companion version"
        )
    if kind != "markdown":
        return None
    return (
        EXACT_HISTORY_SEMANTICS
        if same_bytes_as_predecessor
        else ORDINARY_HISTORY_SEMANTICS
    )


def materialize_stage(
    root: Path,
    *,
    stage_index: int,
    output: Path,
    include_native: bool = False,
) -> dict[str, Any]:
    artifact = load_composite(root)
    stage_count = int(artifact["totals"]["max_versions_per_path"])
    if stage_index < 0 or stage_index >= stage_count:
        raise TierAError(f"stage index must be between 0 and {stage_count - 1}")
    if output.exists():
        raise TierAError(f"refusing to overwrite {output}")
    output.mkdir(parents=True, mode=0o700)
    output.chmod(0o700)
    entries = [dict(entry) for entry in artifact["entries"]]
    lineage = {
        (str(entry["path"]), int(entry["lineage_ordinal"])): entry
        for entry in entries
    }
    selected = [
        entry
        for entry in entries
        if int(entry["lineage_ordinal"]) == stage_index + 1
    ]
    if stage_index == 0:
        expected_paths = {str(entry["path"]) for entry in entries}
        if {str(entry["path"]) for entry in selected} != expected_paths:
            raise TierAError("stage zero does not contain every legacy logical path")
    descriptions = {
        str(entry["path"]): entry
        for entry in selected
        if entry.get("describes_binary_path")
    }
    portable_entries: list[dict[str, Any]] = []
    for entry in sorted(selected, key=lambda item: str(item["path"])):
        relative = f"workspace/{entry['path']}"
        kind = workspace_kind(entry)
        ordinal = int(entry["lineage_ordinal"])
        predecessor = lineage.get((str(entry["path"]), ordinal - 1))
        history_semantics = stage_history_semantics(
            entry,
            predecessor,
            kind=kind,
        )
        source = safe_file(root, str(entry["archive_path"]))
        hardlink_new(source, output.joinpath(*PurePosixPath(relative).parts))
        portable_entries.append(
            {
                "path": entry["path"],
                "archive_path": relative,
                "kind": kind,
                "entry_ref": (
                    "legacy-source:" + sha256_hex(str(entry["source_ref"]).encode())[:32]
                ),
                "version": entry["lineage_ordinal"],
                "content_hash": entry["content_hash"],
                "size_bytes": entry["size_bytes"],
                "media_type": entry["media_type"],
                "modified_unix_ns": entry["portable"].get("modified_unix_ns"),
                "mode": entry["portable"].get("mode"),
                "metadata": portable_metadata_for_entry(
                    entry,
                    descriptions=descriptions,
                    history_semantics=history_semantics,
                ),
                "current": entry["active"],
                "deleted": False,
                "created_at": entry.get("captured_at"),
            }
        )
    if include_native:
        native = artifact["native"]
        archive = native["archive"]
        index = native["index"]
        for item in native["materialized"]:
            relative = f"workspace/{item['path']}"
            hardlink_new(
                safe_file(root, str(item["archive_path"])),
                output.joinpath(*PurePosixPath(relative).parts),
            )
            portable_entries.append(
                {
                    "path": item["path"],
                    "archive_path": relative,
                    "kind": "markdown",
                    "entry_ref": "tier-a-native-record:"
                    + sha256_hex(str(item["record_ref"]).encode())[:32],
                    "version": 1,
                    "content_hash": item["content_hash"],
                    "size_bytes": item["size_bytes"],
                    "media_type": item["media_type"],
                    "modified_unix_ns": None,
                    "mode": 0o600,
                    "metadata": {
                        "kind": "legacy_native_record",
                        "record_ref": item["record_ref"],
                        "record_kind": item["record_kind"],
                        "record_version": item["record_version"],
                        "native_materialization_format": "native-record@v1",
                    },
                    "current": True,
                    "deleted": False,
                    "created_at": None,
                }
            )
        for item in (archive, index):
            relative = f"workspace/{item['path']}"
            hardlink_new(
                safe_file(root, str(item["path"])),
                output.joinpath(*PurePosixPath(relative).parts),
            )
            metadata: dict[str, Any] = {
                "kind": "legacy_native_archive"
                if item is archive
                else "legacy_native_index",
                "native_record_count": native["records"]["count"],
            }
            if item is archive:
                metadata["_brunn_tier_a"] = {
                    "format": PORTABLE_COMPANION_FORMAT,
                    "binary_description_path": archive["description_path"],
                    "binary_description_hash": archive["description_hash"],
                    "copy_method": "deterministic_materialization",
                }
            portable_entries.append(
                {
                    "path": item["path"],
                    "archive_path": relative,
                    "kind": item["kind"],
                    "entry_ref": "tier-a-native:" + sha256_hex(item["path"].encode())[:32],
                    "version": 1,
                    "content_hash": item["content_hash"],
                    "size_bytes": item["size_bytes"],
                    "media_type": item["media_type"],
                    "modified_unix_ns": None,
                    "mode": 0o600,
                    "metadata": metadata,
                    "current": True,
                    "deleted": False,
                    "created_at": None,
                }
            )
        description_relative = f"workspace/{archive['description_path']}"
        hardlink_new(
            safe_file(root, archive["description_path"]),
            output.joinpath(*PurePosixPath(description_relative).parts),
        )
        portable_entries.append(
            {
                "path": archive["description_path"],
                "archive_path": description_relative,
                "kind": "markdown",
                "entry_ref": "tier-a-native-description:"
                + sha256_hex(archive["description_path"].encode())[:32],
                "version": 1,
                "content_hash": archive["description_hash"],
                "size_bytes": archive["description_size_bytes"],
                "media_type": "text/markdown",
                "modified_unix_ns": None,
                "mode": 0o600,
                "metadata": {
                    "kind": "binary_description",
                    "binary_path": archive["path"],
                    "_brunn_tier_a": {
                        "format": PORTABLE_COMPANION_FORMAT,
                        "copy_method": "deterministic_materialization",
                    },
                },
                "current": True,
                "deleted": False,
                "created_at": None,
            }
        )
    manifest = {
        "format": WORKSPACE_EXPORT_FORMAT,
        "version": 1,
        "workspace_generation": 0,
        "workspace_generation_is_snapshot": False,
        "tier_a": {
            "composite_sha256": artifact["composite_sha256"],
            "stage_index": stage_index,
            "stage_count": stage_count,
            "include_native": include_native,
        },
        "entries": sorted(portable_entries, key=lambda item: str(item["archive_path"])),
    }
    write_json_private(output / "manifest.json", manifest)
    write_checksums(output)
    return manifest


def local_audit(root: Path) -> dict[str, Any]:
    artifact = load_composite(root)
    entries = artifact["entries"]
    by_path: dict[str, list[dict[str, Any]]] = defaultdict(list)
    differences: list[dict[str, Any]] = []
    for entry in entries:
        by_path[str(entry["path"])].append(entry)
        path = safe_file(root, str(entry["archive_path"]))
        actual_hash, actual_size = hash_file(path)
        if actual_hash != entry["content_hash"] or actual_size != entry["size_bytes"]:
            differences.append(
                {
                    "kind": "content_identity",
                    "path": entry["path"],
                    "lineage_ordinal": entry["lineage_ordinal"],
                    "hash_match": actual_hash == entry["content_hash"],
                    "size_match": actual_size == entry["size_bytes"],
                }
            )
    lineage_failures = 0
    for path, versions in by_path.items():
        ordinals = [int(entry["lineage_ordinal"]) for entry in versions]
        active = [entry for entry in versions if entry["active"]]
        if ordinals != list(range(1, len(versions) + 1)) or len(active) != 1:
            lineage_failures += 1
            differences.append({"kind": "lineage", "path": path})
    description_failures = 0
    entry_by_path = {str(entry["path"]): entry for entry in entries if entry["active"]}
    for entry in entries:
        description_path = entry.get("binary_description_path")
        if not description_path:
            continue
        description = entry_by_path.get(str(description_path))
        if (
            description is None
            or description.get("describes_binary_path") != entry["path"]
            or description["content_hash"] != entry["binary_description_hash"]
        ):
            description_failures += 1
            differences.append(
                {"kind": "binary_description_pair", "path": entry["path"]}
            )
    checkpoints = artifact["native"]["checkpoints"]
    native_materialization_failures = 0
    for item in artifact["native"]["materialized"]:
        actual_hash, actual_size = hash_file(
            safe_file(root, str(item["archive_path"]))
        )
        if (
            actual_hash != item["content_hash"]
            or actual_size != int(item["size_bytes"])
        ):
            native_materialization_failures += 1
            differences.append(
                {
                    "kind": "native_record_materialization",
                    "record_ref": item["record_ref"],
                }
            )
    try:
        validate_checkpoint_graph(checkpoints)
        checkpoint_failures = 0
    except TierAError as error:
        checkpoint_failures = 1
        differences.append({"kind": "checkpoint_graph", "message": str(error)})
    passed = not differences
    return {
        "schema": AUDIT_SCHEMA,
        "created_at": utc_now(),
        "scope": "composite_local",
        "composite_sha256": artifact["composite_sha256"],
        "verdict": "pass" if passed else "fail",
        "checks": {
            "checksum_ledger": "pass",
            "exact_content_hash_and_size": "pass" if not differences else "see_differences",
            "logical_path_lineage": "pass" if lineage_failures == 0 else "fail",
            "binary_description_byte_copy_pairs": (
                "pass" if description_failures == 0 else "fail"
            ),
            "native_record_capture": (
                "pass" if native_materialization_failures == 0 else "fail"
            ),
            "checkpoint_graph": "pass" if checkpoint_failures == 0 else "fail",
        },
        "counts": {
            "logical_paths": len(by_path),
            "versions": len(entries),
            "binary_descriptions": artifact["totals"]["binary_descriptions"],
            "native_records": artifact["native"]["records"]["count"],
            "native_record_materializations": len(
                artifact["native"]["materialized"]
            ),
            "native_checkpoints": len(checkpoints),
            "differences": len(differences),
        },
        "differences": differences,
        "tier_a_assessment": (
            "local_composite_ready_for_isolated_import"
            if passed
            else "blocked_by_local_fidelity_failure"
        ),
    }


class ApiClient:
    def __init__(self, base_url: str, token: str):
        self.base_url = base_url.rstrip("/") + "/"
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        url = urllib.parse.urljoin(self.base_url, path.lstrip("/"))
        payload = canonical_json(body) if body is not None else None
        request = urllib.request.Request(
            url,
            data=payload,
            method=method,
            headers={
                "Accept": "application/json",
                "Authorization": f"Bearer {self.token}",
                **({"Content-Type": "application/json"} if payload is not None else {}),
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                value = json.loads(response.read())
        except (urllib.error.URLError, json.JSONDecodeError) as error:
            raise TierAError(f"workspace API request failed: {method} {path}: {error}") from error
        if not isinstance(value, dict):
            raise TierAError("workspace API response is not a JSON object")
        return value


def unwrap_data(value: Mapping[str, Any]) -> Mapping[str, Any]:
    data = value.get("data")
    return data if isinstance(data, dict) else value


def token_from_env(name: str) -> str:
    token = os.environ.get(name, "").strip()
    if not token:
        raise TierAError(f"{name} is required")
    return token


def provision_owner(
    *,
    api_url: str,
    token_env: str,
    external_ref: str,
    credential_out: Path,
) -> dict[str, Any]:
    client = ApiClient(api_url, token_from_env(token_env))
    response = client.request(
        "POST",
        "/v1/admin/users",
        body={
            "external_ref": external_ref,
            "display_name": "Tier-A isolated fidelity import",
            "credential_name": "Tier-A migration owner",
        },
    )
    credential = response.get("credential")
    user = response.get("user")
    if not isinstance(credential, dict) or not isinstance(user, dict):
        raise TierAError("isolated user provisioning omitted user or credential")
    token = credential.get("token")
    user_ref = user.get("id")
    credential_ref = credential.get("id")
    if (
        not isinstance(token, str)
        or not token
        or not isinstance(user_ref, str)
        or not isinstance(credential_ref, str)
    ):
        raise TierAError("isolated user provisioning omitted its one-time credential")
    write_json_private(
        credential_out,
        {
            "schema": CREDENTIAL_SCHEMA,
            "api_url": api_url,
            "user_ref": user_ref,
            "credential_ref": credential_ref,
            "access": "owner",
            "token": token,
        },
    )
    return {
        "schema": CREDENTIAL_SCHEMA,
        "verdict": "pass",
        "user_ref": user_ref,
        "credential_ref": credential_ref,
        "credential_out": str(credential_out),
        "token_exposed": False,
    }


def import_checkpoints(
    root: Path,
    *,
    api_url: str,
    token_env: str,
    out: Path,
) -> dict[str, Any]:
    artifact = load_composite(root)
    checkpoints = validate_checkpoint_graph(artifact["native"]["checkpoints"])
    client = ApiClient(api_url, token_from_env(token_env))
    receipts = []
    imported: set[str] = set()
    for checkpoint in checkpoints:
        parent = checkpoint.get("parent_checkpoint_ref")
        if parent is not None and parent not in imported:
            raise TierAError("checkpoint import order does not resolve its parent first")
        response = client.request(
            "POST",
            "/v1/workspace/write",
            body={
                "path": checkpoint["path"],
                "content": checkpoint["content"],
                "media_type": "text/markdown",
                "expected_version": 0,
                "idempotency_key": "tier-a:" + checkpoint["checkpoint_ref"],
                "metadata": checkpoint["metadata"],
            },
        )
        data = unwrap_data(response)
        if (
            data.get("path") != checkpoint["path"]
            or normalize_sha256(data.get("content_hash"), "checkpoint receipt hash")
            != checkpoint["content_hash"]
        ):
            raise TierAError("checkpoint import receipt differs from its exact plan")
        opened = client.request(
            "POST",
            "/v1/workspace/open",
            body={
                "task": "Verify imported checkpoint lineage.",
                "resume_checkpoint_ref": checkpoint["checkpoint_ref"],
                "token_budget": 1_000,
            },
        )
        opened_data = unwrap_data(opened)
        opened_checkpoint = opened_data.get("checkpoint")
        if not isinstance(opened_checkpoint, dict) or (
            opened_checkpoint.get("checkpoint_id") != checkpoint["checkpoint_ref"]
        ):
            raise TierAError("imported checkpoint is not resumable by its exact reference")
        imported.add(str(checkpoint["checkpoint_ref"]))
        receipts.append(
            {
                "checkpoint_ref": checkpoint["checkpoint_ref"],
                "parent_checkpoint_ref": parent,
                "path": checkpoint["path"],
                "content_hash": checkpoint["content_hash"],
                "receipt_version": data.get("version"),
                "resume_verified": True,
            }
        )
    result = {
        "schema": CHECKPOINT_IMPORT_SCHEMA,
        "created_at": utc_now(),
        "composite_sha256": artifact["composite_sha256"],
        "verdict": "pass",
        "checkpoint_count": len(receipts),
        "parent_reference_count": sum(
            receipt["parent_checkpoint_ref"] is not None for receipt in receipts
        ),
        "receipts": receipts,
    }
    write_json_private(out, result)
    return result


def remote_manifest(client: ApiClient) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    cursor: dict[str, Any] = {}
    while True:
        query = {"history": "true", "limit": "1000", **cursor}
        path = "/v1/workspace/manifest?" + urllib.parse.urlencode(query)
        response = unwrap_data(client.request("GET", path))
        page = response.get("entries")
        if not isinstance(page, list) or any(not isinstance(item, dict) for item in page):
            raise TierAError("workspace history manifest omitted its entries")
        entries.extend(dict(item) for item in page)
        next_cursor = response.get("next")
        if next_cursor is None:
            break
        if not isinstance(next_cursor, dict):
            raise TierAError("workspace history manifest returned an invalid cursor")
        required = {"after_path", "after_entry_ref", "after_version"}
        if not required.issubset(next_cursor):
            raise TierAError("workspace history cursor omitted required fields")
        cursor = {key: next_cursor[key] for key in sorted(required)}
    return entries


def expected_service_history(artifact: Mapping[str, Any]) -> list[dict[str, Any]]:
    expected: list[dict[str, Any]] = []
    for entry in artifact["entries"]:
        expected.append(
            {
                "path": entry["path"],
                "version": entry["lineage_ordinal"],
                "content_hash": entry["content_hash"],
                "size_bytes": entry["size_bytes"],
                "kind": workspace_kind(entry),
                "current": bool(entry["active"]),
            }
        )
    native = artifact["native"]
    for item in native["materialized"]:
        expected.append(
            {
                "path": item["path"],
                "version": 1,
                "content_hash": item["content_hash"],
                "size_bytes": item["size_bytes"],
                "kind": "markdown",
                "current": True,
            }
        )
    for item in (
        native["archive"],
        native["index"],
        {
            "path": native["archive"]["description_path"],
            "content_hash": native["archive"]["description_hash"],
            "size_bytes": native["archive"]["description_size_bytes"],
            "kind": "markdown",
        },
    ):
        expected.append(
            {
                "path": item["path"],
                "version": 1,
                "content_hash": item["content_hash"],
                "size_bytes": item["size_bytes"],
                "kind": item["kind"],
                "current": True,
            }
        )
    for checkpoint in native["checkpoints"]:
        expected.append(
            {
                "path": checkpoint["path"],
                "version": 1,
                "content_hash": checkpoint["content_hash"],
                "size_bytes": checkpoint["size_bytes"],
                "kind": "markdown",
                "current": True,
            }
        )
    return expected


def roundtrip_audit(root: Path, *, export_root: Path) -> dict[str, Any]:
    """Verify a full simplified-service history export, including downloaded bytes."""

    artifact = load_composite(root)
    export_root = export_root.resolve(strict=True)
    verify_checksum_tree(export_root)
    manifest = load_json_object(safe_file(export_root, "manifest.json"))
    require_format(manifest, WORKSPACE_EXPORT_FORMAT, export_root / "manifest.json")
    actual_entries = require_entries(manifest, export_root / "manifest.json")
    differences: list[dict[str, Any]] = []
    for entry in actual_entries:
        archive_path = safe_relative_path(
            entry.get("archive_path"), field="round-trip archive path"
        )
        actual_hash, actual_size = hash_file(safe_file(export_root, archive_path))
        declared_hash = normalize_sha256(
            entry.get("content_hash"), "round-trip content hash"
        )
        declared_size = int(entry.get("size_bytes") or -1)
        if actual_hash != declared_hash or actual_size != declared_size:
            differences.append(
                {
                    "kind": "roundtrip_download_identity",
                    "path": entry.get("path"),
                    "version": entry.get("version"),
                    "hash_match": actual_hash == declared_hash,
                    "size_match": actual_size == declared_size,
                }
            )

    expected = expected_service_history(artifact)
    expected_by_path: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for entry in expected:
        expected_by_path[str(entry["path"])].append(entry)
    history_by_path: dict[str, list[dict[str, Any]]] = defaultdict(list)
    current_by_path: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for entry in actual_entries:
        archive_path = str(entry.get("archive_path") or "")
        if archive_path.startswith("history/"):
            history_by_path[str(entry.get("path"))].append(entry)
        elif archive_path.startswith("workspace/"):
            current_by_path[str(entry.get("path"))].append(entry)
        else:
            differences.append(
                {
                    "kind": "roundtrip_unexpected_archive_path",
                    "archive_path": archive_path,
                }
            )
    export_history_versions = sum(map(len, history_by_path.values()))
    export_current_files = sum(map(len, current_by_path.values()))

    for path, expected_versions in expected_by_path.items():
        expected_versions.sort(key=lambda item: int(item["version"]))
        actual_versions = sorted(
            history_by_path.pop(path, []),
            key=lambda item: int(item.get("version") or 0),
        )
        if len(actual_versions) != len(expected_versions):
            differences.append(
                {
                    "kind": "roundtrip_history_count",
                    "path": path,
                    "expected": len(expected_versions),
                    "actual": len(actual_versions),
                }
            )
        else:
            for expected_entry, actual in zip(
                expected_versions, actual_versions, strict=True
            ):
                checks = {
                    "version": int(actual.get("version") or 0)
                    == int(expected_entry["version"]),
                    "content_hash": normalize_sha256(actual.get("content_hash"))
                    == expected_entry["content_hash"],
                    "size_bytes": int(actual.get("size_bytes") or -1)
                    == int(expected_entry["size_bytes"]),
                    "kind": str(actual.get("kind")) == expected_entry["kind"],
                    "current": bool(actual.get("current"))
                    == bool(expected_entry["current"]),
                }
                if not all(checks.values()):
                    differences.append(
                        {
                            "kind": "roundtrip_history_identity",
                            "path": path,
                            "version": expected_entry["version"],
                            "checks": checks,
                        }
                    )
        expected_current = [item for item in expected_versions if item["current"]]
        actual_current = current_by_path.pop(path, [])
        if len(expected_current) != 1 or len(actual_current) != 1:
            differences.append(
                {
                    "kind": "roundtrip_current_count",
                    "path": path,
                    "expected": len(expected_current),
                    "actual": len(actual_current),
                }
            )
        else:
            current = actual_current[0]
            wanted = expected_current[0]
            if (
                normalize_sha256(current.get("content_hash"))
                != wanted["content_hash"]
                or int(current.get("size_bytes") or -1)
                != int(wanted["size_bytes"])
                or int(current.get("version") or 0) != int(wanted["version"])
                or str(current.get("kind")) != wanted["kind"]
                or not bool(current.get("current"))
                or str(current.get("archive_path")) != f"workspace/{path}"
            ):
                differences.append(
                    {"kind": "roundtrip_current_identity", "path": path}
                )
    for path, entries in sorted(history_by_path.items()):
        differences.append(
            {
                "kind": "roundtrip_unexpected_history_path",
                "path": path,
                "versions": len(entries),
            }
        )
    for path, entries in sorted(current_by_path.items()):
        differences.append(
            {
                "kind": "roundtrip_unexpected_current_path",
                "path": path,
                "versions": len(entries),
            }
        )
    passed = not differences
    return {
        "schema": AUDIT_SCHEMA,
        "created_at": utc_now(),
        "scope": "isolated_simplified_service",
        "composite_sha256": artifact["composite_sha256"],
        "verdict": "pass" if passed else "fail",
        "checks": {
            "export_checksum_ledger": "pass",
            "downloaded_bytes_match_export_manifest": (
                "pass"
                if not any(
                    item["kind"] == "roundtrip_download_identity"
                    for item in differences
                )
                else "fail"
            ),
            "every_expected_path_version_hash_size": (
                "pass" if passed else "see_differences"
            ),
            "current_workspace_projection": (
                "pass" if passed else "see_differences"
            ),
            "unexpected_paths": "zero" if passed else "see_differences",
        },
        "counts": {
            "expected_paths": len(expected_by_path),
            "expected_history_versions": len(expected),
            "export_history_versions": export_history_versions,
            "export_current_files": export_current_files,
            "export_manifest_entries": len(actual_entries),
            "differences": len(differences),
        },
        "differences": differences,
        "tier_a_assessment": (
            "fidelity_gate_pass"
            if passed
            else "blocked_by_service_fidelity_failure"
        ),
    }


def service_audit(
    root: Path,
    *,
    api_url: str,
    token_env: str,
) -> dict[str, Any]:
    artifact = load_composite(root)
    client = ApiClient(api_url, token_from_env(token_env))
    remote = remote_manifest(client)
    by_path: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for entry in remote:
        by_path[str(entry.get("path"))].append(entry)
    remote_checkpoint_refs = {
        str(client_metadata.get("checkpoint_ref"))
        for entry in remote
        for metadata in [entry.get("metadata")]
        for client_metadata in [
            (
                metadata.get("client")
                if isinstance(metadata, dict)
                and isinstance(metadata.get("client"), dict)
                else metadata
            )
        ]
        if isinstance(client_metadata, dict)
        and client_metadata.get("kind") == "checkpoint"
        and isinstance(client_metadata.get("checkpoint_ref"), str)
    }
    expected_legacy: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for entry in artifact["entries"]:
        expected_legacy[str(entry["path"])].append(entry)
    differences: list[dict[str, Any]] = []
    for path, expected_versions in expected_legacy.items():
        actual_versions = sorted(by_path.pop(path, []), key=lambda item: int(item["version"]))
        expected_versions = sorted(
            expected_versions, key=lambda item: int(item["lineage_ordinal"])
        )
        if len(actual_versions) != len(expected_versions):
            differences.append(
                {
                    "kind": "version_count",
                    "path": path,
                    "expected": len(expected_versions),
                    "actual": len(actual_versions),
                }
            )
            continue
        for expected, actual in zip(expected_versions, actual_versions, strict=True):
            checks = {
                "version": int(actual.get("version") or 0)
                == int(expected["lineage_ordinal"]),
                "content_hash": normalize_sha256(actual.get("content_hash"))
                == expected["content_hash"],
                "size_bytes": int(actual.get("size_bytes") or -1)
                == int(expected["size_bytes"]),
                "kind": str(actual.get("kind")) == workspace_kind(expected),
                "current": bool(actual.get("current")) == bool(expected["active"]),
            }
            if not all(checks.values()):
                differences.append(
                    {
                        "kind": "version_identity",
                        "path": path,
                        "lineage_ordinal": expected["lineage_ordinal"],
                        "checks": checks,
                    }
                )

    native = artifact["native"]
    for item in native["materialized"]:
        actual = by_path.pop(str(item["path"]), [])
        if len(actual) != 1:
            differences.append(
                {
                    "kind": "native_record_materialization_count",
                    "record_ref": item["record_ref"],
                    "actual": len(actual),
                }
            )
            continue
        if (
            normalize_sha256(actual[0].get("content_hash"))
            != item["content_hash"]
            or int(actual[0].get("size_bytes") or -1)
            != int(item["size_bytes"])
        ):
            differences.append(
                {
                    "kind": "native_record_materialization_identity",
                    "record_ref": item["record_ref"],
                }
            )
    for item in (
        native["archive"],
        native["index"],
        {
            "path": native["archive"]["description_path"],
            "content_hash": native["archive"]["description_hash"],
            "size_bytes": native["archive"]["description_size_bytes"],
        },
    ):
        actual = by_path.pop(str(item["path"]), [])
        if len(actual) != 1:
            differences.append(
                {
                    "kind": "native_materialization_count",
                    "path": item["path"],
                    "actual": len(actual),
                }
            )
            continue
        if (
            normalize_sha256(actual[0].get("content_hash")) != item["content_hash"]
            or int(actual[0].get("size_bytes") or -1) != int(item["size_bytes"])
        ):
            differences.append(
                {"kind": "native_materialization_identity", "path": item["path"]}
            )

    checkpoint_parent_count = 0
    for checkpoint in native["checkpoints"]:
        actual = by_path.pop(str(checkpoint["path"]), [])
        if len(actual) != 1:
            differences.append(
                {
                    "kind": "checkpoint_count",
                    "checkpoint_ref": checkpoint["checkpoint_ref"],
                    "actual": len(actual),
                }
            )
            continue
        metadata = actual[0].get("metadata")
        client_metadata = (
            metadata.get("client")
            if isinstance(metadata, dict) and isinstance(metadata.get("client"), dict)
            else metadata
        )
        if not isinstance(client_metadata, dict):
            client_metadata = {}
        parent = checkpoint.get("parent_checkpoint_ref")
        if parent is not None:
            checkpoint_parent_count += 1
        checks = {
            "hash": normalize_sha256(actual[0].get("content_hash"))
            == checkpoint["content_hash"],
            "checkpoint_ref": client_metadata.get("checkpoint_ref")
            == checkpoint["checkpoint_ref"],
            "parent_checkpoint_ref": client_metadata.get("parent_checkpoint_ref")
            == parent,
            "parent_resolves": parent is None or parent in remote_checkpoint_refs,
        }
        if not all(checks.values()):
            differences.append(
                {
                    "kind": "checkpoint_identity_or_parent",
                    "checkpoint_ref": checkpoint["checkpoint_ref"],
                    "checks": checks,
                }
            )
    for path, actual in sorted(by_path.items()):
        differences.append(
            {"kind": "unexpected_service_path", "path": path, "versions": len(actual)}
        )
    passed = not differences
    return {
        "schema": AUDIT_SCHEMA,
        "created_at": utc_now(),
        "scope": "isolated_simplified_service",
        "composite_sha256": artifact["composite_sha256"],
        "verdict": "pass" if passed else "fail",
        "checks": {
            "every_legacy_path_version_hash_size": "pass" if passed else "see_differences",
            "binary_descriptions": (
                "exact_legacy_bytes_at_exact_legacy_paths"
                if passed
                else "see_differences"
            ),
            "native_record_materializations_and_archive": (
                "pass" if passed else "see_differences"
            ),
            "checkpoint_parent_resolution": "pass" if passed else "see_differences",
            "unexpected_paths": "zero" if passed else "see_differences",
        },
        "counts": {
            "expected_legacy_paths": artifact["totals"]["logical_paths"],
            "expected_legacy_versions": artifact["totals"]["versions"],
            "native_records": native["records"]["count"],
            "native_record_materializations": len(native["materialized"]),
            "checkpoints": len(native["checkpoints"]),
            "checkpoint_parent_references": checkpoint_parent_count,
            "remote_manifest_versions": len(remote),
            "differences": len(differences),
        },
        "differences": differences,
        "tier_a_assessment": (
            "fidelity_gate_pass"
            if passed
            else "blocked_by_service_fidelity_failure"
        ),
    }


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)

    compose_parser = commands.add_parser(
        "compose", help="compose exact current+history+delta bytes"
    )
    compose_parser.add_argument("--current-root", type=Path, required=True)
    compose_parser.add_argument("--history-manifest", type=Path, required=True)
    compose_parser.add_argument("--delta-root", type=Path, required=True)
    compose_parser.add_argument("--native-records", type=Path, required=True)
    compose_parser.add_argument("--out", type=Path, required=True)

    rebuild_parser = commands.add_parser(
        "rebuild-native",
        help="add full native materializations to a verified predecessor composite",
    )
    rebuild_parser.add_argument("--root", type=Path, required=True)
    rebuild_parser.add_argument("--out", type=Path, required=True)

    audit_parser = commands.add_parser("audit-local", help="verify a composite")
    audit_parser.add_argument("--root", type=Path, required=True)
    audit_parser.add_argument("--out", type=Path, required=True)

    stage_parser = commands.add_parser(
        "materialize-stage", help="materialize one bounded replay stage"
    )
    stage_parser.add_argument("--root", type=Path, required=True)
    stage_parser.add_argument("--stage-index", type=int, required=True)
    stage_parser.add_argument("--include-native", action="store_true")
    stage_parser.add_argument("--out", type=Path, required=True)

    checkpoint_parser = commands.add_parser(
        "import-checkpoints",
        help="topologically import and resume-verify native checkpoints",
    )
    checkpoint_parser.add_argument("--root", type=Path, required=True)
    checkpoint_parser.add_argument("--api-url", required=True)
    checkpoint_parser.add_argument(
        "--token-env", default="BRUNN_TIER_A_TOKEN"
    )
    checkpoint_parser.add_argument("--out", type=Path, required=True)

    provision_parser = commands.add_parser(
        "provision-owner",
        help="provision an empty isolated owner and save its token privately",
    )
    provision_parser.add_argument("--api-url", required=True)
    provision_parser.add_argument("--token-env", default="BRUNN_TIER_A_ADMIN_TOKEN")
    provision_parser.add_argument("--external-ref", required=True)
    provision_parser.add_argument("--credential-out", type=Path, required=True)

    service_parser = commands.add_parser(
        "audit-service", help="audit the isolated simplified history manifest"
    )
    service_parser.add_argument("--root", type=Path, required=True)
    service_parser.add_argument("--api-url", required=True)
    service_parser.add_argument("--token-env", default="BRUNN_TIER_A_TOKEN")
    service_parser.add_argument("--out", type=Path, required=True)

    roundtrip_parser = commands.add_parser(
        "audit-roundtrip",
        help="byte-audit a full history export from the isolated service",
    )
    roundtrip_parser.add_argument("--root", type=Path, required=True)
    roundtrip_parser.add_argument("--export-root", type=Path, required=True)
    roundtrip_parser.add_argument("--out", type=Path, required=True)
    return root


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "compose":
            result = compose(
                current_root=args.current_root,
                history_manifest_path=args.history_manifest,
                delta_root=args.delta_root,
                native_records_path=args.native_records,
                output=args.out,
            )
        elif args.command == "rebuild-native":
            result = rebuild_native_materialization(
                args.root,
                output=args.out,
            )
        elif args.command == "audit-local":
            result = local_audit(args.root)
            write_json_private(args.out, result)
        elif args.command == "materialize-stage":
            result = materialize_stage(
                args.root,
                stage_index=args.stage_index,
                output=args.out,
                include_native=args.include_native,
            )
        elif args.command == "import-checkpoints":
            result = import_checkpoints(
                args.root,
                api_url=args.api_url,
                token_env=args.token_env,
                out=args.out,
            )
        elif args.command == "provision-owner":
            result = provision_owner(
                api_url=args.api_url,
                token_env=args.token_env,
                external_ref=args.external_ref,
                credential_out=args.credential_out,
            )
        elif args.command == "audit-service":
            result = service_audit(
                args.root,
                api_url=args.api_url,
                token_env=args.token_env,
            )
            write_json_private(args.out, result)
        elif args.command == "audit-roundtrip":
            result = roundtrip_audit(
                args.root,
                export_root=args.export_root,
            )
            write_json_private(args.out, result)
        else:
            raise TierAError(f"unsupported command: {args.command}")
    except TierAError as error:
        print(f"Tier-A fidelity failure: {error}", file=sys.stderr)
        return 2
    summary = {
        "status": result.get("verdict", "complete"),
        "schema": result.get("schema") or result.get("format"),
        "output": str(getattr(args, "out", "")),
        "counts": result.get("counts") or result.get("totals"),
        "composite_sha256": result.get("composite_sha256"),
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    if result.get("verdict") == "fail":
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import shlex
import shutil
import statistics
import subprocess
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator, Sequence

from native_eval import (
    NativeApiClient,
    NativeApiError,
    provision_evaluation,
    recursively_redact_secrets,
)


PROJECT_ROOT = Path(__file__).resolve().parent
DEFAULT_SCALES = (1_000, 10_000, 64_000)
PRODUCTION_RECORDS = 64_000
FUTURE_RECORDS = 640_000
DEFINITIVE_SAMPLES = 30
QUICK_SAMPLES = 3
VERBATIM_IDENTIFIER_PROBES = 30
VERBATIM_IDENTIFIER_MIN_OFFSET = 2_401
BROAD_QUERY = "deterministic performance-fixture material"
OLD_SOURCE_QUERY = (
    "Reconcile the meridian continuity doctrine with a new request and explain "
    "durable workspace source authority for a fresh agent."
)
SOURCE_TEXT_KEYS = frozenset({"content", "text", "excerpt", "source_text"})
SOURCE_IDENTITY_KEYS = frozenset(
    {
        "path",
        "title",
        "heading",
        "reference",
        "entry_ref",
        "version_ref",
        "content_hash",
    }
)
DEFAULT_THRESHOLDS = {
    "open_p95_ms": 5_000.0,
    "search_p95_ms": 3_000.0,
    "broad_search_p95_ms": 3_000.0,
    "concurrent_write_ms": 3_000.0,
    "concurrent_search_p95_ms": 3_000.0,
    "requested_checkpoints_per_minute": 1.0,
    "read_p95_ms": 1_000.0,
    "checkpoint_ms": 2_000.0,
    "resume_ms": 5_000.0,
    "max_batch_read_ms": 2_000.0,
    "max_checkpoint_sources_ms": 3_000.0,
    "checkpoint_row_growth": 100,
    "checkpoint_bytes_growth": 4 * 1024 * 1024,
    "ten_x_latency_growth": 6.0,
    "latency_growth_floor_ms": 1_000.0,
    "protocol_to_evidence_ratio": 1.0,
}


@dataclass(frozen=True)
class DatabaseSnapshot:
    size_bytes: int
    table_rows: dict[str, int]

    @property
    def total_rows(self) -> int:
        return sum(self.table_rows.values())


@dataclass(frozen=True)
class RunProfile:
    scales: tuple[int, ...]
    samples: int
    definitive: bool
    future_soak_requested: bool
    import_timeout_seconds: float
    semantic_failure_required: bool


def percentile(values: Iterable[float], quantile: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] * (1 - fraction) + ordered[upper] * fraction


def recursive_find(value: Any, key: str) -> Any:
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for item in value.values():
            found = recursive_find(item, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for item in value:
            found = recursive_find(item, key)
            if found is not None:
                return found
    return None


def response_timings(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    timings = value.get("timings_ms")
    return dict(timings) if isinstance(timings, dict) else {}


def flatten_numeric_timings(
    value: Any,
    *,
    prefix: str = "",
) -> dict[str, float]:
    flattened: dict[str, float] = {}
    if isinstance(value, dict):
        for key, item in value.items():
            child = f"{prefix}.{key}" if prefix else str(key)
            flattened.update(flatten_numeric_timings(item, prefix=child))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            child = f"{prefix}.{index}" if prefix else str(index)
            flattened.update(flatten_numeric_timings(item, prefix=child))
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        flattened[prefix] = float(value)
    return flattened


def summarize_timing_samples(
    samples: Sequence[dict[str, Any]],
) -> dict[str, dict[str, float]]:
    by_phase: dict[str, list[float]] = {}
    for sample in samples:
        for phase, value in flatten_numeric_timings(sample).items():
            by_phase.setdefault(phase, []).append(value)
    return {
        phase: {
            "samples": len(values),
            "p50": round(percentile(values, 0.50), 3),
            "p95": round(percentile(values, 0.95), 3),
            "p99": round(percentile(values, 0.99), 3),
        }
        for phase, values in sorted(by_phase.items())
    }


def timing_phase_sum_sane(sample: dict[str, Any]) -> bool:
    total = sample.get("total")
    if not isinstance(total, (int, float)) or total < 0:
        return False
    direct_phases = [
        float(value)
        for key, value in sample.items()
        if key not in {"total", "lanes", "queries"}
        and isinstance(value, (int, float))
        and not isinstance(value, bool)
    ]
    if any(value < 0 for value in direct_phases):
        return False
    return abs(sum(direct_phases) - float(total)) <= max(1.0, float(total) * 0.05)


def rendered_contains(value: Any, needle: str) -> bool:
    return needle.casefold() in json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
    ).casefold()


def source_text_contains(value: Any, needle: str) -> bool:
    folded = needle.casefold()

    def visit(item: Any, parent_key: str | None = None) -> bool:
        if isinstance(item, dict):
            return any(visit(child, key) for key, child in item.items())
        if isinstance(item, list):
            return any(visit(child, parent_key) for child in item)
        return bool(
            isinstance(item, str)
            and parent_key in SOURCE_TEXT_KEYS
            and folded in item.casefold()
        )

    return visit(value)


def response_reports_lane_failure(value: Any, lane: str) -> bool:
    if isinstance(value, dict):
        for key in ("lane_failures", "failed_lanes"):
            failures = value.get(key)
            if isinstance(failures, list) and any(
                str(item).casefold() == lane.casefold()
                for item in failures
            ):
                return True
        if (
            str(value.get("lane", "")).casefold() == lane.casefold()
            and "fail" in str(
                value.get("kind")
                or value.get("status")
                or value.get("message")
                or ""
            ).casefold()
        ):
            return True
        return any(
            response_reports_lane_failure(item, lane)
            for item in value.values()
        )
    if isinstance(value, list):
        return any(response_reports_lane_failure(item, lane) for item in value)
    return False


def response_reports_gap_kind(value: Any, kind: str) -> bool:
    if isinstance(value, dict):
        if str(value.get("kind", "")).casefold() == kind.casefold():
            return True
        return any(
            response_reports_gap_kind(item, kind)
            for item in value.values()
        )
    if isinstance(value, list):
        return any(response_reports_gap_kind(item, kind) for item in value)
    return False


def implementation_fingerprint(
    api_container: str | None,
) -> dict[str, Any]:
    repository = Path(__file__).resolve().parent

    def output(command: list[str]) -> str | None:
        completed = subprocess.run(
            command,
            cwd=repository,
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode != 0:
            return None
        value = completed.stdout.strip()
        return value or None

    revision = output(["git", "rev-parse", "HEAD"])
    tracked_diff = subprocess.run(
        ["git", "diff", "--quiet", "HEAD", "--"],
        cwd=repository,
        check=False,
    )
    untracked = output([
        "git",
        "ls-files",
        "--others",
        "--exclude-standard",
    ])
    untracked_source = [
        path
        for path in (untracked or "").splitlines()
        if not path.startswith(("results/", "runs/"))
    ]
    image_id = None
    image_revision = None
    if api_container:
        image_id = output([
            "docker",
            "inspect",
            "--format={{.Image}}",
            api_container,
        ])
        image_revision = output([
            "docker",
            "inspect",
            (
                "--format={{index .Config.Labels "
                '"org.opencontainers.image.revision"}}'
            ),
            api_container,
        ])
    return {
        "source_revision": revision,
        "tracked_source_clean": tracked_diff.returncode == 0,
        "untracked_source_files": untracked_source,
        "api_container": api_container,
        "api_image_id": image_id,
        "api_image_revision": image_revision,
        "reproducible": bool(
            revision
            and tracked_diff.returncode == 0
            and not untracked_source
            and image_id
            and image_revision == revision
        ),
    }


def synthetic_discovery_key(count: int) -> str:
    return f"terminal-corpus-{count}-current-answer"


def synthetic_discovery_task(count: int) -> str:
    key = synthetic_discovery_key(count)
    return (
        f"What exact current answer is recorded for the `{key}` discovery clue? "
        "Find and return source-backed current evidence without assuming a path."
    )


def response_character_metrics(value: Any) -> dict[str, int | float]:
    rendered = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    source_text_chars = 0
    source_identity_chars = 0

    def visit(item: Any, parent_key: str | None = None) -> None:
        nonlocal source_text_chars, source_identity_chars
        if isinstance(item, dict):
            for key, child in item.items():
                visit(child, key)
        elif isinstance(item, list):
            for child in item:
                visit(child, parent_key)
        elif isinstance(item, str) and parent_key in SOURCE_TEXT_KEYS:
            source_text_chars += len(item)
        elif isinstance(item, str) and parent_key in SOURCE_IDENTITY_KEYS:
            source_identity_chars += len(item)

    visit(value)
    payload_chars = len(rendered)
    metadata_chars = max(0, payload_chars - source_text_chars)
    evidence_chars = source_text_chars + source_identity_chars
    protocol_chars = max(0, payload_chars - evidence_chars)
    return {
        "payload_chars": payload_chars,
        "source_text_chars": source_text_chars,
        "source_identity_chars": source_identity_chars,
        "evidence_chars": evidence_chars,
        "metadata_chars": metadata_chars,
        "protocol_chars": protocol_chars,
        "estimated_payload_tokens": (payload_chars + 3) // 4,
        "estimated_source_tokens": (source_text_chars + 3) // 4,
        "estimated_metadata_tokens": (metadata_chars + 3) // 4,
        "metadata_to_source_ratio": round(
            metadata_chars / max(1, source_text_chars),
            6,
        ),
        "protocol_to_evidence_ratio": round(
            protocol_chars / max(1, evidence_chars),
            6,
        ),
    }


def summarize_response_accounting(
    samples: Sequence[tuple[str, dict[str, Any]]],
) -> dict[str, Any]:
    totals = {
        "payload_chars": 0,
        "source_text_chars": 0,
        "source_identity_chars": 0,
        "evidence_chars": 0,
        "metadata_chars": 0,
        "protocol_chars": 0,
    }
    by_operation: dict[str, dict[str, Any]] = {}
    for operation, body in samples:
        metrics = response_character_metrics(body)
        operation_metrics = by_operation.setdefault(
            operation,
            {
                "samples": 0,
                "payload_chars": 0,
                "source_text_chars": 0,
                "source_identity_chars": 0,
                "evidence_chars": 0,
                "metadata_chars": 0,
                "protocol_chars": 0,
            },
        )
        operation_metrics["samples"] += 1
        for key in totals:
            value = int(metrics[key])
            totals[key] += value
            operation_metrics[key] += value
    for metrics in by_operation.values():
        metrics["metadata_to_source_ratio"] = round(
            metrics["metadata_chars"] / max(1, metrics["source_text_chars"]),
            6,
        )
        metrics["protocol_to_evidence_ratio"] = round(
            metrics["protocol_chars"] / max(1, metrics["evidence_chars"]),
            6,
        )
    return {
        **totals,
        "estimated_payload_tokens": (totals["payload_chars"] + 3) // 4,
        "estimated_source_tokens": (totals["source_text_chars"] + 3) // 4,
        "estimated_metadata_tokens": (totals["metadata_chars"] + 3) // 4,
        "metadata_to_source_ratio": round(
            totals["metadata_chars"] / max(1, totals["source_text_chars"]),
            6,
        ),
        "protocol_to_evidence_ratio": round(
            totals["protocol_chars"] / max(1, totals["evidence_chars"]),
            6,
        ),
        "source_text_keys": sorted(SOURCE_TEXT_KEYS),
        "source_identity_keys": sorted(SOURCE_IDENTITY_KEYS),
        "token_estimate": "ceil(characters / 4); use character gates for pass/fail",
        "by_operation": by_operation,
    }


def resolve_run_profile(args: argparse.Namespace) -> RunProfile:
    definitive = not bool(args.quick)
    samples = (
        int(args.samples)
        if args.samples is not None
        else DEFINITIVE_SAMPLES if definitive else QUICK_SAMPLES
    )
    if samples < 1:
        raise ValueError("--samples must be at least 1")
    if definitive and samples < DEFINITIVE_SAMPLES:
        raise ValueError(
            f"definitive runs require at least {DEFINITIVE_SAMPLES} samples; "
            "use --quick for a deliberately non-definitive run"
        )
    if args.future_soak and not definitive:
        raise ValueError("--future-soak cannot be combined with --quick")

    scales: list[int] = list(args.scales or DEFAULT_SCALES)
    if any(scale < 2 for scale in scales):
        raise ValueError("every scale must contain at least two entries")
    if FUTURE_RECORDS in scales and not args.future_soak:
        raise ValueError(
            f"use --future-soak to run and clearly label the "
            f"{FUTURE_RECORDS:,}-entry future scale"
        )
    if definitive and PRODUCTION_RECORDS not in scales:
        raise ValueError(
            f"definitive runs must include the {PRODUCTION_RECORDS:,}-entry "
            "production shape"
        )
    if args.future_soak and FUTURE_RECORDS not in scales:
        scales.append(FUTURE_RECORDS)
    scales = sorted(set(scales))

    default_import_timeout = 7_200.0 if args.future_soak else 1_800.0
    import_timeout = (
        float(args.import_timeout)
        if args.import_timeout is not None
        else default_import_timeout
    )
    if import_timeout <= 0:
        raise ValueError("--import-timeout must be positive")
    return RunProfile(
        scales=tuple(scales),
        samples=samples,
        definitive=definitive,
        future_soak_requested=bool(args.future_soak),
        import_timeout_seconds=import_timeout,
        semantic_failure_required=definitive,
    )


def synthetic_documents(
    count: int,
    *,
    include_fixture_manifest: bool = False,
) -> Any:
    if count < 2:
        raise ValueError("scale must contain at least two documents")
    target_path = f"Synthetic/records/{count - 1:07d}.md"
    marker = f"narrow-fact-{count}-cobalt"
    discovery_key = synthetic_discovery_key(count)
    available_probe_indexes = [
        index
        for index in range(count)
        if index not in {0, count - 2, count - 1}
    ]
    selected_probe_indexes = available_probe_indexes[:VERBATIM_IDENTIFIER_PROBES]
    probe_number_by_index = {
        index: probe_number
        for probe_number, index in enumerate(selected_probe_indexes, start=1)
    }
    verbatim_identifiers: list[dict[str, Any]] = []
    documents: list[dict[str, Any]] = []
    for index in range(count):
        path = f"Synthetic/records/{index:07d}.md"
        topic = f"topic-{index % 257:03d}"
        body = (
            "---\n"
            f"title: Synthetic record {index:07d}\n"
            f"topic: {topic}\n"
            f"sequence: {index}\n"
            "---\n\n"
            f"# Synthetic record {index:07d}\n\n"
            "This is deterministic performance-fixture material. "
            f"It belongs to {topic} and shard {index % 31:02d}. "
            "The corpus is intentionally repetitive enough to exercise lexical "
            "candidate bounding without making every document identical.\n"
        )
        if index == 0:
            body += (
                "\n## Archived coordination doctrine\n\n"
                "The meridian continuity doctrine preserves durable workspace "
                "source authority across fresh-agent resumes. "
                f"Its exact audit answer is `{old_source_marker(count)}`.\n"
            )
        if index == count - 2 and index != 0:
            body += (
                "\n## Recent incomplete coordination lead\n\n"
                "A new request mentions the meridian continuity doctrine and "
                "durable workspace source authority, but this recent note does "
                "not contain the archived audit answer.\n"
            )
        if path == target_path:
            overflow_marker = lexical_overflow_marker(count)
            body += (
                "\n## Narrow fact\n\n"
                f"The `{discovery_key}` discovery clue has the exact current "
                f"answer `{marker}`. "
                "Only this document establishes that marker.\n\n"
                "## Broad relevance overflow\n\n"
                f"The `{overflow_marker}` clue makes this late-written source "
                f"more relevant to {BROAD_QUERY} than the earlier broad matches. "
                f"{BROAD_QUERY}. {BROAD_QUERY}.\n"
            )
        probe_number = probe_number_by_index.get(index)
        if probe_number is not None:
            identifier_hash = hashlib.sha256(
                f"{count}:{probe_number}:{path}".encode("utf-8")
            ).hexdigest()[:8]
            identifier = (
                f"STRAYID-{count}-{probe_number}-{identifier_hash}"
            )
            section_depth = 2 + (probe_number % 3)
            body += (
                "\n"
                + "#" * section_depth
                + f" Verbatim identifier probe {probe_number}\n\n"
            )
            requested_offset = (
                VERBATIM_IDENTIFIER_MIN_OFFSET
                + 199
                + (probe_number % 7) * 181
            )
            current_bytes = len(body.encode("utf-8"))
            identifier_offset = max(requested_offset, current_bytes + 1)
            body += "x" * (identifier_offset - current_bytes)
            assert len(body.encode("utf-8")) == identifier_offset
            body += identifier + "\n"
            if probe_number % 2 == 0:
                body += (
                    "Deterministic tail material follows the planted identifier. "
                    * 12
                )
            verbatim_identifiers.append({
                "path": path,
                "identifier": identifier,
                "byte_offset": identifier_offset,
                "position": "mid_document" if probe_number % 2 == 0 else "tail",
                "section_depth": section_depth,
            })
        digest = hashlib.sha256(body.encode("utf-8")).hexdigest()
        documents.append({
            "path": path,
            "content": body,
            "content_sha256": digest,
            "media_type": "text/markdown",
        })
    if include_fixture_manifest:
        return (
            documents,
            target_path,
            marker,
            {
                "schema": "straylight-synthetic-fixture@v2",
                "scale": count,
                "verbatim_identifiers": verbatim_identifiers,
            },
        )
    return documents, target_path, marker


def lexical_overflow_marker(scale: int) -> str:
    return f"overflow-broad-relevance-{scale}"


def old_source_marker(scale: int) -> str:
    return f"old-source-recall-{scale}-amber"


def materialize_flat_file_corpus(
    root: Path,
    documents: Sequence[dict[str, Any]],
) -> tuple[float, int]:
    started = time.monotonic()
    total_bytes = 0
    for document in documents:
        relative = Path(str(document["path"]))
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"unsafe synthetic path: {relative}")
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        content = str(document["content"])
        destination.write_text(content, encoding="utf-8")
        total_bytes += len(content.encode("utf-8"))
    return (time.monotonic() - started) * 1000, total_bytes


def deterministic_markdown_paths(root: Path) -> Iterator[Path]:
    for current_root, directories, filenames in os.walk(root):
        directories.sort()
        for filename in sorted(filenames):
            if filename.endswith(".md"):
                yield Path(current_root) / filename


def python_file_search(
    root: Path,
    needle: str,
    *,
    limit: int,
) -> list[str]:
    matches = []
    folded_needle = needle.casefold()
    for path in deterministic_markdown_paths(root):
        try:
            content = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if folded_needle in content.casefold():
            matches.append(path.relative_to(root).as_posix())
            if len(matches) >= limit:
                break
    return matches


def ripgrep_file_search(
    executable: str,
    root: Path,
    needle: str,
    *,
    limit: int,
    expect_unique: bool,
    timeout_seconds: float,
) -> list[str]:
    command = [
        executable,
        "--files-with-matches",
        "--fixed-strings",
        "--ignore-case",
        "--no-messages",
        "--glob",
        "*.md",
        "--",
        needle,
        str(root),
    ]
    if expect_unique:
        completed = subprocess.run(
            command,
            text=True,
            capture_output=True,
            timeout=timeout_seconds,
            check=False,
        )
        if completed.returncode not in {0, 1}:
            raise RuntimeError(
                f"flat-file rg discovery failed with exit {completed.returncode}"
            )
        lines = completed.stdout.splitlines()
    else:
        process = subprocess.Popen(
            command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        assert process.stdout is not None
        lines = []
        try:
            for line in process.stdout:
                lines.append(line.rstrip("\n"))
                if len(lines) >= limit:
                    process.terminate()
                    break
            try:
                process.wait(timeout=min(timeout_seconds, 5.0))
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        finally:
            process.stdout.close()
            if process.poll() is None:
                process.kill()
                process.wait()
        if process.returncode not in {0, 1, -15}:
            raise RuntimeError(
                f"flat-file rg broad search failed with exit {process.returncode}"
            )
    return [
        Path(line).relative_to(root).as_posix()
        for line in lines[:limit]
        if line
    ]


def flat_file_search(
    root: Path,
    needle: str,
    *,
    limit: int,
    expect_unique: bool,
    timeout_seconds: float,
) -> tuple[list[str], str]:
    executable = shutil.which("rg")
    if executable:
        return (
            ripgrep_file_search(
                executable,
                root,
                needle,
                limit=limit,
                expect_unique=expect_unique,
                timeout_seconds=timeout_seconds,
            ),
            "ripgrep",
        )
    return python_file_search(root, needle, limit=limit), "python"


def benchmark_flat_files(
    documents: Sequence[dict[str, Any]],
    *,
    scale: int,
    target_path: str,
    marker: str,
    samples: int,
    timeout_seconds: float,
) -> dict[str, Any]:
    discovery_key = synthetic_discovery_key(scale)
    discovery_times = []
    discovery_found = []
    broad_times = []
    broad_found = []
    read_times = []
    read_found = []
    engines: set[str] = set()
    with tempfile.TemporaryDirectory(
        prefix=f"straylight-flat-{scale}-",
    ) as temporary:
        root = Path(temporary)
        materialize_ms, corpus_bytes = materialize_flat_file_corpus(
            root,
            documents,
        )
        exact_path = root / target_path
        for _ in range(samples):
            started = time.monotonic()
            matches, engine = flat_file_search(
                root,
                discovery_key,
                limit=8,
                expect_unique=True,
                timeout_seconds=timeout_seconds,
            )
            discovery_times.append((time.monotonic() - started) * 1000)
            engines.add(engine)
            discovery_found.append(
                target_path in matches
                and marker in exact_path.read_text(encoding="utf-8")
            )

            started = time.monotonic()
            content = exact_path.read_text(encoding="utf-8")
            read_times.append((time.monotonic() - started) * 1000)
            read_found.append(marker in content)

            started = time.monotonic()
            matches, engine = flat_file_search(
                root,
                BROAD_QUERY,
                limit=8,
                expect_unique=False,
                timeout_seconds=timeout_seconds,
            )
            broad_times.append((time.monotonic() - started) * 1000)
            engines.add(engine)
            broad_found.append(
                bool(matches)
                and all(path.startswith("Synthetic/records/") for path in matches)
            )
    return {
        "engine": "+".join(sorted(engines)),
        "files": len(documents),
        "corpus_bytes": corpus_bytes,
        "materialize_ms": round(materialize_ms, 3),
        "samples": samples,
        "discovery_query": discovery_key,
        "discovery_path_was_provided": False,
        "discovery_ms": [round(value, 3) for value in discovery_times],
        "discovery_p95_ms": round(percentile(discovery_times, 0.95), 3),
        "discovery_found": discovery_found,
        "read_ms": [round(value, 3) for value in read_times],
        "read_p95_ms": round(percentile(read_times, 0.95), 3),
        "read_found": read_found,
        "broad_query": BROAD_QUERY,
        "broad_search_ms": [round(value, 3) for value in broad_times],
        "broad_search_p95_ms": round(percentile(broad_times, 0.95), 3),
        "broad_found": broad_found,
    }


def run_psql(container: str, sql: str) -> str:
    completed = subprocess.run(
        [
            "docker",
            "exec",
            "-i",
            container,
            "psql",
            "-X",
            "-q",
            "-t",
            "-A",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            os.environ.get("POSTGRES_USER", "admin"),
            "-d",
            os.environ.get("POSTGRES_DB", "straylight"),
        ],
        input=sql,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"database snapshot failed in {container}: {completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def database_snapshot(container: str) -> DatabaseSnapshot:
    sql = r"""
CREATE TEMP TABLE benchmark_counts(table_name text PRIMARY KEY, row_count bigint);
DO $$
DECLARE item record;
BEGIN
  FOR item IN
    SELECT schemaname, tablename
    FROM pg_tables
    WHERE schemaname = 'straylight'
    ORDER BY tablename
  LOOP
    EXECUTE format(
      'INSERT INTO benchmark_counts SELECT %L, count(*) FROM %I.%I',
      item.tablename,
      item.schemaname,
      item.tablename
    );
  END LOOP;
END
$$;
SELECT json_build_object(
  'size_bytes', pg_database_size(current_database()),
  'table_rows', COALESCE(
    (SELECT json_object_agg(table_name, row_count) FROM benchmark_counts),
    '{}'::json
  )
);
"""
    payload = json.loads(run_psql(container, sql).splitlines()[-1])
    return DatabaseSnapshot(
        size_bytes=int(payload["size_bytes"]),
        table_rows={
            str(key): int(value)
            for key, value in payload["table_rows"].items()
        },
    )


def index_scan_snapshot(container: str) -> dict[str, int]:
    payload = run_psql(
        container,
        r"""
SELECT pg_stat_force_next_flush();
SELECT COALESCE(
  json_object_agg(indexrelname,idx_scan),
  '{}'::json
)
FROM pg_stat_user_indexes
WHERE schemaname='straylight'
  AND indexrelname IN (
    'search_chunks_fts_idx',
    'search_chunks_embedding_hnsw_idx'
  );
""",
    )
    return {
        str(key): int(value)
        for key, value in json.loads(payload.splitlines()[-1]).items()
    }


def checkpointer_snapshot(container: str) -> dict[str, Any]:
    payload = run_psql(
        container,
        r"""
SELECT json_build_object(
  'num_timed',num_timed,
  'num_requested',num_requested,
  'write_time_ms',write_time,
  'sync_time_ms',sync_time,
  'buffers_written',buffers_written,
  'max_wal_size',current_setting('max_wal_size'),
  'min_wal_size',current_setting('min_wal_size'),
  'wal_compression',current_setting('wal_compression')
)
FROM pg_stat_checkpointer;
""",
    )
    return json.loads(payload.splitlines()[-1])


def counter_growth(before: dict[str, int], after: dict[str, int]) -> dict[str, int]:
    return {
        key: after.get(key, 0) - before.get(key, 0)
        for key in sorted(set(before) | set(after))
    }


def table_growth(
    before: DatabaseSnapshot,
    after: DatabaseSnapshot,
) -> dict[str, int]:
    names = set(before.table_rows) | set(after.table_rows)
    return {
        name: after.table_rows.get(name, 0) - before.table_rows.get(name, 0)
        for name in sorted(names)
        if after.table_rows.get(name, 0) != before.table_rows.get(name, 0)
    }


def simple_checkpoint_footprint(
    container: str,
    checkpoint_id: str,
) -> dict[str, Any]:
    parsed = uuid.UUID(checkpoint_id.removeprefix("checkpoint:"))
    payload = run_psql(
        container,
        f"""
WITH footprint AS (
  SELECT 'entries'::text AS table_name,count(*)::bigint AS rows,
         coalesce(sum(pg_column_size(item)),0)::bigint AS bytes
  FROM straylight.entries AS item
  WHERE item.id='{parsed}'::uuid
  UNION ALL
  SELECT 'entry_versions',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM straylight.entry_versions AS item
  WHERE item.entry_id='{parsed}'::uuid
  UNION ALL
  SELECT 'workspace_changes',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM straylight.workspace_changes AS item
  WHERE item.entry_id='{parsed}'::uuid
  UNION ALL
  SELECT 'search_chunks',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM straylight.search_chunks AS item
  WHERE item.entry_id='{parsed}'::uuid
  UNION ALL
  SELECT 'jobs',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM straylight.jobs AS item
  WHERE item.payload->>'entry_id'='{parsed}'
)
SELECT json_build_object(
  'rows',coalesce(sum(rows),0),
  'bytes',coalesce(sum(bytes),0),
  'tables',coalesce(
    json_object_agg(table_name,rows) FILTER (WHERE rows <> 0),
    '{{}}'::json
  )
)
FROM footprint;
""",
    )
    result = json.loads(payload.splitlines()[-1])
    return {
        "rows": int(result["rows"]),
        "bytes": int(result["bytes"]),
        "tables": {
            str(key): int(value)
            for key, value in result["tables"].items()
        },
    }


def request_with_result(
    client: NativeApiClient,
    path: str,
    payload: dict[str, Any],
) -> tuple[dict[str, Any], float]:
    response = client.post(path, payload)
    return response.body, response.elapsed_ms


def run_external_hook(
    command_text: str,
    *,
    timeout_seconds: float,
) -> dict[str, Any]:
    started = time.monotonic()
    try:
        command = shlex.split(command_text)
    except ValueError:
        return {
            "command": None,
            "exit_code": None,
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
            "pass": False,
            "error": "invalid_command_syntax",
        }
    if not command:
        return {
            "command": None,
            "exit_code": None,
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
            "pass": False,
            "error": "empty_command",
        }
    try:
        completed = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=timeout_seconds,
            check=False,
        )
        return {
            "command": command[0],
            "exit_code": completed.returncode,
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
            "pass": completed.returncode == 0,
        }
    except subprocess.TimeoutExpired:
        return {
            "command": command[0],
            "exit_code": None,
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
            "pass": False,
            "error": "timeout",
        }
    except OSError as error:
        return {
            "command": command[0],
            "exit_code": None,
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
            "pass": False,
            "error": type(error).__name__,
        }


def semantic_failure_probe(
    client: NativeApiClient,
    *,
    protocol: str,
    authorization_scope: str,
    session_id: str,
    scale: int,
    marker: str,
    required: bool,
    start_command: str | None,
    stop_command: str | None,
    settle_seconds: float,
    timeout_seconds: float,
) -> dict[str, Any]:
    required_arguments = [
        "--semantic-failure-start-command",
        "--semantic-failure-stop-command",
    ]
    if not start_command or not stop_command:
        return {
            "status": "not_run",
            "pass": False if required else None,
            "required": required,
            "reason": (
                "The black-box API cannot force an embedding-provider outage. "
                "Run against a disposable stack and supply both external hook "
                "commands; the stop hook must restore the provider."
            ),
            "required_arguments": required_arguments,
        }
    operation_prefix = "/v1/workspace" if protocol == "simple" else "/v1/memory"
    search_path = (
        f"{operation_prefix}/search"
        if protocol == "simple"
        else f"{operation_prefix}/query"
    )
    discovery_key = synthetic_discovery_key(scale)

    def search(query_id: str, query: str, modes: list[str]) -> dict[str, Any]:
        body, _ = request_with_result(
            client,
            search_path,
            {
                "session_id": session_id,
                "queries": [{
                    "id": query_id,
                    "goal": "locate the current terminal-corpus answer",
                    "query": query,
                    "scope": authorization_scope,
                    "modes": modes,
                    "limit": 8,
                }],
            },
        )
        return body

    try:
        baseline = search(
            "semantic-provider-baseline",
            f"Semantically locate the answer associated with {discovery_key}.",
            ["semantic"],
        )
    except NativeApiError as error:
        return {
            "status": "baseline_failed",
            "pass": False,
            "required": required,
            "reason": "semantic-only retrieval did not work before injection",
            "baseline_http_status": error.status,
        }
    baseline_healthy = not response_reports_lane_failure(baseline, "semantic")
    if not baseline_healthy:
        return {
            "status": "baseline_failed",
            "pass": False,
            "required": required,
            "reason": (
                "the semantic lane reported a failure before failure injection"
            ),
            "baseline_semantic_lane_healthy": False,
        }

    start_result = run_external_hook(
        start_command,
        timeout_seconds=timeout_seconds,
    )
    restore_result: dict[str, Any] | None = None
    semantic_failure_observed = False
    semantic_status: int | None = None
    lexical_found = False
    mixed_found = False
    restored_found = False
    probe_error: str | None = None
    try:
        if not start_result["pass"]:
            probe_error = "semantic failure start hook failed"
        else:
            time.sleep(max(0.0, settle_seconds))
            try:
                failed_semantic = search(
                    "semantic-provider-outage",
                    (
                        "During the provider probe, semantically retrieve the "
                        f"answer associated with {discovery_key}."
                    ),
                    ["semantic"],
                )
                semantic_failure_observed = response_reports_lane_failure(
                    failed_semantic,
                    "semantic",
                )
            except NativeApiError as error:
                semantic_failure_observed = True
                semantic_status = error.status

            lexical = search(
                "provider-outage-lexical",
                discovery_key,
                ["exact", "lexical"],
            )
            lexical_found = rendered_contains(lexical, marker)
            mixed = search(
                "provider-outage-mixed",
                discovery_key,
                ["exact", "lexical", "semantic"],
            )
            mixed_found = rendered_contains(mixed, marker)
    except (NativeApiError, RuntimeError) as error:
        probe_error = f"{type(error).__name__}: {error}"
    finally:
        restore_result = run_external_hook(
            stop_command,
            timeout_seconds=timeout_seconds,
        )
        time.sleep(max(0.0, settle_seconds))
        if restore_result["pass"]:
            try:
                restored = search(
                    "semantic-provider-restored",
                    (
                        "After provider restoration, semantically locate the "
                        f"answer associated with {discovery_key}."
                    ),
                    ["semantic"],
                )
                restored_found = not response_reports_lane_failure(
                    restored,
                    "semantic",
                )
            except NativeApiError:
                restored_found = False

    passed = bool(
        start_result["pass"]
        and restore_result["pass"]
        and restored_found
        and semantic_failure_observed
        and lexical_found
        and mixed_found
        and probe_error is None
    )
    return {
        "status": "passed" if passed else "failed",
        "pass": passed,
        "required": required,
        "baseline_semantic_lane_healthy": baseline_healthy,
        "semantic_failure_observed": semantic_failure_observed,
        "semantic_failure_http_status": semantic_status,
        "exact_lexical_found_during_failure": lexical_found,
        "mixed_lane_found_during_failure": mixed_found,
        "semantic_lane_healthy_after_restore": restored_found,
        "start_hook": start_result,
        "restore_hook": restore_result,
        "error": probe_error,
    }


def verbatim_identifier_probe(
    client: NativeApiClient,
    *,
    protocol: str,
    authorization_scope: str,
    session_id: str,
    probes: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    if protocol != "simple":
        return {
            "status": "not_applicable_protocol",
            "expected": 0,
            "returned": 0,
            "pass": None,
            "results": [],
        }
    results = []
    for index, probe in enumerate(probes, start=1):
        response, elapsed_ms = request_with_result(
            client,
            "/v1/workspace/search",
            {
                "session_id": session_id,
                "queries": [{
                    "id": f"verbatim-identifier-{index}",
                    "goal": "return the literal identifier from the exact path",
                    "query": f"{probe['path']} {probe['identifier']}",
                    "scope": authorization_scope,
                    "modes": ["exact"],
                    "limit": 1,
                }],
            },
        )
        present = source_text_contains(response, str(probe["identifier"]))
        results.append({
            **probe,
            "modes": ["exact"],
            "verbatim_in_source_payload": present,
            "elapsed_ms": round(elapsed_ms, 3),
            "payload_chars": response_character_metrics(response)["payload_chars"],
        })
    returned = sum(
        bool(item["verbatim_in_source_payload"])
        for item in results
    )
    expected = len(results)
    return {
        "status": "complete",
        "expected": expected,
        "returned": returned,
        "pass": returned == expected,
        "results": results,
    }


def concurrent_write_search_probe(
    client: NativeApiClient,
    *,
    protocol: str,
    authorization_scope: str,
    session_id: str,
    marker: str,
    run_id: str,
    searches: int = 5,
    rounds: int = 1,
    response_samples: list[tuple[str, dict[str, Any]]] | None = None,
) -> dict[str, Any]:
    if searches < 1 or rounds < 1:
        raise ValueError("concurrent probe requires at least one search and round")
    operation_prefix = "/v1/workspace" if protocol == "simple" else "/v1/memory"
    write_times: list[float] = []
    write_committed_samples: list[bool] = []
    search_times: list[float] = []
    search_found: list[bool] = []
    search_lane_failures: list[dict[str, bool]] = []

    for round_index in range(rounds):
        barrier = threading.Barrier(searches + 1)
        write_marker = f"unrelated-write-{uuid.uuid4().hex}"
        path = f"Synthetic/concurrent/{write_marker}.md"
        content = (
            "# Concurrent write probe\n\n"
            f"This unrelated file contains `{write_marker}`.\n"
        )

        def write() -> tuple[dict[str, Any], float]:
            barrier.wait()
            if protocol == "simple":
                return request_with_result(
                    client,
                    f"{operation_prefix}/write",
                    {
                        "path": path,
                        "content": content,
                        "media_type": "text/markdown",
                        "metadata": {"kind": "performance_probe"},
                    },
                )
            return request_with_result(
                client,
                f"{operation_prefix}/save",
                {
                    "intent": "measure retrieval during an unrelated write",
                    "scope": authorization_scope,
                    "root_refs": [],
                    "source_refs": [],
                    "idempotency_key": f"{run_id}:{write_marker}",
                    "items": [{
                        "action": "create",
                        "kind": "source",
                        "ref": f"performance-probe:{write_marker}",
                        "payload": {
                            "path": path,
                            "source_ref": f"performance-probe:{write_marker}",
                            "source_kind": "performance_probe",
                            "source_version": (
                                "sha256:"
                                + hashlib.sha256(content.encode("utf-8")).hexdigest()
                            ),
                            "title": "Concurrent write probe",
                            "media_type": "text/markdown",
                            "content": content,
                        },
                    }],
                },
            )

        def search(index: int) -> tuple[dict[str, Any], float]:
            barrier.wait()
            return request_with_result(
                client,
                (
                    f"{operation_prefix}/search"
                    if protocol == "simple"
                    else f"{operation_prefix}/query"
                ),
                {
                    "session_id": session_id,
                    "queries": [{
                        "id": f"concurrent-{round_index}-{index}",
                        "goal": "find the existing exact marker",
                        "query": marker,
                        "scope": authorization_scope,
                        "modes": ["exact", "lexical", "semantic"],
                        "limit": 8,
                    }],
                },
            )

        with concurrent.futures.ThreadPoolExecutor(
            max_workers=searches + 1,
        ) as executor:
            write_future = executor.submit(write)
            search_futures = [
                executor.submit(search, index)
                for index in range(searches)
            ]
            write_body, write_ms = write_future.result()
            search_results = [future.result() for future in search_futures]
        if response_samples is not None:
            response_samples.extend(
                ("concurrent_search", body)
                for body, _ in search_results
            )
        write_times.append(write_ms)
        write_committed_samples.append(
            rendered_contains(write_body, write_marker)
        )
        search_times.extend(elapsed for _, elapsed in search_results)
        search_found.extend(
            rendered_contains(body, marker)
            for body, _ in search_results
        )
        search_lane_failures.extend(
            {
                "exact": response_reports_lane_failure(body, "exact"),
                "lexical": response_reports_lane_failure(body, "lexical"),
            }
            for body, _ in search_results
        )

    write_p95_ms = percentile(write_times, 0.95)
    return {
        "rounds": rounds,
        "searches_per_round": searches,
        "write_ms": round(write_p95_ms, 3),
        "write_samples_ms": [round(value, 3) for value in write_times],
        "write_p50_ms": round(percentile(write_times, 0.50), 3),
        "write_p95_ms": round(write_p95_ms, 3),
        "write_max_ms": round(max(write_times), 3),
        "write_committed": all(write_committed_samples),
        "write_committed_samples": write_committed_samples,
        "search_ms": [round(value, 3) for value in search_times],
        "search_p95_ms": round(percentile(search_times, 0.95), 3),
        "search_found": search_found,
        "search_lane_failures": search_lane_failures,
    }


def benchmark_scale(
    admin: NativeApiClient,
    *,
    label: str,
    scale: int,
    samples: int,
    timeout_seconds: float,
    import_timeout_seconds: float,
    db_container: str | None,
    protocol: str,
    run_semantic_failure: bool,
    concurrent_rounds: int,
    semantic_failure_required: bool,
    semantic_failure_start_command: str | None,
    semantic_failure_stop_command: str | None,
    semantic_failure_settle_seconds: float,
    wait_for_semantic: bool,
    unique_queries: bool,
    flat_result_callback: Callable[[dict[str, Any]], None] | None = None,
    flat_file_control_override: dict[str, Any] | None = None,
    flat_file_control_source: str | None = None,
) -> dict[str, Any]:
    scale_started = time.monotonic()
    (
        documents,
        target_path,
        marker,
        fixture_manifest,
    ) = synthetic_documents(scale, include_fixture_manifest=True)
    discovery_key = synthetic_discovery_key(scale)
    flat_file_control = (
        dict(flat_file_control_override)
        if flat_file_control_override is not None
        else benchmark_flat_files(
            documents,
            scale=scale,
            target_path=target_path,
            marker=marker,
            samples=samples,
            timeout_seconds=max(timeout_seconds, 300.0),
        )
    )
    if flat_result_callback is not None:
        flat_result_callback(flat_file_control)
    checkpointer_before = (
        checkpointer_snapshot(db_container)
        if db_container and protocol == "simple"
        else None
    )
    checkpointer_started = time.monotonic()
    case_id = f"scale-{scale}"
    run_id = f"perf-{label}-{int(time.time())}-{scale}"
    started = time.monotonic()
    provisioning = provision_evaluation(
        admin,
        run_id=run_id,
        case_id=case_id,
        display_scope=f"Performance {scale}",
        access_mode="read_write",
        documents=documents,
        timeout_seconds=import_timeout_seconds,
        import_path=(
            "/v1/workspace/admin/eval/import"
            if protocol == "simple"
            else "/v1/admin/eval/import"
        ),
        wait_for_semantic=wait_for_semantic or protocol != "simple",
        batch_size=10_000 if protocol == "simple" else None,
    )
    import_ms = (time.monotonic() - started) * 1000
    client = NativeApiClient(
        base_url=admin.base_url,
        token=provisioning["token"],
        run_id=run_id,
        case_id=case_id,
        timeout=timeout_seconds,
    )
    authorization_scope = provisioning["authorization_scope"]
    task = synthetic_discovery_task(scale)
    operation_prefix = "/v1/workspace" if protocol == "simple" else "/v1/memory"
    index_before = (
        index_scan_snapshot(db_container)
        if db_container and protocol == "simple"
        else None
    )
    open_times: list[float] = []
    search_times: list[float] = []
    broad_search_times: list[float] = []
    overflow_search_times: list[float] = []
    old_source_search_times: list[float] = []
    read_times: list[float] = []
    open_found: list[bool] = []
    search_found: list[bool] = []
    broad_found: list[bool] = []
    overflow_found: list[bool] = []
    old_source_found: list[bool] = []
    old_source_semantic_deferred: list[bool] = []
    read_found: list[bool] = []
    critical_lane_failures: list[dict[str, Any]] = []
    response_samples: list[tuple[str, dict[str, Any]]] = []
    open_timing_samples: list[dict[str, Any]] = []
    search_timing_samples: list[dict[str, Any]] = []
    broad_timing_samples: list[dict[str, Any]] = []
    latest_session_id = ""

    for sample_index in range(samples):
        query_suffix = (
            f" e03-query-{sample_index:04d}-{uuid.uuid4().hex[:8]}"
            if unique_queries
            else ""
        )
        opened, elapsed = request_with_result(
            client,
            f"{operation_prefix}/open",
            {
                "task": task,
                "hints": {
                    "authorization_scope": authorization_scope,
                    "root_refs": [],
                    "open_object_refs": [],
                },
                "mode": "continuation",
                "as_of": "latest",
                "token_budget": 4_000,
            },
        )
        open_times.append(elapsed)
        open_found.append(rendered_contains(opened, marker))
        critical_lane_failures.append({
            "operation": "open",
            "exact": response_reports_lane_failure(opened, "exact"),
            "lexical": response_reports_lane_failure(opened, "lexical"),
        })
        response_samples.append(("open", opened))
        open_timing_samples.append(response_timings(opened))
        latest_session_id = str(recursive_find(opened, "session_id") or "")
        if not latest_session_id:
            raise RuntimeError(f"open omitted session_id at scale {scale}")

        searched, elapsed = request_with_result(
            client,
            (
                f"{operation_prefix}/search"
                if protocol == "simple"
                else f"{operation_prefix}/query"
            ),
            {
                "session_id": latest_session_id,
                "queries": [{
                    "id": "unknown-path-discovery",
                    "goal": "locate the terminal corpus answer without a path",
                    "query": discovery_key + query_suffix,
                    "scope": authorization_scope,
                    "modes": ["exact", "lexical", "semantic"],
                    "limit": 8,
                }],
            },
        )
        search_times.append(elapsed)
        search_found.append(rendered_contains(searched, marker))
        critical_lane_failures.append({
            "operation": "search",
            "exact": response_reports_lane_failure(searched, "exact"),
            "lexical": response_reports_lane_failure(searched, "lexical"),
        })
        response_samples.append(("search", searched))
        search_timing_samples.append(response_timings(searched))

        broad, elapsed = request_with_result(
            client,
            (
                f"{operation_prefix}/search"
                if protocol == "simple"
                else f"{operation_prefix}/query"
            ),
            {
                "session_id": latest_session_id,
                "queries": [{
                    "id": "broad",
                    "goal": "find representative performance fixture sources",
                    "query": BROAD_QUERY + query_suffix,
                    "scope": authorization_scope,
                    "modes": ["lexical", "semantic"],
                    "limit": 8,
                }],
            },
        )
        broad_search_times.append(elapsed)
        broad_found.append(rendered_contains(broad, "Synthetic/records/"))
        critical_lane_failures.append({
            "operation": "broad_search",
            "exact": False,
            "lexical": response_reports_lane_failure(broad, "lexical"),
        })
        response_samples.append(("broad_search", broad))
        broad_timing_samples.append(response_timings(broad))

        overflow_marker = lexical_overflow_marker(scale)
        overflow, elapsed = request_with_result(
            client,
            (
                f"{operation_prefix}/search"
                if protocol == "simple"
                else f"{operation_prefix}/query"
            ),
            {
                "session_id": latest_session_id,
                "queries": [{
                    "id": "bounded-overflow",
                    "goal": "find the relevant late source among broad matches",
                    "query": f"{BROAD_QUERY} OR {overflow_marker}",
                    "scope": authorization_scope,
                    "modes": ["exact", "lexical", "semantic"],
                    "limit": 8,
                }],
            },
        )
        overflow_search_times.append(elapsed)
        overflow_found.append(rendered_contains(overflow, overflow_marker))
        critical_lane_failures.append({
            "operation": "bounded_overflow_search",
            "exact": response_reports_lane_failure(overflow, "exact"),
            "lexical": response_reports_lane_failure(overflow, "lexical"),
        })
        response_samples.append(("bounded_overflow_search", overflow))

        old_marker = old_source_marker(scale)
        old_source, elapsed = request_with_result(
            client,
            (
                f"{operation_prefix}/search"
                if protocol == "simple"
                else f"{operation_prefix}/query"
            ),
            {
                "session_id": latest_session_id,
                "queries": [{
                    "id": "old-source-recall",
                    "goal": "find an older relevant source after many newer writes",
                    "query": OLD_SOURCE_QUERY,
                    "scope": authorization_scope,
                    "modes": ["lexical", "semantic"],
                    "limit": 8,
                }],
            },
        )
        old_source_search_times.append(elapsed)
        old_source_found.append(rendered_contains(old_source, old_marker))
        old_source_semantic_deferred.append(
            response_reports_gap_kind(old_source, "retrieval_lane_deferred")
        )
        critical_lane_failures.append({
            "operation": "old_source_search",
            "exact": False,
            "lexical": response_reports_lane_failure(old_source, "lexical"),
        })
        response_samples.append(("old_source_search", old_source))

        read, elapsed = request_with_result(
            client,
            f"{operation_prefix}/read",
            {
                "session_id": latest_session_id,
                "requests": [{
                    "path": target_path,
                    "view": "full",
                    "max_chars": 8_000,
                }],
            },
        )
        read_times.append(elapsed)
        read_found.append(rendered_contains(read, marker))
        response_samples.append(("read", read))

    verbatim_probe = verbatim_identifier_probe(
        client,
        protocol=protocol,
        authorization_scope=authorization_scope,
        session_id=latest_session_id,
        probes=fixture_manifest["verbatim_identifiers"],
    )

    before_checkpoint = (
        database_snapshot(db_container)
        if db_container and protocol != "simple"
        else None
    )
    checkpoint, checkpoint_ms = request_with_result(
        client,
        f"{operation_prefix}/checkpoint",
        {
            "session_id": latest_session_id,
            "idempotency_key": f"{run_id}:checkpoint",
            "state": {
                "objective": "Resume the narrow marker task.",
                "current_state": [f"Found {marker} in {target_path}."],
                "decisions": ["Treat the exact file as current truth."],
                "open_questions": [],
                "next_actions": ["Read changes since this checkpoint."],
                "artifacts": [target_path],
            },
            "source_refs": [],
        },
    )
    checkpoint_id = str(
        recursive_find(checkpoint, "checkpoint_id")
        or recursive_find(checkpoint, "checkpoint_ref")
        or recursive_find(checkpoint, "id")
        or ""
    )
    if checkpoint_id and not checkpoint_id.startswith("checkpoint:"):
        checkpoint_id = f"checkpoint:{checkpoint_id}"
    after_checkpoint = (
        database_snapshot(db_container)
        if db_container and protocol != "simple"
        else None
    )
    resume_payload = {
        "task": f"Resume and report the current marker for {target_path}.",
        "hints": {
            "authorization_scope": authorization_scope,
            "root_refs": [],
            "open_object_refs": [],
        },
        "mode": "continuation",
        "as_of": "latest",
        "token_budget": 4_000,
    }
    if checkpoint_id:
        resume_payload["resume_checkpoint_ref"] = checkpoint_id
    resume_times: list[float] = []
    resume_found_samples: list[bool] = []
    resume_timing_samples: list[dict[str, Any]] = []
    resumed: dict[str, Any] = {}
    for _ in range(samples):
        resumed, resume_elapsed = request_with_result(
            client,
            f"{operation_prefix}/open",
            resume_payload,
        )
        resume_times.append(resume_elapsed)
        resume_found_samples.append(rendered_contains(resumed, marker))
        resume_timing_samples.append(response_timings(resumed))
        response_samples.append(("resume", resumed))
    resume_ms = percentile(resume_times, 0.95)
    boundary_probe: dict[str, Any] = {"status": "not_applicable"}
    if protocol == "simple" and scale >= PRODUCTION_RECORDS:
        batch_paths = [str(document["path"]) for document in documents[:32]]
        batch_read, batch_read_ms = request_with_result(
            client,
            f"{operation_prefix}/read",
            {
                "session_id": latest_session_id,
                "requests": [
                    {
                        "path": path,
                        "view": "full",
                        "max_chars": 8_000,
                    }
                    for path in batch_paths
                ],
            },
        )
        response_samples.append(("max_batch_read", batch_read))
        batch_items = recursive_find(batch_read, "items")
        source_paths = [str(document["path"]) for document in documents[:64]]
        max_checkpoint, max_checkpoint_ms = request_with_result(
            client,
            f"{operation_prefix}/checkpoint",
            {
                "session_id": latest_session_id,
                "idempotency_key": f"{run_id}:max-checkpoint-sources",
                "state": {
                    "objective": "Exercise the bounded checkpoint source limit.",
                    "current_state": [f"Corpus scale is {scale}."],
                    "decisions": [],
                    "open_questions": [],
                    "next_actions": [],
                    "artifacts": [target_path],
                },
                "source_refs": source_paths,
            },
        )
        response_samples.append(("max_checkpoint_sources", max_checkpoint))
        checkpoint_sources = recursive_find(max_checkpoint, "source_entries")
        boundary_probe = {
            "status": "complete",
            "batch_read_entries": (
                len(batch_items) if isinstance(batch_items, list) else 0
            ),
            "batch_read_ms": round(batch_read_ms, 3),
            "checkpoint_source_entries": (
                len(checkpoint_sources)
                if isinstance(checkpoint_sources, list)
                else 0
            ),
            "checkpoint_ms": round(max_checkpoint_ms, 3),
            "pass": (
                isinstance(batch_items, list)
                and len(batch_items) == len(batch_paths)
                and isinstance(checkpoint_sources, list)
                and len(checkpoint_sources) == len(source_paths)
            ),
        }
    concurrent_probe = concurrent_write_search_probe(
        client,
        protocol=protocol,
        authorization_scope=authorization_scope,
        session_id=latest_session_id,
        marker=marker,
        run_id=run_id,
        rounds=concurrent_rounds,
        response_samples=response_samples,
    )
    failure_probe = (
        semantic_failure_probe(
            client,
            protocol=protocol,
            authorization_scope=authorization_scope,
            session_id=latest_session_id,
            scale=scale,
            marker=marker,
            required=semantic_failure_required,
            start_command=semantic_failure_start_command,
            stop_command=semantic_failure_stop_command,
            settle_seconds=semantic_failure_settle_seconds,
            timeout_seconds=max(timeout_seconds, 30.0),
        )
        if run_semantic_failure
        else {
            "status": "not_applicable_at_this_scale",
            "pass": None,
            "required": False,
        }
    )
    index_after = (
        index_scan_snapshot(db_container)
        if index_before is not None and db_container is not None
        else None
    )
    checkpointer_after = (
        checkpointer_snapshot(db_container)
        if checkpointer_before is not None and db_container is not None
        else None
    )
    checkpointer_elapsed_seconds = time.monotonic() - checkpointer_started
    checkpoint_pressure = None
    if checkpointer_before is not None and checkpointer_after is not None:
        requested = (
            int(checkpointer_after["num_requested"])
            - int(checkpointer_before["num_requested"])
        )
        timed = (
            int(checkpointer_after["num_timed"])
            - int(checkpointer_before["num_timed"])
        )
        checkpoint_pressure = {
            "elapsed_seconds": round(checkpointer_elapsed_seconds, 3),
            "requested_checkpoints": requested,
            "timed_checkpoints": timed,
            "requested_checkpoints_per_minute": round(
                requested / max(checkpointer_elapsed_seconds, 0.001) * 60.0,
                3,
            ),
            "write_time_ms": (
                int(checkpointer_after["write_time_ms"])
                - int(checkpointer_before["write_time_ms"])
            ),
            "sync_time_ms": (
                int(checkpointer_after["sync_time_ms"])
                - int(checkpointer_before["sync_time_ms"])
            ),
            "buffers_written": (
                int(checkpointer_after["buffers_written"])
                - int(checkpointer_before["buffers_written"])
            ),
            "settings": {
                key: checkpointer_after[key]
                for key in ("max_wal_size", "min_wal_size", "wal_compression")
            },
        }
    status_url = provisioning.get("status_url")
    semantic_status_end: dict[str, Any] = {}
    if protocol == "simple" and isinstance(status_url, str) and status_url:
        semantic_status_end = client.get(status_url).data
    checkpoint_growth = (
        table_growth(before_checkpoint, after_checkpoint)
        if before_checkpoint and after_checkpoint
        else {}
    )
    if db_container and protocol == "simple" and checkpoint_id:
        checkpoint_footprint = simple_checkpoint_footprint(
            db_container,
            checkpoint_id,
        )
    else:
        checkpoint_footprint = {
            "bytes": (
                after_checkpoint.size_bytes - before_checkpoint.size_bytes
                if before_checkpoint and after_checkpoint
                else None
            ),
            "rows": sum(checkpoint_growth.values()),
            "tables": checkpoint_growth,
        }
    return {
        "scale": scale,
        "protocol": protocol,
        "documents": scale,
        "target_path": target_path,
        "marker": marker,
        "fixture_manifest": fixture_manifest,
        "verbatim_identifier": verbatim_probe,
        "scale_elapsed_ms": round(
            (time.monotonic() - scale_started) * 1000,
            3,
        ),
        "samples": samples,
        "discovery_query": discovery_key,
        "discovery_task": task,
        "discovery_path_was_provided": target_path in task,
        "import_ms": round(import_ms, 3),
        "open_ms": [round(value, 3) for value in open_times],
        "open_p50_ms": round(percentile(open_times, 0.50), 3),
        "open_p95_ms": round(percentile(open_times, 0.95), 3),
        "search_ms": [round(value, 3) for value in search_times],
        "search_p50_ms": round(percentile(search_times, 0.50), 3),
        "search_p95_ms": round(percentile(search_times, 0.95), 3),
        "broad_search_ms": [round(value, 3) for value in broad_search_times],
        "broad_search_p95_ms": round(percentile(broad_search_times, 0.95), 3),
        "overflow_search_ms": [
            round(value, 3) for value in overflow_search_times
        ],
        "overflow_search_p95_ms": round(
            percentile(overflow_search_times, 0.95),
            3,
        ),
        "old_source_search_ms": [
            round(value, 3) for value in old_source_search_times
        ],
        "old_source_search_p95_ms": round(
            percentile(old_source_search_times, 0.95),
            3,
        ),
        "read_ms": [round(value, 3) for value in read_times],
        "read_p50_ms": round(percentile(read_times, 0.50), 3),
        "read_p95_ms": round(percentile(read_times, 0.95), 3),
        "checkpoint_ms": round(checkpoint_ms, 3),
        "resume_samples_ms": [round(value, 3) for value in resume_times],
        "resume_ms": round(resume_ms, 3),
        "open_found": open_found,
        "search_found": search_found,
        "broad_found": broad_found,
        "overflow_found": overflow_found,
        "old_source_found": old_source_found,
        "old_source_semantic_deferred": old_source_semantic_deferred,
        "read_found": read_found,
        "critical_lane_failures": critical_lane_failures,
        "resume_found": all(resume_found_samples),
        "resume_found_samples": resume_found_samples,
        "timings_ms": {
            "open": summarize_timing_samples(open_timing_samples),
            "search": summarize_timing_samples(search_timing_samples),
            "broad_search": summarize_timing_samples(broad_timing_samples),
            "resume": summarize_timing_samples(resume_timing_samples),
        },
        "timings_phase_sum_sane": all(
            timing_phase_sum_sane(sample)
            for sample in (
                open_timing_samples
                + search_timing_samples
                + broad_timing_samples
                + resume_timing_samples
            )
        ),
        "boundary_probe": boundary_probe,
        "flat_file_control": flat_file_control,
        "flat_file_control_reused_from": flat_file_control_source,
        "service_to_flat_file_latency": {
            "open_discovery_p95_ratio": round(
                percentile(open_times, 0.95)
                / max(0.001, flat_file_control["discovery_p95_ms"]),
                3,
            ),
            "search_discovery_p95_ratio": round(
                percentile(search_times, 0.95)
                / max(0.001, flat_file_control["discovery_p95_ms"]),
                3,
            ),
            "exact_read_p95_ratio": round(
                percentile(read_times, 0.95)
                / max(0.001, flat_file_control["read_p95_ms"]),
                3,
            ),
            "broad_search_p95_ratio": round(
                percentile(broad_search_times, 0.95)
                / max(0.001, flat_file_control["broad_search_p95_ms"]),
                3,
            ),
        },
        "response_accounting": summarize_response_accounting(response_samples),
        "semantic_failure_probe": failure_probe,
        "semantic_catchup": {
            "retrieval_tested_while_pending": not bool(
                provisioning["provisioning"].get("semantic_ready_at_start")
            ),
            "start": provisioning["provisioning"].get(
                "index_status_at_start",
                {},
            ),
            "end": recursively_redact_secrets(semantic_status_end),
            "wait_for_semantic": wait_for_semantic,
            "semantic_ready_responses": all(
                not response_reports_lane_failure(body, "semantic")
                and not response_reports_gap_kind(
                    body,
                    "retrieval_lane_unavailable",
                )
                and not response_reports_gap_kind(
                    body,
                    "retrieval_lane_deferred",
                )
                for _, body in response_samples
                if isinstance(body, dict)
            ),
        },
        "embedding_spend_estimate": {
            "model": "text-embedding-3-small",
            "estimated_input_tokens": (
                sum(len(str(document["content"])) for document in documents) + 3
            ) // 4,
            "usd_per_million_tokens": 0.02,
            "estimated_usd": round(
                (
                    (
                        sum(
                            len(str(document["content"]))
                            for document in documents
                        )
                        + 3
                    )
                    // 4
                )
                / 1_000_000
                * 0.02,
                6,
            )
            if wait_for_semantic
            else 0.0,
            "basis": "ceil(source characters / 4); provider receipt unavailable",
        },
        "concurrent_probe": concurrent_probe,
        "checkpoint_pressure": checkpoint_pressure,
        "index_scan_growth": (
            counter_growth(index_before, index_after)
            if index_before is not None and index_after is not None
            else None
        ),
        "checkpoint_id": checkpoint_id or None,
        "checkpoint_database_growth": checkpoint_footprint,
        "provisioning": {
            key: value
            for key, value in provisioning["provisioning"].items()
            if key not in {"import_response", "ready_response"}
        },
    }


def evaluate_gates(
    scales: list[dict[str, Any]],
    thresholds: dict[str, float | int],
    *,
    required_scales: Sequence[int] = (),
    minimum_samples: int | None = None,
    semantic_failure_required: bool = False,
    require_gin_index: bool = True,
) -> list[dict[str, Any]]:
    largest = max(scales, key=lambda item: item["scale"])
    smallest = min(scales, key=lambda item: item["scale"])
    latency_specs = [
        ("open_p95_ms", "open_p95_ms"),
        ("search_p95_ms", "search_p95_ms"),
        (
            "broad_search_p95_ms",
            "broad_search_p95_ms"
            if "broad_search_p95_ms" in thresholds
            else "search_p95_ms",
        ),
        (
            "overflow_search_p95_ms",
            "broad_search_p95_ms"
            if "broad_search_p95_ms" in thresholds
            else "search_p95_ms",
        ),
        (
            "old_source_search_p95_ms",
            "broad_search_p95_ms"
            if "broad_search_p95_ms" in thresholds
            else "search_p95_ms",
        ),
        ("read_p95_ms", "read_p95_ms"),
        ("checkpoint_ms", "checkpoint_ms"),
        ("resume_ms", "resume_ms"),
    ]
    gates: list[tuple[str, bool, Any, Any]] = [
        (
            "all_opens_return_target",
            all(all(item["open_found"]) for item in scales),
            [item["open_found"] for item in scales],
            True,
        ),
        (
            "all_searches_return_target",
            all(all(item["search_found"]) for item in scales),
            [item["search_found"] for item in scales],
            True,
        ),
        (
            "all_resumes_return_target",
            all(item["resume_found"] for item in scales),
            [item["resume_found"] for item in scales],
            True,
        ),
        (
            "all_exact_reads_return_target",
            all(all(item["read_found"]) for item in scales),
            [item["read_found"] for item in scales],
            True,
        ),
    ]
    if any("timings_ms" in item for item in scales):
        gates.extend([
            (
                "timings_ms_are_reported",
                all(
                    bool(item.get("timings_ms", {}).get("open"))
                    and bool(item.get("timings_ms", {}).get("search"))
                    for item in scales
                ),
                [item.get("timings_ms", {}) for item in scales],
                "per-phase p50/p95/p99 for open and search",
            ),
            (
                "timings_ms_phase_sum_is_sane",
                all(item.get("timings_phase_sum_sane") is True for item in scales),
                [item.get("timings_phase_sum_sane") for item in scales],
                True,
            ),
        ])
    semantic_ready_scales = [
        item
        for item in scales
        if item.get("semantic_catchup", {}).get("wait_for_semantic")
    ]
    if semantic_ready_scales:
        gates.append((
            "semantic_ready_runs_have_no_deferred_or_unavailable_lane",
            all(
                item["semantic_catchup"].get("semantic_ready_responses") is True
                for item in semantic_ready_scales
            ),
            [
                {
                    "scale": item["scale"],
                    "ready": item["semantic_catchup"].get(
                        "semantic_ready_responses"
                    ),
                }
                for item in semantic_ready_scales
            ],
            True,
        ))
    gates.extend(
        (
            f"every_scale_{metric}",
            all(
                float(item.get(metric, 0.0)) <= float(thresholds[threshold_key])
                for item in scales
            ),
            [
                {"scale": item["scale"], "observed": item.get(metric, 0.0)}
                for item in scales
            ],
            thresholds[threshold_key],
        )
        for metric, threshold_key in latency_specs
    )
    critical_failures = [
        {
            "scale": item["scale"],
            "failures": [
                failure
                for failure in item.get("critical_lane_failures", [])
                if failure.get("exact") or failure.get("lexical")
            ],
        }
        for item in scales
    ]
    gates.append((
        "no_exact_or_lexical_lane_failures",
        all(not item["failures"] for item in critical_failures),
        critical_failures,
        [],
    ))
    boundary_scales = [
        item
        for item in scales
        if item.get("protocol") == "simple"
        and item["scale"] >= PRODUCTION_RECORDS
    ]
    if boundary_scales:
        boundaries = [
            {"scale": item["scale"], **item.get("boundary_probe", {})}
            for item in boundary_scales
        ]
        gates.extend([
            (
                "every_scale_max_batch_read_contract",
                all(
                    boundary.get("pass") is True
                    and boundary.get("batch_read_ms", float("inf"))
                    <= thresholds["max_batch_read_ms"]
                    for boundary in boundaries
                ),
                boundaries,
                {
                    "entries": 32,
                    "latency_ms": thresholds["max_batch_read_ms"],
                },
            ),
            (
                "every_scale_max_checkpoint_source_contract",
                all(
                    boundary.get("pass") is True
                    and boundary.get("checkpoint_ms", float("inf"))
                    <= thresholds["max_checkpoint_sources_ms"]
                    for boundary in boundaries
                ),
                boundaries,
                {
                    "source_entries": 64,
                    "latency_ms": thresholds["max_checkpoint_sources_ms"],
                },
            ),
        ])
    if all("semantic_catchup" in item for item in scales):
        pending_retrieval = [
            bool(item["semantic_catchup"]["retrieval_tested_while_pending"])
            and all(item["open_found"])
            and all(item["search_found"])
            and all(item["read_found"])
            for item in scales
        ]
        gates.append((
            "retrieval_works_while_semantic_indexing_is_pending",
            all(pending_retrieval),
            pending_retrieval,
            True,
        ))
    if any("discovery_path_was_provided" in item for item in scales):
        gates.append((
            "unknown_path_discovery_does_not_reveal_target_path",
            all(
                not item.get("discovery_path_was_provided", True)
                for item in scales
            ),
            [
                item.get("discovery_path_was_provided")
                for item in scales
            ],
            False,
        ))
    if required_scales:
        observed_scales = {int(item["scale"]) for item in scales}
        gates.append((
            "all_required_scales_completed",
            set(required_scales).issubset(observed_scales),
            sorted(observed_scales),
            sorted(set(required_scales)),
        ))
    if minimum_samples is not None:
        gates.append((
            "retrieval_sample_count_is_definitive",
            all(int(item.get("samples", 0)) >= minimum_samples for item in scales),
            [item.get("samples", 0) for item in scales],
            f">= {minimum_samples} per scale",
        ))
    if any("broad_found" in item for item in scales):
        gates.append((
            "all_broad_searches_return_sources",
            all(all(item.get("broad_found", [])) for item in scales),
            [item.get("broad_found", []) for item in scales],
            True,
        ))
    if any("overflow_found" in item for item in scales):
        gates.append((
            "bounded_lexical_overflow_returns_late_relevant_source",
            all(all(item.get("overflow_found", [])) for item in scales),
            [item.get("overflow_found", []) for item in scales],
            True,
        ))
    if any("old_source_found" in item for item in scales):
        gates.extend([
            (
                "old_relevant_source_survives_many_newer_writes",
                all(all(item.get("old_source_found", [])) for item in scales),
                [item.get("old_source_found", []) for item in scales],
                True,
            ),
            (
                "semantic_is_not_deferred_by_unrelated_backlog",
                all(
                    not any(item.get("old_source_semantic_deferred", []))
                    for item in scales
                ),
                [
                    item.get("old_source_semantic_deferred", [])
                    for item in scales
                ],
                False,
            ),
        ])
    if any("flat_file_control" in item for item in scales):
        flat_controls = [item.get("flat_file_control", {}) for item in scales]
        gates.extend([
            (
                "flat_file_discovery_returns_target",
                all(
                    all(control.get("discovery_found", []))
                    for control in flat_controls
                ),
                [
                    control.get("discovery_found", [])
                    for control in flat_controls
                ],
                True,
            ),
            (
                "flat_file_exact_read_returns_target",
                all(
                    all(control.get("read_found", []))
                    for control in flat_controls
                ),
                [control.get("read_found", []) for control in flat_controls],
                True,
            ),
            (
                "flat_file_broad_search_returns_sources",
                all(
                    all(control.get("broad_found", []))
                    for control in flat_controls
                ),
                [control.get("broad_found", []) for control in flat_controls],
                True,
            ),
        ])
    if any("response_accounting" in item for item in scales):
        accounting = [
            item.get("response_accounting", {})
            for item in scales
        ]
        gates.append((
            "service_protocol_overhead_does_not_exceed_evidence",
            all(
                int(item.get("evidence_chars", 0)) > 0
                and float(item.get("protocol_to_evidence_ratio", float("inf")))
                <= float(thresholds["protocol_to_evidence_ratio"])
                for item in accounting
            ),
            [
                {
                    "scale": scale["scale"],
                    "source_text_chars": item.get("source_text_chars"),
                    "source_identity_chars": item.get("source_identity_chars"),
                    "protocol_chars": item.get("protocol_chars"),
                    "ratio": item.get("protocol_to_evidence_ratio"),
                }
                for scale, item in zip(scales, accounting)
            ],
            f"ratio <= {thresholds['protocol_to_evidence_ratio']}",
        ))
    verbatim_scales = [
        item
        for item in scales
        if item.get("verbatim_identifier", {}).get("status") == "complete"
    ]
    if verbatim_scales:
        observed = [
            {
                "scale": item["scale"],
                "returned": item["verbatim_identifier"]["returned"],
                "expected": item["verbatim_identifier"]["expected"],
            }
            for item in verbatim_scales
        ]
        gates.append((
            "verbatim_identifier",
            all(
                item["verbatim_identifier"].get("pass") is True
                for item in verbatim_scales
            ),
            observed,
            "every planted identifier appears in exact-lane source payload",
        ))
    if semantic_failure_required:
        probe = largest.get("semantic_failure_probe", {})
        gates.append((
            "semantic_provider_failure_falls_back_to_exact_and_lexical",
            bool(probe.get("pass")),
            {
                "status": probe.get("status", "not_run"),
                "semantic_failure_observed": probe.get(
                    "semantic_failure_observed",
                ),
                "exact_lexical_found": probe.get(
                    "exact_lexical_found_during_failure",
                ),
                "mixed_lane_found": probe.get(
                    "mixed_lane_found_during_failure",
                ),
                "semantic_lane_healthy_after_restore": probe.get(
                    "semantic_lane_healthy_after_restore",
                ),
            },
            True,
        ))
    if "concurrent_probe" in largest:
        concurrent_probe = largest["concurrent_probe"]
        gates.extend([
            (
                "unrelated_write_commits",
                bool(concurrent_probe["write_committed"]),
                concurrent_probe["write_committed"],
                True,
            ),
            (
                "unrelated_write_p95_latency",
                concurrent_probe["write_ms"]
                <= thresholds.get(
                    "concurrent_write_ms",
                    thresholds["search_p95_ms"],
                ),
                concurrent_probe["write_ms"],
                thresholds.get(
                    "concurrent_write_ms",
                    thresholds["search_p95_ms"],
                ),
            ),
            (
                "retrieval_survives_unrelated_write",
                all(concurrent_probe["search_found"]),
                concurrent_probe["search_found"],
                True,
            ),
            (
                "concurrent_exact_and_lexical_lanes_remain_healthy",
                not any(
                    failure.get("exact") or failure.get("lexical")
                    for failure in concurrent_probe.get(
                        "search_lane_failures",
                        [],
                    )
                ),
                concurrent_probe.get("search_lane_failures", []),
                "all false",
            ),
            (
                "concurrent_search_p95",
                concurrent_probe["search_p95_ms"]
                <= thresholds.get(
                    "concurrent_search_p95_ms",
                    thresholds["search_p95_ms"],
                ),
                concurrent_probe["search_p95_ms"],
                thresholds.get(
                    "concurrent_search_p95_ms",
                    thresholds["search_p95_ms"],
                ),
            ),
        ])
        if minimum_samples is not None:
            gates.append((
                "foreground_write_sample_count_is_definitive",
                int(concurrent_probe.get("rounds", 0)) >= minimum_samples,
                concurrent_probe.get("rounds", 0),
                f">= {minimum_samples}",
            ))
    checkpoint_pressure = largest.get("checkpoint_pressure")
    if checkpoint_pressure is not None:
        gates.append((
            "requested_checkpoint_rate_is_bounded",
            checkpoint_pressure["requested_checkpoints_per_minute"]
            <= thresholds["requested_checkpoints_per_minute"],
            checkpoint_pressure,
            (
                f"requested checkpoints <= "
                f"{thresholds['requested_checkpoints_per_minute']} per minute"
            ),
        ))
    index_growth = largest.get("index_scan_growth")
    if index_growth is not None and require_gin_index:
        gates.append((
            "lexical_search_uses_gin_index",
            index_growth.get("search_chunks_fts_idx", 0) > 0,
            index_growth.get("search_chunks_fts_idx", 0),
            "> 0",
        ))
    growth = largest["checkpoint_database_growth"]
    if growth["bytes"] is not None:
        gates.extend([
            (
                "checkpoint_row_growth_is_bounded",
                growth["rows"] <= thresholds["checkpoint_row_growth"],
                growth["rows"],
                thresholds["checkpoint_row_growth"],
            ),
            (
                "checkpoint_storage_growth_is_bounded",
                growth["bytes"] <= thresholds["checkpoint_bytes_growth"],
                growth["bytes"],
                thresholds["checkpoint_bytes_growth"],
            ),
        ])
    if len(scales) > 1:
        open_growth = largest["open_p95_ms"] / max(1.0, smallest["open_p95_ms"])
        search_growth = largest["search_p95_ms"] / max(
            1.0,
            smallest["search_p95_ms"],
        )
        growth_floor_ms = thresholds.get("latency_growth_floor_ms", 1_000.0)
        gates.extend([
            (
                "open_latency_growth_is_materially_bounded",
                largest["open_p95_ms"] <= growth_floor_ms
                or open_growth <= thresholds["ten_x_latency_growth"],
                {
                    "ratio": round(open_growth, 3),
                    "largest_p95_ms": largest["open_p95_ms"],
                },
                {
                    "ratio": thresholds["ten_x_latency_growth"],
                    "applies_above_ms": growth_floor_ms,
                },
            ),
            (
                "search_latency_growth_is_materially_bounded",
                largest["search_p95_ms"] <= growth_floor_ms
                or search_growth <= thresholds["ten_x_latency_growth"],
                {
                    "ratio": round(search_growth, 3),
                    "largest_p95_ms": largest["search_p95_ms"],
                },
                {
                    "ratio": thresholds["ten_x_latency_growth"],
                    "applies_above_ms": growth_floor_ms,
                },
            ),
        ])
    return [
        {
            "name": name,
            "pass": passed,
            "observed": observed,
            "threshold": threshold,
        }
        for name, passed, observed, threshold in gates
    ]


def compare_results(before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    before_scales = {item["scale"]: item for item in before["scales"]}
    after_scales = {item["scale"]: item for item in after["scales"]}
    shared = sorted(set(before_scales) & set(after_scales))
    rows = []
    for scale in shared:
        old = before_scales[scale]
        new = after_scales[scale]
        rows.append({
            "scale": scale,
            "import_delta_pct": percentage_delta(
                old["import_ms"],
                new["import_ms"],
            ),
            "open_p95_delta_pct": percentage_delta(
                old["open_p95_ms"],
                new["open_p95_ms"],
            ),
            "search_p95_delta_pct": percentage_delta(
                old["search_p95_ms"],
                new["search_p95_ms"],
            ),
            "broad_search_p95_delta_pct": (
                percentage_delta(
                    old["broad_search_p95_ms"],
                    new["broad_search_p95_ms"],
                )
                if "broad_search_p95_ms" in old
                and "broad_search_p95_ms" in new
                else None
            ),
            "concurrent_search_p95_delta_pct": (
                percentage_delta(
                    old["concurrent_probe"]["search_p95_ms"],
                    new["concurrent_probe"]["search_p95_ms"],
                )
                if "concurrent_probe" in old and "concurrent_probe" in new
                else None
            ),
            "read_p95_delta_pct": percentage_delta(
                old["read_p95_ms"],
                new["read_p95_ms"],
            ),
            "checkpoint_delta_pct": percentage_delta(
                old["checkpoint_ms"],
                new["checkpoint_ms"],
            ),
            "resume_delta_pct": percentage_delta(
                old["resume_ms"],
                new["resume_ms"],
            ),
            "checkpoint_rows_before": old["checkpoint_database_growth"]["rows"],
            "checkpoint_rows_after": new["checkpoint_database_growth"]["rows"],
            "checkpoint_bytes_before": old["checkpoint_database_growth"]["bytes"],
            "checkpoint_bytes_after": new["checkpoint_database_growth"]["bytes"],
        })
    return {
        "before_label": before["label"],
        "after_label": after["label"],
        "shared_scales": shared,
        "rows": rows,
        "before_pass": before["pass"],
        "after_pass": after["pass"],
    }


def percentage_delta(before: float, after: float) -> float | None:
    if before == 0:
        return None
    return round((after - before) / before * 100.0, 3)


def command_validate(args: argparse.Namespace) -> int:
    documents, target_path, marker = synthetic_documents(args.scale)
    task = synthetic_discovery_task(args.scale)
    assert len(documents) == args.scale
    assert documents[-1]["path"] == target_path
    assert marker in documents[-1]["content"]
    assert sum(marker in item["content"] for item in documents) == 1
    assert target_path not in task
    print(json.dumps({
        "status": "ok",
        "scale": args.scale,
        "target_path": target_path,
        "marker": marker,
        "discovery_key": synthetic_discovery_key(args.scale),
        "discovery_task": task,
        "discovery_path_was_provided": False,
        "payload_bytes": len(json.dumps(documents, separators=(",", ":"))),
    }, indent=2))
    return 0


def load_reused_flat_controls(
    path: Path | None,
    profile: RunProfile,
) -> dict[int, dict[str, Any]]:
    if path is None:
        return {}
    try:
        reused_artifact = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"could not load reused flat-file controls: {error}") from error
    reused = {
        int(item["scale"]): item["flat_file_control"]
        for item in reused_artifact.get("scales", [])
        if isinstance(item, dict)
        and isinstance(item.get("scale"), int)
        and isinstance(item.get("flat_file_control"), dict)
    }
    missing_controls = [scale for scale in profile.scales if scale not in reused]
    if missing_controls:
        raise ValueError(
            "reused flat-file artifact lacks scales: "
            + ", ".join(str(scale) for scale in missing_controls)
        )
    for scale in profile.scales:
        control = reused[scale]
        if (
            control.get("files") != scale
            or control.get("samples") != profile.samples
            or any(
                len(control.get(key, [])) != profile.samples
                for key in ("discovery_found", "read_found", "broad_found")
            )
            or not all(control.get("discovery_found", []))
            or not all(control.get("read_found", []))
            or not all(control.get("broad_found", []))
        ):
            raise ValueError(
                f"reused flat-file control for scale {scale} is incomplete "
                "or does not match this run profile"
            )
    return reused


def command_run(args: argparse.Namespace) -> int:
    try:
        profile = resolve_run_profile(args)
        if bool(args.semantic_failure_start_command) != bool(
            args.semantic_failure_stop_command
        ):
            raise ValueError(
                "semantic failure testing requires both the start and stop "
                "hook commands"
            )
        reused_flat_controls = load_reused_flat_controls(
            args.reuse_flat_controls_from,
            profile,
        )
    except ValueError as error:
        result = {
            "schema": "straylight-performance-eval@v2",
            "created_at": datetime.now().astimezone().isoformat(),
            "label": args.label,
            "pass": False,
            "errors": [{
                "type": "ConfigurationError",
                "message": str(error),
            }],
        }
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result, indent=2))
        return 2

    admin = NativeApiClient(timeout=profile.import_timeout_seconds)
    scales = []
    errors = []
    partial_flat_controls: dict[int, dict[str, Any]] = {}
    largest_requested_scale = max(profile.scales)
    for scale in profile.scales:
        try:
            scales.append(benchmark_scale(
                admin,
                label=args.label,
                scale=scale,
                samples=profile.samples,
                timeout_seconds=args.timeout,
                import_timeout_seconds=profile.import_timeout_seconds,
                db_container=args.db_container,
                protocol=args.protocol,
                run_semantic_failure=scale == largest_requested_scale,
                concurrent_rounds=(
                    profile.samples
                    if scale == largest_requested_scale
                    else min(profile.samples, QUICK_SAMPLES)
                ),
                semantic_failure_required=profile.semantic_failure_required,
                semantic_failure_start_command=(
                    args.semantic_failure_start_command
                ),
                semantic_failure_stop_command=args.semantic_failure_stop_command,
                semantic_failure_settle_seconds=(
                    args.semantic_failure_settle_seconds
                ),
                wait_for_semantic=args.wait_semantic,
                unique_queries=args.unique_queries,
                flat_result_callback=lambda result, current=scale: (
                    partial_flat_controls.__setitem__(current, result)
                ),
                flat_file_control_override=reused_flat_controls.get(scale),
                flat_file_control_source=(
                    str(args.reuse_flat_controls_from)
                    if scale in reused_flat_controls
                    else None
                ),
            ))
            partial_flat_controls.pop(scale, None)
        except (NativeApiError, RuntimeError, TimeoutError) as error:
            error_result = {
                "scale": scale,
                "type": type(error).__name__,
                "message": str(error),
                "status": getattr(error, "status", None),
                "elapsed_ms": getattr(error, "elapsed_ms", None),
            }
            if scale in partial_flat_controls:
                error_result["flat_file_control"] = partial_flat_controls.pop(
                    scale,
                )
            errors.append(error_result)
            break
    required_scales = (
        [PRODUCTION_RECORDS]
        + ([FUTURE_RECORDS] if profile.future_soak_requested else [])
        if profile.definitive
        else []
    )
    gates = (
        evaluate_gates(
            scales,
            DEFAULT_THRESHOLDS,
            required_scales=required_scales,
            minimum_samples=(
                DEFINITIVE_SAMPLES if profile.definitive else None
            ),
            semantic_failure_required=profile.semantic_failure_required,
            require_gin_index=profile.definitive,
        )
        if scales
        else []
    )
    fingerprint = implementation_fingerprint(args.api_container)
    if profile.definitive:
        gates.append({
            "name": "implementation_fingerprint_is_reproducible",
            "pass": fingerprint["reproducible"],
            "actual": fingerprint,
            "threshold": {
                "clean_tracked_source": True,
                "no_untracked_source": True,
                "image_revision_matches_source": True,
            },
        })
    completed_scales = {item["scale"] for item in scales}
    future_soak_status = (
        "not_requested"
        if not profile.future_soak_requested
        else "completed"
        if FUTURE_RECORDS in completed_scales
        else "failed_or_not_reached"
    )
    result = {
        "schema": "straylight-performance-eval@v2",
        "created_at": datetime.now().astimezone().isoformat(),
        "label": args.label,
        "protocol": args.protocol,
        "api_url": admin.base_url,
        "implementation_fingerprint": fingerprint,
        "run_profile": {
            "mode": "definitive" if profile.definitive else "quick",
            "definitive": profile.definitive,
            "samples_per_retrieval": profile.samples,
            "requested_scales": list(profile.scales),
            "required_scales": required_scales,
            "future_soak": {
                "requested": profile.future_soak_requested,
                "records": FUTURE_RECORDS,
                "status": future_soak_status,
            },
            "semantic_failure_probe_required": (
                profile.semantic_failure_required
            ),
            "semantic_failure_hooks_configured": bool(
                args.semantic_failure_start_command
                and args.semantic_failure_stop_command
            ),
            "wait_for_semantic": bool(args.wait_semantic),
            "unique_queries": bool(args.unique_queries),
            "import_timeout_seconds": profile.import_timeout_seconds,
        },
        "production_reference_records": PRODUCTION_RECORDS,
        "future_reference_records": FUTURE_RECORDS,
        "scales": scales,
        "thresholds": DEFAULT_THRESHOLDS,
        "gates": gates,
        "errors": errors,
        "pass": bool(scales and not errors and all(gate["pass"] for gate in gates)),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result["pass"] else 2


def command_compare(args: argparse.Namespace) -> int:
    before = json.loads(args.before.read_text(encoding="utf-8"))
    after = json.loads(args.after.read_text(encoding="utf-8"))
    comparison = compare_results(before, after)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(
            json.dumps(comparison, indent=2) + "\n",
            encoding="utf-8",
        )
    print(json.dumps(comparison, indent=2))
    return 0 if comparison["after_pass"] else 2


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Production-shaped Straylight retrieval and write-amplification benchmark",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate")
    validate.add_argument("--scale", type=int, default=10_000)
    validate.set_defaults(function=command_validate)

    run = subparsers.add_parser("run")
    run.add_argument("--label", required=True)
    run.add_argument("--scales", type=int, nargs="+")
    run.add_argument(
        "--samples",
        "--repeats",
        dest="samples",
        type=int,
        help=(
            f"measured samples per retrieval operation; default "
            f"{DEFINITIVE_SAMPLES}, or {QUICK_SAMPLES} with --quick"
        ),
    )
    run.add_argument(
        "--quick",
        action="store_true",
        help=(
            "run a visibly non-definitive developer check; permits smaller "
            "scales, fewer samples, and an unproven provider-failure fallback"
        ),
    )
    run.add_argument(
        "--future-soak",
        action="store_true",
        help=(
            f"append the explicit {FUTURE_RECORDS:,}-entry future-scale soak; "
            "never silently included or skipped"
        ),
    )
    run.add_argument("--timeout", type=float, default=45.0)
    run.add_argument(
        "--import-timeout",
        type=float,
        help=(
            "seconds allowed for fixture import/index readiness; defaults to "
            "1800, or 7200 with --future-soak"
        ),
    )
    run.add_argument("--db-container")
    run.add_argument(
        "--api-container",
        help=(
            "API container used for the run; definitive artifacts require its "
            "exact image ID and revision label to match a clean source revision"
        ),
    )
    run.add_argument(
        "--protocol",
        choices=("legacy", "simple"),
        default="legacy",
    )
    run.add_argument(
        "--semantic-failure-start-command",
        help=(
            "argv-style command that disables the semantic provider on a "
            "disposable test stack; shell syntax is not evaluated"
        ),
    )
    run.add_argument(
        "--semantic-failure-stop-command",
        help=(
            "argv-style command that restores the semantic provider; always "
            "run after an attempted failure probe"
        ),
    )
    run.add_argument(
        "--semantic-failure-settle-seconds",
        type=float,
        default=2.0,
    )
    run.add_argument(
        "--wait-semantic",
        action="store_true",
        help=(
            "block evaluation provisioning until every imported simple-core "
            "chunk has an embedding, then require semantic-ready responses"
        ),
    )
    run.add_argument(
        "--unique-queries",
        action="store_true",
        help=(
            "append a fresh nonce to measured query strings to profile "
            "query-embedding cache misses"
        ),
    )
    run.add_argument(
        "--reuse-flat-controls-from",
        type=Path,
        help=(
            "reuse matching direct-file controls from a prior artifact while "
            "rerunning every Straylight measurement; provenance is recorded"
        ),
    )
    run.add_argument(
        "--out",
        type=Path,
        required=True,
    )
    run.set_defaults(function=command_run)

    compare = subparsers.add_parser("compare")
    compare.add_argument("--before", type=Path, required=True)
    compare.add_argument("--after", type=Path, required=True)
    compare.add_argument("--out", type=Path)
    compare.set_defaults(function=command_compare)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    return args.function(args)


if __name__ == "__main__":
    raise SystemExit(main())

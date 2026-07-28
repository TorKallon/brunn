#!/usr/bin/env python3

"""Black-box HTTP proof for the D11 semantic deadline and async cache warmup."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from native_eval import NativeApiClient, NativeApiError  # noqa: E402


SCHEMA = "straylight-semantic-http-probe@v1"
SOURCE_TEXT_KEYS = frozenset({"content", "text", "excerpt", "source_text"})


def semantic_failure(value: Any) -> bool:
    if isinstance(value, dict):
        for key in ("lane_failures", "failed_lanes"):
            failures = value.get(key)
            if isinstance(failures, list) and any(
                str(item).casefold().startswith("semantic")
                for item in failures
            ):
                return True
        if str(value.get("lane", "")).casefold() == "semantic":
            rendered = str(
                value.get("kind")
                or value.get("status")
                or value.get("message")
                or ""
            ).casefold()
            if any(
                marker in rendered
                for marker in ("unavailable", "deferred", "failed", "timeout")
            ):
                return True
        return any(semantic_failure(item) for item in value.values())
    if isinstance(value, list):
        return any(semantic_failure(item) for item in value)
    return False


def candidate_count(value: Any) -> int:
    if isinstance(value, dict):
        candidates = value.get("candidates")
        own = len(candidates) if isinstance(candidates, list) else 0
        return own + sum(candidate_count(item) for item in value.values())
    if isinstance(value, list):
        return sum(candidate_count(item) for item in value)
    return 0


def source_text_contains(
    value: Any,
    needle: str,
    *,
    parent_key: str | None = None,
) -> bool:
    if isinstance(value, dict):
        return any(
            source_text_contains(item, needle, parent_key=key)
            for key, item in value.items()
        )
    if isinstance(value, list):
        return any(
            source_text_contains(item, needle, parent_key=parent_key)
            for item in value
        )
    return bool(
        isinstance(value, str)
        and parent_key in SOURCE_TEXT_KEYS
        and needle.casefold() in value.casefold()
    )


def hook_fingerprint(argv: list[str]) -> dict[str, Any]:
    canonical = json.dumps(argv, separators=(",", ":"), ensure_ascii=False)
    return {
        "executable": Path(argv[0]).name,
        "argv_sha256": hashlib.sha256(canonical.encode("utf-8")).hexdigest(),
        "argument_count": len(argv),
    }


def run_hook(command: str, timeout_seconds: float) -> dict[str, Any]:
    started = time.monotonic()
    try:
        argv = shlex.split(command)
    except ValueError as error:
        return {
            "pass": False,
            "error": f"invalid command syntax: {error}",
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    if not argv:
        return {
            "pass": False,
            "error": "empty command",
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    try:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=timeout_seconds,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {
            **hook_fingerprint(argv),
            "pass": False,
            "error": type(error).__name__,
            "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
        }
    return {
        **hook_fingerprint(argv),
        "pass": completed.returncode == 0,
        "exit_code": completed.returncode,
        "elapsed_ms": round((time.monotonic() - started) * 1_000, 3),
    }


def runtime_snapshot(client: NativeApiClient, expected_deadline_ms: int) -> dict[str, Any]:
    ready = client.get("/ready")
    features = ready.data.get("runtime_features")
    if not isinstance(features, dict):
        raise ValueError("/ready omitted D11 runtime_features")
    snapshot = {
        "semantic_lane": features.get("semantic_lane"),
        "embed_cache": features.get("embed_cache"),
        "semantic_deadline_ms": features.get("semantic_deadline_ms"),
        "build_revision": ready.data.get("build_revision"),
    }
    snapshot["pass"] = (
        snapshot["semantic_lane"] is True
        and snapshot["embed_cache"] is True
        and snapshot["semantic_deadline_ms"] == expected_deadline_ms
    )
    if not snapshot["pass"]:
        raise ValueError(
            "HTTP deadline probe requires semantic_lane=on, embed_cache=on, "
            f"semantic_deadline_ms={expected_deadline_ms}: {snapshot}"
        )
    return snapshot


def search(
    client: NativeApiClient,
    *,
    query_id: str,
    query: str,
    modes: list[str],
) -> dict[str, Any]:
    response = client.post(
        "/v1/workspace/search",
        {
            "queries": [
                {
                    "id": query_id,
                    "query": query,
                    "modes": modes,
                    "limit": 8,
                }
            ]
        },
    )
    return {
        "body": response.body,
        "http_status": response.http_status,
        "elapsed_ms": round(response.elapsed_ms, 3),
        "semantic_failure": semantic_failure(response.body),
        "candidate_count": candidate_count(response.body),
    }


def run_probe(args: argparse.Namespace) -> dict[str, Any]:
    if args.injected_delay_ms <= args.deadline_ms:
        raise ValueError("injected delay must exceed the semantic deadline")
    if args.injected_delay_ms <= 50:
        raise ValueError("injected delay must exceed 50ms")
    if args.max_response_ms <= 0 or args.hook_timeout <= 0:
        raise ValueError("response and hook timeouts must be positive")
    slow_argv = shlex.split(args.slow_command)
    restore_argv = shlex.split(args.restore_command)
    if not slow_argv or not restore_argv or slow_argv == restore_argv:
        raise ValueError(
            "slow and restore hooks must be non-empty, distinct argv commands"
        )
    client = NativeApiClient(timeout=max(30.0, args.max_response_ms / 1_000 + 5))
    runtime = runtime_snapshot(client, args.deadline_ms)
    nonce = uuid.uuid4().hex
    baseline = search(
        client,
        query_id="semantic-http-baseline",
        query=f"{args.query} semantic baseline {nonce}",
        modes=["semantic"],
    )
    if baseline["semantic_failure"] or baseline["candidate_count"] < 1:
        raise RuntimeError(
            "semantic-only baseline was not healthy before slow-provider injection"
        )

    slow_hook = run_hook(args.slow_command, args.hook_timeout)
    restore_hook: dict[str, Any] | None = None
    cold: dict[str, Any] | None = None
    warm: dict[str, Any] | None = None
    restored: dict[str, Any] | None = None
    error: str | None = None
    try:
        if not slow_hook["pass"]:
            raise RuntimeError("slow-provider hook failed")
        time.sleep(max(0.0, args.settle_ms / 1_000))
        cache_query = f"{args.query} {args.marker} cache-probe-{nonce}"
        cold = search(
            client,
            query_id="semantic-http-cold-deadline",
            query=cache_query,
            modes=["exact", "lexical", "semantic"],
        )
        effective_response_limit = min(
            args.max_response_ms,
            args.injected_delay_ms - 50,
        )
        cold.update(
            {
                "exact_lexical_marker_retained": source_text_contains(
                    cold["body"],
                    args.marker,
                ),
                "effective_response_limit_ms": effective_response_limit,
            }
        )
        cold["pass"] = (
            cold["http_status"] < 500
            and cold["semantic_failure"]
            and cold["exact_lexical_marker_retained"]
            and cold["elapsed_ms"] <= effective_response_limit
        )
        time.sleep(
            max(
                0.0,
                (
                    args.warm_wait_ms
                    if args.warm_wait_ms is not None
                    else args.injected_delay_ms + 200
                )
                / 1_000,
            )
        )
        warm = search(
            client,
            query_id="semantic-http-warm-cache",
            query=cache_query,
            modes=["exact", "lexical", "semantic"],
        )
        warm.update(
            {
                "exact_lexical_marker_retained": source_text_contains(
                    warm["body"],
                    args.marker,
                )
            }
        )
        warm["pass"] = (
            warm["http_status"] < 500
            and not warm["semantic_failure"]
            and warm["candidate_count"] >= 1
            and warm["exact_lexical_marker_retained"]
            and warm["elapsed_ms"] <= args.max_response_ms
        )
    except (NativeApiError, RuntimeError) as failure:
        error = f"{type(failure).__name__}: {failure}"
    finally:
        restore_hook = run_hook(args.restore_command, args.hook_timeout)
        time.sleep(max(0.0, args.settle_ms / 1_000))
        if restore_hook["pass"]:
            try:
                restored = search(
                    client,
                    query_id="semantic-http-restored",
                    query=f"{args.query} semantic restored {uuid.uuid4().hex}",
                    modes=["semantic"],
                )
                restored["pass"] = (
                    restored["http_status"] < 500
                    and not restored["semantic_failure"]
                    and restored["candidate_count"] >= 1
                )
            except NativeApiError as failure:
                error = error or f"{type(failure).__name__}: {failure}"

    passed = bool(
        slow_hook["pass"]
        and restore_hook
        and restore_hook["pass"]
        and cold
        and cold.get("pass")
        and warm
        and warm.get("pass")
        and restored
        and restored.get("pass")
        and error is None
    )

    def public_sample(sample: dict[str, Any] | None) -> dict[str, Any] | None:
        if sample is None:
            return None
        return {
            key: value
            for key, value in sample.items()
            if key != "body"
        }

    return {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "status": "passed" if passed else "failed",
        "pass": passed,
        "runtime": runtime,
        "injection": {
            "injected_delay_ms": args.injected_delay_ms,
            "semantic_deadline_ms": args.deadline_ms,
            "max_response_ms": args.max_response_ms,
            "slow_hook": slow_hook,
            "restore_hook": restore_hook,
        },
        "baseline": public_sample(baseline),
        "cold_deadline": public_sample(cold),
        "warm_async_cache": public_sample(warm),
        "restored": public_sample(restored),
        "error": error,
        "conclusion": (
            "The first full HTTP response retained exact/lexical evidence and "
            "returned before the deliberately slow provider; the identical "
            "query then hit the asynchronously warmed cache."
            if passed
            else "The full HTTP deadline/cache contract was not proven."
        ),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run a full HTTP slow-provider/deadline/cache probe"
    )
    parser.add_argument("--query", required=True)
    parser.add_argument("--marker", required=True)
    parser.add_argument("--slow-command", required=True)
    parser.add_argument("--restore-command", required=True)
    parser.add_argument("--injected-delay-ms", type=int, default=800)
    parser.add_argument("--deadline-ms", type=int, default=300)
    parser.add_argument("--max-response-ms", type=int, default=750)
    parser.add_argument("--settle-ms", type=int, default=100)
    parser.add_argument("--warm-wait-ms", type=int)
    parser.add_argument("--hook-timeout", type=float, default=30.0)
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        artifact = run_probe(args)
    except Exception as error:
        artifact = {
            "schema": SCHEMA,
            "created_at": datetime.now().astimezone().isoformat(
                timespec="seconds"
            ),
            "status": "failed",
            "pass": False,
            "error": f"{type(error).__name__}: {error}",
        }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(artifact, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(artifact, indent=2))
    return 0 if artifact["pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())

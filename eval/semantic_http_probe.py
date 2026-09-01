#!/usr/bin/env python3

"""Black-box HTTP proof for the D11 semantic deadline and async cache warmup."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shlex
import sys
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from eval.hook_provenance import (  # noqa: E402
    hook_target_matches,
    run_hook as run_provenance_hook,
)
from native_eval import (  # noqa: E402
    NativeApiClient,
    NativeApiError,
    provision_evaluation,
    public_provisioning,
    recursively_redact_secrets,
)


SCHEMA = "brunn-semantic-http-probe@v1"
SOURCE_TEXT_KEYS = frozenset({"content", "text", "excerpt", "source_text"})
DEFAULT_QUERY = "locate the current semantic deadline qualification source"
DEFAULT_MARKER_PREFIX = "brunn-semantic-http-probe"
SAFE_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")


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


def run_hook(
    command: str,
    timeout_seconds: float,
    *,
    require_attestation: bool = False,
    expected_mode: str | None = None,
) -> dict[str, Any]:
    return run_provenance_hook(
        command,
        timeout_seconds=timeout_seconds,
        require_attestation=require_attestation,
        expected_mode=expected_mode,
    )


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


def run_contract(
    args: argparse.Namespace,
    *,
    client: NativeApiClient,
    query: str,
    marker: str,
) -> dict[str, Any]:
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
    runtime = runtime_snapshot(client, args.deadline_ms)
    nonce = uuid.uuid4().hex
    baseline = search(
        client,
        query_id="semantic-http-baseline",
        query=f"{query} semantic baseline {nonce}",
        modes=["semantic"],
    )
    if baseline["semantic_failure"] or baseline["candidate_count"] < 1:
        raise RuntimeError(
            "semantic-only baseline was not healthy before slow-provider injection"
        )

    require_attestation = bool(
        getattr(args, "require_hook_attestation", False)
    )
    slow_hook = run_hook(
        args.slow_command,
        args.hook_timeout,
        require_attestation=require_attestation,
        expected_mode="slow" if require_attestation else None,
    )
    restore_hook: dict[str, Any] | None = None
    cold: dict[str, Any] | None = None
    warm: dict[str, Any] | None = None
    restored: dict[str, Any] | None = None
    error: str | None = None
    try:
        if not slow_hook["pass"]:
            raise RuntimeError("slow-provider hook failed")
        time.sleep(max(0.0, args.settle_ms / 1_000))
        cache_query = f"{query} {marker} cache-probe-{nonce}"
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
                    marker,
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
                    marker,
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
        restore_hook = run_hook(
            args.restore_command,
            args.hook_timeout,
            require_attestation=require_attestation,
            expected_mode="forward" if require_attestation else None,
        )
        time.sleep(max(0.0, args.settle_ms / 1_000))
        if restore_hook["pass"]:
            try:
                restored = search(
                    client,
                    query_id="semantic-http-restored",
                    query=f"{query} semantic restored {uuid.uuid4().hex}",
                    modes=["semantic"],
                )
                restored["pass"] = (
                    restored["http_status"] < 500
                    and not restored["semantic_failure"]
                    and restored["candidate_count"] >= 1
                )
            except NativeApiError as failure:
                error = error or f"{type(failure).__name__}: {failure}"

    hook_target_bound = (
        hook_target_matches(slow_hook, restore_hook or {})
        if require_attestation
        else None
    )
    passed = bool(
        slow_hook["pass"]
        and restore_hook
        and restore_hook["pass"]
        and (hook_target_bound is not False)
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
        "definitive": require_attestation,
        "injection": {
            "injected_delay_ms": args.injected_delay_ms,
            "semantic_deadline_ms": args.deadline_ms,
            "max_response_ms": args.max_response_ms,
            "slow_hook": slow_hook,
            "restore_hook": restore_hook,
            "hook_attestation_required": require_attestation,
            "hook_target_bound": hook_target_bound,
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


def probe_document(
    *,
    nonce: str,
    marker: str,
    query: str,
) -> dict[str, Any]:
    content = (
        f"# Semantic deadline probe {nonce}\n\n"
        f"The unique current qualification marker is `{marker}`.\n\n"
        "This source exists only to prove that exact and lexical evidence "
        "survives a slow semantic embedding request, then that the identical "
        f"query warms asynchronously. Retrieval intent: {query}.\n"
    )
    encoded = content.encode("utf-8")
    return {
        "path": f"eval/semantic-http-probe/{nonce}.md",
        "content": content,
        "content_sha256": hashlib.sha256(encoded).hexdigest(),
        "media_type": "text/markdown",
    }


def cleanup_fixture(
    scoped: NativeApiClient,
    *,
    status_url: str,
) -> dict[str, Any]:
    receipt: dict[str, Any] = {
        "attempted": True,
        "status_url_sha256": hashlib.sha256(
            status_url.encode("utf-8")
        ).hexdigest(),
        "response": None,
        "credential_revocation_verified": False,
        "pass": False,
        "error": None,
    }
    try:
        response = scoped.request("DELETE", status_url)
        public_response = recursively_redact_secrets(response.body)
        receipt["response"] = {
            "http_status": response.http_status,
            "body": public_response,
        }
        cleaned = response.data
        receipt["cleanup_receipt"] = {
            key: cleaned.get(key)
            for key in (
                "status",
                "import_id",
                "entries_removed",
                "search_chunks_removed",
                "jobs_removed",
                "credentials_revoked",
                "revoked_at",
            )
        }
        try:
            scoped.get("/v1/status")
        except NativeApiError as error:
            receipt["credential_revocation_verified"] = error.status == 401
        receipt["pass"] = bool(
            cleaned.get("status") == "cleaned"
            and isinstance(cleaned.get("entries_removed"), int)
            and cleaned["entries_removed"] >= 1
            and isinstance(cleaned.get("credentials_revoked"), int)
            and cleaned["credentials_revoked"] >= 1
            and receipt["credential_revocation_verified"]
        )
    except Exception as error:
        receipt["error"] = f"{type(error).__name__}: {error}"
    return receipt


def run_probe(args: argparse.Namespace) -> dict[str, Any]:
    if not SAFE_ID_PATTERN.fullmatch(args.run_id):
        raise ValueError("--run-id must be a safe 1-128 character identifier")
    if not SAFE_ID_PATTERN.fullmatch(args.marker_prefix):
        raise ValueError(
            "--marker-prefix must be a safe 1-128 character identifier"
        )
    if args.provision_timeout <= 0:
        raise ValueError("--provision-timeout must be positive")
    nonce = uuid.uuid4().hex
    marker = f"{args.marker_prefix}-{nonce}"
    query = args.query.strip()
    if not query:
        raise ValueError("--query must not be empty")
    document = probe_document(nonce=nonce, marker=marker, query=query)
    case_id = f"semantic-http-probe-{nonce}"
    admin = NativeApiClient(
        run_id=args.run_id,
        case_id=case_id,
        timeout=max(30.0, args.max_response_ms / 1_000 + 5),
    )
    metadata: dict[str, Any] | None = None
    scoped: NativeApiClient | None = None
    contract: dict[str, Any] | None = None
    lifecycle_error: str | None = None
    cleanup: dict[str, Any] = {
        "attempted": False,
        "pass": False,
        "error": "fixture was not provisioned",
    }
    try:
        metadata = provision_evaluation(
            admin,
            run_id=args.run_id,
            case_id=case_id,
            display_scope=f"Semantic HTTP probe {nonce}",
            access_mode="read_write",
            documents=[document],
            timeout_seconds=args.provision_timeout,
            import_path="/v1/workspace/admin/eval/import",
            wait_for_semantic=True,
        )
        scoped = NativeApiClient(
            base_url=admin.base_url,
            token=str(metadata["token"]),
            run_id=args.run_id,
            case_id=case_id,
            timeout=max(30.0, args.max_response_ms / 1_000 + 5),
        )
        contract = run_contract(
            args,
            client=scoped,
            query=query,
            marker=marker,
        )
    except Exception as error:
        lifecycle_error = f"{type(error).__name__}: {error}"
    finally:
        if scoped is not None and metadata is not None:
            status_url = str(metadata.get("status_url") or "")
            if status_url:
                cleanup = cleanup_fixture(
                    scoped,
                    status_url=status_url,
                )
            else:
                cleanup = {
                    "attempted": False,
                    "pass": False,
                    "error": "provisioning receipt omitted status_url",
                }
    if contract is None:
        contract = {
            "schema": SCHEMA,
            "created_at": datetime.now().astimezone().isoformat(
                timespec="seconds"
            ),
            "status": "failed",
            "pass": False,
            "definitive": bool(
                getattr(args, "require_hook_attestation", False)
            ),
            "error": lifecycle_error,
            "conclusion": "The full HTTP deadline/cache contract was not proven.",
        }
    contract["fixture"] = {
        "run_id": args.run_id,
        "case_id": case_id,
        "path": document["path"],
        "content_sha256": document["content_sha256"],
        "content_bytes": len(document["content"].encode("utf-8")),
        "marker": marker,
        "marker_sha256": hashlib.sha256(marker.encode("utf-8")).hexdigest(),
        "credential_recorded": False,
        "provisioning": (
            public_provisioning(metadata)
            if metadata is not None
            else None
        ),
    }
    contract["cleanup"] = cleanup
    contract["pass"] = bool(contract.get("pass") and cleanup.get("pass"))
    contract["status"] = "passed" if contract["pass"] else "failed"
    if lifecycle_error and not contract.get("error"):
        contract["error"] = lifecycle_error
    if not cleanup.get("pass"):
        contract["error"] = contract.get("error") or (
            "evaluation fixture cleanup or credential revocation failed"
        )
    return contract


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run a full HTTP slow-provider/deadline/cache probe"
    )
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--query", default=DEFAULT_QUERY)
    parser.add_argument(
        "--marker-prefix",
        "--marker",
        dest="marker_prefix",
        default=DEFAULT_MARKER_PREFIX,
        help=(
            "safe marker prefix; a unique nonce is always appended before "
            "the evaluation fixture is provisioned"
        ),
    )
    parser.add_argument("--slow-command", required=True)
    parser.add_argument("--restore-command", required=True)
    parser.add_argument("--injected-delay-ms", type=int, default=800)
    parser.add_argument("--deadline-ms", type=int, default=300)
    parser.add_argument("--max-response-ms", type=int, default=750)
    parser.add_argument("--settle-ms", type=int, default=100)
    parser.add_argument("--warm-wait-ms", type=int)
    parser.add_argument("--hook-timeout", type=float, default=30.0)
    parser.add_argument("--provision-timeout", type=float, default=1_800.0)
    parser.add_argument(
        "--allow-unattested-hooks",
        action="store_false",
        dest="require_hook_attestation",
        help=(
            "permit a visibly non-definitive local probe without fault-proxy "
            "hook attestations"
        ),
    )
    parser.set_defaults(require_hook_attestation=True)
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

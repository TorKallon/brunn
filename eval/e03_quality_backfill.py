#!/usr/bin/env python3

"""Provision and verify the E03 quality-suite semantic corpus.

The command deliberately uses the ordinary simple-core evaluation import and
waits for its worker-owned embedding jobs.  It does not call an embedding
provider directly and never writes a scoped credential to the result artifact.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence


PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from agent_work_eval import sha256_file, validate  # noqa: E402
from native_eval import (  # noqa: E402
    NativeApiClient,
    provision_evaluation,
    recursively_redact_secrets,
    text_documents,
)


SCHEMA = "straylight-e03-quality-backfill@v1"
RECONCILIATION_SCHEMA = (
    "straylight-e03-quality-receipt-reconciliation@v1"
)
DEFAULT_MANIFESTS = (
    PROJECT_ROOT / "eval" / "work_cases.json",
    PROJECT_ROOT / "eval" / "rupture_ops_cases.json",
    PROJECT_ROOT / "eval" / "recent_work_cases.json",
)
DEFAULT_SCHEMA = PROJECT_ROOT / "eval" / "work_answer_schema.json"
DEFAULT_USD_PER_MILLION_TOKENS = 0.02
DEFAULT_CEILING_USD = 5.0
CHUNK_OVERLAP_ALLOWANCE = 1.25
SEMANTIC_ARMS = 2
DEFINITIVE_DRAWS = 3
SHA256_PATTERN = re.compile(r"^(?:sha256:)?([0-9a-f]{64})$")


def estimated_tokens(characters: int) -> int:
    if characters < 0:
        raise ValueError("characters must not be negative")
    return math.ceil(characters / 4 * CHUNK_OVERLAP_ALLOWANCE)


def estimated_usd(tokens: int, usd_per_million_tokens: float) -> float:
    if tokens < 0:
        raise ValueError("tokens must not be negative")
    if not math.isfinite(usd_per_million_tokens) or usd_per_million_tokens < 0:
        raise ValueError("embedding unit price must be finite and nonnegative")
    return tokens / 1_000_000 * usd_per_million_tokens


def load_suites(
    manifests: Sequence[Path],
    schema: Path,
) -> list[dict[str, Any]]:
    suites: list[dict[str, Any]] = []
    seen_corpora: set[str] = set()
    for manifest_path in manifests:
        validated = validate(manifest_path, schema)
        if validated["errors"]:
            raise ValueError(
                f"{manifest_path}: " + "; ".join(validated["errors"])
            )
        corpus_sha256 = str(validated["corpus_sha256"])
        if corpus_sha256 in seen_corpora:
            raise ValueError(
                "quality backfill manifests must name distinct corpora; "
                f"{manifest_path} duplicates {corpus_sha256}"
            )
        seen_corpora.add(corpus_sha256)
        active_cases = [
            case
            for case in validated["manifest"]["cases"]
            if case.get("active", True)
        ]
        documents = validated["documents"]
        suites.append(
            {
                "slug": manifest_path.stem.removesuffix("_cases"),
                "manifest_path": str(manifest_path.relative_to(PROJECT_ROOT)),
                "manifest_sha256": sha256_file(manifest_path),
                "benchmark_version": validated["manifest"]["benchmark_version"],
                "corpus_root": str(
                    validated["corpus_root"].relative_to(PROJECT_ROOT)
                ),
                "corpus_sha256": corpus_sha256,
                "artifact_tree_sha256": validated["artifact_tree_sha256"],
                "documents": documents,
                "document_count": len(documents),
                "characters": sum(len(document.text) for document in documents),
                "active_cases": len(active_cases),
            }
        )
    return suites


def cost_preflight(
    suites: Sequence[dict[str, Any]],
    *,
    provider_mode: str,
    usd_per_million_tokens: float,
    ceiling_usd: float,
) -> dict[str, Any]:
    if provider_mode not in {"openai", "mock"}:
        raise ValueError("provider_mode must be openai or mock")
    if not math.isfinite(ceiling_usd) or ceiling_usd <= 0:
        raise ValueError("embedding ceiling must be finite and positive")
    unit_price = 0.0 if provider_mode == "mock" else usd_per_million_tokens
    suite_rows = []
    backfill_tokens = 0
    projected_e09_tokens = 0
    for suite in suites:
        tokens = estimated_tokens(int(suite["characters"]))
        e09_copies = int(suite["active_cases"]) * SEMANTIC_ARMS * DEFINITIVE_DRAWS
        e09_tokens = tokens * e09_copies
        backfill_tokens += tokens
        projected_e09_tokens += e09_tokens
        suite_rows.append(
            {
                "suite": suite["slug"],
                "characters": suite["characters"],
                "estimated_input_tokens": tokens,
                "quality_backfill_usd": round(
                    estimated_usd(tokens, unit_price), 6
                ),
                "e09_isolated_provisioning_copies": e09_copies,
                "e09_isolated_provisioning_tokens": e09_tokens,
                "e09_isolated_provisioning_usd": round(
                    estimated_usd(e09_tokens, unit_price), 6
                ),
            }
        )
    quality_usd = estimated_usd(backfill_tokens, unit_price)
    e09_usd = estimated_usd(projected_e09_tokens, unit_price)
    projected_total = quality_usd + e09_usd
    return {
        "provider_mode": provider_mode,
        "model": "text-embedding-3-small",
        "usd_per_million_tokens": unit_price,
        "token_estimator": (
            "ceil(exact UTF-8-decoded source characters / 4 * 1.25)"
        ),
        "chunk_overlap_allowance": CHUNK_OVERLAP_ALLOWANCE,
        "quality_backfill": {
            "estimated_input_tokens": backfill_tokens,
            "estimated_usd": round(quality_usd, 6),
        },
        "e09_isolated_provisioning_projection": {
            "semantic_arms": SEMANTIC_ARMS,
            "draws": DEFINITIVE_DRAWS,
            "estimated_input_tokens": projected_e09_tokens,
            "estimated_usd": round(e09_usd, 6),
            "reason": (
                "agent_work_eval provisions a case-isolated user for every "
                "semantic-arm case run, so corpus embeddings cannot be assumed "
                "to deduplicate across cases or draws"
            ),
        },
        "projected_total_usd": round(projected_total, 6),
        "ceiling_usd": ceiling_usd,
        "ceiling_pass": projected_total <= ceiling_usd,
        "suites": suite_rows,
    }


def semantic_gap(value: Any) -> bool:
    if isinstance(value, dict):
        failures = value.get("lane_failures")
        if isinstance(failures, list) and any(
            str(item).casefold().startswith("semantic")
            for item in failures
        ):
            return True
        if (
            str(value.get("lane", "")).casefold() == "semantic"
            and any(
                marker in str(
                    value.get("kind")
                    or value.get("status")
                    or value.get("message")
                    or ""
                ).casefold()
                for marker in ("unavailable", "deferred", "failed")
            )
        ):
            return True
        return any(semantic_gap(item) for item in value.values())
    if isinstance(value, list):
        return any(semantic_gap(item) for item in value)
    return False


def candidate_count(value: Any) -> int:
    if isinstance(value, dict):
        own = value.get("candidates")
        count = len(own) if isinstance(own, list) else 0
        return count + sum(candidate_count(item) for item in value.values())
    if isinstance(value, list):
        return sum(candidate_count(item) for item in value)
    return 0


def validate_rate_limit_snapshot(value: dict[str, Any]) -> dict[str, Any]:
    features = value.get("runtime_features")
    if not isinstance(features, dict):
        raise ValueError(
            "/ready omitted runtime_features; the D11 rate-limit build is required"
        )
    batch = features.get("embedding_backfill_batch_chunks")
    delay = features.get("embedding_backfill_inter_batch_ms")
    guard = features.get("embedding_backfill_guard")
    foreground_status_url_configured = features.get(
        "embedding_backfill_foreground_status_url_configured"
    )
    valid = (
        guard is True
        and foreground_status_url_configured is True
        and isinstance(batch, int)
        and not isinstance(batch, bool)
        and 1 <= batch <= 64
        and isinstance(delay, int)
        and not isinstance(delay, bool)
        and delay >= 250
    )
    snapshot = {
        "embedding_backfill_guard": guard,
        "embedding_backfill_batch_chunks": batch,
        "embedding_backfill_inter_batch_ms": delay,
        "foreground_status_url_configured": foreground_status_url_configured,
        "pass": valid,
    }
    if not valid:
        raise ValueError(f"invalid embedding backfill rate-limit snapshot: {snapshot}")
    return snapshot


def run_backfill(
    suites: Sequence[dict[str, Any]],
    *,
    run_id: str,
    timeout_seconds: float,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    admin = NativeApiClient(run_id=run_id, case_id="e03-quality-backfill")
    ready = admin.get("/ready")
    rate_limit = validate_rate_limit_snapshot(ready.data)
    results = []
    for suite in suites:
        case_id = f"e03-quality-{suite['slug']}"
        started = time.monotonic()
        metadata = provision_evaluation(
            admin,
            run_id=run_id,
            case_id=case_id,
            display_scope=f"E03 quality corpus: {suite['slug']}",
            access_mode="read_only",
            documents=text_documents(PROJECT_ROOT / suite["corpus_root"]),
            timeout_seconds=timeout_seconds,
            import_path="/v1/workspace/admin/eval/import",
            wait_for_semantic=True,
            batch_size=10_000,
        )
        backfill_elapsed_ms = (time.monotonic() - started) * 1_000
        scoped = NativeApiClient(
            token=str(metadata["token"]),
            run_id=run_id,
            case_id=case_id,
            timeout=min(timeout_seconds, 120.0),
        )
        probe = scoped.post(
            "/v1/workspace/search",
            {
                "queries": [
                    {
                        "id": "e03-semantic-warm-probe",
                        "query": (
                            "Find the most relevant source for this frozen "
                            f"{suite['slug']} evaluation corpus."
                        ),
                        "modes": ["semantic"],
                        "limit": 1,
                    }
                ]
            },
        )
        candidates = candidate_count(probe.body)
        gap = semantic_gap(probe.body)
        if gap or candidates < 1:
            raise RuntimeError(
                f"{suite['slug']} warm probe did not prove semantic coverage "
                f"(gap={gap}, candidates={candidates})"
            )
        provisioning = metadata["provisioning"]
        results.append(
            {
                "suite": suite["slug"],
                "import_id": metadata.get("import_id"),
                "corpus_revision": metadata.get("corpus_revision"),
                "documents": suite["document_count"],
                "characters": suite["characters"],
                "backfill_wall_clock_ms": round(backfill_elapsed_ms, 3),
                "import_http_status": provisioning.get("import_http_status"),
                "import_batch_count": provisioning.get("import_batch_count"),
                "index_status_at_start": recursively_redact_secrets(
                    provisioning.get("index_status_at_start", {})
                ),
                "semantic_waited": provisioning.get("semantic_waited"),
                "warm_probe": {
                    "http_status": probe.http_status,
                    "elapsed_ms": round(probe.elapsed_ms, 3),
                    "semantic_gap": gap,
                    "candidate_count": candidates,
                    "pass": True,
                },
            }
        )
    return results, rate_limit


def actual_accounting(
    suites: Sequence[dict[str, Any]],
    *,
    provider_mode: str,
    usd_per_million_tokens: float,
    billed_input_tokens: int | None,
    billed_usd: float | None,
) -> dict[str, Any]:
    exact_characters = sum(int(suite["characters"]) for suite in suites)
    estimated = estimated_tokens(exact_characters)
    if provider_mode == "mock":
        return {
            "accounting_status": "mock_provider_zero_spend",
            "exact_imported_characters": exact_characters,
            "chunk_overlap_allowance": CHUNK_OVERLAP_ALLOWANCE,
            "accounted_input_tokens": 0,
            "accounted_usd": 0.0,
            "provider_receipt": False,
        }
    if billed_input_tokens is not None and billed_input_tokens < 0:
        raise ValueError("--billed-input-tokens must not be negative")
    if billed_usd is not None and (
        not math.isfinite(billed_usd) or billed_usd < 0
    ):
        raise ValueError("--billed-usd must be finite and nonnegative")
    receipt = billed_input_tokens is not None or billed_usd is not None
    accounted_tokens = billed_input_tokens if billed_input_tokens is not None else estimated
    accounted_cost = (
        billed_usd
        if billed_usd is not None
        else estimated_usd(accounted_tokens, usd_per_million_tokens)
    )
    return {
        "accounting_status": (
            "provider_receipt"
            if receipt
            else "estimated_from_exact_imported_characters_no_provider_receipt"
        ),
        "exact_imported_characters": exact_characters,
        "chunk_overlap_allowance": CHUNK_OVERLAP_ALLOWANCE,
        "accounted_input_tokens": accounted_tokens,
        "accounted_usd": round(accounted_cost, 6),
        "provider_receipt": receipt,
        "billed_input_tokens": billed_input_tokens,
        "billed_usd": billed_usd,
    }


def artifact_base(
    args: argparse.Namespace,
    suites: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    preflight = cost_preflight(
        suites,
        provider_mode=args.provider_mode,
        usd_per_million_tokens=args.usd_per_million_tokens,
        ceiling_usd=args.ceiling_usd,
    )
    return {
        "schema": SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "status": "estimated",
        "pass": preflight["ceiling_pass"],
        "run_id": getattr(args, "run_id", None),
        "implementation": {
            "harness": str(Path(__file__).relative_to(PROJECT_ROOT)),
            "harness_sha256": sha256_file(Path(__file__)),
        },
        "cost": {
            "preflight": preflight,
            "actual": None,
        },
        "suites": [
            {
                key: value
                for key, value in suite.items()
                if key != "documents"
            }
            for suite in suites
        ],
        "rate_limit": None,
        "backfills": [],
        "errors": [],
    }


def write_artifact(path: Path, artifact: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(artifact, indent=2) + "\n", encoding="utf-8")


def normalize_sha256(value: str, *, field: str) -> str:
    match = SHA256_PATTERN.fullmatch(value.casefold())
    if match is None:
        raise ValueError(f"{field} must be a lowercase SHA-256 digest")
    return match.group(1)


def reconcile_receipt(
    *,
    input_path: Path,
    expected_input_sha256: str,
    run_id: str,
    provider_receipt_path: Path,
    billed_input_tokens: int,
    billed_usd: float,
) -> dict[str, Any]:
    """Derive a receipt-bound accounting artifact without changing the run."""

    if input_path.resolve() == provider_receipt_path.resolve():
        raise ValueError(
            "provider receipt must differ from the run artifact"
        )
    expected_digest = normalize_sha256(
        expected_input_sha256,
        field="--input-sha256",
    )
    source_bytes = input_path.read_bytes()
    source_digest = hashlib.sha256(source_bytes).hexdigest()
    if source_digest != expected_digest:
        raise ValueError(
            "input artifact SHA-256 does not match --input-sha256"
        )
    try:
        source = json.loads(source_bytes)
    except json.JSONDecodeError as error:
        raise ValueError("input artifact is not valid JSON") from error
    if not isinstance(source, dict) or source.get("schema") != SCHEMA:
        raise ValueError("input is not an E03 quality-backfill artifact")
    if source.get("run_id") != run_id or not run_id.strip():
        raise ValueError("input artifact run_id does not match --run-id")
    if (
        source.get("status") != "complete"
        or source.get("pass") is not True
        or source.get("errors") != []
    ):
        raise ValueError(
            "only a complete, passing, error-free run can be reconciled"
        )
    implementation = source.get("implementation")
    current_harness_sha256 = sha256_file(Path(__file__))
    if (
        not isinstance(implementation, dict)
        or implementation.get("harness")
        != str(Path(__file__).relative_to(PROJECT_ROOT))
        or implementation.get("harness_sha256")
        != current_harness_sha256
    ):
        raise ValueError(
            "input artifact is not bound to this reconciliation harness"
        )
    cost = source.get("cost")
    preflight = cost.get("preflight") if isinstance(cost, dict) else None
    original_actual = cost.get("actual") if isinstance(cost, dict) else None
    if (
        not isinstance(preflight, dict)
        or preflight.get("provider_mode") != "openai"
        or preflight.get("ceiling_pass") is not True
        or not isinstance(original_actual, dict)
        or original_actual.get("provider_receipt") is not False
        or original_actual.get("accounting_status")
        != "estimated_from_exact_imported_characters_no_provider_receipt"
        or original_actual.get("billed_input_tokens") is not None
        or original_actual.get("billed_usd") is not None
    ):
        raise ValueError(
            "input must be an unreconciled real-provider run"
        )
    if (
        type(billed_input_tokens) is not int
        or billed_input_tokens <= 0
    ):
        raise ValueError("--billed-input-tokens must be a positive integer")
    if not math.isfinite(billed_usd) or billed_usd <= 0:
        raise ValueError("--billed-usd must be finite and positive")
    unit_price = preflight.get("usd_per_million_tokens")
    ceiling_usd = preflight.get("ceiling_usd")
    if (
        not isinstance(unit_price, (int, float))
        or isinstance(unit_price, bool)
        or not math.isfinite(unit_price)
        or unit_price <= 0
        or not isinstance(ceiling_usd, (int, float))
        or isinstance(ceiling_usd, bool)
        or not math.isfinite(ceiling_usd)
        or ceiling_usd <= 0
    ):
        raise ValueError("input artifact has invalid pricing constraints")
    expected_billed_usd = estimated_usd(
        billed_input_tokens,
        float(unit_price),
    )
    receipt_tolerance_usd = max(0.000001, expected_billed_usd * 0.000001)
    if abs(billed_usd - expected_billed_usd) > receipt_tolerance_usd:
        raise ValueError(
            "billed token and USD values are inconsistent with the "
            "artifact's embedding unit price"
        )
    if billed_usd > float(ceiling_usd):
        raise ValueError("provider receipt exceeds the approved ceiling")
    if (
        not provider_receipt_path.is_file()
        or provider_receipt_path.stat().st_size <= 0
    ):
        raise ValueError("--provider-receipt must be a nonempty regular file")
    receipt_bytes = provider_receipt_path.read_bytes()
    receipt_digest = hashlib.sha256(receipt_bytes).hexdigest()
    reconciled_actual = {
        **original_actual,
        "accounting_status": "provider_receipt_reconciled",
        "accounted_input_tokens": billed_input_tokens,
        "accounted_usd": round(billed_usd, 6),
        "provider_receipt": True,
        "billed_input_tokens": billed_input_tokens,
        "billed_usd": round(billed_usd, 6),
    }
    return {
        "schema": RECONCILIATION_SCHEMA,
        "created_at": datetime.now().astimezone().isoformat(
            timespec="seconds"
        ),
        "status": "reconciled",
        "pass": True,
        "run_id": run_id,
        "source": {
            "artifact": str(input_path),
            "artifact_schema": SCHEMA,
            "artifact_sha256": source_digest,
            "harness_sha256": implementation["harness_sha256"],
        },
        "implementation": {
            "harness": str(Path(__file__).relative_to(PROJECT_ROOT)),
            "harness_sha256": current_harness_sha256,
        },
        "provider_receipt": {
            "sha256": receipt_digest,
            "size_bytes": len(receipt_bytes),
            "contents_recorded": False,
        },
        "constraints": {
            "usd_per_million_tokens": unit_price,
            "expected_billed_usd": round(expected_billed_usd, 9),
            "receipt_tolerance_usd": round(receipt_tolerance_usd, 9),
            "ceiling_usd": ceiling_usd,
            "ceiling_pass": True,
            "token_usd_consistency_pass": True,
        },
        "original_actual": original_actual,
        "reconciled_actual": reconciled_actual,
    }


def write_artifact_exclusive(path: Path, artifact: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as output:
        output.write(json.dumps(artifact, indent=2) + "\n")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Estimate or run E03 quality-suite semantic backfill"
    )
    parser.add_argument(
        "--manifest",
        action="append",
        type=Path,
        dest="manifests",
        help="quality-suite manifest; defaults to work, rupture_ops, recent_work",
    )
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    parser.add_argument(
        "--provider-mode",
        choices=("openai", "mock"),
        default="openai",
    )
    parser.add_argument(
        "--usd-per-million-tokens",
        type=float,
        default=DEFAULT_USD_PER_MILLION_TOKENS,
    )
    parser.add_argument("--ceiling-usd", type=float, default=DEFAULT_CEILING_USD)
    subparsers = parser.add_subparsers(dest="command", required=True)
    estimate = subparsers.add_parser("estimate")
    estimate.add_argument("--out", type=Path, required=True)
    run = subparsers.add_parser("run")
    run.add_argument("--run-id", required=True)
    run.add_argument("--timeout", type=float, default=1_800.0)
    run.add_argument("--billed-input-tokens", type=int)
    run.add_argument("--billed-usd", type=float)
    run.add_argument("--out", type=Path, required=True)
    reconcile = subparsers.add_parser("reconcile-receipt")
    reconcile.add_argument("--input", type=Path, required=True)
    reconcile.add_argument("--input-sha256", required=True)
    reconcile.add_argument("--run-id", required=True)
    reconcile.add_argument("--provider-receipt", type=Path, required=True)
    reconcile.add_argument("--billed-input-tokens", type=int, required=True)
    reconcile.add_argument("--billed-usd", type=float, required=True)
    reconcile.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "reconcile-receipt":
        if args.out.resolve() in {
            args.input.resolve(),
            args.provider_receipt.resolve(),
        }:
            raise ValueError(
                "reconciliation output must differ from its source artifacts"
            )
        before = hashlib.sha256(args.input.read_bytes()).hexdigest()
        artifact = reconcile_receipt(
            input_path=args.input,
            expected_input_sha256=args.input_sha256,
            run_id=args.run_id,
            provider_receipt_path=args.provider_receipt,
            billed_input_tokens=args.billed_input_tokens,
            billed_usd=args.billed_usd,
        )
        if hashlib.sha256(args.input.read_bytes()).hexdigest() != before:
            raise RuntimeError(
                "input artifact changed during receipt reconciliation"
            )
        write_artifact_exclusive(args.out, artifact)
        print(json.dumps({
            "status": "reconciled",
            "out": str(args.out),
            "run_id": artifact["run_id"],
            "source_sha256": artifact["source"]["artifact_sha256"],
            "provider_receipt_sha256": (
                artifact["provider_receipt"]["sha256"]
            ),
        }, indent=2))
        return 0
    manifests = tuple(args.manifests or DEFAULT_MANIFESTS)
    suites = load_suites(manifests, args.schema)
    artifact = artifact_base(args, suites)
    if not artifact["cost"]["preflight"]["ceiling_pass"]:
        artifact["status"] = "aborted_preflight_ceiling"
        artifact["pass"] = False
        artifact["errors"].append(
            {
                "type": "EmbeddingCeilingExceeded",
                "message": "projected embeddings spend exceeds the configured ceiling",
            }
        )
        write_artifact(args.out, artifact)
        return 2
    if args.command == "estimate":
        write_artifact(args.out, artifact)
        return 0
    if args.timeout <= 0:
        raise ValueError("--timeout must be positive")
    try:
        backfills, rate_limit = run_backfill(
            suites,
            run_id=args.run_id,
            timeout_seconds=args.timeout,
        )
        actual = actual_accounting(
            suites,
            provider_mode=args.provider_mode,
            usd_per_million_tokens=args.usd_per_million_tokens,
            billed_input_tokens=args.billed_input_tokens,
            billed_usd=args.billed_usd,
        )
        artifact["backfills"] = backfills
        artifact["rate_limit"] = rate_limit
        artifact["cost"]["actual"] = actual
        artifact["pass"] = (
            all(item["warm_probe"]["pass"] for item in backfills)
            and actual["accounted_usd"] <= args.ceiling_usd
        )
        artifact["status"] = "complete" if artifact["pass"] else "failed"
    except Exception as error:  # preserve an operator-usable partial artifact
        artifact["status"] = "failed"
        artifact["pass"] = False
        artifact["errors"].append(
            {"type": type(error).__name__, "message": str(error)}
        )
    write_artifact(args.out, artifact)
    return 0 if artifact["pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())

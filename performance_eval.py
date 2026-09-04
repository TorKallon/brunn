#!/usr/bin/env python3

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import ipaddress
import json
import os
import re
import shlex
import shutil
import statistics
import subprocess
import tempfile
import threading
import time
import urllib.parse
import uuid
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterable, Iterator, Mapping, Sequence

from native_eval import (
    NativeApiClient,
    NativeApiError,
    provision_evaluation,
    public_provisioning,
    recursively_redact_secrets,
)
from eval.e09_step_authorization import load_step_authorization
from eval.hook_provenance import (
    hook_target_matches,
    run_hook as run_provenance_hook,
)
from eval.semantic_http_probe import cleanup_fixture
from semantic_eval_policy import (
    response_has_candidates,
    semantic_counter_delta,
    semantic_rates,
    validate_e09_runtime,
)


PROJECT_ROOT = Path(__file__).resolve().parent
DEFAULT_SCALES = (1_000, 10_000, 64_000)
PRODUCTION_RECORDS = 64_000
FUTURE_RECORDS = 640_000
DEFINITIVE_SAMPLES = 30
QUICK_SAMPLES = 3
VERBATIM_IDENTIFIER_PROBES = 30
VERBATIM_IDENTIFIER_MIN_OFFSET = 2_401
CONCURRENT_SEARCHES_PER_ROUND = 5
BROAD_QUERY = "deterministic performance-fixture material"
OLD_SOURCE_QUERY = (
    "Reconcile the meridian continuity doctrine with a new request and explain "
    "durable workspace source authority for a fresh agent."
)
LEXICAL_CONSOLIDATION_GATE_PROFILE = "e05-lexical-consolidation"
D03_RESUME_DELTAS_GATE_PROFILE = "d03-resume-deltas"
D03_RESUME_QUERY_COUNT_DELTA = 5
E03_SEMANTIC_READY_GATE_PROFILE = "e03-semantic-ready"
E03_ARMS = ("mode1", "mode2", "mode3")
E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS = 43_200.0
E03_WRAPPER_TIMEOUT_SECONDS = 45_000.0
SEMANTIC_FAILURE_PROBE_REQUIRED = "required"
SEMANTIC_FAILURE_PROBE_NOT_APPLICABLE = "not-applicable"
VERBATIM_FEATURE_ACCEPTANCE_REQUIRED = "required"
VERBATIM_FEATURE_ACCEPTANCE_NOT_APPLICABLE = "not-applicable"
DEFAULT_QUERY_BUDGET_PROFILE = "default-safe"
E03_FAILURE_WINDOW_SEARCH_SLO_MS = 3_000.0
E03_EMBEDDING_COST_CEILING_USD = 5.0
E03_MODE3_PREFLIGHT_MAX_USD = 2.5
E03_EMBEDDING_DIMENSIONS = 1_536
E03_EMBEDDING_MODELS = {
    "mode1": "brunn-hashing-v1",
    "mode2": "text-embedding-3-small",
    "mode3": "text-embedding-3-small",
}
E03_COMMON_RUNTIME_EXPECTATIONS: dict[str, Any] = {
    "allow_degraded_embeddings": False,
    "embed_cache": False,
    "embedding_backfill_batch_chunks": 64,
    "embedding_backfill_guard": True,
    "embedding_backfill_foreground_status_url_configured": True,
    "embedding_backfill_foreground_status_timeout_ms": 1_000,
    "embedding_backfill_inter_batch_ms": 250,
    "embedding_backfill_open_p95_limit_ms": 120.0,
    "embedding_backfill_search_p95_limit_ms": 107.0,
    "intention_ledger": False,
    "lexical_single_scan": False,
    "materialize_token_budget": 24_000,
    "observability_timings_ms": True,
    "read_path_roundtrip_v1": False,
    "resume_deltas": False,
    "search_char_cap": False,
    "search_fair_share": False,
    "search_section_demotion_top_n": None,
    "search_top1_hydration": False,
    "semantic_deadline_ms": None,
    "supersession_demotion": False,
    "supersession_demotion_weight": 1.5,
    "verbatim_spans": False,
}
LEXICAL_CONSOLIDATION_REQUIRED_GATES = frozenset({
    "all_required_scales_completed",
    "retrieval_sample_count_is_definitive",
    "query_count_sample_cardinality_is_authoritative",
    "verbatim_identifier_measurement_integrity",
    "bounded_lexical_overflow_returns_late_relevant_source",
    "old_relevant_source_survives_many_newer_writes",
    "no_exact_or_lexical_lane_failures",
    "unrelated_write_commits",
    "retrieval_survives_unrelated_write",
    "concurrent_exact_and_lexical_lanes_remain_healthy",
    "foreground_write_sample_count_is_definitive",
})
BOOLEAN_RUNTIME_FEATURES = frozenset({
    "allow_degraded_embeddings",
    "embed_cache",
    "embedding_backfill_guard",
    "embedding_backfill_foreground_status_url_configured",
    "supersession_demotion",
    "intention_ledger",
    "read_path_roundtrip_v1",
    "lexical_single_scan",
    "observability_timings_ms",
    "resume_deltas",
    "search_char_cap",
    "search_fair_share",
    "search_top1_hydration",
    "semantic_lane",
    "verbatim_spans",
})
RUNTIME_FEATURES = BOOLEAN_RUNTIME_FEATURES | frozenset({
    "embedding_backfill_batch_chunks",
    "embedding_backfill_foreground_status_timeout_ms",
    "embedding_backfill_inter_batch_ms",
    "embedding_backfill_open_p95_limit_ms",
    "embedding_backfill_search_p95_limit_ms",
    "materialize_token_budget",
    "search_section_demotion_top_n",
    "semantic_deadline_ms",
    "supersession_demotion_weight",
})
CURRENT_RUNTIME_FEATURES = frozenset(RUNTIME_FEATURES)
SERVICE_BOOLEAN_FEATURE_FLAGS = BOOLEAN_RUNTIME_FEATURES
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
REGRESSION_THRESHOLDS = {
    "open_p95_ms": 500.0,
    "search_p95_ms": 500.0,
    "broad_search_p95_ms": 500.0,
    "overflow_search_p95_ms": 500.0,
    "old_source_search_p95_ms": 500.0,
    "read_p95_ms": 100.0,
    "checkpoint_ms": 200.0,
    "resume_ms": 400.0,
    "concurrent_write_p95_ms": 500.0,
    "concurrent_search_p95_ms": 750.0,
}
QUERY_BUDGETS_PATH = PROJECT_ROOT / "eval" / "query_budgets.json"
D03_QUERY_BUDGETS_PATH = (
    PROJECT_ROOT / "eval" / "query_budgets.d03-resume-deltas.json"
)
RETRIEVAL_PLAN_CONTRACT_PATH = (
    PROJECT_ROOT / "eval" / "retrieval_plan_contract.json"
)


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
    semantic_failure_probe_posture: str


def percentile(values: Iterable[float], quantile: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] * (1 - fraction) + ordered[upper] * fraction


def parse_feature_states(values: Sequence[str] | None) -> dict[str, bool]:
    states: dict[str, bool] = {}
    for raw in values or ():
        name, separator, value = raw.partition("=")
        name = name.strip()
        normalized = value.strip().lower()
        if not separator or not name or normalized not in {
            "on",
            "off",
            "true",
            "false",
            "1",
            "0",
        }:
            raise ValueError(
                "--feature-state must use NAME=on|off (true/false and 1/0 are accepted)"
            )
        enabled = normalized in {"on", "true", "1"}
        if name not in SERVICE_BOOLEAN_FEATURE_FLAGS:
            raise ValueError(
                f"unknown service feature state {name}; expected one of "
                + ", ".join(sorted(SERVICE_BOOLEAN_FEATURE_FLAGS))
            )
        if name in states and states[name] != enabled:
            raise ValueError(f"conflicting feature states declared for {name}")
        states[name] = enabled
    return dict(sorted(states.items()))


def expected_feature_flags(values: Sequence[str] | None) -> dict[str, bool]:
    parsed: dict[str, bool] = {}
    for value in values or ():
        name, separator, state = value.partition("=")
        if (
            not separator
            or name not in BOOLEAN_RUNTIME_FEATURES
            or state not in {"on", "off"}
        ):
            raise ValueError(
                "--expect-feature-flag requires a known boolean runtime feature "
                "formatted as NAME=on|off"
            )
        if name in parsed:
            raise ValueError(f"duplicate expected feature flag: {name}")
        parsed[name] = state == "on"
    return parsed


def expected_runtime_features(
    flag_values: Sequence[str] | None,
    config_values: Sequence[str] | None,
) -> dict[str, Any]:
    parsed: dict[str, Any] = dict(expected_feature_flags(flag_values))
    for value in config_values or ():
        name, separator, rendered = value.partition("=")
        if not separator or name not in RUNTIME_FEATURES:
            raise ValueError(
                "--expect-runtime-config requires a known runtime feature "
                "formatted as NAME=<JSON value>"
            )
        if name in parsed:
            raise ValueError(f"duplicate expected runtime feature: {name}")
        try:
            parsed[name] = json.loads(rendered)
        except json.JSONDecodeError as exc:
            raise ValueError(
                f"--expect-runtime-config value for {name} is not valid JSON"
            ) from exc
    return dict(sorted(parsed.items()))


def capture_service_runtime_snapshot(
    status: dict[str, Any],
    *,
    expected_features: dict[str, Any],
    expected_build_revision: str | None,
) -> dict[str, Any]:
    if status.get("status") != "ready":
        raise ValueError("service status is not ready")
    build_revision = status.get("build_revision")
    if (
        not isinstance(build_revision, str)
        or not build_revision
        or build_revision == "unknown"
    ):
        raise ValueError("service status omitted a usable build_revision")
    if (
        expected_build_revision is not None
        and build_revision != expected_build_revision
    ):
        raise ValueError(
            "service build revision mismatch: "
            f"expected {expected_build_revision}, actual {build_revision}"
        )
    actual_features = status.get("runtime_features")
    if not isinstance(actual_features, dict):
        raise ValueError(
            "service status omitted the required runtime_features snapshot"
        )
    missing_current = sorted(CURRENT_RUNTIME_FEATURES - set(actual_features))
    if missing_current:
        raise ValueError(
            "service runtime_features snapshot is incomplete; missing "
            f"{missing_current}"
        )
    mismatches = {
        name: {"expected": expected, "actual": actual_features.get(name)}
        for name, expected in expected_features.items()
        if (
            name not in actual_features
            or type(actual_features[name]) is not type(expected)
            or actual_features[name] != expected
        )
    }
    if mismatches:
        raise ValueError(f"service runtime feature mismatch: {mismatches}")
    embeddings = status.get("embeddings")
    if not isinstance(embeddings, dict):
        raise ValueError("service status omitted embeddings metadata")
    snapshot = {
        "schema": "brunn-service-runtime-snapshot@v1",
        "captured_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "status": status["status"],
        "build_revision": build_revision,
        "corpus_revision": status.get("corpus_revision"),
        "revision_sequence": status.get("revision_sequence"),
        "read_only": status.get("read_only"),
        "runtime_features": actual_features,
        "embeddings": embeddings,
    }
    semantic_runtime = status.get("semantic_runtime")
    if semantic_runtime is not None:
        if not isinstance(semantic_runtime, dict):
            raise ValueError("service semantic_runtime snapshot must be an object")
        snapshot["semantic_runtime"] = semantic_runtime
    return snapshot


def fetch_service_runtime_snapshot(
    client: NativeApiClient,
    *,
    expected_features: dict[str, Any],
    expected_build_revision: str | None,
) -> dict[str, Any]:
    status = client.get("/v1/status").data
    if not isinstance(status, dict):
        raise ValueError("service status response was not an object")
    return capture_service_runtime_snapshot(
        status,
        expected_features=expected_features,
        expected_build_revision=expected_build_revision,
    )


def require_stable_runtime_configuration(
    before: dict[str, Any],
    after: dict[str, Any],
) -> None:
    for field in ("build_revision", "runtime_features", "embeddings"):
        if before.get(field) != after.get(field):
            raise ValueError(f"service {field} drifted during the evaluation run")


def verify_service_feature_states(
    client: NativeApiClient,
    expected: dict[str, bool],
) -> dict[str, bool]:
    """Backward-compatible helper for callers that only need boolean checks."""
    snapshot = fetch_service_runtime_snapshot(
        client,
        expected_features=expected,
        expected_build_revision=None,
    )
    actual = snapshot["runtime_features"]
    return {name: bool(actual[name]) for name in sorted(expected)}


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


def response_query_count(value: Any) -> int | None:
    if not isinstance(value, dict):
        return None
    count = value.get("query_count")
    if isinstance(count, int) and not isinstance(count, bool) and count >= 0:
        return count
    return None


QUERY_COUNT_SAMPLE_OPERATIONS = {
    "open": "open",
    "search": "search",
    "broad_search": "search",
    "bounded_overflow_search": "search",
    "old_source_search": "search",
    "verbatim_identifier_search": "search",
    "concurrent_search": "search",
    "read": "read",
    "max_batch_read": "read",
    "write": "write",
    "checkpoint": "checkpoint",
    "resume_delta_checkpoint": "checkpoint",
    "max_checkpoint_sources": "checkpoint",
    "resume": "resume",
}


def expected_query_count_sample_cardinality(
    *,
    scale: int,
    samples_per_retrieval: int,
    verbatim_identifier_probes: int,
    concurrent_rounds: int,
    concurrent_searches_per_round: int,
    resume_delta_fixture_checkpoint: bool = False,
) -> dict[str, int]:
    integer_fields = {
        "scale": scale,
        "samples_per_retrieval": samples_per_retrieval,
        "verbatim_identifier_probes": verbatim_identifier_probes,
        "concurrent_rounds": concurrent_rounds,
        "concurrent_searches_per_round": concurrent_searches_per_round,
    }
    if any(
        not isinstance(value, int) or isinstance(value, bool) or value < 0
        for value in integer_fields.values()
    ):
        raise ValueError(
            "query-count sample cardinality inputs must be non-negative integers"
        )
    if (
        scale < 1
        or samples_per_retrieval < 1
        or concurrent_rounds < 1
        or concurrent_searches_per_round < 1
    ):
        raise ValueError(
            "query-count scale, retrieval samples, concurrent rounds, and "
            "searches per round must be positive"
        )
    if not isinstance(resume_delta_fixture_checkpoint, bool):
        raise ValueError(
            "resume-delta fixture checkpoint classification must be boolean"
        )
    checkpoint_sample_name = (
        "resume_delta_checkpoint"
        if resume_delta_fixture_checkpoint
        else "checkpoint"
    )
    expected = {
        "open": samples_per_retrieval,
        "search": samples_per_retrieval,
        "broad_search": samples_per_retrieval,
        "bounded_overflow_search": samples_per_retrieval,
        "old_source_search": samples_per_retrieval,
        "verbatim_identifier_search": verbatim_identifier_probes,
        "read": samples_per_retrieval,
        checkpoint_sample_name: 1,
        "resume": samples_per_retrieval,
        "write": concurrent_rounds,
        "concurrent_search": (
            concurrent_rounds * concurrent_searches_per_round
        ),
    }
    if scale >= PRODUCTION_RECORDS:
        expected.update({
            "max_batch_read": 1,
            "max_checkpoint_sources": 1,
        })
    return dict(sorted(expected.items()))


def summarize_query_counts(
    samples: Sequence[tuple[str, dict[str, Any]]],
    *,
    expected_cardinality: Mapping[str, int] | None = None,
) -> dict[str, Any]:
    authoritative = expected_cardinality is not None
    normalized_expected: dict[str, int] = {}
    if expected_cardinality is not None:
        for sample_name, cardinality in expected_cardinality.items():
            if sample_name not in QUERY_COUNT_SAMPLE_OPERATIONS:
                raise ValueError(
                    f"unknown expected query-count sample {sample_name!r}"
                )
            if (
                not isinstance(cardinality, int)
                or isinstance(cardinality, bool)
                or cardinality < 0
            ):
                raise ValueError(
                    f"invalid expected cardinality for {sample_name!r}"
                )
            normalized_expected[sample_name] = cardinality
    by_operation: dict[str, list[int]] = {}
    by_sample_name: dict[str, dict[str, Any]] = {
        sample_name: {
            "operation": QUERY_COUNT_SAMPLE_OPERATIONS[sample_name],
            "counts": [],
        }
        for sample_name in normalized_expected
    }
    observed_by_sample_name: dict[str, int] = {}
    missing: dict[str, int] = {}
    missing_by_sample_name: dict[str, int] = {}
    for sample_name, body in samples:
        operation = QUERY_COUNT_SAMPLE_OPERATIONS.get(sample_name)
        if operation is None:
            raise ValueError(
                f"unknown response sample name {sample_name!r}"
            )
        observed_by_sample_name[sample_name] = (
            observed_by_sample_name.get(sample_name, 0) + 1
        )
        sample_summary = by_sample_name.setdefault(
            sample_name,
            {"operation": operation, "counts": []},
        )
        if sample_summary["operation"] != operation:
            raise ValueError(
                f"query-count sample {sample_name!r} changed operations"
            )
        count = response_query_count(body)
        if count is None:
            missing[operation] = missing.get(operation, 0) + 1
            missing_by_sample_name[sample_name] = (
                missing_by_sample_name.get(sample_name, 0) + 1
            )
            continue
        by_operation.setdefault(operation, []).append(count)
        sample_summary["counts"].append(count)

    def count_summary(values: Sequence[int]) -> dict[str, Any]:
        return {
            "samples": len(values),
            "min": min(values) if values else None,
            "max": max(values) if values else None,
            "counts": list(values),
        }

    if not authoritative:
        normalized_expected = dict(observed_by_sample_name)
    all_sample_names = sorted(
        set(normalized_expected) | set(observed_by_sample_name)
    )
    missing_response_samples = {
        sample_name: normalized_expected[sample_name]
        - observed_by_sample_name.get(sample_name, 0)
        for sample_name in sorted(normalized_expected)
        if (
            normalized_expected[sample_name]
            > observed_by_sample_name.get(sample_name, 0)
        )
    }
    extra_response_samples = {
        sample_name: observed_by_sample_name[sample_name]
        - normalized_expected.get(sample_name, 0)
        for sample_name in sorted(observed_by_sample_name)
        if (
            observed_by_sample_name[sample_name]
            > normalized_expected.get(sample_name, 0)
        )
    }
    counted_by_sample_name = {
        sample_name: len(by_sample_name[sample_name]["counts"])
        for sample_name in all_sample_names
    }
    cardinality_pass = bool(
        not missing_response_samples
        and not extra_response_samples
        and not missing_by_sample_name
        and all(
            counted_by_sample_name[sample_name]
            == normalized_expected[sample_name]
            for sample_name in normalized_expected
        )
    )
    return {
        "definition": (
            "completed sqlx::query events within the request scope; includes "
            "authentication, transaction context/setup, application SQL, and "
            "COMMIT; excludes SQLx's unlogged protocol-level BEGIN"
        ),
        "by_operation": {
            operation: count_summary(values)
            for operation, values in sorted(by_operation.items())
        },
        "by_sample_name": {
            sample_name: {
                "operation": summary["operation"],
                "expected_samples": normalized_expected.get(sample_name, 0),
                "observed_samples": observed_by_sample_name.get(sample_name, 0),
                "missing_query_counts": missing_by_sample_name.get(
                    sample_name,
                    0,
                ),
                **count_summary(summary["counts"]),
            }
            for sample_name, summary in sorted(by_sample_name.items())
        },
        "missing_by_operation": dict(sorted(missing.items())),
        "missing_by_sample_name": dict(sorted(missing_by_sample_name.items())),
        "sample_cardinality": {
            "schema": "brunn-query-count-sample-cardinality@v1",
            "authoritative": authoritative,
            "expected_by_sample_name": dict(sorted(normalized_expected.items())),
            "observed_by_sample_name": dict(
                sorted(observed_by_sample_name.items())
            ),
            "counted_by_sample_name": counted_by_sample_name,
            "missing_response_samples": missing_response_samples,
            "extra_response_samples": extra_response_samples,
            "missing_query_count_samples": dict(
                sorted(missing_by_sample_name.items())
            ),
            "pass": cardinality_pass,
        },
    }


def load_query_budgets(
    path: Path = QUERY_BUDGETS_PATH,
    *,
    expected_profile: str | None = None,
) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schema") != "brunn-query-budgets@v1":
        raise ValueError(f"unsupported query-budget schema in {path}")
    profile = payload.get("profile")
    if not isinstance(profile, str) or not profile:
        raise ValueError(f"query-budget profile is missing in {path}")
    if expected_profile is not None and profile != expected_profile:
        raise ValueError(
            f"query-budget profile mismatch in {path}: "
            f"expected {expected_profile}, actual {profile}"
        )
    runtime_features = payload.get("runtime_features")
    if not isinstance(runtime_features, dict) or not runtime_features:
        raise ValueError(
            f"query-budget runtime_features applicability is missing in {path}"
        )
    unknown_features = sorted(set(runtime_features) - RUNTIME_FEATURES)
    if unknown_features:
        raise ValueError(
            f"query-budget contract has unknown runtime features: {unknown_features}"
        )
    operations = payload.get("operations")
    if not isinstance(operations, dict) or not operations:
        raise ValueError(f"query-budget operations are missing in {path}")
    for operation, budget in operations.items():
        if not isinstance(budget, dict):
            raise ValueError(f"invalid query budget for {operation}")
        comparison = budget.get("comparison")
        if comparison == "exact":
            value = budget.get("count")
        elif comparison == "at_most":
            value = budget.get("max")
        else:
            raise ValueError(
                f"query budget {operation} has invalid comparison {comparison!r}"
            )
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise ValueError(f"query budget {operation} is not a non-negative integer")
    return payload


def resolve_query_budget_contract(
    *,
    profile: str,
    path: Path | None,
    runtime_snapshot: dict[str, Any],
    gate_profile: str | None,
    protocol: str,
    retrieval_modes: Sequence[str] = ("exact", "lexical", "semantic"),
) -> dict[str, Any] | None:
    if gate_profile == LEXICAL_CONSOLIDATION_GATE_PROFILE:
        if profile != "not-applicable" or path is not None:
            raise ValueError(
                "the E05 lexical-consolidation gate does not evaluate query "
                "budgets; use --query-budget-profile not-applicable without "
                "--query-budget-contract"
            )
        return None
    if protocol != "simple":
        if profile != "not-applicable" or path is not None:
            raise ValueError(
                "legacy protocol runs do not expose the simple-core query "
                "budget contract; use --query-budget-profile not-applicable"
            )
        return None
    if profile == "calibration":
        if path is not None:
            raise ValueError(
                "query-budget calibration records observed counts and does not "
                "accept --query-budget-contract"
            )
        return {
            "profile": "calibration",
            "path": None,
            "sha256": None,
            "contract": {
                "schema": "brunn-query-budgets@v1",
                "profile": "calibration",
                "runtime_features": runtime_snapshot["runtime_features"],
                "operations": {},
            },
            "acceptance_eligible": False,
            "reason": (
                "count-capture only; author and review a profile-specific "
                "contract before an acceptance run"
            ),
        }
    if profile == "not-applicable":
        raise ValueError(
            "--query-budget-profile not-applicable is allowed only for a "
            "profile that does not evaluate simple-core query counts"
        )
    if path is None:
        if profile == DEFAULT_QUERY_BUDGET_PROFILE:
            path = QUERY_BUDGETS_PATH
        elif (
            profile == D03_RESUME_DELTAS_GATE_PROFILE
            and gate_profile == D03_RESUME_DELTAS_GATE_PROFILE
        ):
            path = D03_QUERY_BUDGETS_PATH
        else:
            raise ValueError(
                f"query-budget profile {profile!r} requires an explicit "
                "--query-budget-contract; launch profiles never inherit the "
                "default-safe contract"
            )
    payload = load_query_budgets(path, expected_profile=profile)
    contract_modes = payload.get("retrieval_modes")
    if contract_modes is not None:
        if (
            not isinstance(contract_modes, list)
            or not contract_modes
            or not all(
                isinstance(mode, str)
                and mode in {"exact", "lexical", "semantic"}
                for mode in contract_modes
            )
            or len(contract_modes) != len(set(contract_modes))
        ):
            raise ValueError(
                f"query-budget profile {profile!r} has invalid retrieval_modes"
            )
        if list(retrieval_modes) != contract_modes:
            raise ValueError(
                f"query-budget profile {profile!r} is bound to retrieval modes "
                f"{contract_modes}, not {list(retrieval_modes)}"
            )
    experiment_variables = payload.get("experiment_variable_features", [])
    if (
        not isinstance(experiment_variables, list)
        or not all(
            isinstance(name, str) and name
            for name in experiment_variables
        )
        or len(experiment_variables) != len(set(experiment_variables))
    ):
        raise ValueError(
            f"query-budget profile {profile!r} has invalid "
            "experiment_variable_features"
        )
    unknown_variables = sorted(
        set(experiment_variables) - BOOLEAN_RUNTIME_FEATURES
    )
    if unknown_variables:
        raise ValueError(
            f"query-budget profile {profile!r} has unknown experiment "
            f"variable features: {unknown_variables}"
        )
    overlap = sorted(
        set(experiment_variables) & set(payload.get("runtime_features", {}))
    )
    if overlap:
        raise ValueError(
            f"query-budget profile {profile!r} cannot both bind and vary "
            f"runtime features: {overlap}"
        )
    actual_features = runtime_snapshot.get("runtime_features")
    if not isinstance(actual_features, dict):
        raise ValueError("runtime snapshot omitted runtime_features")
    expected_features = payload["runtime_features"]
    mismatches = {
        name: {"expected": expected, "actual": actual_features.get(name)}
        for name, expected in expected_features.items()
        if (
            name not in actual_features
            or type(actual_features[name]) is not type(expected)
            or actual_features[name] != expected
        )
    }
    if mismatches:
        raise ValueError(
            f"query-budget profile {profile!r} is not applicable to the "
            f"authenticated runtime: {mismatches}"
        )
    resolved = path.resolve()
    try:
        rendered_path = str(resolved.relative_to(PROJECT_ROOT))
    except ValueError:
        rendered_path = str(resolved)
    return {
        "profile": profile,
        "path": rendered_path,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "contract": payload,
    }


def evaluate_query_budgets(
    summary: dict[str, Any],
    budgets: dict[str, Any],
) -> list[dict[str, Any]]:
    observed = summary.get("by_operation", {})
    missing = summary.get("missing_by_operation", {})
    gates = []
    for operation, budget in budgets["operations"].items():
        values = observed.get(operation, {}).get("counts", [])
        comparison = budget["comparison"]
        threshold = (
            budget["count"] if comparison == "exact" else budget["max"]
        )
        within = bool(values) and all(
            value == threshold if comparison == "exact" else value <= threshold
            for value in values
        )
        gates.append({
            "name": f"query_budget_{operation}",
            "pass": within and int(missing.get(operation, 0)) == 0,
            "observed": {
                "sample_name": operation,
                "counts": values,
                "missing": int(missing.get(operation, 0)),
            },
            "threshold": {
                "comparison": comparison,
                "count": threshold,
            },
        })
    return gates


def valid_resume_delta_fixture(
    fixture: Any,
) -> bool:
    if not isinstance(fixture, dict):
        return False
    checkpoint_version = fixture.get("checkpoint_source_version")
    mutation_version = fixture.get("mutation_version")
    checkpoint_hash = fixture.get("checkpoint_source_content_hash")
    mutation_hash = fixture.get("mutation_content_hash")
    return (
        fixture.get("requested") is True
        and fixture.get("applicable") is True
        and fixture.get("status") == "complete"
        and fixture.get("pass") is True
        and isinstance(fixture.get("source_path"), str)
        and bool(fixture["source_path"])
        and fixture.get("checkpoint_source_entries") == 1
        and isinstance(checkpoint_version, int)
        and not isinstance(checkpoint_version, bool)
        and isinstance(mutation_version, int)
        and not isinstance(mutation_version, bool)
        and mutation_version == checkpoint_version + 1
        and isinstance(checkpoint_hash, str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", checkpoint_hash) is not None
        and isinstance(mutation_hash, str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", mutation_hash) is not None
        and mutation_hash != checkpoint_hash
        and fixture.get("mutation_no_op") is False
        and fixture.get("verified_read_version") == mutation_version
        and fixture.get("verified_read_content_hash") == mutation_hash
        and fixture.get("verified_read_exact_content") is True
        and fixture.get("verified_read_response_truncated") is False
        and isinstance(fixture.get("mutation_marker"), str)
        and bool(fixture["mutation_marker"])
        and fixture.get("expected_treatment_statement_delta")
        == D03_RESUME_QUERY_COUNT_DELTA
        and fixture.get("statement_delta_accounting") == [
            "transaction_context_validation",
            "transaction_context_setup",
            "statement_timeout_setup",
            "batched_version_pair_select",
            "transaction_commit",
        ]
    )


def resume_delta_lineage_receipt(
    response: Any,
    fixture: Mapping[str, Any],
) -> dict[str, Any]:
    deltas = recursive_find(response, "resume_deltas")
    if deltas is None:
        return {
            "status": "not_present",
            "pass": None,
            "delta_count": None,
        }
    if not valid_resume_delta_fixture(fixture):
        return {
            "status": "invalid_fixture",
            "pass": False,
            "delta_count": len(deltas) if isinstance(deltas, list) else None,
        }
    row = (
        deltas[0]
        if (
            isinstance(deltas, list)
            and len(deltas) == 1
            and isinstance(deltas[0], dict)
        )
        else {}
    )
    before = row.get("before")
    after = row.get("after")
    mutation_marker = fixture.get("mutation_marker")
    before_sha256 = (
        "sha256:" + hashlib.sha256(before.encode("utf-8")).hexdigest()
        if isinstance(before, str)
        else None
    )
    after_sha256 = (
        "sha256:" + hashlib.sha256(after.encode("utf-8")).hexdigest()
        if isinstance(after, str)
        else None
    )
    valid = (
        len(deltas) == 1
        if isinstance(deltas, list)
        else False
    ) and (
        row.get("path") == fixture.get("source_path")
        and row.get("pinned_version")
        == fixture.get("checkpoint_source_version")
        and row.get("pinned_sha256")
        == fixture.get("checkpoint_source_content_hash")
        and row.get("current_version") == fixture.get("mutation_version")
        and row.get("current_sha256")
        == fixture.get("mutation_content_hash")
        and row.get("mode") == "whole_pair"
        and isinstance(before, str)
        and isinstance(after, str)
        and before_sha256
        == fixture.get("checkpoint_source_content_hash")
        and after_sha256 == fixture.get("mutation_content_hash")
        and isinstance(mutation_marker, str)
        and mutation_marker not in before
        and mutation_marker in after
    )
    return {
        "status": "complete" if valid else "invalid",
        "pass": bool(valid),
        "delta_count": len(deltas) if isinstance(deltas, list) else None,
        "path": row.get("path"),
        "pinned_version": row.get("pinned_version"),
        "pinned_sha256": row.get("pinned_sha256"),
        "current_version": row.get("current_version"),
        "current_sha256": row.get("current_sha256"),
        "mode": row.get("mode"),
        "before_sha256": before_sha256,
        "after_sha256": after_sha256,
        "mutation_marker_found": (
            isinstance(after, str)
            and isinstance(mutation_marker, str)
            and mutation_marker in after
        ),
    }


def valid_resume_delta_lineage_sample(
    sample: Any,
    fixture: Mapping[str, Any],
) -> bool:
    return (
        isinstance(sample, dict)
        and sample.get("status") == "complete"
        and sample.get("pass") is True
        and sample.get("delta_count") == 1
        and sample.get("path") == fixture.get("source_path")
        and sample.get("pinned_version")
        == fixture.get("checkpoint_source_version")
        and sample.get("pinned_sha256")
        == fixture.get("checkpoint_source_content_hash")
        and sample.get("current_version") == fixture.get("mutation_version")
        and sample.get("current_sha256")
        == fixture.get("mutation_content_hash")
        and sample.get("mode") == "whole_pair"
        and sample.get("before_sha256")
        == fixture.get("checkpoint_source_content_hash")
        and sample.get("after_sha256")
        == fixture.get("mutation_content_hash")
        and sample.get("mutation_marker_found") is True
    )


def load_d03_resume_control(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schema") != "brunn-performance-eval@v2":
        raise ValueError("D03 resume control is not a performance-eval v2 artifact")
    if payload.get("pass") is not True:
        raise ValueError("D03 resume control must be a passing definitive artifact")
    if payload.get("protocol") != "simple":
        raise ValueError("D03 resume control must use the simple protocol")
    run_profile = payload.get("run_profile")
    if (
        not isinstance(run_profile, dict)
        or run_profile.get("definitive") is not True
        or run_profile.get("exercise_resume_delta_fixture") is not True
    ):
        raise ValueError(
            "D03 resume control must be definitive and exercise the "
            "resume-delta fixture"
        )
    if payload.get("retrieval_modes") != ["exact", "lexical"]:
        raise ValueError(
            "D03 resume control must use retrieval modes exact lexical"
        )
    runtime_configuration = payload.get("runtime_configuration")
    if not isinstance(runtime_configuration, dict):
        raise ValueError("D03 resume control omitted runtime_configuration")
    snapshot = runtime_configuration.get("after")
    if not isinstance(snapshot, dict):
        raise ValueError("D03 resume control omitted its final runtime snapshot")
    runtime_features = snapshot.get("runtime_features")
    if (
        not isinstance(runtime_features, dict)
        or runtime_features.get("resume_deltas") is not False
    ):
        raise ValueError(
            "D03 resume control must prove resume_deltas=false at runtime"
        )
    scale = next(
        (
            item
            for item in payload.get("scales", [])
            if isinstance(item, dict) and item.get("scale") == FUTURE_RECORDS
        ),
        None,
    )
    if scale is None:
        raise ValueError(f"D03 resume control omitted the {FUTURE_RECORDS:,} scale")
    fixture = scale.get("resume_delta_fixture")
    if not valid_resume_delta_fixture(fixture):
        raise ValueError(
            "D03 resume control omitted a valid changed checkpoint-source "
            "fixture receipt"
        )
    control_lineage = scale.get("resume_delta_lineage_samples")
    if (
        not isinstance(control_lineage, list)
        or len(control_lineage) < DEFINITIVE_SAMPLES
        or any(
            not isinstance(sample, dict)
            or sample.get("status") != "not_present"
            or sample.get("pass") is not None
            or sample.get("delta_count") is not None
            for sample in control_lineage
        )
    ):
        raise ValueError(
            "D03 resume control must behaviorally prove resume deltas were "
            "absent from every sample"
        )
    counts = (
        scale.get("query_counts", {})
        .get("by_operation", {})
        .get("resume", {})
        .get("counts")
    )
    if (
        not isinstance(counts, list)
        or len(counts) < DEFINITIVE_SAMPLES
        or any(
            not isinstance(value, int) or isinstance(value, bool) or value < 0
            for value in counts
        )
    ):
        raise ValueError(
            "D03 resume control needs at least 30 non-negative resume query counts"
        )
    fingerprint = payload.get("implementation_fingerprint")
    if not isinstance(fingerprint, dict) or not fingerprint.get("reproducible"):
        raise ValueError("D03 resume control omitted a reproducible fingerprint")
    return {
        "path": str(path.resolve()),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "label": payload.get("label"),
        "build_revision": snapshot.get("build_revision"),
        "source_revision": fingerprint.get("source_revision"),
        "runtime_features": runtime_features,
        "retrieval_modes": payload["retrieval_modes"],
        "scale": FUTURE_RECORDS,
        "resume_query_counts": list(counts),
        "resume_p95_ms": scale.get("resume_ms"),
        "resume_delta_fixture": fixture,
        "resume_delta_lineage_samples": control_lineage,
    }


def validate_d03_resume_control_compatibility(
    control: dict[str, Any],
    *,
    runtime_snapshot: dict[str, Any],
    implementation: dict[str, Any],
    retrieval_modes: Sequence[str],
) -> None:
    if control.get("source_revision") != implementation.get("source_revision"):
        raise ValueError(
            "D03 control and treatment must use the same clean source revision"
        )
    if control.get("build_revision") != runtime_snapshot.get("build_revision"):
        raise ValueError(
            "D03 control and treatment must use the same API build revision"
        )
    if list(retrieval_modes) != control.get("retrieval_modes"):
        raise ValueError(
            "D03 control and treatment must use the same retrieval modes"
        )
    control_features = control.get("runtime_features")
    treatment_features = runtime_snapshot.get("runtime_features")
    if (
        not isinstance(control_features, dict)
        or not isinstance(treatment_features, dict)
    ):
        raise ValueError(
            "D03 control and treatment must include full runtime snapshots"
        )
    mismatches = {
        name: {
            "control": control_features.get(name),
            "treatment": treatment_features.get(name),
        }
        for name in sorted(set(control_features) | set(treatment_features))
        if (
            name != "resume_deltas"
            and (
                name not in control_features
                or name not in treatment_features
                or type(control_features[name])
                is not type(treatment_features[name])
                or control_features[name] != treatment_features[name]
            )
        )
    }
    if mismatches:
        raise ValueError(
            "D03 control and treatment runtime features must be identical "
            f"except for resume_deltas: {mismatches}"
        )


def evaluate_d03_resume_delta_gates(
    scales: Sequence[dict[str, Any]],
    control: dict[str, Any],
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    treatment = next(
        (item for item in scales if item.get("scale") == FUTURE_RECORDS),
        None,
    )
    treatment_counts = (
        treatment.get("query_counts", {})
        .get("by_operation", {})
        .get("resume", {})
        .get("counts")
        if isinstance(treatment, dict)
        else None
    )
    control_counts = control["resume_query_counts"]
    paired = (
        list(zip(control_counts, treatment_counts))
        if isinstance(treatment_counts, list)
        else []
    )
    deltas = [after - before for before, after in paired]
    pairing_complete = (
        isinstance(treatment_counts, list)
        and len(control_counts) == len(treatment_counts)
        and len(treatment_counts) >= DEFINITIVE_SAMPLES
    )
    resume_p95 = treatment.get("resume_ms") if isinstance(treatment, dict) else None
    treatment_fixture = (
        treatment.get("resume_delta_fixture")
        if isinstance(treatment, dict)
        else None
    )
    lineage_samples = (
        treatment.get("resume_delta_lineage_samples")
        if isinstance(treatment, dict)
        else None
    )
    fixture_identity_fields = (
        "source_path",
        "checkpoint_source_version",
        "checkpoint_source_content_hash",
        "mutation_version",
        "mutation_content_hash",
        "mutation_marker",
    )
    control_fixture = control.get("resume_delta_fixture")
    fixture_identity_matches = (
        valid_resume_delta_fixture(control_fixture)
        and valid_resume_delta_fixture(treatment_fixture)
        and all(
            control_fixture.get(field) == treatment_fixture.get(field)
            for field in fixture_identity_fields
        )
    )
    lineage_complete = (
        isinstance(lineage_samples, list)
        and len(lineage_samples) == len(treatment_counts or [])
        and len(lineage_samples) >= DEFINITIVE_SAMPLES
        and all(
            valid_resume_delta_lineage_sample(
                sample,
                treatment_fixture,
            )
            for sample in lineage_samples
        )
    )
    record = {
        "control": control,
        "treatment": {
            "scale": FUTURE_RECORDS,
            "resume_query_counts": treatment_counts,
            "resume_p95_ms": resume_p95,
            "resume_delta_fixture": (
                treatment_fixture
            ),
            "resume_delta_lineage_samples": lineage_samples,
        },
        "paired_query_count_deltas": deltas,
        "paired_samples": len(paired),
    }
    gates = [
        {
            "name": "d03_resume_p95_at_640000_is_at_most_150ms",
            "pass": (
                isinstance(resume_p95, (int, float))
                and not isinstance(resume_p95, bool)
                and float(resume_p95) <= 150.0
            ),
            "observed": resume_p95,
            "threshold": {"comparison": "at_most", "milliseconds": 150.0},
        },
        {
            "name": "d03_resume_query_count_delta_is_exactly_five",
            "pass": pairing_complete and all(
                delta == D03_RESUME_QUERY_COUNT_DELTA
                for delta in deltas
            ),
            "observed": {
                "control_counts": control_counts,
                "treatment_counts": treatment_counts,
                "paired_deltas": deltas,
                "paired_samples": len(paired),
            },
            "threshold": {
                "comparison": "paired_exact_delta",
                "delta": D03_RESUME_QUERY_COUNT_DELTA,
                "minimum_samples": DEFINITIVE_SAMPLES,
                "accounting": [
                    "transaction_context_validation",
                    "transaction_context_setup",
                    "statement_timeout_setup",
                    "batched_version_pair_select",
                    "transaction_commit",
                ],
            },
        },
        {
            "name": "d03_resume_delta_lineage_matches_control_fixture",
            "pass": fixture_identity_matches and lineage_complete,
            "observed": {
                "fixture_identity_matches": fixture_identity_matches,
                "lineage_samples": lineage_samples,
                "lineage_sample_count": (
                    len(lineage_samples)
                    if isinstance(lineage_samples, list)
                    else None
                ),
            },
            "threshold": {
                "fixture_identity_fields": list(fixture_identity_fields),
                "minimum_lineage_samples": DEFINITIVE_SAMPLES,
                "every_sample_exactly_one_whole_pair": True,
            },
        },
    ]
    return gates, record


def normalize_sql(value: str) -> str:
    return " ".join(value.split())


def sql_fingerprint(value: str) -> str:
    return hashlib.sha256(normalize_sql(value).encode("utf-8")).hexdigest()


def extract_sql_function_body(source: str, function_name: str) -> str:
    pattern = re.compile(
        rf"CREATE(?:\s+OR\s+REPLACE)?\s+FUNCTION\s+"
        rf"{re.escape(function_name)}\s*\([^)]*\).*?"
        r"\bAS\s+\$\$(.*?)\$\$\s*;",
        re.IGNORECASE | re.DOTALL,
    )
    match = pattern.search(source)
    if match is None:
        raise ValueError(f"could not extract SQL body for {function_name}")
    return match.group(1).strip()


def iter_plan_nodes(plan: Any) -> Iterator[dict[str, Any]]:
    if isinstance(plan, list):
        for item in plan:
            yield from iter_plan_nodes(item)
    elif isinstance(plan, dict):
        if isinstance(plan.get("Node Type"), str):
            yield plan
        for value in plan.values():
            if isinstance(value, (list, dict)):
                yield from iter_plan_nodes(value)


def evaluate_retrieval_plan(
    plan: Any,
    *,
    lane: str,
    expected_index: str,
) -> dict[str, Any]:
    nodes = list(iter_plan_nodes(plan))
    forbidden = [
        {
            "node_type": node.get("Node Type"),
            "relation": node.get("Relation Name"),
        }
        for node in nodes
        if node.get("Node Type") == "Seq Scan"
        and node.get("Relation Name") == "search_chunks"
    ]
    expected_node_type = (
        "Bitmap Index Scan" if lane == "lexical" else "Index Scan"
    )
    matched = [
        {
            "node_type": node.get("Node Type"),
            "index_name": node.get("Index Name"),
        }
        for node in nodes
        if node.get("Node Type") == expected_node_type
        and node.get("Index Name") == expected_index
    ]
    return {
        "pass": bool(matched) and not forbidden,
        "lane": lane,
        "expected": {
            "node_type": expected_node_type,
            "index_name": expected_index,
            "no_seq_scan_on": "search_chunks",
        },
        "matched": matched,
        "forbidden": forbidden,
        "node_types": sorted({
            str(node.get("Node Type"))
            for node in nodes
            if node.get("Node Type")
        }),
    }


def is_expected_mode1_empty_semantic_plan(plan: Any) -> bool:
    """Accept only the two safe plans PostgreSQL uses with zero ready vectors."""
    nodes = list(iter_plan_nodes(plan))
    relation_nodes = [
        node
        for node in nodes
        if node.get("Relation Name") == "search_chunks"
    ]
    index_nodes = [
        node
        for node in nodes
        if str(node.get("Index Name", "")).startswith("search_chunks_")
    ]
    if len(relation_nodes) != 1 or len(index_nodes) != 1:
        return False

    relation_node = relation_nodes[0]
    index_node = index_nodes[0]
    if index_node.get("Index Name") != "search_chunks_semantic_coverage_idx":
        return False
    if relation_node.get("Node Type") == "Index Scan":
        return (
            index_node is relation_node
            and index_node.get("Node Type") == "Index Scan"
        )
    if relation_node.get("Node Type") != "Bitmap Heap Scan":
        return False
    if "embedding IS NOT NULL" not in str(
        relation_node.get("Recheck Cond", "")
    ):
        return False
    descendants = list(iter_plan_nodes(relation_node.get("Plans", [])))
    return (
        index_node.get("Node Type") == "Bitmap Index Scan"
        and any(node is index_node for node in descendants)
    )


def apply_retrieval_plan_applicability(
    assertions: dict[str, Any],
    *,
    retrieval_modes: Sequence[str],
    semantic_lane_enabled: bool,
) -> dict[str, Any]:
    modes = list(retrieval_modes)
    if (
        not modes
        or len(modes) != len(set(modes))
        or any(
            mode not in {"exact", "lexical", "semantic"}
            for mode in modes
        )
    ):
        raise ValueError(f"invalid retrieval modes for plan assertions: {modes}")
    requested = set(modes)
    result = dict(assertions)
    lanes = dict(assertions.get("lanes", {}))
    required_lanes = []
    for lane in ("lexical", "semantic"):
        if lane not in requested:
            lanes[lane] = {
                "status": "not_applicable",
                "pass": True,
                "reason": f"{lane} retrieval was not requested",
            }
            continue
        required_lanes.append(lane)
        if lane == "semantic" and not semantic_lane_enabled:
            lanes[lane] = {
                "status": "runtime_mismatch",
                "pass": False,
                "reason": (
                    "semantic retrieval was requested while the authenticated "
                    "runtime reported semantic_lane=false"
                ),
            }
        elif isinstance(lanes.get(lane), dict):
            lanes[lane] = dict(lanes[lane])
            lanes[lane].setdefault("status", "complete")
    result["lanes"] = lanes
    result["required_lanes"] = required_lanes
    result["retrieval_modes"] = modes
    result["semantic_lane_enabled"] = semantic_lane_enabled
    drift_checks = []
    for record in result.get("sql_drift", []):
        annotated = dict(record)
        annotated["applicable"] = record.get("lane") in required_lanes
        drift_checks.append(annotated)
    result["sql_drift"] = drift_checks
    applicable_drift_lanes = {
        record.get("lane")
        for record in drift_checks
        if record.get("applicable") is True
    }
    result["pass"] = (
        result.get("status") == "complete"
        and all(
            record.get("pass") is True
            for record in drift_checks
            if record.get("applicable") is True
        )
        and set(required_lanes).issubset(applicable_drift_lanes)
        and all(
            isinstance(lanes.get(lane), dict)
            and lanes[lane].get("pass") is True
            for lane in required_lanes
        )
    )
    return result


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


def candidate_audit(
    value: Any,
    *,
    marker: str,
    target_path: str,
    limit: int = 8,
) -> dict[str, Any]:
    """Return bounded, source-bound evidence about retrieval candidates."""
    candidates: list[dict[str, Any]] = []

    def visit(item: Any) -> None:
        if isinstance(item, dict):
            nested = item.get("candidates")
            if isinstance(nested, list):
                candidates.extend(
                    candidate
                    for candidate in nested
                    if isinstance(candidate, dict)
                )
            for key, child in item.items():
                if key != "candidates":
                    visit(child)
        elif isinstance(item, list):
            for child in item:
                visit(child)

    visit(value)
    target_key = target_path.casefold()
    summaries = []
    target_found = False
    for candidate in candidates[:limit]:
        path = candidate.get("path")
        path_matches = (
            isinstance(path, str)
            and path.casefold() == target_key
        )
        marker_in_source_text = source_text_contains(candidate, marker)
        target_found |= path_matches and marker_in_source_text
        summaries.append({
            "path": path if isinstance(path, str) else None,
            "heading": (
                candidate.get("heading")
                if isinstance(candidate.get("heading"), str)
                else None
            ),
            "content_sha256": (
                candidate.get("content_sha256")
                if isinstance(candidate.get("content_sha256"), str)
                else None
            ),
            "version": (
                candidate.get("version")
                if isinstance(candidate.get("version"), int)
                and not isinstance(candidate.get("version"), bool)
                else None
            ),
            "lanes": (
                candidate.get("lanes")
                if isinstance(candidate.get("lanes"), list)
                else None
            ),
            "score": (
                candidate.get("score")
                if isinstance(candidate.get("score"), (int, float))
                and not isinstance(candidate.get("score"), bool)
                else None
            ),
            "target_path_match": path_matches,
            "marker_in_source_text": marker_in_source_text,
        })
    return {
        "candidate_count": len(candidates),
        "target_found": target_found,
        "candidates": summaries,
        "truncated": len(candidates) > limit,
    }


def response_reports_lane_failure(value: Any, lane: str) -> bool:
    if isinstance(value, dict):
        for key in ("lane_failures", "failed_lanes"):
            failures = value.get(key)
            if isinstance(failures, list) and any(
                (
                    str(item).casefold() == lane.casefold()
                    or str(item).casefold().startswith(
                        f"{lane.casefold()}_"
                    )
                )
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
    api_container: str | None = None,
    worker_container: str | None = None,
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
    container_id = None
    container_started_at = None
    if api_container:
        container_id = output([
            "docker",
            "inspect",
            "--format={{.Id}}",
            api_container,
        ])
        container_started_at = output([
            "docker",
            "inspect",
            "--format={{.State.StartedAt}}",
            api_container,
        ])
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
    worker_container_id = None
    worker_container_started_at = None
    worker_running = None
    worker_image_id = None
    worker_image_revision = None
    if worker_container:
        worker_container_id = output([
            "docker", "inspect", "--format={{.Id}}", worker_container,
        ])
        worker_container_started_at = output([
            "docker",
            "inspect",
            "--format={{.State.StartedAt}}",
            worker_container,
        ])
        worker_running = output([
            "docker", "inspect", "--format={{.State.Running}}", worker_container,
        ])
        worker_image_id = output([
            "docker", "inspect", "--format={{.Image}}", worker_container,
        ])
        worker_image_revision = output([
            "docker",
            "inspect",
            (
                "--format={{index .Config.Labels "
                '"org.opencontainers.image.revision"}}'
            ),
            worker_container,
        ])
    return {
        "source_revision": revision,
        "tracked_source_clean": tracked_diff.returncode == 0,
        "untracked_source_files": untracked_source,
        "api_container": api_container,
        "api_container_id": container_id,
        "api_container_started_at": container_started_at,
        "api_image_id": image_id,
        "api_image_revision": image_revision,
        "worker_container": worker_container,
        "worker_container_id": worker_container_id,
        "worker_container_started_at": worker_container_started_at,
        "worker_running": worker_running == "true" if worker_container else None,
        "worker_image_id": worker_image_id,
        "worker_image_revision": worker_image_revision,
        "reproducible": bool(
            revision
            and tracked_diff.returncode == 0
            and not untracked_source
            and container_id
            and container_started_at
            and image_id
            and image_revision == revision
            and (
                not worker_container
                or (
                    worker_container_id
                    and worker_container_started_at
                    and worker_running == "true"
                    and worker_image_id
                    and worker_image_revision == revision
                )
            )
        ),
    }


def e03_mode1_environment_snapshot(api_container: str) -> dict[str, Any]:
    inspected = subprocess.run(
        ["docker", "inspect", api_container],
        text=True,
        capture_output=True,
        check=False,
    )
    if inspected.returncode != 0:
        raise RuntimeError(f"could not inspect E03 Mode 1 API {api_container}")
    try:
        record = json.loads(inspected.stdout)[0]
        config = record["Config"]
        state = record["State"]
    except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise RuntimeError("invalid E03 Mode 1 API inspection") from error
    labels = config.get("Labels")
    labels = labels if isinstance(labels, dict) else {}
    environment = {}
    for item in config.get("Env") or []:
        if isinstance(item, str) and "=" in item:
            key, value = item.split("=", 1)
            environment[key] = value
    compose_project = str(labels.get("com.docker.compose.project") or "")
    worker_query = subprocess.run(
        [
            "docker",
            "ps",
            "-q",
            "--filter",
            f"label=com.docker.compose.project={compose_project}",
            "--filter",
            "label=com.docker.compose.service=worker",
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    if worker_query.returncode != 0:
        raise RuntimeError("could not inspect E03 Mode 1 worker state")
    workers = [line for line in worker_query.stdout.splitlines() if line.strip()]
    result = {
        "api_container_id": record.get("Id"),
        "api_started_at": state.get("StartedAt"),
        "api_running": state.get("Running") is True,
        "compose_project": compose_project or None,
        "compose_service": labels.get("com.docker.compose.service"),
        "api_image_id": record.get("Image"),
        "api_image_revision": labels.get("org.opencontainers.image.revision"),
        "running_worker_count": len(workers),
        "embedding_provider": environment.get(
            "BRUNN_EMBEDDING_PROVIDER",
            "openai",
        ),
        "provider_credential": {
            "openai_api_key_configured": bool(environment.get("OPENAI_API_KEY")),
            "openai_api_key_file_configured": bool(
                environment.get("OPENAI_API_KEY_FILE")
            ),
            "values_recorded": False,
        },
    }
    result["provider_call_path_absent"] = bool(
        not workers
        and not result["provider_credential"]["openai_api_key_configured"]
        and not result["provider_credential"]["openai_api_key_file_configured"]
    )
    result["pass"] = bool(
        result["api_running"]
        and result["compose_project"]
        and result["compose_service"] == "api"
        and result["embedding_provider"] == "hashing"
        and result["running_worker_count"] == 0
        and result["provider_call_path_absent"]
    )
    return result


def e03_container_topology(
    *,
    arm: str,
    api_container: str,
    db_container: str,
    worker_container: str | None,
) -> dict[str, Any]:
    def identity(name: str) -> dict[str, Any]:
        completed = subprocess.run(
            ["docker", "inspect", name],
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode != 0:
            raise ValueError(f"could not inspect E03 container {name}")
        try:
            record = json.loads(completed.stdout)[0]
            labels = record["Config"].get("Labels") or {}
            state = record["State"]
        except (IndexError, KeyError, TypeError, json.JSONDecodeError) as error:
            raise ValueError(f"invalid E03 container inspection for {name}") from error
        return {
            "name": name,
            "container_id": record.get("Id"),
            "image_id": record.get("Image"),
            "started_at": state.get("StartedAt"),
            "running": state.get("Running") is True,
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
            "image_revision": labels.get(
                "org.opencontainers.image.revision"
            ),
        }

    api = identity(api_container)
    db = identity(db_container)
    worker = identity(worker_container) if worker_container else None
    projects = {
        item.get("compose_project")
        for item in (api, db, worker)
        if isinstance(item, dict)
    }
    checks = {
        "one_nonempty_compose_project": (
            len(projects) == 1 and None not in projects and "" not in projects
        ),
        "api_service_running": (
            api["compose_service"] == "api" and api["running"] is True
        ),
        "db_service_running": (
            db["compose_service"] == "db" and db["running"] is True
        ),
        "worker_posture": (
            worker is None
            if arm == "mode1"
            else bool(
                worker
                and worker["compose_service"] == "worker"
                and worker["running"] is True
                and worker["image_id"] == api["image_id"]
            )
        ),
    }
    result = {
        "schema": "brunn-e03-container-topology@v1",
        "arm": arm,
        "api": api,
        "db": db,
        "worker": worker,
        "checks": checks,
        "pass": all(checks.values()),
    }
    if not result["pass"]:
        raise ValueError(f"E03 container topology mismatch: {result}")
    return result


def e03_api_route_binding(
    *,
    api_base_url: str,
    api_container: str,
    db_container: str,
) -> dict[str, Any]:
    """Bind the ambient evaluation URL and database route to one stack."""

    def inspect(name: str) -> dict[str, Any]:
        completed = subprocess.run(
            ["docker", "inspect", name],
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode != 0:
            raise ValueError(f"could not inspect E03 route container {name}")
        try:
            value = json.loads(completed.stdout)[0]
            config = value["Config"]
            state = value["State"]
            network_settings = value["NetworkSettings"]
        except (
            IndexError,
            KeyError,
            TypeError,
            json.JSONDecodeError,
        ) as error:
            raise ValueError(
                f"invalid E03 route container inspection for {name}"
            ) from error
        labels = config.get("Labels")
        labels = labels if isinstance(labels, dict) else {}
        environment = {}
        for row in config.get("Env") or []:
            if isinstance(row, str) and "=" in row:
                key, entry = row.split("=", 1)
                environment[key] = entry
        networks = network_settings.get("Networks")
        networks = networks if isinstance(networks, dict) else {}
        return {
            "container_id": value.get("Id"),
            "running": state.get("Running") is True,
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
            "networks": networks,
            "ports": network_settings.get("Ports") or {},
            "environment": environment,
        }

    parsed_api = urllib.parse.urlsplit(api_base_url)
    api = inspect(api_container)
    db = inspect(db_container)
    api_network_names = sorted(api["networks"])
    db_network_names = sorted(db["networks"])
    shared_network = (
        api_network_names[0]
        if len(api_network_names) == 1
        and api_network_names == db_network_names
        else None
    )
    db_aliases = []
    if shared_network:
        aliases = db["networks"][shared_network].get("Aliases") or []
        db_aliases = sorted(
            alias for alias in aliases if isinstance(alias, str)
        )

    published = api["ports"].get("8080/tcp")
    published = published if isinstance(published, list) else []
    binding = published[0] if len(published) == 1 else {}
    host_ip = str(binding.get("HostIp") or "")
    host_port = str(binding.get("HostPort") or "")
    try:
        loopback_host = bool(
            host_ip and ipaddress.ip_address(host_ip).is_loopback
        )
    except ValueError:
        loopback_host = False

    database_checks: dict[str, bool] = {}
    for name in (
        "BRUNN_DATABASE_URL",
        "BRUNN_READ_ONLY_DATABASE_URL",
    ):
        raw = api["environment"].get(name, "")
        parsed = urllib.parse.urlsplit(raw)
        try:
            port = parsed.port
        except ValueError:
            port = None
        database_checks[name] = bool(
            parsed.scheme in {"postgres", "postgresql"}
            and parsed.hostname == "db"
            and port == 5432
            and not parsed.query
            and not parsed.fragment
        )

    try:
        api_port = parsed_api.port
    except ValueError:
        api_port = None
    checks = {
        "api_url_is_exact_loopback_publish": bool(
            parsed_api.scheme == "http"
            and parsed_api.hostname == host_ip
            and str(api_port or "") == host_port
            and parsed_api.path in {"", "/"}
            and parsed_api.username is None
            and parsed_api.password is None
            and not parsed_api.query
            and not parsed_api.fragment
            and loopback_host
        ),
        "api_has_one_published_service_port": len(published) == 1,
        "same_single_compose_network": shared_network is not None,
        "same_nonempty_compose_project": bool(
            api["compose_project"]
            and api["compose_project"] == db["compose_project"]
        ),
        "api_service_running": bool(
            api["running"] and api["compose_service"] == "api"
        ),
        "db_service_running": bool(
            db["running"] and db["compose_service"] == "db"
        ),
        "named_db_alias_is_bound": "db" in db_aliases,
        "api_database_urls_target_named_db": all(database_checks.values()),
    }
    result = {
        "schema": "brunn-e03-api-route-binding@v1",
        "api_container_id": api["container_id"],
        "db_container_id": db["container_id"],
        "compose_project": api["compose_project"],
        "network": shared_network,
        "published_host": host_ip or None,
        "published_port": int(host_port) if host_port.isdigit() else None,
        "api_base_url_sha256": hashlib.sha256(
            api_base_url.encode("utf-8")
        ).hexdigest(),
        "database_route": {
            "hostname": "db",
            "port": 5432,
            "aliases": db_aliases,
            "checks": database_checks,
            "credential_values_recorded": False,
        },
        "checks": checks,
        "pass": all(checks.values()),
    }
    if not result["pass"]:
        raise ValueError(f"E03 API/database route mismatch: {result}")
    return result


def e03_mode1_coverage_snapshot(
    db_container: str,
    user_ref: str,
) -> dict[str, Any]:
    user_id = uuid.UUID(user_ref.removeprefix("user:"))
    payload = run_psql(
        db_container,
        f"""
SELECT json_build_object(
  'user_ref','user:{user_id}',
  'chunks',(
    SELECT count(*) FROM brunn.search_chunks
    WHERE user_id='{user_id}'::uuid
  ),
  'semantic_ready_chunks',(
    SELECT count(*) FROM brunn.search_chunks
    WHERE user_id='{user_id}'::uuid AND embedding IS NOT NULL
  ),
  'pending_chunks',(
    SELECT count(*) FROM brunn.search_chunks
    WHERE user_id='{user_id}'::uuid AND embedding IS NULL
  ),
  'embed_jobs_queued_or_running',(
    SELECT count(*) FROM brunn.jobs
    WHERE user_id='{user_id}'::uuid AND kind='embed_entry'
      AND status IN ('queued','running')
  )
);
""",
    )
    value = json.loads(payload.splitlines()[-1])
    result = {
        "user_ref": str(value["user_ref"]),
        "chunks": int(value["chunks"]),
        "semantic_ready_chunks": int(value["semantic_ready_chunks"]),
        "pending_chunks": int(value["pending_chunks"]),
        "embed_jobs_queued_or_running": int(
            value["embed_jobs_queued_or_running"]
        ),
    }
    result["pass"] = bool(
        result["chunks"] > 0
        and result["semantic_ready_chunks"] == 0
        and result["pending_chunks"] == result["chunks"]
    )
    return result


def e03_mode1_service_evidence(
    provisioning: dict[str, Any],
) -> dict[str, Any]:
    metadata = provisioning.get("provisioning")
    metadata = metadata if isinstance(metadata, dict) else {}
    imported = metadata.get("import_response")
    imported = imported if isinstance(imported, dict) else {}
    index_status = recursive_find(imported, "index_status")
    index_status = index_status if isinstance(index_status, dict) else {}
    user_ref = str(recursive_find(imported, "user_id") or "")
    try:
        uuid.UUID(user_ref.removeprefix("user:"))
        valid_user_ref = user_ref.startswith("user:")
    except (ValueError, AttributeError):
        valid_user_ref = False
    result = {
        "credential_provenance": provisioning.get("credential_provenance"),
        "authorization_scope_matches_request": (
            provisioning.get("authorization_scope")
            == provisioning.get("requested_authorization_scope")
        ),
        "user_ref": user_ref or None,
        "index_status": {
            lane: index_status.get(lane)
            for lane in ("exact", "lexical", "semantic")
        },
    }
    result["pass"] = bool(
        result["credential_provenance"] == "service_issued_case_scope"
        and result["authorization_scope_matches_request"]
        and valid_user_ref
        and str(index_status.get("exact", "")).casefold() == "ready"
        and str(index_status.get("lexical", "")).casefold() == "ready"
        and str(index_status.get("semantic", "")).casefold() == "pending"
    )
    return result


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


def validate_semantic_failure_probe_posture(
    *,
    posture: str,
    runtime_snapshot: dict[str, Any],
    retrieval_modes: Sequence[str],
    wait_for_semantic: bool,
    e09_arm: str | None,
    protocol: str,
    hooks_configured: bool,
) -> dict[str, Any]:
    runtime_features = runtime_snapshot.get("runtime_features")
    if not isinstance(runtime_features, dict):
        raise ValueError("runtime snapshot omitted runtime_features")
    semantic_lane = runtime_features.get("semantic_lane")
    if not isinstance(semantic_lane, bool):
        raise ValueError("runtime snapshot omitted boolean semantic_lane")
    modes = list(dict.fromkeys(retrieval_modes))
    observed = {
        "semantic_lane": semantic_lane,
        "retrieval_modes": modes,
        "wait_for_semantic": bool(wait_for_semantic),
        "e09_arm": e09_arm,
        "protocol": protocol,
        "hooks_configured": hooks_configured,
    }
    if posture == SEMANTIC_FAILURE_PROBE_REQUIRED:
        eligible = (
            protocol == "simple"
            and semantic_lane
            and "semantic" in modes
            and wait_for_semantic
            and e09_arm in {None, "unbounded_semantic", "deadline_cache"}
            and hooks_configured
        )
        if not eligible:
            raise ValueError(
                "required semantic-failure probe needs --protocol simple, an "
                "authenticated runtime with semantic_lane=true, semantic in "
                "--retrieval-modes, --wait-semantic, an absent or semantic "
                "E09 arm, and both failure/restore hooks"
            )
        reason = (
            "semantic retrieval is active; provider failure and restoration "
            "must be proven"
        )
    elif posture == SEMANTIC_FAILURE_PROBE_NOT_APPLICABLE:
        eligible = (
            not semantic_lane
            and "semantic" not in modes
            and not wait_for_semantic
            and e09_arm in {None, "no_semantic"}
            and not hooks_configured
        )
        if not eligible:
            raise ValueError(
                "semantic-failure probe may be not-applicable only when the "
                "authenticated runtime has semantic_lane=false, retrieval "
                "modes exclude semantic, --wait-semantic is absent, the E09 "
                "arm is absent or no_semantic, and no failure hooks are supplied"
            )
        reason = (
            "semantic retrieval is disabled in both the authenticated runtime "
            "and the requested retrieval modes"
        )
    else:
        raise ValueError(f"unknown semantic-failure probe posture {posture!r}")
    return {
        "posture": posture,
        "eligible": eligible,
        "reason": reason,
        "observed": observed,
    }


def validate_verbatim_feature_acceptance_posture(
    *,
    posture: str,
    runtime_snapshot: Mapping[str, Any],
    expected_features: Mapping[str, Any],
    protocol: str,
) -> dict[str, Any]:
    runtime_features = runtime_snapshot.get("runtime_features")
    if not isinstance(runtime_features, Mapping):
        raise ValueError(
            "verbatim feature-acceptance posture requires authenticated "
            "runtime_features"
        )
    observed = runtime_features.get("verbatim_spans")
    if posture == VERBATIM_FEATURE_ACCEPTANCE_REQUIRED:
        reason = (
            "verbatim identifier feature acceptance remains a blocking gate"
        )
    elif posture == VERBATIM_FEATURE_ACCEPTANCE_NOT_APPLICABLE:
        if (
            protocol != "simple"
            or expected_features.get("verbatim_spans") is not False
            or observed is not False
        ):
            raise ValueError(
                "verbatim feature acceptance may be not-applicable only for "
                "the simple protocol with an explicit authenticated "
                "verbatim_spans=off expectation"
            )
        reason = (
            "verbatim_spans is an explicitly disabled nuisance feature; "
            "measurement integrity remains blocking"
        )
    else:
        raise ValueError(
            f"unknown verbatim feature-acceptance posture {posture!r}"
        )
    return {
        "posture": posture,
        "eligible": True,
        "reason": reason,
        "observed": {"verbatim_spans": observed},
    }


def validate_e09_request_modes(
    e09_arm: str | None,
    retrieval_modes: Sequence[str],
) -> None:
    if (
        e09_arm == "no_semantic"
        and list(dict.fromkeys(retrieval_modes)) != ["exact", "lexical"]
    ):
        raise ValueError(
            "E09 no_semantic requires --retrieval-modes exact lexical"
        )


def validate_lexical_consolidation_request(
    args: argparse.Namespace,
    retrieval_modes: Sequence[str],
    expected_features: Mapping[str, Any],
) -> None:
    if (
        getattr(args, "gate_profile", None)
        != LEXICAL_CONSOLIDATION_GATE_PROFILE
    ):
        return
    if not args.future_soak or args.protocol != "simple":
        raise ValueError(
            "the E05 lexical-consolidation guard profile requires "
            "--future-soak and --protocol simple"
        )
    if list(retrieval_modes) != ["exact", "lexical"]:
        raise ValueError(
            "the E05 lexical-consolidation guard profile requires "
            "--retrieval-modes exact lexical"
        )
    if not isinstance(expected_features.get("lexical_single_scan"), bool):
        raise ValueError(
            "the E05 lexical-consolidation run must explicitly declare "
            "--expect-feature-flag lexical_single_scan=on|off"
        )
    if (
        getattr(
            args,
            "verbatim_feature_acceptance",
            VERBATIM_FEATURE_ACCEPTANCE_REQUIRED,
        )
        != VERBATIM_FEATURE_ACCEPTANCE_NOT_APPLICABLE
    ):
        raise ValueError(
            "the E05 lexical-consolidation profile requires explicit "
            "--verbatim-feature-acceptance not-applicable"
        )


def validate_resume_delta_fixture_request(
    args: argparse.Namespace,
    retrieval_modes: Sequence[str],
) -> None:
    requested = bool(getattr(args, "exercise_resume_delta_fixture", False))
    if requested and (
        not args.future_soak
        or args.protocol != "simple"
        or list(retrieval_modes) != ["exact", "lexical"]
    ):
        raise ValueError(
            "--exercise-resume-delta-fixture requires --future-soak, "
            "--protocol simple, and --retrieval-modes exact lexical"
        )
    if (
        getattr(args, "gate_profile", None)
        == D03_RESUME_DELTAS_GATE_PROFILE
        and not requested
    ):
        raise ValueError(
            "the D03 resume-deltas profile requires "
            "--exercise-resume-delta-fixture"
        )


def validate_e03_request(
    args: argparse.Namespace,
    retrieval_modes: Sequence[str],
    expected_features: dict[str, Any],
) -> None:
    """Fail closed unless the requested run is an explicitly named E03 arm."""
    arm = getattr(args, "e03_arm", None)
    profile_selected = (
        getattr(args, "gate_profile", None)
        == E03_SEMANTIC_READY_GATE_PROFILE
    )
    if arm and not profile_selected:
        raise ValueError(
            "--e03-arm requires --gate-profile e03-semantic-ready"
        )
    if profile_selected and arm not in E03_ARMS:
        raise ValueError(
            "--gate-profile e03-semantic-ready requires explicit "
            "--e03-arm mode1|mode2|mode3"
        )
    if not profile_selected:
        return
    if args.protocol != "simple":
        raise ValueError("the E03 profile requires --protocol simple")
    if args.e09_arm is not None:
        raise ValueError("the E03 profile cannot be combined with --e09-arm")
    if args.query_budget_profile != DEFAULT_QUERY_BUDGET_PROFILE:
        raise ValueError(
            "the E03 profile requires --query-budget-profile default-safe"
        )
    if (
        getattr(
            args,
            "verbatim_feature_acceptance",
            VERBATIM_FEATURE_ACCEPTANCE_REQUIRED,
        )
        != VERBATIM_FEATURE_ACCEPTANCE_NOT_APPLICABLE
    ):
        raise ValueError(
            "the E03 profile requires explicit "
            "--verbatim-feature-acceptance not-applicable"
        )
    if getattr(args, "quick", False) or (
        getattr(args, "samples", None) is not None
        and args.samples != DEFINITIVE_SAMPLES
    ):
        raise ValueError(
            f"the definitive E03 profile requires exactly "
            f"{DEFINITIVE_SAMPLES} samples"
        )
    mismatches = {
        name: {"expected": expected, "declared": expected_features.get(name)}
        for name, expected in E03_COMMON_RUNTIME_EXPECTATIONS.items()
        if (
            name not in expected_features
            or type(expected_features[name]) is not type(expected)
            or expected_features[name] != expected
        )
    }
    semantic_expected = arm != "mode1"
    if expected_features.get("semantic_lane") is not semantic_expected:
        mismatches["semantic_lane"] = {
            "expected": semantic_expected,
            "declared": expected_features.get("semantic_lane"),
        }
    if mismatches:
        raise ValueError(
            "the E03 profile requires an explicit frozen runtime posture: "
            f"{mismatches}"
        )
    modes = list(dict.fromkeys(retrieval_modes))
    hooks = bool(
        args.semantic_failure_start_command
        and args.semantic_failure_stop_command
    )
    if arm == "mode1":
        valid = (
            modes == ["exact", "lexical"]
            and args.semantic_failure_probe
            == SEMANTIC_FAILURE_PROBE_NOT_APPLICABLE
            and not args.wait_semantic
            and not hooks
            and not args.require_semantic_failure_hook_attestation
            and not args.unique_queries
            and args.worker_container is None
        )
        expected = (
            "exact lexical, semantic failure not-applicable, no wait/hooks/"
            "attestation/unique queries, and no worker container"
        )
    elif arm == "mode2":
        valid = (
            modes == ["exact", "lexical", "semantic"]
            and args.semantic_failure_probe == SEMANTIC_FAILURE_PROBE_REQUIRED
            and args.wait_semantic
            and hooks
            and not args.require_semantic_failure_hook_attestation
            and not args.unique_queries
            and bool(args.worker_container)
        )
        expected = (
            "exact lexical semantic, wait, required owned-mock hooks, no hook "
            "attestation, no unique queries, and an explicit worker container"
        )
    else:
        if (
            list(args.scales or DEFAULT_SCALES) != [PRODUCTION_RECORDS]
            or args.future_soak
        ):
            raise ValueError(
                "E03 mode3 is a single-import paired profile and requires "
                f"--scales {PRODUCTION_RECORDS} without --future-soak"
            )
        valid = (
            modes == ["exact", "lexical", "semantic"]
            and args.semantic_failure_probe == SEMANTIC_FAILURE_PROBE_REQUIRED
            and args.wait_semantic
            and hooks
            and args.require_semantic_failure_hook_attestation
            and not args.unique_queries
            and bool(args.worker_container)
        )
        expected = (
            "exact lexical semantic, wait, required attested proxy hooks, and "
            "the built-in paired cold/warm query path with an explicit worker"
        )
    if not valid:
        raise ValueError(f"E03 {arm} posture requires {expected}")


def validate_e03_runtime_metadata(
    arm: str | None,
    runtime_snapshot: dict[str, Any],
) -> None:
    if arm not in E03_ARMS:
        return
    embeddings = runtime_snapshot.get("embeddings")
    if not isinstance(embeddings, dict):
        raise ValueError("E03 runtime omitted embeddings metadata")
    expected_provider = "hashing" if arm == "mode1" else "openai"
    expected_status = "degraded" if arm == "mode1" else "ready"
    expected_model = E03_EMBEDDING_MODELS[arm]
    if (
        embeddings.get("provider") != expected_provider
        or embeddings.get("model") != expected_model
        or embeddings.get("dimensions") != E03_EMBEDDING_DIMENSIONS
        or embeddings.get("status") != expected_status
    ):
        raise ValueError(
            "E03 embedding runtime mismatch: "
            f"arm {arm} requires provider={expected_provider!r}, "
            f"model={expected_model!r}, "
            f"dimensions={E03_EMBEDDING_DIMENSIONS}, and "
            f"status={expected_status!r}"
        )


def resolve_run_profile(args: argparse.Namespace) -> RunProfile:
    definitive = not bool(args.quick)
    semantic_failure_probe = getattr(
        args,
        "semantic_failure_probe",
        SEMANTIC_FAILURE_PROBE_REQUIRED,
    )
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

    e03_semantic_backfill = (
        getattr(args, "gate_profile", None)
        == E03_SEMANTIC_READY_GATE_PROFILE
        and getattr(args, "e03_arm", None) in {"mode2", "mode3"}
    )
    default_import_timeout = (
        E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS
        if e03_semantic_backfill
        else 7_200.0 if args.future_soak else 1_800.0
    )
    import_timeout = (
        float(args.import_timeout)
        if args.import_timeout is not None
        else default_import_timeout
    )
    if import_timeout <= 0:
        raise ValueError("--import-timeout must be positive")
    if (
        e03_semantic_backfill
        and import_timeout < E03_SEMANTIC_IMPORT_TIMEOUT_SECONDS
    ):
        raise ValueError(
            "E03 semantic arms require --import-timeout >= 43200 seconds "
            "to preserve the documented 12-hour stall boundary"
        )
    return RunProfile(
        scales=tuple(scales),
        samples=samples,
        definitive=definitive,
        future_soak_requested=bool(args.future_soak),
        import_timeout_seconds=import_timeout,
        semantic_failure_required=(
            semantic_failure_probe == SEMANTIC_FAILURE_PROBE_REQUIRED
        ),
        semantic_failure_probe_posture=semantic_failure_probe,
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
                "schema": "brunn-synthetic-fixture@v2",
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
        prefix=f"brunn-flat-{scale}-",
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
            os.environ.get("POSTGRES_DB", "brunn"),
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


def sql_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def container_env_value(container: str, name: str) -> str | None:
    completed = subprocess.run(
        [
            "docker",
            "inspect",
            "--format={{json .Config.Env}}",
            container,
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"could not inspect environment for {container}")
    values = json.loads(completed.stdout)
    prefix = f"{name}="
    return next(
        (
            value[len(prefix):]
            for value in values
            if isinstance(value, str) and value.startswith(prefix)
        ),
        None,
    )


def explain_json(container: str, prelude: str, statement: str) -> Any:
    output = run_psql(
        container,
        f"""
BEGIN;
{prelude}
EXPLAIN (FORMAT JSON) {statement};
ROLLBACK;
""",
    )
    return json.loads(output)


def retrieval_plan_assertions(
    container: str,
    *,
    target_path: str,
    query: str,
    contract_path: Path = RETRIEVAL_PLAN_CONTRACT_PATH,
    retrieval_modes: Sequence[str] = ("exact", "lexical", "semantic"),
    semantic_lane_enabled: bool = True,
) -> dict[str, Any]:
    modes = list(retrieval_modes)
    if (
        not modes
        or len(modes) != len(set(modes))
        or any(
            mode not in {"exact", "lexical", "semantic"}
            for mode in modes
        )
    ):
        raise ValueError(f"invalid retrieval modes for plan assertions: {modes}")
    requested = set(modes)
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    if contract.get("schema") != "brunn-retrieval-plan-contract@v1":
        raise ValueError(f"unsupported retrieval plan contract in {contract_path}")
    if not re.fullmatch(r"[a-z_][a-z0-9_]*", str(contract.get("app_role", ""))):
        raise ValueError("retrieval plan contract has an unsafe app role")

    identity_lines = run_psql(
        container,
        f"""
SELECT entry.user_id::text || '|' || credential.id::text
FROM brunn.entries AS entry
CROSS JOIN LATERAL (
  SELECT id
  FROM brunn.api_credentials
  WHERE user_id=entry.user_id AND disabled_at IS NULL
  ORDER BY created_at,id
  LIMIT 1
) AS credential
WHERE entry.path={sql_literal(target_path)}
ORDER BY entry.updated_at DESC,entry.id DESC
LIMIT 1;
""",
    ).splitlines()
    identity = identity_lines[-1] if identity_lines else ""
    try:
        user_id, credential_id = identity.split("|", 1)
        uuid.UUID(user_id)
        uuid.UUID(credential_id)
    except (ValueError, IndexError) as error:
        raise RuntimeError(
            f"could not resolve plan-gate identity for {target_path}"
        ) from error

    vector_literal = (
        "'[" + ",".join(["0.001"] * 1536) + "]'::public.vector"
    )
    request_timeout_text = container_env_value(
        container,
        "BRUNN_REQUEST_TIMEOUT_SECONDS",
    )
    try:
        request_timeout_seconds = int(request_timeout_text or "30")
    except ValueError as error:
        raise RuntimeError(
            "BRUNN_REQUEST_TIMEOUT_SECONDS is not an integer"
        ) from error
    statement_timeout_ms = max(1, request_timeout_seconds - 5) * 1_000
    context_gucs = "\n".join([
        f"SET LOCAL app.current_user_id={sql_literal(user_id)};",
        f"SET LOCAL app.current_credential_id={sql_literal(credential_id)};",
        "SET LOCAL app.context_valid='true';",
        "SET LOCAL app.capabilities='open,query,read';",
        "SET LOCAL app.scope_refs='';",
        "SET LOCAL app.scope_ids='';",
        f"SET LOCAL statement_timeout={sql_literal(f'{statement_timeout_ms}ms')};",
        "SET LOCAL hnsw.iterative_scan='relaxed_order';",
    ])
    role_prelude = (
        f"SET LOCAL ROLE {contract['app_role']};\n" + context_gucs
    )
    owner_prelude = context_gucs + "\nSET LOCAL row_security=off;"

    lanes: dict[str, Any] = {}
    drift_checks = []
    for lane, lane_contract in contract["lanes"].items():
        function_name = str(lane_contract["function_name"])
        regprocedure = str(lane_contract["regprocedure"])
        invocation_sql = str(lane_contract["invocation_sql"])
        if not re.fullmatch(
            r"[a-z_][a-z0-9_]*\.[a-z_][a-z0-9_]*",
            function_name,
        ):
            raise ValueError(f"unsafe retrieval function name for {lane}")
        if not re.fullmatch(
            r"[a-z_][a-z0-9_]*\.[a-z_][a-z0-9_]*\([a-z0-9_(), \[\]]+\)",
            regprocedure,
        ):
            raise ValueError(f"unsafe retrieval regprocedure for {lane}")
        if invocation_sql != f"SELECT * FROM {function_name}($1,$2)":
            raise ValueError(f"retrieval invocation drift for {lane}")
        migration = (PROJECT_ROOT / lane_contract["migration"]).resolve()
        if not migration.is_relative_to(PROJECT_ROOT.resolve()):
            raise ValueError(f"retrieval migration escapes project root for {lane}")
        source_body = extract_sql_function_body(
            migration.read_text(encoding="utf-8"),
            function_name,
        )
        source_fingerprint = sql_fingerprint(source_body)
        installed_body = run_psql(
            container,
            (
                "SELECT prosrc FROM pg_proc "
                f"WHERE oid={sql_literal(regprocedure)}"
                "::regprocedure;"
            ),
        )
        installed_fingerprint = sql_fingerprint(installed_body)
        expected_fingerprint = lane_contract["body_sha256"]
        drift = {
            "lane": lane,
            "migration": lane_contract["migration"],
            "expected_sha256": expected_fingerprint,
            "source_sha256": source_fingerprint,
            "installed_sha256": installed_fingerprint,
            "pass": (
                source_fingerprint == expected_fingerprint
                and installed_fingerprint == expected_fingerprint
            ),
        }
        drift_checks.append(drift)

        if (
            lane not in requested
            or (lane == "semantic" and not semantic_lane_enabled)
        ):
            continue

        if lane == "lexical":
            argument = f"ARRAY[{sql_literal(query)}]::text[]"
            body_statement = re.sub(
                r"\bp_queries\b",
                argument,
                source_body,
            )
        elif lane == "semantic":
            argument = vector_literal
            body_statement = re.sub(
                r"\bp_embedding\b",
                argument,
                source_body,
            )
        else:
            raise ValueError(f"unsupported retrieval plan lane {lane}")

        sort_argument = sql_literal("best_match")
        body_statement = re.sub(
            r"\bp_sort\b",
            sort_argument,
            body_statement,
        )

        invocation = invocation_sql.replace(
            "$1",
            argument,
        ).replace("$2", sort_argument)
        invocation_plan = explain_json(
            container,
            role_prelude,
            invocation,
        )
        body_plan = explain_json(
            container,
            owner_prelude,
            body_statement,
        )
        plan_assertion = evaluate_retrieval_plan(
            body_plan,
            lane=lane,
            expected_index=lane_contract["expected_index"],
        )
        lanes[lane] = {
            "app_role_invocation_explain": invocation_plan,
            "function_owner_body_explain": body_plan,
            "plan_assertion": plan_assertion,
            "pass": drift["pass"] and plan_assertion["pass"],
        }

    return apply_retrieval_plan_applicability({
        "status": "complete",
        "contract": str(contract_path.relative_to(PROJECT_ROOT)),
        "scale_identity_path": target_path,
        "production_gucs": {
            "hnsw.iterative_scan": "relaxed_order",
            "statement_timeout_ms": statement_timeout_ms,
            "row_security_for_function_body": "off",
        },
        "app_role": contract["app_role"],
        "explain_boundary": (
            "PostgreSQL represents SECURITY DEFINER SQL-function calls as a "
            "Function Scan. The gate therefore EXPLAINs the callable as the "
            "app role to prove the invocation boundary, then EXPLAINs the "
            "fingerprinted installed function body with its owner/RLS "
            "semantics to assert nested search_chunks plan nodes."
        ),
        "sql_drift": drift_checks,
        "lanes": lanes,
    }, retrieval_modes=modes, semantic_lane_enabled=semantic_lane_enabled)


def database_snapshot(container: str) -> DatabaseSnapshot:
    sql = r"""
CREATE TEMP TABLE benchmark_counts(table_name text PRIMARY KEY, row_count bigint);
DO $$
DECLARE item record;
BEGIN
  FOR item IN
    SELECT schemaname, tablename
    FROM pg_tables
    WHERE schemaname = 'brunn'
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
WHERE schemaname='brunn'
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
  FROM brunn.entries AS item
  WHERE item.id='{parsed}'::uuid
  UNION ALL
  SELECT 'entry_versions',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM brunn.entry_versions AS item
  WHERE item.entry_id='{parsed}'::uuid
  UNION ALL
  SELECT 'workspace_changes',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM brunn.workspace_changes AS item
  WHERE item.entry_id='{parsed}'::uuid
  UNION ALL
  SELECT 'search_chunks',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM brunn.search_chunks AS item
  WHERE item.entry_id='{parsed}'::uuid
  UNION ALL
  SELECT 'jobs',count(*)::bigint,
         coalesce(sum(pg_column_size(item)),0)::bigint
  FROM brunn.jobs AS item
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
    require_attestation: bool = False,
    expected_mode: str | None = None,
) -> dict[str, Any]:
    return run_provenance_hook(
        command_text,
        timeout_seconds=timeout_seconds,
        require_attestation=require_attestation,
        expected_mode=expected_mode,
    )


def semantic_failure_probe(
    client: NativeApiClient,
    *,
    protocol: str,
    authorization_scope: str,
    session_id: str | None,
    query: str,
    marker: str,
    target_path: str,
    required: bool,
    start_command: str | None,
    stop_command: str | None,
    settle_seconds: float,
    timeout_seconds: float,
    require_hook_attestation: bool = False,
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

    def search(
        query_id: str,
        query_text: str,
        modes: list[str],
    ) -> tuple[dict[str, Any], float]:
        payload: dict[str, Any] = {
            "queries": [{
                "id": query_id,
                "goal": "locate the isolated semantic failure probe source",
                "query": query_text,
                "scope": authorization_scope,
                "modes": modes,
                "limit": 8,
            }],
        }
        if session_id:
            payload["session_id"] = session_id
        body, elapsed_ms = request_with_result(
            client,
            search_path,
            payload,
        )
        return body, round(elapsed_ms, 3)

    latencies_ms: dict[str, float | None] = {
        "baseline_semantic": None,
        "injected_semantic": None,
        "injected_exact_lexical": None,
        "injected_mixed": None,
        "restored_semantic": None,
    }
    candidate_audits: dict[str, dict[str, Any] | None] = {
        "baseline_semantic": None,
        "injected_semantic": None,
        "injected_exact_lexical": None,
        "injected_mixed": None,
        "restored_semantic": None,
    }
    try:
        baseline, latencies_ms["baseline_semantic"] = search(
            "semantic-provider-baseline",
            f"{query} semantic-baseline",
            ["semantic"],
        )
    except NativeApiError as error:
        latencies_ms["baseline_semantic"] = getattr(error, "elapsed_ms", None)
        return {
            "status": "baseline_failed",
            "pass": False,
            "required": required,
            "reason": "semantic-only retrieval did not work before injection",
            "baseline_http_status": error.status,
            "latencies_ms": latencies_ms,
            "candidate_audits": candidate_audits,
        }
    baseline_audit = candidate_audit(
        baseline,
        marker=marker,
        target_path=target_path,
    )
    candidate_audits["baseline_semantic"] = baseline_audit
    baseline_target_found = bool(baseline_audit["target_found"])
    baseline_healthy = bool(
        not response_reports_lane_failure(baseline, "semantic")
        and baseline_target_found
    )
    if not baseline_healthy:
        return {
            "status": "baseline_failed",
            "pass": False,
            "required": required,
            "reason": (
                "semantic-only retrieval did not return the planted target "
                "before failure injection"
            ),
            "baseline_semantic_lane_healthy": False,
            "baseline_semantic_target_found": baseline_target_found,
            "latencies_ms": latencies_ms,
            "candidate_audits": candidate_audits,
        }

    start_result = run_external_hook(
        start_command,
        timeout_seconds=timeout_seconds,
        require_attestation=require_hook_attestation,
        expected_mode="error" if require_hook_attestation else None,
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
                failed_semantic, latencies_ms["injected_semantic"] = search(
                    "semantic-provider-outage",
                    f"{query} semantic-outage",
                    ["semantic"],
                )
                candidate_audits["injected_semantic"] = candidate_audit(
                    failed_semantic,
                    marker=marker,
                    target_path=target_path,
                )
                semantic_failure_observed = response_reports_lane_failure(
                    failed_semantic,
                    "semantic",
                )
            except NativeApiError as error:
                semantic_failure_observed = True
                semantic_status = error.status
                latencies_ms["injected_semantic"] = getattr(
                    error,
                    "elapsed_ms",
                    None,
                )

            lexical, latencies_ms["injected_exact_lexical"] = search(
                "provider-outage-lexical",
                query,
                ["exact", "lexical"],
            )
            lexical_audit = candidate_audit(
                lexical,
                marker=marker,
                target_path=target_path,
            )
            candidate_audits["injected_exact_lexical"] = lexical_audit
            lexical_found = bool(lexical_audit["target_found"])
            mixed, latencies_ms["injected_mixed"] = search(
                "provider-outage-mixed",
                f"{query} mixed-outage",
                ["exact", "lexical", "semantic"],
            )
            mixed_audit = candidate_audit(
                mixed,
                marker=marker,
                target_path=target_path,
            )
            candidate_audits["injected_mixed"] = mixed_audit
            mixed_found = bool(mixed_audit["target_found"])
    except (NativeApiError, RuntimeError) as error:
        probe_error = f"{type(error).__name__}: {error}"
    finally:
        restore_result = run_external_hook(
            stop_command,
            timeout_seconds=timeout_seconds,
            require_attestation=require_hook_attestation,
            expected_mode="forward" if require_hook_attestation else None,
        )
        time.sleep(max(0.0, settle_seconds))
        if restore_result["pass"]:
            try:
                restored, latencies_ms["restored_semantic"] = search(
                    "semantic-provider-restored",
                    f"{query} semantic-restored",
                    ["semantic"],
                )
                restored_audit = candidate_audit(
                    restored,
                    marker=marker,
                    target_path=target_path,
                )
                candidate_audits["restored_semantic"] = restored_audit
                restored_found = bool(
                    not response_reports_lane_failure(restored, "semantic")
                    and restored_audit["target_found"]
                )
            except NativeApiError as error:
                restored_found = False
                latencies_ms["restored_semantic"] = getattr(
                    error,
                    "elapsed_ms",
                    None,
                )

    hook_target_bound = (
        hook_target_matches(start_result, restore_result or {})
        if require_hook_attestation
        else None
    )
    passed = bool(
        start_result["pass"]
        and restore_result["pass"]
        and (hook_target_bound is not False)
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
        "baseline_semantic_target_found": baseline_target_found,
        "semantic_failure_observed": semantic_failure_observed,
        "semantic_failure_http_status": semantic_status,
        "exact_lexical_found_during_failure": lexical_found,
        "mixed_lane_found_during_failure": mixed_found,
        "semantic_lane_healthy_after_restore": restored_found,
        "semantic_target_found_after_restore": restored_found,
        "start_hook": start_result,
        "restore_hook": restore_result,
        "hook_target_bound": hook_target_bound,
        "latencies_ms": latencies_ms,
        "candidate_audits": candidate_audits,
        "error": probe_error,
    }


def isolated_semantic_failure_probe(
    admin: NativeApiClient,
    *,
    run_id: str,
    protocol: str,
    required: bool,
    start_command: str | None,
    stop_command: str | None,
    settle_seconds: float,
    timeout_seconds: float,
    require_hook_attestation: bool = False,
) -> dict[str, Any]:
    if not start_command or not stop_command:
        return semantic_failure_probe(
            admin,
            protocol=protocol,
            authorization_scope="",
            session_id=None,
            query="",
            marker="",
            target_path="",
            required=required,
            start_command=start_command,
            stop_command=stop_command,
            settle_seconds=settle_seconds,
            timeout_seconds=timeout_seconds,
            require_hook_attestation=require_hook_attestation,
        )
    if protocol != "simple":
        return {
            "status": "lifecycle_failed",
            "pass": False,
            "required": required,
            "reason": "isolated semantic failure probing requires simple protocol",
        }

    nonce = uuid.uuid4().hex
    case_id = f"semantic-failure-{nonce}"
    marker = f"semantic-failure-marker-{nonce}"
    query = f"locate semantic failure qualification {marker}"
    path = f"eval/semantic-failure-probe/{nonce}.md"
    content = (
        f"# Semantic failure probe {nonce}\n\n"
        f"The unique current qualification marker is `{marker}`.\n\n"
        f"Retrieval intent: {query}.\n"
    )
    document = {
        "path": path,
        "content": content,
        "content_sha256": hashlib.sha256(content.encode("utf-8")).hexdigest(),
        "media_type": "text/markdown",
    }
    metadata: dict[str, Any] | None = None
    scoped: NativeApiClient | None = None
    core_result: dict[str, Any] | None = None
    lifecycle_error: str | None = None
    cleanup: dict[str, Any] = {
        "attempted": False,
        "pass": False,
        "error": "fixture was not provisioned",
    }
    try:
        probe_admin = NativeApiClient(
            base_url=admin.base_url,
            token=admin.token,
            run_id=run_id,
            case_id=case_id,
            timeout=timeout_seconds,
        )
        metadata = provision_evaluation(
            probe_admin,
            run_id=run_id,
            case_id=case_id,
            display_scope=f"Semantic failure probe {nonce}",
            access_mode="read_write",
            documents=[document],
            timeout_seconds=timeout_seconds,
            import_path="/v1/workspace/admin/eval/import",
            wait_for_semantic=True,
        )
        scoped = NativeApiClient(
            base_url=admin.base_url,
            token=str(metadata["token"]),
            run_id=run_id,
            case_id=case_id,
            timeout=timeout_seconds,
        )
        core_result = semantic_failure_probe(
            scoped,
            protocol=protocol,
            authorization_scope=str(metadata["authorization_scope"]),
            session_id=None,
            query=query,
            marker=marker,
            target_path=path,
            required=required,
            start_command=start_command,
            stop_command=stop_command,
            settle_seconds=settle_seconds,
            timeout_seconds=timeout_seconds,
            require_hook_attestation=require_hook_attestation,
        )
    except Exception as error:
        lifecycle_error = f"{type(error).__name__}: {error}"
    finally:
        if scoped is not None and metadata is not None:
            status_url = str(metadata.get("status_url") or "")
            if status_url:
                cleanup = cleanup_fixture(scoped, status_url=status_url)
            else:
                cleanup = {
                    "attempted": False,
                    "pass": False,
                    "error": "provisioning receipt omitted status_url",
                }

    result = dict(core_result or {
        "status": "lifecycle_failed",
        "pass": False,
        "required": required,
        "reason": "isolated semantic failure fixture did not complete",
    })
    result["fixture"] = {
        "case_id": case_id,
        "path": path,
        "content_sha256": document["content_sha256"],
        "marker_sha256": hashlib.sha256(marker.encode("utf-8")).hexdigest(),
        "credential_recorded": False,
        "provisioning": (
            public_provisioning(metadata)
            if metadata is not None
            else None
        ),
    }
    result["cleanup"] = cleanup
    result["lifecycle_error"] = lifecycle_error
    result["pass"] = bool(result.get("pass") and cleanup.get("pass"))
    if core_result is not None and core_result.get("pass") and not result["pass"]:
        result["status"] = "cleanup_failed"
        result["reason"] = (
            "semantic failure behavior passed but fixture cleanup or "
            "credential revocation failed"
        )
    return result


def verbatim_identifier_probe(
    client: NativeApiClient,
    *,
    protocol: str,
    authorization_scope: str,
    session_id: str,
    probes: Sequence[dict[str, Any]],
    response_samples: list[tuple[str, dict[str, Any]]] | None = None,
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
        if response_samples is not None:
            response_samples.append(("verbatim_identifier_search", response))
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


def e03_mode3_paired_query_probe(
    client: NativeApiClient,
    admin: NativeApiClient,
    *,
    authorization_scope: str,
    session_id: str,
    status_url: str,
    scale: int,
    run_id: str,
    discovery_key: str,
    samples: int,
) -> dict[str, Any]:
    """Measure cold then warm semantic queries without provisioning twice."""
    if samples < 1:
        raise ValueError("paired mode-3 query probe requires samples")

    def status_snapshot() -> dict[str, Any]:
        value = client.get(status_url).data
        if not isinstance(value, dict):
            raise RuntimeError("mode-3 import status was not an object")
        return {
            "import_id": value.get("import_id"),
            "corpus_revision": value.get("corpus_revision"),
            "index_status": value.get("index_status"),
            "index_counts": value.get("index_counts"),
        }

    def counters() -> dict[str, Any]:
        value = admin.get("/v1/status").data
        if not isinstance(value, dict):
            raise RuntimeError("mode-3 service status was not an object")
        runtime = value.get("semantic_runtime")
        return dict(runtime) if isinstance(runtime, dict) else {}

    queries = [
        (
            f"{discovery_key} paired semantic latency probe "
            f"{hashlib.sha256(f'{run_id}:{index}'.encode()).hexdigest()[:16]}"
        )
        for index in range(samples)
    ]
    search_path = "/v1/workspace/search"

    def run_phase(phase: str) -> dict[str, Any]:
        phase_started_ns = time.monotonic_ns()
        before = counters()
        times: list[float] = []
        timing_samples: list[dict[str, Any]] = []
        embed_shares: list[float] = []
        healthy: list[bool] = []
        candidate_samples: list[bool] = []
        for index, query in enumerate(queries):
            body, elapsed_ms = request_with_result(
                client,
                search_path,
                {
                    "session_id": session_id,
                    "queries": [{
                        "id": f"e03-mode3-{phase}-{index:04d}",
                        "goal": "locate the current terminal-corpus answer",
                        "query": query,
                        "scope": authorization_scope,
                        "modes": ["exact", "lexical", "semantic"],
                        "limit": 8,
                    }],
                },
            )
            times.append(elapsed_ms)
            timing = response_timings(body)
            timing_samples.append(timing)
            flattened = flatten_numeric_timings(timing)
            total_ms = flattened.get("total")
            embed_ms = sum(
                value
                for name, value in flattened.items()
                if "embed" in name.casefold()
            )
            embed_shares.append(
                embed_ms / total_ms
                if isinstance(total_ms, (int, float)) and total_ms > 0
                else -1.0
            )
            healthy.append(
                not response_reports_lane_failure(body, "semantic")
                and not response_reports_gap_kind(
                    body,
                    "retrieval_lane_unavailable",
                )
                and not response_reports_gap_kind(
                    body,
                    "retrieval_lane_deferred",
                )
            )
            candidate_samples.append(response_has_candidates(body))
        after = counters()
        phase_finished_ns = time.monotonic_ns()
        counter_delta = semantic_counter_delta(before, after)
        expected_counter_delta = {
            "requested": samples,
            "disabled": 0,
            "cache_hits": 0,
            "cache_misses": 0,
            "negative_cache_hits": 0,
            "cache_bypasses": samples,
            "successes": samples,
            "failures": 0,
            "deferrals": 0,
        }
        counters_match = counter_delta == expected_counter_delta
        return {
            "phase": phase,
            "started_monotonic_ns": phase_started_ns,
            "finished_monotonic_ns": phase_finished_ns,
            "query_sha256": [
                hashlib.sha256(query.encode()).hexdigest() for query in queries
            ],
            "samples": samples,
            "latencies_ms": [round(value, 3) for value in times],
            "p50_ms": round(percentile(times, 0.50), 3),
            "p95_ms": round(percentile(times, 0.95), 3),
            "p99_ms": round(percentile(times, 0.99), 3),
            "max_ms": round(max(times), 3),
            "timings_ms": summarize_timing_samples(timing_samples),
            "timing_phase_sum_sane": [
                timing_phase_sum_sane(item) for item in timing_samples
            ],
            "embed_share_of_total": {
                "samples": [round(value, 6) for value in embed_shares],
                "p50": round(percentile(embed_shares, 0.50), 6),
                "p95": round(percentile(embed_shares, 0.95), 6),
                "p99": round(percentile(embed_shares, 0.99), 6),
            },
            "semantic_lane_healthy": healthy,
            "candidates_returned": candidate_samples,
            "counters_before": before,
            "counters_after": after,
            "counter_delta": counter_delta,
            "expected_counter_delta": expected_counter_delta,
            "counter_contract_pass": counters_match,
            "pass": bool(
                all(healthy)
                and all(candidate_samples)
                and all(timing_samples)
                and all(timing_phase_sum_sane(item) for item in timing_samples)
                and all(value >= 0 for value in embed_shares)
                and counters_match
            ),
        }

    cardinality_before = status_snapshot()
    cold = run_phase("cold_unique")
    cardinality_between = status_snapshot()
    warm = run_phase("warm_repeat")
    cardinality_after = status_snapshot()
    same_queries = cold["query_sha256"] == warm["query_sha256"]
    cardinality_stable = (
        cardinality_before == cardinality_between == cardinality_after
    )
    cold_before_warm = (
        cold["finished_monotonic_ns"] <= warm["started_monotonic_ns"]
    )
    return {
        "schema": "brunn-e03-mode3-paired-query@v1",
        "status": "complete",
        "single_provisioning_event": True,
        "cold_before_warm": cold_before_warm,
        "same_query_strings": same_queries,
        "same_session_id": True,
        "session_id_sha256": hashlib.sha256(session_id.encode()).hexdigest(),
        "cardinality": {
            "before": cardinality_before,
            "between": cardinality_between,
            "after": cardinality_after,
            "stable": cardinality_stable,
        },
        "cold": cold,
        "warm": warm,
        "pass": bool(
            cold["pass"]
            and warm["pass"]
            and cold_before_warm
            and same_queries
            and cardinality_stable
        ),
    }


def concurrent_write_search_probe(
    client: NativeApiClient,
    *,
    protocol: str,
    authorization_scope: str,
    session_id: str,
    marker: str,
    run_id: str,
    retrieval_modes: Sequence[str] = ("exact", "lexical", "semantic"),
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
                        "modes": list(retrieval_modes),
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
            response_samples.append(("write", write_body))
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
    api_container: str | None = None,
    protocol: str,
    retrieval_modes: Sequence[str],
    semantic_lane_enabled: bool,
    run_semantic_failure: bool,
    concurrent_rounds: int,
    semantic_failure_required: bool,
    semantic_failure_start_command: str | None,
    semantic_failure_stop_command: str | None,
    semantic_failure_settle_seconds: float,
    require_semantic_failure_hook_attestation: bool,
    wait_for_semantic: bool,
    unique_queries: bool,
    e09_arm: str | None,
    e03_arm: str | None = None,
    run_e03_mode3_paired: bool = False,
    run_e03_mode1_pending: bool = False,
    exercise_resume_delta_fixture: bool = False,
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
        wait_for_semantic=(
            wait_for_semantic
            or protocol != "simple"
            or e09_arm
            in {
                "unbounded_semantic",
                "deadline_cache",
                "deadline_cache_600",
            }
        ),
        batch_size=10_000 if protocol == "simple" else None,
    )
    import_ms = (time.monotonic() - started) * 1000
    if db_container:
        # A bulk import leaves stale planner statistics; production settles
        # via autovacuum within seconds, but sampling immediately races that
        # window and the first opens pay catastrophic misplans. Analyze the
        # touched tables so measurement starts from steady-state plans.
        run_psql(
            db_container,
            "ANALYZE brunn.entries, brunn.entry_versions, "
            "brunn.search_chunks, brunn.workspace_changes;",
        )
    mode1_pending_evidence: dict[str, Any] | None = None
    if run_e03_mode1_pending:
        if not api_container or not db_container:
            raise RuntimeError(
                "E03 Mode 1 pending proof requires API and DB containers"
            )
        service_evidence = e03_mode1_service_evidence(provisioning)
        user_ref = str(service_evidence.get("user_ref") or "")
        environment_before = e03_mode1_environment_snapshot(api_container)
        coverage_before = (
            e03_mode1_coverage_snapshot(db_container, user_ref)
            if service_evidence.get("pass") is True
            else {"pass": False, "reason": "service evidence was invalid"}
        )
        mode1_pending_evidence = {
            "schema": "brunn-e03-mode1-pending@v1",
            "before_sampling": {
                "service": service_evidence,
                "database": coverage_before,
                "environment": environment_before,
            },
            "after_sampling": None,
            "retrieval_integrity": None,
            "pass": False,
        }
        if not all(
            item.get("pass") is True
            for item in (
                service_evidence,
                coverage_before,
                environment_before,
            )
        ):
            raise RuntimeError(
                "E03 Mode 1 preflight did not prove service-issued exact/"
                "lexical readiness, zero semantic-ready chunks, all chunks "
                "pending, no worker, and no provider credential"
            )
    client = NativeApiClient(
        base_url=admin.base_url,
        token=provisioning["token"],
        run_id=run_id,
        case_id=case_id,
        timeout=timeout_seconds,
    )
    authorization_scope = provisioning["authorization_scope"]
    failure_probe = (
        isolated_semantic_failure_probe(
            admin,
            run_id=run_id,
            protocol=protocol,
            required=semantic_failure_required,
            start_command=semantic_failure_start_command,
            stop_command=semantic_failure_stop_command,
            settle_seconds=semantic_failure_settle_seconds,
            timeout_seconds=max(timeout_seconds, 30.0),
            require_hook_attestation=(
                require_semantic_failure_hook_attestation
            ),
        )
        if run_semantic_failure
        else {
            "status": "not_applicable_at_this_scale",
            "pass": None,
            "required": False,
        }
    )
    semantic_coverage_probe: dict[str, Any] | None = None
    if e09_arm in {
        "unbounded_semantic",
        "deadline_cache",
        "deadline_cache_600",
    }:
        semantic_coverage_probe = client.post(
            "/v1/workspace/search",
            {
                "queries": [{
                    "id": "e09-semantic-coverage",
                    "query": (
                        "E09 semantic coverage sentinel for the fully indexed "
                        f"{scale}-document performance fixture"
                    ),
                    "modes": ["semantic"],
                    "limit": 1,
                }],
            },
        ).body
        if (
            response_reports_lane_failure(
                semantic_coverage_probe,
                "semantic",
            )
            or response_reports_gap_kind(
                semantic_coverage_probe,
                "retrieval_lane_unavailable",
            )
            or response_reports_gap_kind(
                semantic_coverage_probe,
                "retrieval_lane_deferred",
            )
            or not response_has_candidates(semantic_coverage_probe)
        ):
            raise RuntimeError(
                "E09 semantic coverage probe was unavailable, empty, failed, "
                "or deferred"
            )
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
                    "modes": list(retrieval_modes),
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
                    "modes": [
                        mode for mode in retrieval_modes if mode != "exact"
                    ],
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
                    "modes": list(retrieval_modes),
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
                    "modes": [
                        mode for mode in retrieval_modes if mode != "exact"
                    ],
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
        response_samples=response_samples,
    )
    status_url = provisioning.get("status_url")
    mode3_paired_query = (
        e03_mode3_paired_query_probe(
            client,
            admin,
            authorization_scope=authorization_scope,
            session_id=latest_session_id,
            status_url=status_url,
            scale=scale,
            run_id=run_id,
            discovery_key=discovery_key,
            samples=samples,
        )
        if (
            run_e03_mode3_paired
            and protocol == "simple"
            and isinstance(status_url, str)
            and status_url
        )
        else {
            "status": "not_requested",
            "pass": None,
        }
    )
    if run_e03_mode3_paired and mode3_paired_query["status"] != "complete":
        raise RuntimeError(
            "E03 mode3 paired query probe requires an eval import status URL"
        )

    before_checkpoint = (
        database_snapshot(db_container)
        if db_container and protocol != "simple"
        else None
    )
    exercise_resume_delta = (
        exercise_resume_delta_fixture
        and scale == FUTURE_RECORDS
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
            "source_refs": [target_path] if exercise_resume_delta else [],
        },
    )
    checkpoint_sample_name = (
        "resume_delta_checkpoint"
        if exercise_resume_delta
        else "checkpoint"
    )
    response_samples.append((checkpoint_sample_name, checkpoint))
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
    resume_delta_fixture = {
        "requested": bool(exercise_resume_delta_fixture),
        "applicable": exercise_resume_delta,
        "status": "not_applicable",
        "pass": None,
        "source_path": target_path if exercise_resume_delta else None,
    }
    if exercise_resume_delta:
        source_entries = recursive_find(checkpoint, "source_entries")
        checkpoint_source = (
            source_entries[0]
            if (
                isinstance(source_entries, list)
                and len(source_entries) == 1
                and isinstance(source_entries[0], dict)
            )
            else {}
        )
        checkpoint_source_version = checkpoint_source.get("version")
        checkpoint_source_hash = checkpoint_source.get("content_hash")
        if (
            checkpoint_source.get("path") != target_path
            or not isinstance(checkpoint_source_version, int)
            or isinstance(checkpoint_source_version, bool)
            or not isinstance(checkpoint_source_hash, str)
            or re.fullmatch(
                r"sha256:[0-9a-f]{64}",
                checkpoint_source_hash,
            )
            is None
        ):
            raise ValueError(
                "resume-delta performance fixture checkpoint did not bind "
                "the exact target source version"
            )
        target_document = next(
            document
            for document in documents
            if document["path"] == target_path
        )
        mutation_marker = f"PERF-RESUME-DELTA-{scale}"
        mutation_content = (
            str(target_document["content"])
            + "\n\n"
            + f"Post-checkpoint revision marker: `{mutation_marker}`.\n"
        )
        expected_checkpoint_hash = (
            "sha256:"
            + hashlib.sha256(
                str(target_document["content"]).encode("utf-8")
            ).hexdigest()
        )
        expected_mutation_hash = (
            "sha256:"
            + hashlib.sha256(mutation_content.encode("utf-8")).hexdigest()
        )
        mutation, mutation_ms = request_with_result(
            client,
            f"{operation_prefix}/write",
            {
                "path": target_path,
                "content": mutation_content,
                "media_type": "text/markdown",
                "metadata": {
                    "kind": "resume_delta_performance_fixture",
                },
                "expected_version": checkpoint_source_version,
            },
        )
        mutation_read, mutation_read_ms = request_with_result(
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
        mutation_path = recursive_find(mutation, "path")
        mutation_version = recursive_find(mutation, "version")
        mutation_hash = recursive_find(mutation, "content_hash")
        mutation_no_op = recursive_find(mutation, "no_op")
        read_path = recursive_find(mutation_read, "path")
        read_version = recursive_find(mutation_read, "version")
        read_hash = recursive_find(mutation_read, "content_hash")
        read_text = recursive_find(mutation_read, "text")
        read_response_truncated = recursive_find(
            mutation_read,
            "response_truncated",
        )
        resume_delta_fixture = {
            "requested": True,
            "applicable": True,
            "status": "complete",
            "pass": (
                isinstance(source_entries, list)
                and len(source_entries) == 1
                and checkpoint_source.get("path") == target_path
                and isinstance(checkpoint_source_version, int)
                and not isinstance(checkpoint_source_version, bool)
                and checkpoint_source_hash == expected_checkpoint_hash
                and mutation_path == target_path
                and mutation_version == checkpoint_source_version + 1
                and mutation_hash == expected_mutation_hash
                and mutation_no_op is False
                and read_path == target_path
                and read_version == mutation_version
                and read_hash == mutation_hash
                and read_text == mutation_content
                and read_response_truncated is False
            ),
            "source_path": target_path,
            "checkpoint_source_entries": (
                len(source_entries)
                if isinstance(source_entries, list)
                else None
            ),
            "checkpoint_source_version": checkpoint_source_version,
            "checkpoint_source_content_hash": checkpoint_source_hash,
            "mutation_version": mutation_version,
            "mutation_content_hash": mutation_hash,
            "mutation_no_op": mutation_no_op,
            "verified_read_version": read_version,
            "verified_read_content_hash": read_hash,
            "verified_read_exact_content": read_text == mutation_content,
            "verified_read_response_truncated": read_response_truncated,
            "mutation_marker": mutation_marker,
            "mutation_write_ms": round(mutation_ms, 3),
            "mutation_read_ms": round(mutation_read_ms, 3),
            "expected_treatment_statement_delta": (
                D03_RESUME_QUERY_COUNT_DELTA
            ),
            "statement_delta_accounting": [
                "transaction_context_validation",
                "transaction_context_setup",
                "statement_timeout_setup",
                "batched_version_pair_select",
                "transaction_commit",
            ],
        }
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
    resume_delta_lineage_samples: list[dict[str, Any]] = []
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
        resume_delta_lineage_samples.append(
            resume_delta_lineage_receipt(
                resumed,
                resume_delta_fixture,
            )
        )
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
        retrieval_modes=retrieval_modes,
        searches=CONCURRENT_SEARCHES_PER_ROUND,
        rounds=concurrent_rounds,
        response_samples=response_samples,
    )
    if run_e03_mode1_pending:
        if (
            mode1_pending_evidence is None
            or not api_container
            or not db_container
        ):
            raise RuntimeError("E03 Mode 1 pending evidence was lost")
        service_before = mode1_pending_evidence["before_sampling"]["service"]
        user_ref = str(service_before.get("user_ref") or "")
        coverage_after = e03_mode1_coverage_snapshot(db_container, user_ref)
        environment_after = e03_mode1_environment_snapshot(api_container)
        environment_before = mode1_pending_evidence[
            "before_sampling"
        ]["environment"]
        same_api_process = bool(
            environment_after.get("api_container_id")
            == environment_before.get("api_container_id")
            and environment_after.get("api_started_at")
            == environment_before.get("api_started_at")
            and environment_after.get("api_image_id")
            == environment_before.get("api_image_id")
        )
        retrieval_integrity = {
            "open_found": all(open_found),
            "search_found": all(search_found),
            "broad_found": all(broad_found),
            "overflow_found": all(overflow_found),
            "old_source_found": all(old_source_found),
            "read_found": all(read_found),
            "resume_found": all(resume_found_samples),
            "concurrent_write_committed": bool(
                concurrent_probe["write_committed"]
            ),
            "concurrent_search_found": all(
                concurrent_probe["search_found"]
            ),
            "no_exact_or_lexical_failures": all(
                not item.get("exact") and not item.get("lexical")
                for item in critical_lane_failures
            ),
        }
        retrieval_integrity["pass"] = all(retrieval_integrity.values())
        mode1_pending_evidence["after_sampling"] = {
            "database": coverage_after,
            "environment": environment_after,
            "same_api_process": same_api_process,
        }
        mode1_pending_evidence["retrieval_integrity"] = retrieval_integrity
        mode1_pending_evidence["pass"] = bool(
            coverage_after.get("pass") is True
            and environment_after.get("pass") is True
            and same_api_process
            and retrieval_integrity["pass"]
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
    query_counts = summarize_query_counts(
        response_samples,
        expected_cardinality=(
            expected_query_count_sample_cardinality(
                scale=scale,
                samples_per_retrieval=samples,
                verbatim_identifier_probes=len(
                    fixture_manifest["verbatim_identifiers"]
                ),
                concurrent_rounds=concurrent_rounds,
                concurrent_searches_per_round=(
                    CONCURRENT_SEARCHES_PER_ROUND
                ),
                resume_delta_fixture_checkpoint=exercise_resume_delta,
            )
            if protocol == "simple"
            else None
        ),
    )
    plan_assertions = (
        retrieval_plan_assertions(
            db_container,
            target_path=target_path,
            query=discovery_key,
            retrieval_modes=retrieval_modes,
            semantic_lane_enabled=semantic_lane_enabled,
        )
        if db_container
        and protocol == "simple"
        and scale in {PRODUCTION_RECORDS, FUTURE_RECORDS}
        else {
            "status": "not_applicable",
            "pass": None,
            "reason": (
                "plan assertions run only for simple-core 64K/640K scales "
                "with --db-container"
            ),
        }
    )
    source_embedding_tokens = (
        sum(len(str(document["content"])) for document in documents) + 3
    ) // 4
    query_embedding_token_allowance = 100_000 if e03_arm == "mode3" else 0
    billable_embedding_tokens = (
        source_embedding_tokens + query_embedding_token_allowance
        if wait_for_semantic and e03_arm != "mode2"
        else 0
    )
    return {
        "scale": scale,
        "protocol": protocol,
        "documents": scale,
        "target_path": target_path,
        "marker": marker,
        "fixture_manifest": fixture_manifest,
        "verbatim_identifier": verbatim_probe,
        "e03_mode3_paired_query": mode3_paired_query,
        "e03_mode1_pending": mode1_pending_evidence,
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
        "resume_delta_fixture": resume_delta_fixture,
        "resume_delta_lineage_samples": resume_delta_lineage_samples,
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
        "query_counts": query_counts,
        "retrieval_plan_assertions": plan_assertions,
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
        "e09_semantic_coverage_probe": recursively_redact_secrets(
            semantic_coverage_probe
        ),
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
            "provider_billing": (
                "owned_mock_free" if e03_arm == "mode2"
                else "openai_usage_billed" if billable_embedding_tokens
                else "not_billable"
            ),
            "source_embedding_tokens": source_embedding_tokens,
            "query_embedding_token_allowance": query_embedding_token_allowance,
            "estimated_input_tokens": billable_embedding_tokens,
            "usd_per_million_tokens": 0.02,
            "estimated_usd": round(
                billable_embedding_tokens
                / 1_000_000
                * 0.02,
                6,
            )
            if billable_embedding_tokens else 0.0,
            "basis": (
                "ceil(source characters / 4) plus conservative paired/main/"
                "failure query allowance; provider receipt unavailable"
            ),
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


def verbatim_identifier_measurement_evidence(
    scale: dict[str, Any],
) -> dict[str, Any]:
    """Validate the 30-probe D02 measurement without accepting its outcome."""
    probe = scale.get("verbatim_identifier")
    probe = probe if isinstance(probe, dict) else {}
    fixture_manifest = scale.get("fixture_manifest")
    fixture_manifest = (
        fixture_manifest if isinstance(fixture_manifest, dict) else {}
    )
    planted = fixture_manifest.get("verbatim_identifiers")
    results = probe.get("results")
    planted_rows = planted if isinstance(planted, list) else []
    result_rows = results if isinstance(results, list) else []
    expected = probe.get("expected")
    returned = probe.get("returned")
    reported_pass = probe.get("pass")
    expected_is_exact = type(expected) is int and expected == VERBATIM_IDENTIFIER_PROBES
    returned_is_valid = (
        type(returned) is int
        and 0 <= returned <= VERBATIM_IDENTIFIER_PROBES
    )
    planted_is_exact = (
        len(planted_rows) == VERBATIM_IDENTIFIER_PROBES
        and all(isinstance(item, dict) for item in planted_rows)
    )
    results_are_exact = (
        len(result_rows) == VERBATIM_IDENTIFIER_PROBES
        and all(isinstance(item, dict) for item in result_rows)
    )
    row_identity_matches = planted_is_exact and results_are_exact and all(
        all(
            result.get(field) == fixture.get(field)
            for field in ("path", "identifier", "byte_offset")
        )
        for fixture, result in zip(planted_rows, result_rows)
    )
    modes_are_exact_only = results_are_exact and all(
        item.get("modes") == ["exact"] for item in result_rows
    )
    outcomes_are_boolean = results_are_exact and all(
        type(item.get("verbatim_in_source_payload")) is bool
        for item in result_rows
    )
    counted = (
        sum(
            item["verbatim_in_source_payload"]
            for item in result_rows
        )
        if outcomes_are_boolean
        else None
    )
    returned_matches_rows = returned_is_valid and counted == returned
    reported_pass_is_consistent = (
        type(reported_pass) is bool
        and returned_is_valid
        and reported_pass is (returned == VERBATIM_IDENTIFIER_PROBES)
    )
    checks = {
        "status_complete": probe.get("status") == "complete",
        "expected_is_exactly_30": expected_is_exact,
        "fixture_manifest_has_exactly_30_rows": planted_is_exact,
        "results_have_exactly_30_rows": results_are_exact,
        "row_identity_matches_fixture_manifest": row_identity_matches,
        "modes_are_exact_only": modes_are_exact_only,
        "outcomes_are_boolean": outcomes_are_boolean,
        "returned_is_typed_and_bounded": returned_is_valid,
        "returned_equals_counted_outcomes": returned_matches_rows,
        "reported_pass_matches_returned": reported_pass_is_consistent,
    }
    return {
        "scale": scale.get("scale"),
        "pass": all(checks.values()),
        "checks": checks,
        "returned": returned,
        "expected": expected,
        "reported_pass": reported_pass,
        "counted_true_outcomes": counted,
        "result_count": len(result_rows),
        "fixture_count": len(planted_rows),
    }


def evaluate_gates(
    scales: list[dict[str, Any]],
    thresholds: dict[str, float | int],
    *,
    required_scales: Sequence[int] = (),
    minimum_samples: int | None = None,
    semantic_failure_required: bool = False,
    semantic_failure_latency_required: bool = False,
    require_gin_index: bool = True,
    query_budgets: dict[str, Any] | None = None,
    verbatim_feature_acceptance_required: bool = True,
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
    simple_scales = [
        item for item in scales if item.get("protocol") == "simple"
    ]
    if simple_scales:
        gates.append((
            "query_count_sample_cardinality_is_authoritative",
            all(
                item.get("query_counts", {})
                .get("sample_cardinality", {})
                .get("authoritative") is True
                and item.get("query_counts", {})
                .get("sample_cardinality", {})
                .get("pass") is True
                for item in simple_scales
            ),
            [
                item.get("query_counts", {}).get("sample_cardinality")
                for item in simple_scales
            ],
            {
                "authoritative": True,
                "pass": True,
                "missing_response_samples": {},
                "extra_response_samples": {},
                "missing_query_count_samples": {},
            },
        ))
    if any(
        item.get("resume_delta_fixture", {}).get("requested") is True
        for item in simple_scales
    ):
        future_fixture = next(
            (
                item.get("resume_delta_fixture", {})
                for item in simple_scales
                if item.get("scale") == FUTURE_RECORDS
            ),
            {},
        )
        gates.append((
            "resume_delta_fixture_mutates_checkpoint_source_at_640000",
            (
                valid_resume_delta_fixture(future_fixture)
            ),
            future_fixture,
            {
                "scale": FUTURE_RECORDS,
                "checkpoint_source_entries": 1,
                "same_source_version_increment": 1,
                "post_checkpoint_content_hash_changed_and_read_verified": True,
                "expected_treatment_statement_delta": (
                    D03_RESUME_QUERY_COUNT_DELTA
                ),
            },
        ))
    query_count_scales = [
        item
        for item in simple_scales
        if "query_counts" in item
    ]
    if query_count_scales:
        if query_budgets is None:
            query_budgets = load_query_budgets()
        combined_counts: dict[str, list[int]] = {}
        combined_missing: dict[str, int] = {}
        budget_operations = set(query_budgets["operations"])
        for item in query_count_scales:
            summary = item["query_counts"]
            named_samples = summary.get("by_sample_name", {})
            missing_named = summary.get("missing_by_sample_name", {})
            for operation in budget_operations:
                observed = named_samples.get(operation, {})
                combined_counts.setdefault(operation, []).extend(
                    int(value) for value in observed.get("counts", [])
                )
                combined_missing[operation] = (
                    combined_missing.get(operation, 0)
                    + int(missing_named.get(operation, 0))
                )
        combined_summary = {
            "by_operation": {
                operation: {"counts": values}
                for operation, values in combined_counts.items()
            },
            "missing_by_operation": combined_missing,
        }
        gates.extend(
            (
                gate["name"],
                gate["pass"],
                gate["observed"],
                gate["threshold"],
            )
            for gate in evaluate_query_budgets(
                combined_summary,
                query_budgets,
            )
        )
    for item in scales:
        if (
            item.get("protocol") == "simple"
            and item["scale"] in {PRODUCTION_RECORDS, FUTURE_RECORDS}
        ):
            plan = item.get("retrieval_plan_assertions", {})
            gates.append((
                f"retrieval_plan_assertions_at_{item['scale']}",
                plan.get("status") == "complete" and plan.get("pass") is True,
                {
                    "status": plan.get("status", "missing"),
                    "sql_drift": plan.get("sql_drift"),
                    "lanes": {
                        lane: details.get("plan_assertion")
                        for lane, details in plan.get("lanes", {}).items()
                    },
                },
                {
                    "lexical": (
                        "Bitmap Index Scan using search_chunks_fts_idx"
                    ),
                    "semantic": (
                        "Index Scan using search_chunks_embedding_hnsw_idx"
                    ),
                    "forbidden": "Seq Scan on search_chunks",
                    "sql_drift": False,
                },
            ))
    for item in scales:
        if item["scale"] not in {PRODUCTION_RECORDS, FUTURE_RECORDS}:
            continue
        regression_observed = {
            "open_p95_ms": item.get("open_p95_ms"),
            "search_p95_ms": item.get("search_p95_ms"),
            "broad_search_p95_ms": item.get("broad_search_p95_ms"),
            "overflow_search_p95_ms": item.get("overflow_search_p95_ms"),
            "old_source_search_p95_ms": item.get("old_source_search_p95_ms"),
            "read_p95_ms": item.get("read_p95_ms"),
            "checkpoint_ms": item.get("checkpoint_ms"),
            "resume_ms": item.get("resume_ms"),
            "concurrent_write_p95_ms": item.get(
                "concurrent_probe",
                {},
            ).get("write_p95_ms"),
            "concurrent_search_p95_ms": item.get(
                "concurrent_probe",
                {},
            ).get("search_p95_ms"),
        }
        for metric, threshold in REGRESSION_THRESHOLDS.items():
            observed = regression_observed[metric]
            gates.append((
                f"regression_tier_{item['scale']}_{metric}",
                isinstance(observed, (int, float))
                and not isinstance(observed, bool)
                and float(observed) <= threshold,
                observed,
                threshold,
            ))
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
        if item.get("protocol") == "simple"
    ]
    if verbatim_scales:
        measurement_evidence = [
            verbatim_identifier_measurement_evidence(item)
            for item in verbatim_scales
        ]
        gates.append((
            "verbatim_identifier_measurement_integrity",
            all(item["pass"] for item in measurement_evidence),
            measurement_evidence,
            (
                "complete typed 30-row measurement whose planted identities, "
                "exact-only modes, counted outcomes, and reported result agree"
            ),
        ))
        if verbatim_feature_acceptance_required:
            gates.append((
                "verbatim_identifier",
                all(
                    item.get("verbatim_identifier", {}).get("pass") is True
                    for item in verbatim_scales
                ),
                [
                    {
                        "scale": item["scale"],
                        "returned": item.get(
                            "verbatim_identifier",
                            {},
                        ).get("returned"),
                        "expected": item.get(
                            "verbatim_identifier",
                            {},
                        ).get("expected"),
                    }
                    for item in verbatim_scales
                ],
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
        failure_latencies = probe.get("latencies_ms")
        failure_latencies = (
            failure_latencies if isinstance(failure_latencies, dict) else {}
        )
        required_latency_names = {
            "baseline_semantic",
            "injected_semantic",
            "injected_exact_lexical",
            "injected_mixed",
            "restored_semantic",
        }
        if semantic_failure_latency_required:
            gates.append((
                "semantic_failure_window_search_hard_slo",
                set(failure_latencies) == required_latency_names
                and all(
                    isinstance(value, (int, float))
                    and not isinstance(value, bool)
                    and 0 <= float(value) <= E03_FAILURE_WINDOW_SEARCH_SLO_MS
                    for value in failure_latencies.values()
                ),
                failure_latencies,
                {
                    "every_required_call_ms_lte": (
                        E03_FAILURE_WINDOW_SEARCH_SLO_MS
                    ),
                    "required_calls": sorted(required_latency_names),
                },
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


def apply_e03_gate_policy(
    scales: list[dict[str, Any]],
    gates: list[dict[str, Any]],
    *,
    arm: str,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Separate E03 applicability findings from its blocking acceptance gates."""
    if arm not in E03_ARMS:
        raise ValueError(f"unknown E03 arm {arm!r}")
    findings: list[dict[str, Any]] = [{
        "name": "verbatim_identifier_feature_acceptance",
        "blocking": False,
        "pass": all(
            item.get("verbatim_identifier", {}).get("pass") is True
            for item in scales
            if item.get("protocol") == "simple"
        ),
        "outcome": (
            "green"
            if all(
                item.get("verbatim_identifier", {}).get("pass") is True
                for item in scales
                if item.get("protocol") == "simple"
            )
            else "red"
        ),
        "observed": [
            {
                "scale": item.get("scale"),
                "returned": item.get("verbatim_identifier", {}).get("returned"),
                "expected": item.get("verbatim_identifier", {}).get("expected"),
                "reported_pass": item.get(
                    "verbatim_identifier",
                    {},
                ).get("pass"),
            }
            for item in scales
            if item.get("protocol") == "simple"
        ],
        "applicability": (
            "D02 feature acceptance is outside E03 acceptance and "
            "verbatim_spans is intentionally disabled; measurement integrity "
            "remains blocking"
        ),
    }]
    blocking = list(gates)
    if arm == "mode1":
        for gate in blocking:
            if (
                not str(gate.get("name", "")).startswith(
                    "retrieval_plan_assertions_at_"
                )
                or gate.get("pass") is True
            ):
                continue
            observed = gate.get("observed", {})
            lanes = (
                observed.get("lanes", {})
                if isinstance(observed, dict)
                else {}
            )
            lexical = lanes.get("lexical", {})
            semantic = lanes.get("semantic", {})
            drift = (
                observed.get("sql_drift")
                if isinstance(observed, dict)
                else None
            )
            scale = next(
                (
                    item
                    for item in scales
                    if gate.get("name")
                    == f"retrieval_plan_assertions_at_{item.get('scale')}"
                ),
                None,
            )
            pending = (
                scale.get("e03_mode1_pending", {})
                if isinstance(scale, dict)
                else {}
            )
            before_database = (
                pending.get("before_sampling", {}).get("database", {})
                if isinstance(pending, dict)
                else {}
            )
            after_database = (
                pending.get("after_sampling", {}).get("database", {})
                if isinstance(pending, dict)
                else {}
            )
            semantic_plan = (
                scale.get("retrieval_plan_assertions", {})
                .get("lanes", {})
                .get("semantic", {})
                if isinstance(scale, dict)
                else {}
            )
            expected_empty_cardinality_plan = (
                is_expected_mode1_empty_semantic_plan(
                    semantic_plan.get("function_owner_body_explain", [])
                    if isinstance(semantic_plan, dict)
                    else []
                )
            )
            zero_vector_cardinality_proven = (
                isinstance(pending, dict)
                and pending.get("pass") is True
                and all(
                    isinstance(snapshot, dict)
                    and isinstance(snapshot.get("chunks"), int)
                    and snapshot["chunks"] > 0
                    and snapshot.get("semantic_ready_chunks") == 0
                    and snapshot.get("pending_chunks") == snapshot["chunks"]
                    for snapshot in (before_database, after_database)
                )
            )
            mode1_plan_is_healthy = (
                isinstance(lexical, dict)
                and lexical.get("pass") is True
                and isinstance(semantic, dict)
                and semantic.get("pass") is False
                and semantic.get("lane") == "semantic"
                and semantic.get("expected") == {
                    "node_type": "Index Scan",
                    "index_name": "search_chunks_embedding_hnsw_idx",
                    "no_seq_scan_on": "search_chunks",
                }
                and semantic.get("matched") == []
                and semantic.get("forbidden") == []
                and isinstance(drift, list)
                and bool(drift)
                and all(
                    isinstance(item, dict) and item.get("pass") is True
                    for item in drift
                )
                and zero_vector_cardinality_proven
                and expected_empty_cardinality_plan
            )
            if not mode1_plan_is_healthy:
                continue
            gate["pass"] = True
            gate["observed"] = {
                **observed,
                "required_lanes": ["lexical"],
                "inapplicable_lanes": ["semantic"],
            }
            gate["threshold"] = {
                "lexical": (
                    "Bitmap Index Scan using search_chunks_fts_idx"
                ),
                "semantic": (
                    "not applicable: Mode 1 intentionally has zero "
                    "semantic-ready vectors"
                ),
                "forbidden": "Seq Scan on search_chunks",
                "sql_drift": False,
            }
            findings.append({
                "name": (
                    "mode1_semantic_plan_is_inapplicable_without_ready_vectors"
                ),
                "blocking": False,
                "pass": None,
                "outcome": "not_applicable",
                "observed": semantic,
                "applicability": (
                    "Mode 1 proves every chunk remains pending with zero "
                    "embeddings; the HNSW plan remains blocking in semantic-"
                    "ready Modes 2 and 3"
                ),
            })
        pending_evidence = [
            {
                "scale": item.get("scale"),
                "evidence": item.get("e03_mode1_pending"),
            }
            for item in scales
        ]
        blocking.append({
            "name": "mode1_pending_semantic_baseline_is_proven",
            "pass": all(
                isinstance(item["evidence"], dict)
                and item["evidence"].get("pass") is True
                for item in pending_evidence
            ),
            "observed": pending_evidence,
            "threshold": (
                "service-issued exact+lexical-ready corpus; zero embedded "
                "chunks and every chunk pending before/after; worker absent; "
                "hashing provider with no external credential; exact+lexical "
                "retrieval healthy; "
                "same API process"
            ),
        })
    if arm == "mode3":
        regression = [
            gate for gate in blocking
            if str(gate.get("name", "")).startswith("regression_tier_")
        ]
        blocking = [
            gate for gate in blocking
            if not str(gate.get("name", "")).startswith("regression_tier_")
        ]
        findings.extend({
            **gate,
            "blocking": False,
            "outcome": "green" if gate.get("pass") is True else "red",
            "applicability": (
                "D09 regression tiers block modes 1 and 2; Mode 3 provider "
                "latency is an E09 input while E03 hard SLOs still block"
            ),
        } for gate in regression)
        paired = max(scales, key=lambda item: item["scale"]).get(
            "e03_mode3_paired_query",
            {},
        )
        cold = paired.get("cold", {}) if isinstance(paired, dict) else {}
        warm = paired.get("warm", {}) if isinstance(paired, dict) else {}
        blocking.extend([
            {
                "name": "mode3_cold_warm_pair_is_single_corpus",
                "pass": bool(
                    isinstance(paired, dict)
                    and paired.get("status") == "complete"
                    and paired.get("pass") is True
                    and paired.get("single_provisioning_event") is True
                    and paired.get("cold_before_warm") is True
                    and paired.get("same_query_strings") is True
                    and paired.get("same_session_id") is True
                    and paired.get("cardinality", {}).get("stable") is True
                ),
                "observed": paired,
                "threshold": (
                    "one import/session/corpus; cold unique queries precede "
                    "identical warm repeats with stable index cardinality"
                ),
            },
            {
                "name": "mode3_cold_warm_search_hard_slo",
                "pass": all(
                    isinstance(phase.get("max_ms"), (int, float))
                    and not isinstance(phase.get("max_ms"), bool)
                    and float(phase["max_ms"])
                    <= E03_FAILURE_WINDOW_SEARCH_SLO_MS
                    for phase in (cold, warm)
                ),
                "observed": {
                    "cold_max_ms": cold.get("max_ms"),
                    "warm_max_ms": warm.get("max_ms"),
                },
                "threshold": E03_FAILURE_WINDOW_SEARCH_SLO_MS,
            },
        ])
        findings.append({
            "name": "mode3_cold_warm_search_regression_tier",
            "blocking": False,
            "pass": all(
                isinstance(phase.get("p95_ms"), (int, float))
                and not isinstance(phase.get("p95_ms"), bool)
                and float(phase["p95_ms"])
                <= REGRESSION_THRESHOLDS["search_p95_ms"]
                for phase in (cold, warm)
            ),
            "outcome": (
                "green"
                if all(
                    isinstance(phase.get("p95_ms"), (int, float))
                    and not isinstance(phase.get("p95_ms"), bool)
                    and float(phase["p95_ms"])
                    <= REGRESSION_THRESHOLDS["search_p95_ms"]
                    for phase in (cold, warm)
                )
                else "red"
            ),
            "observed": {
                "cold_p95_ms": cold.get("p95_ms"),
                "warm_p95_ms": warm.get("p95_ms"),
            },
            "threshold": REGRESSION_THRESHOLDS["search_p95_ms"],
            "applicability": (
                "nonblocking E09 input; the 3000ms hard SLO remains blocking"
            ),
        })
    return blocking, findings


def evaluate_lexical_consolidation_guards(
    scales: list[dict[str, Any]],
    *,
    required_scales: Sequence[int],
    minimum_samples: int,
) -> list[dict[str, Any]]:
    all_gates = evaluate_gates(
        scales,
        DEFAULT_THRESHOLDS,
        required_scales=required_scales,
        minimum_samples=minimum_samples,
        semantic_failure_required=False,
        require_gin_index=False,
    )
    selected = [
        gate
        for gate in all_gates
        if gate["name"] in LEXICAL_CONSOLIDATION_REQUIRED_GATES
    ]
    largest = max(scales, key=lambda item: item["scale"])
    concurrent = largest.get("concurrent_probe", {})
    write_p95 = concurrent.get("write_p95_ms", concurrent.get("write_ms"))
    selected.append({
        "name": "lexical_consolidation_unrelated_write_p95",
        "pass": isinstance(write_p95, (int, float)) and write_p95 <= 58.0,
        "observed": write_p95,
        "threshold": "<= 58.0ms (2x the v8 29.0ms baseline)",
    })
    return selected


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
    d03_control: dict[str, Any] | None = None
    e03_cost_preflight: dict[str, Any] | None = None
    e03_topology_before: dict[str, Any] | None = None
    e03_route_binding_before: dict[str, Any] | None = None
    try:
        feature_states = parse_feature_states(args.feature_state)
        expected_features = expected_runtime_features(
            args.expect_feature_flag,
            args.expect_runtime_config,
        )
        for name, expected in feature_states.items():
            if name in expected_features and expected_features[name] is not expected:
                raise ValueError(
                    f"conflicting expected feature states declared for {name}"
                )
            expected_features[name] = expected
        retrieval_modes = tuple(dict.fromkeys(args.retrieval_modes))
        profile = resolve_run_profile(args)
        if bool(args.semantic_failure_start_command) != bool(
            args.semantic_failure_stop_command
        ):
            raise ValueError(
                "semantic failure testing requires both the start and stop "
                "hook commands"
            )
        if profile.definitive and not args.expect_build_revision:
            raise ValueError(
                "definitive runs require --expect-build-revision"
            )
        if profile.definitive and not args.api_container:
            raise ValueError("definitive runs require --api-container")
        if (
            profile.definitive
            and args.protocol == "simple"
            and not args.db_container
        ):
            raise ValueError(
                "definitive simple-protocol runs require --db-container"
            )
        validate_e09_request_modes(args.e09_arm, retrieval_modes)
        validate_e03_request(args, retrieval_modes, expected_features)
        if args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE:
            estimated_tokens = 0
            if args.e03_arm == "mode3":
                for scale in profile.scales:
                    preflight_documents, _, _ = synthetic_documents(scale)
                    estimated_tokens += (
                        sum(
                            len(str(item["content"]))
                            for item in preflight_documents
                        )
                        + 3
                    ) // 4
                    del preflight_documents
                estimated_tokens += 100_000
            direct_estimate = round(
                estimated_tokens / 1_000_000 * 0.02,
                6,
            )
            estimated_max = (
                max(E03_MODE3_PREFLIGHT_MAX_USD, direct_estimate)
                if args.e03_arm == "mode3"
                else 0.0
            )
            e03_cost_preflight = {
                "reasoning_billing": "ChatGPT/Codex subscription only",
                "embedding_provider": (
                    "openai" if args.e03_arm == "mode3"
                    else "owned_mock" if args.e03_arm == "mode2"
                    else "hashing"
                ),
                "estimated_input_tokens": estimated_tokens,
                "direct_estimate_usd": direct_estimate,
                "estimated_max_usd": estimated_max,
                "ceiling_usd": E03_EMBEDDING_COST_CEILING_USD,
                "ceiling_pass": (
                    estimated_max <= E03_EMBEDDING_COST_CEILING_USD
                ),
                "user_notification_threshold_usd": 20.0,
                "notification_required": estimated_max > 20.0,
            }
            if not e03_cost_preflight["ceiling_pass"]:
                raise ValueError(
                    "E03 embedding preflight exceeds the strict $5 ceiling"
                )
            if not args.api_container or not args.db_container:
                raise ValueError("E03 requires API and DB containers")
            e03_topology_before = e03_container_topology(
                arm=args.e03_arm,
                api_container=args.api_container,
                db_container=args.db_container,
                worker_container=args.worker_container,
            )
        if (
            args.require_semantic_failure_hook_attestation
            and not args.semantic_failure_start_command
        ):
            raise ValueError(
                "required semantic-failure hook attestation needs both hook "
                "commands"
            )
        validate_lexical_consolidation_request(
            args,
            retrieval_modes,
            expected_features,
        )
        validate_resume_delta_fixture_request(args, retrieval_modes)
        if args.gate_profile == D03_RESUME_DELTAS_GATE_PROFILE:
            if (
                not args.future_soak
                or args.protocol != "simple"
                or list(retrieval_modes) != ["exact", "lexical"]
                or expected_features.get("resume_deltas") is not True
            ):
                raise ValueError(
                    "the D03 resume-deltas profile requires --future-soak, "
                    "--protocol simple, --retrieval-modes exact lexical, and "
                    "--expect-feature-flag resume_deltas=on"
                )
            if args.resume_control_from is None:
                raise ValueError(
                    "the D03 resume-deltas profile requires "
                    "--resume-control-from"
                )
            d03_control = load_d03_resume_control(args.resume_control_from)
        elif args.resume_control_from is not None:
            raise ValueError(
                "--resume-control-from requires "
                "--gate-profile d03-resume-deltas"
            )
        reused_flat_controls = load_reused_flat_controls(
            args.reuse_flat_controls_from,
            profile,
        )
        admin = NativeApiClient(timeout=profile.import_timeout_seconds)
        if args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE:
            e03_route_binding_before = e03_api_route_binding(
                api_base_url=admin.base_url,
                api_container=args.api_container,
                db_container=args.db_container,
            )
        runtime_status_before = admin.get("/v1/status").data
        if not isinstance(runtime_status_before, dict):
            raise ValueError("service status response was not an object")
        runtime_snapshot_before = capture_service_runtime_snapshot(
            runtime_status_before,
            expected_features=expected_features,
            expected_build_revision=args.expect_build_revision,
        )
        validate_e03_runtime_metadata(args.e03_arm, runtime_snapshot_before)
        semantic_failure_posture = validate_semantic_failure_probe_posture(
            posture=args.semantic_failure_probe,
            runtime_snapshot=runtime_snapshot_before,
            retrieval_modes=retrieval_modes,
            wait_for_semantic=args.wait_semantic,
            e09_arm=args.e09_arm,
            protocol=args.protocol,
            hooks_configured=bool(
                args.semantic_failure_start_command
                and args.semantic_failure_stop_command
            ),
        )
        verbatim_feature_acceptance_posture = (
            validate_verbatim_feature_acceptance_posture(
                posture=args.verbatim_feature_acceptance,
                runtime_snapshot=runtime_snapshot_before,
                expected_features=expected_features,
                protocol=args.protocol,
            )
        )
        query_budget_contract = resolve_query_budget_contract(
            profile=args.query_budget_profile,
            path=args.query_budget_contract,
            runtime_snapshot=runtime_snapshot_before,
            gate_profile=args.gate_profile,
            protocol=args.protocol,
            retrieval_modes=retrieval_modes,
        )
        fingerprint_before = implementation_fingerprint(
            args.api_container,
            args.worker_container,
        )
        if profile.definitive and not fingerprint_before["reproducible"]:
            raise ValueError(
                "definitive runs require a reproducible implementation "
                f"fingerprint: {fingerprint_before}"
            )
        if (
            args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE
            and (
                not args.expect_build_revision
                or fingerprint_before.get("source_revision")
                != args.expect_build_revision
                or fingerprint_before.get("api_image_revision")
                != args.expect_build_revision
                or (
                    args.e03_arm in {"mode2", "mode3"}
                    and fingerprint_before.get("worker_image_revision")
                    != args.expect_build_revision
                )
                or runtime_snapshot_before.get("build_revision")
                != args.expect_build_revision
            )
        ):
            raise ValueError(
                "E03 requires the exact expected full revision to match source, "
                "API image label, and authenticated runtime"
            )
        if d03_control is not None:
            validate_d03_resume_control_compatibility(
                d03_control,
                runtime_snapshot=runtime_snapshot_before,
                implementation=fingerprint_before,
                retrieval_modes=retrieval_modes,
            )
    except (NativeApiError, OSError, ValueError, json.JSONDecodeError) as error:
        return write_configuration_error(args, error)

    e09_runtime_before: dict[str, Any] | None = None
    e09_provenance: dict[str, Any] | None = None
    e09_step_authorization: dict[str, Any] | None = None
    if args.e09_arm:
        try:
            if args.protocol != "simple":
                raise ValueError("E09 arms require --protocol simple")
            if not fingerprint_before["reproducible"]:
                raise ValueError(
                    "E09 requires a reproducible implementation: "
                    f"{fingerprint_before}"
                )
            if args.e09_arm == "deadline_cache_600":
                if (
                    args.e09_step_policy is None
                    or args.e09_step_policy_sha256 is None
                ):
                    raise ValueError(
                        "E09 deadline_cache_600 requires --e09-step-policy "
                        "and --e09-step-policy-sha256"
                    )
                e09_step_authorization = load_step_authorization(
                    args.e09_step_policy,
                    args.e09_step_policy_sha256,
                    expected_source_revision=str(
                        fingerprint_before["source_revision"]
                    ),
                )
            elif (
                args.e09_step_policy is not None
                or args.e09_step_policy_sha256 is not None
            ):
                raise ValueError(
                    "step-policy arguments are valid only for "
                    "--e09-arm deadline_cache_600"
                )
            e09_runtime_before = runtime_status_before
            e09_provenance = validate_e09_runtime(
                e09_runtime_before,
                args.e09_arm,
                step_authorization=e09_step_authorization,
            )
            if (
                e09_provenance["build_revision"]
                != fingerprint_before["source_revision"]
            ):
                raise ValueError(
                    "E09 API build revision does not match the clean source "
                    "revision"
                )
        except (NativeApiError, ValueError) as error:
            return write_configuration_error(args, error)
    elif (
        args.e09_step_policy is not None
        or args.e09_step_policy_sha256 is not None
    ):
        return write_configuration_error(
            args,
            ValueError(
                "step-policy arguments require "
                "--e09-arm deadline_cache_600"
            ),
        )
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
                api_container=args.api_container,
                protocol=args.protocol,
                retrieval_modes=retrieval_modes,
                semantic_lane_enabled=runtime_snapshot_before[
                    "runtime_features"
                ]["semantic_lane"],
                run_semantic_failure=(
                    profile.semantic_failure_required
                    and scale == largest_requested_scale
                ),
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
                require_semantic_failure_hook_attestation=(
                    args.require_semantic_failure_hook_attestation
                ),
                wait_for_semantic=args.wait_semantic,
                unique_queries=args.unique_queries,
                e09_arm=args.e09_arm,
                e03_arm=args.e03_arm,
                run_e03_mode3_paired=(
                    args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE
                    and args.e03_arm == "mode3"
                    and scale == largest_requested_scale
                ),
                run_e03_mode1_pending=(
                    args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE
                    and args.e03_arm == "mode1"
                ),
                exercise_resume_delta_fixture=bool(
                    args.exercise_resume_delta_fixture
                ),
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
    if scales and args.gate_profile == LEXICAL_CONSOLIDATION_GATE_PROFILE:
        gates = evaluate_lexical_consolidation_guards(
            scales,
            required_scales=required_scales,
            minimum_samples=DEFINITIVE_SAMPLES,
        )
    elif scales:
        gates = evaluate_gates(
            scales,
            DEFAULT_THRESHOLDS,
            required_scales=required_scales,
            minimum_samples=(
                DEFINITIVE_SAMPLES if profile.definitive else None
            ),
            semantic_failure_required=profile.semantic_failure_required,
            semantic_failure_latency_required=(
                args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE
                and args.e03_arm in {"mode2", "mode3"}
            ),
            require_gin_index=profile.definitive,
            query_budgets=(
                query_budget_contract["contract"]
                if query_budget_contract is not None
                else None
            ),
            verbatim_feature_acceptance_required=(
                args.verbatim_feature_acceptance
                == VERBATIM_FEATURE_ACCEPTANCE_REQUIRED
            ),
        )
    else:
        gates = []
    nonblocking_findings: list[dict[str, Any]] = []
    if scales and args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE:
        gates, nonblocking_findings = apply_e03_gate_policy(
            scales,
            gates,
            arm=args.e03_arm,
        )
    e03_embedding_cost: dict[str, Any] | None = None
    if args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE:
        accounted_usd = round(sum(
            float(item.get("embedding_spend_estimate", {}).get(
                "estimated_usd",
                0.0,
            ))
            for item in scales
        ), 6)
        e03_embedding_cost = {
            "preflight": e03_cost_preflight,
            "accounted_estimate_usd": accounted_usd,
            "ceiling_usd": E03_EMBEDDING_COST_CEILING_USD,
            "mode2_mock_billing_is_zero": (
                args.e03_arm != "mode2" or accounted_usd == 0.0
            ),
        }
        gates.append({
            "name": "e03_embedding_cost_ceiling",
            "pass": bool(
                e03_cost_preflight
                and e03_cost_preflight.get("ceiling_pass") is True
                and accounted_usd <= E03_EMBEDDING_COST_CEILING_USD
                and e03_embedding_cost["mode2_mock_billing_is_zero"]
            ),
            "observed": e03_embedding_cost,
            "threshold": f"<= ${E03_EMBEDDING_COST_CEILING_USD:.2f}",
        })
    fingerprint = implementation_fingerprint(
        args.api_container,
        args.worker_container,
    )
    e03_topology_after: dict[str, Any] | None = None
    e03_route_binding_after: dict[str, Any] | None = None
    if args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE:
        stable_fingerprint_fields = (
            "source_revision",
            "api_container_id",
            "api_container_started_at",
            "api_image_id",
            "api_image_revision",
            "worker_container_id",
            "worker_container_started_at",
            "worker_running",
            "worker_image_id",
            "worker_image_revision",
        )
        drift = {
            field: {
                "before": fingerprint_before.get(field),
                "after": fingerprint.get(field),
            }
            for field in stable_fingerprint_fields
            if fingerprint_before.get(field) != fingerprint.get(field)
        }
        if drift:
            errors.append({
                "type": "ProvenanceDrift",
                "message": f"E03 API/source provenance drifted: {drift}",
                "stage": "implementation_fingerprint_after",
            })
        try:
            e03_topology_after = e03_container_topology(
                arm=args.e03_arm,
                api_container=args.api_container,
                db_container=args.db_container,
                worker_container=args.worker_container,
            )
            if e03_topology_after != e03_topology_before:
                raise ValueError(
                    "E03 API/DB/worker topology drifted during the run"
                )
        except ValueError as error:
            errors.append({
                "type": type(error).__name__,
                "message": str(error),
                "stage": "container_topology_after",
            })
        try:
            e03_route_binding_after = e03_api_route_binding(
                api_base_url=admin.base_url,
                api_container=args.api_container,
                db_container=args.db_container,
            )
            if e03_route_binding_after != e03_route_binding_before:
                raise ValueError(
                    "E03 API/database route drifted during the run"
                )
        except ValueError as error:
            errors.append({
                "type": type(error).__name__,
                "message": str(error),
                "stage": "api_route_binding_after",
            })
        gates.append({
            "name": "e03_container_topology_is_stable",
            "pass": bool(
                e03_topology_before
                and e03_topology_after == e03_topology_before
            ),
            "observed": {
                "before": e03_topology_before,
                "after": e03_topology_after,
            },
            "threshold": (
                "same isolated Compose project and API/DB/worker container "
                "identities before and after"
            ),
        })
        gates.append({
            "name": "e03_api_database_route_is_stable",
            "pass": bool(
                e03_route_binding_before
                and e03_route_binding_after == e03_route_binding_before
            ),
            "observed": {
                "before": e03_route_binding_before,
                "after": e03_route_binding_after,
            },
            "threshold": (
                "ambient API URL is the named container's exact loopback "
                "publish and that API targets the named DB on one network"
            ),
        })
    runtime_status_after: dict[str, Any] | None = None
    runtime_snapshot_after: dict[str, Any] | None = None
    try:
        runtime_status_after = admin.get("/v1/status").data
        if not isinstance(runtime_status_after, dict):
            raise ValueError("service status response was not an object")
        runtime_snapshot_after = capture_service_runtime_snapshot(
            runtime_status_after,
            expected_features=expected_features,
            expected_build_revision=args.expect_build_revision,
        )
        validate_e03_runtime_metadata(args.e03_arm, runtime_snapshot_after)
        require_stable_runtime_configuration(
            runtime_snapshot_before,
            runtime_snapshot_after,
        )
    except (NativeApiError, ValueError) as error:
        errors.append({
            "type": type(error).__name__,
            "message": str(error),
            "stage": "runtime_configuration_after",
        })
    e09_runtime: dict[str, Any] | None = None
    if (
        args.e09_arm
        and e09_runtime_before is not None
        and runtime_status_after is not None
    ):
        try:
            final_provenance = validate_e09_runtime(
                runtime_status_after,
                args.e09_arm,
                step_authorization=e09_step_authorization,
            )
            if final_provenance != e09_provenance:
                raise ValueError(
                    "E09 runtime flags or build revision drifted during the run"
                )
            delta = semantic_counter_delta(
                e09_runtime_before.get("semantic_runtime", {}),
                runtime_status_after.get("semantic_runtime", {}),
            )
            e09_runtime = {
                "provenance": final_provenance,
                "counters_before": e09_runtime_before.get(
                    "semantic_runtime",
                    {},
                ),
                "counters_after": runtime_status_after.get(
                    "semantic_runtime",
                    {},
                ),
                "counter_delta": delta,
                "rates": semantic_rates(delta),
            }
        except (NativeApiError, ValueError) as error:
            errors.append({
                "type": type(error).__name__,
                "message": str(error),
                "stage": "e09_runtime_provenance",
            })
    d03_resume_delta: dict[str, Any] | None = None
    if args.gate_profile == D03_RESUME_DELTAS_GATE_PROFILE:
        assert d03_control is not None
        d03_gates, d03_resume_delta = evaluate_d03_resume_delta_gates(
            scales,
            d03_control,
        )
        gates.extend(d03_gates)
    if args.query_budget_profile == "calibration":
        gates.append({
            "name": "query_budget_calibration_is_not_acceptance",
            "pass": False,
            "observed": {
                "profile": "calibration",
                "query_counts_recorded": bool(
                    any(item.get("query_counts") for item in scales)
                ),
            },
            "threshold": (
                "author and review a runtime-bound query-budget contract, then "
                "rerun with that named profile"
            ),
        })
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
        "schema": "brunn-performance-eval@v2",
        "created_at": datetime.now().astimezone().isoformat(),
        "label": args.label,
        "protocol": args.protocol,
        "gate_profile": args.gate_profile,
        "e03_arm": args.e03_arm,
        "retrieval_modes": list(retrieval_modes),
        "declared_feature_states": feature_states,
        "expected_runtime_features": expected_features,
        "expected_build_revision": args.expect_build_revision,
        "semantic_failure_probe_posture": semantic_failure_posture,
        "verbatim_feature_acceptance_posture": (
            verbatim_feature_acceptance_posture
        ),
        "runtime_configuration": {
            "before": runtime_snapshot_before,
            "after": runtime_snapshot_after,
            "stable": (
                runtime_snapshot_after is not None
                and runtime_snapshot_before.get("build_revision")
                == runtime_snapshot_after.get("build_revision")
                and runtime_snapshot_before.get("runtime_features")
                == runtime_snapshot_after.get("runtime_features")
                and runtime_snapshot_before.get("embeddings")
                == runtime_snapshot_after.get("embeddings")
            ),
        },
        "run_tags": sorted(set(args.run_tag)),
        "api_url": admin.base_url,
        "implementation_fingerprint": fingerprint,
        "e09_runtime": e09_runtime,
        "d03_resume_delta": d03_resume_delta,
        "e09_step_authorization": e09_step_authorization,
        "e03_embedding_cost": e03_embedding_cost,
        "e03_container_topology": {
            "before": e03_topology_before,
            "after": e03_topology_after,
            "stable": (
                e03_topology_before is not None
                and e03_topology_after == e03_topology_before
            ),
        } if args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE else None,
        "e03_api_route_binding": {
            "before": e03_route_binding_before,
            "after": e03_route_binding_after,
            "stable": (
                e03_route_binding_before is not None
                and e03_route_binding_after == e03_route_binding_before
            ),
        } if args.gate_profile == E03_SEMANTIC_READY_GATE_PROFILE else None,
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
            "semantic_failure_probe": semantic_failure_posture,
            "semantic_failure_hooks_configured": bool(
                args.semantic_failure_start_command
                and args.semantic_failure_stop_command
            ),
            "semantic_failure_hook_attestation_required": bool(
                args.require_semantic_failure_hook_attestation
            ),
            "wait_for_semantic": bool(args.wait_semantic),
            "unique_queries": bool(args.unique_queries),
            "exercise_resume_delta_fixture": bool(
                args.exercise_resume_delta_fixture
            ),
            "import_timeout_seconds": profile.import_timeout_seconds,
        },
        "production_reference_records": PRODUCTION_RECORDS,
        "future_reference_records": FUTURE_RECORDS,
        "scales": scales,
        "thresholds": DEFAULT_THRESHOLDS,
        "regression_thresholds": REGRESSION_THRESHOLDS,
        "query_budget_contract": query_budget_contract,
        "retrieval_plan_contract": {
            "path": str(
                RETRIEVAL_PLAN_CONTRACT_PATH.relative_to(PROJECT_ROOT)
            ),
            "sha256": hashlib.sha256(
                RETRIEVAL_PLAN_CONTRACT_PATH.read_bytes()
            ).hexdigest(),
        },
        "gates": gates,
        "nonblocking_findings": nonblocking_findings,
        "errors": errors,
        "pass": bool(scales and not errors and all(gate["pass"] for gate in gates)),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result["pass"] else 2


def write_configuration_error(
    args: argparse.Namespace,
    error: Exception,
) -> int:
    semantic_failure_posture = {
        "posture": getattr(
            args,
            "semantic_failure_probe",
            SEMANTIC_FAILURE_PROBE_REQUIRED,
        ),
        "eligible": False,
        "reason": "configuration preflight failed before eligibility proof",
    }
    result = {
        "schema": "brunn-performance-eval@v2",
        "created_at": datetime.now().astimezone().isoformat(),
        "label": args.label,
        "pass": False,
        "semantic_failure_probe": semantic_failure_posture,
        "semantic_failure_probe_posture": semantic_failure_posture,
        "expected_build_revision": getattr(args, "expect_build_revision", None),
        "query_budget_profile": getattr(
            args,
            "query_budget_profile",
            DEFAULT_QUERY_BUDGET_PROFILE,
        ),
        "verbatim_feature_acceptance": getattr(
            args,
            "verbatim_feature_acceptance",
            VERBATIM_FEATURE_ACCEPTANCE_REQUIRED,
        ),
        "errors": [{
            "type": "ConfigurationError",
            "message": str(error),
        }],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 2


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
        description="Production-shaped Brunn retrieval and write-amplification benchmark",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate")
    validate.add_argument("--scale", type=int, default=10_000)
    validate.set_defaults(function=command_validate)

    run = subparsers.add_parser("run")
    run.add_argument("--label", required=True)
    run.add_argument(
        "--gate-profile",
        choices=(
            D03_RESUME_DELTAS_GATE_PROFILE,
            E03_SEMANTIC_READY_GATE_PROFILE,
            LEXICAL_CONSOLIDATION_GATE_PROFILE,
        ),
        help="run an experiment-specific deterministic gate subset",
    )
    run.add_argument(
        "--e03-arm",
        choices=E03_ARMS,
        help=(
            "explicit E03 same-build arm; mode3 performs one paired cold/warm "
            "query probe after a single corpus import"
        ),
    )
    run.add_argument(
        "--feature-state",
        action="append",
        default=[],
        metavar="NAME=on|off",
        help=(
            "deprecated alias for --expect-feature-flag; fail closed unless "
            "the authenticated runtime matches NAME=on|off"
        ),
    )
    run.add_argument(
        "--expect-feature-flag",
        action="append",
        default=[],
        metavar="NAME=on|off",
        help=(
            "fail closed unless /v1/status runtime_features matches NAME=on|off"
        ),
    )
    run.add_argument(
        "--expect-runtime-config",
        action="append",
        default=[],
        metavar="NAME=JSON",
        help=(
            "fail closed unless /v1/status runtime_features matches "
            "NAME=<JSON value>"
        ),
    )
    run.add_argument(
        "--expect-build-revision",
        help=(
            "fail closed unless the authenticated API build revision matches; "
            "required for definitive runs"
        ),
    )
    run.add_argument(
        "--run-tag",
        action="append",
        default=[],
        help="attach a stable experiment tag to the result artifact",
    )
    run.add_argument(
        "--retrieval-modes",
        nargs="+",
        choices=("exact", "lexical", "semantic"),
        default=["exact", "lexical", "semantic"],
        help="retrieval lanes requested by benchmark search operations",
    )
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
            "scales and fewer samples while retaining fail-closed semantic "
            "probe posture"
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
            "seconds allowed for fixture import/index readiness; E03 semantic "
            "arms default to the documented 12-hour stall boundary, otherwise "
            "1800, or 7200 with --future-soak"
        ),
    )
    run.add_argument("--db-container")
    run.add_argument(
        "--worker-container",
        help=(
            "worker container required by E03 modes 2/3; its running process, "
            "image ID, and revision are bound into provenance"
        ),
    )
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
        "--e09-arm",
        choices=(
            "no_semantic",
            "unbounded_semantic",
            "deadline_cache",
            "deadline_cache_600",
        ),
        help=(
            "fail-closed E09 arm selection; verifies clean source, API image "
            "revision, runtime flags, and semantic cache/deferral counters"
        ),
    )
    run.add_argument(
        "--e09-step-policy",
        type=Path,
        help=(
            "immutable approved E09 step-policy artifact; required only for "
            "deadline_cache_600"
        ),
    )
    run.add_argument(
        "--e09-step-policy-sha256",
        help=(
            "expected SHA-256 of --e09-step-policy; required only for "
            "deadline_cache_600"
        ),
    )
    run.add_argument(
        "--semantic-failure-probe",
        choices=(
            SEMANTIC_FAILURE_PROBE_REQUIRED,
            SEMANTIC_FAILURE_PROBE_NOT_APPLICABLE,
        ),
        default=SEMANTIC_FAILURE_PROBE_REQUIRED,
        help=(
            "required by default; not-applicable is accepted only for an "
            "authenticated semantic-disabled exact/lexical runtime"
        ),
    )
    run.add_argument(
        "--verbatim-feature-acceptance",
        choices=(
            VERBATIM_FEATURE_ACCEPTANCE_REQUIRED,
            VERBATIM_FEATURE_ACCEPTANCE_NOT_APPLICABLE,
        ),
        default=VERBATIM_FEATURE_ACCEPTANCE_REQUIRED,
        help=(
            "required by default; not-applicable is accepted only for a "
            "simple-protocol run that explicitly authenticates "
            "verbatim_spans=off, while measurement integrity stays blocking"
        ),
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
        "--require-semantic-failure-hook-attestation",
        action="store_true",
        help=(
            "require checked-in hook fingerprints plus a fault-proxy "
            "attestation for injected error and restored forward modes"
        ),
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
            "rerunning every Brunn measurement; provenance is recorded"
        ),
    )
    run.add_argument(
        "--resume-control-from",
        type=Path,
        help=(
            "passing resume_deltas=off definitive artifact paired with the "
            "d03-resume-deltas treatment"
        ),
    )
    run.add_argument(
        "--exercise-resume-delta-fixture",
        action="store_true",
        help=(
            "at the 640K scale, checkpoint the target source and mutate that "
            "same source before resume sampling; required by D03 treatment "
            "and its matched control"
        ),
    )
    run.add_argument(
        "--query-budget-profile",
        default=DEFAULT_QUERY_BUDGET_PROFILE,
        help=(
            "named query-budget applicability profile; non-default and launch "
            "profiles require an explicit contract"
        ),
    )
    run.add_argument(
        "--query-budget-contract",
        type=Path,
        help=(
            "profile-specific query-budget JSON; mandatory for launch and "
            "other non-default profiles"
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

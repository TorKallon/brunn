"""Fail-closed arm selection and provenance helpers for E09.

This module contains no provider calls. It only validates the runtime feature
state reported by a Straylight service and computes counter deltas.
"""

from __future__ import annotations

from typing import Any, Mapping


E09_ARM_SETTINGS: dict[str, dict[str, Any]] = {
    "no_semantic": {
        "semantic_lane": False,
        "embed_cache": True,
        "semantic_deadline_ms": 300,
        "embedding_backfill_guard": True,
    },
    "unbounded_semantic": {
        "semantic_lane": True,
        "embed_cache": False,
        "semantic_deadline_ms": None,
        "embedding_backfill_guard": True,
    },
    "deadline_cache": {
        "semantic_lane": True,
        "embed_cache": True,
        "semantic_deadline_ms": 300,
        "embedding_backfill_guard": True,
    },
}

SEMANTIC_COUNTERS = (
    "requested",
    "disabled",
    "cache_hits",
    "cache_misses",
    "negative_cache_hits",
    "cache_bypasses",
    "successes",
    "failures",
    "deferrals",
)


def response_has_candidates(value: Any) -> bool:
    """Return true when a workspace response contains at least one candidate."""
    if isinstance(value, Mapping):
        candidates = value.get("candidates")
        if isinstance(candidates, list) and candidates:
            return True
        return any(response_has_candidates(item) for item in value.values())
    if isinstance(value, list):
        return any(response_has_candidates(item) for item in value)
    return False


def expected_e09_features(arm: str) -> dict[str, Any]:
    try:
        return dict(E09_ARM_SETTINGS[arm])
    except KeyError as exc:
        raise ValueError(f"unknown E09 arm: {arm}") from exc


def enforced_retrieval_modes(arm: str | None) -> tuple[str, ...]:
    if arm is None:
        return ()
    expected_e09_features(arm)
    return ("exact", "lexical") if arm == "no_semantic" else ()


def validate_e09_runtime(
    status: Mapping[str, Any],
    arm: str,
) -> dict[str, Any]:
    expected = expected_e09_features(arm)
    features = status.get("runtime_features")
    if not isinstance(features, Mapping):
        raise ValueError(
            "E09 requires /v1/status runtime_features provenance from the "
            "instrumented API build"
        )
    mismatches = {
        key: {"expected": value, "actual": features.get(key)}
        for key, value in expected.items()
        if features.get(key) != value
    }
    if mismatches:
        raise ValueError(
            f"E09 {arm} runtime flag mismatch: {mismatches}"
        )
    build_revision = status.get("build_revision")
    if not isinstance(build_revision, str) or not build_revision.strip():
        raise ValueError("E09 requires a non-empty API build_revision")
    return {
        "arm": arm,
        "build_revision": build_revision,
        "runtime_features": dict(features),
        "expected_features": expected,
        "enforced_request_modes": list(enforced_retrieval_modes(arm)),
    }


def semantic_counter_delta(
    before: Mapping[str, Any],
    after: Mapping[str, Any],
) -> dict[str, int]:
    delta: dict[str, int] = {}
    for key in SEMANTIC_COUNTERS:
        start = before.get(key, 0)
        end = after.get(key, 0)
        if not isinstance(start, int) or not isinstance(end, int):
            raise ValueError(f"semantic counter {key} must be an integer")
        if end < start:
            raise ValueError(
                f"semantic counter {key} moved backwards ({start} -> {end})"
            )
        delta[key] = end - start
    return delta


def semantic_rates(delta: Mapping[str, int]) -> dict[str, float | None]:
    requested = int(delta.get("requested", 0))
    cache_lookups = (
        int(delta.get("cache_hits", 0))
        + int(delta.get("cache_misses", 0))
        + int(delta.get("negative_cache_hits", 0))
    )
    return {
        "cache_hit_rate": (
            int(delta.get("cache_hits", 0)) / cache_lookups
            if cache_lookups
            else None
        ),
        "deferral_rate": (
            int(delta.get("deferrals", 0)) / requested
            if requested
            else None
        ),
    }

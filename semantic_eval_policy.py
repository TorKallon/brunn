"""Fail-closed arm selection and provenance helpers for E09.

This module contains no provider calls. It only validates the runtime feature
state reported by a Straylight service and computes counter deltas.
"""

from __future__ import annotations

import re
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
    "deadline_cache_600": {
        "semantic_lane": True,
        "embed_cache": True,
        "semantic_deadline_ms": 600,
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
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


def validate_step_binding(
    arm: str,
    step_authorization: Mapping[str, Any] | None,
) -> str | None:
    if arm != "deadline_cache_600":
        if step_authorization is not None:
            raise ValueError(
                "E09 step authorization is valid only for deadline_cache_600"
            )
        return None
    authorization_id = (
        step_authorization.get("authorization_id")
        if isinstance(step_authorization, Mapping)
        else None
    )
    if (
        not isinstance(step_authorization, Mapping)
        or step_authorization.get("schema")
        != "straylight-e09-step-authorization-binding@v1"
        or step_authorization.get("deadline_before_ms") != 300
        or step_authorization.get("deadline_after_ms") != 600
        or step_authorization.get("automatic_1000ms_step_allowed") is not False
        or not isinstance(authorization_id, str)
        or not SHA256_PATTERN.fullmatch(authorization_id)
        or not isinstance(step_authorization.get("artifact_sha256"), str)
        or not SHA256_PATTERN.fullmatch(
            str(step_authorization["artifact_sha256"])
        )
    ):
        raise ValueError(
            "E09 deadline_cache_600 requires a checked step-policy binding"
        )
    return authorization_id


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


def expected_e09_features(
    arm: str,
    *,
    step_authorization: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    validate_step_binding(arm, step_authorization)
    try:
        return dict(E09_ARM_SETTINGS[arm])
    except KeyError as exc:
        raise ValueError(f"unknown E09 arm: {arm}") from exc


def enforced_retrieval_modes(
    arm: str | None,
    *,
    step_authorization: Mapping[str, Any] | None = None,
) -> tuple[str, ...]:
    if arm is None:
        if step_authorization is not None:
            raise ValueError(
                "E09 step authorization requires an E09 arm"
            )
        return ()
    expected_e09_features(
        arm,
        step_authorization=step_authorization,
    )
    return ("exact", "lexical") if arm == "no_semantic" else ()


def validate_e09_runtime(
    status: Mapping[str, Any],
    arm: str,
    *,
    step_authorization: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    authorization_id = validate_step_binding(arm, step_authorization)
    expected = expected_e09_features(
        arm,
        step_authorization=step_authorization,
    )
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
        "enforced_request_modes": list(enforced_retrieval_modes(
            arm,
            step_authorization=step_authorization,
        )),
        "step_authorization_id": authorization_id,
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

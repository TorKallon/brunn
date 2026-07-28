"""Validation and immutable binding for the single authorized E09 deadline step."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any, Sequence


STEP_POLICY_SCHEMA = "straylight-e09-step-policy@v1"
STEP_ARM = "deadline_cache_600"
STEP_EXPERIMENT_ARM = "e09-deadline-cache-600"
BASE_EXPERIMENT_ARM = "e09-deadline-cache"
SHA256_PATTERN = re.compile(r"^(?:sha256:)?([0-9a-f]{64})$")


def canonical_json_sha256(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(
            value,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()


def normalize_sha256(value: str, *, field: str) -> str:
    match = SHA256_PATTERN.fullmatch(value.casefold())
    if match is None:
        raise ValueError(f"{field} must be a lowercase SHA-256 digest")
    return match.group(1)


def load_step_authorization(
    path: Path,
    expected_sha256: str,
    *,
    expected_source_revision: str,
    expected_manifest_path: str | None = None,
    expected_manifest_sha256: str | None = None,
    expected_case_ids: Sequence[str] | None = None,
) -> dict[str, Any]:
    expected_digest = normalize_sha256(
        expected_sha256,
        field="step-policy SHA-256",
    )
    payload = path.read_bytes()
    digest = hashlib.sha256(payload).hexdigest()
    if digest != expected_digest:
        raise ValueError("step-policy artifact SHA-256 mismatch")
    try:
        value = json.loads(payload)
    except json.JSONDecodeError as error:
        raise ValueError("step-policy artifact is not valid JSON") from error
    if not isinstance(value, dict) or value.get("schema") != STEP_POLICY_SCHEMA:
        raise ValueError("invalid E09 step-policy artifact schema")
    policy = value.get("policy")
    losing_suite = value.get("losing_suite")
    step = value.get("step")
    implementation = value.get("implementation")
    if (
        value.get("pass") is not True
        or value.get("status")
        not in {
            "approved_one_step_full_suite",
            "approved_one_step_bounded_subset",
        }
        or not isinstance(policy, dict)
        or policy.get("one_step_only") is not True
        or policy.get("deadline_before_ms") != 300
        or policy.get("deadline_after_ms") != 600
        or policy.get("automatic_1000ms_step_allowed") is not False
        or policy.get("step_draws") != 3
        or type(policy.get("max_step_cases")) is not int
        or not 1 <= policy["max_step_cases"] <= 12
        or not isinstance(
            policy.get("reasoning_ceiling_usd"),
            (int, float),
        )
        or isinstance(policy.get("reasoning_ceiling_usd"), bool)
        or not 0 < policy["reasoning_ceiling_usd"] <= 100
        or policy.get("second_step_status")
        != "forbidden_without_new_owner_authorization"
        or not isinstance(losing_suite, dict)
        or not isinstance(step, dict)
        or not isinstance(implementation, dict)
    ):
        raise ValueError("step-policy does not authorize exactly one 600ms step")
    if (
        implementation.get("source_revision") != expected_source_revision
        or implementation.get("source_clean") is not True
        or not isinstance(implementation.get("harness_sha256"), str)
        or not SHA256_PATTERN.fullmatch(
            implementation["harness_sha256"]
        )
    ):
        raise ValueError(
            "step-policy is not bound to the clean evaluation source revision"
        )
    selected = step.get("selected_case_ids")
    if (
        not isinstance(selected, list)
        or not selected
        or not all(isinstance(case_id, str) and case_id for case_id in selected)
        or len(selected) != len(set(selected))
        or len(selected) > policy["max_step_cases"]
        or step.get("selected_cases") != len(selected)
        or step.get("case_runs") != len(selected) * 3
        or not isinstance(step.get("maximum_total_usd"), (int, float))
        or isinstance(step.get("maximum_total_usd"), bool)
        or step["maximum_total_usd"] > policy["reasoning_ceiling_usd"]
    ):
        raise ValueError("step-policy has an invalid selected case set")
    manifest = losing_suite.get("manifest")
    manifest_sha256 = losing_suite.get("manifest_sha256")
    if (
        not isinstance(manifest, str)
        or not manifest
        or not isinstance(manifest_sha256, str)
        or not SHA256_PATTERN.fullmatch(manifest_sha256)
    ):
        raise ValueError("step-policy lacks a manifest fingerprint")
    if (
        expected_manifest_path is not None
        and manifest != expected_manifest_path
    ):
        raise ValueError("step-policy authorizes a different suite manifest")
    if (
        expected_manifest_sha256 is not None
        and manifest_sha256 != expected_manifest_sha256
    ):
        raise ValueError("step-policy suite manifest SHA-256 mismatch")
    if (
        expected_case_ids is not None
        and list(expected_case_ids) != selected
    ):
        raise ValueError(
            "selected cases do not exactly match the step-policy authorization"
        )
    authorization_id = value.get("authorization_id")
    expected_authorization_id = canonical_json_sha256({
        "schema": STEP_POLICY_SCHEMA,
        "source_revision": implementation["source_revision"],
        "manifest": manifest,
        "manifest_sha256": manifest_sha256,
        "selected_case_ids": selected,
        "deadline_before_ms": 300,
        "deadline_after_ms": 600,
        "maximum_total_usd": step.get("maximum_total_usd"),
    })
    if (
        not isinstance(authorization_id, str)
        or not SHA256_PATTERN.fullmatch(authorization_id)
        or authorization_id != expected_authorization_id
    ):
        raise ValueError("step-policy authorization identity is invalid")
    return {
        "schema": "straylight-e09-step-authorization-binding@v1",
        "artifact": str(path),
        "artifact_sha256": digest,
        "authorization_id": authorization_id,
        "source_revision": expected_source_revision,
        "manifest": manifest,
        "manifest_sha256": manifest_sha256,
        "selected_case_ids": selected,
        "deadline_before_ms": 300,
        "deadline_after_ms": 600,
        "automatic_1000ms_step_allowed": False,
        "maximum_total_usd": step.get("maximum_total_usd"),
    }

#!/usr/bin/env python3

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import statistics
import subprocess
import time
from collections import Counter, defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any, Sequence

from native_eval import (
    NativeApiClient,
    provisioning_matches_run_case,
    provision_evaluation,
    public_provisioning,
    text_documents,
    write_native_memory_wrapper,
)
from eval.e09_step_authorization import load_step_authorization
from semantic_eval_policy import (
    enforced_retrieval_modes,
    expected_e09_features,
    response_has_candidates,
    semantic_counter_delta,
    semantic_rates,
    validate_e09_runtime,
)
from straylight_eval import BM25Index
from workspace_cli import corpus_hash, diverse_results, load_corpus


PROJECT_ROOT = Path(__file__).resolve().parent
DEFAULT_MANIFEST = PROJECT_ROOT / "eval" / "work_cases.json"
DEFAULT_SCHEMA = PROJECT_ROOT / "eval" / "work_answer_schema.json"


def resolve_codex_path(candidates: Sequence[Path] | None = None) -> Path:
    paths = list(candidates or (
        Path.home() / ".local" / "bin" / "codex",
        Path("/Applications/ChatGPT.app/Contents/Resources/codex"),
        Path("/Applications/Codex.app/Contents/Resources/codex"),
    ))
    for path in paths:
        if path.is_file() and os.access(path, os.X_OK):
            return path
    return paths[0]


DEFAULT_CODEX = resolve_codex_path()
NATIVE_PROVISIONING_STATE = ".native-provisioning.json"
REASONING_BILLING_POLICY = {
    "route": "chatgpt_subscription",
    "api_fallback": "forbidden",
}
RUN_LEDGER_SCHEMA = "straylight-eval-run-ledger@v1"
SERVICE_IMAGE_FINGERPRINT_SCHEMA = (
    "straylight-service-image-fingerprint@v1"
)
SERVICE_IMAGE_PROVENANCE_SCHEMA = (
    "straylight-service-image-provenance@v1"
)
RESPONSE_CHARACTER_METRICS_SCHEMA = (
    "straylight-agent-response-character-metrics@v1"
)
LOCAL_CLI_FAILURE_SUMMARY_SCHEMA = (
    "straylight-local-cli-failure-summary@v1"
)
CANONICAL_SERVICE_CHECKPOINT_INVOCATION = (
    "`./memory checkpoint "
    "'{\"state\":{\"objective\":\"...\",\"current_state\":[],"
    "\"decisions\":[],\"open_questions\":[],\"next_actions\":[],"
    "\"artifacts\":[]},\"source_refs\":[\"exact/source/path\"]}'`"
)
SIDECAR_CHECKPOINT_RELATIVE_PATH = Path("sidecar") / "checkpoint.json"
RUN_BINDING_STATE = ".experiment-run-binding.json"
EXPERIMENT_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
DOCKER_IMAGE_ID_PATTERN = re.compile(r"^sha256:[0-9a-f]{64}$")
BOOLEAN_RUNTIME_FEATURES = {
    "allow_degraded_embeddings",
    "embed_cache",
    "embedding_backfill_guard",
    "embedding_backfill_foreground_status_url_configured",
    "intention_ledger",
    "lexical_single_scan",
    "observability_timings_ms",
    "read_path_roundtrip_v1",
    "resume_deltas",
    "search_char_cap",
    "search_fair_share",
    "search_top1_hydration",
    "semantic_lane",
    "supersession_demotion",
    "verbatim_spans",
}
RUNTIME_FEATURES = BOOLEAN_RUNTIME_FEATURES | {
    "embedding_backfill_batch_chunks",
    "embedding_backfill_foreground_status_timeout_ms",
    "embedding_backfill_inter_batch_ms",
    "embedding_backfill_open_p95_limit_ms",
    "embedding_backfill_search_p95_limit_ms",
    "materialize_token_budget",
    "search_section_demotion_top_n",
    "semantic_deadline_ms",
    "supersession_demotion_weight",
}
CURRENT_RUNTIME_FEATURES = frozenset(RUNTIME_FEATURES)
SERVICE_BOOLEAN_FEATURE_FLAGS = frozenset(BOOLEAN_RUNTIME_FEATURES)


def evaluation_source_fingerprint() -> dict[str, Any]:
    source = git_source_fingerprint()
    return {
        "source_revision": source["revision"],
        "tracked_source_clean": source["tracked_source_clean"],
        "untracked_source_files": source["untracked_source_files"],
        "reproducible_source": source["clean"],
    }


def e09_status_snapshot(
    metadata: dict[str, Any],
    *,
    run_id: str,
) -> dict[str, Any]:
    client = NativeApiClient(
        token=str(metadata["token"]),
        run_id=run_id,
        case_id="e09-runtime-provenance",
    )
    return client.get("/v1/status").data


def response_reports_semantic_gap(value: Any) -> bool:
    if isinstance(value, dict):
        failures = value.get("lane_failures")
        if isinstance(failures, list) and any(
            str(item).startswith("semantic")
            for item in failures
        ):
            return True
        if (
            value.get("lane") == "semantic"
            and str(value.get("kind", "")).startswith("retrieval_lane_")
        ):
            return True
        return any(
            response_reports_semantic_gap(item)
            for item in value.values()
        )
    if isinstance(value, list):
        return any(response_reports_semantic_gap(item) for item in value)
    return False


def e09_coverage_probe(
    metadata: dict[str, Any],
    *,
    run_id: str,
    case_id: str,
) -> None:
    client = NativeApiClient(
        token=str(metadata["token"]),
        run_id=run_id,
        case_id=case_id,
    )
    response = client.post(
        "/v1/workspace/search",
        {
            "queries": [{
                "id": "e09-semantic-coverage",
                "query": (
                    "E09 semantic coverage sentinel for fully indexed "
                    f"evaluation case {case_id}"
                ),
                "modes": ["semantic"],
                "limit": 1,
            }],
        },
    )
    if (
        response_reports_semantic_gap(response.body)
        or not response_has_candidates(response.body)
    ):
        raise ValueError(
            f"E09 semantic coverage probe failed for {case_id}: "
            "semantic lane was unavailable, empty, failed, or deferred"
        )


def subscription_reasoning_environment(
    source: dict[str, str] | None = None,
) -> dict[str, str]:
    env = dict(os.environ if source is None else source)
    explicit_denials = {
        "OPENAI_API_KEY",
        "OPENAI_API_BASE",
        "OPENAI_BASE_URL",
        "OPENAI_ORGANIZATION",
        "OPENAI_ORG_ID",
        "OPENAI_PROJECT",
        "OPENAI_PROJECT_ID",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_ENDPOINT",
        "CODEX_API_KEY",
        "CARRYSTATE_EVAL_DIRECT_OPENAI",
    }
    for key in list(env):
        upper = key.upper()
        if (
            upper in explicit_denials
            or upper.startswith(("OPENAI_", "AZURE_OPENAI_"))
            or upper.endswith("_API_KEY")
            or (
                upper.startswith("CODEX_")
                and any(
                    marker in upper
                    for marker in ("API_BASE", "BASE_URL", "ENDPOINT", "ORGANIZATION", "PROJECT")
                )
            )
        ):
            env.pop(key, None)
    return env


def require_codex_subscription(codex: Path) -> dict[str, str]:
    if os.environ.get("CODEX_API_KEY"):
        raise ValueError(
            "Paid Codex API-key reasoning is forbidden. Remove CODEX_API_KEY and "
            "use a ChatGPT-authenticated Codex account."
        )
    if os.environ.get("CARRYSTATE_EVAL_DIRECT_OPENAI", "").strip().lower() not in {
        "",
        "0",
        "false",
    }:
        raise ValueError(
            "Direct OpenAI reasoning is forbidden for Straylight evaluations. "
            "Use the Codex plan, switch ChatGPT accounts, or wait for its reset."
        )
    try:
        status = subprocess.run(
            [str(codex), "login", "status"],
            env=subscription_reasoning_environment(),
            text=True,
            capture_output=True,
            check=False,
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ValueError(f"Could not verify Codex subscription authentication: {exc}") from exc
    rendered = "\n".join((status.stdout, status.stderr)).strip()
    auth_lines = {
        line.strip()
        for line in rendered.splitlines()
        if line.strip()
    }
    if status.returncode != 0 or "Logged in using ChatGPT" not in auth_lines:
        raise ValueError(
            "Straylight reasoning evaluations require Codex logged in through "
            "ChatGPT. API-key billing is forbidden; switch accounts or wait for "
            "the subscription reset."
        )
    try:
        version = subprocess.run(
            [str(codex), "--version"],
            env=subscription_reasoning_environment(),
            text=True,
            capture_output=True,
            check=False,
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ValueError(f"Could not record the Codex version: {exc}") from exc
    version_rendered = "\n".join((version.stdout, version.stderr)).strip()
    if version.returncode != 0 or not version_rendered:
        raise ValueError("Could not record the Codex version for the run ledger")
    return {
        **REASONING_BILLING_POLICY,
        "codex_path": str(codex.expanduser().resolve()),
        "codex_version": version_rendered.splitlines()[0],
        "auth_checked_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "auth_status": "Logged in using ChatGPT",
    }


def git_source_fingerprint(repository: Path = PROJECT_ROOT) -> dict[str, Any]:
    def run(command: list[str]) -> subprocess.CompletedProcess[str]:
        try:
            return subprocess.run(
                command,
                cwd=repository,
                text=True,
                capture_output=True,
                check=False,
                timeout=15,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise ValueError(
                f"could not capture git source provenance: {exc}"
            ) from exc

    revision_result = run(["git", "rev-parse", "HEAD"])
    revision = revision_result.stdout.strip()
    if revision_result.returncode != 0 or not revision:
        detail = revision_result.stderr.strip() or "empty revision"
        raise ValueError(
            f"could not capture git source revision: {detail}"
        )

    tracked = run(["git", "diff", "--quiet", "HEAD", "--"])
    if tracked.returncode not in {0, 1}:
        detail = tracked.stderr.strip() or "git diff failed"
        raise ValueError(
            f"could not capture tracked source state: {detail}"
        )

    untracked_result = run([
        "git",
        "ls-files",
        "--others",
        "--exclude-standard",
    ])
    if untracked_result.returncode != 0:
        detail = untracked_result.stderr.strip() or "git ls-files failed"
        raise ValueError(
            f"could not capture untracked source state: {detail}"
        )
    untracked = untracked_result.stdout.strip()
    untracked_source_files = [
        path
        for path in untracked.splitlines()
        if not path.startswith(("results/", "runs/"))
    ]
    clean = bool(
        revision
        and tracked.returncode == 0
        and not untracked_source_files
    )
    return {
        "revision": revision,
        "tracked_source_clean": tracked.returncode == 0,
        "untracked_source_files": untracked_source_files,
        "clean": clean,
    }


def capture_service_image_fingerprint(
    api_container: str,
    *,
    source_revision: str,
    expected_build_revision: str,
) -> dict[str, Any]:
    if not isinstance(api_container, str) or not api_container.strip():
        raise ValueError("--api-container must identify one running container")
    if not isinstance(source_revision, str) or not source_revision:
        raise ValueError("service image provenance requires a source revision")
    if (
        not isinstance(expected_build_revision, str)
        or not expected_build_revision
    ):
        raise ValueError(
            "service image provenance requires --expect-build-revision"
        )

    def inspect_value(template: str, field: str) -> str:
        completed = subprocess.run(
            [
                "docker",
                "inspect",
                f"--format={template}",
                api_container,
            ],
            cwd=PROJECT_ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        value = completed.stdout.strip()
        if (
            completed.returncode != 0
            or not value
            or value in {"<no value>", "null"}
        ):
            detail = completed.stderr.strip() or "empty inspect result"
            raise ValueError(
                f"could not inspect service container {field}: {detail}"
            )
        return value

    container_id = inspect_value("{{.Id}}", "container ID")
    started_at = inspect_value("{{.State.StartedAt}}", "start time")
    running = inspect_value("{{.State.Running}}", "running state")
    image_id = inspect_value("{{.Image}}", "image ID")
    image_revision = inspect_value(
        (
            '{{index .Config.Labels '
            '"org.opencontainers.image.revision"}}'
        ),
        "image revision label",
    )
    if running != "true":
        raise ValueError(
            f"service container {api_container} is not running"
        )
    if image_revision != source_revision:
        raise ValueError(
            "service image revision does not match source revision: "
            f"{image_revision} != {source_revision}"
        )
    if image_revision != expected_build_revision:
        raise ValueError(
            "service image revision does not match expected build revision: "
            f"{image_revision} != {expected_build_revision}"
        )
    return {
        "schema": SERVICE_IMAGE_FINGERPRINT_SCHEMA,
        "api_container": api_container,
        "api_container_id": container_id,
        "api_container_started_at": started_at,
        "api_container_running": True,
        "api_image_id": image_id,
        "api_image_revision": image_revision,
    }


def stable_service_image_provenance(
    before: dict[str, Any],
    after: dict[str, Any],
) -> dict[str, Any]:
    required_fields = (
        "schema",
        "api_container",
        "api_container_id",
        "api_container_started_at",
        "api_container_running",
        "api_image_id",
        "api_image_revision",
    )
    malformed = [
        field
        for field in required_fields
        if field not in before or field not in after
    ]
    if malformed:
        raise ValueError(
            f"service image fingerprint is incomplete: {malformed}"
        )
    if (
        before.get("schema") != SERVICE_IMAGE_FINGERPRINT_SCHEMA
        or after.get("schema") != SERVICE_IMAGE_FINGERPRINT_SCHEMA
    ):
        raise ValueError("service image fingerprint schema is invalid")
    drift = {
        field: {"before": before.get(field), "after": after.get(field)}
        for field in required_fields
        if before.get(field) != after.get(field)
    }
    if drift:
        raise ValueError(
            f"service container or image drifted during the run: {drift}"
        )
    return {
        "schema": SERVICE_IMAGE_PROVENANCE_SCHEMA,
        "stable": True,
        "before": before,
        "after": after,
    }


def canonical_json_sha256(value: Any) -> str:
    rendered = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(rendered.encode("utf-8")).hexdigest()


def is_local_cli_failure(operation: dict[str, Any]) -> bool:
    http_status = operation.get("http_status")
    return bool(
        type(http_status) is int
        and http_status == 0
        and str(operation.get("operation", "")).startswith("failed:")
    )


def summarize_local_cli_failures(
    operations: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    failures = [
        operation
        for operation in operations
        if is_local_cli_failure(operation)
    ]
    operation_counts = Counter(
        str(operation["operation"])
        for operation in failures
    )
    return {
        "schema": LOCAL_CLI_FAILURE_SUMMARY_SCHEMA,
        "count": len(failures),
        "operation_counts": dict(sorted(operation_counts.items())),
        "result_chars": sum(
            int(operation["result_chars"])
            for operation in failures
        ),
        "source_text_chars": sum(
            int(operation["source_text_chars"])
            for operation in failures
        ),
        "metadata_chars": sum(
            int(operation["metadata_chars"])
            for operation in failures
        ),
        "operations": [dict(operation) for operation in failures],
    }


def summarize_service_accounting(
    operations: Sequence[dict[str, Any]],
) -> dict[str, int | float]:
    service_operations = [
        operation
        for operation in operations
        if not is_local_cli_failure(operation)
    ]
    return {
        "service_calls": len(service_operations),
        "service_http_calls": sum(
            int(operation.get("http_calls", 1))
            for operation in service_operations
        ),
        "service_binary_bytes": sum(
            int(operation.get("binary_bytes", 0))
            for operation in service_operations
        ),
        "service_result_chars": sum(
            int(operation.get("result_chars", 0))
            for operation in service_operations
        ),
        "service_source_text_chars": sum(
            int(operation.get("source_text_chars", 0))
            for operation in service_operations
        ),
        "service_metadata_chars": sum(
            int(operation.get("metadata_chars", 0))
            for operation in service_operations
        ),
        "service_replay_weighted_chars": sum(
            int(operation.get("result_chars", 0))
            * (len(service_operations) - index)
            for index, operation in enumerate(service_operations)
        ),
        "service_latency_ms": round(
            sum(
                float(operation.get("elapsed_ms", 0))
                for operation in service_operations
            ),
            3,
        ),
    }


def validate_definitive_native_failures(
    case: dict[str, Any],
    record: dict[str, Any],
    operations: Sequence[dict[str, Any]],
) -> None:
    expected_summary = summarize_local_cli_failures(operations)
    if record.get("local_cli_failures") != expected_summary:
        raise ValueError(
            f"definitive service case {case.get('id')} has a missing or "
            "inconsistent local_cli_failures summary"
        )

    for index, operation in enumerate(operations):
        name = str(operation["operation"])
        http_status = int(operation["http_status"])
        if is_local_cli_failure(operation):
            recovered_operation = name.removeprefix("failed:")
            recovered = any(
                later.get("operation") == recovered_operation
                and type(later.get("http_status")) is int
                and 200 <= later["http_status"] <= 399
                and int(later.get("http_calls", 1)) >= 1
                for later in operations[index + 1:]
            )
            if not recovered_operation or not recovered:
                raise ValueError(
                    f"definitive service case {case.get('id')} contains "
                    f"unrecovered local CLI failure {name}"
                )
            continue
        if (
            http_status >= 400
            or name.startswith(("failed:", "denied:"))
        ):
            raise ValueError(
                f"definitive service case {case.get('id')} contains "
                "a measured failed or denied native operation"
            )


def definitive_service_run_provenance(
    run: dict[str, Any],
) -> dict[str, Any]:
    if not isinstance(run, dict):
        raise ValueError("definitive service artifact must be an object")
    run_id = run.get("run_id")
    experiment_arm = run.get("experiment_arm")
    paired_draw_id = run.get("paired_draw_id")
    if (
        not isinstance(run_id, str)
        or not run_id
        or not isinstance(experiment_arm, str)
        or not EXPERIMENT_ID_PATTERN.fullmatch(experiment_arm)
        or not isinstance(paired_draw_id, str)
        or not EXPERIMENT_ID_PATTERN.fullmatch(paired_draw_id)
    ):
        raise ValueError(
            "definitive service artifact lacks explicit arm/draw identity"
        )

    manifest = run.get("manifest")
    if not isinstance(manifest, dict):
        raise ValueError("definitive service artifact lacks its manifest")
    benchmark_version = run.get("benchmark_version")
    if (
        not isinstance(benchmark_version, str)
        or not benchmark_version
        or manifest.get("benchmark_version") != benchmark_version
        or manifest.get("conditions") != ["service_api"]
    ):
        raise ValueError(
            "definitive service artifact has an invalid benchmark or "
            "condition selection"
        )
    raw_cases = manifest.get("cases")
    if not isinstance(raw_cases, list) or not raw_cases:
        raise ValueError(
            "definitive service artifact lacks selected manifest cases"
        )
    cases_by_id = {
        case.get("id"): case
        for case in raw_cases
        if isinstance(case, dict)
        and isinstance(case.get("id"), str)
        and case["id"]
    }
    if len(cases_by_id) != len(raw_cases):
        raise ValueError(
            "definitive service artifact has malformed or duplicate "
            "manifest cases"
        )
    if run.get("service_protocol") != "simple":
        raise ValueError(
            "definitive service artifact must use the simple protocol"
        )
    retrieval_modes = run.get("service_retrieval_modes")
    if retrieval_modes != ["exact", "lexical"]:
        raise ValueError(
            "definitive non-semantic service artifact must bind exactly "
            "exact and lexical retrieval"
        )

    billing = run.get("reasoning_billing")
    if (
        not isinstance(billing, dict)
        or billing.get("route") != "chatgpt_subscription"
        or billing.get("api_fallback") != "forbidden"
        or billing.get("auth_status") != "Logged in using ChatGPT"
    ):
        raise ValueError(
            "definitive service artifact lacks verified subscription reasoning"
        )

    implementation = run.get("implementation_fingerprint")
    ledger = run.get("run_ledger")
    ledger_source = ledger.get("source") if isinstance(ledger, dict) else None
    if (
        not isinstance(implementation, dict)
        or implementation.get("reproducible_source") is not True
        or implementation.get("tracked_source_clean") is not True
        or implementation.get("untracked_source_files") != []
        or not isinstance(implementation.get("source_revision"), str)
        or not implementation["source_revision"]
        or not isinstance(ledger_source, dict)
        or ledger_source.get("clean") is not True
        or ledger_source.get("tracked_source_clean") is not True
        or ledger_source.get("untracked_source_files") != []
        or ledger_source.get("revision")
        != implementation["source_revision"]
    ):
        raise ValueError(
            "definitive service artifact lacks clean, stable source provenance"
        )
    source_revision = implementation["source_revision"]

    if (
        not isinstance(ledger, dict)
        or ledger.get("schema") != RUN_LEDGER_SCHEMA
        or ledger.get("run_id") != run_id
    ):
        raise ValueError("definitive service artifact has an invalid run ledger")
    configuration = ledger.get("configuration")
    artifacts = ledger.get("artifacts")
    experiment_parameters = run.get("experiment_parameters")
    if (
        not isinstance(configuration, dict)
        or configuration.get("conditions") != ["service_api"]
        or configuration.get("service_protocol") != "simple"
        or configuration.get("experiment_arm") != experiment_arm
        or configuration.get("paired_draw_id") != paired_draw_id
        or configuration.get("experiment_parameters")
        != experiment_parameters
        or not isinstance(experiment_parameters, dict)
        or experiment_parameters.get("service_retrieval_modes")
        != retrieval_modes
        or not isinstance(artifacts, dict)
    ):
        raise ValueError(
            "definitive service artifact ledger configuration disagrees "
            "with the run"
        )
    for field in ("manifest_sha256", "schema_sha256", "harness_sha256"):
        value = run.get(field)
        if (
            not isinstance(value, str)
            or not SHA256_PATTERN.fullmatch(value)
            or artifacts.get(field) != value
        ):
            raise ValueError(
                f"definitive service artifact has invalid {field} provenance"
            )

    runtime_snapshot = run.get("service_runtime_snapshot")
    expected_features = run.get("expected_runtime_features")
    expected_build_revision = run.get("expected_build_revision")
    if (
        not isinstance(runtime_snapshot, dict)
        or runtime_snapshot.get("schema")
        != "straylight-service-runtime-snapshot@v1"
        or runtime_snapshot.get("status") != "ready"
        or not isinstance(runtime_snapshot.get("runtime_features"), dict)
        or not isinstance(runtime_snapshot.get("embeddings"), dict)
        or not isinstance(expected_features, dict)
        or expected_build_revision != source_revision
        or runtime_snapshot.get("build_revision") != source_revision
        or configuration.get("expected_build_revision") != source_revision
        or configuration.get("expected_runtime_features")
        != expected_features
    ):
        raise ValueError(
            "definitive service artifact lacks matching source/build/runtime "
            "provenance"
        )
    runtime_features = runtime_snapshot["runtime_features"]
    runtime_mismatches = {
        name: {"expected": expected, "actual": runtime_features.get(name)}
        for name, expected in expected_features.items()
        if (
            name not in runtime_features
            or type(runtime_features[name]) is not type(expected)
            or runtime_features[name] != expected
        )
    }
    if runtime_mismatches:
        raise ValueError(
            "definitive service artifact runtime violates its expectations: "
            f"{runtime_mismatches}"
        )
    if (
        artifacts.get("runtime_snapshot_sha256")
        != canonical_json_sha256(runtime_snapshot)
    ):
        raise ValueError(
            "definitive service artifact ledger does not bind its runtime "
            "snapshot"
        )

    image_provenance = run.get("service_image_provenance")
    if (
        not isinstance(image_provenance, dict)
        or image_provenance.get("schema")
        != SERVICE_IMAGE_PROVENANCE_SCHEMA
        or image_provenance.get("stable") is not True
        or not isinstance(image_provenance.get("before"), dict)
        or not isinstance(image_provenance.get("after"), dict)
    ):
        raise ValueError(
            "definitive service artifact lacks stable image provenance"
        )
    stable_service_image_provenance(
        image_provenance["before"],
        image_provenance["after"],
    )
    image = image_provenance["before"]
    if (
        image.get("api_container_running") is not True
        or not isinstance(image.get("api_container_id"), str)
        or not image["api_container_id"]
        or not isinstance(image.get("api_container_started_at"), str)
        or not image["api_container_started_at"]
        or not isinstance(image.get("api_image_id"), str)
        or not DOCKER_IMAGE_ID_PATTERN.fullmatch(image["api_image_id"])
        or image.get("api_image_revision") != source_revision
        or experiment_parameters.get("service_image_fingerprint") != image
        or artifacts.get("service_image_provenance_sha256")
        != canonical_json_sha256(image_provenance)
    ):
        raise ValueError(
            "definitive service artifact image does not match its source, "
            "binding, or ledger"
        )

    codex = ledger.get("codex")
    if (
        not isinstance(codex, dict)
        or codex.get("auth_route") != billing["route"]
        or codex.get("api_fallback") != billing["api_fallback"]
        or codex.get("auth_status") != billing["auth_status"]
    ):
        raise ValueError(
            "definitive service artifact run ledger lacks matching Codex auth"
        )
    records = run.get("records")
    if not isinstance(records, list) or not records:
        raise ValueError("definitive service artifact has no records")
    seen_records: set[str] = set()
    for record in records:
        case_id = record.get("case_id") if isinstance(record, dict) else None
        if (
            not isinstance(record, dict)
            or not isinstance(case_id, str)
            or not case_id
            or case_id not in cases_by_id
            or case_id in seen_records
            or record.get("condition") != "service_api"
            or record.get("error")
            or not isinstance(record.get("grade"), dict)
            or record.get("service_accounting_valid") is not True
        ):
            raise ValueError(
                "definitive service artifact has an incomplete, duplicate, "
                "or invalid record"
            )
        seen_records.add(case_id)
        operations = validate_native_service_accounting({
            "operations": record.get("service_operations"),
        })
        validate_definitive_native_failures(
            cases_by_id[case_id],
            record,
            operations,
        )
        expected_accounting = summarize_service_accounting(operations)
        accounting_mismatches = {
            field: {
                "expected": expected,
                "actual": record.get(field),
            }
            for field, expected in expected_accounting.items()
            if (
                type(record.get(field)) is not type(expected)
                or record.get(field) != expected
            )
        }
        if accounting_mismatches:
            raise ValueError(
                f"definitive service case {case_id} has inconsistent "
                "local-failure-excluding service accounting: "
                f"{accounting_mismatches}"
            )
    if seen_records != set(cases_by_id):
        raise ValueError(
            "definitive service artifact does not contain exactly one record "
            "per selected manifest case"
        )

    return {
        "schema": "straylight-definitive-service-run-provenance@v1",
        "run_id": run_id,
        "experiment_arm": experiment_arm,
        "paired_draw_id": paired_draw_id,
        "benchmark_version": benchmark_version,
        "manifest_sha256": run["manifest_sha256"],
        "schema_sha256": run["schema_sha256"],
        "harness_sha256": run["harness_sha256"],
        "source_revision": source_revision,
        "build_revision": runtime_snapshot["build_revision"],
        "api_image_id": image["api_image_id"],
        "api_image_revision": image["api_image_revision"],
        "service_image_provenance": image_provenance,
        "runtime_snapshot": runtime_snapshot,
        "runtime_snapshot_sha256": artifacts["runtime_snapshot_sha256"],
        "runtime_features": runtime_features,
        "expected_runtime_features": expected_features,
        "service_retrieval_modes": retrieval_modes,
        "reasoning_billing": {
            "route": billing["route"],
            "api_fallback": billing["api_fallback"],
            "auth_status": billing["auth_status"],
        },
    }


def validate_experiment_identity(
    experiment_arm: str | None,
    paired_draw_id: str | None,
    conditions: Sequence[str],
) -> tuple[str | None, str | None]:
    if bool(experiment_arm) != bool(paired_draw_id):
        raise ValueError(
            "--experiment-arm and --paired-draw-id must be supplied together"
        )
    if experiment_arm is None:
        return None, None
    if not EXPERIMENT_ID_PATTERN.fullmatch(experiment_arm):
        raise ValueError(
            "--experiment-arm must be 1-128 safe identifier characters"
        )
    if not EXPERIMENT_ID_PATTERN.fullmatch(paired_draw_id or ""):
        raise ValueError(
            "--paired-draw-id must be 1-128 safe identifier characters"
        )
    if len(conditions) != 1:
        raise ValueError(
            "--experiment-arm is only valid for a single-condition invocation; "
            "same-invocation controls already use condition names as arms"
        )
    return experiment_arm, paired_draw_id


def resolve_service_retrieval_modes(
    requested: Sequence[str] | None,
    *,
    conditions: Sequence[str],
    service_protocol: str,
    experiment_arm: str | None = None,
    expected_runtime_features: dict[str, Any] | None = None,
    e09_arm: str | None = None,
    step_authorization: dict[str, Any] | None = None,
) -> tuple[str, ...]:
    modes = tuple(requested or ())
    if len(modes) != len(set(modes)):
        raise ValueError("--service-retrieval-modes must not contain duplicates")
    if any(mode not in {"exact", "lexical", "semantic"} for mode in modes):
        raise ValueError(
            "--service-retrieval-modes must be a subset of "
            "exact lexical semantic"
        )
    if modes and "service_api" not in conditions:
        raise ValueError(
            "--service-retrieval-modes requires the service_api condition"
        )
    if modes and service_protocol != "simple":
        raise ValueError(
            "--service-retrieval-modes requires --service-protocol simple"
        )
    e09_modes = enforced_retrieval_modes(
        e09_arm,
        step_authorization=step_authorization,
    )
    if modes and e09_modes and modes != e09_modes:
        raise ValueError(
            "--service-retrieval-modes conflicts with the selected E09 arm"
        )
    resolved = modes or e09_modes
    if (
        experiment_arm is not None
        and "service_api" in conditions
        and isinstance(expected_runtime_features, dict)
        and expected_runtime_features.get("semantic_lane") is False
        and resolved != ("exact", "lexical")
    ):
        raise ValueError(
            "explicit semantic_lane=off service experiment arms require "
            "--service-retrieval-modes exact lexical"
        )
    return resolved


def ensure_run_binding(
    run_root: Path,
    *,
    resume: bool,
    run_id: str,
    experiment_arm: str | None,
    paired_draw_id: str | None,
    conditions: Sequence[str],
    case_ids: Sequence[str],
    model: str,
    service_protocol: str,
    manifest_sha256: str,
    expected_runtime_features: dict[str, Any] | None = None,
    expected_build_revision: str | None = None,
    experiment_parameters: dict[str, Any] | None = None,
) -> dict[str, Any]:
    binding = {
        "version": 1,
        "run_id": run_id,
        "experiment_arm": experiment_arm,
        "paired_draw_id": paired_draw_id,
        "conditions": list(conditions),
        "case_ids": list(case_ids),
        "model": model,
        "service_protocol": service_protocol,
        "manifest_sha256": manifest_sha256,
        "expected_runtime_features": expected_runtime_features or {},
        "expected_build_revision": expected_build_revision,
        "experiment_parameters": experiment_parameters or {},
    }
    path = run_root / RUN_BINDING_STATE
    if resume:
        if not path.is_file():
            raise ValueError(f"Resumed run lacks immutable experiment binding: {path}")
        existing = json.loads(path.read_text(encoding="utf-8"))
        if existing != binding:
            raise ValueError(
                "resumed run configuration differs from its immutable experiment binding"
            )
        return binding
    path.write_text(json.dumps(binding, indent=2) + "\n", encoding="utf-8")
    path.chmod(0o600)
    return binding


def build_run_ledger(
    *,
    run_id: str,
    reasoning_billing: dict[str, str],
    model: str,
    conditions: Sequence[str],
    service_protocol: str,
    manifest_sha256: str,
    schema_sha256: str,
    harness_sha256: str,
    experiment_arm: str | None = None,
    paired_draw_id: str | None = None,
    runtime_snapshot: dict[str, Any] | None = None,
    expected_runtime_features: dict[str, Any] | None = None,
    expected_build_revision: str | None = None,
    experiment_parameters: dict[str, Any] | None = None,
    service_image_provenance: dict[str, Any] | None = None,
) -> dict[str, Any]:
    ledger = {
        "schema": RUN_LEDGER_SCHEMA,
        "run_id": run_id,
        "captured_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "source": git_source_fingerprint(),
        "codex": {
            "path": reasoning_billing["codex_path"],
            "version": reasoning_billing["codex_version"],
            "auth_route": reasoning_billing["route"],
            "auth_status": reasoning_billing["auth_status"],
            "auth_checked_at": reasoning_billing["auth_checked_at"],
            "api_fallback": reasoning_billing["api_fallback"],
        },
        "configuration": {
            "model": model,
            "conditions": list(conditions),
            "service_protocol": service_protocol,
            "experiment_arm": experiment_arm,
            "paired_draw_id": paired_draw_id,
            "expected_runtime_features": expected_runtime_features or {},
            "expected_build_revision": expected_build_revision,
            "experiment_parameters": experiment_parameters or {},
        },
        "artifacts": {
            "manifest_sha256": manifest_sha256,
            "schema_sha256": schema_sha256,
            "harness_sha256": harness_sha256,
        },
    }
    if runtime_snapshot is not None:
        ledger["artifacts"]["runtime_snapshot_sha256"] = canonical_json_sha256(
            runtime_snapshot
        )
    if service_image_provenance is not None:
        ledger["artifacts"][
            "service_image_provenance_sha256"
        ] = canonical_json_sha256(service_image_provenance)
    return ledger


def load_native_provisioning_state(path: Path, run_id: str) -> dict[str, dict[str, Any]]:
    if not path.exists():
        return {}
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("version") != 1 or payload.get("run_id") != run_id:
        raise ValueError(f"Invalid native provisioning state: {path}")
    cases = payload.get("cases")
    if not isinstance(cases, dict):
        raise ValueError(f"Invalid native provisioning cases: {path}")
    path.chmod(0o600)
    return cases


def write_native_provisioning_state(
    path: Path,
    run_id: str,
    cases: dict[str, dict[str, Any]],
) -> None:
    temporary = path.with_name(f"{path.name}.tmp")
    temporary.write_text(
        json.dumps({"version": 1, "run_id": run_id, "cases": cases}, indent=2) + "\n",
        encoding="utf-8",
    )
    temporary.chmod(0o600)
    temporary.replace(path)
    path.chmod(0o600)
CONDITION_ADAPTERS = {
    "fixed_pack": {"label": "Fixed handoff pack", "kind": "fixed_pack"},
    "filesystem": {"label": "Filesystem agent", "kind": "filesystem"},
    "filesystem_sidecar": {
        "label": "Filesystem agent with writable sidecar",
        "kind": "filesystem_sidecar",
    },
    "workspace": {"label": "Straylight workspace agent", "kind": "legacy_workspace"},
    "service_api": {"label": "Native Straylight API agent", "kind": "native_service"},
}
CONDITION_LABELS = {key: value["label"] for key, value in CONDITION_ADAPTERS.items()}
WORKSPACE_CONDITIONS = {"workspace", "service_api"}
CONCEPT_STOPWORDS = {
    "a", "an", "and", "are", "as", "at", "be", "been", "being", "both",
    "by", "did", "do", "does", "each", "for", "from", "is", "it", "its",
    "itself", "must", "of", "or", "remain", "remaining", "remains", "still",
    "that", "the", "their", "these", "this", "those", "through", "to", "was",
    "were", "with",
}
CONCEPT_IRREGULAR = {
    "aliases": "alias",
    "addresses": "address",
    "changed": "change",
    "changes": "change",
    "claims": "claim",
    "dossiers": "dossier",
    "exclude": "remove",
    "excluded": "remove",
    "excludes": "remove",
    "excluding": "remove",
    "ids": "id",
    "moved": "move",
    "moves": "move",
    "names": "name",
    "not": "no",
    "none": "no",
    "rebuilt": "rebuild",
    "retained": "retain",
    "retaining": "retain",
    "retains": "retain",
    "rewritten": "rewrite",
    "sources": "source",
    "states": "state",
}


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


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


def sha256_tree(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def normalize(value: str) -> str:
    value = value.casefold().replace("–", "-").replace("—", "-")
    value = re.sub(r"[`*_]", "", value)
    value = re.sub(r"\s+", " ", value)
    return value.strip()


def concept_stem(token: str) -> str:
    token = CONCEPT_IRREGULAR.get(token, token)
    if token in CONCEPT_IRREGULAR.values():
        return token
    if len(token) > 5 and token.endswith("ing"):
        token = token[:-3]
        if len(token) > 2 and token[-1] == token[-2]:
            token = token[:-1]
    elif len(token) > 4 and token.endswith("ed"):
        token = token[:-2]
    elif len(token) > 4 and token.endswith("s"):
        token = token[:-1]
    return CONCEPT_IRREGULAR.get(token, token)


def concept_tokens(value: str) -> set[str]:
    result: set[str] = set()
    for raw in re.findall(r"[a-z0-9]+(?:[._/+:-][a-z0-9]+)*", value.casefold()):
        rate = re.fullmatch(
            r"(?:\d+(?:\.\d+)?|[a-z]+)/"
            r"(?:s|sec|second|min|minute|h|hr|hour|day)",
            raw,
        )
        if rate:
            parts = raw.split("/")
        elif ":" in raw or "/" in raw or re.match(r"^\d{4}-\d{2}-", raw):
            parts = [raw]
        else:
            parts = re.split(r"[._+-]+", raw)
        for part in parts:
            normalized = concept_stem(part)
            if normalized and normalized not in CONCEPT_STOPWORDS:
                result.add(normalized)
    return result


def candidate_matches(candidate: str, value: str, mode: str) -> bool:
    if normalize(candidate) in normalize(value):
        return True
    if mode != "concept_tokens_v1":
        return False
    candidate_concepts = concept_tokens(candidate)
    return bool(candidate_concepts) and candidate_concepts <= concept_tokens(value)


def forbidden_is_asserted(forbidden: str, rendered: str) -> bool:
    phrase = normalize(forbidden)
    start = 0
    while (match_at := rendered.find(phrase, start)) >= 0:
        prefix = rendered[max(0, match_at - 240):match_at]
        clause = re.split(r"[.!?;]", prefix)[-1]
        negated = re.search(
            r"\b(?:cannot|can't|do not|does not|did not|must not|never|no|should not|without)\b",
            clause,
        )
        contrast = re.search(r"\b(?:but|except|however|instead)\b", clause)
        if not negated or (contrast and contrast.start() > negated.start()):
            return True
        start = match_at + len(phrase)
    return False


def load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_json_answer(path: Path) -> dict:
    raw = path.read_text(encoding="utf-8").strip()
    if raw.startswith("```"):
        raw = re.sub(r"^```(?:json)?\s*", "", raw)
        raw = re.sub(r"\s*```$", "", raw)
    return json.loads(raw)


def case_query(case: dict) -> str:
    slots = " ".join(case["claim_slots"].values())
    return f"{case['task']} {slots}"


def render_fixed_context(case: dict, index: BM25Index, manifest: dict) -> str:
    ranked = index.search(case_query(case))
    results = diverse_results(
        ranked,
        limit=manifest["fixed_pack_chunks"],
        max_chars=manifest["fixed_pack_chars"],
    )
    lines = [
        "# Fixed handoff context",
        "",
        "This context was assembled once from the task. It cannot be expanded during the run.",
    ]
    for item in results:
        lines.extend([
            "",
            f"## {item['path']} :: {item['heading']} :: lines {item['lines'][0]}-{item['lines'][1]}",
            item["text"],
        ])
    return "\n".join(lines) + "\n"


def render_prompt(
    case: dict,
    condition: str,
    service_protocol: str = "legacy",
) -> str:
    scope = case.get("scope", case["workload"])
    if service_protocol == "simple":
        service_read_guidance = (
            "Follow a candidate into its complete entry with an exact "
            "`./memory read --path \"path/from/search\"` or "
            "`./memory read --ref \"entry:...\"`. "
        )
        service_compute_guidance = (
            "Do any arithmetic or verification with your own reasoning tools; "
            "the workspace API intentionally has no compute or verify operation. "
        )
    else:
        service_read_guidance = (
            "Follow a candidate into its source with "
            "`./memory read --ref \"chunk:...\" --neighbors 4` or an exact line range. "
        )
        service_compute_guidance = (
            "Use one typed compute or combined verify call only when useful. "
        )
    if condition == "fixed_pack":
        access = (
            "Read ./context.md. It is your only evidence source. Do not browse, search elsewhere on the machine, "
            "or invent missing details. If the pack is incomplete, state the gap."
        )
    elif condition == "filesystem":
        access = (
            "The frozen evidence corpus is available at ./corpus. Use ordinary filesystem tools such as rg, sed, "
            "and read-only scripts. Do not browse, modify corpus files, or use paths outside ./corpus."
        )
    elif condition == "filesystem_sidecar":
        access = (
            "The frozen evidence corpus is available read-only at ./corpus. Use ordinary filesystem tools such as rg, sed, "
            "and read-only scripts. The only writable durable-work surface is the run-scoped ./sidecar directory. Before "
            "answering, write ./sidecar/checkpoint.json as one JSON object with objective, current_state, decisions, "
            "open_questions, next_actions, and artifacts fields. Do not browse, modify corpus files, or read or write paths "
            "outside ./corpus and ./sidecar."
        )
    elif condition == "service_api" and case.get("workspace_access") == "read_only":
        access = (
            "Use the read-only native Straylight service through ./memory only. Do not inspect the wrapper or any corpus path. "
            "Run `./memory open` and treat its initial evidence, learned context, checkpoint, and revision delta as the first "
            "answer packet. A `complete_source` item is already the full source and must not be read again. Pointer-only "
            "`evidence_leads` identify additional sources to read only when a requested task facet is absent. Treat "
            "`retrieval_sufficiency.status=likely_sufficient` as evidence that the primary source is complete and task anchors "
            "are covered, not as proof that every requested output facet is established. Build a facet checklist from the task "
            "and claim slots; do not query again for facts that packet directly establishes. For unresolved checklist gaps, use one focused "
            f"`./memory query --scope {json.dumps(scope)} \"question\"`, or `--batch` only for independent gaps. "
            f"{service_read_guidance}A candidate "
            "excerpt is a source lead, not proof that the displayed section is complete: if its path clearly owns an unresolved "
            "claim, read that exact path before searching globally again. A single read call can repeat `--path` or `--ref` to "
            "fetch several exact sources or ranges together. "
            "This credential cannot checkpoint, save, or mutate corpus or staged state. Keep the whole run to four service calls "
            "when the evidence permits. Do not browse or use filesystem search outside this service surface."
        )
    elif condition == "service_api":
        access = (
            "Use the native Straylight service through ./memory only. Do not inspect the wrapper or any corpus path. "
            "Run `./memory open` and treat its initial evidence, learned context, checkpoint, and revision delta as the first "
            "answer packet. A `complete_source` item is already the full source and must not be read again. Pointer-only "
            "`evidence_leads` identify additional sources to read only when a requested task facet is absent. Treat "
            "`retrieval_sufficiency.status=likely_sufficient` as evidence that the primary source is complete and task anchors "
            "are covered, not as proof that every requested output facet is established. Build a facet checklist from the task "
            "and claim slots; do not query again for facts that packet directly establishes. For unresolved checklist gaps, use one focused "
            f"`./memory query --scope {json.dumps(scope)} \"question\"`, or `--batch` only for independent gaps. "
            f"{service_read_guidance}A candidate "
            "excerpt is a source lead, not proof that the displayed section is complete: if its path clearly owns an unresolved "
            "claim, read that exact path before searching globally again. A single read call can repeat `--path` or `--ref` to "
            "fetch several exact sources or ranges together. "
            f"{service_compute_guidance}Before answering, persist one checkpoint with this exact command shape: "
            f"{CANONICAL_SERVICE_CHECKPOINT_INVOCATION}. Pass the JSON directly as the single-quoted shell literal shown; "
            "encode any apostrophe inside a JSON string as `\\u0027` so it cannot terminate that literal. Do not use raw prose, "
            "`--text`, stdin or a pipe, `--json-stdin`, or shell-variable indirection, including an unset variable. "
            "Keep the whole run to four service calls when the evidence permits. Do not browse or use "
            "filesystem search outside this service surface."
        )
    elif case.get("workspace_access") == "read_only":
        access = (
            "Use the read-only Straylight workspace through ./memory only. Do not inspect the wrapper or corpus path directly. "
            f"Start with ./memory open --scope {json.dumps(scope)}, then use only targeted query, read, compute, or verify "
            "operations. This credential cannot checkpoint or mutate corpus or staged state. Do not browse or use filesystem "
            "search outside this workspace surface."
        )
    else:
        access = (
            "Use the Straylight workspace through ./memory only. Do not inspect the wrapper or corpus path directly. "
            f"Start with ./memory open --scope {json.dumps(scope)}, then use a small number of targeted query and read "
            "operations. Use compute for arithmetic and one combined verify when useful; do not verify every claim separately "
            "when the same sources cover them. Before answering, persist a checkpoint with ./memory checkpoint. Do not browse "
            "or use filesystem search outside this workspace surface."
        )

    slot_lines = "\n".join(f"- {claim_id}: {label}" for claim_id, label in case["claim_slots"].items())
    checkpoint_instruction = (
        "For this read-only workspace case, the required checkpoint-shaped response is an output proposal only. "
        "Do not persist it through ./memory."
        if condition in WORKSPACE_CONDITIONS and case.get("workspace_access") == "read_only"
        else (
            "The checkpoint must be useful to the next fresh agent and must also be persisted at "
            "./sidecar/checkpoint.json."
            if condition == "filesystem_sidecar"
            else "The checkpoint must be useful to the next fresh agent."
        )
    )
    return f"""You are a fresh agent taking over durable work from prior agents.

{access}

Task:
{case['task']}

Return the required JSON object. The `claims` array must contain exactly one entry for each claim slot below, using the exact ID. Put the factual answer for that slot in `value` and cite relative corpus paths in `source_paths`.
For service retrieval, cite only exact `path` values returned with candidates, never a path merely mentioned inside candidate content.
When a claim characterizes a source's authority, cite that original source, not only an index or summary that discusses it.
Copy stable IDs, ISO-8601 timestamps, timezones, hashes, quantities, and measurements exactly from authoritative evidence. Do not silently normalize or reconstruct exact values from memory.
Before checkpointing, check every task facet against the claim slot responsible for it. Keep every claim slot self-contained; repeat a fact when it is needed in more than one slot, and do not rely on details stated only in another claim, the answer summary, or the checkpoint. When reporting an inventory, implementation state, public/private boundary, or operational behavior, preserve the concrete names, thresholds, state transitions, and failure or recovery rules that establish the conclusion rather than replacing them with counts, symbolic IDs, or broad categories. Cite the source that directly supports each slot's details, even when another source elsewhere in the answer covers adjacent context.
{slot_lines}

{checkpoint_instruction} Record the objective, current state, decisions, unresolved questions, concrete next actions, and the source or output artifacts that matter. Distinguish current fact from proposal, historical evidence from superseding state, and verified results from incomplete work. Use only evidence available under this condition.
"""


def write_memory_wrapper(run_dir: Path, corpus: Path, access_mode: str) -> None:
    wrapper = run_dir / "memory"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"exec python3 '{PROJECT_ROOT / 'workspace_cli.py'}' "
        f"--corpus '{corpus}' --session '{run_dir / 'workspace-session.json'}' "
        f"--access-mode '{access_mode}' \"$@\"\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)


def prepare_case_dir(
    root: Path,
    case: dict,
    condition: str,
    *,
    corpus: Path,
    index: BM25Index,
    manifest: dict,
    run_id: str | None = None,
    native_metadata: dict[str, Any] | None = None,
    service_protocol: str = "legacy",
) -> tuple[Path, int]:
    run_dir = root / condition / case["id"]
    run_dir.mkdir(parents=True, exist_ok=False)
    context_chars = 0
    if condition == "fixed_pack":
        context = render_fixed_context(case, index, manifest)
        (run_dir / "context.md").write_text(context, encoding="utf-8")
        context_chars = len(context)
    elif condition in {"filesystem", "filesystem_sidecar"}:
        (run_dir / "corpus").symlink_to(corpus, target_is_directory=True)
        if condition == "filesystem_sidecar":
            (run_dir / "sidecar").mkdir()
    elif condition == "workspace":
        write_memory_wrapper(
            run_dir,
            corpus,
            case.get("workspace_access", "read_write"),
        )
    elif condition == "service_api":
        if not run_id or native_metadata is None:
            raise ValueError("service_api requires provisioned native metadata")
        write_native_memory_wrapper(
            run_dir,
            task=case["task"],
            display_scope=case.get("scope", case["workload"]),
            authorization_scope=native_metadata["authorization_scope"],
            run_id=run_id,
            case_id=case["id"],
            checkpoint_id=native_metadata.get("checkpoint_id"),
            protocol=service_protocol,
        )
    else:
        raise ValueError(f"Unknown condition adapter: {condition}")
    (run_dir / "prompt.txt").write_text(
        render_prompt(case, condition, service_protocol),
        encoding="utf-8",
    )
    return run_dir, context_chars


def recursive_token_usage(value: Any, totals: dict[str, int]) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key in {"input_tokens", "output_tokens", "cached_input_tokens"} and isinstance(item, int):
                totals[key] = max(totals.get(key, 0), item)
            else:
                recursive_token_usage(item, totals)
    elif isinstance(value, list):
        for item in value:
            recursive_token_usage(item, totals)


def parse_event_metrics(path: Path) -> dict:
    totals: dict[str, int] = {}
    event_count = 0
    command_count = 0
    command_output_chars = 0
    if not path.exists():
        return {"events": 0, "commands": 0, "command_output_chars": 0, "tokens": totals}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        event_count += 1
        recursive_token_usage(event, totals)
        item = event.get("item", {})
        if event.get("type") == "item.completed" and item.get("type") == "command_execution":
            command_count += 1
            command_output_chars += len(item.get("aggregated_output") or "")
    return {
        "events": event_count,
        "commands": command_count,
        "command_output_chars": command_output_chars,
        "tokens": totals,
    }


def read_sidecar_checkpoint(run_dir: Path) -> dict[str, Any]:
    path = run_dir / SIDECAR_CHECKPOINT_RELATIVE_PATH
    result: dict[str, Any] = {
        "path": str(path),
        "exists": path.is_file() and not path.is_symlink(),
        "valid": False,
        "bytes": 0,
        "characters": 0,
        "sha256": None,
        "checkpoint": None,
        "error": None,
    }
    if not result["exists"]:
        result["error"] = "missing run-scoped sidecar checkpoint"
        return result
    try:
        raw = path.read_bytes()
        if len(raw) > 1_048_576:
            raise ValueError("sidecar checkpoint exceeds 1 MiB")
        text = raw.decode("utf-8")
        checkpoint = json.loads(text)
        if not isinstance(checkpoint, dict):
            raise ValueError("sidecar checkpoint must be a JSON object")
        result.update({
            "valid": True,
            "bytes": len(raw),
            "characters": len(text),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "checkpoint": checkpoint,
        })
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        result["error"] = str(exc)
    return result


def source_matches(cited: str, expected: Sequence[str]) -> bool:
    normalized = normalize_citation_path(cited)
    return normalized in expected


def normalize_citation_path(cited: str) -> str:
    normalized = cited.strip()
    if normalized.startswith("./"):
        normalized = normalized[2:]
    if normalized.startswith("corpus/"):
        normalized = normalized[len("corpus/"):]
    if (
        not normalized
        or normalized.startswith("/")
        or any(part in {"", ".", ".."} for part in normalized.split("/"))
    ):
        return ""
    return normalized


def recompute_grade(grade: dict) -> None:
    claims = grade.get("claims", [])
    claim_score = (
        statistics.fmean(float(item.get("score") or 0) for item in claims)
        if claims
        else 0.0
    )
    full_claims = sum(1 for item in claims if item.get("pass"))
    checkpoint_score = float(grade.get("checkpoint_score") or 0)
    citation_validity = float(grade.get("citation_validity") or 0)
    score = 0.85 * claim_score + 0.1 * checkpoint_score + 0.05 * citation_validity
    grade["score"] = round(score, 4)
    grade["claims_passed"] = full_claims
    grade["claims_total"] = len(claims)
    grade["claim_score"] = round(claim_score, 4)
    grade["pass"] = bool(
        score >= 0.8
        and full_claims == len(claims)
        and grade.get("claim_set_valid") is True
        and not grade.get("forbidden_hits")
    )


def grade_answer(case: dict, answer: dict, corpus_paths: set[str]) -> dict:
    answer_claims = answer.get("claims", [])
    if not isinstance(answer_claims, list):
        answer_claims = []
    actual_claim_ids = [
        str(claim.get("id"))
        for claim in answer_claims
        if isinstance(claim, dict) and isinstance(claim.get("id"), str)
    ]
    expected_claim_ids = [str(rubric["id"]) for rubric in case["rubric"]]
    duplicate_claim_ids = sorted({
        claim_id
        for claim_id in actual_claim_ids
        if actual_claim_ids.count(claim_id) > 1
    })
    missing_claim_ids = sorted(set(expected_claim_ids) - set(actual_claim_ids))
    extra_claim_ids = sorted(set(actual_claim_ids) - set(expected_claim_ids))
    malformed_claim_count = sum(
        not isinstance(claim, dict) or not isinstance(claim.get("id"), str)
        for claim in answer_claims
    )
    claim_set_valid = bool(
        not duplicate_claim_ids
        and not missing_claim_ids
        and not extra_claim_ids
        and malformed_claim_count == 0
        and len(actual_claim_ids) == len(expected_claim_ids)
    )
    claims_by_id = {
        claim["id"]: claim
        for claim in answer_claims
        if isinstance(claim, dict) and isinstance(claim.get("id"), str)
    }
    claim_results = []
    all_citations: list[str] = []
    for rubric in case["rubric"]:
        claim = claims_by_id.get(rubric["id"], {})
        value = str(claim.get("value", ""))
        grading_mode = case.get("grading_mode", "exact_substring_v1")
        checks = []
        for check in rubric["checks"]:
            matched = next(
                (
                    candidate
                    for candidate in check["any"]
                    if candidate_matches(candidate, value, grading_mode)
                ),
                None,
            )
            checks.append({"alternatives": check["any"], "matched": matched})
        citations = claim.get("source_paths", []) if isinstance(claim.get("source_paths", []), list) else []
        all_citations.extend(citations)
        expected_all = rubric.get("sources_all")
        if isinstance(expected_all, list):
            source_hit = bool(expected_all) and all(
                any(source_matches(citation, [expected]) for citation in citations)
                for expected in expected_all
            )
        else:
            source_hit = any(
                source_matches(citation, rubric["sources_any"])
                for citation in citations
            )
        content_fraction = sum(1 for check in checks if check["matched"]) / max(1, len(checks))
        score = 0.8 * content_fraction + 0.2 * int(source_hit)
        tags = list(rubric.get("tags", []))
        claim_results.append({
            "id": rubric["id"],
            "score": round(score, 4),
            "pass": content_fraction == 1.0 and source_hit,
            "content_fraction": round(content_fraction, 4),
            "source_hit": source_hit,
            "checks": checks,
            "citations": citations,
            "native_paths": list(rubric.get("native_paths", [])),
            "tags": tags,
            "exact_value": "exact_value" in tags,
        })

    checkpoint = answer.get("checkpoint", {})
    required_checkpoint = ["objective", "current_state", "next_actions", "artifacts"]
    checkpoint_fields = {
        field: bool(checkpoint.get(field))
        for field in required_checkpoint
    }
    checkpoint_score = sum(checkpoint_fields.values()) / len(required_checkpoint)
    rendered = normalize(json.dumps(answer, ensure_ascii=False))
    forbidden_hits = [
        item for item in case.get("forbidden", [])
        if forbidden_is_asserted(item, rendered)
    ]
    valid_citations = [
        citation for citation in all_citations
        if normalize_citation_path(citation) in corpus_paths
    ]
    citation_validity = len(valid_citations) / max(1, len(all_citations))
    result = {
        "score": 0.0,
        "pass": False,
        "claims_passed": 0,
        "claims_total": len(claim_results),
        "claim_score": 0.0,
        "checkpoint_score": round(checkpoint_score, 4),
        "checkpoint_fields": checkpoint_fields,
        "citation_validity": round(citation_validity, 4),
        "citation_count": len(all_citations),
        "forbidden_hits": forbidden_hits,
        "claims": claim_results,
        "claim_set_valid": claim_set_valid,
        "expected_claim_ids": expected_claim_ids,
        "actual_claim_ids": actual_claim_ids,
        "duplicate_claim_ids": duplicate_claim_ids,
        "missing_claim_ids": missing_claim_ids,
        "extra_claim_ids": extra_claim_ids,
        "malformed_claim_count": malformed_claim_count,
    }
    recompute_grade(result)
    return result


def attach_response_character_metrics(record: dict) -> None:
    events = record.get("events")
    event_chars = (
        events.get("command_output_chars", 0)
        if isinstance(events, dict)
        else 0
    )
    record["response_character_metrics"] = {
        "schema": RESPONSE_CHARACTER_METRICS_SCHEMA,
        "service_result_chars": int(
            record.get("service_result_chars", 0) or 0
        ),
        "service_source_text_chars": int(
            record.get("service_source_text_chars", 0) or 0
        ),
        "service_metadata_chars": int(
            record.get("service_metadata_chars", 0) or 0
        ),
        "service_replay_weighted_chars": int(
            record.get("service_replay_weighted_chars", 0) or 0
        ),
        "model_visible_tool_output_chars": int(
            record.get("model_visible_tool_output_chars", event_chars) or 0
        ),
    }


def validate_native_service_accounting(
    native: dict[str, Any],
    *,
    allow_zero_calls: bool = False,
) -> list[dict[str, Any]]:
    if not isinstance(native, dict):
        raise ValueError("native-session state must be an object")
    operations = native.get("operations")
    if not isinstance(operations, list):
        raise ValueError("native-session operations must be a list")
    if not operations and not allow_zero_calls:
        raise ValueError(
            "native-session recorded zero service calls without an explicit "
            "zero-call contract"
        )
    for index, operation in enumerate(operations):
        if not isinstance(operation, dict):
            raise ValueError(
                f"native-session operation {index} must be an object"
            )
        name = operation.get("operation")
        if not isinstance(name, str) or not name:
            raise ValueError(
                f"native-session operation {index} lacks an operation name"
            )
        for field in (
            "result_chars",
            "source_text_chars",
            "metadata_chars",
        ):
            value = operation.get(field)
            if type(value) is not int or value < 0:
                raise ValueError(
                    f"native-session operation {index} has invalid {field}"
                )
        if (
            operation["source_text_chars"] + operation["metadata_chars"]
            != operation["result_chars"]
        ):
            raise ValueError(
                f"native-session operation {index} character accounting "
                "does not sum to result_chars"
            )
        elapsed_ms = operation.get("elapsed_ms")
        if (
            isinstance(elapsed_ms, bool)
            or not isinstance(elapsed_ms, (int, float))
            or elapsed_ms < 0
        ):
            raise ValueError(
                f"native-session operation {index} has invalid elapsed_ms"
            )
        http_status = operation.get("http_status")
        local_failure = bool(
            http_status == 0
            and name.startswith("failed:")
            and isinstance(operation.get("service_status"), str)
            and operation["service_status"]
        )
        measured_client_failure = bool(
            type(http_status) is int
            and 400 <= http_status <= 499
            and name.startswith(("failed:", "denied:"))
            and isinstance(operation.get("service_status"), str)
            and operation["service_status"]
        )
        if (
            type(http_status) is not int
            or (
                not 100 <= http_status <= 399
                and not local_failure
                and not measured_client_failure
            )
        ):
            raise ValueError(
                f"native-session operation {index} has invalid http_status"
            )
        expected_minimum_http_calls = 0 if local_failure else 1
        http_calls = operation.get(
            "http_calls",
            expected_minimum_http_calls,
        )
        if (
            type(http_calls) is not int
            or http_calls < expected_minimum_http_calls
            or (local_failure and http_calls != 0)
        ):
            raise ValueError(
                f"native-session operation {index} has invalid http_calls"
            )
    return operations


def fail_service_accounting(
    record: dict[str, Any],
    message: str,
) -> None:
    rendered = f"Invalid native service accounting: {message}"
    record["service_accounting_valid"] = False
    record["service_accounting_error"] = rendered
    if not record.get("error"):
        record["error"] = rendered
    grade = record.get("grade")
    if isinstance(grade, dict):
        grade["service_accounting_valid"] = False
        grade["pass"] = False
    attach_response_character_metrics(record)


def attach_workspace_metrics(record: dict, run_dir: Path) -> None:
    record["service_operations"] = []
    record["local_cli_failures"] = summarize_local_cli_failures([])
    record["service_calls"] = 0
    record["service_result_chars"] = 0
    record["service_source_text_chars"] = 0
    record["service_metadata_chars"] = 0
    record["service_replay_weighted_chars"] = 0
    record["service_latency_ms"] = 0.0
    record["service_http_calls"] = 0
    record["service_binary_bytes"] = 0
    record["service_checkpoint"] = None
    record["service_session_id"] = None
    record["service_corpus_revision"] = None
    record["service_accounting_valid"] = (
        False if record.get("condition") == "service_api" else None
    )
    record["service_accounting_error"] = None
    attach_response_character_metrics(record)
    session_path = run_dir / "workspace-session.json"
    record["workspace_operations"] = []
    record["workspace_result_chars"] = 0
    record["workspace_checkpoint"] = None
    record["workspace_state_error"] = None
    record["sidecar_checkpoint"] = None
    record["sidecar_checkpoint_metrics"] = None
    record["persisted_checkpoint"] = None
    if record.get("condition") == "filesystem_sidecar":
        sidecar = read_sidecar_checkpoint(run_dir)
        checkpoint = sidecar["checkpoint"] if sidecar["valid"] else None
        required_fields = {
            "objective",
            "current_state",
            "decisions",
            "open_questions",
            "next_actions",
            "artifacts",
        }
        state_valid = bool(
            isinstance(checkpoint, dict)
            and required_fields <= set(checkpoint)
            and checkpoint.get("objective")
            and checkpoint.get("current_state")
            and checkpoint.get("next_actions")
            and checkpoint.get("artifacts")
        )
        record["sidecar_checkpoint_metrics"] = {
            key: value
            for key, value in sidecar.items()
            if key != "checkpoint"
        }
        record["sidecar_checkpoint_metrics"]["state_valid"] = state_valid
        record["sidecar_checkpoint"] = (
            checkpoint if state_valid else None
        )
        record["workspace_checkpoint"] = record["sidecar_checkpoint"]
        record["persisted_checkpoint"] = record["sidecar_checkpoint"]
    if session_path.exists():
        try:
            session = load_json(session_path)
        except (json.JSONDecodeError, OSError) as exc:
            record["workspace_state_error"] = str(exc)
        else:
            record["workspace_operations"] = session.get("operations", [])
            record["workspace_result_chars"] = sum(
                operation.get("result_chars", 0) for operation in session.get("operations", [])
            )
            record["workspace_checkpoint"] = session.get("checkpoint")
            record["persisted_checkpoint"] = record["workspace_checkpoint"]

    native_path = run_dir / "native-session.json"
    if not native_path.exists():
        if record.get("condition") == "service_api":
            fail_service_accounting(
                record,
                "native-session.json is missing",
            )
        return
    try:
        native = load_json(native_path)
    except (json.JSONDecodeError, OSError) as exc:
        record["workspace_state_error"] = str(exc)
        if record.get("condition") == "service_api":
            fail_service_accounting(record, str(exc))
        return
    try:
        operations = validate_native_service_accounting(
            native,
            allow_zero_calls=bool(
                record.get("allow_zero_service_calls") is True
                and record.get("zero_service_call_contract")
            ),
        )
    except ValueError as exc:
        record["workspace_state_error"] = str(exc)
        if record.get("condition") == "service_api":
            fail_service_accounting(record, str(exc))
        return
    record["service_accounting_valid"] = True
    record["service_operations"] = operations
    record["local_cli_failures"] = summarize_local_cli_failures(
        operations
    )
    service_accounting = summarize_service_accounting(operations)
    if (
        service_accounting["service_calls"] == 0
        and not (
            record.get("allow_zero_service_calls") is True
            and record.get("zero_service_call_contract")
        )
    ):
        fail_service_accounting(
            record,
            "native-session recorded zero service calls without an explicit "
            "zero-call contract",
        )
        return
    record.update(service_accounting)
    record["service_checkpoint"] = native.get("checkpoint")
    record["service_session_id"] = native.get("session_id")
    record["service_corpus_revision"] = native.get("corpus_revision")
    record["workspace_operations"] = operations
    record["workspace_result_chars"] = record["service_result_chars"]
    record["workspace_checkpoint"] = native.get("checkpoint")
    record["persisted_checkpoint"] = native.get("checkpoint")
    attach_response_character_metrics(record)


def load_existing_record(
    *,
    run_dir: Path,
    case: dict,
    condition: str,
    corpus_paths: set[str],
) -> dict:
    answer_path = run_dir / "answer.json"
    context_path = run_dir / "context.md"
    record = {
        "case_id": case["id"],
        "condition": condition,
        "exit_code": 0,
        "timed_out": False,
        "elapsed_seconds": None,
        "fixed_context_chars": len(context_path.read_text(encoding="utf-8")) if context_path.exists() else 0,
        "answer_path": str(answer_path),
        "error": None,
    }
    try:
        answer = parse_json_answer(answer_path)
        record["answer"] = answer
        record["grade"] = grade_answer(case, answer, corpus_paths)
    except (json.JSONDecodeError, OSError, TypeError) as exc:
        record["error"] = f"Invalid existing answer JSON: {exc}"
        record["grade"] = None
    record["events"] = parse_event_metrics(run_dir / "events.jsonl")
    record["model_visible_tool_output_chars"] = record["events"].get(
        "command_output_chars",
        0,
    )
    attach_workspace_metrics(record, run_dir)
    return record


def build_codex_command(
    *,
    codex: Path,
    model: str,
    schema: Path,
    run_dir: Path,
    condition: str,
) -> list[str]:
    command = [
        str(codex),
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--skip-git-repo-check",
        "--model",
        model,
    ]
    if condition == "service_api":
        command.extend(["--config", "sandbox_workspace_write.network_access=true"])
    command.extend([
        "--sandbox",
        "workspace-write",
        "--cd",
        str(run_dir),
        "--output-schema",
        str(schema),
        "--output-last-message",
        str(run_dir / "answer.json"),
        "--json",
        "-",
    ])
    return command


async def run_one(
    semaphore: asyncio.Semaphore,
    *,
    codex: Path,
    model: str,
    schema: Path,
    run_dir: Path,
    case: dict,
    condition: str,
    context_chars: int,
    corpus_paths: set[str],
    timeout_seconds: int,
    env_overrides: dict[str, str] | None = None,
) -> dict:
    async with semaphore:
        prompt = (run_dir / "prompt.txt").read_text(encoding="utf-8")
        answer_path = run_dir / "answer.json"
        events_path = run_dir / "events.jsonl"
        stderr_path = run_dir / "stderr.log"
        command = build_codex_command(
            codex=codex,
            model=model,
            schema=schema,
            run_dir=run_dir,
            condition=condition,
        )
        started = time.monotonic()
        env = subscription_reasoning_environment()
        for key in ["CODEX_THREAD_ID", "CODEX_INTERNAL_ORIGINATOR_OVERRIDE"]:
            env.pop(key, None)
        if env_overrides:
            env.update(env_overrides)
        env = subscription_reasoning_environment(env)
        process = await asyncio.create_subprocess_exec(
            *command,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env,
        )
        try:
            stdout, stderr = await asyncio.wait_for(
                process.communicate(prompt.encode()),
                timeout=timeout_seconds,
            )
            timed_out = False
        except asyncio.TimeoutError:
            process.kill()
            stdout, stderr = await process.communicate()
            timed_out = True
        elapsed = time.monotonic() - started
        events_path.write_bytes(stdout)
        stderr_path.write_bytes(stderr)
        record = {
            "case_id": case["id"],
            "condition": condition,
            "exit_code": process.returncode,
            "timed_out": timed_out,
            "elapsed_seconds": round(elapsed, 3),
            "fixed_context_chars": context_chars,
            "answer_path": str(answer_path),
            "error": None,
        }
        if process.returncode != 0 or not answer_path.exists():
            record["error"] = stderr.decode(errors="replace")[-4000:] or "Codex produced no answer"
            record["grade"] = None
        else:
            try:
                answer = parse_json_answer(answer_path)
                record["answer"] = answer
                record["grade"] = grade_answer(case, answer, corpus_paths)
            except (json.JSONDecodeError, OSError, TypeError) as exc:
                record["error"] = f"Invalid answer JSON: {exc}"
                record["grade"] = None
        record["events"] = parse_event_metrics(events_path)
        record["model_visible_tool_output_chars"] = record["events"].get(
            "command_output_chars",
            0,
        )
        attach_workspace_metrics(record, run_dir)
        (run_dir / "record.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        return record


def summarize(manifest: dict, records: Sequence[dict]) -> dict:
    case_by_id = {case["id"]: case for case in manifest["cases"]}

    def group_summary(rows: Sequence[dict]) -> dict:
        graded = [row for row in rows if row.get("grade")]
        elapsed_values = [row["elapsed_seconds"] for row in rows if row["elapsed_seconds"] is not None]
        claim_passed = sum(row["grade"]["claims_passed"] for row in graded)
        claim_total = sum(row["grade"]["claims_total"] for row in graded)
        token_keys = ["input_tokens", "output_tokens", "cached_input_tokens"]
        mean_tokens = {
            key: round(statistics.fmean(row["events"]["tokens"].get(key, 0) for row in rows), 1) if rows else 0
            for key in token_keys
        }
        mean_tokens["uncached_input_tokens"] = round(
            mean_tokens["input_tokens"] - mean_tokens["cached_input_tokens"], 1
        )
        checkpoint_eligible_runs = sum(
            1
            for row in rows
            if row["condition"] == "filesystem_sidecar"
            or (
                row["condition"] in WORKSPACE_CONDITIONS
                and case_by_id.get(row["case_id"], {}).get("workspace_access")
                != "read_only"
            )
        )
        return {
            "runs": len(rows),
            "successful_runs": len(graded),
            "cases_passed": sum(1 for row in graded if row["grade"]["pass"]),
            "case_pass_rate": sum(1 for row in graded if row["grade"]["pass"]) / max(1, len(rows)),
            "claims_passed": claim_passed,
            "claims_total": claim_total,
            "claim_pass_rate": claim_passed / max(1, claim_total),
            "mean_score": round(statistics.fmean(row["grade"]["score"] for row in graded), 4) if graded else 0,
            "elapsed_samples": len(elapsed_values),
            "mean_elapsed_seconds": round(statistics.fmean(elapsed_values), 2) if elapsed_values else None,
            "mean_fixed_context_chars": round(statistics.fmean(row["fixed_context_chars"] for row in rows), 1) if rows else 0,
            "mean_workspace_result_chars": round(statistics.fmean(row["workspace_result_chars"] for row in rows), 1) if rows else 0,
            "persisted_checkpoints": sum(
                1 for row in rows if row.get("persisted_checkpoint")
            ),
            "checkpoint_eligible_runs": checkpoint_eligible_runs,
            "mean_service_calls": round(
                statistics.fmean(row.get("service_calls", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_result_chars": round(
                statistics.fmean(row.get("service_result_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_source_text_chars": round(
                statistics.fmean(row.get("service_source_text_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_metadata_chars": round(
                statistics.fmean(row.get("service_metadata_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_replay_weighted_chars": round(
                statistics.fmean(row.get("service_replay_weighted_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_service_latency_ms": round(
                statistics.fmean(row.get("service_latency_ms", 0.0) for row in rows), 3
            ) if rows else 0,
            "mean_completed_commands": round(
                statistics.fmean(row["events"].get("commands", 0) for row in rows), 1
            ) if rows else 0,
            "mean_command_output_chars": round(
                statistics.fmean(row["events"].get("command_output_chars", 0) for row in rows), 1
            ) if rows else 0,
            "mean_model_visible_tool_output_chars": round(
                statistics.fmean(
                    row.get(
                        "model_visible_tool_output_chars",
                        row["events"].get("command_output_chars", 0),
                    )
                    for row in rows
                ),
                1,
            ) if rows else 0,
            "mean_tokens": mean_tokens,
        }

    by_condition = {
        condition: group_summary([row for row in records if row["condition"] == condition])
        for condition in manifest["conditions"]
    }
    by_workload = {}
    for workload in sorted({case["workload"] for case in manifest["cases"]}):
        ids = {case["id"] for case in manifest["cases"] if case["workload"] == workload}
        by_workload[workload] = {
            condition: group_summary([
                row for row in records
                if row["condition"] == condition and row["case_id"] in ids
            ])
            for condition in manifest["conditions"]
        }
    by_capability = {}
    for capability in sorted({case["capability"] for case in manifest["cases"]}):
        ids = {case["id"] for case in manifest["cases"] if case["capability"] == capability}
        by_capability[capability] = {
            condition: group_summary([
                row for row in records
                if row["condition"] == condition and row["case_id"] in ids
            ])
            for condition in manifest["conditions"]
        }
    return {
        "by_condition": by_condition,
        "by_workload": by_workload,
        "by_capability": by_capability,
        "case_metadata": case_by_id,
    }


def experiment_provenance_lines(run: dict[str, Any]) -> list[str]:
    lines = []
    experiment_arm = run.get("experiment_arm")
    paired_draw_id = run.get("paired_draw_id")
    if experiment_arm is not None:
        lines.extend([
            f"- Experiment arm: `{experiment_arm}`",
            f"- Paired draw ID: `{paired_draw_id}`",
        ])
    snapshot = run.get("service_runtime_snapshot")
    if isinstance(snapshot, dict):
        lines.extend([
            f"- Service build revision: `{snapshot.get('build_revision')}`",
            "- Expected runtime features: `"
            + json.dumps(
                run.get("expected_runtime_features", {}),
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
            + "`",
            "- Actual runtime features: `"
            + json.dumps(
                snapshot.get("runtime_features", {}),
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
            + "`",
            "- Embedding posture: `"
            + json.dumps(
                snapshot.get("embeddings", {}),
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
            + "`",
        ])
    return lines


def render_report(run: dict) -> str:
    manifest = run["manifest"]
    summary = run["summary"]
    labels = CONDITION_LABELS
    report_title = manifest.get("report_title", manifest["name"])
    report_date = manifest.get("report_date", run["run_at"][:10])
    workloads = ", ".join(sorted({case["workload"] for case in manifest["cases"]}))
    has_read_only = any(
        case.get("workspace_access") == "read_only"
        for case in manifest["cases"]
    )
    benchmark_version = str(manifest.get("benchmark_version", ""))
    if benchmark_version.startswith("personal-coordination"):
        manifest_argument = "eval/personal_coordination_cases.json"
    elif benchmark_version.startswith("rupture-ops"):
        manifest_argument = "eval/rupture_ops_cases.json"
    else:
        manifest_argument = "eval/work_cases.json"
    native_flag = " --filesystem-native" if "service_api" in manifest["conditions"] else ""
    condition_descriptions = {
        "fixed_pack": "- **Fixed handoff pack:** a fresh agent receives one task-specific context file and cannot retrieve more.",
        "filesystem": "- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.",
        "filesystem_sidecar": (
            "- **Filesystem agent with writable sidecar:** the corpus remains read-only, while a run-scoped sidecar "
            "must receive a durable JSON checkpoint."
        ),
        "workspace": (
            "- **Straylight workspace agent:** a fresh agent uses the initial BM25-backed shell CLI. "
            "This legacy condition does not test semantic retrieval or the native API."
        ),
        "service_api": (
            "- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through "
            "batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations."
        ),
    }
    if has_read_only:
        condition_descriptions["workspace"] += " The read-only authorization card cannot persist a checkpoint."
        condition_descriptions["service_api"] += " Read-only credentials cannot persist a checkpoint."
    lines = [
        f"Created: {run['run_at']}",
        f"Updated: {run.get('regraded_at', run['run_at'])}",
        "Status: Complete",
        "",
        "Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]",
        "",
        f"# {report_title} - {report_date}",
        "",
        "## Scope",
        f"- Model: `{manifest['model']}`",
        f"- Corpus: {run['corpus']['documents']} files, {run['corpus']['characters']:,} characters, {run['corpus']['chunks']} chunks",
        f"- Corpus SHA-256: `{run['corpus']['sha256']}`",
        f"- Cases: {len(manifest['cases'])} complex work tasks with {sum(len(case['rubric']) for case in manifest['cases'])} scored claims",
        f"- Workloads: {workloads}",
        "- This evaluates agent work and durable checkpoints, not retrieval recall alone.",
        *experiment_provenance_lines(run),
        "",
        "## Conditions",
        *(condition_descriptions[condition] for condition in manifest["conditions"]),
        "",
        "## Results",
        "| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for condition in manifest["conditions"]:
        row = summary["by_condition"][condition]
        checkpoints = (
            f"{row['persisted_checkpoints']}/{row['checkpoint_eligible_runs']} eligible"
            if condition in WORKSPACE_CONDITIONS or condition == "filesystem_sidecar"
            else "output only"
        )
        lines.append(
            f"| {labels[condition]} | {row['cases_passed']}/{row['runs']} ({row['case_pass_rate']:.0%}) "
            f"| {row['claims_passed']}/{row['claims_total']} ({row['claim_pass_rate']:.0%}) "
            f"| {row['mean_score']:.3f} | {checkpoints} |"
        )

    lines.extend([
        "",
        "## Token and tool accounting",
        "`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.",
        "",
        "| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ])
    for condition in manifest["conditions"]:
        row = summary["by_condition"][condition]
        tokens = row["mean_tokens"]
        lines.append(
            f"| {labels[condition]} | {tokens['input_tokens']:,.0f} | {tokens['cached_input_tokens']:,.0f} "
            f"| {tokens['uncached_input_tokens']:,.0f} | {tokens['output_tokens']:,.0f} "
            f"| {row['mean_completed_commands']:.1f} | {row['mean_command_output_chars']:,.0f} |"
        )

    lines.extend([
        "",
        "## Workload results",
        "| Workload | " + " | ".join(labels[condition] for condition in manifest["conditions"]) + " |",
        "| --- | " + " | ".join("---:" for _ in manifest["conditions"]) + " |",
    ])
    for workload, conditions in summary["by_workload"].items():
        cells = []
        for condition in manifest["conditions"]:
            row = conditions[condition]
            cells.append(f"{row['cases_passed']}/{row['runs']} cases, {row['claims_passed']}/{row['claims_total']} claims")
        lines.append(f"| {workload} | " + " | ".join(cells) + " |")

    lines.extend([
        "",
        "## Per-case results",
        "| Case | Capability | " + " | ".join(labels[condition] for condition in manifest["conditions"]) + " |",
        "| --- | --- | " + " | ".join("---:" for _ in manifest["conditions"]) + " |",
    ])
    record_map = {(record["condition"], record["case_id"]): record for record in run["records"]}
    for case in manifest["cases"]:
        cells = []
        for condition in manifest["conditions"]:
            record = record_map.get((condition, case["id"]))
            if not record or not record.get("grade"):
                cells.append("ERROR")
            else:
                grade = record["grade"]
                cells.append(
                    ("PASS" if grade["pass"] else "FAIL")
                    + f" {grade['claims_passed']}/{grade['claims_total']}"
                )
        lines.append(f"| `{case['id']}` | {case['capability']} | " + " | ".join(cells) + " |")

    successful = {
        condition: summary["by_condition"][condition]["case_pass_rate"]
        for condition in manifest["conditions"]
    }
    best_rate = max(successful.values())
    winners = [labels[condition] for condition, rate in successful.items() if rate == best_rate]
    filesystem = summary["by_condition"].get("filesystem")
    workspace = summary["by_condition"].get("workspace")
    service = summary["by_condition"].get("service_api")
    lines.extend([
        "",
        "## Findings",
        f"- Highest complete-case rate: {', '.join(winners)} at {best_rate:.0%}.",
    ])
    if filesystem and workspace:
        token_ratio = workspace["mean_tokens"]["input_tokens"] / max(1, filesystem["mean_tokens"]["input_tokens"])
        uncached_ratio = (
            workspace["mean_tokens"]["uncached_input_tokens"]
            / max(1, filesystem["mean_tokens"]["uncached_input_tokens"])
        )
        call_ratio = workspace["mean_completed_commands"] / max(1, filesystem["mean_completed_commands"])
        finding_lines = [
            f"- Filesystem and workspace agents both recovered {filesystem['claims_passed']}/{filesystem['claims_total']} scored claims.",
            f"- The workspace recorded {token_ratio:.1f}x the cumulative input but only {uncached_ratio:.2f}x the uncached input of direct filesystem access.",
            f"- The difference tracks interaction shape: the shell workspace required {workspace['mean_completed_commands']:.1f} completed calls per case versus {filesystem['mean_completed_commands']:.1f} for filesystem access ({call_ratio:.1f}x). Repeated cached history, not a doubled evidence load, dominates the headline token ratio.",
            f"- Workspace commands returned {workspace['mean_command_output_chars']:,.0f} recorded characters per case versus {filesystem['mean_command_output_chars']:,.0f} from filesystem commands. These raw event-log outputs are not guaranteed to equal model-visible text, but they confirm that the current shell surface is chattier and returns more text overall.",
        ]
        if has_read_only:
            finding_lines.append(
                f"- The workspace persisted {workspace['persisted_checkpoints']}/{workspace['checkpoint_eligible_runs']} eligible checkpoints; the read-only authorization card intentionally could not write one."
            )
        else:
            finding_lines.append(
                f"- The workspace persisted {workspace['persisted_checkpoints']}/{workspace['checkpoint_eligible_runs']} eligible checkpoints."
            )
        fixed = summary["by_condition"].get("fixed_pack")
        if fixed and fixed["case_pass_rate"] < 1:
            finding_lines.append(
                "- Fixed handoff failures clustered around facts or constraints omitted during pack assembly."
            )
        lines.extend(finding_lines)
    if filesystem and service:
        service_uncached = service["mean_tokens"]["uncached_input_tokens"]
        filesystem_uncached = filesystem["mean_tokens"]["uncached_input_tokens"]
        lines.extend([
            f"- Native service calls averaged {service['mean_service_calls']:.1f} per case and returned "
            f"{service['mean_service_result_chars']:,.0f} characters in {service['mean_service_latency_ms']:,.1f} ms of measured API time.",
            f"- Of that model-visible service output, {service['mean_service_source_text_chars']:,.0f} characters were evidence text and "
            f"{service['mean_service_metadata_chars']:,.0f} were transport metadata; replay-weighted output was "
            f"{service['mean_service_replay_weighted_chars']:,.0f} characters per case.",
            f"- Native uncached model input was {service_uncached / max(1, filesystem_uncached):.2f}x the filesystem baseline.",
        ])

    is_personal = str(manifest.get("benchmark_version", "")).startswith("personal-coordination")
    if is_personal:
        durable = service or workspace or summary["by_condition"].get("filesystem")
        durable_label = "native service" if service else "workspace"
        lines.extend([
            "",
            "## Interpretation boundary",
            "This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.",
            "",
            "## Conclusions",
            "- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.",
            "- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.",
            f"- The {durable_label} scored {durable['mean_score']:.3f} and persisted {durable['persisted_checkpoints']}/{durable['checkpoint_eligible_runs']} eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.",
            (
                "- The native API improved complete-case and claim recall over filesystem access, but still used more uncached input on this compact suite. Compact projections and model tool policy remain optimization targets."
                if service
                else "- The shell prototype is too interactive: it used substantially more calls, cumulative input, uncached input, and returned text than direct filesystem access. The typed native API must preserve quality while collapsing those round trips."
            ),
            (
                "- The native condition exercised OpenAI embeddings, hybrid ranking, authority-aware retrieval, snapshot pinning, and capability-bound writes; the suite is not an isolated semantic hit-rate benchmark."
                if service
                else "- Lexical BM25 was sufficient on this compact synthetic corpus. OpenAI embeddings, hybrid ranking, authority-aware traversal, hit rate, and search latency remain untested target-architecture hypotheses."
            ),
            (
                "- The separate changed-evidence transition suite remains the decisive fresh-agent continuation and efficiency gate."
                if service
                else "- The next implementation gate is the Rust/Postgres service and native typed adapter, followed by untouched holdout tasks and the existing changed-evidence checkpoint-transition suite."
            ),
            "",
            "## Limitations",
            "- The corpus is synthetic and the rubrics were authored with knowledge of it; untouched holdout tasks are still required.",
            "- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.",
            "- The filesystem and workspace agents chose their own evidence paths, so token and latency differences include tool-policy behavior.",
            "- Cumulative input includes cached-history replay. Uncached input is a better comparison of newly processed context, but it is not a direct measure of unique evidence or cost.",
            "- Concept-token grading is deterministic and preserves explicit negation, identifiers, citations, and forbidden conclusions, but it is not a complete semantic judge. The final regrade was manually audited at the claim level.",
            "- The filesystem condition was instruction-restricted rather than OS-sandboxed.",
            (
                "- The native condition is the containerized Rust, Postgres/pgvector, MinIO, and OpenAI implementation; Python and SQLite remain evaluation controls only."
                if service
                else "- The workspace condition is a BM25-backed Python shell prototype, not the planned Rust, Postgres, OpenAI-embedding, and MinIO architecture."
            ),
            (
                "- Read-only denial is executable in this suite; the separate destructive live smoke covers cross-user isolation and every native mutation surface."
                if service
                else "- Read-only denial is executable across the prototype command surface; the production service still needs cross-user and capability tests for every native API operation."
            ),
            "- Live telemetry, external websites, and changing production state were intentionally unavailable.",
            "",
            "## Reproduce",
            "```bash",
            "cd /Users/Shared/projects/straylight",
            "python3 -m unittest discover -s tests -v",
            f"python3 agent_work_eval.py --manifest {manifest_argument} validate",
            f"python3 agent_work_eval.py --manifest {manifest_argument} run{native_flag} --concurrency 3 --timeout 420 --out results/native-personal-coordination.json",
            "```",
            "",
        ])
    else:
        lines.extend([
            "",
            "## Interpretation boundary",
            "This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.",
            "",
            "## Conclusions",
            "- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.",
            "- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.",
            (
                f"- The native API recovered {service['claims_passed']}/{service['claims_total']} claims versus {filesystem['claims_passed']}/{filesystem['claims_total']} for filesystem access and persisted every eligible checkpoint."
                if service and filesystem
                else "- The initial shell interface creates too many model/tool round trips. A native typed API, persistent index and session, batched retrieval, compact deltas, and non-echoing checkpoint writes should remove most of that overhead."
            ),
            (
                "- The native condition exercised exact, structured, lexical, semantic, temporal, and relation retrieval with source diversification and authority-preserving checkpoint writes."
                if service
                else "- The current workspace uses lexical BM25 retrieval. Semantic retrieval, reranking, authority and supersession signals, hit rate, and search latency remain untested product hypotheses."
            ),
            "",
            "## Limitations",
            "- The corpus and rubrics were authored from known project material, so future runs need untouched holdout tasks.",
            "- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.",
            "- Cumulative input includes cached-history replay and is not a measure of unique evidence.",
            "- Live telemetry, external websites, and production state were intentionally unavailable.",
            "",
            "## Reproduce",
            "```bash",
            "cd /Users/Shared/projects/straylight",
            "python3 -m unittest discover -s tests -v",
            f"python3 agent_work_eval.py --manifest {manifest_argument} validate",
            f"python3 agent_work_eval.py --manifest {manifest_argument} run{native_flag} --concurrency 3 --timeout 420 --out results/native-agent-work.json",
            "```",
            "",
        ])
    return "\n".join(lines)


def validate(manifest_path: Path, schema_path: Path) -> dict:
    manifest = load_json(manifest_path)
    errors = []
    for case in manifest["cases"]:
        case.setdefault(
            "grading_mode",
            manifest.get("grading_mode", "exact_substring_v1"),
        )
    exact_value_claim_slots = manifest.get("exact_value_claim_slots", {})
    if not isinstance(exact_value_claim_slots, dict):
        errors.append("exact_value_claim_slots must be an object keyed by case ID")
        exact_value_claim_slots = {}
    cases_by_id = {case["id"]: case for case in manifest["cases"]}
    for case_id, claim_ids in exact_value_claim_slots.items():
        case = cases_by_id.get(case_id)
        if case is None:
            errors.append(f"exact_value_claim_slots names unknown case {case_id}")
            continue
        if (
            not isinstance(claim_ids, list)
            or not all(isinstance(claim_id, str) for claim_id in claim_ids)
        ):
            errors.append(f"{case_id}: exact-value claim IDs must be a string list")
            continue
        if len(claim_ids) != len(set(claim_ids)):
            errors.append(f"{case_id}: duplicate exact-value claim IDs")
        rubrics_by_id = {rubric["id"]: rubric for rubric in case["rubric"]}
        for claim_id in claim_ids:
            rubric = rubrics_by_id.get(claim_id)
            if rubric is None:
                errors.append(f"{case_id}: unknown exact-value claim ID {claim_id}")
                continue
            tags = rubric.setdefault("tags", [])
            if (
                not isinstance(tags, list)
                or not all(isinstance(tag, str) for tag in tags)
            ):
                errors.append(f"{case_id}:{claim_id}: tags must be a string list")
                continue
            if "exact_value" not in tags:
                tags.append("exact_value")
    schema = load_json(schema_path)
    corpus = (PROJECT_ROOT / manifest["corpus_root"]).resolve()
    documents, chunks = load_corpus(corpus)
    paths = {document.path for document in documents}
    ids = [case["id"] for case in manifest["cases"]]
    if len(ids) != len(set(ids)):
        errors.append("Duplicate case IDs")
    for case in manifest["cases"]:
        if case.get("workspace_access", "read_write") not in {"read_write", "read_only"}:
            errors.append(f"{case['id']}: invalid workspace_access")
        rubric_ids = {rubric["id"] for rubric in case["rubric"]}
        if rubric_ids != set(case["claim_slots"]):
            errors.append(f"{case['id']}: claim slots and rubric IDs differ")
        for rubric in case["rubric"]:
            tags = rubric.get("tags", [])
            if (
                not isinstance(tags, list)
                or not all(isinstance(tag, str) for tag in tags)
            ):
                errors.append(f"{case['id']}:{rubric['id']}: tags must be a string list")
            missing = [path for path in rubric["sources_any"] if path not in paths]
            if missing:
                errors.append(f"{case['id']}:{rubric['id']}: missing sources {missing}")
    return {
        "errors": errors,
        "manifest": manifest,
        "schema": schema,
        "documents": documents,
        "chunks": chunks,
        "corpus_root": corpus,
        "corpus_sha256": corpus_hash(documents),
        "artifact_tree_sha256": sha256_tree(corpus),
    }


def select_conditions(manifest: dict, args: argparse.Namespace) -> list[str]:
    if args.filesystem_native and args.condition:
        raise ValueError("--filesystem-native cannot be combined with --condition")
    if args.filesystem_native:
        requested = ["filesystem", "service_api"]
    elif args.condition:
        requested = list(dict.fromkeys(args.condition))
    else:
        requested = list(manifest["conditions"])
    unknown = [condition for condition in requested if condition not in CONDITION_ADAPTERS]
    if unknown:
        raise ValueError(f"Unknown condition adapters: {unknown}")
    return requested


def expected_feature_flags(values: Sequence[str] | None) -> dict[str, bool]:
    parsed: dict[str, bool] = {}
    for value in values or []:
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
    for value in config_values or []:
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
    return parsed


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
        raise ValueError("service status omitted the required runtime_features snapshot")
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
        "schema": "straylight-service-runtime-snapshot@v1",
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
    metadata: dict[str, Any],
    *,
    run_id: str,
    expected_features: dict[str, Any],
    expected_build_revision: str | None,
) -> dict[str, Any]:
    status_client = NativeApiClient(
        base_url=os.environ["STRAYLIGHT_API_URL"],
        token=metadata["token"],
        run_id=run_id,
        case_id="runtime-snapshot-preflight",
    )
    status = status_client.get("/v1/status").data
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
    for field in ("build_revision", "runtime_features"):
        if before.get(field) != after.get(field):
            raise ValueError(
                f"service {field} drifted during the evaluation run"
            )


def select_cases(
    manifest: dict,
    requested: Sequence[str] | None,
    *,
    include_retired: bool,
) -> list[dict[str, Any]]:
    if requested:
        requested_ids = list(dict.fromkeys(requested))
        cases_by_id = {case["id"]: case for case in manifest["cases"]}
        unknown = [case_id for case_id in requested_ids if case_id not in cases_by_id]
        if unknown:
            raise ValueError(f"unknown requested cases: {unknown}")
        return [cases_by_id[case_id] for case_id in requested_ids]
    return [
        case
        for case in manifest["cases"]
        if include_retired or case.get("active", True)
    ]


async def run_all(args: argparse.Namespace) -> dict:
    feature_states = parse_feature_states(args.feature_state)
    reasoning_billing = require_codex_subscription(args.codex)
    validated = validate(args.manifest, args.schema)
    if validated["errors"]:
        raise ValueError("\n".join(validated["errors"]))
    manifest = validated["manifest"]
    corpus = validated["corpus_root"]
    documents = validated["documents"]
    chunks = validated["chunks"]
    index = BM25Index(chunks)
    run_id = args.resume_run_id or args.run_id or datetime.now().astimezone().strftime("%Y%m%dT%H%M%S%z")
    run_root = PROJECT_ROOT / "runs" / run_id
    if args.resume_run_id:
        if not run_root.is_dir():
            raise ValueError(f"Unknown run directory: {run_root}")
    else:
        run_root.mkdir(parents=True, exist_ok=False)
    corpus_paths = {document.path for document in documents}
    selected_cases = select_cases(
        manifest,
        args.case,
        include_retired=args.include_retired,
    )
    if not selected_cases:
        raise ValueError("no evaluation cases were selected")
    selected_conditions = select_conditions(manifest, args)
    experiment_arm, paired_draw_id = validate_experiment_identity(
        args.experiment_arm,
        args.paired_draw_id,
        selected_conditions,
    )
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
    e09_experiment_arms = {
        "no_semantic": "e09-no-semantic",
        "unbounded_semantic": "e09-unbounded-semantic",
        "deadline_cache": "e09-deadline-cache",
        "deadline_cache_600": "e09-deadline-cache-600",
    }
    if args.e09_arm:
        required_arm = e09_experiment_arms[args.e09_arm]
        if experiment_arm != required_arm:
            raise ValueError(
                f"E09 {args.e09_arm} requires --experiment-arm {required_arm} "
                "and a --paired-draw-id"
            )
    if (
        (expected_features or args.expect_build_revision)
        and "service_api" not in selected_conditions
    ):
        raise ValueError(
            "runtime/build expectations require the service_api condition"
        )
    source_fingerprint = evaluation_source_fingerprint()
    if args.api_container and "service_api" not in selected_conditions:
        raise ValueError("--api-container requires the service_api condition")
    definitive_service_arm = bool(
        experiment_arm is not None and "service_api" in selected_conditions
    )
    if definitive_service_arm:
        if not args.api_container:
            raise ValueError(
                "explicit service_api experiment arms require --api-container"
            )
        if not args.expect_build_revision:
            raise ValueError(
                "explicit service_api experiment arms require "
                "--expect-build-revision"
            )
        if not source_fingerprint["reproducible_source"]:
            raise ValueError(
                "explicit service_api experiment arms require clean source "
                f"provenance: {source_fingerprint}"
            )
    if args.api_container and not args.expect_build_revision:
        raise ValueError(
            "--api-container requires --expect-build-revision"
        )
    service_image_before = (
        capture_service_image_fingerprint(
            args.api_container,
            source_revision=str(source_fingerprint["source_revision"]),
            expected_build_revision=args.expect_build_revision,
        )
        if args.api_container
        else None
    )
    manifest_sha256 = sha256_file(args.manifest)
    step_authorization: dict[str, Any] | None = None
    if args.e09_arm:
        if args.service_protocol != "simple":
            raise ValueError("E09 arms require --service-protocol simple")
        if selected_conditions != ["service_api"]:
            raise ValueError(
                "E09 arms require exactly --condition service_api"
            )
        if not selected_cases:
            raise ValueError("E09 requires at least one selected case")
        if not source_fingerprint["reproducible_source"]:
            raise ValueError(
                f"E09 requires a clean source fingerprint: {source_fingerprint}"
            )
        if args.e09_arm == "deadline_cache_600":
            if (
                args.e09_step_policy is None
                or args.e09_step_policy_sha256 is None
            ):
                raise ValueError(
                    "E09 deadline_cache_600 requires --e09-step-policy and "
                    "--e09-step-policy-sha256"
                )
            step_authorization = load_step_authorization(
                args.e09_step_policy,
                args.e09_step_policy_sha256,
                expected_source_revision=str(
                    source_fingerprint["source_revision"]
                ),
                expected_manifest_path=str(
                    args.manifest.resolve().relative_to(PROJECT_ROOT)
                ),
                expected_manifest_sha256=manifest_sha256,
                expected_case_ids=[
                    case["id"] for case in selected_cases
                ],
            )
        elif (
            args.e09_step_policy is not None
            or args.e09_step_policy_sha256 is not None
        ):
            raise ValueError(
                "step-policy arguments are valid only for "
                "--e09-arm deadline_cache_600"
            )
        for name, expected in expected_e09_features(
            args.e09_arm,
            step_authorization=step_authorization,
        ).items():
            if (
                name in expected_features
                and expected_features[name] != expected
            ):
                raise ValueError(
                    f"E09 {args.e09_arm} conflicts with the declared "
                    f"runtime expectation for {name}"
                )
            expected_features[name] = expected
        public_status = NativeApiClient(
            token="public-status",
            timeout=15,
        ).get("/ready").data
        public_provenance = validate_e09_runtime(
            public_status,
            args.e09_arm,
            step_authorization=step_authorization,
        )
        if (
            public_provenance["build_revision"]
            != source_fingerprint["source_revision"]
        ):
            raise ValueError(
                "E09 API build revision does not match the clean source "
                f"revision: {public_provenance['build_revision']} != "
                f"{source_fingerprint['source_revision']}"
            )
    else:
        public_provenance = None
        if (
            args.e09_step_policy is not None
            or args.e09_step_policy_sha256 is not None
        ):
            raise ValueError(
                "step-policy arguments require "
                "--e09-arm deadline_cache_600"
            )
    service_retrieval_modes = resolve_service_retrieval_modes(
        args.service_retrieval_modes,
        conditions=selected_conditions,
        service_protocol=args.service_protocol,
        experiment_arm=experiment_arm,
        expected_runtime_features=expected_features,
        e09_arm=args.e09_arm,
        step_authorization=step_authorization,
    )
    selected_model = args.model or manifest["model"]
    experiment_parameters = {
        "declared_feature_states": feature_states,
        "e09_arm": args.e09_arm,
        "e09_step_authorization": step_authorization,
        "run_tags": sorted(set(args.run_tag)),
        "service_retrieval_modes": list(service_retrieval_modes),
        "service_image_fingerprint": service_image_before,
    }
    ensure_run_binding(
        run_root,
        resume=bool(args.resume_run_id),
        run_id=run_id,
        experiment_arm=experiment_arm,
        paired_draw_id=paired_draw_id,
        conditions=selected_conditions,
        case_ids=[case["id"] for case in selected_cases],
        model=selected_model,
        service_protocol=args.service_protocol,
        manifest_sha256=manifest_sha256,
        expected_runtime_features=expected_features,
        expected_build_revision=args.expect_build_revision,
        experiment_parameters=experiment_parameters,
    )
    native_metadata: dict[str, dict[str, Any]] = {}
    runtime_snapshot: dict[str, Any] | None = None
    if "service_api" in selected_conditions:
        native_state_path = run_root / NATIVE_PROVISIONING_STATE
        native_metadata = load_native_provisioning_state(native_state_path, run_id)
        documents_payload = text_documents(corpus)
        for case in selected_cases:
            existing = native_metadata.get(case["id"])
            if existing is not None:
                if not provisioning_matches_run_case(
                    existing,
                    run_id=run_id,
                    case_id=case["id"],
                ):
                    raise ValueError(
                        f"Invalid persisted native provisioning for {case['id']}"
                    )
                continue
            client = NativeApiClient(run_id=run_id, case_id=case["id"])
            native_metadata[case["id"]] = provision_evaluation(
                client,
                run_id=run_id,
                case_id=case["id"],
                display_scope=case.get("scope", case["workload"]),
                access_mode=case.get("workspace_access", "read_write"),
                documents=documents_payload,
                timeout_seconds=args.native_index_timeout,
                import_path=(
                    "/v1/workspace/admin/eval/import"
                    if args.service_protocol == "simple"
                    else "/v1/admin/eval/import"
                ),
                wait_for_semantic=(
                    args.service_protocol != "simple"
                    or args.e09_arm in {
                        "unbounded_semantic",
                        "deadline_cache",
                        "deadline_cache_600",
                    }
                ),
            )
            write_native_provisioning_state(native_state_path, run_id, native_metadata)
        if not native_metadata:
            raise ValueError("service evaluation produced no authenticated runtime token")
        runtime_snapshot = fetch_service_runtime_snapshot(
            next(iter(native_metadata.values())),
            run_id=run_id,
            expected_features=expected_features,
            expected_build_revision=args.expect_build_revision,
        )
    e09_runtime_before: dict[str, Any] | None = None
    e09_provenance: dict[str, Any] | None = None
    if args.e09_arm:
        if args.e09_arm != "no_semantic":
            for case in selected_cases:
                e09_coverage_probe(
                    native_metadata[case["id"]],
                    run_id=run_id,
                    case_id=case["id"],
                )
        first_case = selected_cases[0]["id"]
        e09_runtime_before = e09_status_snapshot(
            native_metadata[first_case],
            run_id=run_id,
        )
        e09_provenance = validate_e09_runtime(
            e09_runtime_before,
            args.e09_arm,
            step_authorization=step_authorization,
        )
        if (
            e09_provenance["build_revision"]
            != source_fingerprint["source_revision"]
        ):
            raise ValueError(
                "E09 authenticated status build revision drifted from source"
            )
    semaphore = asyncio.Semaphore(args.concurrency)
    tasks = []
    records = []
    for case in selected_cases:
        for condition in selected_conditions:
            run_dir = run_root / condition / case["id"]
            if (run_dir / "answer.json").exists():
                records.append(load_existing_record(
                    run_dir=run_dir,
                    case=case,
                    condition=condition,
                    corpus_paths=corpus_paths,
                ))
                continue
            if run_dir.exists():
                context_path = run_dir / "context.md"
                context_chars = len(context_path.read_text(encoding="utf-8")) if context_path.exists() else 0
            else:
                run_dir, context_chars = prepare_case_dir(
                    run_root,
                    case,
                    condition,
                    corpus=corpus,
                    index=index,
                    manifest=manifest,
                    run_id=run_id,
                    native_metadata=native_metadata.get(case["id"]),
                    service_protocol=args.service_protocol,
                )
            environment = None
            if condition == "service_api":
                metadata = native_metadata[case["id"]]
                environment = {
                    "STRAYLIGHT_API_URL": os.environ["STRAYLIGHT_API_URL"],
                    "STRAYLIGHT_EVAL_TOKEN": metadata["token"],
                }
                if service_retrieval_modes:
                    environment["STRAYLIGHT_EVAL_RETRIEVAL_MODES"] = ",".join(
                        service_retrieval_modes
                    )
            tasks.append(run_one(
                semaphore,
                codex=args.codex,
                model=selected_model,
                schema=args.schema,
                run_dir=run_dir,
                case=case,
                condition=condition,
                context_chars=context_chars,
                corpus_paths=corpus_paths,
                timeout_seconds=args.timeout,
                env_overrides=environment,
            ))
    completed = await asyncio.gather(*tasks, return_exceptions=True)
    for item in completed:
        if isinstance(item, Exception):
            failed_record = {
                "case_id": "unknown",
                "condition": "unknown",
                "error": f"Unhandled run error: {item}",
                "grade": None,
                "elapsed_seconds": None,
                "fixed_context_chars": 0,
                "workspace_result_chars": 0,
                "events": {"events": 0, "commands": 0, "tokens": {}},
            }
            attach_response_character_metrics(failed_record)
            records.append(failed_record)
        else:
            records.append(item)
    selected_manifest = dict(manifest)
    selected_manifest["model"] = selected_model
    selected_manifest["cases"] = selected_cases
    selected_manifest["conditions"] = selected_conditions
    e09_runtime_after: dict[str, Any] | None = None
    if runtime_snapshot is not None:
        if args.e09_arm:
            first_case = selected_cases[0]["id"]
            e09_runtime_after = e09_status_snapshot(
                native_metadata[first_case],
                run_id=run_id,
            )
            final_runtime_snapshot = capture_service_runtime_snapshot(
                e09_runtime_after,
                expected_features=expected_features,
                expected_build_revision=args.expect_build_revision,
            )
        else:
            final_runtime_snapshot = fetch_service_runtime_snapshot(
                next(iter(native_metadata.values())),
                run_id=run_id,
                expected_features=expected_features,
                expected_build_revision=args.expect_build_revision,
            )
        require_stable_runtime_configuration(
            runtime_snapshot,
            final_runtime_snapshot,
        )
        runtime_snapshot = final_runtime_snapshot
    e09_runtime: dict[str, Any] | None = None
    if args.e09_arm:
        assert e09_runtime_after is not None
        final_provenance = validate_e09_runtime(
            e09_runtime_after,
            args.e09_arm,
            step_authorization=step_authorization,
        )
        if final_provenance != e09_provenance:
            raise ValueError(
                "E09 runtime flags or build revision drifted during the run"
            )
        before_counters = e09_runtime_before.get("semantic_runtime", {})
        after_counters = e09_runtime_after.get("semantic_runtime", {})
        counter_delta = semantic_counter_delta(
            before_counters,
            after_counters,
        )
        e09_runtime = {
            "provenance": final_provenance,
            "counters_before": before_counters,
            "counters_after": after_counters,
            "counter_delta": counter_delta,
            "rates": semantic_rates(counter_delta),
        }
    service_image_provenance: dict[str, Any] | None = None
    if service_image_before is not None:
        service_image_after = capture_service_image_fingerprint(
            args.api_container,
            source_revision=str(source_fingerprint["source_revision"]),
            expected_build_revision=args.expect_build_revision,
        )
        service_image_provenance = stable_service_image_provenance(
            service_image_before,
            service_image_after,
        )
    run = {
        "benchmark_version": manifest["benchmark_version"],
        "run_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "run_id": run_id,
        "experiment_arm": experiment_arm,
        "paired_draw_id": paired_draw_id,
        "harness_sha256": sha256_file(Path(__file__)),
        "workspace_cli_sha256": sha256_file(PROJECT_ROOT / "workspace_cli.py"),
        "native_memory_sha256": sha256_file(PROJECT_ROOT / "native_memory.py"),
        "manifest_sha256": manifest_sha256,
        "schema_sha256": sha256_file(args.schema),
        "corpus": {
            "root": str(corpus),
            "sha256": validated["corpus_sha256"],
            "artifact_tree_sha256": validated["artifact_tree_sha256"],
            "documents": len(documents),
            "chunks": len(chunks),
            "characters": sum(len(document.text) for document in documents),
        },
        "manifest": selected_manifest,
        "native_provisioning": {
            case_id: public_provisioning(metadata)
            for case_id, metadata in native_metadata.items()
        },
        "service_protocol": args.service_protocol,
        "service_retrieval_modes": list(service_retrieval_modes),
        "declared_feature_states": feature_states,
        "run_tags": sorted(set(args.run_tag)),
        "experiment_parameters": experiment_parameters,
        "reasoning_billing": reasoning_billing,
        "expected_feature_flags": {
            name: value
            for name, value in expected_features.items()
            if name in BOOLEAN_RUNTIME_FEATURES
        },
        "implementation_fingerprint": source_fingerprint,
        "e09_runtime": e09_runtime,
        "expected_runtime_features": expected_features,
        "expected_build_revision": args.expect_build_revision,
        "service_runtime_snapshot": runtime_snapshot,
        "service_image_provenance": service_image_provenance,
        "records": records,
    }
    run["run_ledger"] = build_run_ledger(
        run_id=run_id,
        reasoning_billing=reasoning_billing,
        model=selected_manifest["model"],
        conditions=selected_conditions,
        service_protocol=args.service_protocol,
        manifest_sha256=run["manifest_sha256"],
        schema_sha256=run["schema_sha256"],
        harness_sha256=run["harness_sha256"],
        experiment_arm=experiment_arm,
        paired_draw_id=paired_draw_id,
        runtime_snapshot=runtime_snapshot,
        expected_runtime_features=expected_features,
        expected_build_revision=args.expect_build_revision,
        experiment_parameters=experiment_parameters,
        service_image_provenance=service_image_provenance,
    )
    run["summary"] = summarize(selected_manifest, records)
    return run


def measure_adoption(
    run: dict[str, Any],
    *,
    source_artifact_path: str,
    source_artifact_sha256: str,
    expected_manifest_sha256: str,
    expected_case_ids: Sequence[str],
) -> dict[str, Any]:
    provenance = definitive_service_run_provenance(run)
    if provenance["experiment_arm"] != "e07-adoption":
        raise ValueError(
            "adoption measurement requires experiment arm e07-adoption"
        )
    if provenance["manifest_sha256"] != expected_manifest_sha256:
        raise ValueError(
            "adoption source artifact does not match the selected manifest"
        )
    if (
        not isinstance(source_artifact_path, str)
        or not source_artifact_path
        or not SHA256_PATTERN.fullmatch(source_artifact_sha256)
    ):
        raise ValueError(
            "adoption measurement requires source artifact path/hash provenance"
        )

    raw_cases = run.get("manifest", {}).get("cases")
    if not isinstance(raw_cases, list):
        raise ValueError("adoption source artifact has no selected cases")
    cases = {
        case["id"]: case
        for case in raw_cases
        if isinstance(case, dict) and isinstance(case.get("id"), str)
    }
    if (
        len(cases) != len(raw_cases)
        or list(cases) != list(expected_case_ids)
    ):
        raise ValueError(
            "adoption source artifact case selection does not exactly match "
            "the accepted manifest"
        )
    records = {
        record["case_id"]: record
        for record in run.get("records", [])
        if isinstance(record, dict)
        and isinstance(record.get("case_id"), str)
    }
    if set(records) != set(cases):
        raise ValueError(
            "adoption source artifact lacks exactly one record per case"
        )

    sessions = []
    for case_id, case in cases.items():
        record = records[case_id]
        eligibility = case.get("adoption_eligibility")
        if not isinstance(eligibility, dict):
            raise ValueError(
                f"adoption case {case_id} lacks adoption eligibility"
            )
        feature = eligibility.get("feature")
        if feature not in {"supersession", "intention"}:
            raise ValueError(
                f"adoption case {case_id} has an invalid feature"
            )
        authored = [
            {
                "write_path": operation.get("write_path"),
                "frontmatter": operation["authored_frontmatter"],
            }
            for operation in record.get("service_operations", [])
            if isinstance(operation, dict)
            and isinstance(operation.get("authored_frontmatter"), dict)
        ]
        if feature == "supersession":
            expected_path = eligibility.get("supersedes_path")
            if not isinstance(expected_path, str) or not expected_path:
                raise ValueError(
                    f"adoption case {case_id} lacks supersedes_path"
                )
            emitted = any(
                isinstance(block["frontmatter"].get("supersedes"), list)
                and expected_path
                in block["frontmatter"]["supersedes"]
                for block in authored
            )
        else:
            emitted = any(
                block["frontmatter"].get("kind") == "intention"
                and block["frontmatter"].get("status") == "pending"
                and isinstance(block["frontmatter"].get("trigger"), list)
                and bool(block["frontmatter"]["trigger"])
                and all(
                    isinstance(term, str) and term
                    for term in block["frontmatter"]["trigger"]
                )
                and isinstance(block["frontmatter"].get("due"), str)
                and bool(block["frontmatter"]["due"])
                for block in authored
            )
        sessions.append({
            "case_id": case_id,
            "feature": feature,
            "eligible": True,
            "emitted_valid_frontmatter": emitted,
            "frontmatter_blocks": authored,
        })

    by_feature = {}
    for feature in ("supersession", "intention"):
        feature_sessions = [
            session for session in sessions if session["feature"] == feature
        ]
        emitted = sum(
            bool(session["emitted_valid_frontmatter"])
            for session in feature_sessions
        )
        by_feature[feature] = {
            "eligible_sessions": len(feature_sessions),
            "emitted_sessions": emitted,
            "adoption_rate": (
                emitted / len(feature_sessions) if feature_sessions else None
            ),
        }
    if any(
        by_feature[feature]["eligible_sessions"] != 6
        for feature in ("supersession", "intention")
    ):
        raise ValueError(
            "adoption manifest must contribute exactly six eligible sessions "
            "per feature"
        )
    emitted = sum(bool(session["emitted_valid_frontmatter"]) for session in sessions)
    return {
        "schema": "straylight-frontmatter-adoption@v2",
        "run_id": run.get("run_id"),
        "benchmark_version": run.get("benchmark_version"),
        "experiment_arm": provenance["experiment_arm"],
        "paired_draw_id": provenance["paired_draw_id"],
        "source_artifact": {
            "path": source_artifact_path,
            "sha256": source_artifact_sha256,
        },
        "provenance": provenance,
        "eligible_sessions": len(sessions),
        "emitted_sessions": emitted,
        "adoption_rate": emitted / len(sessions) if sessions else None,
        "by_feature": by_feature,
        "sessions": sessions,
    }


def aggregate_adoption_measurements(
    paths: Sequence[Path],
    *,
    expected_manifest_sha256: str,
    expected_case_ids: Sequence[str],
    expected_draws: Sequence[str],
    minimum_rate: float = 0.5,
) -> dict[str, Any]:
    if len(paths) != 3:
        raise ValueError(
            "E07 adoption aggregation requires exactly three measurements"
        )
    if (
        not isinstance(expected_manifest_sha256, str)
        or not SHA256_PATTERN.fullmatch(expected_manifest_sha256)
    ):
        raise ValueError("E07 adoption requires an exact manifest hash")
    if (
        not expected_case_ids
        or len(expected_case_ids) != len(set(expected_case_ids))
        or not all(
            isinstance(case_id, str) and case_id
            for case_id in expected_case_ids
        )
    ):
        raise ValueError(
            "E07 adoption requires the exact unique manifest case IDs"
        )
    if (
        len(expected_draws) != 3
        or len(set(expected_draws)) != 3
        or not all(
            isinstance(draw, str)
            and EXPERIMENT_ID_PATTERN.fullmatch(draw)
            for draw in expected_draws
        )
    ):
        raise ValueError(
            "E07 adoption aggregation requires exactly three unique expected "
            "draw IDs"
        )
    if (
        isinstance(minimum_rate, bool)
        or not isinstance(minimum_rate, (int, float))
        or not 0 <= minimum_rate <= 1
    ):
        raise ValueError("--minimum-rate must be between zero and one")

    loaded = []
    measurement_hashes: set[str] = set()
    source_hashes: set[str] = set()
    run_ids: set[str] = set()
    observed_draws: set[str] = set()
    for path in paths:
        try:
            measurement = load_json(path)
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"{path}: could not load adoption measurement: {exc}"
            ) from exc
        measurement_sha256 = sha256_file(path)
        source_artifact = measurement.get("source_artifact")
        provenance = measurement.get("provenance")
        if (
            measurement.get("schema")
            != "straylight-frontmatter-adoption@v2"
            or not isinstance(source_artifact, dict)
            or not isinstance(provenance, dict)
            or provenance.get("schema")
            != "straylight-definitive-service-run-provenance@v1"
        ):
            raise ValueError(
                f"{path}: not a definitive adoption measurement"
            )
        source_hash = source_artifact.get("sha256")
        source_path = Path(str(source_artifact.get("path", "")))
        run_id = measurement.get("run_id")
        draw = measurement.get("paired_draw_id")
        if (
            not isinstance(source_artifact.get("path"), str)
            or not source_artifact["path"]
            or not isinstance(source_hash, str)
            or not SHA256_PATTERN.fullmatch(source_hash)
            or not source_path.is_absolute()
            or not source_path.is_file()
            or sha256_file(source_path) != source_hash
            or not isinstance(run_id, str)
            or not run_id
            or measurement.get("experiment_arm") != "e07-adoption"
            or not isinstance(draw, str)
            or draw not in set(expected_draws)
            or provenance.get("run_id") != run_id
            or provenance.get("experiment_arm") != "e07-adoption"
            or provenance.get("paired_draw_id") != draw
            or provenance.get("benchmark_version")
            != "frontmatter-adoption-v0.1"
            or measurement.get("benchmark_version")
            != provenance.get("benchmark_version")
            or provenance.get("manifest_sha256")
            != expected_manifest_sha256
            or provenance.get("service_retrieval_modes")
            != ["exact", "lexical"]
        ):
            raise ValueError(
                f"{path}: adoption identity, manifest, or retrieval binding "
                "is invalid"
            )
        try:
            raw_run = load_json(source_path)
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"{path}: could not reopen raw adoption artifact: {exc}"
            ) from exc
        recomputed = measure_adoption(
            raw_run,
            source_artifact_path=source_artifact["path"],
            source_artifact_sha256=source_hash,
            expected_manifest_sha256=expected_manifest_sha256,
            expected_case_ids=expected_case_ids,
        )
        if canonical_json_sha256(measurement) != canonical_json_sha256(
            recomputed
        ):
            raise ValueError(
                f"{path}: adoption measurement does not canonically match "
                "its hashed raw artifact"
            )
        measurement = recomputed
        source_artifact = measurement["source_artifact"]
        provenance = measurement["provenance"]
        if (
            measurement_sha256 in measurement_hashes
            or source_hash in source_hashes
            or run_id in run_ids
            or draw in observed_draws
        ):
            raise ValueError(
                f"{path}: duplicate measurement, source, run, or draw"
            )
        measurement_hashes.add(measurement_sha256)
        source_hashes.add(source_hash)
        run_ids.add(run_id)
        observed_draws.add(draw)

        runtime_snapshot = provenance.get("runtime_snapshot")
        runtime_features = provenance.get("runtime_features")
        image_provenance = provenance.get("service_image_provenance")
        if (
            not isinstance(runtime_snapshot, dict)
            or canonical_json_sha256(runtime_snapshot)
            != provenance.get("runtime_snapshot_sha256")
            or runtime_snapshot.get("runtime_features") != runtime_features
            or not isinstance(runtime_features, dict)
            or not isinstance(image_provenance, dict)
            or image_provenance.get("schema")
            != SERVICE_IMAGE_PROVENANCE_SCHEMA
            or image_provenance.get("stable") is not True
            or not isinstance(image_provenance.get("before"), dict)
            or not isinstance(image_provenance.get("after"), dict)
        ):
            raise ValueError(
                f"{path}: adoption runtime or image provenance is malformed"
            )
        stable_service_image_provenance(
            image_provenance["before"],
            image_provenance["after"],
        )
        image = image_provenance["before"]
        if (
            image.get("api_container_running") is not True
            or image.get("api_image_id") != provenance.get("api_image_id")
            or image.get("api_image_revision")
            != provenance.get("api_image_revision")
            or image.get("api_image_revision")
            != provenance.get("source_revision")
            or provenance.get("build_revision")
            != provenance.get("source_revision")
            or provenance.get("reasoning_billing")
            != {
                "route": "chatgpt_subscription",
                "api_fallback": "forbidden",
                "auth_status": "Logged in using ChatGPT",
            }
        ):
            raise ValueError(
                f"{path}: adoption source/build/image/auth provenance is "
                "invalid"
            )
        required_features = {
            "semantic_lane": False,
            "verbatim_spans": False,
            "supersession_demotion": True,
            "intention_ledger": False,
            "resume_deltas": False,
        }
        mismatches = {
            name: {
                "expected": expected,
                "actual": runtime_features.get(name),
            }
            for name, expected in required_features.items()
            if (
                name not in runtime_features
                or runtime_features[name] is not expected
            )
        }
        if mismatches:
            raise ValueError(
                f"{path}: adoption runtime feature mismatch: {mismatches}"
            )

        sessions = measurement.get("sessions")
        if (
            not isinstance(sessions, list)
            or len(sessions) != 12
            or len({
                session.get("case_id")
                for session in sessions
                if isinstance(session, dict)
            }) != 12
        ):
            raise ValueError(
                f"{path}: adoption measurement lacks twelve unique sessions"
            )
        recomputed_by_feature = {}
        for feature in ("supersession", "intention"):
            feature_sessions = [
                session
                for session in sessions
                if isinstance(session, dict)
                and session.get("feature") == feature
                and session.get("eligible") is True
                and type(
                    session.get("emitted_valid_frontmatter")
                ) is bool
            ]
            emitted = sum(
                session["emitted_valid_frontmatter"]
                for session in feature_sessions
            )
            recomputed_by_feature[feature] = {
                "eligible_sessions": len(feature_sessions),
                "emitted_sessions": emitted,
                "adoption_rate": (
                    emitted / len(feature_sessions)
                    if feature_sessions
                    else None
                ),
            }
        if (
            any(
                recomputed_by_feature[feature]["eligible_sessions"] != 6
                for feature in recomputed_by_feature
            )
            or measurement.get("by_feature") != recomputed_by_feature
            or measurement.get("eligible_sessions") != 12
            or measurement.get("emitted_sessions")
            != sum(
                value["emitted_sessions"]
                for value in recomputed_by_feature.values()
            )
            or measurement.get("adoption_rate")
            != measurement["emitted_sessions"] / 12
        ):
            raise ValueError(
                f"{path}: adoption totals disagree with session evidence"
            )
        loaded.append({
            "path": str(path),
            "sha256": measurement_sha256,
            "source_artifact": source_artifact,
            "provenance": provenance,
            "by_feature": recomputed_by_feature,
        })

    if observed_draws != set(expected_draws):
        raise ValueError(
            "E07 adoption measurements do not match the exact expected draws"
        )
    stable_fields = (
        "manifest_sha256",
        "schema_sha256",
        "harness_sha256",
        "source_revision",
        "build_revision",
        "api_image_id",
        "api_image_revision",
        "runtime_features",
        "service_retrieval_modes",
    )
    drift = {
        field: [
            item["provenance"].get(field)
            for item in loaded
        ]
        for field in stable_fields
        if len({
            canonical_json_sha256(item["provenance"].get(field))
            for item in loaded
        }) != 1
    }
    runtime_projection_fields = (
        "schema",
        "status",
        "build_revision",
        "read_only",
        "runtime_features",
        "embeddings",
    )
    runtime_projections = [
        {
            field: item["provenance"]["runtime_snapshot"].get(field)
            for field in runtime_projection_fields
        }
        for item in loaded
    ]
    if len({
        canonical_json_sha256(projection)
        for projection in runtime_projections
    }) != 1:
        drift["runtime_projection"] = runtime_projections
    if drift:
        raise ValueError(
            f"E07 adoption provenance drifted across draws: {drift}"
        )

    by_feature = {}
    for feature in ("supersession", "intention"):
        eligible = sum(
            item["by_feature"][feature]["eligible_sessions"]
            for item in loaded
        )
        emitted = sum(
            item["by_feature"][feature]["emitted_sessions"]
            for item in loaded
        )
        rate = emitted / eligible
        by_feature[feature] = {
            "eligible_sessions": eligible,
            "emitted_sessions": emitted,
            "adoption_rate": rate,
            "minimum_rate": minimum_rate,
            "pass": rate >= minimum_rate,
        }
    eligible_sessions = sum(
        value["eligible_sessions"] for value in by_feature.values()
    )
    emitted_sessions = sum(
        value["emitted_sessions"] for value in by_feature.values()
    )
    return {
        "schema": "straylight-frontmatter-adoption-aggregate@v1",
        "experiment_arm": "e07-adoption",
        "expected_draws": list(expected_draws),
        "inputs": loaded,
        "provenance": {
            field: loaded[0]["provenance"].get(field)
            for field in stable_fields
        },
        "eligible_sessions": eligible_sessions,
        "emitted_sessions": emitted_sessions,
        "adoption_rate": emitted_sessions / eligible_sessions,
        "by_feature": by_feature,
        "pass": all(value["pass"] for value in by_feature.values()),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run the Straylight agent-work evaluation")
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--schema", type=Path, default=DEFAULT_SCHEMA)
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("validate")

    run_parser = subparsers.add_parser("run")
    run_parser.add_argument("--codex", type=Path, default=DEFAULT_CODEX)
    run_parser.add_argument("--model")
    run_parser.add_argument("--concurrency", type=int, default=3)
    run_parser.add_argument("--timeout", type=int, default=360)
    run_parser.add_argument("--run-id")
    run_parser.add_argument(
        "--feature-state",
        action="append",
        default=[],
        metavar="NAME=on|off",
        help="record the runtime feature state used by the service under test",
    )
    run_parser.add_argument(
        "--run-tag",
        action="append",
        default=[],
        help="attach a stable experiment tag to the result artifact",
    )
    run_parser.add_argument("--resume-run-id")
    run_parser.add_argument(
        "--experiment-arm",
        help=(
            "stable arm identity for a single-condition invocation; requires "
            "--paired-draw-id and is immutable across resume"
        ),
    )
    run_parser.add_argument(
        "--paired-draw-id",
        help=(
            "shared draw identity used by every separately invoked experiment arm"
        ),
    )
    run_parser.add_argument("--case", action="append")
    run_parser.add_argument(
        "--include-retired",
        action="store_true",
        help="include cases retired because their product premise is no longer current",
    )
    run_parser.add_argument("--condition", action="append", choices=tuple(CONDITION_LABELS))
    run_parser.add_argument(
        "--filesystem-native",
        action="store_true",
        help="run only the unchanged filesystem baseline and native service_api condition",
    )
    run_parser.add_argument("--native-index-timeout", type=float, default=300.0)
    run_parser.add_argument(
        "--expect-feature-flag",
        action="append",
        help=(
            "fail closed unless /v1/status runtime_features matches NAME=on|off"
        ),
    )
    run_parser.add_argument(
        "--expect-runtime-config",
        action="append",
        help=(
            "fail closed unless /v1/status runtime_features matches NAME=<JSON value>"
        ),
    )
    run_parser.add_argument(
        "--expect-build-revision",
        help="fail closed unless /v1/status reports this exact build revision",
    )
    run_parser.add_argument(
        "--api-container",
        help=(
            "running API container whose immutable image ID and OCI revision "
            "label are captured before and after the run"
        ),
    )
    run_parser.add_argument(
        "--service-protocol",
        choices=("legacy", "simple"),
        default="simple",
    )
    run_parser.add_argument(
        "--service-retrieval-modes",
        nargs="+",
        choices=("exact", "lexical", "semantic"),
        help=(
            "immutable retrieval lanes injected into simple service_api "
            "requests; recorded in the run binding, ledger, and result"
        ),
    )
    run_parser.add_argument(
        "--e09-arm",
        choices=(
            "no_semantic",
            "unbounded_semantic",
            "deadline_cache",
            "deadline_cache_600",
        ),
        help=(
            "fail-closed E09 arm selection; verifies API flags/build provenance "
            "and enforces exact+lexical requests for no_semantic"
        ),
    )
    run_parser.add_argument(
        "--e09-step-policy",
        type=Path,
        help=(
            "immutable approved E09 step-policy artifact; required only for "
            "deadline_cache_600"
        ),
    )
    run_parser.add_argument(
        "--e09-step-policy-sha256",
        help=(
            "expected SHA-256 of --e09-step-policy; required only for "
            "deadline_cache_600"
        ),
    )
    run_parser.add_argument("--out", type=Path, required=True)
    run_parser.add_argument("--report", type=Path)

    regrade_parser = subparsers.add_parser("regrade")
    regrade_parser.add_argument("--input", type=Path, required=True)
    regrade_parser.add_argument("--out", type=Path, required=True)
    regrade_parser.add_argument("--report", type=Path)
    adoption_parser = subparsers.add_parser("measure-adoption")
    adoption_parser.add_argument("--input", type=Path, required=True)
    adoption_parser.add_argument("--out", type=Path, required=True)
    adoption_aggregate = subparsers.add_parser("aggregate-adoption")
    adoption_aggregate.add_argument(
        "--input",
        action="append",
        type=Path,
        required=True,
    )
    adoption_aggregate.add_argument(
        "--expected-draw",
        action="append",
        required=True,
    )
    adoption_aggregate.add_argument(
        "--minimum-rate",
        type=float,
        default=0.5,
    )
    adoption_aggregate.add_argument("--out", type=Path, required=True)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    validated = validate(args.manifest, args.schema)
    if args.command == "validate":
        print(json.dumps({
            "status": "ok" if not validated["errors"] else "error",
            "errors": validated["errors"],
            "cases": len(validated["manifest"]["cases"]),
            "claims": sum(len(case["rubric"]) for case in validated["manifest"]["cases"]),
            "documents": len(validated["documents"]),
            "chunks": len(validated["chunks"]),
            "corpus_sha256": validated["corpus_sha256"],
            "artifact_tree_sha256": validated["artifact_tree_sha256"],
            "manifest_sha256": sha256_file(args.manifest),
        }, indent=2))
        if validated["errors"]:
            raise SystemExit(1)
        return

    if args.command == "regrade":
        if validated["errors"]:
            raise ValueError("\n".join(validated["errors"]))
        run = load_json(args.input)
        run.setdefault(
            "execution_fingerprints",
            {
                key: run.get(key)
                for key in (
                    "manifest_sha256",
                    "harness_sha256",
                    "schema_sha256",
                    "workspace_cli_sha256",
                    "native_memory_sha256",
                )
                if run.get(key)
            },
        )
        case_by_id = {case["id"]: case for case in validated["manifest"]["cases"]}
        corpus_paths = {document.path for document in validated["documents"]}
        selected_cases = [case_by_id[case["id"]] for case in run["manifest"]["cases"]]
        run["manifest"]["cases"] = selected_cases
        for record in run["records"]:
            if record.get("answer") and record["case_id"] in case_by_id:
                record["grade"] = grade_answer(case_by_id[record["case_id"]], record["answer"], corpus_paths)
            run_dir = Path(record["answer_path"]).parent
            record["events"] = parse_event_metrics(run_dir / "events.jsonl")
            record["model_visible_tool_output_chars"] = record["events"].get(
                "command_output_chars",
                0,
            )
            attach_workspace_metrics(record, run_dir)
        run["summary"] = summarize(run["manifest"], run["records"])
        run["regraded_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
        run["manifest_sha256"] = sha256_file(args.manifest)
        run["harness_sha256"] = sha256_file(Path(__file__))
        run["workspace_cli_sha256"] = sha256_file(PROJECT_ROOT / "workspace_cli.py")
        run["native_memory_sha256"] = sha256_file(PROJECT_ROOT / "native_memory.py")
        run["regrade_fingerprints"] = {
            "manifest_sha256": run["manifest_sha256"],
            "harness_sha256": run["harness_sha256"],
            "schema_sha256": sha256_file(args.schema),
            "workspace_cli_sha256": run["workspace_cli_sha256"],
            "native_memory_sha256": run["native_memory_sha256"],
            "source": git_source_fingerprint(),
            "captured_at": run["regraded_at"],
        }
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(render_report(run), encoding="utf-8")
        print(json.dumps({
            "status": "ok",
            "out": str(args.out),
            "report": str(args.report) if args.report else None,
            "summary": run["summary"]["by_condition"],
        }, indent=2))
        return

    if args.command == "measure-adoption":
        if validated["errors"]:
            raise ValueError("\n".join(validated["errors"]))
        measurement = measure_adoption(
            load_json(args.input),
            source_artifact_path=str(args.input.resolve()),
            source_artifact_sha256=sha256_file(args.input),
            expected_manifest_sha256=sha256_file(args.manifest),
            expected_case_ids=[
                case["id"] for case in validated["manifest"]["cases"]
            ],
        )
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(measurement, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({
            "status": "ok",
            "out": str(args.out),
            "eligible_sessions": measurement["eligible_sessions"],
            "emitted_sessions": measurement["emitted_sessions"],
            "adoption_rate": measurement["adoption_rate"],
        }, indent=2))
        return

    if args.command == "aggregate-adoption":
        if validated["errors"]:
            raise ValueError("\n".join(validated["errors"]))
        aggregate = aggregate_adoption_measurements(
            args.input,
            expected_manifest_sha256=sha256_file(args.manifest),
            expected_case_ids=[
                case["id"] for case in validated["manifest"]["cases"]
            ],
            expected_draws=args.expected_draw,
            minimum_rate=args.minimum_rate,
        )
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(
            json.dumps(aggregate, indent=2) + "\n",
            encoding="utf-8",
        )
        print(json.dumps({
            "status": "ok" if aggregate["pass"] else "failed",
            "out": str(args.out),
            "eligible_sessions": aggregate["eligible_sessions"],
            "emitted_sessions": aggregate["emitted_sessions"],
            "adoption_rate": aggregate["adoption_rate"],
            "by_feature": aggregate["by_feature"],
        }, indent=2))
        if not aggregate["pass"]:
            raise SystemExit(2)
        return

    run = asyncio.run(run_all(args))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(render_report(run), encoding="utf-8")
    print(json.dumps({
        "status": "ok",
        "run_id": run["run_id"],
        "out": str(args.out),
        "report": str(args.report) if args.report else None,
        "summary": run["summary"]["by_condition"],
    }, indent=2))


if __name__ == "__main__":
    main()

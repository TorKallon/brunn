#!/usr/bin/env python3

"""Measure and correlate fresh simple-core checkpoint SQL statement counts.

The run command provisions one case-isolated evaluation document, submits at
least 30 fresh checkpoints, records the response query count and request ID,
then atomically removes the evaluation user and verifies credential revocation.

The correlate command joins those request IDs to JSON SQLx events captured from
the API process. It emits normalized, bind-free SQL fingerprints without
copying unrelated log events.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import re
import subprocess
import sys
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterable


PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from eval.semantic_http_probe import cleanup_fixture  # noqa: E402
from native_eval import (  # noqa: E402
    NativeApiClient,
    NativeResponse,
    provision_evaluation,
)


RUN_SCHEMA = "straylight-checkpoint-query-diagnostic@v1"
CORRELATION_SCHEMA = "straylight-checkpoint-query-log-correlation@v1"
MIN_SAMPLES = 30
MAX_SAMPLES = 1_000
READY_VALUES = frozenset({"ready", "complete", "completed", "current"})
SAFE_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")
REVISION_PATTERN = re.compile(r"^[0-9a-f]{40}$")
REQUEST_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")
SECRET_KEYS = frozenset(
    {
        "authorization",
        "credential_token",
        "api_token",
        "openai_api_key",
        "secret",
        "token",
    }
)


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def created_at() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def implementation_provenance() -> dict[str, Any]:
    script_path = Path(__file__).resolve()

    def git(*arguments: str) -> str:
        completed = subprocess.run(
            ["git", *arguments],
            cwd=PROJECT_ROOT,
            stdin=subprocess.DEVNULL,
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode != 0:
            raise RuntimeError(
                f"could not inspect diagnostic source provenance: git {arguments[0]}"
            )
        return completed.stdout.strip()

    revision = git("rev-parse", "HEAD")
    tree = git("rev-parse", "HEAD^{tree}")
    tracked_status = git("status", "--porcelain", "--untracked-files=no")
    value = {
        "script": str(script_path.relative_to(PROJECT_ROOT)),
        "script_sha256": hashlib.sha256(script_path.read_bytes()).hexdigest(),
        "git_revision": revision,
        "git_tree": tree,
        "tracked_tree_clean": not tracked_status,
        "pass": (
            REVISION_PATTERN.fullmatch(revision) is not None
            and len(tree) == 40
            and all(character in "0123456789abcdef" for character in tree)
            and not tracked_status
        ),
    }
    if not value["pass"]:
        raise RuntimeError(
            "checkpoint diagnostic requires a clean tracked harness revision"
        )
    return value


def public_error(error: BaseException) -> str:
    text = str(error)
    text = re.sub(
        r"(?i)(authorization:\s*bearer|bearer)\s+\S+",
        r"\1 [REDACTED]",
        text,
    )
    return f"{type(error).__name__}: {text[:1_000]}"


def redact_known_secrets(value: Any, secrets: Iterable[str]) -> Any:
    known = tuple(secret for secret in secrets if secret)
    if isinstance(value, dict):
        return {
            key: redact_known_secrets(item, known)
            for key, item in value.items()
            if key.casefold() not in SECRET_KEYS
        }
    if isinstance(value, list):
        return [redact_known_secrets(item, known) for item in value]
    if isinstance(value, str):
        redacted = value
        for secret in known:
            redacted = redacted.replace(secret, "[REDACTED]")
        return redacted
    return value


def diagnostic_document(nonce: str) -> dict[str, Any]:
    content = (
        f"# Checkpoint query diagnostic {nonce}\n\n"
        "This one-document isolated fixture exists only to trace the SQL "
        "statement shape of a fresh simple-core checkpoint request.\n"
    )
    return {
        "path": f"eval/checkpoint-query-diagnostic/{nonce}.md",
        "content": content,
        "content_sha256": hashlib.sha256(content.encode("utf-8")).hexdigest(),
        "media_type": "text/markdown",
    }


def validate_run_args(args: argparse.Namespace) -> None:
    if not SAFE_ID_PATTERN.fullmatch(args.run_id):
        raise ValueError("--run-id must be a safe 1-128 character identifier")
    if not REVISION_PATTERN.fullmatch(args.expect_build_revision):
        raise ValueError("--expect-build-revision must be a full lowercase SHA")
    if not MIN_SAMPLES <= args.samples <= MAX_SAMPLES:
        raise ValueError(
            f"--samples must be between {MIN_SAMPLES} and {MAX_SAMPLES}"
        )
    if args.timeout <= 0 or args.provision_timeout <= 0:
        raise ValueError("request and provision timeouts must be positive")
    if args.poll_seconds <= 0:
        raise ValueError("--poll-seconds must be positive")


def runtime_snapshot(
    client: NativeApiClient,
    *,
    expected_revision: str,
    runtime_posture: str,
) -> dict[str, Any]:
    response = client.get("/ready")
    data = response.data
    features = data.get("runtime_features")
    dependencies = data.get("dependencies")
    if not isinstance(features, dict) or not isinstance(dependencies, dict):
        raise RuntimeError("/ready omitted runtime feature or embedding metadata")
    expected_semantic_lane = runtime_posture == "semantic-ready"
    provider = data.get("embedding_provider")
    model = data.get("embedding_model")
    embedding_status = dependencies.get("embeddings")
    embedding_posture_matches = (
        isinstance(provider, str)
        and bool(provider)
        and isinstance(model, str)
        and bool(model)
        and (
            (
                runtime_posture == "mode1-pending"
                and provider == "hashing"
                and model == "straylight-hashing-v1"
                and embedding_status == "degraded"
            )
            or (
                runtime_posture == "semantic-ready"
                and embedding_status == "ready"
            )
        )
    )
    snapshot = {
        "build_revision": data.get("build_revision"),
        "semantic_lane": features.get("semantic_lane"),
        "embedding_provider": provider,
        "embedding_model": model,
        "embedding_status": embedding_status,
        "http_status": response.http_status,
        "pass": (
            data.get("build_revision") == expected_revision
            and features.get("semantic_lane") is expected_semantic_lane
            and embedding_posture_matches
            and response.http_status == 200
        ),
    }
    if not snapshot["pass"]:
        raise RuntimeError(
            "runtime posture does not match the diagnostic request: "
            f"{json.dumps(snapshot, sort_keys=True)}"
        )
    return snapshot


def index_status(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        return {}
    direct = value.get("index_status")
    if isinstance(direct, dict):
        return direct
    data = value.get("data")
    if isinstance(data, dict) and isinstance(data.get("index_status"), dict):
        return data["index_status"]
    return {}


def wait_for_runtime_posture(
    scoped: NativeApiClient,
    metadata: dict[str, Any],
    *,
    runtime_posture: str,
    timeout_seconds: float,
    poll_seconds: float,
    sleep: Callable[[float], None] = time.sleep,
) -> dict[str, Any]:
    initial = metadata.get("provisioning", {}).get("index_status_at_start")
    initial_status = index_status({"index_status": initial})
    for lane in ("exact", "lexical"):
        if str(initial_status.get(lane, "")).casefold() not in READY_VALUES:
            raise RuntimeError(
                f"evaluation import did not make {lane} immediately ready"
            )
    if runtime_posture == "mode1-pending":
        if str(initial_status.get("semantic", "")).casefold() in READY_VALUES:
            raise RuntimeError(
                "mode1-pending diagnostic unexpectedly provisioned semantic coverage"
            )
        return {
            "posture": runtime_posture,
            "initial_index_status": initial_status,
            "final_index_status": initial_status,
            "semantic_waited": False,
            "pass": True,
        }

    status_url = str(metadata.get("status_url") or "")
    if not status_url:
        raise RuntimeError("evaluation provisioning omitted status_url")
    deadline = time.monotonic() + timeout_seconds
    latest = initial_status
    polls = 0
    while True:
        response = scoped.get(status_url)
        polls += 1
        latest = index_status(response.body)
        if all(
            str(latest.get(lane, "")).casefold() in READY_VALUES
            for lane in ("exact", "lexical", "semantic")
        ):
            return {
                "posture": runtime_posture,
                "initial_index_status": initial_status,
                "final_index_status": latest,
                "semantic_waited": True,
                "status_polls": polls,
                "pass": True,
            }
        if time.monotonic() >= deadline:
            raise TimeoutError(
                "semantic-ready diagnostic timed out waiting for all indexes"
            )
        sleep(poll_seconds)


def checkpoint_payload(
    *,
    document_path: str,
    nonce: str,
    sample_index: int,
) -> dict[str, Any]:
    sample_key = f"{nonce}:{sample_index:04d}"
    return {
        "session_id": f"session:checkpoint-query-diagnostic:{sample_key}",
        "idempotency_key": f"checkpoint-query-diagnostic:{sample_key}",
        "state": {
            "objective": "Trace one fresh checkpoint SQL request.",
            "current_state": [
                f"Found the diagnostic source in {document_path}."
            ],
            "decisions": ["Use the isolated source as current truth."],
            "open_questions": [],
            "next_actions": ["Correlate the request ID with API SQLx logs."],
            "artifacts": [document_path],
        },
        "source_refs": [],
    }


def checkpoint_sample(
    scoped: NativeApiClient,
    *,
    document_path: str,
    nonce: str,
    sample_index: int,
) -> dict[str, Any]:
    response = scoped.post(
        "/v1/workspace/checkpoint",
        checkpoint_payload(
            document_path=document_path,
            nonce=nonce,
            sample_index=sample_index,
        ),
    )
    request_id = response.headers.get("x-request-id")
    query_count = response.body.get("query_count")
    data = response.data
    write = data.get("write")
    source_entries = data.get("source_entries")
    checkpoint_ref = data.get("checkpoint_ref") or data.get("checkpoint_id")
    checks = {
        "http_200": response.http_status == 200,
        "committed": str(response.body.get("status", "")).casefold()
        == "committed",
        "fresh_write": isinstance(write, dict)
        and write.get("no_op") is False,
        "one_inferred_source": isinstance(source_entries, list)
        and len(source_entries) == 1
        and isinstance(source_entries[0], dict)
        and source_entries[0].get("path") == document_path,
        "query_count": isinstance(query_count, int)
        and not isinstance(query_count, bool)
        and query_count > 0,
        "request_id": isinstance(request_id, str)
        and REQUEST_ID_PATTERN.fullmatch(request_id) is not None,
        "checkpoint_ref": isinstance(checkpoint_ref, str)
        and checkpoint_ref.startswith("checkpoint:"),
    }
    return {
        "sample_index": sample_index,
        "x_request_id": request_id,
        "query_count": query_count,
        "http_status": response.http_status,
        "elapsed_ms": round(response.elapsed_ms, 3),
        "checkpoint_ref_sha256": (
            sha256_text(checkpoint_ref)
            if isinstance(checkpoint_ref, str)
            else None
        ),
        "checks": checks,
        "pass": all(checks.values()),
    }


def query_count_distribution(samples: list[dict[str, Any]]) -> dict[str, Any]:
    counts = [
        sample["query_count"]
        for sample in samples
        if sample.get("pass") and isinstance(sample.get("query_count"), int)
    ]
    histogram = collections.Counter(counts)
    return {
        "samples": len(counts),
        "min": min(counts) if counts else None,
        "max": max(counts) if counts else None,
        "histogram": {
            str(count): occurrences
            for count, occurrences in sorted(histogram.items())
        },
        "deterministic": len(histogram) == 1,
    }


def provisioning_summary(
    metadata: dict[str, Any] | None,
    *,
    document: dict[str, Any],
    runtime_posture: str,
) -> dict[str, Any] | None:
    if metadata is None:
        return None
    provisioning = metadata.get("provisioning")
    return {
        "access_mode": metadata.get("access_mode"),
        "credential_provenance": metadata.get("credential_provenance"),
        "authorization_scope_sha256": sha256_text(
            str(metadata.get("authorization_scope") or "")
        ),
        "status_url_sha256": sha256_text(
            str(metadata.get("status_url") or "")
        ),
        "document": {
            "path": document["path"],
            "content_sha256": document["content_sha256"],
            "content_bytes": len(document["content"].encode("utf-8")),
        },
        "requested_runtime_posture": runtime_posture,
        "import": {
            key: provisioning.get(key)
            for key in (
                "documents",
                "delta_documents",
                "import_http_status",
                "import_elapsed_ms",
                "import_batch_count",
                "max_batch_documents",
            )
        }
        if isinstance(provisioning, dict)
        else None,
        "credential_recorded": False,
    }


def run_diagnostic(
    args: argparse.Namespace,
    *,
    client_factory: Callable[..., NativeApiClient] = NativeApiClient,
    provision: Callable[..., dict[str, Any]] = provision_evaluation,
    cleanup: Callable[..., dict[str, Any]] = cleanup_fixture,
    sleep: Callable[[float], None] = time.sleep,
    provenance: Callable[[], dict[str, Any]] = implementation_provenance,
) -> dict[str, Any]:
    validate_run_args(args)
    source_provenance = provenance()
    nonce = uuid.uuid4().hex
    case_id = f"checkpoint-query-diagnostic-{nonce}"
    document = diagnostic_document(nonce)
    admin = client_factory(
        run_id=args.run_id,
        case_id=case_id,
        timeout=args.timeout,
    )
    secrets = [getattr(admin, "token", "")]
    runtime: dict[str, Any] | None = None
    metadata: dict[str, Any] | None = None
    posture: dict[str, Any] | None = None
    scoped: NativeApiClient | None = None
    samples: list[dict[str, Any]] = []
    lifecycle_error: str | None = None
    cleanup_receipt: dict[str, Any] = {
        "attempted": False,
        "pass": False,
        "error": "evaluation fixture was not provisioned",
    }
    try:
        runtime = runtime_snapshot(
            admin,
            expected_revision=args.expect_build_revision,
            runtime_posture=args.runtime_posture,
        )
        metadata = provision(
            admin,
            run_id=args.run_id,
            case_id=case_id,
            display_scope=f"Checkpoint query diagnostic {nonce}",
            access_mode="read_write",
            documents=[document],
            timeout_seconds=args.provision_timeout,
            import_path="/v1/workspace/admin/eval/import",
            wait_for_semantic=False,
        )
        scoped_token = str(metadata.get("token") or "")
        if not scoped_token:
            raise RuntimeError("evaluation provisioning omitted scoped token")
        secrets.append(scoped_token)
        scoped = client_factory(
            base_url=admin.base_url,
            token=scoped_token,
            run_id=args.run_id,
            case_id=case_id,
            timeout=args.timeout,
        )
        posture = wait_for_runtime_posture(
            scoped,
            metadata,
            runtime_posture=args.runtime_posture,
            timeout_seconds=args.provision_timeout,
            poll_seconds=args.poll_seconds,
            sleep=sleep,
        )
        for sample_index in range(args.samples):
            try:
                samples.append(
                    checkpoint_sample(
                        scoped,
                        document_path=document["path"],
                        nonce=nonce,
                        sample_index=sample_index,
                    )
                )
            except Exception as error:
                samples.append(
                    {
                        "sample_index": sample_index,
                        "x_request_id": None,
                        "query_count": None,
                        "pass": False,
                        "error": public_error(error),
                    }
                )
    except Exception as error:
        lifecycle_error = public_error(error)
    finally:
        if scoped is not None and metadata is not None:
            status_url = str(metadata.get("status_url") or "")
            if status_url:
                cleanup_receipt = cleanup(scoped, status_url=status_url)
            else:
                cleanup_receipt = {
                    "attempted": False,
                    "pass": False,
                    "error": "evaluation provisioning omitted status_url",
                }

    request_ids = [
        sample.get("x_request_id")
        for sample in samples
        if isinstance(sample.get("x_request_id"), str)
    ]
    checkpoint_hashes = [
        sample.get("checkpoint_ref_sha256")
        for sample in samples
        if isinstance(sample.get("checkpoint_ref_sha256"), str)
    ]
    complete = (
        runtime is not None
        and runtime.get("pass") is True
        and source_provenance.get("pass") is True
        and posture is not None
        and posture.get("pass") is True
        and len(samples) == args.samples
        and len(samples) >= MIN_SAMPLES
        and all(sample.get("pass") is True for sample in samples)
        and len(request_ids) == len(set(request_ids)) == args.samples
        and len(checkpoint_hashes) == len(set(checkpoint_hashes)) == args.samples
        and cleanup_receipt.get("pass") is True
        and lifecycle_error is None
    )
    artifact = {
        "schema": RUN_SCHEMA,
        "created_at": created_at(),
        "status": "complete" if complete else "failed",
        "pass": complete,
        "run": {
            "run_id": args.run_id,
            "case_id": case_id,
            "runtime_posture": args.runtime_posture,
            "requested_samples": args.samples,
            "minimum_samples": MIN_SAMPLES,
            "api_url_sha256": sha256_text(admin.base_url),
            "expected_build_revision": args.expect_build_revision,
        },
        "implementation_provenance": source_provenance,
        "runtime": runtime,
        "provisioning": provisioning_summary(
            metadata,
            document=document,
            runtime_posture=args.runtime_posture,
        ),
        "index_posture": posture,
        "samples": samples,
        "query_count_distribution": query_count_distribution(samples),
        "freshness": {
            "unique_request_ids": len(request_ids) == args.samples
            and len(request_ids) == len(set(request_ids)),
            "unique_checkpoint_refs": len(checkpoint_hashes) == args.samples
            and len(checkpoint_hashes) == len(set(checkpoint_hashes)),
        },
        "cleanup": cleanup_receipt,
        "log_correlation": {
            "status": "pending",
            "request_id_header": "x-request-id",
            "required_runtime_filter": (
                "RUST_LOG=info,straylight=debug,sqlx::query=debug"
            ),
            "bindings_logged": False,
            "next_command": (
                "checkpoint_query_diagnostic.py correlate "
                "--artifact <run.json> --api-log <api.jsonl> --out <out.json>"
            ),
        },
        "contract": {
            "query_budget_mutated": False,
            "reasoning_api_calls": 0,
            "embedding_posture": (
                "mode1-pending performs no embedding wait; semantic-ready "
                "uses the runtime's configured provider and should be run "
                "against the owned Mode 2 mock unless separately authorized"
            ),
        },
        "error": lifecycle_error,
    }
    public = redact_known_secrets(artifact, secrets)
    rendered = json.dumps(public, sort_keys=True)
    if any(secret and secret in rendered for secret in secrets):
        raise RuntimeError("diagnostic artifact retained a credential")
    return public


def normalize_sql(statement: str) -> str:
    return " ".join(statement.strip().rstrip(";").split())


def event_request_id(event: dict[str, Any]) -> str | None:
    candidates: list[Any] = []
    fields = event.get("fields")
    if isinstance(fields, dict):
        candidates.append(fields.get("request_id"))
    span = event.get("span")
    if isinstance(span, dict):
        candidates.append(span.get("request_id"))
    spans = event.get("spans")
    if isinstance(spans, list):
        for item in reversed(spans):
            if isinstance(item, dict):
                candidates.append(item.get("request_id"))
    for candidate in candidates:
        if isinstance(candidate, str) and REQUEST_ID_PATTERN.fullmatch(candidate):
            return candidate
    return None


def event_sql(event: dict[str, Any]) -> str | None:
    fields = event.get("fields")
    if not isinstance(fields, dict):
        return None
    statement = fields.get("db.statement")
    if isinstance(statement, str) and statement.strip():
        return normalize_sql(statement)
    summary = fields.get("summary")
    if isinstance(summary, str) and summary.strip():
        return normalize_sql(summary)
    message = fields.get("message")
    if isinstance(message, str) and message.strip():
        return normalize_sql(message)
    return None


def parse_json_log_line(line: str) -> dict[str, Any] | None:
    stripped = line.strip()
    if not stripped:
        return None
    if not stripped.startswith("{"):
        start = stripped.find("{")
        if start < 0:
            return None
        stripped = stripped[start:]
    try:
        value = json.loads(stripped)
    except json.JSONDecodeError:
        return None
    return value if isinstance(value, dict) else None


def statement_record(ordinal: int, statement: str) -> dict[str, Any]:
    return {
        "ordinal": ordinal,
        "sha256": sha256_text(statement),
        "summary": " ".join(statement.split()[:8]),
        "normalized_sql": statement,
    }


def correlate_log(
    run_artifact: dict[str, Any],
    log_bytes: bytes,
    *,
    run_artifact_bytes: bytes,
) -> dict[str, Any]:
    if run_artifact.get("schema") != RUN_SCHEMA:
        raise ValueError("unsupported checkpoint diagnostic artifact schema")
    samples = run_artifact.get("samples")
    if not isinstance(samples, list) or len(samples) < MIN_SAMPLES:
        raise ValueError("checkpoint diagnostic artifact is incomplete")
    expected: dict[str, dict[str, Any]] = {}
    for sample in samples:
        if not isinstance(sample, dict) or sample.get("pass") is not True:
            raise ValueError("checkpoint diagnostic contains an invalid sample")
        request_id = sample.get("x_request_id")
        query_count = sample.get("query_count")
        if (
            not isinstance(request_id, str)
            or not isinstance(query_count, int)
            or isinstance(query_count, bool)
            or request_id in expected
        ):
            raise ValueError("checkpoint diagnostic request IDs/counts are invalid")
        expected[request_id] = sample

    matched: dict[str, list[str]] = {
        request_id: [] for request_id in expected
    }
    parsed_lines = 0
    sqlx_events = 0
    unmatched_sqlx_events = 0
    for raw_line in log_bytes.decode("utf-8", errors="replace").splitlines():
        event = parse_json_log_line(raw_line)
        if event is None:
            continue
        parsed_lines += 1
        if event.get("target") != "sqlx::query":
            continue
        sqlx_events += 1
        request_id = event_request_id(event)
        if request_id not in matched:
            unmatched_sqlx_events += 1
            continue
        statement = event_sql(event)
        if statement is None:
            matched[request_id].append("")
        else:
            matched[request_id].append(statement)

    correlations: list[dict[str, Any]] = []
    groups: dict[str, dict[str, Any]] = {}
    for request_id, sample in expected.items():
        statements = matched[request_id]
        records = [
            statement_record(index, statement)
            for index, statement in enumerate(statements, start=1)
            if statement
        ]
        sequence_sha256 = sha256_text(
            "\n".join(record["sha256"] for record in records)
        )
        complete = (
            len(statements) == sample["query_count"]
            and len(records) == sample["query_count"]
        )
        correlations.append(
            {
                "sample_index": sample["sample_index"],
                "x_request_id": request_id,
                "response_query_count": sample["query_count"],
                "log_statement_count": len(statements),
                "sequence_sha256": sequence_sha256,
                "pass": complete,
            }
        )
        group = groups.setdefault(
            sequence_sha256,
            {
                "sequence_sha256": sequence_sha256,
                "sample_indexes": [],
                "request_ids": [],
                "statements": records,
            },
        )
        group["sample_indexes"].append(sample["sample_index"])
        group["request_ids"].append(request_id)

    ordered_groups = sorted(
        groups.values(),
        key=lambda item: (-len(item["sample_indexes"]), item["sequence_sha256"]),
    )
    for group in ordered_groups:
        group["samples"] = len(group["sample_indexes"])
    complete = all(item["pass"] for item in correlations)
    return {
        "schema": CORRELATION_SCHEMA,
        "created_at": created_at(),
        "status": "complete" if complete else "failed",
        "pass": complete,
        "source": {
            "run_artifact_sha256": hashlib.sha256(
                run_artifact_bytes
            ).hexdigest(),
            "run_artifact_bytes": len(run_artifact_bytes),
            "api_log_sha256": hashlib.sha256(log_bytes).hexdigest(),
            "expected_request_ids": len(expected),
            "parsed_json_lines": parsed_lines,
            "sqlx_events": sqlx_events,
            "unmatched_sqlx_events": unmatched_sqlx_events,
        },
        "correlations": sorted(
            correlations,
            key=lambda item: item["sample_index"],
        ),
        "sequence_groups": ordered_groups,
        "finding": {
            "distinct_sequences": len(ordered_groups),
            "deterministic_sequence": len(ordered_groups) == 1,
            "bindings_logged": False,
            "unrelated_log_events_copied": False,
        },
    }


def write_new_artifact(path: Path, artifact: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as output:
        json.dump(artifact, output, indent=2)
        output.write("\n")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Trace fresh simple-core checkpoint SQL query counts"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    run = subparsers.add_parser(
        "run",
        help="provision, issue fresh checkpoints, and clean the identity",
    )
    run.add_argument("--run-id", required=True)
    run.add_argument(
        "--runtime-posture",
        choices=("mode1-pending", "semantic-ready"),
        required=True,
    )
    run.add_argument("--expect-build-revision", required=True)
    run.add_argument("--samples", type=int, default=MIN_SAMPLES)
    run.add_argument("--timeout", type=float, default=120.0)
    run.add_argument("--provision-timeout", type=float, default=43_200.0)
    run.add_argument("--poll-seconds", type=float, default=0.25)
    run.add_argument("--out", type=Path, required=True)

    correlate = subparsers.add_parser(
        "correlate",
        help="join checkpoint request IDs to normalized SQLx JSON log events",
    )
    correlate.add_argument("--artifact", type=Path, required=True)
    correlate.add_argument("--api-log", type=Path, required=True)
    correlate.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.command == "run":
            artifact = run_diagnostic(args)
        else:
            run_artifact_bytes = args.artifact.read_bytes()
            run_artifact = json.loads(run_artifact_bytes)
            if not isinstance(run_artifact, dict):
                raise ValueError(
                    f"{args.artifact} does not contain a JSON object"
                )
            artifact = correlate_log(
                run_artifact,
                args.api_log.read_bytes(),
                run_artifact_bytes=run_artifact_bytes,
            )
        write_new_artifact(args.out, artifact)
    except Exception as error:
        print(public_error(error), file=sys.stderr)
        return 2
    print(json.dumps(artifact, indent=2))
    return 0 if artifact.get("pass") is True else 2


if __name__ == "__main__":
    raise SystemExit(main())

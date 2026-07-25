use std::{
    collections::HashMap,
    env,
    sync::atomic::{AtomicI64, Ordering},
    time::{Duration, Instant},
};

use axum::{
    extract::{MatchedPath, Request},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use metrics::{Label, counter, gauge, histogram};
use metrics_exporter_dogstatsd::DogStatsDBuilder;
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;

use crate::{
    db::AppState,
    models::{Coverage, ResponseStatus},
    read_service::{
        ComputeResult, ItemStatus, OpenResult, QueryBatchResult, ReadBatchResult, VerifyResult,
    },
};

static HTTP_IN_FLIGHT: AtomicI64 = AtomicI64::new(0);

pub fn init(component: &'static str) -> Result<bool, String> {
    if !env_bool("STRAYLIGHT_METRICS_ENABLED", false)? {
        return Ok(false);
    }

    let host = env::var("DD_AGENT_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_owned());
    let port = env::var("DD_DOGSTATSD_PORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "8125".to_owned());
    let address = env::var("STRAYLIGHT_DOGSTATSD_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{host}:{port}"));
    let flush_interval = Duration::from_secs(env_u64("STRAYLIGHT_METRICS_FLUSH_SECONDS", 3)?);
    let labels = vec![
        Label::new("env", env_default("DD_ENV", "development")),
        Label::new("service", env_default("DD_SERVICE", "straylight")),
        Label::new(
            "version",
            env_default("DD_VERSION", env!("CARGO_PKG_VERSION")),
        ),
        Label::new("component", component),
    ];

    DogStatsDBuilder::default()
        .with_remote_address(address)
        .map_err(|error| error.to_string())?
        .with_flush_interval(flush_interval)
        .with_global_labels(labels)
        .set_global_prefix("straylight")
        .with_telemetry(true)
        .with_histogram_sampling(true)
        .send_histograms_as_distributions(true)
        .install()
        .map_err(|error| error.to_string())?;

    describe_metrics();
    counter!("runtime.starts").increment(1);
    gauge!("runtime.info").set(1.0);
    Ok(true)
}

pub async fn http_middleware(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let request_bytes = content_length(request.headers());
    let in_flight = HTTP_IN_FLIGHT.fetch_add(1, Ordering::Relaxed) + 1;
    gauge!("http.in_flight").set(in_flight as f64);

    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed_ms = elapsed_ms(started);
    let status_code = response.status().as_u16().to_string();
    let status_class = status_class(response.status());

    counter!(
        "http.requests",
        "method" => method.clone(),
        "route" => route.clone(),
        "status_code" => status_code.clone(),
        "status_class" => status_class
    )
    .increment(1);
    histogram!(
        "http.request.duration_ms",
        "method" => method.clone(),
        "route" => route.clone(),
        "status_code" => status_code
    )
    .record(elapsed_ms);
    if let Some(bytes) = request_bytes {
        histogram!(
            "http.request.bytes",
            "method" => method.clone(),
            "route" => route.clone()
        )
        .record(bytes as f64);
    }
    if let Some(bytes) = content_length(response.headers()) {
        histogram!(
            "http.response.bytes",
            "method" => method,
            "route" => route
        )
        .record(bytes as f64);
    }

    let in_flight = HTTP_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed) - 1;
    gauge!("http.in_flight").set(in_flight.max(0) as f64);
    response
}

pub fn spawn_runtime_metrics(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            gauge!("runtime.alive").set(1.0);
            let transfer_available = state.transfer_limiter.available_permits();
            let transfer_in_use = state
                .config
                .max_concurrent_transfers
                .saturating_sub(transfer_available);
            gauge!("asset.transfer.permits", "state" => "available").set(transfer_available as f64);
            gauge!("asset.transfer.permits", "state" => "in_use").set(transfer_in_use as f64);
            record_pool(
                "auth",
                &state.auth_pool,
                state.config.database_max_connections,
            );
            record_pool(
                "read_write",
                &state.rw_pool,
                state.config.database_max_connections,
            );
            record_pool(
                "read_only",
                &state.ro_pool,
                state.config.database_max_connections,
            );
            if let Some(pool) = &state.admin_pool {
                record_pool("admin", pool, 4);
                if let Err(error) = record_queue_snapshot(pool).await {
                    counter!("telemetry.snapshot.errors", "snapshot" => "worker_queues")
                        .increment(1);
                    tracing::warn!(?error, "could not collect worker queue metrics");
                }
                if let Err(error) = record_asset_snapshot(pool).await {
                    counter!("telemetry.snapshot.errors", "snapshot" => "asset_storage")
                        .increment(1);
                    tracing::warn!(?error, "could not collect asset storage metrics");
                }
            }
            let database_ready = sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(&state.auth_pool)
                .await
                .is_ok();
            record_dependency_state("database", database_ready);
            let object_store_ready = state.object_store.health_check().await.is_ok();
            record_dependency_state("object_store", object_store_ready);
            let embedding_degraded = state.embedder.is_degraded();
            gauge!(
                "dependency.state",
                "dependency" => "embeddings",
                "state" => "ready"
            )
            .set(if embedding_degraded { 0.0 } else { 1.0 });
            gauge!(
                "dependency.state",
                "dependency" => "embeddings",
                "state" => "degraded"
            )
            .set(if embedding_degraded { 1.0 } else { 0.0 });
        }
    });
}

fn record_dependency_state(dependency: &'static str, ready: bool) {
    gauge!(
        "dependency.state",
        "dependency" => dependency,
        "state" => "ready"
    )
    .set(if ready { 1.0 } else { 0.0 });
    gauge!(
        "dependency.state",
        "dependency" => dependency,
        "state" => "unavailable"
    )
    .set(if ready { 0.0 } else { 1.0 });
}

pub fn record_operation(operation: &'static str, status: ResponseStatus, output: &impl Serialize) {
    let status = response_status(status);
    counter!(
        "operation.responses",
        "operation" => operation,
        "status" => status
    )
    .increment(1);
    if let Ok(bytes) = serde_json::to_vec(output) {
        histogram!(
            "operation.response.bytes",
            "operation" => operation,
            "status" => status
        )
        .record(bytes.len() as f64);
    }
}

pub fn record_open(result: &OpenResult) {
    record_coverage("open", &result.coverage);
    histogram!("retrieval.open.results").record(result.initial_evidence.len() as f64);
    histogram!("retrieval.open.hydrated_sources").record(result.hydrated_sources.len() as f64);
    histogram!("retrieval.open.hydrated_bytes").record(
        result
            .hydrated_sources
            .iter()
            .map(|source| source.text.len())
            .sum::<usize>() as f64,
    );
    counter!(
        "retrieval.open.sufficiency",
        "status" => bounded_value(
            result
                .retrieval_sufficiency
                .get("status")
                .and_then(Value::as_str),
            &["sufficient", "likely_sufficient", "insufficient", "unknown"]
        )
    )
    .increment(1);
    histogram!("retrieval.open.conflicts").record(result.conflicts.len() as f64);
    histogram!("retrieval.open.gaps").record(result.gaps.len() as f64);
    histogram!("retrieval.open.ambiguities").record(result.ambiguities.len() as f64);
}

pub fn record_query(result: &QueryBatchResult) {
    record_coverage("query", &result.coverage);
    histogram!("retrieval.query.batch_items").record(result.items.len() as f64);
    for item in &result.items {
        counter!(
            "retrieval.query.items",
            "status" => response_status(item.status)
        )
        .increment(1);
        histogram!(
            "retrieval.query.results",
            "status" => response_status(item.status)
        )
        .record(item.results.len() as f64);
        histogram!("retrieval.query.conflicts").record(item.conflicts.len() as f64);
        histogram!("retrieval.query.gaps").record(item.gaps.len() as f64);
        histogram!("retrieval.query.ambiguities").record(item.ambiguities.len() as f64);
    }
}

pub fn record_read(result: &ReadBatchResult) {
    record_coverage("read", &result.coverage);
    histogram!("read.batch_items").record(result.items.len() as f64);
    for item in &result.items {
        let status = item_status(item.status);
        counter!(
            "read.items",
            "status" => status,
            "view" => bounded_value(
                Some(item.view.as_str()),
                &[
                    "current_state", "structured", "outline", "full", "range", "neighbors",
                    "relationships", "history", "diff", "last_known_good", "materialize_scope"
                ]
            )
        )
        .increment(1);
        histogram!("read.returned_tokens", "status" => status)
            .record(item.truncation.returned_tokens as f64);
        if item.truncation.truncated {
            counter!("read.truncated", "status" => status).increment(1);
        }
    }
}

pub fn record_compute(result: &ComputeResult) {
    histogram!("compute.steps").record(result.steps.len() as f64);
    histogram!("compute.rows_returned").record(result.rows_returned as f64);
    histogram!("compute.estimated_tokens").record(result.estimated_tokens as f64);
    for step in &result.steps {
        counter!(
            "compute.step.results",
            "operator" => bounded_value(
                Some(step.operator.as_str()),
                &[
                    "catalog", "query", "search", "read", "batch_read", "neighbors",
                    "timeline", "history", "diff", "group", "aggregate", "traverse",
                    "resolve_identity", "expand_recurrence", "state_history", "impact",
                    "compare_applicability", "proximity", "gate_rollup"
                ]
            ),
            "status" => item_status(step.status)
        )
        .increment(1);
        histogram!(
            "compute.step.evidence_refs",
            "operator" => bounded_value(Some(step.operator.as_str()), &[
                "catalog", "query", "search", "read", "batch_read", "neighbors",
                "timeline", "history", "diff", "group", "aggregate", "traverse",
                "resolve_identity", "expand_recurrence", "state_history", "impact",
                "compare_applicability", "proximity", "gate_rollup"
            ])
        )
        .record(step.evidence_refs.len() as f64);
    }
}

pub fn record_verify(result: &VerifyResult) {
    record_coverage("verify", &result.coverage);
    histogram!("verify.claims").record(result.claims.len() as f64);
    for claim in &result.claims {
        counter!(
            "verify.classifications",
            "classification" => serialize_enum(&claim.classification)
        )
        .increment(1);
        histogram!(
            "verify.evidence",
            "classification" => serialize_enum(&claim.classification)
        )
        .record(claim.evidence.len() as f64);
        for check in &claim.structural_checks {
            counter!(
                "verify.structural_checks",
                "status" => bounded_value(
                    Some(check.status.as_str()),
                    &["passed", "failed", "unknown", "not_applicable"]
                )
            )
            .increment(1);
        }
    }
}

pub fn record_write(operation: &'static str, result: &Value) {
    let replayed = result
        .get("replayed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    counter!(
        "write.operations",
        "operation" => operation,
        "replayed" => if replayed { "true" } else { "false" }
    )
    .increment(1);
    let items = result
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    histogram!("write.batch_items", "operation" => operation).record(items.len() as f64);
    for item in items {
        counter!(
            "write.items",
            "operation" => operation,
            "action" => bounded_value(
                item.get("action").and_then(Value::as_str),
                &["create", "revise", "supersede", "retract", "tombstone"]
            ),
            "kind" => bounded_value(
                item.get("kind").and_then(Value::as_str),
                &[
                    "source", "evidence", "asset", "object", "claim", "relation",
                    "state_assignment", "state_transition", "temporal_spec",
                    "recurrence_spec", "policy", "identity_review", "import_receipt",
                    "tombstone", "checkpoint"
                ]
            ),
            "disposition" => bounded_value(
                item.get("disposition").and_then(Value::as_str),
                &[
                    "created", "committed", "revised", "superseded", "retracted",
                    "deduplicated", "no_op", "needs_review"
                ]
            )
        )
        .increment(1);
    }
    if !replayed && result.get("corpus_revision").is_some() {
        counter!("write.revisions", "operation" => operation).increment(1);
    }
}

pub fn record_capture(status: ResponseStatus, data: &Value) {
    let status = response_status(status);
    counter!("capture.outcomes", "status" => status).increment(1);
    let item_count = data
        .pointer("/draft/save_request/items")
        .or_else(|| data.get("items"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    histogram!("capture.items", "status" => status).record(item_count as f64);
    let issue_count = data
        .get("issues")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    histogram!("capture.issues", "status" => status).record(issue_count as f64);
    let model_status = bounded_value(
        data.pointer("/model/status").and_then(Value::as_str),
        &[
            "completed",
            "refused",
            "invalid_output",
            "degraded_unavailable",
        ],
    );
    counter!(
        "capture.model_outcomes",
        "capture_status" => status,
        "model_status" => model_status
    )
    .increment(1);
    if status == "committed" {
        record_write("capture", data);
    }
}

pub fn record_stage(upload_count: usize, upload_bytes: usize, result: &Value) {
    counter!("stage.operations").increment(1);
    histogram!("stage.uploads").record(upload_count as f64);
    histogram!("stage.upload_bytes").record(upload_bytes as f64);
    let entries = result
        .pointer("/inventory/entries")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    histogram!("stage.inventory_entries").record(entries.len() as f64);
    histogram!("stage.quarantined_entries").record(
        entries
            .iter()
            .filter(|entry| {
                entry
                    .get("quarantined")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count() as f64,
    );
    histogram!("stage.warnings").record(
        result
            .get("warnings")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default() as f64,
    );
}

pub fn record_dream_action(action: &'static str, result: &Value) {
    let status = bounded_value(
        result.get("status").and_then(Value::as_str),
        &[
            "queued",
            "running",
            "selecting",
            "opening_snapshot",
            "generating",
            "gating",
            "evaluating",
            "awaiting_review",
            "gated",
            "quarantined",
            "stale",
            "succeeded",
            "completed",
            "failed",
            "canceled",
            "rejected",
            "discarded",
            "promoted",
            "rolled_back",
            "enabled",
            "paused",
        ],
    );
    let kind = bounded_value(
        result
            .get("job_type")
            .or_else(|| result.get("job_kind"))
            .or_else(|| result.get("dream_kind"))
            .and_then(Value::as_str),
        &[
            "deterministic_maintenance",
            "deep_consolidation",
            "repair_only",
        ],
    );
    counter!(
        "dream.actions",
        "action" => action,
        "status" => status,
        "kind" => kind
    )
    .increment(1);
    if let Some(items) = result
        .pointer("/candidate/items")
        .or_else(|| result.get("items"))
        .and_then(Value::as_array)
    {
        histogram!("dream.candidate_items", "action" => action).record(items.len() as f64);
    }
}

pub fn record_dream_job(result: &Value, outcome: &'static str, started: Instant) {
    let status = bounded_value(
        result.get("status").and_then(Value::as_str),
        &[
            "queued",
            "selecting",
            "opening_snapshot",
            "generating",
            "gating",
            "evaluating",
            "awaiting_review",
            "quarantined",
            "stale",
            "failed",
            "canceled",
            "rejected",
            "discarded",
            "promoting",
            "monitoring",
            "promoted",
            "rolled_back",
        ],
    );
    let kind = bounded_value(
        result.get("job_type").and_then(Value::as_str),
        &[
            "deterministic_maintenance",
            "deep_consolidation",
            "repair_only",
        ],
    );
    counter!(
        "worker.jobs",
        "queue" => "dream",
        "job_kind" => kind,
        "result" => outcome,
        "status" => status
    )
    .increment(1);
    histogram!(
        "worker.job.duration_ms",
        "queue" => "dream",
        "job_kind" => kind,
        "result" => outcome,
        "status" => status
    )
    .record(elapsed_ms(started));
}

pub fn record_model_run(
    purpose: &'static str,
    model: &str,
    status: &str,
    usage: &Value,
    started: Instant,
) {
    let model = bounded_model(model);
    let status = bounded_value(
        Some(status),
        &[
            "completed",
            "refused",
            "invalid_output",
            "degraded_unavailable",
            "skipped",
            "not_requested",
        ],
    );
    counter!(
        "model.requests",
        "purpose" => purpose,
        "provider" => "openai",
        "model" => model.clone(),
        "status" => status
    )
    .increment(1);
    histogram!(
        "model.duration_ms",
        "purpose" => purpose,
        "provider" => "openai",
        "model" => model,
        "status" => status
    )
    .record(elapsed_ms(started));
    for (token_type, pointers) in [
        ("input", &["/input_tokens"][..]),
        ("output", &["/output_tokens"][..]),
        ("total", &["/total_tokens"][..]),
        (
            "cached_input",
            &[
                "/input_tokens_details/cached_tokens",
                "/input_token_details/cached_tokens",
            ][..],
        ),
        (
            "reasoning_output",
            &[
                "/output_tokens_details/reasoning_tokens",
                "/output_token_details/reasoning_tokens",
            ][..],
        ),
    ] {
        if let Some(value) = pointers
            .iter()
            .find_map(|pointer| usage.pointer(pointer).and_then(Value::as_u64))
        {
            histogram!(
                "model.tokens",
                "purpose" => purpose,
                "token_type" => token_type
            )
            .record(value as f64);
        }
    }
}

pub fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

pub fn record_retrieval_lane(
    lane: &'static str,
    candidate_count: usize,
    failed: bool,
    started: Instant,
) {
    let result = if failed { "failed" } else { "success" };
    counter!(
        "retrieval.lane.runs",
        "lane" => lane,
        "result" => result
    )
    .increment(1);
    histogram!(
        "retrieval.lane.duration_ms",
        "lane" => lane,
        "result" => result
    )
    .record(elapsed_ms(started));
    histogram!(
        "retrieval.lane.raw_candidates",
        "lane" => lane,
        "result" => result
    )
    .record(candidate_count as f64);
}

pub fn response_status(status: ResponseStatus) -> &'static str {
    match status {
        ResponseStatus::Complete => "complete",
        ResponseStatus::Partial => "partial",
        ResponseStatus::Degraded => "degraded",
        ResponseStatus::Ambiguous => "ambiguous",
        ResponseStatus::Stale => "stale",
        ResponseStatus::Inconsistent => "inconsistent",
        ResponseStatus::Committed => "committed",
        ResponseStatus::NoOp => "no_op",
        ResponseStatus::AcceptedProcessing => "accepted_processing",
        ResponseStatus::NeedsReview => "needs_review",
        ResponseStatus::Conflict => "conflict",
        ResponseStatus::RejectedByPolicy => "rejected_by_policy",
    }
}

pub fn item_status(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Complete => "complete",
        ItemStatus::Partial => "partial",
        ItemStatus::Unsupported => "unsupported",
        ItemStatus::Failed => "failed",
    }
}

fn record_coverage(operation: &'static str, coverage: &Coverage) {
    gauge!(
        "retrieval.coverage.absence_safe",
        "operation" => operation
    )
    .set(if coverage.absence_safe { 1.0 } else { 0.0 });
    for (search_state, partitions) in [
        ("searched", coverage.searched.as_slice()),
        ("unsearched", coverage.unsearched.as_slice()),
    ] {
        for partition in partitions {
            let lane = bounded_value(
                Some(partition.lane.as_str()),
                &[
                    "exact",
                    "structured",
                    "lexical",
                    "semantic",
                    "temporal",
                    "relations",
                    "query",
                    "read",
                    "continuation",
                    "materialize_scope",
                    "hard_gated_derived_view",
                    "structured_verify",
                    "evidence_verify",
                    "absence_verify",
                ],
            );
            let completeness = bounded_value(
                Some(partition.completeness.as_str()),
                &[
                    "complete",
                    "best_effort",
                    "unsearched",
                    "not_requested",
                    "failed",
                    "bounded",
                    "partial",
                    "unavailable",
                ],
            );
            counter!(
                "retrieval.lane.executions",
                "operation" => operation,
                "lane" => lane,
                "search_state" => search_state,
                "completeness" => completeness,
                "failed" => if partition.failure_reason.is_some() { "true" } else { "false" }
            )
            .increment(1);
            histogram!(
                "retrieval.lane.searched_records",
                "operation" => operation,
                "lane" => lane
            )
            .record(partition.searched_count as f64);
            histogram!(
                "retrieval.lane.candidates",
                "operation" => operation,
                "lane" => lane
            )
            .record(partition.candidate_count as f64);
        }
    }
}

fn record_pool(name: &'static str, pool: &sqlx::PgPool, max: u32) {
    let size = pool.size();
    let idle = pool.num_idle() as u32;
    let in_use = size.saturating_sub(idle);
    gauge!("db.pool.connections", "pool" => name, "state" => "open").set(size as f64);
    gauge!("db.pool.connections", "pool" => name, "state" => "idle").set(idle as f64);
    gauge!("db.pool.connections", "pool" => name, "state" => "in_use").set(in_use as f64);
    gauge!("db.pool.utilization", "pool" => name).set(if max == 0 {
        0.0
    } else {
        in_use as f64 / max as f64
    });
}

async fn record_queue_snapshot(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    const QUEUES: [(&str, &[&str]); 6] = [
        ("background", &["queued", "retry_wait", "running", "failed"]),
        (
            "dream",
            &[
                "queued",
                "selecting",
                "opening_snapshot",
                "generating",
                "gating",
                "evaluating",
                "awaiting_review",
                "failed",
            ],
        ),
        ("deletion", &["queued", "propagating", "failed", "blocked"]),
        (
            "account_export",
            &["queued", "running", "deleting", "failed"],
        ),
        (
            "account_deletion",
            &["queued", "running", "awaiting_backup_expiry", "failed"],
        ),
        ("asset_upload", &["uploading", "verifying", "failed"]),
    ];
    let rows = sqlx::query(
        r#"
        SELECT queue,status,count(*)::bigint AS depth,
               extract(epoch FROM clock_timestamp()-min(created_at))::float8 AS oldest_age_seconds
        FROM (
          SELECT 'background'::text AS queue,status,created_at
          FROM straylight.background_jobs
          WHERE status IN ('queued','retry_wait','running','failed')
          UNION ALL
          SELECT 'dream'::text AS queue,status,created_at
          FROM straylight.dream_jobs
          WHERE status IN (
            'queued','selecting','opening_snapshot','generating','gating',
            'evaluating','awaiting_review','failed'
          )
          UNION ALL
          SELECT 'deletion'::text AS queue,status,created_at
          FROM straylight.deletion_jobs
          WHERE status IN ('queued','propagating','failed','blocked')
          UNION ALL
          SELECT 'account_export'::text AS queue,status,created_at
          FROM straylight.account_exports
          WHERE status IN ('queued','running','deleting','failed')
          UNION ALL
          SELECT 'account_deletion'::text AS queue,status,created_at
          FROM straylight.account_deletion_requests
          WHERE status IN ('queued','running','awaiting_backup_expiry','failed')
          UNION ALL
          SELECT 'asset_upload'::text AS queue,status,created_at
          FROM straylight.asset_uploads
          WHERE status IN ('uploading','verifying','failed')
        ) pending
        GROUP BY queue,status
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut values = HashMap::new();
    for row in rows {
        values.insert(
            (
                row.try_get::<String, _>("queue")?,
                row.try_get::<String, _>("status")?,
            ),
            (
                row.try_get::<i64, _>("depth")?,
                row.try_get::<f64, _>("oldest_age_seconds")?,
            ),
        );
    }
    for (queue, statuses) in QUEUES {
        for status in statuses {
            let (depth, age) = values
                .get(&(queue.to_owned(), (*status).to_owned()))
                .copied()
                .unwrap_or((0, 0.0));
            gauge!("worker.queue.depth", "queue" => queue, "status" => *status).set(depth as f64);
            gauge!(
                "worker.queue.oldest_age_seconds",
                "queue" => queue,
                "status" => *status
            )
            .set(age.max(0.0));
        }
    }
    Ok(())
}

async fn record_asset_snapshot(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let row = sqlx::query(
        r#"
        WITH physical AS (
          SELECT user_id,object_key,max(size_bytes)::bigint AS size_bytes
          FROM straylight.asset_versions
          GROUP BY user_id,object_key
        )
        SELECT
          (SELECT count(*)::bigint FROM straylight.asset_versions) AS logical_versions,
          (SELECT coalesce(sum(size_bytes),0)::float8
             FROM straylight.asset_versions) AS logical_bytes,
          (SELECT count(*)::bigint FROM physical) AS physical_objects,
          (SELECT coalesce(sum(size_bytes),0)::float8 FROM physical) AS physical_bytes
        "#,
    )
    .fetch_one(pool)
    .await?;
    let logical_bytes = row.try_get::<f64, _>("logical_bytes")?;
    let physical_bytes = row.try_get::<f64, _>("physical_bytes")?;
    gauge!("asset.storage.logical_versions").set(row.try_get::<i64, _>("logical_versions")? as f64);
    gauge!("asset.storage.logical_bytes").set(logical_bytes);
    gauge!("asset.storage.physical_objects").set(row.try_get::<i64, _>("physical_objects")? as f64);
    gauge!("asset.storage.physical_bytes").set(physical_bytes);
    Ok(())
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

fn bounded_value(value: Option<&str>, allowed: &[&'static str]) -> &'static str {
    value
        .and_then(|value| allowed.iter().copied().find(|allowed| *allowed == value))
        .unwrap_or("other")
}

fn bounded_model(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        "unknown".to_owned()
    } else {
        model.chars().take(80).collect()
    }
}

fn serialize_enum(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn env_default(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => value
            .parse::<bool>()
            .map_err(|error| format!("invalid {name}: {error}")),
        Err(_) => Ok(default),
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|error| format!("invalid {name}: {error}")),
        Err(_) => Ok(default),
    }
}

fn describe_metrics() {
    metrics::describe_counter!("http.requests", "Completed HTTP requests.");
    metrics::describe_histogram!(
        "http.request.duration_ms",
        metrics::Unit::Milliseconds,
        "End-to-end HTTP request duration."
    );
    metrics::describe_counter!("api.errors", "Structured API errors by stable code.");
    metrics::describe_histogram!(
        "operation.response.bytes",
        metrics::Unit::Bytes,
        "Serialized reasoning or write response size."
    );
    metrics::describe_histogram!(
        "retrieval.lane.candidates",
        "Candidate records produced by one retrieval lane."
    );
    metrics::describe_histogram!(
        "model.duration_ms",
        metrics::Unit::Milliseconds,
        "OpenAI model request duration."
    );
    metrics::describe_gauge!(
        "db.pool.utilization",
        "Fraction of configured database connections currently in use."
    );
    metrics::describe_gauge!(
        "worker.queue.oldest_age_seconds",
        metrics::Unit::Seconds,
        "Age of the oldest pending durable job."
    );
    metrics::describe_gauge!(
        "asset.transfer.permits",
        "Configured binary-transfer permits by available or in-use state."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_dimensions_bucket_unbounded_values() {
        assert_eq!(
            bounded_value(Some("semantic"), &["exact", "semantic"]),
            "semantic"
        );
        assert_eq!(
            bounded_value(Some("user-created-lane"), &["exact", "semantic"]),
            "other"
        );
        assert_eq!(bounded_value(None, &["exact"]), "other");
    }

    #[test]
    fn response_statuses_are_stable_tags() {
        assert_eq!(response_status(ResponseStatus::Complete), "complete");
        assert_eq!(response_status(ResponseStatus::NeedsReview), "needs_review");
        assert_eq!(
            response_status(ResponseStatus::AcceptedProcessing),
            "accepted_processing"
        );
    }
}

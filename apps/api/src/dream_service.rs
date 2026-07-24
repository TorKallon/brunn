//! Phase-0 shadow dreaming.
//!
//! This module deliberately has no promotion operation. Dream processing may
//! create candidate corpus revisions, candidate manifests, audit records, and
//! review records, but it never advances an active manifest. Rollback is the
//! sole manifest mutation here and requires an existing promotion receipt.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::Instant,
};

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{
        Capability, CredentialId, DreamCreateRequest, DreamReviewDecision, DreamReviewRequest,
        ListQuery, UserId,
    },
    quota::{self, UsageReservation},
    refs::RecordRef,
    telemetry,
};

const PHASE: &str = "phase_0_shadow";
const DREAM_POLICY_VERSION: &str = "dream-policy:phase0-v1";
const PROMPT_VERSION: &str = "prompt:source-preserving-consolidation-v1";
const SCHEMA_VERSION: &str = "schema:dream-candidate-v2";
const EVALUATOR_VERSION: &str = "evaluator:paired-lexical-shadow-v1";
const RETRIEVAL_POLICY_VERSION: &str = "retrieval-policy:paired-lexical-shadow-v1";
const DEFAULT_MAX_RECORDS: usize = 10_000;
const MAX_MAX_RECORDS: usize = 100_000;
const DEFAULT_MAX_MODEL_INPUT_BYTES: usize = 240_000;
const MAX_MAX_MODEL_INPUT_BYTES: usize = 1_000_000;
const DEFAULT_MAX_MODEL_OUTPUT_TOKENS: u64 = 8_192;
const MAX_MAX_MODEL_OUTPUT_TOKENS: u64 = 32_768;
const DEFAULT_MAX_CANDIDATE_ITEMS: usize = 64;
const MAX_MAX_CANDIDATE_ITEMS: usize = 256;
const DEFAULT_MAX_EVALUATION_QUERIES: usize = 64;
const MAX_MAX_EVALUATION_QUERIES: usize = 256;
const EVALUATION_TOP_K: usize = 5;
const AUTOMATIC_DIRTY_INCREMENT: i32 = 10;
const DEFAULT_LOCK_TTL_SECONDS: i64 = 900;
const MAX_DETAIL_ITEMS: i64 = 500;

const DREAM_INSTRUCTIONS: &str = r#"You are Straylight's Phase-0 dream worker. Build only shadow-derived retrieval aids over the supplied immutable evidence. Preserve sources and disagreements. Never invent canonical facts, merge identities, delete evidence, change scope or policy, or claim validation without direct evidence. Every candidate claim and expected query must cite supplied source UUIDs. Use add_derived_view, add_alias, add_soft_link, add_cluster, or add_flag for safe additive aids. Use propose_same_as or propose_supersession only as review-required hypotheses. Return an empty candidate list when the evidence cannot support a useful consolidation."#;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DreamRollbackRequest {
    pub reason: String,
    #[serde(default)]
    pub expected_candidate_revision: Option<String>,
    #[serde(default)]
    pub expected_status: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DreamSchedulerControlRequest {
    pub scope: String,
    pub enabled: Option<bool>,
    pub paused: Option<bool>,
    pub repair_only: Option<bool>,
    pub dirty_score_delta: Option<i32>,
    pub dirty_reason: Option<String>,
    pub root_refs: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct NormalizedCreate {
    scope_ref: String,
    root_refs: Vec<String>,
    root_ids: Vec<Uuid>,
    trigger_reasons: Vec<String>,
    job_type: String,
    compute_budget: Value,
    max_records: usize,
    max_model_input_bytes: usize,
    max_model_output_tokens: u64,
    max_candidate_items: usize,
    max_evaluation_queries: usize,
    idempotency_key: String,
    request_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RegionMember {
    record_id: Uuid,
    record_kind: String,
    record_version: Option<i32>,
}

#[derive(Clone, Debug, Serialize)]
struct SourceDocument {
    record_id: Uuid,
    record_kind: String,
    record_version: Option<i32>,
    text: String,
    source_refs: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelCandidateItem {
    target_record_id: Uuid,
    disposition: String,
    title: String,
    content: String,
    source_refs: Vec<Uuid>,
    claims: Vec<ModelCandidateClaim>,
    query_aids: Vec<String>,
    rationale: String,
    authority_effect: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelCandidateClaim {
    text: String,
    source_refs: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelExpectedQuery {
    family: String,
    query: String,
    expected_source_refs: Vec<Uuid>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ModelCandidateOutput {
    candidate_items: Vec<ModelCandidateItem>,
    expected_queries: Vec<ModelExpectedQuery>,
    unresolved: Vec<String>,
}

#[derive(Clone, Debug)]
struct ModelRun {
    status: String,
    requested_model: String,
    returned_model: Option<String>,
    response_id: Option<String>,
    request_hash: String,
    output: Value,
    usage: Value,
    degradation_reason: Option<String>,
    candidates: Vec<ModelCandidateItem>,
    expected_queries: Vec<ModelExpectedQuery>,
    validation_errors: Vec<Value>,
}

impl ModelRun {
    fn completed(&self) -> bool {
        self.status == "completed" && self.validation_errors.is_empty()
    }
}

#[derive(Clone, Debug)]
struct RetrievalDocument {
    record_id: Uuid,
    record_kind: String,
    text: String,
    source_refs: Vec<Uuid>,
    origin: String,
}

#[derive(Clone, Debug, Serialize)]
struct FrozenQuery {
    query_ref: String,
    family: String,
    query: String,
    expected_record_ids: Vec<Uuid>,
    expected_source_refs: Vec<Uuid>,
    origin: String,
}

#[derive(Clone, Debug, Serialize)]
struct RetrievalHit {
    record_id: Uuid,
    record_kind: String,
    score: i64,
    source_refs: Vec<Uuid>,
    origin: String,
}

#[derive(Clone, Debug)]
struct RetrievalEvaluation {
    frozen_suite_hash: String,
    result: Value,
    passed: bool,
    regressions: usize,
    candidate_score: f64,
    active_score: f64,
}

#[derive(Clone, Debug, Serialize)]
struct Finding {
    finding_ref: String,
    target_record_id: Uuid,
    category: String,
    hard_integrity_issue: bool,
    source_refs: Vec<Uuid>,
    details: Value,
}

#[derive(Clone, Debug, Default, Serialize)]
struct MaintenanceReport {
    exact_duplicates: Vec<Finding>,
    broken_references: Vec<Finding>,
    conflicting_claim_heads: Vec<Finding>,
    stale_sources: Vec<Finding>,
    recurrence_issues: Vec<Finding>,
    state_issues: Vec<Finding>,
    policy_issues: Vec<Finding>,
    lineage_issues: Vec<Finding>,
}

impl MaintenanceReport {
    fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.exact_duplicates
            .iter()
            .chain(&self.broken_references)
            .chain(&self.conflicting_claim_heads)
            .chain(&self.stale_sources)
            .chain(&self.recurrence_issues)
            .chain(&self.state_issues)
            .chain(&self.policy_issues)
            .chain(&self.lineage_issues)
    }
}

#[derive(Clone, Debug, Serialize)]
struct CandidateDisposition {
    target_record_id: Uuid,
    disposition: String,
    requires_review: bool,
    source_refs: Vec<Uuid>,
    proposal: Value,
}

#[derive(Clone, Debug)]
struct GateSpec {
    gate_ref: String,
    gate_kind: String,
    hard_gate: bool,
    passed: bool,
    evidence_refs: Vec<Uuid>,
    details: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManifestSnapshot {
    manifest_id: Uuid,
    active_corpus_revision_id: Uuid,
    manifest_hash: String,
    generation: i64,
}

#[derive(Clone, Debug)]
struct WorkerSelection {
    job_id: Uuid,
    auth: AuthContext,
}

/// Creates an idempotent Phase-0 dream job pinned to the current active revision.
pub async fn create_job(
    state: &AppState,
    auth: &AuthContext,
    request: DreamCreateRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let normalized = normalize_create(request)?;
    let mut transaction = state.begin_write(auth).await?;

    let scope_row = sqlx::query(
        r#"
        SELECT
          scope_row.id AS scope_id,
          manifest.active_corpus_revision_id,
          manifest.manifest_hash AS active_manifest_hash,
          policy.id AS policy_id,
          policy.current_version AS policy_version
        FROM straylight.scopes AS scope_row
        JOIN straylight.active_manifests AS manifest
          ON manifest.user_id=scope_row.user_id AND manifest.scope_id=scope_row.id
        JOIN straylight.policies AS policy
          ON policy.user_id=scope_row.user_id AND policy.is_default
        WHERE scope_row.user_id=$1 AND scope_row.scope_ref=$2
        FOR SHARE OF manifest
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&normalized.scope_ref)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", &normalized.scope_ref))?;

    let scope_id: Uuid = scope_row.try_get("scope_id")?;
    let base_revision_id: Uuid = scope_row.try_get("active_corpus_revision_id")?;
    let active_manifest_hash: String = scope_row.try_get("active_manifest_hash")?;
    let policy_id: Uuid = scope_row.try_get("policy_id")?;
    let policy_version: i32 = scope_row.try_get("policy_version")?;

    let create_lock = format!(
        "dream-create:{}:{scope_id}:{}",
        auth.user_id.0, normalized.idempotency_key
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&create_lock)
        .execute(&mut *transaction)
        .await?;

    let job_id = deterministic_uuid(&format!(
        "dream-job:{}:{scope_id}:{}",
        auth.user_id.0, normalized.idempotency_key
    ));
    if let Some(existing_hash) = sqlx::query_scalar::<_, Option<String>>(
        r#"
        SELECT event.details->>'request_hash'
        FROM straylight.dream_jobs AS job
        LEFT JOIN LATERAL (
          SELECT details
          FROM straylight.dream_job_events
          WHERE user_id=job.user_id AND dream_job_id=job.id
          ORDER BY recorded_at, id
          LIMIT 1
        ) AS event ON true
        WHERE job.user_id=$1 AND job.id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(job_id)
    .fetch_optional(&mut *transaction)
    .await?
    .flatten()
    {
        if existing_hash != normalized.request_hash {
            return Err(ApiError::conflict(
                "idempotency_conflict",
                "the idempotency key was already used for a different dream request",
                json!({"idempotency_key": normalized.idempotency_key}),
            ));
        }
        let mut detail = job_detail_in_tx(&mut transaction, auth.user_id.0, job_id).await?;
        detail["idempotent_replay"] = Value::Bool(true);
        transaction.commit().await?;
        return Ok(detail);
    }

    let member_rows = sqlx::query(
        r#"
        SELECT record_id, record_kind, record_version
        FROM straylight.corpus_members
        WHERE user_id=$1 AND scope_id=$2 AND corpus_revision_id=$3
          AND disposition='active'
        ORDER BY record_id
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(base_revision_id)
    .fetch_all(&mut *transaction)
    .await?;
    let members = member_rows
        .into_iter()
        .map(|row| {
            Ok(RegionMember {
                record_id: row.try_get("record_id")?,
                record_kind: row.try_get("record_kind")?,
                record_version: row.try_get("record_version")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    if members.len() > normalized.max_records {
        return Err(ApiError::with_details(
            StatusCode::PAYLOAD_TOO_LARGE,
            "dream_region_too_large",
            "the expanded region exceeds the compute budget; split it rather than truncating it",
            json!({
                "records": members.len(),
                "max_records": normalized.max_records,
                "scope": normalized.scope_ref,
            }),
        ));
    }
    let member_ids: BTreeSet<_> = members.iter().map(|member| member.record_id).collect();
    let missing_roots: Vec<_> = normalized
        .root_ids
        .iter()
        .filter(|root| !member_ids.contains(root))
        .map(Uuid::to_string)
        .collect();
    if !missing_roots.is_empty() {
        return Err(ApiError::with_details(
            StatusCode::UNPROCESSABLE_ENTITY,
            "dream_root_not_in_active_revision",
            "every dream root must resolve inside the pinned active revision",
            json!({"missing_root_ids": missing_roots, "base_revision": base_revision_id}),
        ));
    }

    // Phase 0 deliberately over-expands to the complete authorized scope. This
    // avoids silent boundary loss while region heuristics are still learning.
    let expanded_root_set_hash = hash_json(&json!(
        members
            .iter()
            .map(|member| member.record_id)
            .collect::<Vec<_>>()
    ));
    let region_manifest = json!({
        "phase": PHASE,
        "scope_id": scope_id,
        "base_corpus_revision_id": base_revision_id,
        "active_manifest_hash": active_manifest_hash,
        "requested_root_refs": normalized.root_refs,
        "expansion": "complete_authorized_scope",
        "members": members,
        "policy_id": policy_id,
        "policy_version": policy_version,
    });
    let region_manifest_hash = hash_json(&region_manifest);
    let region_ref = format!(
        "region:{}:{}",
        scope_id.simple(),
        &expanded_root_set_hash[..16]
    );

    let region_id = if let Some(region_id) = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM straylight.dream_regions
        WHERE user_id=$1 AND scope_id=$2 AND region_ref=$3
          AND root_set_hash=$4 AND manifest_hash=$5
          AND policy_id=$6 AND policy_version=$7
        ORDER BY version DESC
        LIMIT 1
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(&region_ref)
    .bind(&expanded_root_set_hash)
    .bind(&region_manifest_hash)
    .bind(policy_id)
    .bind(policy_version)
    .fetch_optional(&mut *transaction)
    .await?
    {
        region_id
    } else {
        let version = sqlx::query_scalar::<_, i32>(
            r#"
            SELECT coalesce(max(version), 0)::integer + 1
            FROM straylight.dream_regions
            WHERE user_id=$1 AND scope_id=$2 AND region_ref=$3
            "#,
        )
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(&region_ref)
        .fetch_one(&mut *transaction)
        .await?;
        let region_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO straylight.dream_regions (
              id,user_id,scope_id,region_ref,version,root_set_hash,manifest_hash,
              policy_id,policy_version
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            "#,
        )
        .bind(region_id)
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(&region_ref)
        .bind(version)
        .bind(&expanded_root_set_hash)
        .bind(&region_manifest_hash)
        .bind(policy_id)
        .bind(policy_version)
        .execute(&mut *transaction)
        .await?;
        let ids: Vec<_> = members.iter().map(|member| member.record_id).collect();
        sqlx::query(
            r#"
            INSERT INTO straylight.dream_region_members (
              user_id,region_id,record_id,expansion_reason
            )
            SELECT $1,$2,member_id,'complete authorized scope for Phase 0'
            FROM unnest($3::uuid[]) AS member_id
            "#,
        )
        .bind(auth.user_id.0)
        .bind(region_id)
        .bind(ids)
        .execute(&mut *transaction)
        .await?;
        region_id
    };

    let evidence_watermark = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM straylight.source_episodes
        WHERE user_id=$1 AND scope_id=$2
        ORDER BY created_at DESC, id DESC LIMIT 1
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .fetch_optional(&mut *transaction)
    .await?
    .map(|id| format!("source:{id}"))
    .unwrap_or_else(|| format!("revision:{base_revision_id}"));

    let model_version = if normalized.job_type == "deep_consolidation" {
        format!("openai:{}", state.config.dream_model)
    } else {
        "model:not-requested".to_owned()
    };
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_jobs (
          id,user_id,scope_id,job_type,trigger_reasons,status,
          base_corpus_revision_id,evidence_watermark,region_id,
          dream_policy_version,model_version,prompt_version,schema_version,
          evaluator_version,compute_budget,requested_by_credential_id
        ) VALUES (
          $1,$2,$3,$4,$5,'queued',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15
        )
        "#,
    )
    .bind(job_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(&normalized.job_type)
    .bind(&normalized.trigger_reasons)
    .bind(base_revision_id)
    .bind(&evidence_watermark)
    .bind(region_id)
    .bind(DREAM_POLICY_VERSION)
    .bind(&model_version)
    .bind(PROMPT_VERSION)
    .bind(SCHEMA_VERSION)
    .bind(EVALUATOR_VERSION)
    .bind(&normalized.compute_budget)
    .bind(auth.credential_id.0)
    .execute(&mut *transaction)
    .await?;
    insert_job_event(
        &mut transaction,
        auth.user_id.0,
        scope_id,
        job_id,
        None,
        "queued",
        json!({
            "phase": PHASE,
            "idempotency_key": normalized.idempotency_key,
            "request_hash": normalized.request_hash,
            "base_revision": base_revision_id,
            "region_manifest_hash": region_manifest_hash,
            "requested_roots": normalized.root_refs,
            "expanded_record_count": members.len(),
            "model_version": model_version,
            "bounds": {
                "max_records": normalized.max_records,
                "max_model_input_bytes": normalized.max_model_input_bytes,
                "max_model_output_tokens": normalized.max_model_output_tokens,
                "max_candidate_items": normalized.max_candidate_items,
                "max_evaluation_queries": normalized.max_evaluation_queries,
            },
            "automatic_promotion": false,
        }),
    )
    .await?;
    insert_audit(
        &mut transaction,
        auth,
        scope_id,
        job_id,
        "dream.created",
        json!({
            "phase": PHASE,
            "base_revision": base_revision_id,
            "region_id": region_id,
            "request_hash": normalized.request_hash,
        }),
    )
    .await?;
    let detail = job_detail_in_tx(&mut transaction, auth.user_id.0, job_id).await?;
    transaction.commit().await?;
    Ok(detail)
}

/// Lists only jobs visible through the authenticated user's RLS projection.
pub async fn list_jobs(
    state: &AppState,
    auth: &AuthContext,
    query: &ListQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let mut transaction = state.begin_read(auth).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let cursor_id = query
        .cursor
        .as_deref()
        .map(|value| parse_uuid_ref(value, &["dream"]))
        .transpose()?;
    let cursor_time = if let Some(cursor_id) = cursor_id {
        Some(
            sqlx::query_scalar::<_, DateTime<Utc>>(
                "SELECT created_at FROM straylight.dream_jobs WHERE user_id=$1 AND id=$2",
            )
            .bind(auth.user_id.0)
            .bind(cursor_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| ApiError::not_found("dream_cursor_not_found", &cursor_id.to_string()))?,
        )
    } else {
        None
    };
    let rows = sqlx::query(
        r#"
        SELECT
          job.id, job.status, job.job_type, job.trigger_reasons,
          job.base_corpus_revision_id, job.created_at, job.completed_at,
          scope_row.scope_ref, region.region_ref,
          candidate.candidate_corpus_revision_id,
          coalesce((
            SELECT max(recorded_at) FROM straylight.dream_job_events AS event
            WHERE event.user_id=job.user_id AND event.dream_job_id=job.id
          ), job.created_at) AS updated_at
        FROM straylight.dream_jobs AS job
        JOIN straylight.scopes AS scope_row
          ON scope_row.user_id=job.user_id AND scope_row.id=job.scope_id
        JOIN straylight.dream_regions AS region
          ON region.user_id=job.user_id AND region.id=job.region_id
        LEFT JOIN straylight.dream_candidate_revisions AS candidate
          ON candidate.user_id=job.user_id AND candidate.dream_job_id=job.id
        WHERE job.user_id=$1
          AND ($2::text IS NULL OR scope_row.scope_ref=$2)
          AND ($3::text IS NULL OR job.status=$3)
          AND (
            $4::timestamptz IS NULL OR job.created_at < $4
            OR (job.created_at=$4 AND job.id < $5)
          )
        ORDER BY job.created_at DESC, job.id DESC
        LIMIT $6
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.scope.as_deref())
    .bind(query.status.as_deref())
    .bind(cursor_time)
    .bind(cursor_id)
    .bind((limit + 1) as i64)
    .fetch_all(&mut *transaction)
    .await?;

    let has_more = rows.len() > limit;
    let mut items = Vec::with_capacity(rows.len().min(limit));
    for row in rows.into_iter().take(limit) {
        let id: Uuid = row.try_get("id")?;
        let triggers: Vec<String> = row.try_get("trigger_reasons")?;
        items.push(json!({
            "id": id,
            "status": row.try_get::<String, _>("status")?,
            "job_type": row.try_get::<String, _>("job_type")?,
            "trigger": triggers,
            "scope": row.try_get::<String, _>("scope_ref")?,
            "region": row.try_get::<String, _>("region_ref")?,
            "base_revision": row.try_get::<Uuid, _>("base_corpus_revision_id")?,
            "candidate_revision": row.try_get::<Option<Uuid>, _>("candidate_corpus_revision_id")?,
            "created_at": row.try_get::<DateTime<Utc>, _>("created_at")?,
            "updated_at": row.try_get::<DateTime<Utc>, _>("updated_at")?,
            "completed_at": row.try_get::<Option<DateTime<Utc>>, _>("completed_at")?,
            "phase": PHASE,
            "automatic_promotion": false,
        }));
    }
    let next_cursor = has_more
        .then(|| {
            items
                .last()
                .and_then(|item| item["id"].as_str())
                .map(str::to_owned)
        })
        .flatten();
    transaction.commit().await?;
    Ok(json!({"items": items, "next_cursor": next_cursor, "phase": PHASE}))
}

pub async fn get_job(state: &AppState, auth: &AuthContext, dream_id: &str) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let dream_id = parse_uuid_ref(dream_id, &["dream"])?;
    let mut transaction = state.begin_read(auth).await?;
    let detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
    transaction.commit().await?;
    Ok(detail)
}

pub async fn run_now_job(
    state: &AppState,
    auth: &AuthContext,
    mut request: DreamCreateRequest,
) -> ApiResult<Value> {
    if !request
        .trigger
        .iter()
        .any(|trigger| trigger.trim() == "manual_run_now")
    {
        request.trigger.push("manual_run_now".to_owned());
    }
    create_job(state, auth, request).await
}

pub async fn list_scheduler_controls(
    state: &AppState,
    auth: &AuthContext,
    scope: Option<&str>,
) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let mut transaction = state.begin_read(auth).await?;
    let rows = scheduler_rows_in_tx(
        &mut transaction,
        auth.user_id.0,
        scope,
        state.config.dream_scheduler_enabled,
        state.config.dream_dirty_threshold,
        state.config.dream_inactivity_window.as_secs(),
    )
    .await?;
    transaction.commit().await?;
    Ok(json!({
        "phase": PHASE,
        "scheduler_enabled_globally": state.config.dream_scheduler_enabled,
        "dirty_threshold": state.config.dream_dirty_threshold,
        "inactivity_seconds": state.config.dream_inactivity_window.as_secs(),
        "cooldown_seconds": state.config.dream_cooldown.as_secs(),
        "items": rows,
    }))
}

pub async fn update_scheduler_control(
    state: &AppState,
    auth: &AuthContext,
    request: DreamSchedulerControlRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let scope_ref = request.scope.trim();
    if scope_ref.is_empty() || scope_ref.len() > 240 {
        return Err(ApiError::invalid(
            "scheduler scope must contain between 1 and 240 characters",
        ));
    }
    let dirty_delta = request.dirty_score_delta.unwrap_or(0);
    if !(-100_000..=100_000).contains(&dirty_delta) {
        return Err(ApiError::invalid(
            "dirty_score_delta must be between -100000 and 100000",
        ));
    }
    let reason = request
        .dirty_reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned);
    if reason.as_ref().is_some_and(|reason| reason.len() > 240) {
        return Err(ApiError::invalid(
            "scheduler dirty_reason must be at most 240 characters",
        ));
    }
    let root_ids = request
        .root_refs
        .as_ref()
        .map(|roots| parse_root_ids(roots))
        .transpose()?;
    let mut transaction = state.begin_write(auth).await?;
    let scope_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.scopes WHERE user_id=$1 AND scope_ref=$2",
    )
    .bind(auth.user_id.0)
    .bind(scope_ref)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", scope_ref))?;
    if let Some(root_ids) = root_ids.as_ref() {
        let matching = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.record_keys WHERE user_id=$1 AND scope_id=$2 AND record_id=ANY($3::uuid[])",
        )
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(root_ids)
        .fetch_one(&mut *transaction)
        .await?;
        if matching != root_ids.len() as i64 {
            return Err(ApiError::with_details(
                StatusCode::UNPROCESSABLE_ENTITY,
                "scheduler_root_scope_mismatch",
                "every dirty root must belong to the controlled scope",
                json!({"scope": scope_ref, "root_ids": root_ids}),
            ));
        }
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_scheduler_controls (
          user_id,scope_id,enabled,paused,repair_only,dirty_score,
          dirty_reasons,dirty_root_ids,dirty_since,observed_corpus_revision_id,
          updated_by_credential_id
        )
        SELECT $1,$2,coalesce($3,true),coalesce($4,false),coalesce($5,false),
               greatest(0,$6),
               CASE WHEN $7::text IS NULL THEN '{}'::text[] ELSE ARRAY[$7] END,
               coalesce($8::uuid[],'{}'::uuid[]),
               CASE WHEN $6 > 0 THEN clock_timestamp() ELSE NULL END,
               manifest.active_corpus_revision_id,$9
        FROM straylight.active_manifests AS manifest
        WHERE manifest.user_id=$1 AND manifest.scope_id=$2
        ON CONFLICT (user_id,scope_id) DO UPDATE SET
          enabled=coalesce($3,straylight.dream_scheduler_controls.enabled),
          paused=coalesce($4,straylight.dream_scheduler_controls.paused),
          repair_only=coalesce($5,straylight.dream_scheduler_controls.repair_only),
          dirty_score=greatest(0,straylight.dream_scheduler_controls.dirty_score+$6),
          dirty_reasons=CASE WHEN $7::text IS NULL
            THEN straylight.dream_scheduler_controls.dirty_reasons
            ELSE ARRAY(
              SELECT DISTINCT reason_value
              FROM unnest(
                straylight.dream_scheduler_controls.dirty_reasons||ARRAY[$7::text]
              ) AS reason_value
              ORDER BY reason_value
            )
          END,
          dirty_root_ids=coalesce($8,straylight.dream_scheduler_controls.dirty_root_ids),
          dirty_since=CASE
            WHEN $6 > 0 THEN coalesce(straylight.dream_scheduler_controls.dirty_since,clock_timestamp())
            WHEN greatest(0,straylight.dream_scheduler_controls.dirty_score+$6)=0 THEN NULL
            ELSE straylight.dream_scheduler_controls.dirty_since
          END,
          updated_by_credential_id=$9,
          updated_at=clock_timestamp()
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(request.enabled)
    .bind(request.paused)
    .bind(request.repair_only)
    .bind(dirty_delta)
    .bind(reason.as_deref())
    .bind(root_ids.as_deref())
    .bind(auth.credential_id.0)
    .execute(&mut *transaction)
    .await?;
    insert_scheduler_audit(
        &mut transaction,
        auth,
        scope_id,
        "dream.scheduler_control_updated",
        json!({
            "scope": scope_ref,
            "enabled": request.enabled,
            "paused": request.paused,
            "repair_only": request.repair_only,
            "dirty_score_delta": dirty_delta,
            "dirty_reason": reason,
            "dirty_root_ids": root_ids,
        }),
    )
    .await?;
    let mut rows = scheduler_rows_in_tx(
        &mut transaction,
        auth.user_id.0,
        Some(scope_ref),
        state.config.dream_scheduler_enabled,
        state.config.dream_dirty_threshold,
        state.config.dream_inactivity_window.as_secs(),
    )
    .await?;
    let result = rows
        .pop()
        .ok_or_else(|| ApiError::not_found("scheduler_control_not_found", scope_ref))?;
    transaction.commit().await?;
    Ok(result)
}

fn parse_root_ids(root_refs: &[String]) -> ApiResult<Vec<Uuid>> {
    if root_refs.len() > 256 {
        return Err(ApiError::invalid(
            "scheduler controls may contain at most 256 root refs",
        ));
    }
    let mut ids = root_refs
        .iter()
        .map(|reference| {
            let reference = reference.trim();
            Uuid::parse_str(reference)
                .ok()
                .or_else(|| RecordRef::parse(reference).ok().map(|record| record.id))
                .ok_or_else(|| {
                    ApiError::invalid(format!("invalid scheduler root ref: {reference}"))
                })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

async fn scheduler_rows_in_tx(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope: Option<&str>,
    globally_enabled: bool,
    dirty_threshold: i32,
    inactivity_seconds: u64,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT scope_row.scope_ref,scope_row.id AS scope_id,
               manifest.active_corpus_revision_id,
               ($3 AND coalesce(control.enabled,true)) AS enabled,
               coalesce(control.paused,false) AS paused,
               coalesce(control.repair_only,false) AS repair_only,
               coalesce(control.dirty_score,0) AS dirty_score,
               coalesce(control.dirty_reasons,'{}'::text[]) AS dirty_reasons,
               coalesce(control.dirty_root_ids,'{}'::uuid[]) AS dirty_root_ids,
               control.dirty_since,control.observed_corpus_revision_id,
               control.consolidated_corpus_revision_id,control.last_scheduled_at,
               control.last_completed_at,control.cooldown_until,control.updated_at,
               activity.latest_activity,
               EXISTS (
                 SELECT 1 FROM straylight.dream_jobs AS job
                 WHERE job.user_id=scope_row.user_id AND job.scope_id=scope_row.id
                   AND job.status IN (
                     'queued','selecting','opening_snapshot','generating','gating','evaluating'
                   )
               ) AS has_active_job
        FROM straylight.scopes AS scope_row
        JOIN straylight.active_manifests AS manifest
          ON manifest.user_id=scope_row.user_id AND manifest.scope_id=scope_row.id
        LEFT JOIN straylight.dream_scheduler_controls AS control
          ON control.user_id=scope_row.user_id AND control.scope_id=scope_row.id
        LEFT JOIN LATERAL (
          SELECT max(activity_at) AS latest_activity FROM (
            SELECT max(source.created_at) AS activity_at
            FROM straylight.source_episodes AS source
            WHERE source.user_id=scope_row.user_id AND source.scope_id=scope_row.id
            UNION ALL
            SELECT revision.created_at
            FROM straylight.corpus_revisions AS revision
            WHERE revision.user_id=manifest.user_id
              AND revision.id=manifest.active_corpus_revision_id
          ) AS activity_rows
        ) AS activity ON true
        WHERE scope_row.user_id=$1 AND ($2::text IS NULL OR scope_row.scope_ref=$2)
        ORDER BY scope_row.scope_ref
        "#,
    )
    .bind(user_id)
    .bind(scope)
    .bind(globally_enabled)
    .fetch_all(&mut **transaction)
    .await?;
    rows.into_iter()
        .map(|row| {
            let enabled: bool = row.try_get("enabled")?;
            let paused: bool = row.try_get("paused")?;
            let dirty_score: i32 = row.try_get("dirty_score")?;
            let cooldown_until: Option<DateTime<Utc>> = row.try_get("cooldown_until")?;
            let latest_activity: Option<DateTime<Utc>> = row.try_get("latest_activity")?;
            let has_active_job: bool = row.try_get("has_active_job")?;
            Ok(json!({
                "scope": row.try_get::<String, _>("scope_ref")?,
                "scope_id": row.try_get::<Uuid, _>("scope_id")?,
                "enabled": enabled,
                "paused": paused,
                "repair_only": row.try_get::<bool, _>("repair_only")?,
                "dirty_score": dirty_score,
                "dirty_reasons": row.try_get::<Vec<String>, _>("dirty_reasons")?,
                "dirty_root_ids": row.try_get::<Vec<Uuid>, _>("dirty_root_ids")?,
                "dirty_since": row.try_get::<Option<DateTime<Utc>>, _>("dirty_since")?,
                "observed_corpus_revision_id": row.try_get::<Option<Uuid>, _>("observed_corpus_revision_id")?,
                "consolidated_corpus_revision_id": row.try_get::<Option<Uuid>, _>("consolidated_corpus_revision_id")?,
                "active_corpus_revision_id": row.try_get::<Uuid, _>("active_corpus_revision_id")?,
                "last_scheduled_at": row.try_get::<Option<DateTime<Utc>>, _>("last_scheduled_at")?,
                "last_completed_at": row.try_get::<Option<DateTime<Utc>>, _>("last_completed_at")?,
                "cooldown_until": cooldown_until,
                "latest_activity": latest_activity,
                "has_active_job": has_active_job,
                "eligible_now": scheduler_eligible(
                    enabled,
                    paused,
                    dirty_score,
                    dirty_threshold,
                    cooldown_until,
                    latest_activity,
                    Utc::now(),
                    inactivity_seconds,
                    has_active_job,
                ),
                "updated_at": row.try_get::<Option<DateTime<Utc>>, _>("updated_at")?,
                "phase": PHASE,
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::from)
}

fn scheduler_eligible(
    enabled: bool,
    paused: bool,
    dirty_score: i32,
    dirty_threshold: i32,
    cooldown_until: Option<DateTime<Utc>>,
    latest_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    inactivity_seconds: u64,
    has_active_job: bool,
) -> bool {
    let cooldown_complete = cooldown_until.is_none_or(|cooldown| cooldown <= now);
    let inactive_long_enough = latest_activity.is_none_or(|activity| {
        now.signed_duration_since(activity).num_seconds()
            >= i64::try_from(inactivity_seconds).unwrap_or(i64::MAX)
    });
    enabled
        && !paused
        && dirty_score >= dirty_threshold.max(1)
        && cooldown_complete
        && inactive_long_enough
        && !has_active_job
}

fn scheduler_idempotency_key(
    scope_id: Uuid,
    active_revision_id: Uuid,
    repair_only: bool,
    last_completed_at: Option<DateTime<Utc>>,
) -> String {
    let cycle = last_completed_at
        .map(|completed| completed.timestamp_micros().to_string())
        .unwrap_or_else(|| "initial".to_owned());
    format!(
        "scheduler:{scope_id}:{active_revision_id}:{}:{cycle}",
        if repair_only { "repair" } else { "deep" }
    )
}

pub async fn schedule_next_dirty_job(state: &AppState) -> ApiResult<Option<Value>> {
    if !state.config.dream_scheduler_enabled {
        return Ok(None);
    }
    let pool = state.admin_pool.as_ref().ok_or_else(|| {
        ApiError::public(
            StatusCode::SERVICE_UNAVAILABLE,
            "dream_scheduler_discovery_unavailable",
            "the dream scheduler requires DATABASE_URL_ADMIN; content processing still uses RLS",
        )
    })?;
    let automatic_score = state
        .config
        .dream_dirty_threshold
        .max(AUTOMATIC_DIRTY_INCREMENT);
    let mut admin = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_scheduler_controls (
          user_id,scope_id,enabled,dirty_score,dirty_reasons,dirty_since,
          observed_corpus_revision_id
        )
        SELECT manifest.user_id,manifest.scope_id,true,
               CASE WHEN EXISTS (
                 SELECT 1 FROM straylight.corpus_members AS member
                 WHERE member.user_id=manifest.user_id
                   AND member.corpus_revision_id=manifest.active_corpus_revision_id
                   AND member.disposition='active'
               ) THEN $1 ELSE 0 END,
               CASE WHEN EXISTS (
                 SELECT 1 FROM straylight.corpus_members AS member
                 WHERE member.user_id=manifest.user_id
                   AND member.corpus_revision_id=manifest.active_corpus_revision_id
                   AND member.disposition='active'
               ) THEN ARRAY['initial_corpus']::text[] ELSE '{}'::text[] END,
               CASE WHEN EXISTS (
                 SELECT 1 FROM straylight.corpus_members AS member
                 WHERE member.user_id=manifest.user_id
                   AND member.corpus_revision_id=manifest.active_corpus_revision_id
                   AND member.disposition='active'
               ) THEN clock_timestamp() ELSE NULL END,
               manifest.active_corpus_revision_id
        FROM straylight.active_manifests AS manifest
        ON CONFLICT (user_id,scope_id) DO UPDATE SET
          dirty_score=CASE
            WHEN straylight.dream_scheduler_controls.observed_corpus_revision_id
                   IS DISTINCT FROM excluded.observed_corpus_revision_id
              THEN greatest(straylight.dream_scheduler_controls.dirty_score,$1)
            ELSE straylight.dream_scheduler_controls.dirty_score
          END,
          dirty_reasons=CASE
            WHEN straylight.dream_scheduler_controls.observed_corpus_revision_id
                   IS DISTINCT FROM excluded.observed_corpus_revision_id
              THEN ARRAY(
                SELECT DISTINCT reason_value
                FROM unnest(
                  straylight.dream_scheduler_controls.dirty_reasons
                  ||ARRAY['corpus_revision_changed']::text[]
                ) AS reason_value ORDER BY reason_value
              )
            ELSE straylight.dream_scheduler_controls.dirty_reasons
          END,
          dirty_since=CASE
            WHEN straylight.dream_scheduler_controls.observed_corpus_revision_id
                   IS DISTINCT FROM excluded.observed_corpus_revision_id
              THEN coalesce(straylight.dream_scheduler_controls.dirty_since,clock_timestamp())
            ELSE straylight.dream_scheduler_controls.dirty_since
          END,
          observed_corpus_revision_id=excluded.observed_corpus_revision_id,
          updated_at=clock_timestamp()
        "#,
    )
    .bind(automatic_score)
    .execute(&mut *admin)
    .await?;
    let row = sqlx::query(
        r#"
        SELECT control.user_id,control.scope_id,scope_row.scope_ref,
               manifest.active_corpus_revision_id,control.repair_only,
               control.last_completed_at,
               control.dirty_reasons,
               ARRAY(
                 SELECT root_id
                 FROM unnest(control.dirty_root_ids) AS root_id
                 WHERE EXISTS (
                   SELECT 1 FROM straylight.corpus_members AS member
                   WHERE member.user_id=control.user_id
                     AND member.corpus_revision_id=manifest.active_corpus_revision_id
                     AND member.record_id=root_id AND member.disposition='active'
                 )
                 ORDER BY root_id
               ) AS dirty_root_ids,
               credential.id AS credential_id,credential.capabilities,
               ARRAY(
                 SELECT granted_scope.scope_ref::text
                 FROM straylight.credential_scope_grants AS all_grants
                 JOIN straylight.scopes AS granted_scope
                   ON granted_scope.user_id=all_grants.user_id
                  AND granted_scope.id=all_grants.scope_id
                 WHERE all_grants.user_id=control.user_id
                   AND all_grants.credential_id=credential.id
                 ORDER BY granted_scope.scope_ref
               ) AS scope_refs,
               activity.latest_activity
        FROM straylight.dream_scheduler_controls AS control
        JOIN straylight.scopes AS scope_row
          ON scope_row.user_id=control.user_id AND scope_row.id=control.scope_id
        JOIN straylight.active_manifests AS manifest
          ON manifest.user_id=control.user_id AND manifest.scope_id=control.scope_id
        JOIN LATERAL (
          SELECT candidate.id,candidate.capabilities
          FROM straylight.api_credentials AS candidate
          JOIN straylight.credential_scope_grants AS grant_row
            ON grant_row.user_id=candidate.user_id
           AND grant_row.credential_id=candidate.id
           AND grant_row.scope_id=control.scope_id
          WHERE candidate.user_id=control.user_id
            AND candidate.disabled_at IS NULL
            AND 'dream'=ANY(candidate.capabilities)
          ORDER BY candidate.created_at,candidate.id
          LIMIT 1
        ) AS credential ON true
        LEFT JOIN LATERAL (
          SELECT max(activity_at) AS latest_activity FROM (
            SELECT max(source.created_at) AS activity_at
            FROM straylight.source_episodes AS source
            WHERE source.user_id=control.user_id AND source.scope_id=control.scope_id
            UNION ALL
            SELECT revision.created_at
            FROM straylight.corpus_revisions AS revision
            WHERE revision.user_id=manifest.user_id
              AND revision.id=manifest.active_corpus_revision_id
          ) AS activity_rows
        ) AS activity ON true
        WHERE control.enabled AND NOT control.paused
          AND control.dirty_score >= $1
          AND (control.cooldown_until IS NULL OR control.cooldown_until <= clock_timestamp())
          AND activity.latest_activity <= clock_timestamp()-make_interval(secs=>$2::double precision)
          AND NOT EXISTS (
            SELECT 1 FROM straylight.dream_jobs AS job
            WHERE job.user_id=control.user_id AND job.scope_id=control.scope_id
              AND job.status IN (
                'queued','selecting','opening_snapshot','generating','gating','evaluating'
              )
          )
        ORDER BY control.dirty_score DESC,control.dirty_since,control.scope_id
        FOR UPDATE OF control SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(state.config.dream_dirty_threshold.max(1))
    .bind(i64::try_from(state.config.dream_inactivity_window.as_secs()).unwrap_or(i64::MAX))
    .fetch_optional(&mut *admin)
    .await?;
    let Some(row) = row else {
        admin.rollback().await?;
        return Ok(None);
    };
    let user_id: Uuid = row.try_get("user_id")?;
    let scope_id: Uuid = row.try_get("scope_id")?;
    let scope_ref: String = row.try_get("scope_ref")?;
    let active_revision_id: Uuid = row.try_get("active_corpus_revision_id")?;
    let repair_only: bool = row.try_get("repair_only")?;
    let last_completed_at: Option<DateTime<Utc>> = row.try_get("last_completed_at")?;
    let mut triggers: Vec<String> = row.try_get("dirty_reasons")?;
    triggers.push("scheduler_eligible".to_owned());
    triggers.sort();
    triggers.dedup();
    let root_ids: Vec<Uuid> = row.try_get("dirty_root_ids")?;
    let capabilities: Vec<String> = row.try_get("capabilities")?;
    let auth = AuthContext {
        credential_id: CredentialId(row.try_get("credential_id")?),
        user_id: UserId(user_id),
        capabilities: capabilities.into_iter().collect(),
        scope_refs: row.try_get("scope_refs")?,
        read_only: false,
    };
    admin.commit().await?;

    let result = create_job(
        state,
        &auth,
        DreamCreateRequest {
            scope: scope_ref.clone(),
            root_refs: root_ids.iter().map(Uuid::to_string).collect(),
            trigger: triggers,
            job_type: if repair_only {
                "repair_only".to_owned()
            } else {
                "deep_consolidation".to_owned()
            },
            repair_only,
            compute_budget: None,
            expected_query_families: None,
            idempotency_key: scheduler_idempotency_key(
                scope_id,
                active_revision_id,
                repair_only,
                last_completed_at,
            ),
        },
    )
    .await?;
    let mut transaction = state.begin_write(&auth).await?;
    sqlx::query(
        r#"
        UPDATE straylight.dream_scheduler_controls
        SET last_scheduled_at=clock_timestamp(),updated_by_credential_id=$3,
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND scope_id=$2
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .execute(&mut *transaction)
    .await?;
    insert_scheduler_audit(
        &mut transaction,
        &auth,
        scope_id,
        "dream.scheduler_scheduled",
        json!({
            "scope": scope_ref,
            "base_revision": active_revision_id,
            "repair_only": repair_only,
            "dream_id": result.get("id"),
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Some(result))
}

async fn update_scheduler_after_run(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    base_revision_id: Uuid,
    final_status: &str,
    cooldown_seconds: u64,
) -> ApiResult<()> {
    let successful_shadow = final_status == "awaiting_review";
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_scheduler_controls (
          user_id,scope_id,enabled,dirty_score,dirty_reasons,dirty_since,
          observed_corpus_revision_id,consolidated_corpus_revision_id,
          last_completed_at,cooldown_until
        ) VALUES (
          $1,$2,true,CASE WHEN $4 THEN 0 ELSE 1 END,
          CASE WHEN $4 THEN '{}'::text[] ELSE ARRAY['last_run_quarantined']::text[] END,
          CASE WHEN $4 THEN NULL ELSE clock_timestamp() END,$3,
          CASE WHEN $4 THEN $3 ELSE NULL END,clock_timestamp(),
          clock_timestamp()+make_interval(secs=>$5::double precision)
        )
        ON CONFLICT (user_id,scope_id) DO UPDATE SET
          dirty_score=CASE WHEN $4 THEN 0 ELSE straylight.dream_scheduler_controls.dirty_score END,
          dirty_reasons=CASE WHEN $4 THEN '{}'::text[]
            ELSE ARRAY(
              SELECT DISTINCT reason_value
              FROM unnest(
                straylight.dream_scheduler_controls.dirty_reasons
                ||ARRAY['last_run_quarantined']::text[]
              ) AS reason_value ORDER BY reason_value
            )
          END,
          dirty_root_ids=CASE WHEN $4 THEN '{}'::uuid[]
            ELSE straylight.dream_scheduler_controls.dirty_root_ids END,
          dirty_since=CASE WHEN $4 THEN NULL
            ELSE coalesce(straylight.dream_scheduler_controls.dirty_since,clock_timestamp()) END,
          observed_corpus_revision_id=$3,
          consolidated_corpus_revision_id=CASE WHEN $4 THEN $3
            ELSE straylight.dream_scheduler_controls.consolidated_corpus_revision_id END,
          last_completed_at=clock_timestamp(),
          cooldown_until=clock_timestamp()+make_interval(secs=>$5::double precision),
          updated_at=clock_timestamp()
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(successful_shadow)
    .bind(i64::try_from(cooldown_seconds).unwrap_or(i64::MAX))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_scheduler_audit(
    transaction: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope_id: Uuid,
    action: &str,
    details: Value,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,actor_ref,action,details,content_free
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(format!("credential:{}", auth.credential_id.0))
    .bind(action)
    .bind(details)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn cancel_job(state: &AppState, auth: &AuthContext, dream_id: &str) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let dream_id = parse_uuid_ref(dream_id, &["dream"])?;
    let mut transaction = state.begin_write(auth).await?;
    let row = sqlx::query(
        "SELECT scope_id,status FROM straylight.dream_jobs WHERE user_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(auth.user_id.0)
    .bind(dream_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::not_found("dream_not_found", dream_id.to_string().as_str()))?;
    let scope_id: Uuid = row.try_get("scope_id")?;
    let status: String = row.try_get("status")?;
    if is_terminal_status(&status) {
        let mut detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
        detail["cancel_outcome"] = Value::String("already_terminal".to_owned());
        transaction.commit().await?;
        return Ok(detail);
    }
    if matches!(status.as_str(), "promoting" | "monitoring") {
        return Err(ApiError::conflict(
            "dream_cancel_conflict",
            "a promoting or monitored revision cannot be canceled; use rollback after promotion",
            json!({"dream_id": dream_id, "status": status}),
        ));
    }
    sqlx::query(
        "UPDATE straylight.dream_jobs SET status='canceled',completed_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(dream_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM straylight.dream_region_locks WHERE user_id=$1 AND dream_job_id=$2")
        .bind(auth.user_id.0)
        .bind(dream_id)
        .execute(&mut *transaction)
        .await?;
    insert_job_event(
        &mut transaction,
        auth.user_id.0,
        scope_id,
        dream_id,
        Some(&status),
        "canceled",
        json!({"phase": PHASE, "active_manifest_changed": false}),
    )
    .await?;
    insert_audit(
        &mut transaction,
        auth,
        scope_id,
        dream_id,
        "dream.canceled",
        json!({"from_status": status, "active_manifest_changed": false}),
    )
    .await?;
    let detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
    transaction.commit().await?;
    Ok(detail)
}

pub async fn review_job(
    state: &AppState,
    auth: &AuthContext,
    dream_id: &str,
    request: DreamReviewRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let dream_id = parse_uuid_ref(dream_id, &["dream"])?;
    let expected_candidate = parse_uuid_ref(
        &request.expected_candidate_revision,
        &["candidate", "revision"],
    )?;
    let mut transaction = state.begin_write(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT
          job.scope_id, job.status, candidate.id AS candidate_id,
          candidate.candidate_corpus_revision_id
        FROM straylight.dream_jobs AS job
        JOIN straylight.dream_candidate_revisions AS candidate
          ON candidate.user_id=job.user_id AND candidate.dream_job_id=job.id
        WHERE job.user_id=$1 AND job.id=$2
        FOR UPDATE OF job
        "#,
    )
    .bind(auth.user_id.0)
    .bind(dream_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::not_found("dream_candidate_not_found", &dream_id.to_string()))?;
    let scope_id: Uuid = row.try_get("scope_id")?;
    let status: String = row.try_get("status")?;
    let candidate_id: Uuid = row.try_get("candidate_id")?;
    let candidate_corpus_revision_id: Uuid = row.try_get("candidate_corpus_revision_id")?;
    if expected_candidate != candidate_id && expected_candidate != candidate_corpus_revision_id {
        return Err(ApiError::conflict(
            "candidate_revision_conflict",
            "the reviewed candidate is not the current candidate for this dream",
            json!({
                "expected": expected_candidate,
                "candidate_id": candidate_id,
                "candidate_corpus_revision_id": candidate_corpus_revision_id,
            }),
        ));
    }
    if !matches!(status.as_str(), "awaiting_review" | "quarantined") {
        return Err(ApiError::conflict(
            "dream_review_conflict",
            "this dream is not reviewable in its current state",
            json!({"status": status}),
        ));
    }

    let hard_failures = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM straylight.dream_gate_results
        WHERE user_id=$1 AND candidate_revision_id=$2 AND hard_gate AND NOT passed
        "#,
    )
    .bind(auth.user_id.0)
    .bind(candidate_id)
    .fetch_one(&mut *transaction)
    .await?;
    let unsafe_additions = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM straylight.dream_candidate_items
        WHERE user_id=$1 AND candidate_revision_id=$2
          AND disposition NOT IN ('keep','add_derived_view','add_alias','add_soft_link','add_cluster','add_flag')
        "#,
    )
    .bind(auth.user_id.0)
    .bind(candidate_id)
    .fetch_one(&mut *transaction)
    .await?;
    if request.decision == DreamReviewDecision::Accept && hard_failures > 0 {
        return Err(ApiError::conflict(
            "dream_hard_gate_failed",
            "a candidate with failed hard gates cannot be approved",
            json!({"hard_gate_failures": hard_failures}),
        ));
    }

    let (decision, next_status, default_reason) = match request.decision {
        DreamReviewDecision::Accept => (
            "approved",
            "awaiting_review",
            "Approved for Phase-0 learning; no promotion performed",
        ),
        DreamReviewDecision::Reject => (
            "rejected",
            "discarded",
            "Rejected during Phase-0 shadow review",
        ),
        DreamReviewDecision::Quarantine => (
            "changes_requested",
            "quarantined",
            "Quarantined during Phase-0 shadow review",
        ),
    };
    let reason = request
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or(default_reason)
        .to_owned();

    let existing = sqlx::query(
        r#"
        SELECT decision,reason FROM straylight.dream_reviews
        WHERE user_id=$1 AND candidate_revision_id=$2
        ORDER BY reviewed_at DESC,id DESC LIMIT 1
        "#,
    )
    .bind(auth.user_id.0)
    .bind(candidate_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let idempotent_replay = match existing {
        Some(existing) => {
            existing.try_get::<String, _>("decision")? == decision
                && existing.try_get::<String, _>("reason")? == reason
        }
        None => false,
    };
    if idempotent_replay {
        let mut detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
        detail["review_outcome"] = Value::String("idempotent_replay".to_owned());
        transaction.commit().await?;
        return Ok(detail);
    }

    // The normalized schema requires a record reference for reviewer_ref while
    // the HTTP request identifies the reviewer by credential. The job itself is
    // the immutable control-plane record; the actual credential remains in the
    // audit event and is never conflated with a person object.
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_reviews (
          id,user_id,scope_id,candidate_revision_id,target_record_id,
          reviewer_ref,decision,reason
        ) VALUES ($1,$2,$3,$4,NULL,$5,$6,$7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(candidate_id)
    .bind(dream_id)
    .bind(decision)
    .bind(&reason)
    .execute(&mut *transaction)
    .await?;
    if next_status != status {
        sqlx::query(
            r#"
            UPDATE straylight.dream_jobs
            SET status=$3,completed_at=CASE WHEN $3 IN ('discarded','quarantined')
              THEN clock_timestamp() ELSE completed_at END
            WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(dream_id)
        .bind(next_status)
        .execute(&mut *transaction)
        .await?;
    }
    insert_job_event(
        &mut transaction,
        auth.user_id.0,
        scope_id,
        dream_id,
        Some(&status),
        next_status,
        json!({
            "phase": PHASE,
            "review_decision": decision,
            "reviewer_credential_id": auth.credential_id.0,
            "hard_gates_passed": hard_failures == 0,
            "safe_derived_additions_only": unsafe_additions == 0,
            "promotion_available": false,
            "reason": reason,
        }),
    )
    .await?;
    insert_audit(
        &mut transaction,
        auth,
        scope_id,
        dream_id,
        "dream.reviewed",
        json!({
            "decision": decision,
            "candidate_revision": candidate_corpus_revision_id,
            "hard_gates_passed": hard_failures == 0,
            "safe_derived_additions_only": unsafe_additions == 0,
            "phase0_shadow_only": true,
        }),
    )
    .await?;
    let detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
    transaction.commit().await?;
    Ok(detail)
}

/// Rolls back only a revision represented by an immutable promotion receipt.
pub async fn rollback_job(
    state: &AppState,
    auth: &AuthContext,
    dream_id: &str,
    request: DreamRollbackRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Dream)?;
    let dream_id = parse_uuid_ref(dream_id, &["dream"])?;
    let reason = request.reason.trim();
    if reason.is_empty() {
        return Err(ApiError::invalid("rollback reason must not be empty"));
    }
    let expected_candidate = request
        .expected_candidate_revision
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_uuid_ref(value, &["candidate", "revision"]))
        .transpose()?;
    let mut transaction = state.begin_write(auth).await?;
    let receipt = sqlx::query(
        r#"
        SELECT
          promotion.id AS promotion_id, promotion.scope_id,
          promotion.candidate_revision_id,
          promotion.previous_corpus_revision_id,
          promotion.promoted_corpus_revision_id,
          promotion.manifest_generation,
          candidate.candidate_corpus_revision_id,
          job.status
        FROM straylight.dream_promotion_receipts AS promotion
        JOIN straylight.dream_jobs AS job
          ON job.user_id=promotion.user_id AND job.id=promotion.dream_job_id
        JOIN straylight.dream_candidate_revisions AS candidate
          ON candidate.user_id=promotion.user_id AND candidate.id=promotion.candidate_revision_id
        WHERE promotion.user_id=$1 AND promotion.dream_job_id=$2
        FOR UPDATE OF job
        "#,
    )
    .bind(auth.user_id.0)
    .bind(dream_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| {
        ApiError::conflict(
            "promotion_receipt_required",
            "rollback is unavailable because this Phase-0 dream was never promoted",
            json!({"dream_id": dream_id, "phase": PHASE}),
        )
    })?;
    let promotion_id: Uuid = receipt.try_get("promotion_id")?;
    let scope_id: Uuid = receipt.try_get("scope_id")?;
    let candidate_id: Uuid = receipt.try_get("candidate_revision_id")?;
    let candidate_corpus_revision_id: Uuid = receipt.try_get("candidate_corpus_revision_id")?;
    let previous_revision_id: Uuid = receipt.try_get("previous_corpus_revision_id")?;
    let promoted_revision_id: Uuid = receipt.try_get("promoted_corpus_revision_id")?;
    let promoted_generation: i64 = receipt.try_get("manifest_generation")?;
    let status: String = receipt.try_get("status")?;
    if let Some(expected) = expected_candidate
        && expected != candidate_id
        && expected != candidate_corpus_revision_id
    {
        return Err(ApiError::conflict(
            "candidate_revision_conflict",
            "rollback candidate precondition failed",
            json!({
                "expected": expected,
                "candidate_id": candidate_id,
                "candidate_corpus_revision_id": candidate_corpus_revision_id,
            }),
        ));
    }
    if let Some(expected_status) = request.expected_status.as_deref()
        && !expected_status.is_empty()
        && expected_status != status
    {
        return Err(ApiError::conflict(
            "dream_status_conflict",
            "rollback status precondition failed",
            json!({"expected": expected_status, "actual": status}),
        ));
    }
    if sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.dream_rollback_receipts WHERE user_id=$1 AND promotion_receipt_id=$2",
    )
    .bind(auth.user_id.0)
    .bind(promotion_id)
    .fetch_optional(&mut *transaction)
    .await?
    .is_some()
    {
        let mut detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
        detail["rollback_outcome"] = Value::String("idempotent_replay".to_owned());
        transaction.commit().await?;
        return Ok(detail);
    }
    if !matches!(status.as_str(), "promoted" | "monitoring") {
        return Err(ApiError::conflict(
            "rollback_status_conflict",
            "only a promoted or monitored dream can be rolled back",
            json!({"status": status}),
        ));
    }

    let manifest = read_manifest_for_update(&mut transaction, auth.user_id.0, scope_id).await?;
    if manifest.active_corpus_revision_id != promoted_revision_id
        || manifest.generation != promoted_generation
    {
        return Err(ApiError::conflict(
            "manifest_compare_and_swap_failed",
            "the active manifest no longer points at the promoted revision",
            json!({
                "expected_revision": promoted_revision_id,
                "actual_revision": manifest.active_corpus_revision_id,
                "expected_generation": promoted_generation,
                "actual_generation": manifest.generation,
            }),
        ));
    }
    let restored_hash = sqlx::query_scalar::<_, String>(
        "SELECT manifest_hash FROM straylight.corpus_revisions WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(previous_revision_id)
    .fetch_one(&mut *transaction)
    .await?;
    let next_generation = manifest.generation + 1;
    let updated = sqlx::query(
        r#"
        UPDATE straylight.active_manifests
        SET active_corpus_revision_id=$4,manifest_hash=$5,generation=$6,
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND scope_id=$2 AND id=$3 AND generation=$7
          AND active_corpus_revision_id=$8
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(manifest.manifest_id)
    .bind(previous_revision_id)
    .bind(&restored_hash)
    .bind(next_generation)
    .bind(manifest.generation)
    .bind(promoted_revision_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "manifest_compare_and_swap_failed",
            "the active manifest changed while rollback was being committed",
            json!({"manifest_id": manifest.manifest_id}),
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.active_manifest_history (
          id,user_id,scope_id,manifest_id,generation,corpus_revision_id,
          manifest_hash,change_kind,dream_job_id
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,'rollback',$8)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(manifest.manifest_id)
    .bind(next_generation)
    .bind(previous_revision_id)
    .bind(&restored_hash)
    .bind(dream_id)
    .execute(&mut *transaction)
    .await?;
    let rollback_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_rollback_receipts (
          id,user_id,scope_id,promotion_receipt_id,failed_corpus_revision_id,
          restored_corpus_revision_id,manifest_generation,reason
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(rollback_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(promotion_id)
    .bind(promoted_revision_id)
    .bind(previous_revision_id)
    .bind(next_generation)
    .bind(reason)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE straylight.dream_jobs SET status='rolled_back',completed_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(dream_id)
    .execute(&mut *transaction)
    .await?;
    insert_job_event(
        &mut transaction,
        auth.user_id.0,
        scope_id,
        dream_id,
        Some(&status),
        "rolled_back",
        json!({
            "promotion_receipt_id": promotion_id,
            "rollback_receipt_id": rollback_id,
            "failed_revision": promoted_revision_id,
            "restored_revision": previous_revision_id,
            "manifest_generation": next_generation,
            "reason": reason,
        }),
    )
    .await?;
    insert_audit(
        &mut transaction,
        auth,
        scope_id,
        dream_id,
        "dream.rolled_back",
        json!({
            "promotion_receipt_id": promotion_id,
            "rollback_receipt_id": rollback_id,
            "manifest_compare_and_swap": true,
        }),
    )
    .await?;
    let detail = job_detail_in_tx(&mut transaction, auth.user_id.0, dream_id).await?;
    transaction.commit().await?;
    Ok(detail)
}

/// Claims and processes one queued job. Queue discovery uses the migration
/// administrator only to locate a credential-scoped job; all content reads and
/// writes then run through `begin_write`, which installs that credential's RLS
/// context. Without an administrator discovery URL this fails closed.
pub async fn process_next_job(state: &AppState, worker_id: &str) -> ApiResult<Option<Value>> {
    let worker_id = worker_id.trim();
    if worker_id.is_empty() || worker_id.len() > 160 {
        return Err(ApiError::invalid(
            "worker_id must contain between 1 and 160 characters",
        ));
    }
    let Some(selection) = discover_next_job(state).await? else {
        return Ok(None);
    };
    let started = Instant::now();
    selection.auth.require(Capability::Dream)?;
    if !claim_job(state, &selection, worker_id).await? {
        metrics::counter!("worker.job.claim_conflicts", "queue" => "dream").increment(1);
        return Ok(None);
    }
    match process_claimed_job(state, &selection, worker_id).await {
        Ok(detail) => {
            telemetry::record_dream_job(&detail, "succeeded", started);
            Ok(Some(detail))
        }
        Err(error) => {
            let failure = sanitized_failure(&error);
            if let Err(mark_error) = mark_job_failed(state, &selection, worker_id, failure).await {
                tracing::error!(
                    dream_job_id = %selection.job_id,
                    worker_id,
                    error = ?mark_error,
                    "failed to persist dream failure state"
                );
            }
            telemetry::record_dream_job(&json!({"status": "failed"}), "failed", started);
            Err(error)
        }
    }
}

async fn discover_next_job(state: &AppState) -> ApiResult<Option<WorkerSelection>> {
    let admin_url = state.config.database_url_admin.as_deref().ok_or_else(|| {
        ApiError::public(
            StatusCode::SERVICE_UNAVAILABLE,
            "dream_worker_discovery_unavailable",
            "Phase-0 worker discovery requires DATABASE_URL_ADMIN; content processing still uses RLS",
        )
    })?;
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(admin_url)
        .await?;
    let row = sqlx::query(
        r#"
        SELECT
          job.id AS job_id, job.user_id, job.requested_by_credential_id,
          credential.capabilities,
          array_agg(scope_row.scope_ref::text ORDER BY scope_row.scope_ref) AS scope_refs
        FROM straylight.dream_jobs AS job
        JOIN straylight.api_credentials AS credential
          ON credential.user_id=job.user_id
         AND credential.id=job.requested_by_credential_id
         AND credential.disabled_at IS NULL
        JOIN straylight.credential_scope_grants AS grant_row
          ON grant_row.user_id=credential.user_id
         AND grant_row.credential_id=credential.id
        JOIN straylight.scopes AS scope_row
          ON scope_row.user_id=grant_row.user_id AND scope_row.id=grant_row.scope_id
        WHERE (
          job.status='queued'
          OR (
            job.status='selecting'
            AND NOT EXISTS (
              SELECT 1 FROM straylight.dream_region_locks AS lock_row
              WHERE lock_row.user_id=job.user_id
                AND lock_row.dream_job_id=job.id
                AND lock_row.expires_at > clock_timestamp()
            )
          )
        )
          AND 'dream'=ANY(credential.capabilities)
          AND EXISTS (
            SELECT 1 FROM straylight.credential_scope_grants AS job_grant
            WHERE job_grant.user_id=job.user_id
              AND job_grant.credential_id=credential.id
              AND job_grant.scope_id=job.scope_id
          )
        GROUP BY job.id,job.user_id,job.created_at,
                 job.requested_by_credential_id,credential.capabilities
        ORDER BY job.created_at,job.id
        LIMIT 1
        "#,
    )
    .fetch_optional(&admin_pool)
    .await?;
    admin_pool.close().await;
    let Some(row) = row else {
        return Ok(None);
    };
    let capabilities: Vec<String> = row.try_get("capabilities")?;
    let auth = AuthContext {
        credential_id: CredentialId(row.try_get("requested_by_credential_id")?),
        user_id: UserId(row.try_get("user_id")?),
        capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
        scope_refs: row.try_get("scope_refs")?,
        read_only: false,
    };
    Ok(Some(WorkerSelection {
        job_id: row.try_get("job_id")?,
        auth,
    }))
}

async fn claim_job(
    state: &AppState,
    selection: &WorkerSelection,
    worker_id: &str,
) -> ApiResult<bool> {
    let mut transaction = state.begin_write(&selection.auth).await?;
    let row = sqlx::query(
        r#"
        SELECT job.scope_id,job.region_id,job.status,job.compute_budget,region.region_ref
        FROM straylight.dream_jobs AS job
        JOIN straylight.dream_regions AS region
          ON region.user_id=job.user_id AND region.id=job.region_id
        WHERE job.user_id=$1 AND job.id=$2
        FOR UPDATE OF job
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(selection.job_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Ok(false);
    };
    let status: String = row.try_get("status")?;
    if !matches!(status.as_str(), "queued" | "selecting") {
        transaction.rollback().await?;
        return Ok(false);
    }
    let scope_id: Uuid = row.try_get("scope_id")?;
    let region_id: Uuid = row.try_get("region_id")?;
    let region_ref: String = row.try_get("region_ref")?;
    let compute_budget: Value = row.try_get("compute_budget")?;
    let ttl_seconds = compute_budget
        .get("lock_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_LOCK_TTL_SECONDS)
        .clamp(60, 3_600);

    sqlx::query(
        r#"
        DELETE FROM straylight.dream_region_locks AS lock_row
        USING straylight.dream_regions AS locked_region
        WHERE lock_row.user_id=$1
          AND locked_region.user_id=lock_row.user_id
          AND locked_region.id=lock_row.region_id
          AND locked_region.scope_id=$2
          AND locked_region.region_ref=$3
          AND lock_row.expires_at <= clock_timestamp()
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(&region_ref)
    .execute(&mut *transaction)
    .await?;
    let overlapping_lock = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT lock_row.dream_job_id
        FROM straylight.dream_region_locks AS lock_row
        JOIN straylight.dream_regions AS locked_region
          ON locked_region.user_id=lock_row.user_id AND locked_region.id=lock_row.region_id
        WHERE lock_row.user_id=$1 AND locked_region.scope_id=$2
          AND locked_region.region_ref=$3
          AND lock_row.expires_at > clock_timestamp()
        LIMIT 1
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(&region_ref)
    .fetch_optional(&mut *transaction)
    .await?;
    if overlapping_lock.is_some() {
        transaction.rollback().await?;
        return Ok(false);
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_region_locks (
          user_id,scope_id,region_id,dream_job_id,expires_at
        ) VALUES ($1,$2,$3,$4,clock_timestamp()+make_interval(secs=>$5::integer))
        ON CONFLICT (user_id,region_id) DO NOTHING
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(region_id)
    .bind(selection.job_id)
    .bind(ttl_seconds as i32)
    .execute(&mut *transaction)
    .await?;
    let owns_lock = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM straylight.dream_region_locks
          WHERE user_id=$1 AND region_id=$2 AND dream_job_id=$3
            AND expires_at > clock_timestamp()
        )
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(region_id)
    .bind(selection.job_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !owns_lock {
        transaction.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        r#"
        UPDATE straylight.dream_jobs
        SET status='selecting',started_at=coalesce(started_at,clock_timestamp())
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(selection.job_id)
    .execute(&mut *transaction)
    .await?;
    insert_job_event(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        Some(&status),
        "selecting",
        json!({
            "worker_id": worker_id,
            "region_lock": "acquired",
            "region_ref": region_ref,
            "lock_ttl_seconds": ttl_seconds,
            "reclaimed": status == "selecting",
        }),
    )
    .await?;
    insert_audit(
        &mut transaction,
        &selection.auth,
        scope_id,
        selection.job_id,
        "dream.worker_claimed",
        json!({"worker_id": worker_id, "region_id": region_id}),
    )
    .await?;
    transaction.commit().await?;
    Ok(true)
}

async fn process_claimed_job(
    state: &AppState,
    selection: &WorkerSelection,
    worker_id: &str,
) -> ApiResult<Value> {
    let mut opening = state.begin_write(&selection.auth).await?;
    let job = sqlx::query(
        r#"
        SELECT scope_id,region_id,status,base_corpus_revision_id,job_type,compute_budget
        FROM straylight.dream_jobs
        WHERE user_id=$1 AND id=$2
        FOR UPDATE
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(selection.job_id)
    .fetch_optional(&mut *opening)
    .await?
    .ok_or_else(|| ApiError::not_found("dream_not_found", &selection.job_id.to_string()))?;
    let scope_id: Uuid = job.try_get("scope_id")?;
    let region_id: Uuid = job.try_get("region_id")?;
    let status: String = job.try_get("status")?;
    let base_revision_id: Uuid = job.try_get("base_corpus_revision_id")?;
    let job_type: String = job.try_get("job_type")?;
    let compute_budget: Value = job.try_get("compute_budget")?;
    if status != "selecting" {
        return Err(ApiError::conflict(
            "dream_worker_claim_lost",
            "the dream is no longer in the selecting state",
            json!({"status": status, "worker_id": worker_id}),
        ));
    }
    let manifest_before =
        read_manifest_for_share(&mut opening, selection.auth.user_id.0, scope_id).await?;
    if manifest_before.active_corpus_revision_id != base_revision_id {
        transition_job(
            &mut opening,
            selection.auth.user_id.0,
            scope_id,
            selection.job_id,
            "selecting",
            "stale",
            true,
            json!({
                "pinned_base_revision": base_revision_id,
                "active_revision": manifest_before.active_corpus_revision_id,
                "action": "reselect_and_rerun",
                "active_manifest_changed_by_job": false,
            }),
        )
        .await?;
        release_region_lock(&mut opening, selection.auth.user_id.0, selection.job_id).await?;
        let detail =
            job_detail_in_tx(&mut opening, selection.auth.user_id.0, selection.job_id).await?;
        opening.commit().await?;
        return Ok(detail);
    }
    transition_job(
        &mut opening,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        "selecting",
        "opening_snapshot",
        false,
        json!({
            "worker_id": worker_id,
            "base_revision": base_revision_id,
            "manifest_generation": manifest_before.generation,
            "manifest_hash": manifest_before.manifest_hash,
        }),
    )
    .await?;
    let documents = load_source_documents(
        &mut opening,
        selection.auth.user_id.0,
        scope_id,
        base_revision_id,
        region_id,
    )
    .await?;
    let members = documents
        .iter()
        .map(|document| RegionMember {
            record_id: document.record_id,
            record_kind: document.record_kind.clone(),
            record_version: document.record_version,
        })
        .collect::<Vec<_>>();
    let max_records = compute_budget
        .get("max_records")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_RECORDS as u64) as usize;
    if members.len() > max_records {
        return Err(ApiError::with_details(
            StatusCode::PAYLOAD_TOO_LARGE,
            "dream_region_too_large",
            "the pinned region exceeds its compute budget and cannot be silently truncated",
            json!({"records": members.len(), "max_records": max_records}),
        ));
    }
    let referenced_source_refs = documents
        .iter()
        .flat_map(|document| document.source_refs.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let allowed_source_refs = source_bearing_refs(
        &mut opening,
        selection.auth.user_id.0,
        scope_id,
        region_id,
        &referenced_source_refs,
    )
    .await?;
    let member_ids = members
        .iter()
        .map(|member| member.record_id)
        .collect::<BTreeSet<_>>();
    let expected_query_families = compute_budget
        .get("expected_query_families")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let max_model_input_bytes = compute_budget
        .get("max_model_input_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_MODEL_INPUT_BYTES as u64) as usize;
    let max_model_output_tokens = compute_budget
        .get("max_model_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_MODEL_OUTPUT_TOKENS);
    let max_candidate_items = compute_budget
        .get("max_candidate_items")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_CANDIDATE_ITEMS as u64) as usize;
    let max_evaluation_queries = compute_budget
        .get("max_evaluation_queries")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_EVALUATION_QUERIES as u64) as usize;
    let source_snapshot_hash_before = hash_json(&serde_json::to_value(&documents)?);
    transition_job(
        &mut opening,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        "opening_snapshot",
        "generating",
        false,
        json!({
            "region_id": region_id,
            "record_count": members.len(),
            "source_bearing_ref_count": allowed_source_refs.len(),
            "source_snapshot_hash": source_snapshot_hash_before,
            "full_source_expansion": "bounded source-bearing records reopened",
            "model_consolidation": if job_type == "deep_consolidation" {
                "requested"
            } else {
                "not_requested_for_job_type"
            },
        }),
    )
    .await?;
    opening.commit().await?;

    let model_run = run_model_consolidation(
        state,
        &job_type,
        selection.auth.user_id.0,
        selection.job_id,
        base_revision_id,
        &documents,
        &member_ids,
        &allowed_source_refs,
        &expected_query_families,
        max_model_input_bytes,
        max_model_output_tokens,
        max_candidate_items,
        max_evaluation_queries,
    )
    .await;

    let mut transaction = state.begin_write(&selection.auth).await?;
    let current_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM straylight.dream_jobs WHERE user_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(selection.auth.user_id.0)
    .bind(selection.job_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| ApiError::not_found("dream_not_found", &selection.job_id.to_string()))?;
    if current_status != "generating" {
        return Err(ApiError::conflict(
            "dream_worker_claim_lost",
            "the dream changed state while model consolidation was running",
            json!({"status": current_status, "worker_id": worker_id}),
        ));
    }
    let manifest_current =
        read_manifest_for_share(&mut transaction, selection.auth.user_id.0, scope_id).await?;
    insert_model_receipt(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        &model_run,
    )
    .await?;
    if manifest_current != manifest_before {
        transition_job(
            &mut transaction,
            selection.auth.user_id.0,
            scope_id,
            selection.job_id,
            "generating",
            "stale",
            true,
            json!({
                "pinned_manifest": manifest_json(&manifest_before),
                "current_manifest": manifest_json(&manifest_current),
                "model_receipt_retained": true,
                "action": "reselect_and_rerun",
            }),
        )
        .await?;
        release_region_lock(&mut transaction, selection.auth.user_id.0, selection.job_id).await?;
        let detail =
            job_detail_in_tx(&mut transaction, selection.auth.user_id.0, selection.job_id).await?;
        transaction.commit().await?;
        return Ok(detail);
    }

    let reopened_documents = load_source_documents(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        base_revision_id,
        region_id,
    )
    .await?;
    let source_snapshot_hash_after = hash_json(&serde_json::to_value(&reopened_documents)?);
    let report = run_deterministic_maintenance(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        base_revision_id,
        region_id,
    )
    .await?;
    let mut dispositions = build_disposition_map(&members, &report);
    merge_model_candidates(&mut dispositions, &model_run.candidates);
    let complete_map = dispositions.len() == members.len();
    let safe_derived_additions_only = dispositions.iter().all(is_safe_disposition);
    let active_documents = active_retrieval_documents(&reopened_documents);
    let candidate_documents = candidate_retrieval_documents(&model_run.candidates);
    let frozen_queries = build_frozen_queries(
        &active_documents,
        &expected_query_families,
        &model_run.expected_queries,
        max_evaluation_queries,
    );
    let evaluation = evaluate_retrieval(
        base_revision_id,
        &active_documents,
        &candidate_documents,
        &frozen_queries,
    );
    transition_job(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        "generating",
        "gating",
        false,
        json!({
            "maintenance": report,
            "model_status": model_run.status,
            "model_candidate_count": model_run.candidates.len(),
            "model_validation_errors": model_run.validation_errors,
            "complete_disposition_map": complete_map,
            "disposition_count": dispositions.len(),
            "safe_derived_additions_only": safe_derived_additions_only,
            "frozen_query_count": frozen_queries.len(),
        }),
    )
    .await?;

    let candidate_manifest = json!({
        "phase": PHASE,
        "dream_job_id": selection.job_id,
        "base_corpus_revision_id": base_revision_id,
        "source_snapshot_hash": source_snapshot_hash_after,
        "active_manifest": {
            "id": manifest_before.manifest_id,
            "generation": manifest_before.generation,
            "hash": manifest_before.manifest_hash,
        },
        "model_receipt": {
            "status": model_run.status,
            "request_hash": model_run.request_hash,
            "output_hash": hash_json(&model_run.output),
        },
        "candidate_items": dispositions,
        "frozen_suite_hash": evaluation.frozen_suite_hash,
        "shadow_only": true,
    });
    let candidate_manifest_hash = hash_json(&candidate_manifest);
    let candidate_corpus_revision_id = Uuid::now_v7();
    let candidate_revision_number = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO straylight.corpus_revisions (
          id,user_id,scope_id,parent_revision_id,manifest_hash
        ) VALUES ($1,$2,$3,$4,$5)
        RETURNING revision_number
        "#,
    )
    .bind(candidate_corpus_revision_id)
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(&candidate_manifest_hash)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.corpus_members (
          user_id,scope_id,corpus_revision_id,record_id,record_kind,
          record_version,disposition
        )
        SELECT user_id,scope_id,$4,record_id,record_kind,record_version,disposition
        FROM straylight.corpus_members
        WHERE user_id=$1 AND scope_id=$2 AND corpus_revision_id=$3
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(candidate_corpus_revision_id)
    .execute(&mut *transaction)
    .await?;

    let gates = build_gates(
        &report,
        members.len(),
        dispositions.len(),
        safe_derived_additions_only,
        &job_type,
        &model_run,
        &evaluation,
        &source_snapshot_hash_before,
        &source_snapshot_hash_after,
        &candidate_manifest_hash,
    );
    let hard_gates_passed = gates.iter().all(|gate| !gate.hard_gate || gate.passed);
    let candidate_status = if hard_gates_passed {
        "gated"
    } else {
        "quarantined"
    };
    let candidate_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_candidate_revisions (
          id,user_id,scope_id,dream_job_id,base_corpus_revision_id,
          candidate_corpus_revision_id,candidate_manifest_hash,status
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(candidate_id)
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(selection.job_id)
    .bind(base_revision_id)
    .bind(candidate_corpus_revision_id)
    .bind(&candidate_manifest_hash)
    .bind(candidate_status)
    .execute(&mut *transaction)
    .await?;
    let disposition_json = serde_json::to_value(&dispositions)?;
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_candidate_items (
          user_id,candidate_revision_id,target_record_id,disposition,
          requires_review,source_refs,proposal
        )
        SELECT
          $1,$2,(item->>'target_record_id')::uuid,item->>'disposition',
          (item->>'requires_review')::boolean,
          ARRAY(
            SELECT source_ref::uuid
            FROM jsonb_array_elements_text(item->'source_refs') AS source_ref
          ),
          item->'proposal'
        FROM jsonb_array_elements($3::jsonb) AS item
        "#,
    )
    .bind(selection.auth.user_id.0)
    .bind(candidate_id)
    .bind(disposition_json)
    .execute(&mut *transaction)
    .await?;
    for gate in &gates {
        sqlx::query(
            r#"
            INSERT INTO straylight.dream_gate_results (
              id,user_id,scope_id,candidate_revision_id,gate_ref,gate_kind,
              hard_gate,passed,evidence_refs,details
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(selection.auth.user_id.0)
        .bind(scope_id)
        .bind(candidate_id)
        .bind(&gate.gate_ref)
        .bind(&gate.gate_kind)
        .bind(gate.hard_gate)
        .bind(gate.passed)
        .bind(&gate.evidence_refs)
        .bind(&gate.details)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_evaluations (
          id,user_id,scope_id,candidate_revision_id,active_corpus_revision_id,
          frozen_suite_hash,reader_version,retrieval_policy_version,result,passed
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(selection.auth.user_id.0)
    .bind(scope_id)
    .bind(candidate_id)
    .bind(base_revision_id)
    .bind(&evaluation.frozen_suite_hash)
    .bind("reader:paired-lexical-shadow-v1")
    .bind(RETRIEVAL_POLICY_VERSION)
    .bind(&evaluation.result)
    .bind(evaluation.passed)
    .execute(&mut *transaction)
    .await?;
    transition_job(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        "gating",
        "evaluating",
        false,
        json!({
            "candidate_id": candidate_id,
            "candidate_corpus_revision_id": candidate_corpus_revision_id,
            "candidate_revision_number": candidate_revision_number,
            "hard_gates_passed": hard_gates_passed,
            "safe_derived_additions_only": safe_derived_additions_only,
            "retrieval_evaluation_passed": evaluation.passed,
        }),
    )
    .await?;

    let manifest_after =
        read_manifest_for_share(&mut transaction, selection.auth.user_id.0, scope_id).await?;
    if manifest_after != manifest_before {
        return Err(ApiError::conflict(
            "phase0_manifest_mutation_detected",
            "the active manifest changed during shadow candidate construction",
            json!({
                "before": manifest_json(&manifest_before),
                "after": manifest_json(&manifest_after),
            }),
        ));
    }
    let final_status = if hard_gates_passed {
        "awaiting_review"
    } else {
        "quarantined"
    };
    transition_job(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        selection.job_id,
        "evaluating",
        final_status,
        final_status == "quarantined",
        json!({
            "phase": PHASE,
            "job_type": job_type,
            "candidate_id": candidate_id,
            "candidate_corpus_revision_id": candidate_corpus_revision_id,
            "candidate_revision_number": candidate_revision_number,
            "candidate_manifest_hash": candidate_manifest_hash,
            "hard_gates_passed": hard_gates_passed,
            "safe_derived_additions_only": safe_derived_additions_only,
            "active_manifest_before": manifest_json(&manifest_before),
            "active_manifest_after": manifest_json(&manifest_after),
            "active_manifest_byte_identical": true,
            "automatic_promotion": false,
            "deep_model_output": model_run.status,
        }),
    )
    .await?;
    update_scheduler_after_run(
        &mut transaction,
        selection.auth.user_id.0,
        scope_id,
        base_revision_id,
        final_status,
        state.config.dream_cooldown.as_secs(),
    )
    .await?;
    insert_audit(
        &mut transaction,
        &selection.auth,
        scope_id,
        selection.job_id,
        "dream.shadow_completed",
        json!({
            "worker_id": worker_id,
            "candidate_revision": candidate_corpus_revision_id,
            "candidate_revision_number": candidate_revision_number,
            "status": final_status,
            "model_status": model_run.status,
            "hard_gates_passed": hard_gates_passed,
            "active_manifest_changed": false,
        }),
    )
    .await?;
    release_region_lock(&mut transaction, selection.auth.user_id.0, selection.job_id).await?;
    let detail =
        job_detail_in_tx(&mut transaction, selection.auth.user_id.0, selection.job_id).await?;
    transaction.commit().await?;
    Ok(detail)
}

async fn mark_job_failed(
    state: &AppState,
    selection: &WorkerSelection,
    worker_id: &str,
    failure: Value,
) -> ApiResult<()> {
    let mut transaction = state.begin_write(&selection.auth).await?;
    let row = sqlx::query(
        "SELECT scope_id,status FROM straylight.dream_jobs WHERE user_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(selection.auth.user_id.0)
    .bind(selection.job_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(row) = row {
        let scope_id: Uuid = row.try_get("scope_id")?;
        let status: String = row.try_get("status")?;
        if !is_terminal_status(&status) {
            sqlx::query(
                "UPDATE straylight.dream_jobs SET status='failed',completed_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
            )
            .bind(selection.auth.user_id.0)
            .bind(selection.job_id)
            .execute(&mut *transaction)
            .await?;
            insert_job_event(
                &mut transaction,
                selection.auth.user_id.0,
                scope_id,
                selection.job_id,
                Some(&status),
                "failed",
                json!({
                    "worker_id": worker_id,
                    "failure": failure,
                    "active_manifest_changed": false,
                    "partial_candidate_activation": false,
                }),
            )
            .await?;
            insert_audit(
                &mut transaction,
                &selection.auth,
                scope_id,
                selection.job_id,
                "dream.failed",
                json!({"worker_id": worker_id, "failure": failure}),
            )
            .await?;
        }
        release_region_lock(&mut transaction, selection.auth.user_id.0, selection.job_id).await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn load_source_documents(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    base_revision_id: Uuid,
    region_id: Uuid,
) -> ApiResult<Vec<SourceDocument>> {
    let rows = sqlx::query(
        r#"
        WITH member AS (
          SELECT key_row.record_id,key_row.record_kind,corpus.record_version
          FROM straylight.dream_region_members AS region_member
          JOIN straylight.record_keys AS key_row
            ON key_row.user_id=region_member.user_id
           AND key_row.record_id=region_member.record_id
          LEFT JOIN straylight.corpus_members AS corpus
            ON corpus.user_id=region_member.user_id
           AND corpus.corpus_revision_id=$3
           AND corpus.record_id=region_member.record_id
           AND corpus.disposition='active'
          WHERE region_member.user_id=$1 AND region_member.region_id=$4
        ), rich AS (
          SELECT object_row.id AS record_id,
                 concat_ws(E'\n',revision.label,revision.properties::text) AS body,
                 coalesce((
                   SELECT array_agg(evidence.id ORDER BY evidence.id)
                   FROM straylight.evidence_items AS evidence
                   WHERE evidence.user_id=object_row.user_id
                     AND evidence.source_episode_id=revision.source_episode_id
                 ),ARRAY[revision.source_episode_id]) AS source_refs
          FROM straylight.objects AS object_row
          JOIN member ON member.record_id=object_row.id AND member.record_kind='object'
          JOIN straylight.object_revisions AS revision
            ON revision.user_id=object_row.user_id
           AND revision.object_id=object_row.id
           AND revision.version=object_row.current_version
          WHERE object_row.user_id=$1 AND object_row.scope_id=$2
          UNION ALL
          SELECT claim.id,
                 concat_ws(E'\n',claim.predicate::text,claim.value::text,
                   'formation='||claim.formation_method,
                   'mode='||claim.claim_mode,
                   'support='||claim.support_state,
                   'authority='||claim.authority,
                   'canonicality='||claim.canonicality),
                 coalesce((
                   SELECT array_agg(link.evidence_id ORDER BY link.evidence_id)
                   FROM straylight.claim_evidence_links AS link
                   WHERE link.user_id=claim.user_id AND link.claim_id=claim.id
                 ),ARRAY[claim.source_episode_id])
          FROM straylight.claims AS claim
          JOIN member ON member.record_id=claim.id AND member.record_kind='claim'
          WHERE claim.user_id=$1 AND claim.scope_id=$2
          UNION ALL
          SELECT evidence.id,
                 concat_ws(E'\n',evidence.evidence_kind,evidence.locator::text,evidence.text_content),
                 ARRAY[evidence.id]
          FROM straylight.evidence_items AS evidence
          JOIN member ON member.record_id=evidence.id AND member.record_kind='evidence'
          WHERE evidence.user_id=$1 AND evidence.scope_id=$2
          UNION ALL
          SELECT source.id,
                 concat_ws(E'\n',source.title,source.source_ref,source.source_kind,
                   source.source_version,source.native_locator::text,source.metadata::text),
                 coalesce((
                   SELECT array_agg(evidence.id ORDER BY evidence.id)
                   FROM straylight.evidence_items AS evidence
                   WHERE evidence.user_id=source.user_id
                     AND evidence.source_episode_id=source.id
                 ),ARRAY[source.id])
          FROM straylight.source_episodes AS source
          JOIN member ON member.record_id=source.id AND member.record_kind='source_episode'
          WHERE source.user_id=$1 AND source.scope_id=$2
          UNION ALL
          SELECT chunk.id,
                 concat_ws(E'\n',array_to_string(chunk.heading_path,' / '),chunk.content),
                 coalesce((
                   SELECT array_agg(evidence.id ORDER BY evidence.id)
                   FROM straylight.evidence_items AS evidence
                   WHERE evidence.user_id=chunk.user_id
                     AND evidence.source_episode_id=revision.source_episode_id
                 ),ARRAY[revision.source_episode_id])
          FROM straylight.chunks AS chunk
          JOIN member ON member.record_id=chunk.id AND member.record_kind='chunk'
          JOIN straylight.document_revisions AS revision
            ON revision.user_id=chunk.user_id
           AND revision.document_id=chunk.document_id
           AND revision.version=chunk.document_version
          WHERE chunk.user_id=$1 AND chunk.scope_id=$2
          UNION ALL
          SELECT document.id,
                 concat_ws(E'\n',revision.title,revision.media_type,
                   revision.native_locator::text),
                 coalesce((
                   SELECT array_agg(evidence.id ORDER BY evidence.id)
                   FROM straylight.evidence_items AS evidence
                   WHERE evidence.user_id=document.user_id
                     AND evidence.source_episode_id=revision.source_episode_id
                 ),ARRAY[revision.source_episode_id])
          FROM straylight.documents AS document
          JOIN member ON member.record_id=document.id AND member.record_kind='document'
          JOIN straylight.document_revisions AS revision
            ON revision.user_id=document.user_id
           AND revision.document_id=document.id
           AND revision.version=document.current_version
          WHERE document.user_id=$1 AND document.scope_id=$2
          UNION ALL
          SELECT relation.id,
                 concat_ws(E'\n',revision.predicate::text,revision.qualifiers::text,(
                   SELECT string_agg(endpoint.role||'='||endpoint.target_record_id::text,
                     E'\n' ORDER BY endpoint.role,endpoint.position)
                   FROM straylight.relation_endpoints AS endpoint
                   WHERE endpoint.user_id=relation.user_id
                     AND endpoint.relation_id=relation.id
                     AND endpoint.relation_version=relation.current_version
                 )),
                 coalesce((
                   SELECT array_agg(link.evidence_id ORDER BY link.evidence_id)
                   FROM straylight.relation_evidence_links AS link
                   WHERE link.user_id=relation.user_id
                     AND link.relation_id=relation.id
                     AND link.relation_version=relation.current_version
                 ),ARRAY[revision.source_episode_id])
          FROM straylight.relations AS relation
          JOIN member ON member.record_id=relation.id AND member.record_kind='relation'
          JOIN straylight.relation_revisions AS revision
            ON revision.user_id=relation.user_id
           AND revision.relation_id=relation.id
           AND revision.version=relation.current_version
          WHERE relation.user_id=$1 AND relation.scope_id=$2
          UNION ALL
          SELECT checkpoint.id,
                 concat_ws(E'\n',checkpoint.title,checkpoint.summary,(
                   SELECT string_agg(goal.goal_text,E'\n' ORDER BY goal.position)
                   FROM straylight.checkpoint_goals AS goal
                   WHERE goal.user_id=checkpoint.user_id
                     AND goal.checkpoint_id=checkpoint.id
                 ),(
                   SELECT string_agg(action.action_text,E'\n' ORDER BY action.position)
                   FROM straylight.checkpoint_actions AS action
                   WHERE action.user_id=checkpoint.user_id
                     AND action.checkpoint_id=checkpoint.id
                 )),
                 coalesce((
                   SELECT array_agg(source_ref.source_record_id ORDER BY source_ref.position)
                   FROM straylight.checkpoint_source_refs AS source_ref
                   WHERE source_ref.user_id=checkpoint.user_id
                     AND source_ref.checkpoint_id=checkpoint.id
                 ),ARRAY[checkpoint.id])
          FROM straylight.checkpoints AS checkpoint
          JOIN member ON member.record_id=checkpoint.id AND member.record_kind='checkpoint'
          WHERE checkpoint.user_id=$1 AND checkpoint.scope_id=$2
        )
        SELECT member.record_id,member.record_kind,member.record_version,
               coalesce(nullif(rich.body,''),member.record_kind||' '||member.record_id::text) AS body,
               coalesce(rich.source_refs,ARRAY[member.record_id]) AS source_refs
        FROM member LEFT JOIN rich ON rich.record_id=member.record_id
        ORDER BY member.record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(SourceDocument {
                record_id: row.try_get("record_id")?,
                record_kind: row.try_get("record_kind")?,
                record_version: row.try_get("record_version")?,
                text: row.try_get("body")?,
                source_refs: row.try_get("source_refs")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::from)
}

async fn source_bearing_refs(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    region_id: Uuid,
    referenced_source_refs: &[Uuid],
) -> ApiResult<BTreeSet<Uuid>> {
    let refs = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT source.id
        FROM straylight.source_episodes AS source
        WHERE source.user_id=$1 AND source.scope_id=$2
          AND (
            source.id=ANY($4::uuid[])
            OR EXISTS (
              SELECT 1 FROM straylight.dream_region_members AS member
              WHERE member.user_id=source.user_id AND member.region_id=$3
                AND member.record_id=source.id
            )
          )
        UNION
        SELECT evidence.id
        FROM straylight.evidence_items AS evidence
        WHERE evidence.user_id=$1 AND evidence.scope_id=$2
          AND (
            evidence.id=ANY($4::uuid[])
            OR EXISTS (
              SELECT 1 FROM straylight.dream_region_members AS member
              WHERE member.user_id=evidence.user_id AND member.region_id=$3
                AND member.record_id=evidence.id
            )
            OR EXISTS (
              SELECT 1 FROM straylight.dream_region_members AS member
              WHERE member.user_id=evidence.user_id AND member.region_id=$3
                AND member.record_id=evidence.source_episode_id
            )
          )
        ORDER BY 1
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(region_id)
    .bind(referenced_source_refs)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(refs.into_iter().collect())
}

fn dream_output_schema() -> Value {
    let source_refs = json!({
        "type": "array",
        "items": {"type": "string"}
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "candidate_items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "target_record_id": {"type": "string"},
                        "disposition": {"type": "string", "enum": [
                            "add_derived_view", "add_alias", "add_soft_link",
                            "add_cluster", "add_flag", "propose_same_as",
                            "propose_supersession"
                        ]},
                        "title": {"type": "string"},
                        "content": {"type": "string"},
                        "source_refs": source_refs.clone(),
                        "claims": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "text": {"type": "string"},
                                    "source_refs": source_refs.clone()
                                },
                                "required": ["text", "source_refs"]
                            }
                        },
                        "query_aids": {
                            "type": "array",
                            "items": {"type": "string"}
                        },
                        "rationale": {"type": "string"},
                        "authority_effect": {
                            "type": "string", "enum": ["none", "review_required"]
                        }
                    },
                    "required": [
                        "target_record_id", "disposition", "title", "content",
                        "source_refs", "claims", "query_aids", "rationale",
                        "authority_effect"
                    ]
                }
            },
            "expected_queries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "family": {"type": "string"},
                        "query": {"type": "string"},
                        "expected_source_refs": source_refs
                    },
                    "required": ["family", "query", "expected_source_refs"]
                }
            },
            "unresolved": {
                "type": "array",
                "items": {"type": "string"}
            }
        },
        "required": ["candidate_items", "expected_queries", "unresolved"]
    })
}

async fn run_model_consolidation(
    state: &AppState,
    job_type: &str,
    user_id: Uuid,
    dream_id: Uuid,
    base_revision_id: Uuid,
    documents: &[SourceDocument],
    member_ids: &BTreeSet<Uuid>,
    allowed_source_refs: &BTreeSet<Uuid>,
    expected_query_families: &[String],
    max_input_bytes: usize,
    max_output_tokens: u64,
    max_candidate_items: usize,
    max_evaluation_queries: usize,
) -> ModelRun {
    let started = Instant::now();
    let result = run_model_consolidation_inner(
        state,
        job_type,
        user_id,
        dream_id,
        base_revision_id,
        documents,
        member_ids,
        allowed_source_refs,
        expected_query_families,
        max_input_bytes,
        max_output_tokens,
        max_candidate_items,
        max_evaluation_queries,
    )
    .await;
    telemetry::record_model_run(
        "dream",
        &result.requested_model,
        &result.status,
        &result.usage,
        started,
    );
    metrics::histogram!("dream.model.candidates", "status" => result.status.clone())
        .record(result.candidates.len() as f64);
    metrics::histogram!(
        "dream.model.validation_errors",
        "status" => result.status.clone()
    )
    .record(result.validation_errors.len() as f64);
    result
}

async fn run_model_consolidation_inner(
    state: &AppState,
    job_type: &str,
    user_id: Uuid,
    dream_id: Uuid,
    base_revision_id: Uuid,
    documents: &[SourceDocument],
    member_ids: &BTreeSet<Uuid>,
    allowed_source_refs: &BTreeSet<Uuid>,
    expected_query_families: &[String],
    max_input_bytes: usize,
    max_output_tokens: u64,
    max_candidate_items: usize,
    max_evaluation_queries: usize,
) -> ModelRun {
    let requested_model = state.config.dream_model.clone();
    if job_type != "deep_consolidation" {
        return ModelRun {
            status: "not_requested".to_owned(),
            requested_model,
            returned_model: None,
            response_id: None,
            request_hash: hash_json(&json!({
                "job_type": job_type,
                "base_revision": base_revision_id,
            })),
            output: json!({}),
            usage: json!({}),
            degradation_reason: Some("model consolidation is not part of this job type".to_owned()),
            candidates: vec![],
            expected_queries: vec![],
            validation_errors: vec![],
        };
    }
    let bundle = json!({
        "phase": PHASE,
        "dream_id": dream_id,
        "base_revision": base_revision_id,
        "expected_query_families": expected_query_families,
        "records": documents,
        "constraints": {
            "shadow_only": true,
            "source_preserving": true,
            "automatic_promotion": false,
            "allowed_source_refs": allowed_source_refs,
        }
    });
    let bundle_bytes = match serde_json::to_vec(&bundle) {
        Ok(bytes) => bytes,
        Err(error) => {
            return degraded_model_run(
                requested_model,
                "invalid_output",
                hash_json(&json!({"dream_id": dream_id})),
                format!("could not serialize bounded evidence bundle: {error}"),
            );
        }
    };
    let request_hash = hex::encode(Sha256::digest(&bundle_bytes));
    if bundle_bytes.len() > max_input_bytes {
        return degraded_model_run(
            requested_model,
            "invalid_output",
            request_hash,
            format!(
                "bounded evidence bundle is {} bytes, above max_model_input_bytes={max_input_bytes}; split the region",
                bundle_bytes.len()
            ),
        );
    }
    let Some(api_key) = state.config.openai_api_key.as_deref() else {
        return degraded_model_run(
            requested_model,
            "degraded_unavailable",
            request_hash,
            "OPENAI_API_KEY is unavailable; deterministic maintenance ran but deep consolidation did not"
                .to_owned(),
        );
    };
    let Some(admin_pool) = state.admin_pool.as_ref() else {
        return degraded_model_run(
            requested_model,
            "degraded_unavailable",
            request_hash,
            "the worker has no administrative database pool for model quota enforcement".to_owned(),
        );
    };
    if let Err(error) = quota::reserve_for_user(
        admin_pool,
        user_id,
        UsageReservation {
            model_input_chars: (bundle_bytes.len() + DREAM_INSTRUCTIONS.len()) as u64,
            model_output_tokens: max_output_tokens,
            ..UsageReservation::default()
        },
    )
    .await
    {
        return degraded_model_run(
            requested_model,
            "quota_exceeded",
            request_hash,
            error.to_string(),
        );
    }
    let client = match Client::builder()
        .timeout(state.config.request_timeout)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return degraded_model_run(
                requested_model,
                "degraded_unavailable",
                request_hash,
                format!("could not create OpenAI client: {error}"),
            );
        }
    };
    let request = json!({
        "model": requested_model,
        "store": false,
        "safety_identifier": safety_identifier(&state.config.continuation_secret, user_id),
        "instructions": DREAM_INSTRUCTIONS,
        "input": String::from_utf8_lossy(&bundle_bytes),
        "max_output_tokens": max_output_tokens,
        "text": {
            "format": {
                "type": "json_schema",
                "name": "straylight_phase0_dream_candidate",
                "strict": true,
                "schema": dream_output_schema()
            }
        }
    });
    let response = match client
        .post(format!(
            "{}/responses",
            state.config.openai_base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return degraded_model_run(
                requested_model,
                "degraded_unavailable",
                request_hash,
                format!("OpenAI Responses request failed: {error}"),
            );
        }
    };
    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return degraded_model_run(
                requested_model,
                "degraded_unavailable",
                request_hash,
                format!("could not read OpenAI response: {error}"),
            );
        }
    };
    if !status.is_success() {
        let error_body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        let error_code = error_body
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let error_param = error_body
            .pointer("/error/param")
            .and_then(Value::as_str)
            .unwrap_or("unspecified");
        let error_message = error_body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("OpenAI did not return a structured error message")
            .chars()
            .take(1_000)
            .collect::<String>();
        return degraded_model_run(
            requested_model,
            "degraded_unavailable",
            request_hash,
            format!(
                "OpenAI Responses returned HTTP {status} ({error_code}) for {error_param}: {error_message}"
            ),
        );
    }
    let response: Value = match serde_json::from_slice(&body) {
        Ok(response) => response,
        Err(error) => {
            return degraded_model_run(
                requested_model,
                "invalid_output",
                request_hash,
                format!("OpenAI response was not valid JSON: {error}"),
            );
        }
    };
    let returned_model = response
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
    let mut refusal = None;
    let mut output_text = None;
    for content in response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
    {
        match content.get("type").and_then(Value::as_str) {
            Some("output_text") => {
                output_text = content
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            Some("refusal") => {
                refusal = content
                    .get("refusal")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            _ => {}
        }
    }
    if let Some(refusal) = refusal {
        return ModelRun {
            status: "refused".to_owned(),
            requested_model,
            returned_model,
            response_id,
            request_hash,
            output: json!({"refusal": refusal}),
            usage,
            degradation_reason: Some(
                "the model refused the bounded consolidation request".to_owned(),
            ),
            candidates: vec![],
            expected_queries: vec![],
            validation_errors: vec![],
        };
    }
    let Some(output_text) = output_text else {
        return ModelRun {
            status: "invalid_output".to_owned(),
            requested_model,
            returned_model,
            response_id,
            request_hash,
            output: json!({}),
            usage,
            degradation_reason: Some("OpenAI response did not contain output_text".to_owned()),
            candidates: vec![],
            expected_queries: vec![],
            validation_errors: vec![],
        };
    };
    let output_value = match serde_json::from_str::<Value>(&output_text) {
        Ok(value) => value,
        Err(error) => {
            return ModelRun {
                status: "invalid_output".to_owned(),
                requested_model,
                returned_model,
                response_id,
                request_hash,
                output: json!({"unparsed_output": output_text}),
                usage,
                degradation_reason: Some(format!("structured output was invalid JSON: {error}")),
                candidates: vec![],
                expected_queries: vec![],
                validation_errors: vec![],
            };
        }
    };
    let parsed = match serde_json::from_value::<ModelCandidateOutput>(output_value.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            return ModelRun {
                status: "invalid_output".to_owned(),
                requested_model,
                returned_model,
                response_id,
                request_hash,
                output: output_value,
                usage,
                degradation_reason: Some(format!("structured output shape was invalid: {error}")),
                candidates: vec![],
                expected_queries: vec![],
                validation_errors: vec![],
            };
        }
    };
    let validation_errors = validate_model_output(
        &parsed,
        member_ids,
        allowed_source_refs,
        max_candidate_items,
        max_evaluation_queries,
    );
    let completed = validation_errors.is_empty();
    ModelRun {
        status: if completed {
            "completed"
        } else {
            "invalid_output"
        }
        .to_owned(),
        requested_model,
        returned_model,
        response_id,
        request_hash,
        output: output_value,
        usage,
        degradation_reason: (!completed)
            .then(|| "model output failed deterministic target or source validation".to_owned()),
        candidates: if completed {
            parsed.candidate_items
        } else {
            vec![]
        },
        expected_queries: if completed {
            parsed.expected_queries
        } else {
            vec![]
        },
        validation_errors,
    }
}

fn safety_identifier(secret: &str, user_id: Uuid) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts continuation secrets of any length");
    mac.update(b"straylight:dream-safety-identifier:v1:");
    mac.update(user_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn degraded_model_run(
    requested_model: String,
    status: &str,
    request_hash: String,
    reason: String,
) -> ModelRun {
    ModelRun {
        status: status.to_owned(),
        requested_model,
        returned_model: None,
        response_id: None,
        request_hash,
        output: json!({}),
        usage: json!({}),
        degradation_reason: Some(reason),
        candidates: vec![],
        expected_queries: vec![],
        validation_errors: vec![],
    }
}

fn validate_model_output(
    output: &ModelCandidateOutput,
    member_ids: &BTreeSet<Uuid>,
    allowed_source_refs: &BTreeSet<Uuid>,
    max_candidate_items: usize,
    max_evaluation_queries: usize,
) -> Vec<Value> {
    let mut errors = Vec::new();
    if output.candidate_items.len() > max_candidate_items {
        errors.push(json!({
            "code": "candidate_limit_exceeded",
            "actual": output.candidate_items.len(),
            "maximum": max_candidate_items,
        }));
    }
    if output.expected_queries.len() > max_evaluation_queries {
        errors.push(json!({
            "code": "query_limit_exceeded",
            "actual": output.expected_queries.len(),
            "maximum": max_evaluation_queries,
        }));
    }
    if output.unresolved.len() > 128 {
        errors.push(json!({
            "code": "unresolved_limit_exceeded",
            "actual": output.unresolved.len(),
            "maximum": 128,
        }));
    }
    for (index, unresolved) in output.unresolved.iter().enumerate() {
        if unresolved.trim().is_empty() || unresolved.chars().count() > 1_000 {
            errors.push(json!({
                "code": "unresolved_text_invalid",
                "unresolved_index": index,
            }));
        }
    }
    let allowed_dispositions = [
        "add_derived_view",
        "add_alias",
        "add_soft_link",
        "add_cluster",
        "add_flag",
        "propose_same_as",
        "propose_supersession",
    ];
    for (index, item) in output.candidate_items.iter().enumerate() {
        if !member_ids.contains(&item.target_record_id) {
            errors.push(json!({
                "code": "target_outside_pinned_region",
                "candidate_index": index,
                "target_record_id": item.target_record_id,
            }));
        }
        if !allowed_dispositions.contains(&item.disposition.as_str()) {
            errors.push(json!({
                "code": "unsupported_disposition",
                "candidate_index": index,
                "disposition": item.disposition,
            }));
        }
        if item.title.trim().is_empty()
            || item.content.trim().is_empty()
            || item.rationale.trim().is_empty()
            || item.title.chars().count() > 240
            || item.content.chars().count() > 12_000
            || item.rationale.chars().count() > 2_000
        {
            errors.push(json!({
                "code": "candidate_content_invalid",
                "candidate_index": index,
            }));
        }
        if item.source_refs.is_empty()
            || item
                .source_refs
                .iter()
                .any(|source| !allowed_source_refs.contains(source))
        {
            errors.push(json!({
                "code": "candidate_source_lineage_invalid",
                "candidate_index": index,
                "source_refs": item.source_refs,
            }));
        }
        if item.source_refs.iter().collect::<BTreeSet<_>>().len() != item.source_refs.len() {
            errors.push(json!({
                "code": "candidate_source_lineage_duplicated",
                "candidate_index": index,
            }));
        }
        if item.claims.len() > 128 || item.query_aids.len() > 64 {
            errors.push(json!({
                "code": "candidate_nested_limit_exceeded",
                "candidate_index": index,
                "claim_count": item.claims.len(),
                "query_aid_count": item.query_aids.len(),
            }));
        }
        if item
            .query_aids
            .iter()
            .any(|aid| aid.trim().is_empty() || aid.chars().count() > 240)
        {
            errors.push(json!({
                "code": "candidate_query_aid_invalid",
                "candidate_index": index,
            }));
        }
        let risky = matches!(
            item.disposition.as_str(),
            "propose_same_as" | "propose_supersession"
        );
        if (risky && item.authority_effect != "review_required")
            || (!matches!(item.authority_effect.as_str(), "none" | "review_required"))
        {
            errors.push(json!({
                "code": "authority_effect_invalid",
                "candidate_index": index,
                "disposition": item.disposition,
                "authority_effect": item.authority_effect,
            }));
        }
        for (claim_index, claim) in item.claims.iter().enumerate() {
            if claim.text.trim().is_empty()
                || claim.text.chars().count() > 2_000
                || claim.source_refs.is_empty()
                || claim
                    .source_refs
                    .iter()
                    .any(|source| !allowed_source_refs.contains(source))
            {
                errors.push(json!({
                    "code": "candidate_claim_lineage_invalid",
                    "candidate_index": index,
                    "claim_index": claim_index,
                    "source_refs": claim.source_refs,
                }));
            }
            if claim.source_refs.iter().collect::<BTreeSet<_>>().len() != claim.source_refs.len() {
                errors.push(json!({
                    "code": "candidate_claim_lineage_duplicated",
                    "candidate_index": index,
                    "claim_index": claim_index,
                }));
            }
        }
    }
    for (index, query) in output.expected_queries.iter().enumerate() {
        if query.family.trim().is_empty()
            || query.query.trim().is_empty()
            || query.family.chars().count() > 120
            || query.query.chars().count() > 1_000
            || query.expected_source_refs.is_empty()
            || query
                .expected_source_refs
                .iter()
                .any(|source| !allowed_source_refs.contains(source))
        {
            errors.push(json!({
                "code": "expected_query_lineage_invalid",
                "query_index": index,
                "expected_source_refs": query.expected_source_refs,
            }));
        }
        if query
            .expected_source_refs
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != query.expected_source_refs.len()
        {
            errors.push(json!({
                "code": "expected_query_lineage_duplicated",
                "query_index": index,
            }));
        }
    }
    errors
}

async fn insert_model_receipt(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    dream_id: Uuid,
    model_run: &ModelRun,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_model_receipts (
          id,user_id,scope_id,dream_job_id,provider,requested_model,
          returned_model,response_id,request_hash,prompt_version,status,output,
          validation,usage,degradation_reason
        ) VALUES ($1,$2,$3,$4,'openai',$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(dream_id)
    .bind(&model_run.requested_model)
    .bind(&model_run.returned_model)
    .bind(&model_run.response_id)
    .bind(&model_run.request_hash)
    .bind(PROMPT_VERSION)
    .bind(&model_run.status)
    .bind(&model_run.output)
    .bind(json!({"errors": model_run.validation_errors}))
    .bind(&model_run.usage)
    .bind(&model_run.degradation_reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn merge_model_candidates(
    dispositions: &mut [CandidateDisposition],
    candidates: &[ModelCandidateItem],
) {
    let mut by_target = BTreeMap::new();
    for (index, disposition) in dispositions.iter().enumerate() {
        by_target.insert(disposition.target_record_id, index);
    }
    for candidate in candidates {
        let Some(index) = by_target.get(&candidate.target_record_id).copied() else {
            continue;
        };
        let disposition = &mut dispositions[index];
        if disposition_priority(&candidate.disposition)
            > disposition_priority(&disposition.disposition)
        {
            disposition.disposition.clone_from(&candidate.disposition);
        }
        disposition.requires_review |= candidate.authority_effect == "review_required"
            || !matches!(
                candidate.disposition.as_str(),
                "add_derived_view" | "add_alias" | "add_soft_link" | "add_cluster" | "add_flag"
            );
        let mut source_refs: BTreeSet<_> = disposition.source_refs.iter().copied().collect();
        source_refs.extend(candidate.source_refs.iter().copied());
        for claim in &candidate.claims {
            source_refs.extend(claim.source_refs.iter().copied());
        }
        disposition.source_refs = source_refs.into_iter().collect();
        let proposal = disposition
            .proposal
            .as_object_mut()
            .expect("candidate proposal is always an object");
        proposal
            .entry("model_candidates".to_owned())
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .expect("model_candidates is initialized as an array")
            .push(serde_json::to_value(candidate).expect("model candidate serialization"));
    }
}

fn disposition_priority(disposition: &str) -> u8 {
    match disposition {
        "propose_scope_move" | "propose_tombstone" => 7,
        "propose_same_as" | "propose_supersession" => 6,
        "add_derived_view" => 5,
        "add_alias" | "add_soft_link" => 4,
        "add_flag" => 3,
        "add_cluster" => 2,
        _ => 1,
    }
}

fn active_retrieval_documents(documents: &[SourceDocument]) -> Vec<RetrievalDocument> {
    documents
        .iter()
        .map(|document| RetrievalDocument {
            record_id: document.record_id,
            record_kind: document.record_kind.clone(),
            text: document.text.clone(),
            source_refs: document.source_refs.clone(),
            origin: "active".to_owned(),
        })
        .collect()
}

fn candidate_retrieval_documents(candidates: &[ModelCandidateItem]) -> Vec<RetrievalDocument> {
    candidates
        .iter()
        .map(|candidate| {
            let mut parts = vec![candidate.title.clone(), candidate.content.clone()];
            parts.extend(candidate.claims.iter().map(|claim| claim.text.clone()));
            parts.extend(candidate.query_aids.iter().cloned());
            RetrievalDocument {
                record_id: candidate.target_record_id,
                record_kind: "shadow_derived_view".to_owned(),
                text: parts.join("\n"),
                source_refs: candidate.source_refs.clone(),
                origin: "candidate".to_owned(),
            }
        })
        .collect()
}

fn lexical_tokens(value: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "and", "are", "for", "from", "has", "have", "into", "its", "not", "that", "the", "this",
        "was", "were", "what", "when", "where", "which", "with",
    ];
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-' | ':' | '.' | '/' | '@') {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            if current.len() >= 3 && !STOP_WORDS.contains(&current.as_str()) {
                tokens.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if !current.is_empty() && current.len() >= 3 && !STOP_WORDS.contains(&current.as_str()) {
        tokens.push(current);
    }
    tokens
}

fn retrieve(documents: &[RetrievalDocument], query: &str, limit: usize) -> Vec<RetrievalHit> {
    let query_tokens: BTreeSet<_> = lexical_tokens(query).into_iter().collect();
    if query_tokens.is_empty() {
        return vec![];
    }
    let normalized_query = query.trim().to_lowercase();
    let mut by_record: BTreeMap<Uuid, RetrievalHit> = BTreeMap::new();
    for document in documents {
        let document_tokens = lexical_tokens(&document.text);
        let counts = document_tokens
            .into_iter()
            .fold(HashMap::new(), |mut counts, token| {
                *counts.entry(token).or_insert(0_i64) += 1;
                counts
            });
        let matched = query_tokens
            .iter()
            .filter_map(|token| counts.get(token).map(|count| (token, count)))
            .collect::<Vec<_>>();
        if matched.is_empty() {
            continue;
        }
        let all_tokens = matched.len() == query_tokens.len();
        let phrase_match = !normalized_query.is_empty()
            && document.text.to_lowercase().contains(&normalized_query);
        let score = matched.len() as i64 * 100
            + matched
                .iter()
                .map(|(_, count)| (**count).min(5) * 10)
                .sum::<i64>()
            + if all_tokens { 500 } else { 0 }
            + if phrase_match { 1_000 } else { 0 }
            + if document.origin == "active" { 1 } else { 0 };
        let hit = RetrievalHit {
            record_id: document.record_id,
            record_kind: document.record_kind.clone(),
            score,
            source_refs: document.source_refs.clone(),
            origin: document.origin.clone(),
        };
        match by_record.get(&document.record_id) {
            Some(existing) if existing.score >= hit.score => {}
            _ => {
                by_record.insert(document.record_id, hit);
            }
        }
    }
    let mut hits: Vec<_> = by_record.into_values().collect();
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.record_id.cmp(&right.record_id))
    });
    hits.truncate(limit);
    hits
}

fn build_frozen_queries(
    active_documents: &[RetrievalDocument],
    expected_query_families: &[String],
    model_queries: &[ModelExpectedQuery],
    maximum: usize,
) -> Vec<FrozenQuery> {
    let mut queries = Vec::new();
    for family in expected_query_families {
        if queries.len() >= maximum {
            break;
        }
        let baseline = retrieve(active_documents, family, EVALUATION_TOP_K);
        queries.push(frozen_query(
            family,
            family,
            baseline.iter().map(|hit| hit.record_id).collect(),
            baseline
                .iter()
                .flat_map(|hit| hit.source_refs.iter().copied())
                .collect(),
            "requested_family",
        ));
    }
    for query in model_queries {
        if queries.len() >= maximum {
            break;
        }
        let expected_sources: BTreeSet<_> = query.expected_source_refs.iter().copied().collect();
        let expected_records = active_documents
            .iter()
            .filter(|document| {
                document
                    .source_refs
                    .iter()
                    .any(|source| expected_sources.contains(source))
            })
            .map(|document| document.record_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        queries.push(frozen_query(
            &query.family,
            &query.query,
            expected_records,
            query.expected_source_refs.clone(),
            "model_generated",
        ));
    }

    let mut document_frequency: HashMap<String, usize> = HashMap::new();
    let mut token_sets = Vec::with_capacity(active_documents.len());
    for document in active_documents {
        let tokens: BTreeSet<_> = lexical_tokens(&document.text).into_iter().collect();
        for token in &tokens {
            *document_frequency.entry(token.clone()).or_default() += 1;
        }
        token_sets.push(tokens);
    }
    for (document, tokens) in active_documents.iter().zip(token_sets) {
        if queries.len() >= maximum {
            break;
        }
        let mut tokens: Vec<_> = tokens
            .into_iter()
            .filter(|token| !looks_like_uuid(token))
            .collect();
        tokens.sort_by(|left, right| {
            document_frequency
                .get(left)
                .cmp(&document_frequency.get(right))
                .then_with(|| right.len().cmp(&left.len()))
                .then_with(|| left.cmp(right))
        });
        tokens.truncate(3);
        if tokens.is_empty() {
            continue;
        }
        queries.push(frozen_query(
            &format!("generated_{}", document.record_kind),
            &tokens.join(" "),
            vec![document.record_id],
            document.source_refs.clone(),
            "deterministic_generated",
        ));
    }
    queries
}

fn frozen_query(
    family: &str,
    query: &str,
    expected_record_ids: Vec<Uuid>,
    expected_source_refs: Vec<Uuid>,
    origin: &str,
) -> FrozenQuery {
    let material = json!({
        "family": family,
        "query": query,
        "expected_record_ids": expected_record_ids,
        "expected_source_refs": expected_source_refs,
        "origin": origin,
    });
    FrozenQuery {
        query_ref: format!("query:{}", &hash_json(&material)[..16]),
        family: family.to_owned(),
        query: query.to_owned(),
        expected_record_ids,
        expected_source_refs,
        origin: origin.to_owned(),
    }
}

fn looks_like_uuid(value: &str) -> bool {
    Uuid::parse_str(value.trim_matches(|character| matches!(character, ':' | '/' | '.'))).is_ok()
}

fn query_passed(query: &FrozenQuery, hits: &[RetrievalHit]) -> bool {
    if query.expected_record_ids.is_empty() && query.expected_source_refs.is_empty() {
        return hits.is_empty();
    }
    let expected_records: BTreeSet<_> = query.expected_record_ids.iter().copied().collect();
    let expected_sources: BTreeSet<_> = query.expected_source_refs.iter().copied().collect();
    hits.iter().any(|hit| {
        expected_records.contains(&hit.record_id)
            || hit
                .source_refs
                .iter()
                .any(|source| expected_sources.contains(source))
    })
}

fn evaluate_retrieval(
    base_revision_id: Uuid,
    active_documents: &[RetrievalDocument],
    candidate_documents: &[RetrievalDocument],
    queries: &[FrozenQuery],
) -> RetrievalEvaluation {
    let mut candidate_corpus = active_documents.to_vec();
    candidate_corpus.extend_from_slice(candidate_documents);
    let mut traces = Vec::with_capacity(queries.len());
    let mut active_passes = 0usize;
    let mut candidate_passes = 0usize;
    let mut regressions = 0usize;
    let mut required_failures = 0usize;
    for query in queries {
        let active_hits = retrieve(active_documents, &query.query, EVALUATION_TOP_K);
        let candidate_hits = retrieve(&candidate_corpus, &query.query, EVALUATION_TOP_K);
        let active_passed = query_passed(query, &active_hits);
        let candidate_passed = query_passed(query, &candidate_hits);
        active_passes += usize::from(active_passed);
        candidate_passes += usize::from(candidate_passed);
        let regression = active_passed && !candidate_passed;
        regressions += usize::from(regression);
        if (!query.expected_record_ids.is_empty() || !query.expected_source_refs.is_empty())
            && !candidate_passed
        {
            required_failures += 1;
        }
        traces.push(json!({
            "query": query,
            "active": {"passed": active_passed, "hits": active_hits},
            "candidate": {"passed": candidate_passed, "hits": candidate_hits},
            "regression": regression,
        }));
    }
    let denominator = queries.len().max(1) as f64;
    let active_score = active_passes as f64 / denominator;
    let candidate_score = candidate_passes as f64 / denominator;
    let frozen_suite_hash = hash_json(&json!({
        "base_revision": base_revision_id,
        "queries": queries,
        "evaluator": EVALUATOR_VERSION,
        "retrieval_policy": RETRIEVAL_POLICY_VERSION,
    }));
    let passed = regressions == 0 && required_failures == 0;
    RetrievalEvaluation {
        frozen_suite_hash,
        result: json!({
            "mode": "paired_frozen_shadow_retrieval",
            "baseline_score": active_score,
            "candidate_score": candidate_score,
            "regressions": regressions,
            "required_failures": required_failures,
            "reader_queries_executed": queries.len() * 2,
            "query_count": queries.len(),
            "query_families": queries.iter().map(|query| query.family.clone()).collect::<BTreeSet<_>>(),
            "active_document_count": active_documents.len(),
            "candidate_derived_document_count": candidate_documents.len(),
            "same_reader": true,
            "same_tool_budget": true,
            "same_retrieval_policy": RETRIEVAL_POLICY_VERSION,
            "traces": traces,
        }),
        passed,
        regressions,
        candidate_score,
        active_score,
    }
}

async fn run_deterministic_maintenance(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    base_revision_id: Uuid,
    region_id: Uuid,
) -> ApiResult<MaintenanceReport> {
    let mut report = MaintenanceReport::default();

    let duplicate_rows = sqlx::query(
        r#"
        WITH region AS (
          SELECT record_id FROM straylight.dream_region_members
          WHERE user_id=$1 AND region_id=$4
        ), duplicate_sets AS (
          SELECT 'asset'::text AS record_kind,av.content_hash::text AS content_hash,
                 array_agg(asset.id ORDER BY asset.id) AS record_ids
          FROM straylight.assets AS asset
          JOIN region ON region.record_id=asset.id
          JOIN straylight.asset_versions AS av
            ON av.user_id=asset.user_id AND av.asset_id=asset.id
           AND av.version=asset.current_version
          WHERE asset.user_id=$1 AND asset.scope_id=$2
          GROUP BY av.content_hash HAVING count(*) > 1
          UNION ALL
          SELECT 'source_episode',source.content_hash::text,
                 array_agg(source.id ORDER BY source.id)
          FROM straylight.source_episodes AS source
          JOIN region ON region.record_id=source.id
          WHERE source.user_id=$1 AND source.scope_id=$2
            AND source.content_hash IS NOT NULL
          GROUP BY source.content_hash HAVING count(*) > 1
          UNION ALL
          SELECT 'document',revision.content_hash::text,
                 array_agg(document.id ORDER BY document.id)
          FROM straylight.documents AS document
          JOIN region ON region.record_id=document.id
          JOIN straylight.document_revisions AS revision
            ON revision.user_id=document.user_id
           AND revision.document_id=document.id
           AND revision.version=document.current_version
          WHERE document.user_id=$1 AND document.scope_id=$2
          GROUP BY revision.content_hash HAVING count(*) > 1
        )
        SELECT record_kind,content_hash,record_ids
        FROM duplicate_sets ORDER BY record_kind,content_hash
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in duplicate_rows {
        let record_kind: String = row.try_get("record_kind")?;
        let content_hash: String = row.try_get("content_hash")?;
        let record_ids: Vec<Uuid> = row.try_get("record_ids")?;
        for record_id in &record_ids {
            report.exact_duplicates.push(Finding {
                finding_ref: format!("exact_duplicate:{record_kind}:{content_hash}:{record_id}"),
                target_record_id: *record_id,
                category: "exact_duplicate".to_owned(),
                hard_integrity_issue: false,
                source_refs: record_ids.clone(),
                details: json!({
                    "record_kind": record_kind,
                    "content_hash": content_hash,
                    "duplicate_record_ids": record_ids,
                    "action": "add_non_destructive_duplicate_cluster",
                }),
            });
        }
    }

    let broken_rows = sqlx::query(
        r#"
        WITH region AS (
          SELECT record_id FROM straylight.dream_region_members
          WHERE user_id=$1 AND region_id=$4
        ), broken AS (
          SELECT member.record_id AS target_record_id,'missing_record_key'::text AS issue
          FROM straylight.corpus_members AS member
          LEFT JOIN straylight.record_keys AS key_row
            ON key_row.user_id=member.user_id AND key_row.record_id=member.record_id
          WHERE member.user_id=$1 AND member.scope_id=$2
            AND member.corpus_revision_id=$3 AND key_row.record_id IS NULL
          UNION ALL
          SELECT claim.id,'claim_missing_about_target'
          FROM straylight.claims AS claim JOIN region ON region.record_id=claim.id
          WHERE claim.user_id=$1 AND claim.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.claim_about_targets AS target
              WHERE target.user_id=claim.user_id AND target.claim_id=claim.id
            )
          UNION ALL
          SELECT claim.id,'claim_missing_provenance'
          FROM straylight.claims AS claim JOIN region ON region.record_id=claim.id
          WHERE claim.user_id=$1 AND claim.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.claim_evidence_links AS evidence_link
              WHERE evidence_link.user_id=claim.user_id
                AND evidence_link.claim_id=claim.id
                AND evidence_link.support_kind='supporting'
            )
            AND NOT EXISTS (
              SELECT 1 FROM straylight.claim_lineage AS lineage
              WHERE lineage.user_id=claim.user_id AND lineage.claim_id=claim.id
            )
          UNION ALL
          SELECT relation.id,'relation_missing_current_endpoints'
          FROM straylight.relations AS relation JOIN region ON region.record_id=relation.id
          WHERE relation.user_id=$1 AND relation.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.relation_endpoints AS endpoint
              WHERE endpoint.user_id=relation.user_id
                AND endpoint.relation_id=relation.id
                AND endpoint.relation_version=relation.current_version
            )
          UNION ALL
          SELECT relation.id,'relation_missing_current_evidence'
          FROM straylight.relations AS relation JOIN region ON region.record_id=relation.id
          WHERE relation.user_id=$1 AND relation.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.relation_evidence_links AS evidence_link
              WHERE evidence_link.user_id=relation.user_id
                AND evidence_link.relation_id=relation.id
                AND evidence_link.relation_version=relation.current_version
                AND evidence_link.support_kind='supporting'
            )
          UNION ALL
          SELECT object_row.id,'object_missing_current_revision_or_profile'
          FROM straylight.objects AS object_row JOIN region ON region.record_id=object_row.id
          WHERE object_row.user_id=$1 AND object_row.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.object_revisions AS revision
              JOIN straylight.object_revision_profiles AS profile
                ON profile.user_id=revision.user_id
               AND profile.object_id=revision.object_id
               AND profile.object_version=revision.version
              WHERE revision.user_id=object_row.user_id
                AND revision.object_id=object_row.id
                AND revision.version=object_row.current_version
            )
          UNION ALL
          SELECT document.id,'document_missing_current_revision'
          FROM straylight.documents AS document JOIN region ON region.record_id=document.id
          WHERE document.user_id=$1 AND document.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.document_revisions AS revision
              WHERE revision.user_id=document.user_id
                AND revision.document_id=document.id
                AND revision.version=document.current_version
            )
          UNION ALL
          SELECT asset.id,'asset_missing_current_version'
          FROM straylight.assets AS asset JOIN region ON region.record_id=asset.id
          WHERE asset.user_id=$1 AND asset.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.asset_versions AS version
              WHERE version.user_id=asset.user_id
                AND version.asset_id=asset.id
                AND version.version=asset.current_version
            )
        )
        SELECT target_record_id,issue FROM broken ORDER BY issue,target_record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in broken_rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let issue: String = row.try_get("issue")?;
        report.broken_references.push(Finding {
            finding_ref: format!("broken_reference:{issue}:{target_record_id}"),
            target_record_id,
            category: "broken_reference".to_owned(),
            hard_integrity_issue: true,
            source_refs: vec![target_record_id],
            details: json!({"issue": issue}),
        });
    }

    let conflict_rows = sqlx::query(
        r#"
        WITH region AS (
          SELECT record_id FROM straylight.dream_region_members
          WHERE user_id=$1 AND region_id=$4
        ), active_claims AS (
          SELECT claim.*
          FROM straylight.claims AS claim
          JOIN straylight.corpus_members AS member
            ON member.user_id=claim.user_id AND member.record_id=claim.id
           AND member.corpus_revision_id=$3 AND member.disposition='active'
          WHERE claim.user_id=$1 AND claim.scope_id=$2
            AND claim.support_state NOT IN ('refuted')
            AND NOT EXISTS (
              SELECT 1
              FROM straylight.claim_lineage AS successor_edge
              JOIN straylight.corpus_members AS successor_member
                ON successor_member.user_id=successor_edge.user_id
               AND successor_member.record_id=successor_edge.claim_id
               AND successor_member.corpus_revision_id=$3
               AND successor_member.disposition='active'
              WHERE successor_edge.user_id=claim.user_id
                AND successor_edge.predecessor_claim_id=claim.id
                AND successor_edge.lineage_kind IN ('corrects','supersedes','retracts')
            )
        )
        SELECT
          target.target_record_id,claim.predicate::text AS predicate,
          claim.valid_temporal_id,
          array_agg(claim.id ORDER BY claim.recorded_at,claim.id) AS claim_ids,
          jsonb_agg(jsonb_build_object(
            'claim_id',claim.id,'value',claim.value,'support_state',claim.support_state,
            'authority',claim.authority,'canonicality',claim.canonicality
          ) ORDER BY claim.recorded_at,claim.id) AS claims
        FROM active_claims AS claim
        JOIN straylight.claim_about_targets AS target
          ON target.user_id=claim.user_id AND target.claim_id=claim.id
        JOIN region ON region.record_id=target.target_record_id
        GROUP BY target.target_record_id,claim.predicate,claim.valid_temporal_id
        HAVING count(DISTINCT claim.value::text) > 1
        ORDER BY target.target_record_id,claim.predicate
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in conflict_rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let predicate: String = row.try_get("predicate")?;
        let claim_ids: Vec<Uuid> = row.try_get("claim_ids")?;
        report.conflicting_claim_heads.push(Finding {
            finding_ref: format!("conflicting_claim_heads:{target_record_id}:{predicate}"),
            target_record_id,
            category: "conflicting_claim_heads".to_owned(),
            hard_integrity_issue: false,
            source_refs: claim_ids,
            details: json!({
                "predicate": predicate,
                "valid_temporal_id": row.try_get::<Option<Uuid>, _>("valid_temporal_id")?,
                "claims": row.try_get::<Value, _>("claims")?,
                "action": "preserve disagreement and add a conflict flag",
            }),
        });
    }

    let stale_rows = sqlx::query(
        r#"
        SELECT source.id,source.source_ref,source.source_version,source.captured_at,
               source.metadata
        FROM straylight.source_episodes AS source
        JOIN straylight.dream_region_members AS member
          ON member.user_id=source.user_id AND member.record_id=source.id
         AND member.region_id=$3
        WHERE source.user_id=$1 AND source.scope_id=$2
          AND (
            lower(coalesce(source.metadata->>'available','true')) IN ('false','no','0')
            OR lower(coalesce(source.metadata->>'stale','false')) IN ('true','yes','1')
            OR lower(coalesce(source.metadata->>'deleted','false')) IN ('true','yes','1')
            OR (
              source.metadata ? 'valid_until'
              AND pg_input_is_valid(source.metadata->>'valid_until','timestamp with time zone')
              AND (source.metadata->>'valid_until')::timestamptz < clock_timestamp()
            )
          )
        ORDER BY source.id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in stale_rows {
        let target_record_id: Uuid = row.try_get("id")?;
        report.stale_sources.push(Finding {
            finding_ref: format!("stale_source:{target_record_id}"),
            target_record_id,
            category: "stale_source".to_owned(),
            hard_integrity_issue: false,
            source_refs: vec![target_record_id],
            details: json!({
                "source_ref": row.try_get::<String, _>("source_ref")?,
                "source_version": row.try_get::<Option<String>, _>("source_version")?,
                "captured_at": row.try_get::<DateTime<Utc>, _>("captured_at")?,
                "metadata": row.try_get::<Value, _>("metadata")?,
                "action": "add_staleness_or_source_availability_flag",
            }),
        });
    }

    let recurrence_rows = sqlx::query(
        r#"
        WITH region AS (
          SELECT record_id FROM straylight.dream_region_members
          WHERE user_id=$1 AND region_id=$4
        ), issues AS (
          SELECT occurrence.occurrence_object_id AS target_record_id,
                 'missing_current_occurrence_revision'::text AS issue,
                 occurrence.occurrence_object_id AS source_ref
          FROM straylight.event_occurrences AS occurrence
          JOIN region ON region.record_id=occurrence.occurrence_object_id
          JOIN straylight.objects AS object_row
            ON object_row.user_id=occurrence.user_id
           AND object_row.id=occurrence.occurrence_object_id
          WHERE occurrence.user_id=$1 AND occurrence.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.event_occurrence_revisions AS revision
              WHERE revision.user_id=occurrence.user_id
                AND revision.occurrence_object_id=occurrence.occurrence_object_id
                AND revision.object_version=object_row.current_version
            )
          UNION ALL
          SELECT exclusion.recurrence_spec_id,'exclusion_evidence_not_active',exclusion.evidence_id
          FROM straylight.recurrence_exclusions AS exclusion
          JOIN region ON region.record_id=exclusion.recurrence_spec_id
          WHERE exclusion.user_id=$1
            AND NOT EXISTS (
              SELECT 1 FROM straylight.corpus_members AS member
              WHERE member.user_id=exclusion.user_id
                AND member.corpus_revision_id=$3
                AND member.record_id=exclusion.evidence_id
                AND member.disposition='active'
            )
          UNION ALL
          SELECT addition.recurrence_spec_id,'addition_evidence_not_active',addition.evidence_id
          FROM straylight.recurrence_additions AS addition
          JOIN region ON region.record_id=addition.recurrence_spec_id
          WHERE addition.user_id=$1
            AND NOT EXISTS (
              SELECT 1 FROM straylight.corpus_members AS member
              WHERE member.user_id=addition.user_id
                AND member.corpus_revision_id=$3
                AND member.record_id=addition.evidence_id
                AND member.disposition='active'
            )
        )
        SELECT target_record_id,issue,source_ref FROM issues
        ORDER BY issue,target_record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in recurrence_rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let source_ref: Uuid = row.try_get("source_ref")?;
        let issue: String = row.try_get("issue")?;
        report.recurrence_issues.push(Finding {
            finding_ref: format!("recurrence_integrity:{issue}:{target_record_id}"),
            target_record_id,
            category: "recurrence_integrity".to_owned(),
            hard_integrity_issue: true,
            source_refs: vec![source_ref],
            details: json!({"issue": issue, "preserve_occurrence_identity": true}),
        });
    }

    let state_rows = sqlx::query(
        r#"
        WITH region AS (
          SELECT record_id FROM straylight.dream_region_members
          WHERE user_id=$1 AND region_id=$4
        ), issues AS (
          SELECT head.target_record_id,'state_head_claim_not_active'::text AS issue,
                 head.current_claim_id AS source_ref
          FROM straylight.state_heads AS head
          JOIN region ON region.record_id=head.target_record_id
          WHERE head.user_id=$1 AND head.scope_id=$2
            AND NOT EXISTS (
              SELECT 1 FROM straylight.corpus_members AS member
              WHERE member.user_id=head.user_id
                AND member.corpus_revision_id=$3
                AND member.record_id=head.current_claim_id
                AND member.disposition='active'
            )
          UNION ALL
          SELECT head.target_record_id,'state_head_assignment_mismatch',head.current_claim_id
          FROM straylight.state_heads AS head
          JOIN region ON region.record_id=head.target_record_id
          JOIN straylight.state_assignments AS assignment
            ON assignment.user_id=head.user_id AND assignment.claim_id=head.current_claim_id
          WHERE head.user_id=$1 AND head.scope_id=$2
            AND (
              assignment.target_record_id<>head.target_record_id
              OR assignment.state_machine_ref::text<>head.state_machine_ref::text
            )
        )
        SELECT target_record_id,issue,source_ref FROM issues
        ORDER BY issue,target_record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in state_rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let source_ref: Uuid = row.try_get("source_ref")?;
        let issue: String = row.try_get("issue")?;
        report.state_issues.push(Finding {
            finding_ref: format!("state_integrity:{issue}:{target_record_id}"),
            target_record_id,
            category: "state_integrity".to_owned(),
            hard_integrity_issue: true,
            source_refs: vec![source_ref],
            details: json!({
                "issue": issue,
                "cross_machine_inference_allowed": false,
            }),
        });
    }

    let policy_rows = sqlx::query(
        r#"
        WITH region AS (
          SELECT record_id FROM straylight.dream_region_members
          WHERE user_id=$1 AND region_id=$3
        ), issues AS (
          SELECT binding.target_record_id,'policy_binding_scope_mismatch'::text AS issue,
                 binding.id AS source_ref
          FROM straylight.policy_bindings AS binding
          JOIN region ON region.record_id=binding.target_record_id
          JOIN straylight.record_keys AS key_row
            ON key_row.user_id=binding.user_id AND key_row.record_id=binding.target_record_id
          WHERE binding.user_id=$1 AND binding.scope_id<>key_row.scope_id
          UNION ALL
          SELECT binding.target_record_id,'field_policy_binding_scope_mismatch',binding.id
          FROM straylight.field_policy_bindings AS binding
          JOIN region ON region.record_id=binding.target_record_id
          JOIN straylight.record_keys AS key_row
            ON key_row.user_id=binding.user_id AND key_row.record_id=binding.target_record_id
          WHERE binding.user_id=$1 AND binding.scope_id<>key_row.scope_id
          UNION ALL
          SELECT member.record_id,'region_member_scope_mismatch',member.record_id
          FROM region AS member
          JOIN straylight.record_keys AS key_row
            ON key_row.user_id=$1 AND key_row.record_id=member.record_id
          WHERE key_row.scope_id<>$2
        )
        SELECT target_record_id,issue,source_ref FROM issues
        ORDER BY issue,target_record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in policy_rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let source_ref: Uuid = row.try_get("source_ref")?;
        let issue: String = row.try_get("issue")?;
        report.policy_issues.push(Finding {
            finding_ref: format!("policy_integrity:{issue}:{target_record_id}"),
            target_record_id,
            category: "policy_integrity".to_owned(),
            hard_integrity_issue: true,
            source_refs: vec![source_ref],
            details: json!({"issue": issue, "fail_closed": true}),
        });
    }

    let lineage_rows = sqlx::query(
        r#"
        WITH RECURSIVE
        region_claims AS (
          SELECT claim.id
          FROM straylight.claims AS claim
          JOIN straylight.dream_region_members AS member
            ON member.user_id=claim.user_id AND member.record_id=claim.id
           AND member.region_id=$4
          WHERE claim.user_id=$1 AND claim.scope_id=$2
        ),
        edges AS (
          SELECT lineage.claim_id,lineage.predecessor_claim_id
          FROM straylight.claim_lineage AS lineage
          JOIN straylight.claims AS claim
            ON claim.user_id=lineage.user_id AND claim.id=lineage.claim_id
          WHERE lineage.user_id=$1 AND claim.scope_id=$2
        ),
        walk(start_id,current_id,path,cycle) AS (
          SELECT edge.claim_id,edge.predecessor_claim_id,
                 ARRAY[edge.claim_id,edge.predecessor_claim_id],
                 edge.predecessor_claim_id=edge.claim_id
          FROM edges AS edge JOIN region_claims ON region_claims.id=edge.claim_id
          UNION ALL
          SELECT walk.start_id,edge.predecessor_claim_id,
                 walk.path||edge.predecessor_claim_id,
                 edge.predecessor_claim_id=ANY(walk.path)
          FROM walk JOIN edges AS edge ON edge.claim_id=walk.current_id
          WHERE NOT walk.cycle AND cardinality(walk.path)<128
        ),
        ancestry(start_id,current_id,path) AS (
          SELECT id,id,ARRAY[id] FROM region_claims
          UNION ALL
          SELECT ancestry.start_id,edge.predecessor_claim_id,
                 ancestry.path||edge.predecessor_claim_id
          FROM ancestry JOIN edges AS edge ON edge.claim_id=ancestry.current_id
          WHERE NOT edge.predecessor_claim_id=ANY(ancestry.path)
            AND cardinality(ancestry.path)<128
        ),
        issues AS (
          SELECT DISTINCT walk.start_id AS target_record_id,
                 'claim_lineage_cycle'::text AS issue,walk.current_id AS source_ref
          FROM walk WHERE walk.cycle
          UNION ALL
          SELECT claim_row.id,'lineage_without_direct_evidence_terminal',claim_row.id
          FROM region_claims AS claim_row
          WHERE NOT EXISTS (
            SELECT 1 FROM ancestry
            JOIN straylight.claim_evidence_links AS evidence_link
              ON evidence_link.user_id=$1
             AND evidence_link.claim_id=ancestry.current_id
             AND evidence_link.support_kind='supporting'
            WHERE ancestry.start_id=claim_row.id
          )
          UNION ALL
          SELECT claim.id,'cross_scope_claim_evidence',evidence_link.evidence_id
          FROM straylight.claims AS claim
          JOIN region_claims ON region_claims.id=claim.id
          JOIN straylight.claim_evidence_links AS evidence_link
            ON evidence_link.user_id=claim.user_id AND evidence_link.claim_id=claim.id
          JOIN straylight.evidence_items AS evidence
            ON evidence.user_id=evidence_link.user_id AND evidence.id=evidence_link.evidence_id
          WHERE claim.user_id=$1 AND evidence.scope_id<>claim.scope_id
        )
        SELECT target_record_id,issue,source_ref FROM issues
        ORDER BY issue,target_record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(region_id)
    .fetch_all(&mut **transaction)
    .await?;
    for row in lineage_rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let source_ref: Uuid = row.try_get("source_ref")?;
        let issue: String = row.try_get("issue")?;
        report.lineage_issues.push(Finding {
            finding_ref: format!("lineage_integrity:{issue}:{target_record_id}"),
            target_record_id,
            category: "lineage_integrity".to_owned(),
            hard_integrity_issue: true,
            source_refs: vec![source_ref],
            details: json!({"issue": issue, "direct_source_lineage_required": true}),
        });
    }

    Ok(report)
}

fn build_disposition_map(
    members: &[RegionMember],
    report: &MaintenanceReport,
) -> Vec<CandidateDisposition> {
    let mut by_target = BTreeMap::new();
    for member in members {
        by_target.insert(
            member.record_id,
            CandidateDisposition {
                target_record_id: member.record_id,
                disposition: "keep".to_owned(),
                requires_review: false,
                source_refs: vec![member.record_id],
                proposal: json!({
                    "phase": PHASE,
                    "record_kind": member.record_kind,
                    "record_version": member.record_version,
                    "findings": [],
                    "omission_semantics": "keep",
                }),
            },
        );
    }
    for finding in report.findings() {
        let Some(item) = by_target.get_mut(&finding.target_record_id) else {
            continue;
        };
        if finding.category == "exact_duplicate" && item.disposition == "keep" {
            item.disposition = "add_cluster".to_owned();
        } else if finding.category != "exact_duplicate" {
            item.disposition = "add_flag".to_owned();
        }
        item.requires_review |= finding.hard_integrity_issue;
        let mut source_refs: BTreeSet<_> = item.source_refs.iter().copied().collect();
        source_refs.extend(finding.source_refs.iter().copied());
        item.source_refs = source_refs.into_iter().collect();
        if let Some(findings) = item
            .proposal
            .get_mut("findings")
            .and_then(Value::as_array_mut)
        {
            findings.push(serde_json::to_value(finding).expect("Finding serialization"));
        }
    }
    by_target.into_values().collect()
}

fn build_gates(
    report: &MaintenanceReport,
    member_count: usize,
    disposition_count: usize,
    safe_derived_additions_only: bool,
    job_type: &str,
    model_run: &ModelRun,
    evaluation: &RetrievalEvaluation,
    source_snapshot_hash_before: &str,
    source_snapshot_hash_after: &str,
    candidate_manifest_hash: &str,
) -> Vec<GateSpec> {
    let model_required = job_type == "deep_consolidation";
    let derived_claim_count = model_run
        .candidates
        .iter()
        .map(|candidate| candidate.claims.len())
        .sum::<usize>();
    let model_evidence_refs = model_run
        .candidates
        .iter()
        .flat_map(|candidate| candidate.source_refs.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    vec![
        GateSpec {
            gate_ref: "gate:references@v1".to_owned(),
            gate_kind: "reference".to_owned(),
            hard_gate: true,
            passed: report.broken_references.is_empty(),
            evidence_refs: finding_refs(&report.broken_references),
            details: json!({"broken_reference_count": report.broken_references.len()}),
        },
        GateSpec {
            gate_ref: "gate:hashes@v1".to_owned(),
            gate_kind: "hash".to_owned(),
            hard_gate: true,
            passed: source_snapshot_hash_before == source_snapshot_hash_after
                && source_snapshot_hash_before.len() == 64
                && candidate_manifest_hash.len() == 64,
            evidence_refs: vec![],
            details: json!({
                "source_snapshot_hash_before": source_snapshot_hash_before,
                "source_snapshot_hash_after": source_snapshot_hash_after,
                "candidate_manifest_hash": candidate_manifest_hash,
                "candidate_manifest": "sha256_canonical_json",
                "source_snapshot_reopened_without_mutation": true,
                "candidate_application_idempotent_by_hash": true,
            }),
        },
        GateSpec {
            gate_ref: "gate:temporal-recurrence@v1".to_owned(),
            gate_kind: "temporal".to_owned(),
            hard_gate: true,
            passed: report.recurrence_issues.is_empty(),
            evidence_refs: finding_refs(&report.recurrence_issues),
            details: json!({
                "recurrence_issue_count": report.recurrence_issues.len(),
                "occurrence_identity_rewrites": 0,
            }),
        },
        GateSpec {
            gate_ref: "gate:policy-boundary@v1".to_owned(),
            gate_kind: "policy".to_owned(),
            hard_gate: true,
            passed: report.policy_issues.is_empty(),
            evidence_refs: finding_refs(&report.policy_issues),
            details: json!({"policy_issue_count": report.policy_issues.len()}),
        },
        GateSpec {
            gate_ref: "gate:direct-lineage@v1".to_owned(),
            gate_kind: "lineage".to_owned(),
            hard_gate: true,
            passed: report.lineage_issues.is_empty() && model_run.validation_errors.is_empty(),
            evidence_refs: {
                let mut refs = finding_refs(&report.lineage_issues);
                refs.extend(model_evidence_refs);
                refs.sort_unstable();
                refs.dedup();
                refs
            },
            details: json!({
                "lineage_issue_count": report.lineage_issues.len(),
                "model_validation_errors": model_run.validation_errors,
                "derived_claims_created": derived_claim_count,
                "direct_source_refs_required": true,
            }),
        },
        GateSpec {
            gate_ref: "gate:named-state-machines@v1".to_owned(),
            gate_kind: "state_machine".to_owned(),
            hard_gate: true,
            passed: report.state_issues.is_empty(),
            evidence_refs: finding_refs(&report.state_issues),
            details: json!({
                "state_issue_count": report.state_issues.len(),
                "cross_machine_promotions": 0,
            }),
        },
        GateSpec {
            gate_ref: "gate:canonical-heads@v1".to_owned(),
            gate_kind: "canonical_head".to_owned(),
            hard_gate: true,
            passed: true,
            evidence_refs: finding_refs(&report.conflicting_claim_heads),
            details: json!({
                "existing_conflict_clusters": report.conflicting_claim_heads.len(),
                "new_canonical_heads": 0,
                "conflicts_preserved_not_resolved": true,
                "risky_model_proposals_require_review": true,
            }),
        },
        GateSpec {
            gate_ref: "gate:complete-preservation@v1".to_owned(),
            gate_kind: "preservation".to_owned(),
            hard_gate: true,
            passed: member_count == disposition_count,
            evidence_refs: vec![],
            details: json!({
                "region_member_count": member_count,
                "disposition_count": disposition_count,
                "omissions_default_to_keep": true,
                "destructive_dispositions": 0,
            }),
        },
        GateSpec {
            gate_ref: "gate:structural-active-vs-candidate@v1".to_owned(),
            gate_kind: "retrieval_regression".to_owned(),
            hard_gate: true,
            passed: evaluation.passed,
            evidence_refs: vec![],
            details: json!({
                "active_score": evaluation.active_score,
                "candidate_score": evaluation.candidate_score,
                "regressions": evaluation.regressions,
                "safe_derived_additions_only": safe_derived_additions_only,
                "same_reader_and_policy": true,
                "frozen_suite_hash": evaluation.frozen_suite_hash,
            }),
        },
        GateSpec {
            gate_ref: "gate:model-consolidation@v1".to_owned(),
            gate_kind: "task_family".to_owned(),
            hard_gate: model_required,
            passed: !model_required || model_run.completed(),
            evidence_refs: vec![],
            details: json!({
                "required": model_required,
                "status": model_run.status,
                "requested_model": model_run.requested_model,
                "returned_model": model_run.returned_model,
                "degradation_reason": model_run.degradation_reason,
                "candidate_items": model_run.candidates.len(),
            }),
        },
    ]
}

fn finding_refs(findings: &[Finding]) -> Vec<Uuid> {
    findings
        .iter()
        .flat_map(|finding| finding.source_refs.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_safe_disposition(item: &CandidateDisposition) -> bool {
    matches!(
        item.disposition.as_str(),
        "keep" | "add_derived_view" | "add_alias" | "add_soft_link" | "add_cluster" | "add_flag"
    )
}

async fn job_detail_in_tx(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    dream_id: Uuid,
) -> ApiResult<Value> {
    let row = sqlx::query(
        r#"
        SELECT
          job.id,job.scope_id,job.status,job.job_type,job.trigger_reasons,
          job.base_corpus_revision_id,job.evidence_watermark,
          job.dream_policy_version,job.model_version,job.prompt_version,
          job.schema_version,job.evaluator_version,job.compute_budget,
          job.requested_by_credential_id,job.started_at,job.completed_at,job.created_at,
          scope_row.scope_ref,region.id AS region_id,region.region_ref,
          region.version AS region_version,region.root_set_hash,region.manifest_hash AS region_manifest_hash,
          candidate.id AS candidate_id,
          candidate.candidate_corpus_revision_id,candidate.candidate_manifest_hash,
          candidate.status AS candidate_status,
          manifest.id AS active_manifest_id,
          manifest.active_corpus_revision_id,manifest.manifest_hash AS active_manifest_hash,
          manifest.generation AS active_manifest_generation,
          coalesce((
            SELECT max(recorded_at) FROM straylight.dream_job_events AS event
            WHERE event.user_id=job.user_id AND event.dream_job_id=job.id
          ),job.created_at) AS updated_at
        FROM straylight.dream_jobs AS job
        JOIN straylight.scopes AS scope_row
          ON scope_row.user_id=job.user_id AND scope_row.id=job.scope_id
        JOIN straylight.dream_regions AS region
          ON region.user_id=job.user_id AND region.id=job.region_id
        LEFT JOIN straylight.dream_candidate_revisions AS candidate
          ON candidate.user_id=job.user_id AND candidate.dream_job_id=job.id
        JOIN straylight.active_manifests AS manifest
          ON manifest.user_id=job.user_id AND manifest.scope_id=job.scope_id
        WHERE job.user_id=$1 AND job.id=$2
        "#,
    )
    .bind(user_id)
    .bind(dream_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or_else(|| ApiError::not_found("dream_not_found", &dream_id.to_string()))?;

    let scope_id: Uuid = row.try_get("scope_id")?;
    let status: String = row.try_get("status")?;
    let candidate_id: Option<Uuid> = row.try_get("candidate_id")?;
    let candidate_corpus_revision_id: Option<Uuid> = row.try_get("candidate_corpus_revision_id")?;
    let mut diff = Vec::new();
    let mut disposition_summary = Map::new();
    let mut lineage_ids = BTreeSet::new();
    let mut diff_truncated = false;
    if let Some(candidate_id) = candidate_id {
        for summary in sqlx::query(
            r#"
            SELECT disposition,count(*) AS item_count
            FROM straylight.dream_candidate_items
            WHERE user_id=$1 AND candidate_revision_id=$2
            GROUP BY disposition ORDER BY disposition
            "#,
        )
        .bind(user_id)
        .bind(candidate_id)
        .fetch_all(&mut **transaction)
        .await?
        {
            disposition_summary.insert(
                summary.try_get::<String, _>("disposition")?,
                json!(summary.try_get::<i64, _>("item_count")?),
            );
        }
        let rows = sqlx::query(
            r#"
            SELECT target_record_id,disposition,requires_review,source_refs,proposal
            FROM straylight.dream_candidate_items
            WHERE user_id=$1 AND candidate_revision_id=$2 AND disposition<>'keep'
            ORDER BY target_record_id
            LIMIT $3
            "#,
        )
        .bind(user_id)
        .bind(candidate_id)
        .bind(MAX_DETAIL_ITEMS + 1)
        .fetch_all(&mut **transaction)
        .await?;
        diff_truncated = rows.len() as i64 > MAX_DETAIL_ITEMS;
        for item in rows.into_iter().take(MAX_DETAIL_ITEMS as usize) {
            let source_refs: Vec<Uuid> = item.try_get("source_refs")?;
            lineage_ids.extend(source_refs.iter().copied());
            let proposal: Value = item.try_get("proposal")?;
            let kind = if proposal.get("model_candidates").is_some() {
                "shadow_derived_view"
            } else {
                "shadow_derived_metadata"
            };
            diff.push(json!({
                "id": item.try_get::<Uuid, _>("target_record_id")?,
                "kind": kind,
                "target": item.try_get::<Uuid, _>("target_record_id")?,
                "disposition": item.try_get::<String, _>("disposition")?,
                "requires_review": item.try_get::<bool, _>("requires_review")?,
                "before": {"active": true, "unchanged": true},
                "after": proposal,
                "source_refs": source_refs,
            }));
        }
    }

    let mut evidence_by_id = BTreeMap::new();
    if !lineage_ids.is_empty() {
        let ids: Vec<_> = lineage_ids.iter().copied().collect();
        for evidence in sqlx::query(
            r#"
            SELECT evidence.id,evidence.locator,evidence.content_hash,
                   source.title,source.source_ref
            FROM straylight.evidence_items AS evidence
            JOIN straylight.source_episodes AS source
              ON source.user_id=evidence.user_id
             AND source.id=evidence.source_episode_id
            WHERE evidence.user_id=$1 AND evidence.id=ANY($2::uuid[])
            "#,
        )
        .bind(user_id)
        .bind(ids)
        .fetch_all(&mut **transaction)
        .await?
        {
            let id: Uuid = evidence.try_get("id")?;
            evidence_by_id.insert(
                id,
                json!({
                    "source_id": id,
                    "source_title": evidence.try_get::<Option<String>, _>("title")?,
                    "source_ref": evidence.try_get::<String, _>("source_ref")?,
                    "locator": evidence.try_get::<Value, _>("locator")?,
                    "hash": evidence.try_get::<String, _>("content_hash")?,
                }),
            );
        }
    }
    let lineage: Vec<_> = lineage_ids
        .into_iter()
        .map(|id| {
            evidence_by_id.get(&id).cloned().unwrap_or_else(|| {
                json!({
                    "source_id": id,
                    "source_title": "Source-bearing record",
                    "locator": null,
                    "hash": null,
                })
            })
        })
        .collect();

    let mut gates = Vec::new();
    if let Some(candidate_id) = candidate_id {
        for gate in sqlx::query(
            r#"
            SELECT id,gate_ref,gate_kind,hard_gate,passed,evidence_refs,details,evaluated_at
            FROM straylight.dream_gate_results
            WHERE user_id=$1 AND candidate_revision_id=$2
            ORDER BY hard_gate DESC,gate_ref
            "#,
        )
        .bind(user_id)
        .bind(candidate_id)
        .fetch_all(&mut **transaction)
        .await?
        {
            gates.push(json!({
                "id": gate.try_get::<Uuid, _>("id")?,
                "name": gate.try_get::<String, _>("gate_ref")?,
                "kind": gate.try_get::<String, _>("gate_kind")?,
                "hard_gate": gate.try_get::<bool, _>("hard_gate")?,
                "status": if gate.try_get::<bool, _>("passed")? { "passed" } else { "failed" },
                "evidence_refs": gate.try_get::<Vec<Uuid>, _>("evidence_refs")?,
                "details": gate.try_get::<Value, _>("details")?,
                "evaluated_at": gate.try_get::<DateTime<Utc>, _>("evaluated_at")?,
            }));
        }
    }
    let hard_gate_failures = gates
        .iter()
        .filter(|gate| gate["hard_gate"] == Value::Bool(true) && gate["status"] == "failed")
        .count();
    let unsafe_additions = diff
        .iter()
        .filter(|item| {
            !matches!(
                item["disposition"].as_str(),
                Some(
                    "keep"
                        | "add_derived_view"
                        | "add_alias"
                        | "add_soft_link"
                        | "add_cluster"
                        | "add_flag"
                )
            )
        })
        .count();

    let evaluation = if let Some(candidate_id) = candidate_id {
        sqlx::query(
            r#"
            SELECT id,result,passed,evaluated_at,reader_version,retrieval_policy_version
            FROM straylight.dream_evaluations
            WHERE user_id=$1 AND candidate_revision_id=$2
            ORDER BY evaluated_at DESC,id DESC LIMIT 1
            "#,
        )
        .bind(user_id)
        .bind(candidate_id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(|evaluation| -> Result<Value, sqlx::Error> {
            let mut result: Value = evaluation.try_get("result")?;
            result["id"] = json!(evaluation.try_get::<Uuid, _>("id")?);
            result["passed"] = json!(evaluation.try_get::<bool, _>("passed")?);
            result["evaluated_at"] = json!(evaluation.try_get::<DateTime<Utc>, _>("evaluated_at")?);
            result["reader_version"] = json!(evaluation.try_get::<String, _>("reader_version")?);
            result["retrieval_policy_version"] =
                json!(evaluation.try_get::<String, _>("retrieval_policy_version")?);
            Ok(result)
        })
        .transpose()?
    } else {
        None
    };
    let model_receipt = sqlx::query(
        r#"
        SELECT id,provider,requested_model,returned_model,response_id,request_hash,
               prompt_version,status,output,validation,usage,degradation_reason,created_at
        FROM straylight.dream_model_receipts
        WHERE user_id=$1 AND dream_job_id=$2
        "#,
    )
    .bind(user_id)
    .bind(dream_id)
    .fetch_optional(&mut **transaction)
    .await?
    .map(|receipt| -> Result<Value, sqlx::Error> {
        Ok(json!({
            "id": receipt.try_get::<Uuid, _>("id")?,
            "provider": receipt.try_get::<String, _>("provider")?,
            "requested_model": receipt.try_get::<String, _>("requested_model")?,
            "returned_model": receipt.try_get::<Option<String>, _>("returned_model")?,
            "response_id": receipt.try_get::<Option<String>, _>("response_id")?,
            "request_hash": receipt.try_get::<String, _>("request_hash")?,
            "prompt_version": receipt.try_get::<String, _>("prompt_version")?,
            "status": receipt.try_get::<String, _>("status")?,
            "output": receipt.try_get::<Value, _>("output")?,
            "validation": receipt.try_get::<Value, _>("validation")?,
            "usage": receipt.try_get::<Value, _>("usage")?,
            "degradation_reason": receipt.try_get::<Option<String>, _>("degradation_reason")?,
            "created_at": receipt.try_get::<DateTime<Utc>, _>("created_at")?,
        }))
    })
    .transpose()?;

    let events = sqlx::query(
        r#"
        SELECT id,from_status,to_status,details,recorded_at
        FROM straylight.dream_job_events
        WHERE user_id=$1 AND dream_job_id=$2
        ORDER BY recorded_at,id
        LIMIT 500
        "#,
    )
    .bind(user_id)
    .bind(dream_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|event| -> Result<Value, sqlx::Error> {
        Ok(json!({
            "id": event.try_get::<Uuid, _>("id")?,
            "action": format!(
                "{} -> {}",
                event.try_get::<Option<String>, _>("from_status")?.as_deref().unwrap_or("created"),
                event.try_get::<String, _>("to_status")?,
            ),
            "status": event.try_get::<String, _>("to_status")?,
            "actor": "Straylight dream control plane",
            "details": event.try_get::<Value, _>("details")?,
            "created_at": event.try_get::<DateTime<Utc>, _>("recorded_at")?,
        }))
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()?;

    let review = if let Some(candidate_id) = candidate_id {
        sqlx::query(
            r#"
            SELECT decision,reason,reviewer_ref,reviewed_at
            FROM straylight.dream_reviews
            WHERE user_id=$1 AND candidate_revision_id=$2
            ORDER BY reviewed_at DESC,id DESC LIMIT 1
            "#,
        )
        .bind(user_id)
        .bind(candidate_id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(|review| -> Result<Value, sqlx::Error> {
            Ok(json!({
                "decision": review.try_get::<String, _>("decision")?,
                "reviewer": review.try_get::<Uuid, _>("reviewer_ref")?,
                "note": review.try_get::<String, _>("reason")?,
                "decided_at": review.try_get::<DateTime<Utc>, _>("reviewed_at")?,
            }))
        })
        .transpose()?
    } else {
        None
    };
    let promotion_receipt = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(receipt) FROM straylight.dream_promotion_receipts AS receipt WHERE user_id=$1 AND dream_job_id=$2",
    )
    .bind(user_id)
    .bind(dream_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let rollback_receipt = if let Some(promotion) = promotion_receipt.as_ref() {
        let promotion_id = promotion["id"]
            .as_str()
            .and_then(|id| Uuid::parse_str(id).ok());
        if let Some(promotion_id) = promotion_id {
            sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(receipt) FROM straylight.dream_rollback_receipts AS receipt WHERE user_id=$1 AND promotion_receipt_id=$2",
            )
            .bind(user_id)
            .bind(promotion_id)
            .fetch_optional(&mut **transaction)
            .await?
        } else {
            None
        }
    } else {
        None
    };
    let review_approved = review
        .as_ref()
        .and_then(|review| review["decision"].as_str())
        == Some("approved");
    let risk_level = if hard_gate_failures > 0 {
        "high"
    } else if diff
        .iter()
        .any(|item| item["requires_review"] == Value::Bool(true))
    {
        "review_required"
    } else {
        "low"
    };

    Ok(json!({
        "id": dream_id,
        "scope": row.try_get::<String, _>("scope_ref")?,
        "scope_id": scope_id,
        "region": row.try_get::<String, _>("region_ref")?,
        "region_id": row.try_get::<Uuid, _>("region_id")?,
        "region_version": row.try_get::<i32, _>("region_version")?,
        "region_manifest_hash": row.try_get::<String, _>("region_manifest_hash")?,
        "root_set_hash": row.try_get::<String, _>("root_set_hash")?,
        "status": status,
        "job_type": row.try_get::<String, _>("job_type")?,
        "trigger": row.try_get::<Vec<String>, _>("trigger_reasons")?,
        "base_revision": row.try_get::<Uuid, _>("base_corpus_revision_id")?,
        "evidence_watermark": row.try_get::<String, _>("evidence_watermark")?,
        "candidate_id": candidate_id,
        "candidate_revision": candidate_corpus_revision_id,
        "candidate_manifest_hash": row.try_get::<Option<String>, _>("candidate_manifest_hash")?,
        "candidate_status": row.try_get::<Option<String>, _>("candidate_status")?,
        "rollback_to": row.try_get::<Uuid, _>("base_corpus_revision_id")?,
        "risk_level": risk_level,
        "diff": diff,
        "diff_truncated": diff_truncated,
        "disposition_summary": disposition_summary,
        "gates": gates,
        "lineage": lineage,
        "evaluation": evaluation,
        "model_receipt": model_receipt,
        "audit": events,
        "review": review,
        "promotion_receipt": promotion_receipt,
        "rollback_receipt": rollback_receipt,
        "active_manifest": {
            "id": row.try_get::<Uuid, _>("active_manifest_id")?,
            "active_corpus_revision_id": row.try_get::<Uuid, _>("active_corpus_revision_id")?,
            "manifest_hash": row.try_get::<String, _>("active_manifest_hash")?,
            "generation": row.try_get::<i64, _>("active_manifest_generation")?,
        },
        "versions": {
            "dream_policy": row.try_get::<String, _>("dream_policy_version")?,
            "model": row.try_get::<String, _>("model_version")?,
            "prompt": row.try_get::<String, _>("prompt_version")?,
            "schema": row.try_get::<String, _>("schema_version")?,
            "evaluator": row.try_get::<String, _>("evaluator_version")?,
        },
        "compute_budget": row.try_get::<Value, _>("compute_budget")?,
        "requested_by_credential_id": row.try_get::<Option<Uuid>, _>("requested_by_credential_id")?,
        "created_at": row.try_get::<DateTime<Utc>, _>("created_at")?,
        "started_at": row.try_get::<Option<DateTime<Utc>>, _>("started_at")?,
        "completed_at": row.try_get::<Option<DateTime<Utc>>, _>("completed_at")?,
        "updated_at": row.try_get::<DateTime<Utc>, _>("updated_at")?,
        "phase0": {
            "shadow_only": true,
            "automatic_promotion": false,
            "promotion_operation_exposed": false,
            "hard_gates_passed": hard_gate_failures == 0,
            "safe_derived_additions_only": unsafe_additions == 0,
            "review_approved": review_approved,
            "future_phase_promotion_eligible": hard_gate_failures == 0
                && unsafe_additions == 0 && review_approved,
            "active_manifest_mutation_allowed": false,
        },
        "unsupported": [
            "candidate semantic-vector index construction",
            "automatic or manual promotion in Phase 0",
        ],
    }))
}

async fn transition_job(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    dream_id: Uuid,
    from_status: &str,
    to_status: &str,
    complete: bool,
    details: Value,
) -> ApiResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE straylight.dream_jobs
        SET status=$5,
            started_at=coalesce(started_at,clock_timestamp()),
            completed_at=CASE WHEN $6 THEN clock_timestamp() ELSE completed_at END
        WHERE user_id=$1 AND scope_id=$2 AND id=$3 AND status=$4
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(dream_id)
    .bind(from_status)
    .bind(to_status)
    .bind(complete)
    .execute(&mut **transaction)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "dream_state_conflict",
            "the dream state changed while the worker was processing it",
            json!({"expected": from_status, "requested": to_status}),
        ));
    }
    insert_job_event(
        transaction,
        user_id,
        scope_id,
        dream_id,
        Some(from_status),
        to_status,
        details,
    )
    .await
}

async fn insert_job_event(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    dream_id: Uuid,
    from_status: Option<&str>,
    to_status: &str,
    details: Value,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_job_events (
          id,user_id,scope_id,dream_job_id,from_status,to_status,details
        ) VALUES ($1,$2,$3,$4,$5,$6,$7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(dream_id)
    .bind(from_status)
    .bind(to_status)
    .bind(details)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_audit(
    transaction: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope_id: Uuid,
    dream_id: Uuid,
    action: &str,
    details: Value,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,actor_ref,action,target_record_id,
          details,content_free
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(format!("credential:{}", auth.credential_id.0))
    .bind(action)
    .bind(dream_id)
    .bind(details)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn release_region_lock(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    dream_id: Uuid,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM straylight.dream_region_locks WHERE user_id=$1 AND dream_job_id=$2")
        .bind(user_id)
        .bind(dream_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn read_manifest_for_share(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
) -> ApiResult<ManifestSnapshot> {
    read_manifest(transaction, user_id, scope_id, "FOR SHARE").await
}

async fn read_manifest_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
) -> ApiResult<ManifestSnapshot> {
    read_manifest(transaction, user_id, scope_id, "FOR UPDATE").await
}

async fn read_manifest(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    lock_clause: &str,
) -> ApiResult<ManifestSnapshot> {
    let sql = match lock_clause {
        "FOR SHARE" => {
            "SELECT id,active_corpus_revision_id,manifest_hash,generation FROM straylight.active_manifests WHERE user_id=$1 AND scope_id=$2 FOR SHARE"
        }
        "FOR UPDATE" => {
            "SELECT id,active_corpus_revision_id,manifest_hash,generation FROM straylight.active_manifests WHERE user_id=$1 AND scope_id=$2 FOR UPDATE"
        }
        _ => {
            return Err(ApiError::Internal(
                "unsupported manifest lock clause".to_owned(),
            ));
        }
    };
    let row = sqlx::query(sql)
        .bind(user_id)
        .bind(scope_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| ApiError::not_found("active_manifest_not_found", &scope_id.to_string()))?;
    Ok(ManifestSnapshot {
        manifest_id: row.try_get("id")?,
        active_corpus_revision_id: row.try_get("active_corpus_revision_id")?,
        manifest_hash: row.try_get("manifest_hash")?,
        generation: row.try_get("generation")?,
    })
}

fn manifest_json(manifest: &ManifestSnapshot) -> Value {
    json!({
        "id": manifest.manifest_id,
        "active_corpus_revision_id": manifest.active_corpus_revision_id,
        "manifest_hash": manifest.manifest_hash,
        "generation": manifest.generation,
    })
}

fn normalize_create(request: DreamCreateRequest) -> ApiResult<NormalizedCreate> {
    let scope_ref = request.scope.trim().to_owned();
    if scope_ref.is_empty() || scope_ref.len() > 240 {
        return Err(ApiError::invalid(
            "dream scope must contain between 1 and 240 characters",
        ));
    }
    let idempotency_key = request.idempotency_key.trim().to_owned();
    if idempotency_key.is_empty() || idempotency_key.len() > 240 {
        return Err(ApiError::invalid(
            "dream idempotency_key must contain between 1 and 240 characters",
        ));
    }
    if request.root_refs.len() > 256 {
        return Err(ApiError::invalid(
            "a dream may contain at most 256 root refs",
        ));
    }
    let mut root_pairs = request
        .root_refs
        .iter()
        .map(|reference| {
            let reference = reference.trim();
            if reference.is_empty() {
                return Err(ApiError::invalid("dream root refs must not be empty"));
            }
            let id = Uuid::parse_str(reference)
                .ok()
                .or_else(|| RecordRef::parse(reference).ok().map(|record| record.id))
                .ok_or_else(|| ApiError::invalid(format!("invalid dream root ref: {reference}")))?;
            Ok((reference.to_owned(), id))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    root_pairs.sort_by_key(|pair| pair.1);
    root_pairs.dedup_by_key(|pair| pair.1);
    let (root_refs, root_ids): (Vec<_>, Vec<_>) = root_pairs.into_iter().unzip();

    let mut trigger_reasons: Vec<_> = request
        .trigger
        .iter()
        .map(|reason| reason.trim())
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned)
        .collect();
    trigger_reasons.sort();
    trigger_reasons.dedup();
    if trigger_reasons.is_empty() {
        trigger_reasons.push("manual".to_owned());
    }
    if trigger_reasons.len() > 32 {
        return Err(ApiError::invalid(
            "a dream may contain at most 32 trigger reasons",
        ));
    }
    let job_type = if request.repair_only {
        "repair_only".to_owned()
    } else {
        request.job_type.trim().to_owned()
    };
    if !matches!(
        job_type.as_str(),
        "deterministic_maintenance" | "deep_consolidation" | "repair_only"
    ) {
        return Err(ApiError::invalid(
            "dream job_type must be deterministic_maintenance, deep_consolidation, or repair_only",
        ));
    }
    let mut budget = match request.compute_budget.unwrap_or_else(|| json!({})) {
        Value::Object(object) => object,
        _ => return Err(ApiError::invalid("dream compute_budget must be an object")),
    };
    let max_records = budget
        .get("max_records")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_RECORDS as u64) as usize;
    if max_records == 0 || max_records > MAX_MAX_RECORDS {
        return Err(ApiError::invalid(format!(
            "dream compute_budget.max_records must be between 1 and {MAX_MAX_RECORDS}"
        )));
    }
    let max_model_input_bytes = budget
        .get("max_model_input_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_MODEL_INPUT_BYTES as u64) as usize;
    if max_model_input_bytes == 0 || max_model_input_bytes > MAX_MAX_MODEL_INPUT_BYTES {
        return Err(ApiError::invalid(format!(
            "dream compute_budget.max_model_input_bytes must be between 1 and {MAX_MAX_MODEL_INPUT_BYTES}"
        )));
    }
    let max_model_output_tokens = budget
        .get("max_model_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_MODEL_OUTPUT_TOKENS);
    if max_model_output_tokens == 0 || max_model_output_tokens > MAX_MAX_MODEL_OUTPUT_TOKENS {
        return Err(ApiError::invalid(format!(
            "dream compute_budget.max_model_output_tokens must be between 1 and {MAX_MAX_MODEL_OUTPUT_TOKENS}"
        )));
    }
    let max_candidate_items = budget
        .get("max_candidate_items")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_CANDIDATE_ITEMS as u64) as usize;
    if max_candidate_items == 0 || max_candidate_items > MAX_MAX_CANDIDATE_ITEMS {
        return Err(ApiError::invalid(format!(
            "dream compute_budget.max_candidate_items must be between 1 and {MAX_MAX_CANDIDATE_ITEMS}"
        )));
    }
    let max_evaluation_queries = budget
        .get("max_evaluation_queries")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_EVALUATION_QUERIES as u64) as usize;
    if max_evaluation_queries == 0 || max_evaluation_queries > MAX_MAX_EVALUATION_QUERIES {
        return Err(ApiError::invalid(format!(
            "dream compute_budget.max_evaluation_queries must be between 1 and {MAX_MAX_EVALUATION_QUERIES}"
        )));
    }
    let lock_ttl_seconds = budget
        .get("lock_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_LOCK_TTL_SECONDS);
    if !(60..=3_600).contains(&lock_ttl_seconds) {
        return Err(ApiError::invalid(
            "dream compute_budget.lock_ttl_seconds must be between 60 and 3600",
        ));
    }
    let mut expected_query_families = request.expected_query_families.unwrap_or_default();
    expected_query_families = expected_query_families
        .into_iter()
        .map(|family| family.trim().to_owned())
        .filter(|family| !family.is_empty())
        .collect();
    expected_query_families.sort();
    expected_query_families.dedup();
    if expected_query_families.len() > 128 {
        return Err(ApiError::invalid(
            "a dream may contain at most 128 expected query families",
        ));
    }
    budget.insert("phase".to_owned(), json!(PHASE));
    budget.insert("max_records".to_owned(), json!(max_records));
    budget.insert(
        "max_model_input_bytes".to_owned(),
        json!(max_model_input_bytes),
    );
    budget.insert(
        "max_model_output_tokens".to_owned(),
        json!(max_model_output_tokens),
    );
    budget.insert("max_candidate_items".to_owned(), json!(max_candidate_items));
    budget.insert(
        "max_evaluation_queries".to_owned(),
        json!(max_evaluation_queries),
    );
    budget.insert("lock_ttl_seconds".to_owned(), json!(lock_ttl_seconds));
    budget.insert(
        "expected_query_families".to_owned(),
        json!(expected_query_families),
    );
    budget.insert("automatic_promotion".to_owned(), Value::Bool(false));
    let compute_budget = Value::Object(budget);
    let request_hash = hash_json(&json!({
        "scope": scope_ref,
        "root_refs": root_refs,
        "trigger_reasons": trigger_reasons,
        "job_type": job_type,
        "compute_budget": compute_budget,
        "idempotency_key": idempotency_key,
    }));
    Ok(NormalizedCreate {
        scope_ref,
        root_refs,
        root_ids,
        trigger_reasons,
        job_type,
        compute_budget,
        max_records,
        max_model_input_bytes,
        max_model_output_tokens,
        max_candidate_items,
        max_evaluation_queries,
        idempotency_key,
        request_hash,
    })
}

fn parse_uuid_ref(value: &str, prefixes: &[&str]) -> ApiResult<Uuid> {
    let value = value.trim();
    if let Ok(id) = Uuid::parse_str(value) {
        return Ok(id);
    }
    for prefix in prefixes {
        if let Some(raw) = value.strip_prefix(&format!("{prefix}:"))
            && let Ok(id) = Uuid::parse_str(raw)
        {
            return Ok(id);
        }
    }
    Err(ApiError::invalid(format!("invalid reference: {value}")))
}

fn deterministic_uuid(material: &str) -> Uuid {
    let digest = Sha256::digest(material.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn hash_json(value: &Value) -> String {
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical).expect("JSON value serialization");
    hex::encode(Sha256::digest(bytes))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            serde_json::to_value(sorted).expect("BTreeMap JSON serialization")
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "promoted" | "quarantined" | "discarded" | "failed" | "canceled" | "stale" | "rolled_back"
    )
}

fn sanitized_failure(error: &ApiError) -> Value {
    match error {
        ApiError::Public {
            code,
            message,
            details,
            ..
        } => json!({"class": "public", "code": code, "message": message, "details": details}),
        ApiError::Database(_) => json!({"class": "database", "code": "database_error"}),
        ApiError::Migration(_) => json!({"class": "migration", "code": "migration_error"}),
        ApiError::Json(_) => json!({"class": "serialization", "code": "json_error"}),
        ApiError::Internal(_) => json!({"class": "internal", "code": "internal_error"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: Uuid, kind: &str) -> RegionMember {
        RegionMember {
            record_id: id,
            record_kind: kind.to_owned(),
            record_version: Some(1),
        }
    }

    fn completed_model(candidates: Vec<ModelCandidateItem>) -> ModelRun {
        ModelRun {
            status: "completed".to_owned(),
            requested_model: "test-model".to_owned(),
            returned_model: Some("test-model".to_owned()),
            response_id: Some("resp_test".to_owned()),
            request_hash: "0".repeat(64),
            output: json!({}),
            usage: json!({}),
            degradation_reason: None,
            candidates,
            expected_queries: vec![],
            validation_errors: vec![],
        }
    }

    fn passing_evaluation() -> RetrievalEvaluation {
        RetrievalEvaluation {
            frozen_suite_hash: "1".repeat(64),
            result: json!({}),
            passed: true,
            regressions: 0,
            candidate_score: 1.0,
            active_score: 1.0,
        }
    }

    #[test]
    fn canonical_hash_ignores_object_key_order() {
        assert_eq!(
            hash_json(&json!({"b": 2, "a": {"z": 1, "x": 0}})),
            hash_json(&json!({"a": {"x": 0, "z": 1}, "b": 2}))
        );
    }

    #[test]
    fn deterministic_job_ids_are_stable_and_namespaced() {
        let first = deterministic_uuid("dream-job:user:scope:key");
        let replay = deterministic_uuid("dream-job:user:scope:key");
        let other = deterministic_uuid("dream-job:user:scope:other");
        assert_eq!(first, replay);
        assert_ne!(first, other);
        assert_eq!(first.get_version_num(), 5);
    }

    #[test]
    fn openai_safety_identifier_is_stable_hashed_and_user_scoped() {
        let user = Uuid::now_v7();
        let other = Uuid::now_v7();
        let first = safety_identifier("test-secret", user);
        assert_eq!(first, safety_identifier("test-secret", user));
        assert_ne!(first, safety_identifier("test-secret", other));
        assert_ne!(first, safety_identifier("other-secret", user));
        assert_eq!(first.len(), 64);
        assert!(!first.contains(&user.to_string()));
    }

    #[test]
    fn disposition_map_is_complete_and_omissions_keep() {
        let kept_id = Uuid::now_v7();
        let duplicate_id = Uuid::now_v7();
        let report = MaintenanceReport {
            exact_duplicates: vec![Finding {
                finding_ref: "duplicate:test".to_owned(),
                target_record_id: duplicate_id,
                category: "exact_duplicate".to_owned(),
                hard_integrity_issue: false,
                source_refs: vec![duplicate_id],
                details: json!({}),
            }],
            ..MaintenanceReport::default()
        };
        let dispositions = build_disposition_map(
            &[member(kept_id, "object"), member(duplicate_id, "asset")],
            &report,
        );
        assert_eq!(dispositions.len(), 2);
        assert_eq!(
            dispositions
                .iter()
                .find(|item| item.target_record_id == kept_id)
                .unwrap()
                .disposition,
            "keep"
        );
        assert_eq!(
            dispositions
                .iter()
                .find(|item| item.target_record_id == duplicate_id)
                .unwrap()
                .disposition,
            "add_cluster"
        );
    }

    #[test]
    fn hard_findings_remain_flags_and_require_review() {
        let target = Uuid::now_v7();
        let report = MaintenanceReport {
            lineage_issues: vec![Finding {
                finding_ref: "lineage:test".to_owned(),
                target_record_id: target,
                category: "lineage_integrity".to_owned(),
                hard_integrity_issue: true,
                source_refs: vec![target],
                details: json!({"issue": "cycle"}),
            }],
            ..MaintenanceReport::default()
        };
        let dispositions = build_disposition_map(&[member(target, "claim")], &report);
        assert_eq!(dispositions[0].disposition, "add_flag");
        assert!(dispositions[0].requires_review);
        assert!(is_safe_disposition(&dispositions[0]));
        let gates = build_gates(
            &report,
            1,
            1,
            true,
            "deterministic_maintenance",
            &completed_model(vec![]),
            &passing_evaluation(),
            &"2".repeat(64),
            &"2".repeat(64),
            &"3".repeat(64),
        );
        assert!(
            gates
                .iter()
                .any(|gate| gate.gate_kind == "lineage" && gate.hard_gate && !gate.passed)
        );
    }

    #[test]
    fn phase_zero_has_no_promotion_status_or_cancelable_promoted_state() {
        assert!(is_terminal_status("promoted"));
        assert!(is_terminal_status("quarantined"));
        assert!(!is_terminal_status("awaiting_review"));
        assert!(!is_terminal_status("evaluating"));
    }

    #[test]
    fn references_accept_plain_and_prefixed_uuids() {
        let id = Uuid::now_v7();
        assert_eq!(parse_uuid_ref(&id.to_string(), &["dream"]).unwrap(), id);
        assert_eq!(
            parse_uuid_ref(&format!("dream:{id}"), &["dream"]).unwrap(),
            id
        );
        assert!(parse_uuid_ref("dream:not-a-uuid", &["dream"]).is_err());
    }

    #[test]
    fn model_candidates_must_stay_in_region_and_cite_direct_sources() {
        let target = Uuid::now_v7();
        let source = Uuid::now_v7();
        let outside = Uuid::now_v7();
        let output = ModelCandidateOutput {
            candidate_items: vec![ModelCandidateItem {
                target_record_id: outside,
                disposition: "add_derived_view".to_owned(),
                title: "Compiled project view".to_owned(),
                content: "A source-grounded view".to_owned(),
                source_refs: vec![outside],
                claims: vec![],
                query_aids: vec!["current project status".to_owned()],
                rationale: "Improves a recurring query".to_owned(),
                authority_effect: "none".to_owned(),
            }],
            expected_queries: vec![],
            unresolved: vec![],
        };
        let errors = validate_model_output(
            &output,
            &BTreeSet::from([target]),
            &BTreeSet::from([source]),
            8,
            8,
        );
        assert!(
            errors
                .iter()
                .any(|error| { error["code"] == "target_outside_pinned_region" })
        );
        assert!(
            errors
                .iter()
                .any(|error| { error["code"] == "candidate_source_lineage_invalid" })
        );
    }

    #[test]
    fn deep_dream_schema_stays_within_strict_structured_output_subset() {
        fn inspect(value: &Value) {
            if let Some(object) = value.as_object() {
                for unsupported in [
                    "format",
                    "uniqueItems",
                    "minItems",
                    "maxItems",
                    "minLength",
                    "maxLength",
                ] {
                    assert!(
                        !object.contains_key(unsupported),
                        "unsupported {unsupported}"
                    );
                }
                if object.get("type").and_then(Value::as_str) == Some("object") {
                    assert_eq!(
                        object.get("additionalProperties"),
                        Some(&Value::Bool(false))
                    );
                    let properties = object
                        .get("properties")
                        .and_then(Value::as_object)
                        .expect("strict object has properties");
                    let required = object
                        .get("required")
                        .and_then(Value::as_array)
                        .expect("strict object has required fields");
                    assert_eq!(required.len(), properties.len());
                }
                for child in object.values() {
                    inspect(child);
                }
            } else if let Some(array) = value.as_array() {
                for child in array {
                    inspect(child);
                }
            }
        }

        inspect(&dream_output_schema());
    }

    #[test]
    fn deterministic_model_validation_enforces_removed_wire_schema_bounds() {
        let target = Uuid::now_v7();
        let source = Uuid::now_v7();
        let output = ModelCandidateOutput {
            candidate_items: vec![ModelCandidateItem {
                target_record_id: target,
                disposition: "add_derived_view".to_owned(),
                title: "x".repeat(241),
                content: "content".to_owned(),
                source_refs: vec![source, source],
                claims: vec![],
                query_aids: vec![],
                rationale: "bounded rationale".to_owned(),
                authority_effect: "none".to_owned(),
            }],
            expected_queries: vec![],
            unresolved: vec![],
        };
        let errors = validate_model_output(
            &output,
            &BTreeSet::from([target]),
            &BTreeSet::from([source]),
            8,
            8,
        );
        assert!(
            errors
                .iter()
                .any(|error| error["code"] == "candidate_content_invalid")
        );
        assert!(
            errors
                .iter()
                .any(|error| error["code"] == "candidate_source_lineage_duplicated")
        );
    }

    #[test]
    fn model_candidates_are_merged_as_inspectable_shadow_views() {
        let target = Uuid::now_v7();
        let source = Uuid::now_v7();
        let mut dispositions =
            build_disposition_map(&[member(target, "object")], &MaintenanceReport::default());
        merge_model_candidates(
            &mut dispositions,
            &[ModelCandidateItem {
                target_record_id: target,
                disposition: "add_derived_view".to_owned(),
                title: "Current project state".to_owned(),
                content: "The source-backed current state.".to_owned(),
                source_refs: vec![source],
                claims: vec![ModelCandidateClaim {
                    text: "Current state is source backed.".to_owned(),
                    source_refs: vec![source],
                }],
                query_aids: vec!["current state".to_owned()],
                rationale: "Expected query family".to_owned(),
                authority_effect: "none".to_owned(),
            }],
        );
        assert_eq!(dispositions[0].disposition, "add_derived_view");
        let mut expected_sources = vec![source, target];
        expected_sources.sort_unstable();
        assert_eq!(dispositions[0].source_refs, expected_sources);
        assert_eq!(
            dispositions[0].proposal["model_candidates"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn paired_retrieval_evaluation_detects_candidate_regression() {
        let expected = Uuid::now_v7();
        let source = Uuid::now_v7();
        let active = vec![RetrievalDocument {
            record_id: expected,
            record_kind: "object".to_owned(),
            text: "alpha only".to_owned(),
            source_refs: vec![source],
            origin: "active".to_owned(),
        }];
        let candidate = (0..6)
            .map(|_| RetrievalDocument {
                record_id: Uuid::now_v7(),
                record_kind: "shadow_derived_view".to_owned(),
                text: "alpha beta".to_owned(),
                source_refs: vec![Uuid::now_v7()],
                origin: "candidate".to_owned(),
            })
            .collect::<Vec<_>>();
        let queries = vec![frozen_query(
            "rare_detail",
            "alpha beta",
            vec![expected],
            vec![],
            "test",
        )];
        let evaluation = evaluate_retrieval(Uuid::now_v7(), &active, &candidate, &queries);
        assert!(!evaluation.passed);
        assert_eq!(evaluation.regressions, 1);
        assert!(evaluation.candidate_score < evaluation.active_score);
    }

    #[test]
    fn degraded_deep_model_is_a_hard_gate_not_a_silent_success() {
        let report = MaintenanceReport::default();
        let model = degraded_model_run(
            "test-model".to_owned(),
            "degraded_unavailable",
            "0".repeat(64),
            "provider unavailable".to_owned(),
        );
        let gates = build_gates(
            &report,
            1,
            1,
            true,
            "deep_consolidation",
            &model,
            &passing_evaluation(),
            &"2".repeat(64),
            &"2".repeat(64),
            &"3".repeat(64),
        );
        assert!(gates.iter().any(|gate| {
            gate.gate_ref == "gate:model-consolidation@v1" && gate.hard_gate && !gate.passed
        }));
    }

    #[test]
    fn scheduler_honors_pause_inactivity_cooldown_and_active_jobs() {
        let now = Utc::now();
        let inactive = now - chrono::Duration::minutes(45);
        assert!(scheduler_eligible(
            true,
            false,
            10,
            10,
            None,
            Some(inactive),
            now,
            1_800,
            false,
        ));
        assert!(!scheduler_eligible(
            true,
            true,
            10,
            10,
            None,
            Some(inactive),
            now,
            1_800,
            false,
        ));
        assert!(!scheduler_eligible(
            true,
            false,
            10,
            10,
            Some(now + chrono::Duration::minutes(5)),
            Some(inactive),
            now,
            1_800,
            false,
        ));
        assert!(!scheduler_eligible(
            true,
            false,
            10,
            10,
            None,
            Some(now),
            now,
            1_800,
            false,
        ));
        assert!(!scheduler_eligible(
            true,
            false,
            10,
            10,
            None,
            Some(inactive),
            now,
            1_800,
            true,
        ));
    }

    #[test]
    fn scheduler_idempotency_is_stable_within_and_distinct_across_cycles() {
        let scope_id = Uuid::now_v7();
        let revision_id = Uuid::now_v7();
        let completed_at = Utc::now();
        let initial = scheduler_idempotency_key(scope_id, revision_id, false, None);
        assert_eq!(
            initial,
            scheduler_idempotency_key(scope_id, revision_id, false, None)
        );
        assert_ne!(
            initial,
            scheduler_idempotency_key(scope_id, revision_id, false, Some(completed_at))
        );
        assert_ne!(
            scheduler_idempotency_key(scope_id, revision_id, false, Some(completed_at)),
            scheduler_idempotency_key(scope_id, revision_id, true, Some(completed_at))
        );
    }
}

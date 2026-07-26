//! Snapshot-pinned read and retrieval semantics for Straylight.
//!
//! This module deliberately contains no HTTP handlers and performs no durable
//! mutations. A caller provisions a session, then passes its ID here. Every
//! database access uses an RLS-context transaction and also constrains user,
//! scope, and corpus revision explicitly.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use pgvector::Vector;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    continuation::ContinuationCodec,
    db::AppState,
    error::{ApiError, ApiResult},
    ingest::estimate_tokens,
    models::{
        Capability, ComputeOperator, ComputeRequest, ComputeStep, Coverage, CoveragePartition,
        Freshness, OpenRequest, QueryRequest, QueryScope, QuerySpec, ReadRequest, ReadSpec,
        ReadView, ResponseStatus, RetrievalMode, TemporalSpecV1, Truncation,
        VerificationClassification, VerifyClaim, VerifyRequest,
    },
    quota::{self, UsageReservation},
    recurrence,
    retrieval::{RetrievalCandidate, reciprocal_rank_fusion, source_coherent_rank_fusion},
    telemetry,
};

const DEFAULT_TOKEN_BUDGET: usize = 24_000;
const DEFAULT_READ_CHARS: usize = 20_000;
const MAX_COMPUTE_ROWS: usize = 10_000;
const MAX_GRAPH_DEPTH: i32 = 5;
const MAX_MATERIALIZED_GENERIC_RECORDS: usize = 1_000;
const CORPUS_MAP_SAMPLE_PER_KIND: usize = 12;
const MAX_REVISION_SOURCE_CHANGES: usize = 8;
const MAX_REVISION_SOURCE_CONTENT_CHARS: usize = 12_000;
const OPEN_COMPLETE_SOURCE_LIMIT: usize = 4;
const OPEN_COMPLETE_SOURCE_TOTAL_CHARS: usize = 32_000;

#[derive(Clone, Copy)]
enum QuerySelection {
    Diversified,
    SourceCoherent,
}

#[derive(Clone, Debug, Serialize)]
pub struct PinnedSession {
    pub session_id: Uuid,
    pub session_ref: String,
    pub user_id: Uuid,
    pub scope_id: Uuid,
    pub scope_ref: String,
    pub credential_id: Uuid,
    pub corpus_revision_id: Uuid,
    pub corpus_revision_ref: String,
    pub revision_number: i64,
    pub manifest_hash: String,
    pub task_hash: String,
    pub task_size_bytes: usize,
    pub capability_snapshot: Vec<String>,
    pub authorization_scope_refs: Vec<String>,
    pub policy_projection_hash: String,
    pub mode: String,
    pub parent_session_id: Option<Uuid>,
    pub resumed_checkpoint_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub freshness: Freshness,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpenResult {
    pub session_id: String,
    pub corpus_revision: String,
    pub resolved_scope: Value,
    pub corpus_map: Value,
    pub resume_checkpoint: Option<Value>,
    pub revision_delta: Option<Value>,
    pub learned_context: Value,
    pub initial_evidence: Vec<RetrievalCandidate>,
    pub hydrated_sources: Vec<HydratedSource>,
    pub retrieval_sufficiency: Value,
    pub initial_case_file: Option<Value>,
    pub freshness: Freshness,
    pub coverage: Coverage,
    pub conflicts: Vec<Value>,
    pub gaps: Vec<Value>,
    pub ambiguities: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HydratedSource {
    pub source_ref: String,
    pub path: Option<String>,
    pub title: Option<String>,
    pub source_version: Option<String>,
    pub content_hash: Option<String>,
    pub authority: Option<String>,
    pub canonicality: Option<String>,
    pub provenance: Option<Value>,
    pub recorded_at: Option<String>,
    pub text: String,
    pub ranges: Vec<Value>,
    pub complete: bool,
    pub selected_references: Vec<String>,
    pub why_selected: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueryBatchResult {
    pub session_id: String,
    pub corpus_revision: String,
    pub status: ResponseStatus,
    pub items: Vec<QueryItemResult>,
    pub freshness: Freshness,
    pub coverage: Coverage,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueryItemResult {
    pub id: String,
    pub status: ResponseStatus,
    pub results: Vec<RetrievalCandidate>,
    pub coverage: Coverage,
    pub conflicts: Vec<Value>,
    pub gaps: Vec<Value>,
    pub ambiguities: Vec<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReadBatchResult {
    pub session_id: String,
    pub corpus_revision: String,
    pub status: ResponseStatus,
    pub items: Vec<ReadItemResult>,
    pub freshness: Freshness,
    pub coverage: Coverage,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReadItemResult {
    pub reference: String,
    pub view: String,
    pub status: ItemStatus,
    pub data: Option<Value>,
    pub error: Option<ItemError>,
    pub truncation: Truncation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Complete,
    Partial,
    Unsupported,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct ItemError {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComputeResult {
    pub session_id: String,
    pub corpus_revision: String,
    pub status: ResponseStatus,
    pub steps: Vec<ComputeStepResult>,
    pub rows_returned: usize,
    pub estimated_tokens: usize,
    pub freshness: Freshness,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComputeStepResult {
    pub id: String,
    pub operator: String,
    pub status: ItemStatus,
    pub output: Value,
    pub evidence_refs: Vec<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifyResult {
    pub session_id: String,
    pub corpus_revision: String,
    pub status: ResponseStatus,
    pub claims: Vec<VerifyItemResult>,
    pub freshness: Freshness,
    pub coverage: Coverage,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifyItemResult {
    pub id: String,
    pub claim: String,
    pub classification: VerificationClassification,
    pub evidence: Vec<EvidencePassage>,
    pub structural_checks: Vec<StructuralCheck>,
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidencePassage {
    pub evidence_ref: String,
    pub source_ref: String,
    pub source_version: Option<String>,
    pub content_hash: String,
    pub locator: Value,
    pub text: Option<String>,
    pub support_kind: String,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StructuralCheck {
    pub check: String,
    pub status: String,
    pub details: Value,
}

#[derive(Clone, Debug)]
struct ResolvedRecord {
    id: Uuid,
    kind: String,
    version: Option<i32>,
}

#[derive(Clone, Debug)]
struct LoadedText {
    metadata: Value,
    text: String,
}

#[derive(Clone, Debug)]
struct StepExecution {
    status: ItemStatus,
    output: Value,
    evidence_refs: Vec<String>,
    message: Option<String>,
}

#[derive(Clone, Debug)]
struct StagedCorpus {
    id: Uuid,
    stage_ref: String,
    scope_id: Uuid,
    scope_ref: String,
    credential_id: Uuid,
    base_corpus_revision_id: Uuid,
    input_hash: String,
    inventory_hash: String,
    status: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl StagedCorpus {
    fn revision_ref(&self) -> String {
        format!("stage-revision:sha256:{}", self.inventory_hash)
    }

    fn freshness(&self) -> Freshness {
        Freshness {
            source_updated_at: Some(self.created_at),
            normalized_at: Some(self.created_at),
            lexical_index_updated_at: Some(self.created_at),
            semantic_index_updated_at: None,
            replica_applied_at: None,
        }
    }
}

#[derive(Clone, Debug)]
struct StagedEntry {
    id: Uuid,
    path: String,
    entry_kind: String,
    media_type: Option<String>,
    size_bytes: i64,
    content_hash: Option<String>,
    readability: String,
    inspection: Value,
    asset_id: Option<Uuid>,
    asset_version: Option<i32>,
    object_key: Option<String>,
    object_version_id: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VerifyEvidenceReference {
    Evidence(Uuid),
    Chunk(Uuid),
}

/// Load a session and prove that it remains usable by the same credential for
/// the requested operation. The raw task prompt is never loaded or stored.
pub async fn load_session(
    state: &AppState,
    auth: &AuthContext,
    session_id: &str,
    capability: Capability,
) -> ApiResult<PinnedSession> {
    auth.require(capability)?;
    let session_id = parse_uuid_ref(session_id, &["session", "ms"])?;
    let mut tx = state.begin_read(auth).await?;
    let session = load_session_tx(&mut tx, auth, session_id, capability).await?;
    tx.commit().await?;
    Ok(session)
}

/// Render the initial map, optional bounded case file, resume checkpoint, and
/// revision delta for a session already provisioned by the write/service layer.
pub async fn open(
    state: &AppState,
    auth: &AuthContext,
    session_id: &str,
    request: &OpenRequest,
) -> ApiResult<OpenResult> {
    request.validate()?;
    let session = load_session(state, auth, session_id, Capability::Open).await?;
    validate_open_request(&session, request)?;

    let mut tx = state.begin_read(auth).await?;
    let corpus_map = corpus_map_tx(&mut tx, &session).await?;
    let (resolved_scope, ambiguities) = resolved_scope_tx(&mut tx, &session, request).await?;

    let requested_checkpoint = request
        .resume_checkpoint_ref
        .as_deref()
        .map(|value| parse_uuid_ref(value, &["checkpoint"]))
        .transpose()?;
    if let (Some(requested), Some(bound)) = (requested_checkpoint, session.resumed_checkpoint_id)
        && requested != bound
    {
        return Err(ApiError::conflict(
            "revision_mismatch",
            "the requested checkpoint is not the checkpoint bound to this session",
            json!({
                "requested_checkpoint": record_ref("checkpoint", requested),
                "session_checkpoint": record_ref("checkpoint", bound)
            }),
        ));
    }
    let checkpoint_id = requested_checkpoint.or(session.resumed_checkpoint_id);
    let resume_checkpoint = if let Some(checkpoint_id) = checkpoint_id {
        Some(checkpoint_tx(&mut tx, &session, checkpoint_id).await?)
    } else {
        None
    };
    let revision_delta = if let Some(checkpoint) = &resume_checkpoint {
        let prior = checkpoint
            .get("corpus_revision_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        if let Some(prior) = prior {
            Some(revision_delta_tx(&mut tx, &session, prior).await?)
        } else {
            None
        }
    } else {
        None
    };
    let self_contained_continuation =
        continuation_is_self_contained(resume_checkpoint.as_ref(), revision_delta.as_ref());

    let token_budget = request.token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET).max(1);
    let scope_tokens = if self_contained_continuation {
        None
    } else {
        Some(scope_token_count_tx(&mut tx, &session, token_budget).await?)
    };
    let initial_case_file = if scope_tokens.is_some_and(|count| count <= token_budget) {
        Some(materialize_scope_tx(&mut tx, &session, token_budget).await?)
    } else {
        None
    };
    let learned_context = learned_context_tx(&mut tx, &session, token_budget).await?;
    tx.commit().await?;

    let (initial_evidence, mut coverage, conflicts, gaps) = if self_contained_continuation {
        let mut coverage = Coverage::default();
        coverage.unsearched.push(CoveragePartition {
            partition: "initial_task_retrieval".to_owned(),
            lane: "continuation".to_owned(),
            completeness: "not_needed".to_owned(),
            index_revision: Some(session.corpus_revision_ref.clone()),
            searched_count: 0,
            candidate_count: 0,
            failure_reason: None,
        });
        (Vec::new(), coverage, Vec::new(), Vec::new())
    } else {
        let initial_request = QueryRequest {
            session_id: session.session_ref.clone(),
            queries: Vec::new(),
            goal: Some(
                "Build an inspectable initial case file without answering the task".to_owned(),
            ),
            query: Some(request.task.clone()),
            scope: Some(QueryScope::Structured {
                authorization_scope: Some(session.scope_ref.clone()),
                root_refs: request.hints.root_refs.clone(),
            }),
            modes: vec![
                RetrievalMode::Exact,
                RetrievalMode::Structured,
                RetrievalMode::Lexical,
                RetrievalMode::Semantic,
                RetrievalMode::Temporal,
                RetrievalMode::Relations,
            ],
            r#where: Map::new(),
            state_filter: None,
            expand: None,
            limit: Some(12),
        };
        let specs = initial_request.normalized_queries()?;
        let initial =
            run_query_batch(state, auth, &session, specs, QuerySelection::SourceCoherent).await?;
        let initial_item = initial.items.into_iter().next();
        let initial_evidence = initial_item
            .as_ref()
            .map(|item| item.results.clone())
            .unwrap_or_default();
        let conflicts = initial_item
            .as_ref()
            .map(|item| item.conflicts.clone())
            .unwrap_or_default();
        let gaps = initial_item
            .as_ref()
            .map(|item| item.gaps.clone())
            .unwrap_or_default();
        (initial_evidence, initial.coverage, conflicts, gaps)
    };
    let materialization_complete = initial_case_file
        .as_ref()
        .and_then(|value| value.get("complete"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let materialization_partition = CoveragePartition {
        partition: "bounded_scope_materialization".to_owned(),
        lane: "materialize_scope".to_owned(),
        completeness: if self_contained_continuation {
            "not_needed".to_owned()
        } else if materialization_complete {
            "complete".to_owned()
        } else if initial_case_file.is_some() {
            "partial".to_owned()
        } else {
            "not_materialized".to_owned()
        },
        index_revision: Some(session.corpus_revision_ref.clone()),
        searched_count: usize::from(initial_case_file.is_some()),
        candidate_count: usize::from(initial_case_file.is_some()),
        failure_reason: if self_contained_continuation {
            None
        } else if initial_case_file.is_none() {
            Some(format!(
                "scope estimate {} tokens exceeds budget {token_budget}",
                scope_tokens.unwrap_or_default()
            ))
        } else if !materialization_complete {
            Some("one or more active corpus rows were omitted by materialization limits".to_owned())
        } else {
            None
        },
    };
    if self_contained_continuation {
        coverage.unsearched.push(materialization_partition);
    } else {
        coverage.searched.push(materialization_partition);
    }
    let learned_status = learned_context
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("not_available");
    let learned_items = learned_context
        .get("items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    coverage.searched.push(CoveragePartition {
        partition: "automatic_learned_context".to_owned(),
        lane: "hard_gated_derived_view".to_owned(),
        completeness: learned_status.to_owned(),
        index_revision: Some(session.corpus_revision_ref.clone()),
        searched_count: 1,
        candidate_count: learned_items,
        failure_reason: (learned_status != "fresh")
            .then(|| "no fresh hard-gated derived view matches the pinned revision".to_owned()),
    });
    let hydrated_sources = if self_contained_continuation {
        Vec::new()
    } else {
        hydrate_open_sources(state, auth, &session, &initial_evidence).await?
    };
    let retrieval_sufficiency =
        open_retrieval_sufficiency(&request.task, &initial_evidence, &hydrated_sources);
    Ok(OpenResult {
        session_id: session.session_ref.clone(),
        corpus_revision: session.corpus_revision_ref.clone(),
        resolved_scope,
        corpus_map,
        resume_checkpoint,
        revision_delta,
        learned_context,
        initial_evidence,
        hydrated_sources,
        retrieval_sufficiency,
        initial_case_file,
        freshness: session.freshness,
        coverage,
        conflicts,
        gaps,
        ambiguities,
    })
}

async fn hydrate_open_sources(
    state: &AppState,
    auth: &AuthContext,
    session: &PinnedSession,
    candidates: &[RetrievalCandidate],
) -> ApiResult<Vec<HydratedSource>> {
    let mut groups = Vec::<(String, Vec<&RetrievalCandidate>)>::new();
    for candidate in candidates {
        let Some(path) = candidate.path.as_deref() else {
            continue;
        };
        if let Some((_, grouped)) = groups.iter_mut().find(|(key, _)| key == path) {
            grouped.push(candidate);
        } else {
            groups.push((path.to_owned(), vec![candidate]));
        }
    }

    let mut complete_chars = 0usize;
    let mut sources = Vec::with_capacity(groups.len());
    for (index, (path, grouped)) in groups.into_iter().enumerate() {
        let remaining = OPEN_COMPLETE_SOURCE_TOTAL_CHARS.saturating_sub(complete_chars);
        let complete = if index < OPEN_COMPLETE_SOURCE_LIMIT
            && remaining > 0
            && let Some(loaded) = try_load_complete_open_source(state, auth, session, &path).await
        {
            let chars = loaded.text.chars().count();
            (chars <= remaining).then_some(loaded)
        } else {
            None
        };

        if let Some(loaded) = complete {
            complete_chars = complete_chars.saturating_add(loaded.text.chars().count());
            sources.push(complete_hydrated_source(&path, &grouped, loaded));
        } else {
            sources.push(section_hydrated_source(&path, &grouped));
        }
    }
    Ok(sources)
}

async fn try_load_complete_open_source(
    state: &AppState,
    auth: &AuthContext,
    session: &PinnedSession,
    path: &str,
) -> Option<LoadedText> {
    let mut tx = state.begin_read(auth).await.ok()?;
    let loaded = match resolve_record_tx(&mut tx, session, path).await {
        Ok(record) => load_text_tx(&mut tx, state, session, &record, None).await,
        Err(error) => Err(error),
    };
    match loaded {
        Ok(loaded) => {
            if tx.commit().await.is_ok() {
                Some(loaded)
            } else {
                None
            }
        }
        Err(_) => {
            let _ = tx.rollback().await;
            None
        }
    }
}

fn complete_hydrated_source(
    path: &str,
    candidates: &[&RetrievalCandidate],
    loaded: LoadedText,
) -> HydratedSource {
    let line_count = loaded.text.lines().count().max(1);
    let why_selected = hydration_reasons(candidates, "complete_source_hydration");
    HydratedSource {
        source_ref: loaded
            .metadata
            .get("source_ref")
            .and_then(Value::as_str)
            .unwrap_or(&candidates[0].source_ref)
            .to_owned(),
        path: Some(path.to_owned()),
        title: loaded
            .metadata
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                candidates
                    .iter()
                    .find_map(|candidate| candidate.heading.clone())
            }),
        source_version: loaded
            .metadata
            .get("source_version")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                candidates
                    .iter()
                    .find_map(|candidate| candidate.source_version.clone())
            }),
        content_hash: loaded
            .metadata
            .get("content_hash")
            .and_then(Value::as_str)
            .map(str::to_owned),
        authority: candidates
            .iter()
            .find_map(|candidate| candidate.authority.clone()),
        canonicality: candidates
            .iter()
            .find_map(|candidate| candidate.canonicality.clone()),
        provenance: candidates
            .iter()
            .find_map(|candidate| candidate.provenance.clone()),
        recorded_at: candidates
            .iter()
            .find_map(|candidate| candidate.recorded_at.clone()),
        text: loaded.text,
        ranges: vec![json!({
            "path": path,
            "start_line": 1,
            "end_line": line_count,
        })],
        complete: true,
        selected_references: candidates
            .iter()
            .map(|candidate| candidate.reference.clone())
            .collect(),
        why_selected,
    }
}

fn section_hydrated_source(path: &str, candidates: &[&RetrievalCandidate]) -> HydratedSource {
    let mut seen_content = HashSet::new();
    let mut sections = Vec::new();
    let mut ranges = Vec::new();
    for candidate in candidates {
        if seen_content.insert(candidate.content.clone()) {
            sections.push(candidate.content.clone());
        }
        if let Some(range) = candidate
            .valid_time
            .as_ref()
            .filter(|range| !range.is_null())
        {
            ranges.push(range.clone());
        }
    }
    HydratedSource {
        source_ref: candidates[0].source_ref.clone(),
        path: Some(path.to_owned()),
        title: candidates
            .iter()
            .find_map(|candidate| candidate.heading.clone()),
        source_version: candidates
            .iter()
            .find_map(|candidate| candidate.source_version.clone()),
        content_hash: (candidates.len() == 1).then(|| candidates[0].content_hash.clone()),
        authority: candidates
            .iter()
            .find_map(|candidate| candidate.authority.clone()),
        canonicality: candidates
            .iter()
            .find_map(|candidate| candidate.canonicality.clone()),
        provenance: candidates
            .iter()
            .find_map(|candidate| candidate.provenance.clone()),
        recorded_at: candidates
            .iter()
            .find_map(|candidate| candidate.recorded_at.clone()),
        text: sections.join("\n\n"),
        ranges,
        complete: false,
        selected_references: candidates
            .iter()
            .map(|candidate| candidate.reference.clone())
            .collect(),
        why_selected: hydration_reasons(candidates, "selected_source_sections"),
    }
}

fn hydration_reasons(candidates: &[&RetrievalCandidate], hydration_reason: &str) -> Vec<String> {
    let mut reasons = BTreeSet::new();
    for candidate in candidates {
        reasons.extend(candidate.why_selected.iter().cloned());
    }
    reasons.insert(hydration_reason.to_owned());
    reasons.into_iter().collect()
}

fn open_retrieval_sufficiency(
    task: &str,
    candidates: &[RetrievalCandidate],
    sources: &[HydratedSource],
) -> Value {
    let mut seen_anchors = HashSet::new();
    let anchors = lexical_discovery_terms(task)
        .into_iter()
        .map(|term| match term.as_str() {
            "latest" | "newest" => "current".to_owned(),
            _ => term,
        })
        .filter(|term| !sufficiency_instruction_word(term) && seen_anchors.insert(term.clone()))
        .take(20)
        .collect::<Vec<_>>();
    let mut searchable = String::new();
    for source in sources {
        searchable.push_str(&source.source_ref);
        searchable.push(' ');
        if let Some(path) = &source.path {
            searchable.push_str(path);
            searchable.push(' ');
        }
        if let Some(title) = &source.title {
            searchable.push_str(title);
            searchable.push(' ');
        }
        searchable.push_str(&source.text);
        searchable.push(' ');
    }
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.path.is_none())
    {
        searchable.push_str(&candidate.source_ref);
        searchable.push(' ');
        searchable.push_str(&candidate.content);
        searchable.push(' ');
    }
    let searchable = searchable.to_lowercase();
    let unresolved = anchors
        .iter()
        .filter(|anchor| !searchable.contains(anchor.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let complete_sources = sources.iter().filter(|source| source.complete).count();
    let primary_source_complete = sources.first().is_some_and(|source| source.complete);
    let covered = anchors.len().saturating_sub(unresolved.len());
    let coverage_ratio = if anchors.is_empty() {
        1.0
    } else {
        covered as f64 / anchors.len() as f64
    };
    let status = if candidates.is_empty() {
        "no_evidence"
    } else if primary_source_complete && coverage_ratio >= 0.75 {
        "likely_sufficient"
    } else {
        "inspect_then_query_gaps"
    };
    json!({
        "status": status,
        "basis": "task_anchor_coverage_and_source_completeness",
        "anchor_count": anchors.len(),
        "covered_anchor_count": covered,
        "unresolved_anchors": unresolved.into_iter().take(12).collect::<Vec<_>>(),
        "selected_source_count": sources.len(),
        "complete_source_count": complete_sources,
    })
}

fn sufficiency_instruction_word(term: &str) -> bool {
    matches!(
        term,
        "answer"
            | "assess"
            | "brief"
            | "compact"
            | "create"
            | "decide"
            | "describe"
            | "distinguish"
            | "explain"
            | "give"
            | "identify"
            | "include"
            | "leave"
            | "list"
            | "name"
            | "prepare"
            | "preserve"
            | "report"
            | "required"
            | "return"
            | "state"
            | "task"
            | "tell"
            | "without"
    )
}

/// Return only a snapshot-matching, non-authoritative dream view whose hard
/// gates and paired retrieval evaluation passed. Stale or review-required
/// material is reported as status metadata but is never injected into context.
async fn learned_context_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    open_token_budget: usize,
) -> ApiResult<Value> {
    let candidate = sqlx::query(
        r#"
        SELECT candidate.id,candidate.candidate_corpus_revision_id,
               candidate.candidate_manifest_hash,candidate.created_at,
               job.id AS dream_job_id,job.completed_at,
               evaluation.id AS evaluation_id,evaluation.frozen_suite_hash,
               evaluation.result AS evaluation_result,
               model.requested_model,model.returned_model,model.usage
        FROM straylight.dream_candidate_revisions AS candidate
        JOIN straylight.dream_jobs AS job
          ON job.user_id=candidate.user_id AND job.id=candidate.dream_job_id
        JOIN straylight.dream_evaluations AS evaluation
          ON evaluation.user_id=candidate.user_id
         AND evaluation.candidate_revision_id=candidate.id
         AND evaluation.passed
        LEFT JOIN straylight.dream_model_receipts AS model
          ON model.user_id=job.user_id AND model.dream_job_id=job.id
        WHERE candidate.user_id=$1 AND candidate.scope_id=$2
          AND candidate.base_corpus_revision_id=$3
          AND candidate.status='gated' AND job.status='awaiting_review'
          AND NOT EXISTS (
            SELECT 1 FROM straylight.dream_gate_results AS gate
            WHERE gate.user_id=candidate.user_id
              AND gate.candidate_revision_id=candidate.id
              AND gate.hard_gate AND NOT gate.passed
          )
        ORDER BY job.completed_at DESC NULLS LAST,candidate.created_at DESC,candidate.id DESC
        LIMIT 1
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?;

    let scheduler = sqlx::query(
        r#"
        SELECT dirty_score,dirty_reasons,dirty_since,last_completed_at,
               consolidated_corpus_revision_id,cooldown_until,
               enabled,paused
        FROM straylight.dream_scheduler_controls
        WHERE user_id=$1 AND scope_id=$2
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(candidate) = candidate else {
        let latest = sqlx::query(
            r#"
            SELECT candidate.base_corpus_revision_id,candidate.status,
                   job.status AS job_status,job.completed_at
            FROM straylight.dream_candidate_revisions AS candidate
            JOIN straylight.dream_jobs AS job
              ON job.user_id=candidate.user_id AND job.id=candidate.dream_job_id
            WHERE candidate.user_id=$1 AND candidate.scope_id=$2
            ORDER BY job.completed_at DESC NULLS LAST,candidate.created_at DESC
            LIMIT 1
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .fetch_optional(&mut **tx)
        .await?;
        let dirty_score = scheduler
            .as_ref()
            .and_then(|row| row.try_get::<i32, _>("dirty_score").ok())
            .unwrap_or_default();
        let status = if dirty_score > 0 {
            "refresh_pending"
        } else if latest.is_some() {
            "stale"
        } else {
            "not_available"
        };
        return Ok(json!({
            "status": status,
            "included_by_default": false,
            "authority": "derived_non_authoritative",
            "pinned_corpus_revision": session.corpus_revision_ref,
            "items": [],
            "scheduler": scheduler.map(|row| json!({
                "enabled": row.try_get::<bool,_>("enabled").unwrap_or(false),
                "paused": row.try_get::<bool,_>("paused").unwrap_or(false),
                "dirty_score": row.try_get::<i32,_>("dirty_score").unwrap_or_default(),
                "dirty_reasons": row.try_get::<Vec<String>,_>("dirty_reasons").unwrap_or_default(),
                "dirty_since": row.try_get::<Option<DateTime<Utc>>,_>("dirty_since").unwrap_or(None),
                "last_completed_at": row.try_get::<Option<DateTime<Utc>>,_>("last_completed_at").unwrap_or(None),
                "cooldown_until": row.try_get::<Option<DateTime<Utc>>,_>("cooldown_until").unwrap_or(None)
            })),
            "latest_candidate": latest.map(|row| json!({
                "base_corpus_revision": row.try_get::<Uuid,_>("base_corpus_revision_id").ok().map(|id| record_ref("revision",id)),
                "candidate_status": row.try_get::<String,_>("status").ok(),
                "job_status": row.try_get::<String,_>("job_status").ok(),
                "completed_at": row.try_get::<Option<DateTime<Utc>>,_>("completed_at").ok().flatten()
            })),
            "reason": "No fresh hard-gated derived view matches this immutable session revision."
        }));
    };

    let candidate_id: Uuid = candidate.try_get("id")?;
    let rows = sqlx::query(
        r#"
        SELECT item.target_record_id,
               CASE target.record_kind
                 WHEN 'source_episode' THEN 'source'
                 WHEN 'temporal_spec' THEN 'temporal'
                 WHEN 'recurrence_spec' THEN 'recurrence'
                 ELSE target.record_kind
               END AS target_kind,
               item.disposition,item.proposal,
               ARRAY(
                 SELECT
                   CASE key.record_kind
                     WHEN 'source_episode' THEN 'source'
                     WHEN 'temporal_spec' THEN 'temporal'
                     WHEN 'recurrence_spec' THEN 'recurrence'
                     ELSE key.record_kind
                   END || ':' || source_id::text
                 FROM unnest(item.source_refs) AS source_id
                 JOIN straylight.record_keys AS key
                   ON key.user_id=item.user_id AND key.record_id=source_id
                 ORDER BY source_id
               ) AS source_refs
        FROM straylight.dream_candidate_items AS item
        JOIN straylight.record_keys AS target
          ON target.user_id=item.user_id AND target.record_id=item.target_record_id
        WHERE item.user_id=$1 AND item.candidate_revision_id=$2
          AND NOT item.requires_review
          AND item.disposition IN (
            'add_derived_view','add_alias','add_soft_link','add_cluster','add_flag'
          )
        ORDER BY item.disposition,item.target_record_id
        "#,
    )
    .bind(session.user_id)
    .bind(candidate_id)
    .fetch_all(&mut **tx)
    .await?;
    let derived_budget = (open_token_budget / 4).min(6_000);
    let mut estimated_tokens: usize = 0;
    let mut omitted_items: usize = 0;
    let mut items = Vec::new();
    for row in rows {
        let target_record_id: Uuid = row.try_get("target_record_id")?;
        let target_kind: String = row.try_get("target_kind")?;
        let item = json!({
            "target_ref": record_ref(&target_kind,target_record_id),
            "disposition": row.try_get::<String,_>("disposition")?,
            "source_refs": row.try_get::<Vec<String>,_>("source_refs")?,
            "proposal": row.try_get::<Value,_>("proposal")?
        });
        let item_tokens = estimate_tokens(&serde_json::to_string(&item)?);
        if estimated_tokens.saturating_add(item_tokens) > derived_budget {
            omitted_items += 1;
            continue;
        }
        estimated_tokens += item_tokens;
        items.push(item);
    }
    let included_by_default = !items.is_empty();
    Ok(json!({
        "status": "fresh",
        "included_by_default": included_by_default,
        "authority": "derived_non_authoritative",
        "truth_boundary": "The pinned evidence ledger remains canonical; every learned item is inspectable and rebuildable.",
        "pinned_corpus_revision": session.corpus_revision_ref,
        "dream_ref": record_ref("dream",candidate.try_get::<Uuid,_>("dream_job_id")?),
        "candidate_ref": format!("candidate:{candidate_id}"),
        "candidate_corpus_revision": record_ref(
            "revision",
            candidate.try_get::<Uuid,_>("candidate_corpus_revision_id")?
        ),
        "candidate_manifest_hash": format!(
            "sha256:{}",
            candidate.try_get::<String,_>("candidate_manifest_hash")?
        ),
        "generated_at": candidate.try_get::<Option<DateTime<Utc>>,_>("completed_at")?
            .or(Some(candidate.try_get::<DateTime<Utc>,_>("created_at")?)),
        "hard_gates_passed": true,
        "retrieval_evaluation": {
            "id": candidate.try_get::<Uuid,_>("evaluation_id")?,
            "frozen_suite_hash": format!(
                "sha256:{}",
                candidate.try_get::<String,_>("frozen_suite_hash")?
            ),
            "result": candidate.try_get::<Value,_>("evaluation_result")?
        },
        "model": {
            "requested": candidate.try_get::<Option<String>,_>("requested_model")?,
            "returned": candidate.try_get::<Option<String>,_>("returned_model")?,
            "usage": candidate.try_get::<Option<Value>,_>("usage")?.unwrap_or_else(|| json!({}))
        },
        "items": items,
        "estimated_tokens": estimated_tokens,
        "omitted_items": omitted_items,
        "truncated": omitted_items > 0
    }))
}

/// Run a batch of exact, structured-state, FTS, pgvector, temporal, and
/// relation queries against one immutable session revision.
pub async fn query(
    state: &AppState,
    auth: &AuthContext,
    request: &QueryRequest,
) -> ApiResult<QueryBatchResult> {
    let specs = request.normalized_queries()?;
    if is_stage_ref(&request.session_id) {
        let stage = load_staged_corpus(state, auth, &request.session_id, Capability::Query).await?;
        return run_staged_query_batch(state, auth, &stage, specs).await;
    }
    let session = load_session(state, auth, &request.session_id, Capability::Query).await?;
    run_query_batch(state, auth, &session, specs, QuerySelection::Diversified).await
}

async fn run_query_batch(
    state: &AppState,
    auth: &AuthContext,
    session: &PinnedSession,
    specs: Vec<QuerySpec>,
    selection: QuerySelection,
) -> ApiResult<QueryBatchResult> {
    let semantic_indices: Vec<usize> = specs
        .iter()
        .enumerate()
        .filter(|(_, spec)| {
            effective_modes(spec).contains(&RetrievalMode::Semantic)
                && spec
                    .query
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .map(|(index, _)| index)
        .collect();
    let semantic_inputs: Vec<String> = semantic_indices
        .iter()
        .map(|index| specs[*index].query.clone().unwrap_or_default())
        .collect();
    let mut semantic_vectors = HashMap::<usize, Vec<f32>>::new();
    let mut embedding_failure = None;
    if !semantic_inputs.is_empty() {
        quota::reserve_for_auth(
            state,
            auth,
            UsageReservation {
                embedding_input_chars: semantic_inputs.iter().map(String::len).sum::<usize>()
                    as u64,
                ..UsageReservation::default()
            },
        )
        .await?;
        if state.embedder.dimensions() != 1536 {
            embedding_failure = Some(format!(
                "configured embedding dimensions {} do not match the vector(1536) schema",
                state.embedder.dimensions()
            ));
        } else {
            match state.embedder.embed(&semantic_inputs).await {
                Ok(vectors) => {
                    for (index, vector) in semantic_indices.iter().copied().zip(vectors) {
                        semantic_vectors.insert(index, vector);
                    }
                }
                Err(error) => embedding_failure = Some(error.to_string()),
            }
        }
    }

    let mut items = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        let mut tx = state.begin_read(auth).await?;
        match query_item_tx(
            &mut tx,
            session,
            spec,
            semantic_vectors.get(&index),
            embedding_failure.as_deref(),
            state.embedder.model(),
            selection,
        )
        .await
        {
            Ok(item) => {
                tx.commit().await?;
                items.push(item);
            }
            Err(error) => {
                tx.rollback().await?;
                items.push(QueryItemResult {
                    id: spec.id.clone().unwrap_or_else(|| format!("q{index}")),
                    status: ResponseStatus::Inconsistent,
                    results: Vec::new(),
                    coverage: Coverage {
                        searched: Vec::new(),
                        unsearched: vec![CoveragePartition {
                            partition: "query_item".to_owned(),
                            lane: "query".to_owned(),
                            completeness: "failed".to_owned(),
                            index_revision: Some(session.corpus_revision_ref.clone()),
                            searched_count: 0,
                            candidate_count: 0,
                            failure_reason: Some(error.to_string()),
                        }],
                        absence_safe: false,
                    },
                    conflicts: Vec::new(),
                    gaps: vec![json!({"kind": "query_item_failed", "message": error.to_string()})],
                    ambiguities: Vec::new(),
                });
            }
        }
    }

    let status = if items
        .iter()
        .any(|item| item.status == ResponseStatus::Degraded)
    {
        ResponseStatus::Degraded
    } else if items
        .iter()
        .any(|item| item.status == ResponseStatus::Ambiguous)
    {
        ResponseStatus::Ambiguous
    } else if items.iter().any(|item| {
        matches!(
            item.status,
            ResponseStatus::Inconsistent | ResponseStatus::Partial
        )
    }) {
        ResponseStatus::Partial
    } else {
        ResponseStatus::Complete
    };
    let mut coverage = Coverage::default();
    for item in &items {
        coverage.searched.extend(item.coverage.searched.clone());
        coverage.unsearched.extend(item.coverage.unsearched.clone());
    }
    coverage.absence_safe = false;
    Ok(QueryBatchResult {
        session_id: session.session_ref.clone(),
        corpus_revision: session.corpus_revision_ref.clone(),
        status,
        items,
        freshness: session.freshness.clone(),
        coverage,
    })
}

async fn query_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    semantic_vector: Option<&Vec<f32>>,
    embedding_failure: Option<&str>,
    embedding_model: &str,
    selection: QuerySelection,
) -> ApiResult<QueryItemResult> {
    validate_query_scope(session, spec.scope.as_ref())?;
    let root_ids = query_root_ids_tx(tx, session, spec).await?;
    let modes = effective_modes(spec);
    let total_records = corpus_record_count_tx(tx, session).await?;
    let mut lanes: Vec<(&'static str, Vec<RetrievalCandidate>)> = Vec::new();
    let mut coverage = Coverage::default();
    let mut degraded = false;

    for mode in [
        RetrievalMode::Exact,
        RetrievalMode::Structured,
        RetrievalMode::Lexical,
        RetrievalMode::Semantic,
        RetrievalMode::Temporal,
        RetrievalMode::Relations,
    ] {
        if !modes.contains(&mode) {
            coverage.unsearched.push(CoveragePartition {
                partition: mode_name(mode).to_owned(),
                lane: mode_name(mode).to_owned(),
                completeness: "not_requested".to_owned(),
                index_revision: Some(session.corpus_revision_ref.clone()),
                searched_count: 0,
                candidate_count: 0,
                failure_reason: None,
            });
            continue;
        }
        let lane_started = Instant::now();
        let (lane_name, results, failure, completeness) = match mode {
            RetrievalMode::Exact => (
                "exact",
                exact_lane_tx(tx, session, spec, &root_ids).await?,
                None,
                "complete",
            ),
            RetrievalMode::Structured => (
                "structured",
                structured_lane_tx(tx, session, spec, &root_ids).await?,
                None,
                "complete",
            ),
            RetrievalMode::Lexical => {
                if spec
                    .query
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    (
                        "lexical",
                        Vec::new(),
                        Some("query text is required".to_owned()),
                        "unsearched",
                    )
                } else {
                    (
                        "lexical",
                        lexical_lane_tx(tx, session, spec, &root_ids).await?,
                        None,
                        "best_effort",
                    )
                }
            }
            RetrievalMode::Semantic => {
                if let Some(failure) = embedding_failure {
                    degraded = true;
                    (
                        "semantic",
                        Vec::new(),
                        Some(failure.to_owned()),
                        "unsearched",
                    )
                } else if let Some(vector) = semantic_vector {
                    (
                        "semantic",
                        semantic_lane_tx(tx, session, spec, vector, embedding_model, &root_ids)
                            .await?,
                        None,
                        "best_effort",
                    )
                } else {
                    (
                        "semantic",
                        Vec::new(),
                        Some("query text is required".to_owned()),
                        "unsearched",
                    )
                }
            }
            RetrievalMode::Temporal => (
                "temporal",
                temporal_lane_tx(tx, session, spec, &root_ids).await?,
                None,
                "complete",
            ),
            RetrievalMode::Relations => (
                "relations",
                relation_lane_tx(tx, session, spec, &root_ids).await?,
                None,
                "complete",
            ),
        };
        let candidate_count = results.len();
        telemetry::record_retrieval_lane(
            lane_name,
            candidate_count,
            failure.is_some(),
            lane_started,
        );
        let partition = CoveragePartition {
            partition: lane_name.to_owned(),
            lane: lane_name.to_owned(),
            completeness: completeness.to_owned(),
            index_revision: Some(session.corpus_revision_ref.clone()),
            searched_count: if matches!(mode, RetrievalMode::Semantic | RetrievalMode::Lexical) {
                candidate_count
            } else {
                total_records
            },
            candidate_count,
            failure_reason: failure,
        };
        if partition.failure_reason.is_some() {
            coverage.unsearched.push(partition);
        } else {
            coverage.searched.push(partition);
            lanes.push((lane_name, results));
        }
    }

    let mut selected = match selection {
        QuerySelection::Diversified => reciprocal_rank_fusion(lanes, spec.limit, 3),
        QuerySelection::SourceCoherent => source_coherent_rank_fusion(lanes, spec.limit, 3),
    };
    normalize_selection_reasons(&mut selected);
    expand_selected_tx(tx, session, spec, &mut selected).await?;
    selected.truncate(spec.limit);
    let conflicts = contradiction_summaries_tx(tx, session, &selected).await?;
    let ambiguities = exact_object_ambiguities(spec, &selected);
    let gaps = if selected.is_empty() {
        vec![json!({
            "kind": "no_result",
            "message": "No candidate is not evidence that the requested fact does not exist"
        })]
    } else {
        Vec::new()
    };
    coverage.absence_safe = false;
    Ok(QueryItemResult {
        id: spec.id.clone().unwrap_or_else(|| "q0".to_owned()),
        status: if degraded {
            ResponseStatus::Degraded
        } else if !ambiguities.is_empty() {
            ResponseStatus::Ambiguous
        } else if coverage
            .unsearched
            .iter()
            .any(|item| item.completeness == "unsearched")
        {
            ResponseStatus::Partial
        } else {
            ResponseStatus::Complete
        },
        results: selected,
        coverage,
        conflicts,
        gaps,
        ambiguities,
    })
}

async fn load_session_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    session_id: Uuid,
    capability: Capability,
) -> ApiResult<PinnedSession> {
    let row = sqlx::query(
        r#"
        SELECT
          s.id, s.user_id, s.scope_id, sc.scope_ref, s.credential_id,
          s.corpus_revision_id, cr.revision_number, cr.manifest_hash,
          s.task_hash, s.task_size_bytes, s.capability_snapshot,
          s.authorization_scope_refs, s.policy_projection_hash, s.mode,
          s.parent_session_id, s.resumed_checkpoint_id, s.created_at, s.expires_at,
          (SELECT max(se.captured_at) FROM straylight.source_episodes se
             WHERE se.user_id = s.user_id AND se.scope_id = s.scope_id) AS source_updated_at,
          (SELECT max(c.created_at) FROM straylight.chunks c
             WHERE c.user_id = s.user_id AND c.scope_id = s.scope_id) AS lexical_index_updated_at,
          (SELECT max(e.created_at) FROM straylight.embeddings e
             WHERE e.user_id = s.user_id AND e.scope_id = s.scope_id) AS semantic_index_updated_at
        FROM straylight.sessions s
        JOIN straylight.scopes sc
          ON sc.user_id = s.user_id AND sc.id = s.scope_id
        JOIN straylight.corpus_revisions cr
          ON cr.user_id = s.user_id AND cr.id = s.corpus_revision_id
        WHERE s.user_id = $1
          AND s.id = $2
          AND s.credential_id = $3
          AND s.expires_at > clock_timestamp()
          AND sc.scope_ref = ANY($4::text[])
          AND sc.scope_ref = ANY(s.authorization_scope_refs)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(auth.credential_id.0)
    .bind(&auth.scope_refs)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", &session_id.to_string()))?;

    let capability_snapshot: Vec<String> = row.try_get("capability_snapshot")?;
    if !capability_snapshot
        .iter()
        .any(|value| value == capability.as_str())
    {
        return Err(ApiError::capability(capability.as_str()));
    }
    let corpus_revision_id: Uuid = row.try_get("corpus_revision_id")?;
    let source_updated_at: Option<DateTime<Utc>> = row.try_get("source_updated_at")?;
    let lexical_index_updated_at: Option<DateTime<Utc>> =
        row.try_get("lexical_index_updated_at")?;
    let semantic_index_updated_at: Option<DateTime<Utc>> =
        row.try_get("semantic_index_updated_at")?;
    Ok(PinnedSession {
        session_id,
        session_ref: record_ref("session", session_id),
        user_id: row.try_get("user_id")?,
        scope_id: row.try_get("scope_id")?,
        scope_ref: row.try_get("scope_ref")?,
        credential_id: row.try_get("credential_id")?,
        corpus_revision_id,
        corpus_revision_ref: record_ref("revision", corpus_revision_id),
        revision_number: row.try_get("revision_number")?,
        manifest_hash: prefixed_hash(row.try_get::<String, _>("manifest_hash")?),
        task_hash: row.try_get("task_hash")?,
        task_size_bytes: usize::try_from(row.try_get::<i32, _>("task_size_bytes")?)
            .unwrap_or_default(),
        capability_snapshot,
        authorization_scope_refs: row.try_get("authorization_scope_refs")?,
        policy_projection_hash: prefixed_hash(row.try_get::<String, _>("policy_projection_hash")?),
        mode: row.try_get("mode")?,
        parent_session_id: row.try_get("parent_session_id")?,
        resumed_checkpoint_id: row.try_get("resumed_checkpoint_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
        freshness: Freshness {
            source_updated_at,
            normalized_at: lexical_index_updated_at,
            lexical_index_updated_at,
            semantic_index_updated_at,
            replica_applied_at: None,
        },
    })
}

fn validate_open_request(session: &PinnedSession, request: &OpenRequest) -> ApiResult<()> {
    let hash = sha256_hex(request.task.as_bytes());
    if hash != session.task_hash || request.task.len() != session.task_size_bytes {
        return Err(ApiError::conflict(
            "session_task_mismatch",
            "the supplied ephemeral task does not match the task hash bound to the session",
            json!({"task_size_bytes": request.task.len()}),
        ));
    }
    if !open_revision_matches(
        &request.as_of,
        session.corpus_revision_id,
        session.revision_number,
    ) {
        return Err(ApiError::conflict(
            "revision_mismatch",
            "open.as_of does not match the immutable session revision",
            json!({"session_revision": session.corpus_revision_ref}),
        ));
    }
    if let Some(scope) = &request.hints.authorization_scope
        && scope != &session.scope_ref
    {
        return Err(ApiError::public(
            http::StatusCode::FORBIDDEN,
            "capability_denied",
            "the requested authorization scope is not the session scope",
        ));
    }
    Ok(())
}

fn open_revision_matches(value: &str, revision_id: Uuid, revision_number: i64) -> bool {
    value == "latest"
        || value == revision_number.to_string()
        || parse_uuid_ref(value, &["revision"]).is_ok_and(|parsed| parsed == revision_id)
}

fn validate_query_scope(session: &PinnedSession, scope: Option<&QueryScope>) -> ApiResult<()> {
    let requested = match scope {
        Some(QueryScope::Ref(value)) => Some(value.as_str()),
        Some(QueryScope::Structured {
            authorization_scope,
            ..
        }) => authorization_scope.as_deref(),
        None => None,
    };
    if requested.is_some_and(|value| value != session.scope_ref) {
        return Err(ApiError::public(
            http::StatusCode::FORBIDDEN,
            "capability_denied",
            "query scope cannot broaden the immutable session scope",
        ));
    }
    Ok(())
}

fn query_root_references(spec: &QuerySpec) -> Vec<String> {
    let mut roots = BTreeSet::new();
    if let Some(QueryScope::Structured { root_refs, .. }) = &spec.scope {
        roots.extend(root_refs.iter().cloned());
    }
    if let Some(scope_root) = string_filter(&spec.r#where, "scope_root") {
        roots.insert(scope_root);
    }
    roots.into_iter().collect()
}

async fn query_root_ids_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
) -> ApiResult<Vec<Uuid>> {
    let mut ids = Vec::new();
    for reference in query_root_references(spec) {
        if reference.trim().is_empty() {
            return Err(ApiError::invalid("query root refs cannot be empty"));
        }
        ids.push(resolve_record_tx(tx, session, &reference).await?.id);
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(ids);
    }

    let mut region_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH active AS MATERIALIZED (
          SELECT member.record_id,member.record_kind,member.record_version
          FROM straylight.corpus_members AS member
          WHERE member.user_id=$1 AND member.scope_id=$2
            AND member.corpus_revision_id=$3 AND member.disposition='active'
        ),
        seed AS (
          SELECT active.record_id
          FROM active
          WHERE active.record_id=ANY($4::uuid[])
        ),
        root_relations AS (
          SELECT DISTINCT endpoint.relation_id,endpoint.relation_version
          FROM straylight.relation_endpoints AS endpoint
          JOIN active AS relation_member
            ON relation_member.record_id=endpoint.relation_id
           AND relation_member.record_kind='relation'
           AND relation_member.record_version=endpoint.relation_version
          WHERE endpoint.user_id=$1
            AND endpoint.target_record_id IN (SELECT record_id FROM seed)
        ),
        relation_neighbors AS (
          SELECT relation_id AS record_id FROM root_relations
          UNION
          SELECT endpoint.target_record_id
          FROM straylight.relation_endpoints AS endpoint
          JOIN root_relations AS root_relation
            ON root_relation.relation_id=endpoint.relation_id
           AND root_relation.relation_version=endpoint.relation_version
          JOIN active AS target_member
            ON target_member.record_id=endpoint.target_record_id
          WHERE endpoint.user_id=$1
        ),
        semantic_seed_base AS (
          SELECT record_id FROM seed
          UNION
          SELECT record_id FROM relation_neighbors
        ),
        event_neighbors AS (
          SELECT occurrence.occurrence_object_id AS record_id
          FROM straylight.event_occurrences AS occurrence
          JOIN active AS occurrence_member
            ON occurrence_member.record_id=occurrence.occurrence_object_id
           AND occurrence_member.record_kind='object'
          WHERE occurrence.user_id=$1
            AND occurrence.series_object_id IN (
              SELECT record_id FROM semantic_seed_base
            )
          UNION
          SELECT occurrence.series_object_id
          FROM straylight.event_occurrences AS occurrence
          JOIN active AS series_member
            ON series_member.record_id=occurrence.series_object_id
           AND series_member.record_kind='object'
          WHERE occurrence.user_id=$1
            AND occurrence.occurrence_object_id IN (
              SELECT record_id FROM semantic_seed_base
            )
        ),
        checkpoint_neighbors AS (
          SELECT source_ref.source_record_id AS record_id
          FROM straylight.checkpoint_source_refs AS source_ref
          JOIN active AS checkpoint_member
            ON checkpoint_member.record_id=source_ref.checkpoint_id
           AND checkpoint_member.record_kind='checkpoint'
          JOIN active AS source_member
            ON source_member.record_id=source_ref.source_record_id
          WHERE source_ref.user_id=$1 AND source_ref.corpus_revision_id=$3
            AND source_ref.checkpoint_id IN (
              SELECT record_id FROM semantic_seed_base
            )
        ),
        semantic_seed AS (
          SELECT record_id FROM semantic_seed_base
          UNION
          SELECT record_id FROM event_neighbors
          UNION
          SELECT record_id FROM checkpoint_neighbors
        ),
        rooted_claims AS (
          SELECT DISTINCT about.claim_id AS record_id
          FROM straylight.claim_about_targets AS about
          JOIN active AS claim_member
            ON claim_member.record_id=about.claim_id
           AND claim_member.record_kind='claim'
          WHERE about.user_id=$1
            AND about.target_record_id IN (SELECT record_id FROM semantic_seed)
        ),
        semantic_region AS (
          SELECT record_id FROM semantic_seed
          UNION
          SELECT record_id FROM rooted_claims
        ),
        linked_evidence AS (
          SELECT link.evidence_id AS record_id
          FROM straylight.claim_evidence_links AS link
          JOIN active AS evidence_member
            ON evidence_member.record_id=link.evidence_id
           AND evidence_member.record_kind='evidence'
          WHERE link.user_id=$1
            AND link.claim_id IN (SELECT record_id FROM semantic_region)
          UNION
          SELECT link.evidence_id
          FROM straylight.relation_evidence_links AS link
          JOIN active AS relation_member
            ON relation_member.record_id=link.relation_id
           AND relation_member.record_kind='relation'
           AND relation_member.record_version=link.relation_version
          JOIN active AS evidence_member
            ON evidence_member.record_id=link.evidence_id
           AND evidence_member.record_kind='evidence'
          WHERE link.user_id=$1
            AND link.relation_id IN (SELECT record_id FROM semantic_region)
          UNION
          SELECT handle.evidence_id
          FROM straylight.object_handles AS handle
          JOIN active AS evidence_member
            ON evidence_member.record_id=handle.evidence_id
           AND evidence_member.record_kind='evidence'
          WHERE handle.user_id=$1
            AND handle.object_id IN (SELECT record_id FROM semantic_region)
        ),
        direct_sources AS (
          SELECT object_revision.source_episode_id AS record_id
          FROM semantic_region AS region
          JOIN active AS object_member
            ON object_member.record_id=region.record_id
           AND object_member.record_kind='object'
          JOIN straylight.object_revisions AS object_revision
            ON object_revision.user_id=$1
           AND object_revision.object_id=object_member.record_id
           AND object_revision.version=object_member.record_version
          UNION
          SELECT claim.source_episode_id
          FROM semantic_region AS region
          JOIN active AS claim_member
            ON claim_member.record_id=region.record_id
           AND claim_member.record_kind='claim'
          JOIN straylight.claims AS claim
            ON claim.user_id=$1 AND claim.id=claim_member.record_id
          UNION
          SELECT relation_revision.source_episode_id
          FROM semantic_region AS region
          JOIN active AS relation_member
            ON relation_member.record_id=region.record_id
           AND relation_member.record_kind='relation'
          JOIN straylight.relation_revisions AS relation_revision
            ON relation_revision.user_id=$1
           AND relation_revision.relation_id=relation_member.record_id
           AND relation_revision.version=relation_member.record_version
          UNION
          SELECT document_revision.source_episode_id
          FROM semantic_region AS region
          JOIN active AS document_member
            ON document_member.record_id=region.record_id
           AND document_member.record_kind='document'
          JOIN straylight.document_revisions AS document_revision
            ON document_revision.user_id=$1
           AND document_revision.document_id=document_member.record_id
           AND document_revision.version=document_member.record_version
          UNION
          SELECT document_revision.source_episode_id
          FROM semantic_region AS region
          JOIN active AS chunk_member
            ON chunk_member.record_id=region.record_id
           AND chunk_member.record_kind='chunk'
          JOIN straylight.chunks AS chunk
            ON chunk.user_id=$1 AND chunk.id=chunk_member.record_id
          JOIN straylight.document_revisions AS document_revision
            ON document_revision.user_id=chunk.user_id
           AND document_revision.document_id=chunk.document_id
           AND document_revision.version=chunk.document_version
          UNION
          SELECT evidence.source_episode_id
          FROM semantic_region AS region
          JOIN active AS evidence_member
            ON evidence_member.record_id=region.record_id
           AND evidence_member.record_kind='evidence'
          JOIN straylight.evidence_items AS evidence
            ON evidence.user_id=$1 AND evidence.id=evidence_member.record_id
          UNION
          SELECT evidence.source_episode_id
          FROM linked_evidence AS linked
          JOIN straylight.evidence_items AS evidence
            ON evidence.user_id=$1 AND evidence.id=linked.record_id
          UNION
          SELECT link.source_episode_id
          FROM semantic_region AS region
          JOIN active AS asset_member
            ON asset_member.record_id=region.record_id
           AND asset_member.record_kind='asset'
          JOIN straylight.source_asset_links AS link
            ON link.user_id=$1 AND link.asset_id=asset_member.record_id
          UNION
          SELECT region.record_id
          FROM semantic_region AS region
          JOIN active AS source_member
            ON source_member.record_id=region.record_id
           AND source_member.record_kind='source_episode'
        ),
        source_region AS (
          SELECT DISTINCT source_member.record_id
          FROM direct_sources AS source
          JOIN active AS source_member
            ON source_member.record_id=source.record_id
           AND source_member.record_kind='source_episode'
        ),
        evidence_region AS (
          SELECT evidence_member.record_id
          FROM active AS evidence_member
          JOIN straylight.evidence_items AS evidence
            ON evidence.user_id=$1 AND evidence.id=evidence_member.record_id
          WHERE evidence_member.record_kind='evidence'
            AND evidence.source_episode_id IN (SELECT record_id FROM source_region)
          UNION
          SELECT record_id FROM linked_evidence
        ),
        document_region AS (
          SELECT document_member.record_id
          FROM active AS document_member
          JOIN straylight.document_revisions AS document_revision
            ON document_revision.user_id=$1
           AND document_revision.document_id=document_member.record_id
           AND document_revision.version=document_member.record_version
          WHERE document_member.record_kind='document'
            AND document_revision.source_episode_id IN (
              SELECT record_id FROM source_region
            )
        ),
        chunk_region AS (
          SELECT chunk_member.record_id
          FROM active AS chunk_member
          JOIN straylight.chunks AS chunk
            ON chunk.user_id=$1 AND chunk.id=chunk_member.record_id
          WHERE chunk_member.record_kind='chunk'
            AND chunk.document_id IN (SELECT record_id FROM document_region)
        ),
        asset_region AS (
          SELECT asset_member.record_id
          FROM active AS asset_member
          JOIN straylight.source_asset_links AS link
            ON link.user_id=$1 AND link.asset_id=asset_member.record_id
          WHERE asset_member.record_kind='asset'
            AND link.source_episode_id IN (SELECT record_id FROM source_region)
        )
        SELECT record_id FROM semantic_region
        UNION SELECT record_id FROM source_region
        UNION SELECT record_id FROM evidence_region
        UNION SELECT record_id FROM document_region
        UNION SELECT record_id FROM chunk_region
        UNION SELECT record_id FROM asset_region
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    region_ids.sort_unstable();
    region_ids.dedup();
    Ok(region_ids)
}

async fn corpus_map_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
) -> ApiResult<Value> {
    let rows = sqlx::query(
        r#"
        SELECT record_kind, record_id, record_version, disposition, kind_count
        FROM straylight_auth.read_manifest_sample($1, $2, $3, $4)
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(i32::try_from(CORPUS_MAP_SAMPLE_PER_KIND).unwrap_or(i32::MAX))
    .fetch_all(&mut **tx)
    .await?;
    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    let mut record_counts = BTreeMap::<String, usize>::new();
    for row in rows {
        let kind: String = row.try_get("record_kind")?;
        let id: Uuid = row.try_get("record_id")?;
        record_counts.insert(
            kind.clone(),
            usize::try_from(row.try_get::<i64, _>("kind_count")?).unwrap_or(usize::MAX),
        );
        groups.entry(kind.clone()).or_default().push(json!({
            "ref": record_ref(&kind, id),
            "version": row.try_get::<Option<i32>, _>("record_version")?,
            "disposition": row.try_get::<String, _>("disposition")?
        }));
    }
    let profiles = sqlx::query(
        r#"
        SELECT p.profile_ref::text AS profile_ref, count(*)::bigint AS count
        FROM straylight.corpus_members cm
        JOIN straylight.object_revision_profiles p
          ON p.user_id = cm.user_id
         AND p.object_id = cm.record_id
         AND p.object_version = cm.record_version
        WHERE cm.user_id = $1 AND cm.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
        GROUP BY p.profile_ref ORDER BY p.profile_ref
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    let profile_counts: Vec<Value> = profiles
        .into_iter()
        .map(|row| {
            Ok(json!({
                "profile": row.try_get::<String, _>("profile_ref")?,
                "count": row.try_get::<i64, _>("count")?
            }))
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let truncated = record_counts
        .iter()
        .any(|(kind, count)| groups.get(kind).map(Vec::len).unwrap_or_default() < *count);
    Ok(json!({
        "records": groups,
        "record_counts": record_counts,
        "profile_counts": profile_counts,
        "sample_limit_per_kind": CORPUS_MAP_SAMPLE_PER_KIND,
        "truncated": truncated,
        "available_views": [
            "current_state", "structured", "outline", "full", "range", "neighbors",
            "relationships", "history", "diff", "last_known_good", "materialize_scope"
        ]
    }))
}

async fn resolved_scope_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    request: &OpenRequest,
) -> ApiResult<(Value, Vec<Value>)> {
    let rows = sqlx::query(
        r#"
        SELECT root.position, root.record_id, rk.record_kind
        FROM straylight.session_root_refs root
        JOIN straylight.record_keys rk
          ON rk.user_id = root.user_id AND rk.record_id = root.record_id
        WHERE root.user_id = $1 AND root.session_id = $2
        ORDER BY root.position
        "#,
    )
    .bind(session.user_id)
    .bind(session.session_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut roots = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("record_id")?;
        let kind: String = row.try_get("record_kind")?;
        roots.push(record_descriptor_tx(tx, session, id, &kind).await?);
    }

    let mut ambiguities = Vec::new();
    let mut requested = Vec::new();
    for reference in request
        .hints
        .root_refs
        .iter()
        .chain(request.hints.open_object_refs.iter())
    {
        let candidates = resolve_reference_candidates_tx(tx, session, reference).await?;
        if candidates.len() == 1 {
            requested.push(candidates[0].clone());
        } else if candidates.len() > 1 {
            ambiguities.push(json!({"input": reference, "candidates": candidates}));
        }
    }
    Ok((
        json!({
            "authorization_scope": session.scope_ref,
            "root_objects": roots,
            "requested_lenses": requested,
            "ambiguities": ambiguities
        }),
        ambiguities,
    ))
}

async fn record_descriptor_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    id: Uuid,
    kind: &str,
) -> ApiResult<Value> {
    let label: Option<String> = if kind == "object" {
        sqlx::query_scalar(
            r#"
            SELECT orev.label
            FROM straylight.corpus_members cm
            JOIN straylight.object_revisions orev
              ON orev.user_id = cm.user_id AND orev.object_id = cm.record_id
             AND orev.version = cm.record_version
            WHERE cm.user_id = $1 AND cm.scope_id = $2
              AND cm.corpus_revision_id = $3 AND cm.record_id = $4
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(session.corpus_revision_id)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .flatten()
    } else {
        None
    };
    let profiles: Vec<String> = if kind == "object" {
        sqlx::query_scalar(
            r#"
            SELECT p.profile_ref::text
            FROM straylight.corpus_members cm
            JOIN straylight.object_revision_profiles p
              ON p.user_id = cm.user_id AND p.object_id = cm.record_id
             AND p.object_version = cm.record_version
            WHERE cm.user_id = $1 AND cm.scope_id = $2
              AND cm.corpus_revision_id = $3 AND cm.record_id = $4
            ORDER BY p.profile_ref
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(session.corpus_revision_id)
        .bind(id)
        .fetch_all(&mut **tx)
        .await?
    } else {
        Vec::new()
    };
    Ok(json!({
        "ref": record_ref(kind, id),
        "kind": kind,
        "label": label,
        "type_profiles": profiles,
        "resolution": "exact"
    }))
}

async fn resolve_reference_candidates_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    reference: &str,
) -> ApiResult<Vec<Value>> {
    if let Ok(id) = parse_uuid_ref(reference, &[]) {
        let rows = sqlx::query(
            r#"
            SELECT rk.record_kind
            FROM straylight.record_keys rk
            JOIN straylight.corpus_members cm
              ON cm.user_id = rk.user_id AND cm.record_id = rk.record_id
            WHERE rk.user_id = $1 AND rk.scope_id = $2 AND rk.record_id = $3
              AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(id)
        .bind(session.corpus_revision_id)
        .fetch_all(&mut **tx)
        .await?;
        let mut result = Vec::new();
        for row in rows {
            let kind: String = row.try_get("record_kind")?;
            result.push(record_descriptor_tx(tx, session, id, &kind).await?);
        }
        if !result.is_empty() {
            return Ok(result);
        }
    }

    let rows = sqlx::query(
        r#"
        SELECT DISTINCT candidate.record_id, candidate.record_kind
        FROM (
          SELECT h.object_id AS record_id, 'object'::text AS record_kind
          FROM straylight.object_handles h
          WHERE h.user_id = $1 AND h.scope_id = $2 AND h.valid_to IS NULL
            AND lower(h.handle) = lower($4)
          UNION ALL
          SELECT se.id, 'source_episode'::text
          FROM straylight.source_episodes se
          WHERE se.user_id = $1 AND se.scope_id = $2
            AND lower(se.source_ref) = lower($4)
          UNION ALL
          SELECT orev.object_id, 'object'::text
          FROM straylight.object_revisions orev
          JOIN straylight.corpus_members cm0
            ON cm0.user_id = orev.user_id AND cm0.record_id = orev.object_id
           AND cm0.record_version = orev.version
          WHERE orev.user_id = $1 AND cm0.scope_id = $2
            AND cm0.corpus_revision_id = $3 AND cm0.disposition = 'active'
            AND lower(orev.label) = lower($4)
        ) candidate
        JOIN straylight.corpus_members cm
          ON cm.user_id = $1 AND cm.record_id = candidate.record_id
         AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
        LIMIT 25
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(reference)
    .fetch_all(&mut **tx)
    .await?;
    let mut result = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("record_id")?;
        let kind: String = row.try_get("record_kind")?;
        result.push(record_descriptor_tx(tx, session, id, &kind).await?);
    }
    Ok(result)
}

async fn checkpoint_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    checkpoint_id: Uuid,
) -> ApiResult<Value> {
    let base: Value = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'checkpoint_ref', 'checkpoint:' || replace(cp.id::text, '-', ''),
          'checkpoint_id', cp.id::text,
          'parent_checkpoint_ref', CASE WHEN cp.parent_checkpoint_id IS NULL THEN NULL
            ELSE 'checkpoint:' || replace(cp.parent_checkpoint_id::text, '-', '') END,
          'corpus_revision', 'revision:' || replace(cp.corpus_revision_id::text, '-', ''),
          'corpus_revision_id', cp.corpus_revision_id::text,
          'title', cp.title, 'summary', cp.summary, 'created_at', cp.created_at
        )
        FROM straylight.checkpoints cp
        WHERE cp.user_id = $1 AND cp.scope_id = $2 AND cp.id = $3
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(checkpoint_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("checkpoint_not_found", &checkpoint_id.to_string()))?;

    let goals = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'goal', goal_text, 'work_item_ref', CASE WHEN work_item_object_id IS NULL THEN NULL ELSE 'object:' || replace(work_item_object_id::text, '-', '') END, 'state_claim_ref', CASE WHEN state_claim_id IS NULL THEN NULL ELSE 'claim:' || replace(state_claim_id::text, '-', '') END) FROM straylight.checkpoint_goals WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let states = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'state_claim_ref', 'claim:' || replace(state_claim_id::text, '-', '')) FROM straylight.checkpoint_state_refs WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let gaps = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'kind', gap_kind, 'description', description, 'related_ref', CASE WHEN related_record_id IS NULL THEN NULL ELSE replace(related_record_id::text, '-', '') END) FROM straylight.checkpoint_gaps WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let decisions = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'kind', decision_kind, 'claim_ref', 'claim:' || replace(claim_id::text, '-', '')) FROM straylight.checkpoint_decisions WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let gates = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'description', description, 'gate_ref', CASE WHEN gate_object_id IS NULL THEN NULL ELSE 'object:' || replace(gate_object_id::text, '-', '') END, 'state_claim_ref', CASE WHEN state_claim_id IS NULL THEN NULL ELSE 'claim:' || replace(state_claim_id::text, '-', '') END) FROM straylight.checkpoint_gates WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let actions = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'action', action_text, 'responsible_ref', CASE WHEN responsible_record_id IS NULL THEN NULL ELSE replace(responsible_record_id::text, '-', '') END, 'due_temporal_ref', CASE WHEN due_temporal_id IS NULL THEN NULL ELSE 'temporal_spec:' || replace(due_temporal_id::text, '-', '') END, 'preconditions', preconditions) FROM straylight.checkpoint_actions WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let sources = checkpoint_component_tx(
        tx,
        "SELECT jsonb_build_object('position', position, 'source_record_id', source_record_id::text, 'source_version', source_version, 'locator', locator) FROM straylight.checkpoint_source_refs WHERE user_id = $1 AND checkpoint_id = $2 ORDER BY position",
        session.user_id,
        checkpoint_id,
    ).await?;
    let mut object = base.as_object().cloned().unwrap_or_default();
    object.insert("ordered_goals".to_owned(), Value::Array(goals));
    object.insert("state_refs".to_owned(), Value::Array(states));
    object.insert("gaps".to_owned(), Value::Array(gaps));
    object.insert("decisions".to_owned(), Value::Array(decisions));
    object.insert("acceptance_gates".to_owned(), Value::Array(gates));
    object.insert("next_actions".to_owned(), Value::Array(actions));
    object.insert("source_refs".to_owned(), Value::Array(sources));
    Ok(Value::Object(object))
}

async fn checkpoint_component_tx(
    tx: &mut Transaction<'_, Postgres>,
    sql: &'static str,
    user_id: Uuid,
    checkpoint_id: Uuid,
) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query_scalar(sql)
        .bind(user_id)
        .bind(checkpoint_id)
        .fetch_all(&mut **tx)
        .await?)
}

async fn revision_delta_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    from_revision: Uuid,
) -> ApiResult<Value> {
    if from_revision == session.corpus_revision_id {
        return Ok(json!({
            "from_revision": record_ref("revision", from_revision),
            "to_revision": session.corpus_revision_ref,
            "added": [],
            "removed": [],
            "changed": [],
            "source_changes": [],
            "source_change_summary": {
                "total": 0, "returned": 0, "content_complete": true
            },
            "self_contained": true,
            "incomplete_reasons": []
        }));
    }
    let rows = sqlx::query(
        r#"
        WITH before_rows AS (
          SELECT record_id, record_kind::text, record_version, disposition
          FROM straylight.corpus_members
          WHERE user_id = $1 AND scope_id = $2 AND corpus_revision_id = $3
        ), after_rows AS (
          SELECT record_id, record_kind::text, record_version, disposition
          FROM straylight.corpus_members
          WHERE user_id = $1 AND scope_id = $2 AND corpus_revision_id = $4
        )
        SELECT coalesce(a.record_id, b.record_id) AS record_id,
               coalesce(a.record_kind, b.record_kind) AS record_kind,
               b.record_version AS before_version, a.record_version AS after_version,
               b.disposition AS before_disposition, a.disposition AS after_disposition
        FROM after_rows a FULL OUTER JOIN before_rows b USING (record_id)
        WHERE a.record_id IS NULL OR b.record_id IS NULL
           OR a.record_version IS DISTINCT FROM b.record_version
           OR a.disposition IS DISTINCT FROM b.disposition
        ORDER BY record_kind, record_id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(from_revision)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut document_changes = 0usize;
    let mut record_kinds_requiring_read = BTreeSet::new();
    for row in rows {
        let id: Uuid = row.try_get("record_id")?;
        let kind: String = row.try_get("record_kind")?;
        let before: Option<i32> = row.try_get("before_version")?;
        let after: Option<i32> = row.try_get("after_version")?;
        let before_disposition: Option<String> = row.try_get("before_disposition")?;
        let after_disposition: Option<String> = row.try_get("after_disposition")?;
        let item = json!({
            "ref": record_ref(&kind, id),
            "before_version": before,
            "after_version": after,
            "before_disposition": before_disposition,
            "after_disposition": after_disposition
        });
        if kind == "document" {
            document_changes += 1;
        } else if !matches!(kind.as_str(), "chunk" | "evidence" | "source_episode") {
            record_kinds_requiring_read.insert(kind.clone());
        }
        if before.is_none() {
            added.push(item);
        } else if after.is_none() {
            removed.push(item);
        } else {
            changed.push(item);
        }
    }
    let source_changes = revision_source_changes_tx(tx, session, from_revision).await?;
    let mut incomplete_reasons = record_kinds_requiring_read
        .into_iter()
        .map(|kind| format!("{kind}_change_requires_read"))
        .collect::<Vec<_>>();
    if source_changes.total != document_changes {
        incomplete_reasons.push("one_or_more_document_changes_lack_source_content".to_owned());
    }
    if !source_changes.complete {
        incomplete_reasons.push("revision_source_content_was_bounded".to_owned());
    }
    let self_contained = document_changes > 0 && incomplete_reasons.is_empty();
    Ok(json!({
        "from_revision": record_ref("revision", from_revision),
        "to_revision": session.corpus_revision_ref,
        "added": added,
        "removed": removed,
        "changed": changed,
        "source_changes": source_changes.items,
        "source_change_summary": {
            "total": source_changes.total,
            "returned": source_changes.returned,
            "content_complete": source_changes.complete
        },
        "self_contained": self_contained,
        "incomplete_reasons": incomplete_reasons
    }))
}

fn continuation_is_self_contained(
    checkpoint: Option<&Value>,
    revision_delta: Option<&Value>,
) -> bool {
    checkpoint.is_some()
        && revision_delta
            .and_then(|value| value.get("self_contained"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

struct RevisionSourceChanges {
    items: Vec<Value>,
    total: usize,
    returned: usize,
    complete: bool,
}

async fn revision_source_changes_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    from_revision: Uuid,
) -> ApiResult<RevisionSourceChanges> {
    let rows = sqlx::query(
        r#"
        WITH before_docs AS (
          SELECT record_id AS document_id, record_version, disposition::text
          FROM straylight.corpus_members
          WHERE user_id = $1 AND scope_id = $2 AND corpus_revision_id = $3
            AND record_kind::text = 'document'
        ), after_docs AS (
          SELECT record_id AS document_id, record_version, disposition::text
          FROM straylight.corpus_members
          WHERE user_id = $1 AND scope_id = $2 AND corpus_revision_id = $4
            AND record_kind::text = 'document'
        ), changed_docs AS (
          SELECT coalesce(a.document_id, b.document_id) AS document_id,
                 b.record_version AS before_version,
                 a.record_version AS after_version,
                 b.disposition AS before_disposition,
                 a.disposition AS after_disposition
          FROM after_docs a FULL OUTER JOIN before_docs b USING (document_id)
          WHERE a.document_id IS NULL OR b.document_id IS NULL
             OR a.record_version IS DISTINCT FROM b.record_version
             OR a.disposition IS DISTINCT FROM b.disposition
        )
        SELECT changed_docs.document_id, changed_docs.before_version,
               changed_docs.after_version, changed_docs.before_disposition,
               changed_docs.after_disposition, dr.title, dr.media_type,
               dr.content_hash, dr.native_locator, se.source_ref,
               se.source_version,
               left(coalesce(string_agg(c.content, E'\n\n' ORDER BY c.ordinal), ''), $6) AS content,
               char_length(coalesce(string_agg(c.content, E'\n\n' ORDER BY c.ordinal), ''))::bigint
                 AS content_characters,
               count(*) OVER()::bigint AS total_count
        FROM changed_docs
        JOIN straylight.document_revisions dr
          ON dr.user_id = $1 AND dr.document_id = changed_docs.document_id
         AND dr.version = coalesce(changed_docs.after_version, changed_docs.before_version)
        JOIN straylight.source_episodes se
          ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        LEFT JOIN straylight.chunks c
          ON c.user_id = dr.user_id AND c.document_id = dr.document_id
         AND c.document_version = dr.version
        GROUP BY changed_docs.document_id, changed_docs.before_version,
                 changed_docs.after_version, changed_docs.before_disposition,
                 changed_docs.after_disposition, dr.title, dr.media_type,
                 dr.content_hash, dr.native_locator, se.source_ref,
                 se.source_version
        ORDER BY se.source_ref, changed_docs.document_id
        LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(from_revision)
    .bind(session.corpus_revision_id)
    .bind(i64::try_from(MAX_REVISION_SOURCE_CHANGES + 1).unwrap_or(i64::MAX))
    .bind(i32::try_from(MAX_REVISION_SOURCE_CONTENT_CHARS).unwrap_or(i32::MAX))
    .fetch_all(&mut **tx)
    .await?;

    let total = rows
        .first()
        .map(|row| row.try_get::<i64, _>("total_count"))
        .transpose()?
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_default();
    let mut complete = total <= MAX_REVISION_SOURCE_CHANGES;
    let mut items = Vec::with_capacity(rows.len().min(MAX_REVISION_SOURCE_CHANGES));
    for row in rows.into_iter().take(MAX_REVISION_SOURCE_CHANGES) {
        let before_version: Option<i32> = row.try_get("before_version")?;
        let after_version: Option<i32> = row.try_get("after_version")?;
        let content_characters =
            usize::try_from(row.try_get::<i64, _>("content_characters")?).unwrap_or(usize::MAX);
        let content_truncated = content_characters > MAX_REVISION_SOURCE_CONTENT_CHARS;
        complete &= !content_truncated;
        let change = if before_version.is_none() {
            "added"
        } else if after_version.is_none() {
            "removed"
        } else {
            "changed"
        };
        let document_id: Uuid = row.try_get("document_id")?;
        items.push(json!({
            "change": change,
            "document_ref": record_ref("document", document_id),
            "before_version": before_version,
            "after_version": after_version,
            "before_disposition": row.try_get::<Option<String>, _>("before_disposition")?,
            "after_disposition": row.try_get::<Option<String>, _>("after_disposition")?,
            "source_ref": row.try_get::<String, _>("source_ref")?,
            "source_version": row.try_get::<Option<String>, _>("source_version")?,
            "title": row.try_get::<Option<String>, _>("title")?,
            "media_type": row.try_get::<String, _>("media_type")?,
            "content_hash": prefixed_hash(row.try_get::<String, _>("content_hash")?),
            "native_locator": row.try_get::<Value, _>("native_locator")?,
            "content": row.try_get::<String, _>("content")?,
            "content_characters": content_characters,
            "content_truncated": content_truncated
        }));
    }
    Ok(RevisionSourceChanges {
        returned: items.len(),
        items,
        total,
        complete,
    })
}

async fn scope_token_count_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    token_budget: usize,
) -> ApiResult<usize> {
    let (_, chunk_tokens) = manifest_stats_tx(tx, session).await?;
    let mut count = chunk_tokens;
    if count > token_budget {
        return Ok(count);
    }

    let rows = sqlx::query(
        r#"
        SELECT rk.record_id, rk.record_kind, cm.record_version
        FROM straylight.corpus_members cm
        JOIN straylight.record_keys rk
          ON rk.user_id = cm.user_id AND rk.record_id = cm.record_id
         AND rk.record_kind = cm.record_kind
        WHERE cm.user_id = $1 AND cm.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND rk.record_kind <> 'chunk'
        ORDER BY rk.record_kind, rk.record_id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let record = ResolvedRecord {
            id: row.try_get("record_id")?,
            kind: row.try_get("record_kind")?,
            version: row.try_get("record_version")?,
        };
        let value = structured_record_tx(tx, session, &record, None).await?;
        count = count.saturating_add(estimate_tokens(&serde_json::to_string(&value)?));
        if count > token_budget {
            break;
        }
    }
    Ok(count)
}

async fn corpus_record_count_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
) -> ApiResult<usize> {
    Ok(manifest_stats_tx(tx, session).await?.0)
}

async fn manifest_stats_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
) -> ApiResult<(usize, usize)> {
    let row = sqlx::query(
        "SELECT record_count, chunk_tokens FROM straylight_auth.read_manifest_stats($1, $2, $3)",
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::capability("read"))?;
    Ok((
        usize::try_from(row.try_get::<i64, _>("record_count")?).unwrap_or(usize::MAX),
        usize::try_from(row.try_get::<i64, _>("chunk_tokens")?).unwrap_or(usize::MAX),
    ))
}

async fn materialize_scope_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    token_budget: usize,
) -> ApiResult<Value> {
    let family_rows = sqlx::query(
        r#"
        SELECT record_kind, count(*)::bigint AS record_count
        FROM straylight.corpus_members
        WHERE user_id = $1 AND scope_id = $2
          AND corpus_revision_id = $3 AND disposition = 'active'
        GROUP BY record_kind ORDER BY record_kind
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut available = BTreeMap::<String, usize>::new();
    for row in family_rows {
        available.insert(
            row.try_get("record_kind")?,
            usize::try_from(row.try_get::<i64, _>("record_count")?).unwrap_or(usize::MAX),
        );
    }
    let mut returned = BTreeMap::<String, usize>::new();
    let mut used = 0usize;

    let document_rows = sqlx::query(
        r#"
        SELECT d.id AS document_id, dr.version, dr.title, dr.media_type,
               dr.content_hash, dr.native_locator, se.source_ref, se.source_version
        FROM straylight.documents d
        JOIN straylight.corpus_members dm
          ON dm.user_id = d.user_id AND dm.record_id = d.id
        JOIN straylight.document_revisions dr
          ON dr.user_id = d.user_id AND dr.document_id = d.id
         AND dr.version = dm.record_version
        JOIN straylight.source_episodes se
          ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        WHERE d.user_id = $1 AND d.scope_id = $2
          AND dm.corpus_revision_id = $3 AND dm.disposition = 'active'
        ORDER BY se.source_ref, d.id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut documents = BTreeMap::<Uuid, Value>::new();
    for row in document_rows {
        let id: Uuid = row.try_get("document_id")?;
        let document = json!({
            "document_ref": record_ref("document", id),
            "version": row.try_get::<i32, _>("version")?,
            "title": row.try_get::<Option<String>, _>("title")?,
            "media_type": row.try_get::<String, _>("media_type")?,
            "content_hash": prefixed_hash(row.try_get::<String, _>("content_hash")?),
            "native_locator": row.try_get::<Value, _>("native_locator")?,
            "source_ref": row.try_get::<String, _>("source_ref")?,
            "source_version": row.try_get::<Option<String>, _>("source_version")?,
            "chunks": []
        });
        let tokens = estimate_tokens(&serde_json::to_string(&document)?);
        if used.saturating_add(tokens) <= token_budget {
            used += tokens;
            documents.insert(id, document);
            *returned.entry("document".to_owned()).or_default() += 1;
        }
    }

    let chunk_rows = sqlx::query(
        r#"
        SELECT d.id AS document_id, c.ordinal, c.heading_path,
               c.locator, c.content, c.token_count
        FROM straylight.documents d
        JOIN straylight.corpus_members dm
          ON dm.user_id = d.user_id AND dm.record_id = d.id
        JOIN straylight.document_revisions dr
          ON dr.user_id = d.user_id AND dr.document_id = d.id
         AND dr.version = dm.record_version
        JOIN straylight.source_episodes se
          ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        JOIN straylight.chunks c
          ON c.user_id = dr.user_id AND c.document_id = dr.document_id
         AND c.document_version = dr.version
        JOIN straylight.corpus_members cm
          ON cm.user_id = c.user_id AND cm.record_id = c.id
        WHERE d.user_id = $1 AND d.scope_id = $2
          AND dm.corpus_revision_id = $3 AND dm.disposition = 'active'
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
        ORDER BY se.source_ref, d.id, c.ordinal
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in chunk_rows {
        let tokens = usize::try_from(row.try_get::<i32, _>("token_count")?).unwrap_or_default();
        let id: Uuid = row.try_get("document_id")?;
        if let Some(entry) = documents.get_mut(&id)
            && used.saturating_add(tokens) <= token_budget
        {
            used += tokens;
            entry["chunks"]
                .as_array_mut()
                .expect("chunks array")
                .push(json!({
                    "ordinal": row.try_get::<i32, _>("ordinal")?,
                    "heading_path": row.try_get::<Vec<String>, _>("heading_path")?,
                    "locator": row.try_get::<Value, _>("locator")?,
                    "content": row.try_get::<String, _>("content")?
                }));
            *returned.entry("chunk".to_owned()).or_default() += 1;
        }
    }

    let generic_rows = sqlx::query(
        r#"
        SELECT rk.record_id, rk.record_kind, cm.record_version
        FROM straylight.corpus_members cm
        JOIN straylight.record_keys rk
          ON rk.user_id = cm.user_id AND rk.record_id = cm.record_id
         AND rk.record_kind = cm.record_kind
        WHERE cm.user_id = $1 AND cm.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND rk.record_kind NOT IN ('document', 'chunk')
        ORDER BY rk.record_kind, rk.record_id
        LIMIT $4
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(i64::try_from(MAX_MATERIALIZED_GENERIC_RECORDS + 1).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    let mut records = Vec::new();
    for row in generic_rows
        .into_iter()
        .take(MAX_MATERIALIZED_GENERIC_RECORDS)
    {
        let record = ResolvedRecord {
            id: row.try_get("record_id")?,
            kind: row.try_get("record_kind")?,
            version: row.try_get("record_version")?,
        };
        let value = structured_record_tx(tx, session, &record, None).await?;
        let tokens = estimate_tokens(&serde_json::to_string(&value)?);
        if used.saturating_add(tokens) <= token_budget {
            used += tokens;
            *returned.entry(record.kind.clone()).or_default() += 1;
            records.push(value);
        }
    }
    let omitted = materialization_omissions(&available, &returned);
    Ok(json!({
        "scope_ref": session.scope_ref,
        "corpus_revision": session.corpus_revision_ref,
        "documents": documents.into_values().collect::<Vec<_>>(),
        "records": records,
        "family_counts": {
            "available": available,
            "returned": returned,
            "omitted": omitted
        },
        "limits": {
            "token_budget": token_budget,
            "generic_records": MAX_MATERIALIZED_GENERIC_RECORDS
        },
        "estimated_tokens": used,
        "complete": omitted.is_empty()
    }))
}

fn materialization_omissions(
    available: &BTreeMap<String, usize>,
    returned: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    available
        .iter()
        .filter_map(|(kind, count)| {
            let omitted = count.saturating_sub(returned.get(kind).copied().unwrap_or_default());
            (omitted > 0).then(|| (kind.clone(), omitted))
        })
        .collect()
}

fn effective_modes(spec: &QuerySpec) -> Vec<RetrievalMode> {
    if spec.modes.is_empty() {
        [
            RetrievalMode::Exact,
            RetrievalMode::Structured,
            RetrievalMode::Lexical,
            RetrievalMode::Semantic,
            RetrievalMode::Temporal,
            RetrievalMode::Relations,
        ]
        .into_iter()
        .collect()
    } else {
        spec.modes.clone()
    }
}

fn mode_name(mode: RetrievalMode) -> &'static str {
    match mode {
        RetrievalMode::Exact => "exact",
        RetrievalMode::Structured => "structured",
        RetrievalMode::Lexical => "lexical",
        RetrievalMode::Semantic => "semantic",
        RetrievalMode::Temporal => "temporal",
        RetrievalMode::Relations => "relations",
    }
}

async fn exact_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let query = spec.query.as_deref().unwrap_or("").trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let parsed_id = parse_uuid_ref(query, &[]).ok();
    let limit = lane_limit(spec.limit);
    let rows = sqlx::query(
        r#"
        SELECT c.id, se.source_ref, dr.native_locator ->> 'path' AS path,
               array_to_string(c.heading_path, ' / ') AS heading, c.content,
               c.content_hash, se.source_version, se.source_kind,
               se.metadata AS source_metadata,
               (
                 SELECT jsonb_build_object(
                   'asset_ref','asset:' || described.asset_id::text,
                   'version',described.asset_version,
                   'path',se.metadata->>'original_path'
                 )
                 FROM straylight.source_asset_links AS described
                 WHERE described.user_id=se.user_id
                   AND described.source_episode_id=se.id
                   AND described.role='describes'
                 ORDER BY described.asset_id,described.asset_version
                 LIMIT 1
               ) AS described_asset,
               dr.recorded_at, c.locator,
               ARRAY(
                 SELECT 'evidence:' || replace(ei.id::text, '-', '')
                 FROM straylight.evidence_items ei
                 JOIN straylight.corpus_members ecm
                   ON ecm.user_id = ei.user_id AND ecm.record_id = ei.id
                 WHERE ei.user_id = c.user_id AND ei.scope_id = c.scope_id
                   AND ei.source_episode_id = dr.source_episode_id
                   AND ei.evidence_kind = 'source_native'
                   AND ecm.corpus_revision_id = $3 AND ecm.disposition = 'active'
                 ORDER BY ei.created_at, ei.id LIMIT 32
               ) AS evidence_refs,
               (CASE WHEN c.id = $5 THEN 4.0
                    WHEN lower(c.content_hash) = lower(regexp_replace($4, '^sha256:', '')) THEN 3.0
                    WHEN lower(coalesce(dr.title, '')) = lower($4) THEN 2.0
                    WHEN lower(se.source_ref) = lower($4) THEN 2.0
                    ELSE 1.0 END)::float8 AS score
        FROM straylight.chunks c
        JOIN straylight.corpus_members cm
          ON cm.user_id = c.user_id AND cm.record_id = c.id
        JOIN straylight.document_revisions dr
          ON dr.user_id = c.user_id AND dr.document_id = c.document_id
         AND dr.version = c.document_version
        JOIN straylight.source_episodes se
          ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        WHERE c.user_id = $1 AND c.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND (c.id = $5 OR lower(c.content_hash) = lower(regexp_replace($4, '^sha256:', ''))
            OR lower(coalesce(dr.title, '')) = lower($4)
            OR lower(se.source_ref) = lower($4)
            OR lower(c.content) LIKE lower($8) ESCAPE '\')
          AND (cardinality($7::uuid[]) = 0 OR c.id = ANY($7::uuid[])
            OR c.document_id = ANY($7::uuid[]) OR dr.source_episode_id = ANY($7::uuid[]))
        ORDER BY score DESC, se.source_ref, c.ordinal
        LIMIT $6
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(query)
    .bind(parsed_id)
    .bind(limit)
    .bind(root_ids)
    .bind(sql_like_literal_contains(query))
    .fetch_all(&mut **tx)
    .await?;
    let mut candidates = chunk_rows(rows, "exact")?;

    let object_rows = sqlx::query(
        r#"
        SELECT o.id, orev.label, orev.properties, orev.recorded_at,
               se.source_ref, se.source_version,
               (CASE WHEN o.id = $5 THEN 4.0
                    WHEN lower(coalesce(orev.label, '')) = lower($4) THEN 3.0
                    ELSE 1.0 END)::float8 AS score
        FROM straylight.objects o
        JOIN straylight.corpus_members cm
          ON cm.user_id = o.user_id AND cm.record_id = o.id
        JOIN straylight.object_revisions orev
          ON orev.user_id = o.user_id AND orev.object_id = o.id
         AND orev.version = cm.record_version
        JOIN straylight.source_episodes se
          ON se.user_id = orev.user_id AND se.id = orev.source_episode_id
        WHERE o.user_id = $1 AND o.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND (o.id = $5 OR lower(coalesce(orev.label, '')) = lower($4)
            OR position(lower($4) in lower(orev.properties::text)) > 0
            OR EXISTS (SELECT 1 FROM straylight.object_handles h
                WHERE h.user_id = o.user_id AND h.object_id = o.id
                  AND h.valid_to IS NULL AND lower(h.handle) = lower($4)))
          AND (cardinality($7::uuid[]) = 0 OR o.id = ANY($7::uuid[]))
        ORDER BY score DESC, orev.label NULLS LAST, o.id LIMIT $6
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(query)
    .bind(parsed_id)
    .bind(limit)
    .bind(root_ids)
    .fetch_all(&mut **tx)
    .await?;
    for row in object_rows {
        let id: Uuid = row.try_get("id")?;
        let properties: Value = row.try_get("properties")?;
        candidates.push(RetrievalCandidate {
            reference: record_ref("object", id),
            source_ref: row.try_get("source_ref")?,
            path: None,
            heading: row.try_get("label")?,
            content: serde_json::to_string(&properties)?,
            content_hash: sha256_prefixed_json(&properties)?,
            source_version: row.try_get("source_version")?,
            authority: None,
            canonicality: Some("canonical_at_revision".to_owned()),
            provenance: None,
            recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
            valid_time: None,
            evidence_refs: Vec::new(),
            why_selected: vec!["exact_identifier_match".to_owned()],
            lane_scores: HashMap::new(),
            score: row.try_get("score")?,
        });
    }
    Ok(candidates)
}

async fn lexical_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let query = spec.query.as_deref().unwrap_or("");
    let discovery_terms = lexical_discovery_terms(query);
    let discovery_query = lexical_discovery_query(query);
    let prefer_source_start = source_start_intent(query);
    let rows = sqlx::query(
        r#"
        WITH requested AS (
          SELECT websearch_to_tsquery('english', $4) AS terms,
                 $7::text[] AS raw_terms
        ), matching_sources AS MATERIALIZED (
          SELECT se.id
          FROM straylight.source_episodes se
          CROSS JOIN requested
          WHERE se.user_id = $1 AND se.scope_id = $2
            AND to_tsvector(
              'english',
              straylight.lexical_source_text(se.source_ref)
            ) @@ requested.terms
        ), candidate_chunks AS MATERIALIZED (
          SELECT c.id
          FROM straylight.chunks c
          CROSS JOIN requested
          WHERE c.user_id = $1 AND c.scope_id = $2
            AND c.search_vector @@ requested.terms
          UNION
          SELECT c.id
          FROM matching_sources source
          JOIN straylight.document_revisions dr
            ON dr.user_id = $1 AND dr.source_episode_id = source.id
          JOIN straylight.chunks c
            ON c.user_id = dr.user_id AND c.document_id = dr.document_id
           AND c.document_version = dr.version
        )
        SELECT c.id, se.source_ref, dr.native_locator ->> 'path' AS path,
               array_to_string(c.heading_path, ' / ') AS heading, c.content,
               c.content_hash, se.source_version, se.source_kind,
               se.metadata AS source_metadata,
               (
                 SELECT jsonb_build_object(
                   'asset_ref','asset:' || described.asset_id::text,
                   'version',described.asset_version,
                   'path',se.metadata->>'original_path'
                 )
                 FROM straylight.source_asset_links AS described
                 WHERE described.user_id=se.user_id
                   AND described.source_episode_id=se.id
                   AND described.role='describes'
                 ORDER BY described.asset_id,described.asset_version
                 LIMIT 1
               ) AS described_asset,
               dr.recorded_at, c.locator,
               ARRAY(
                 SELECT 'evidence:' || replace(ei.id::text, '-', '')
                 FROM straylight.evidence_items ei
                 JOIN straylight.corpus_members ecm
                   ON ecm.user_id = ei.user_id AND ecm.record_id = ei.id
                 WHERE ei.user_id = c.user_id AND ei.scope_id = c.scope_id
                   AND ei.source_episode_id = dr.source_episode_id
                   AND ei.evidence_kind = 'source_native'
                   AND ecm.corpus_revision_id = $3 AND ecm.disposition = 'active'
                 ORDER BY ei.created_at, ei.id LIMIT 32
               ) AS evidence_refs,
               coverage.term_coverage::float8
                 + 3.0 * coverage.path_coverage::float8
                 + ts_rank_cd(
                 setweight(to_tsvector('english', straylight.lexical_source_text(
                   se.source_ref
                 )), 'A') || c.search_vector,
                 requested.terms,
                 1
               )::float8 AS score,
               coverage.term_coverage + 3 * coverage.path_coverage AS term_coverage
        FROM candidate_chunks candidate
        JOIN straylight.chunks c
          ON c.user_id = $1 AND c.id = candidate.id
        JOIN straylight.corpus_members cm
          ON cm.user_id = c.user_id AND cm.record_id = c.id
        JOIN straylight.document_revisions dr
          ON dr.user_id = c.user_id AND dr.document_id = c.document_id
         AND dr.version = c.document_version
        JOIN straylight.source_episodes se
          ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        CROSS JOIN requested
        CROSS JOIN LATERAL (
          SELECT
            count(*) FILTER (WHERE (
              setweight(to_tsvector('english', straylight.lexical_source_text(
                se.source_ref
              )), 'A') || c.search_vector
            ) @@ plainto_tsquery('english', term.value)) AS term_coverage,
            count(*) FILTER (WHERE
              to_tsvector('english', straylight.lexical_source_text(se.source_ref))
                @@ plainto_tsquery('english', term.value)
            ) AS path_coverage
          FROM unnest(requested.raw_terms) AS term(value)
        ) coverage
        WHERE c.user_id = $1 AND c.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND (
            setweight(to_tsvector('english', straylight.lexical_source_text(
              se.source_ref
            )), 'A') || c.search_vector
          ) @@ requested.terms
          AND (cardinality($6::uuid[]) = 0 OR c.id = ANY($6::uuid[])
            OR c.document_id = ANY($6::uuid[]) OR dr.source_episode_id = ANY($6::uuid[]))
        ORDER BY term_coverage DESC,
                 CASE WHEN $8 THEN c.ordinal ELSE 0 END,
                 score DESC, se.source_ref, c.ordinal
        LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(discovery_query)
    .bind(lane_limit(spec.limit))
    .bind(root_ids)
    .bind(discovery_terms)
    .bind(prefer_source_start)
    .fetch_all(&mut **tx)
    .await?;
    chunk_rows(rows, "lexical")
}

fn lexical_discovery_query(query: &str) -> String {
    let terms = lexical_discovery_terms(query);
    if terms.is_empty() {
        query.to_owned()
    } else {
        terms
            .into_iter()
            .map(|term| format!("\"{term}\""))
            .collect::<Vec<_>>()
            .join(" OR ")
    }
}

fn sql_like_literal_contains(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

fn lexical_discovery_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let normalized = split_identifier_words(query);
    for raw in normalized.split(|character: char| !character.is_alphanumeric()) {
        let term = raw.to_lowercase();
        if term.len() < 2 || lexical_stopword(&term) || !seen.insert(term.clone()) {
            continue;
        }
        terms.push(term.clone());
        if matches!(term.as_str(), "latest" | "newest" | "current") {
            for synonym in ["latest", "newest", "current"] {
                if seen.insert(synonym.to_owned()) {
                    terms.push(synonym.to_owned());
                }
            }
        }
        if terms.len() >= 24 {
            break;
        }
    }
    terms
}

fn split_identifier_words(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    let mut normalized = String::with_capacity(value.len() + 8);
    for (index, character) in characters.iter().copied().enumerate() {
        if index > 0
            && character.is_ascii_uppercase()
            && (characters[index - 1].is_ascii_lowercase()
                || characters[index - 1].is_ascii_digit()
                || (characters[index - 1].is_ascii_uppercase()
                    && characters
                        .get(index + 1)
                        .is_some_and(|next| next.is_ascii_lowercase())))
        {
            normalized.push(' ');
        }
        normalized.push(character);
    }
    normalized
}

fn lexical_stopword(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "by"
            | "for"
            | "from"
            | "how"
            | "in"
            | "is"
            | "it"
            | "of"
            | "on"
            | "or"
            | "that"
            | "the"
            | "this"
            | "to"
            | "was"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "with"
    )
}

fn source_start_intent(query: &str) -> bool {
    query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .any(|term| {
            matches!(
                term.as_str(),
                "beginning" | "first" | "header" | "largest" | "top"
            )
        })
}

async fn semantic_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    vector: &[f32],
    embedding_model: &str,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let rows = sqlx::query(
        r#"
        WITH ranked AS MATERIALIZED (
          SELECT e.user_id, e.scope_id, e.target_record_id,
                 e.source_content_hash, rk.record_kind, cm.record_version,
                 (1.0 - (e.embedding <=> $4))::float8 AS score
          FROM straylight.embeddings e
          JOIN straylight.record_keys rk
            ON rk.user_id = e.user_id AND rk.record_id = e.target_record_id
          JOIN straylight.corpus_members cm
            ON cm.user_id = e.user_id AND cm.record_id = e.target_record_id
          WHERE e.user_id = $1 AND e.scope_id = $2
            AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
            AND e.model = $5
            AND (cardinality($7::uuid[]) = 0
              OR e.target_record_id = ANY($7::uuid[])
              OR EXISTS (
                SELECT 1 FROM straylight.chunks rooted_chunk
                WHERE rooted_chunk.user_id = e.user_id
                  AND rooted_chunk.id = e.target_record_id
                  AND rooted_chunk.document_id = ANY($7::uuid[])
              )
              OR EXISTS (
                SELECT 1 FROM straylight.evidence_items rooted_evidence
                WHERE rooted_evidence.user_id = e.user_id
                  AND rooted_evidence.id = e.target_record_id
                  AND rooted_evidence.source_episode_id = ANY($7::uuid[])
              )
              OR EXISTS (
                SELECT 1 FROM straylight.claim_about_targets cat
                WHERE cat.user_id = e.user_id AND cat.claim_id = e.target_record_id
                  AND cat.target_record_id = ANY($7::uuid[])
              ))
          ORDER BY e.embedding <=> $4, e.target_record_id
          LIMIT $6
        )
        SELECT e.target_record_id AS id, e.record_kind,
               coalesce(se_chunk.source_ref, se_object.source_ref, se_evidence.source_ref,
                        se_claim.source_ref, 'record:' || e.record_kind) AS source_ref,
               dr.native_locator ->> 'path' AS path,
               coalesce(array_to_string(c.heading_path, ' / '), orev.label, cl.predicate::text,
                        e.record_kind) AS heading,
               coalesce(c.content, ev.text_content, orev.properties::text, cl.value::text, '') AS content,
               coalesce(c.content_hash, ev.content_hash, e.source_content_hash) AS content_hash,
               coalesce(se_chunk.source_version, se_object.source_version,
                        se_evidence.source_version, se_claim.source_version) AS source_version,
               se_chunk.source_kind,se_chunk.metadata AS source_metadata,
               (
                 SELECT jsonb_build_object(
                   'asset_ref','asset:' || described.asset_id::text,
                   'version',described.asset_version,
                   'path',se_chunk.metadata->>'original_path'
                 )
                 FROM straylight.source_asset_links AS described
                 WHERE described.user_id=se_chunk.user_id
                   AND described.source_episode_id=se_chunk.id
                   AND described.role='describes'
                 ORDER BY described.asset_id,described.asset_version
                 LIMIT 1
               ) AS described_asset,
               cl.authority, cl.canonicality,
               coalesce(cl.recorded_at, orev.recorded_at, ev.created_at, c.created_at) AS recorded_at,
               CASE WHEN e.record_kind = 'chunk' THEN ARRAY(
                 SELECT 'evidence:' || replace(ei.id::text, '-', '')
                 FROM straylight.evidence_items ei
                 JOIN straylight.corpus_members ecm
                   ON ecm.user_id = ei.user_id AND ecm.record_id = ei.id
                 WHERE ei.user_id = c.user_id AND ei.scope_id = c.scope_id
                   AND ei.source_episode_id = dr.source_episode_id
                   AND ei.evidence_kind = 'source_native'
                   AND ecm.corpus_revision_id = $3 AND ecm.disposition = 'active'
                 ORDER BY ei.created_at, ei.id LIMIT 32
               ) ELSE ARRAY[]::text[] END AS evidence_refs,
               e.score
        FROM ranked e
        LEFT JOIN straylight.chunks c
          ON e.record_kind = 'chunk'
         AND c.user_id = e.user_id AND c.id = e.target_record_id
        LEFT JOIN straylight.document_revisions dr
          ON dr.user_id = c.user_id AND dr.document_id = c.document_id
         AND dr.version = c.document_version
        LEFT JOIN straylight.source_episodes se_chunk
          ON se_chunk.user_id = dr.user_id AND se_chunk.id = dr.source_episode_id
        LEFT JOIN straylight.objects o
          ON e.record_kind = 'object'
         AND o.user_id = e.user_id AND o.id = e.target_record_id
        LEFT JOIN straylight.object_revisions orev
          ON orev.user_id = o.user_id AND orev.object_id = o.id
         AND orev.version = e.record_version
        LEFT JOIN straylight.source_episodes se_object
          ON se_object.user_id = orev.user_id AND se_object.id = orev.source_episode_id
        LEFT JOIN straylight.evidence_items ev
          ON e.record_kind = 'evidence'
         AND ev.user_id = e.user_id AND ev.id = e.target_record_id
        LEFT JOIN straylight.source_episodes se_evidence
          ON se_evidence.user_id = ev.user_id AND se_evidence.id = ev.source_episode_id
        LEFT JOIN straylight.claims cl
          ON e.record_kind = 'claim'
         AND cl.user_id = e.user_id AND cl.id = e.target_record_id
        LEFT JOIN straylight.source_episodes se_claim
          ON se_claim.user_id = cl.user_id AND se_claim.id = cl.source_episode_id
        ORDER BY e.score DESC, e.target_record_id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(Vector::from(vector.to_vec()))
    .bind(embedding_model)
    .bind(lane_limit(spec.limit))
    .bind(root_ids)
    .fetch_all(&mut **tx)
    .await?;
    generic_candidate_rows(rows, "semantic")
}

async fn structured_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let profile = string_filter(&spec.r#where, "type_profile");
    let predicate = string_filter(&spec.r#where, "predicate");
    let record_kind = string_filter(&spec.r#where, "record_kind");
    let authority = string_filter(&spec.r#where, "authority");
    let canonicality = string_filter(&spec.r#where, "canonicality");
    let mut candidates = Vec::new();
    if record_kind.as_deref().is_none_or(|kind| kind == "object")
        && predicate.is_none()
        && authority.is_none()
        && canonicality.is_none()
    {
        let rows = sqlx::query(
            r#"
            SELECT o.id, orev.label, orev.properties, orev.recorded_at,
                   se.source_ref, se.source_version,
                   array_agg(p.profile_ref::text ORDER BY p.profile_ref) AS profiles
            FROM straylight.objects o
            JOIN straylight.corpus_members cm
              ON cm.user_id = o.user_id AND cm.record_id = o.id
            JOIN straylight.object_revisions orev
              ON orev.user_id = o.user_id AND orev.object_id = o.id
             AND orev.version = cm.record_version
            JOIN straylight.object_revision_profiles p
              ON p.user_id = orev.user_id AND p.object_id = orev.object_id
             AND p.object_version = orev.version
            JOIN straylight.source_episodes se
              ON se.user_id = orev.user_id AND se.id = orev.source_episode_id
            WHERE o.user_id = $1 AND o.scope_id = $2
              AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
              AND ($4::text IS NULL OR p.profile_ref::text = $4)
              AND (cardinality($6::uuid[]) = 0 OR o.id = ANY($6::uuid[]))
            GROUP BY o.id, orev.label, orev.properties, orev.recorded_at,
                     se.source_ref, se.source_version
            ORDER BY orev.label NULLS LAST, o.id LIMIT $5
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(session.corpus_revision_id)
        .bind(profile.as_deref())
        .bind(lane_limit(spec.limit))
        .bind(root_ids)
        .fetch_all(&mut **tx)
        .await?;
        for row in rows {
            let id: Uuid = row.try_get("id")?;
            let properties: Value = row.try_get("properties")?;
            let profiles: Vec<String> = row.try_get("profiles")?;
            candidates.push(RetrievalCandidate {
                reference: record_ref("object", id),
                source_ref: row.try_get("source_ref")?,
                path: None,
                heading: row.try_get("label")?,
                content: serde_json::to_string(
                    &json!({"profiles": profiles, "properties": properties}),
                )?,
                content_hash: sha256_prefixed_json(&properties)?,
                source_version: row.try_get("source_version")?,
                authority: None,
                canonicality: Some("canonical_at_revision".to_owned()),
                provenance: None,
                recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
                valid_time: None,
                evidence_refs: Vec::new(),
                why_selected: vec!["structured_filter_match".to_owned()],
                lane_scores: HashMap::new(),
                score: 1.0,
            });
        }
    }
    if record_kind.as_deref().is_none_or(|kind| kind == "claim") && profile.is_none() {
        let rows = sqlx::query(
            r#"
            SELECT c.id, c.predicate::text AS predicate, c.value, c.authority,
                   c.canonicality, c.recorded_at, se.source_ref, se.source_version,
                   ts.kind AS temporal_kind, to_jsonb(ts) AS valid_time,
                   ARRAY(SELECT 'evidence:' || replace(link.evidence_id::text, '-', '')
                         FROM straylight.claim_evidence_links link
                         WHERE link.user_id = c.user_id AND link.claim_id = c.id
                         ORDER BY link.evidence_id) AS evidence_refs
            FROM straylight.claims c
            JOIN straylight.corpus_members cm
              ON cm.user_id = c.user_id AND cm.record_id = c.id
            JOIN straylight.source_episodes se
              ON se.user_id = c.user_id AND se.id = c.source_episode_id
            LEFT JOIN straylight.temporal_specs ts
              ON ts.user_id = c.user_id AND ts.id = c.valid_temporal_id
            WHERE c.user_id = $1 AND c.scope_id = $2
              AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
              AND ($4::text IS NULL OR c.predicate::text = $4)
              AND ($5::text IS NULL OR c.authority = $5)
              AND ($6::text IS NULL OR c.canonicality = $6)
              AND (cardinality($7::uuid[]) = 0 OR c.id = ANY($7::uuid[])
                OR EXISTS (
                  SELECT 1 FROM straylight.claim_about_targets cat
                  WHERE cat.user_id = c.user_id AND cat.claim_id = c.id
                    AND cat.target_record_id = ANY($7::uuid[])
                ))
            ORDER BY c.recorded_at DESC, c.id LIMIT $8
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(session.corpus_revision_id)
        .bind(predicate.as_deref())
        .bind(authority.as_deref())
        .bind(canonicality.as_deref())
        .bind(root_ids)
        .bind(lane_limit(spec.limit))
        .fetch_all(&mut **tx)
        .await?;
        for row in rows {
            let id: Uuid = row.try_get("id")?;
            let value: Value = row.try_get("value")?;
            candidates.push(RetrievalCandidate {
                reference: record_ref("claim", id),
                source_ref: row.try_get("source_ref")?,
                path: None,
                heading: Some(row.try_get("predicate")?),
                content: serde_json::to_string(&value)?,
                content_hash: sha256_prefixed_json(&value)?,
                source_version: row.try_get("source_version")?,
                authority: row.try_get("authority")?,
                canonicality: row.try_get("canonicality")?,
                provenance: None,
                recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
                valid_time: row.try_get("valid_time")?,
                evidence_refs: row.try_get("evidence_refs")?,
                why_selected: vec!["structured_filter_match".to_owned()],
                lane_scores: HashMap::new(),
                score: 1.0,
            });
        }
    }
    if let Some(filter) = &spec.state_filter
        && record_kind.as_deref().is_none_or(|kind| kind == "claim")
        && predicate
            .as_deref()
            .is_none_or(|value| value == filter.machine_ref)
    {
        candidates.extend(
            state_lane_tx(
                tx,
                session,
                filter,
                authority.as_deref(),
                canonicality.as_deref(),
                profile.as_deref(),
                root_ids,
                spec.limit,
            )
            .await?,
        );
    }
    Ok(candidates)
}

const STATE_LANE_SQL: &str = r#"
    WITH latest_states AS (
      SELECT DISTINCT ON (sa.target_record_id, sa.state_machine_ref)
             sa.target_record_id, sa.state_machine_ref::text, sa.state_value,
             sa.claim_id, c.recorded_at, c.authority, c.canonicality,
             c.source_episode_id
      FROM straylight.state_assignments sa
      JOIN straylight.claims c
        ON c.user_id = sa.user_id AND c.id = sa.claim_id
      JOIN straylight.corpus_members cm
        ON cm.user_id = sa.user_id AND cm.record_id = sa.claim_id
      WHERE sa.user_id = $1 AND sa.scope_id = $2
        AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
        AND sa.state_machine_ref::text = $4
      ORDER BY sa.target_record_id, sa.state_machine_ref, c.recorded_at DESC, c.id DESC
    ), filtered_states AS (
      SELECT * FROM latest_states head
      WHERE (cardinality($5::text[]) = 0 OR head.state_value = ANY($5::text[]))
        AND ($6::text IS NULL OR head.authority = $6)
        AND ($7::text IS NULL OR head.canonicality = $7)
        AND (cardinality($9::uuid[]) = 0
          OR head.target_record_id = ANY($9::uuid[])
          OR head.claim_id = ANY($9::uuid[]))
    )
    SELECT ps.*, rk.record_kind, se.source_ref, se.source_version,
           coalesce(orev.label, rk.record_kind || ':' || ps.target_record_id::text) AS label,
           ARRAY(SELECT 'evidence:' || replace(link.evidence_id::text, '-', '')
                 FROM straylight.claim_evidence_links link
                 WHERE link.user_id = $1 AND link.claim_id = ps.claim_id) AS evidence_refs
    FROM filtered_states ps
    JOIN straylight.record_keys rk
      ON rk.user_id = $1 AND rk.record_id = ps.target_record_id
    JOIN straylight.source_episodes se
      ON se.user_id = $1 AND se.id = ps.source_episode_id
    LEFT JOIN straylight.corpus_members om
      ON om.user_id = $1 AND om.record_id = ps.target_record_id
     AND om.corpus_revision_id = $3 AND om.disposition = 'active'
    LEFT JOIN straylight.object_revisions orev
      ON orev.user_id = $1 AND orev.object_id = ps.target_record_id
     AND orev.version = om.record_version
    WHERE ($8::text IS NULL OR EXISTS (
      SELECT 1 FROM straylight.object_revision_profiles profile
      WHERE profile.user_id = $1 AND profile.object_id = ps.target_record_id
        AND profile.object_version = om.record_version
        AND profile.profile_ref::text = $8
    ))
    ORDER BY ps.recorded_at DESC LIMIT $10
"#;

async fn state_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    filter: &crate::models::StateFilter,
    authority: Option<&str>,
    canonicality: Option<&str>,
    profile: Option<&str>,
    root_ids: &[Uuid],
    limit: usize,
) -> ApiResult<Vec<RetrievalCandidate>> {
    let rows = sqlx::query(STATE_LANE_SQL)
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(session.corpus_revision_id)
        .bind(&filter.machine_ref)
        .bind(&filter.states)
        .bind(authority)
        .bind(canonicality)
        .bind(profile)
        .bind(root_ids)
        .bind(lane_limit(limit))
        .fetch_all(&mut **tx)
        .await?;
    let mut candidates = Vec::new();
    for row in rows {
        let target: Uuid = row.try_get("target_record_id")?;
        let claim: Uuid = row.try_get("claim_id")?;
        let value: String = row.try_get("state_value")?;
        candidates.push(RetrievalCandidate {
            reference: record_ref("claim", claim),
            source_ref: row.try_get("source_ref")?,
            path: None,
            heading: Some(row.try_get("label")?),
            content: serde_json::to_string(&json!({
                "target_ref": record_ref(&row.try_get::<String, _>("record_kind")?, target),
                "state_machine": row.try_get::<String, _>("state_machine_ref")?,
                "state": value
            }))?,
            content_hash: sha256_prefixed_json(&json!(value))?,
            source_version: row.try_get("source_version")?,
            authority: row.try_get("authority")?,
            canonicality: row.try_get("canonicality")?,
            provenance: None,
            recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
            valid_time: None,
            evidence_refs: row.try_get("evidence_refs")?,
            why_selected: vec![
                "canonical_object_state".to_owned(),
                "structured_filter_match".to_owned(),
            ],
            lane_scores: HashMap::new(),
            score: 1.5,
        });
    }
    Ok(candidates)
}

async fn temporal_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let query = spec.query.as_deref().unwrap_or("");
    let rows = sqlx::query(
        r#"
        SELECT c.id, c.predicate::text AS predicate, c.value, c.authority,
               c.canonicality, c.recorded_at, se.source_ref, se.source_version,
               to_jsonb(ts) - 'user_id' - 'scope_id' AS valid_time,
               ARRAY(SELECT 'evidence:' || replace(link.evidence_id::text, '-', '')
                     FROM straylight.claim_evidence_links link
                     WHERE link.user_id = c.user_id AND link.claim_id = c.id) AS evidence_refs,
               (CASE WHEN position(lower($4) in lower(to_jsonb(ts)::text)) > 0 THEN 2.0 ELSE 1.0 END)::float8 AS score
        FROM straylight.claims c
        JOIN straylight.corpus_members cm
          ON cm.user_id = c.user_id AND cm.record_id = c.id
        JOIN straylight.temporal_specs ts
          ON ts.user_id = c.user_id AND ts.id = c.valid_temporal_id
        JOIN straylight.source_episodes se
          ON se.user_id = c.user_id AND se.id = c.source_episode_id
        WHERE c.user_id = $1 AND c.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND ($4 = '' OR position(lower($4) in lower(c.value::text || ' ' || c.predicate::text || ' ' || to_jsonb(ts)::text)) > 0)
          AND (cardinality($6::uuid[]) = 0 OR c.id = ANY($6::uuid[])
            OR EXISTS (
              SELECT 1 FROM straylight.claim_about_targets cat
              WHERE cat.user_id = c.user_id AND cat.claim_id = c.id
                AND cat.target_record_id = ANY($6::uuid[])
            ))
        ORDER BY score DESC, c.recorded_at DESC LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(query)
    .bind(lane_limit(spec.limit))
    .bind(root_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut candidates = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let value: Value = row.try_get("value")?;
        candidates.push(RetrievalCandidate {
            reference: record_ref("claim", id),
            source_ref: row.try_get("source_ref")?,
            path: None,
            heading: Some(row.try_get("predicate")?),
            content: serde_json::to_string(&value)?,
            content_hash: sha256_prefixed_json(&value)?,
            source_version: row.try_get("source_version")?,
            authority: row.try_get("authority")?,
            canonicality: row.try_get("canonicality")?,
            provenance: None,
            recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
            valid_time: row.try_get("valid_time")?,
            evidence_refs: row.try_get("evidence_refs")?,
            why_selected: vec!["temporal_match".to_owned()],
            lane_scores: HashMap::new(),
            score: row.try_get("score")?,
        });
    }
    if currentness_intent(query) {
        candidates.extend(timestamped_chunk_lane_tx(tx, session, spec, root_ids).await?);
    }
    Ok(candidates)
}

async fn timestamped_chunk_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let query = spec.query.as_deref().unwrap_or("");
    let discovery_terms = lexical_discovery_terms(query);
    let discovery_query = lexical_discovery_query(query);
    let rows = sqlx::query(
        r#"
        WITH requested AS (
          SELECT websearch_to_tsquery('english', $4) AS terms,
                 $7::text[] AS raw_terms
        ), timestamped AS (
          SELECT c.id, c.user_id, c.scope_id, c.document_id, c.document_version,
                 c.ordinal, c.heading_path, c.content, c.content_hash, c.locator,
                 se.id AS source_episode_id, se.source_ref, se.source_version,
                 se.source_kind,se.metadata AS source_metadata,
                 (
                   SELECT jsonb_build_object(
                     'asset_ref','asset:' || described.asset_id::text,
                     'version',described.asset_version,
                     'path',se.metadata->>'original_path'
                   )
                   FROM straylight.source_asset_links AS described
                   WHERE described.user_id=se.user_id
                     AND described.source_episode_id=se.id
                     AND described.role='describes'
                   ORDER BY described.asset_id,described.asset_version
                   LIMIT 1
                 ) AS described_asset,
                 dr.native_locator, dr.recorded_at,
                 coalesce(
                   substring(left(c.content, 500) from
                     '20[0-9]{2}-[0-9]{2}-[0-9]{2}[T ][0-9]{2}:[0-9]{2}:[0-9]{2}Z?'),
                   substring(left(c.content, 500) from
                     '20[0-9]{2}-[0-9]{2}-[0-9]{2}[T ][0-9]{2}:[0-9]{2}Z?'),
                   substring(left(c.content, 500) from
                     '20[0-9]{2}-[0-9]{2}-[0-9]{2}')
                 ) AS content_time,
                 coverage.term_coverage::float8
                   + 3.0 * coverage.path_coverage::float8
                   + ts_rank_cd(
                   setweight(to_tsvector('english', straylight.lexical_source_text(
                     se.source_ref
                   )), 'A') || c.search_vector,
                   requested.terms,
                   1
                 )::float8 AS score,
                 coverage.term_coverage + 3 * coverage.path_coverage AS term_coverage
          FROM straylight.chunks c
          JOIN straylight.corpus_members cm
            ON cm.user_id = c.user_id AND cm.record_id = c.id
          JOIN straylight.document_revisions dr
            ON dr.user_id = c.user_id AND dr.document_id = c.document_id
           AND dr.version = c.document_version
          JOIN straylight.source_episodes se
            ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
          CROSS JOIN requested
          CROSS JOIN LATERAL (
            SELECT
              count(*) FILTER (WHERE (
                setweight(to_tsvector('english', straylight.lexical_source_text(
                  se.source_ref
                )), 'A') || c.search_vector
              ) @@ plainto_tsquery('english', term.value)) AS term_coverage,
              count(*) FILTER (WHERE
                to_tsvector('english', straylight.lexical_source_text(se.source_ref))
                  @@ plainto_tsquery('english', term.value)
              ) AS path_coverage
            FROM unnest(requested.raw_terms) AS term(value)
          ) coverage
          WHERE c.user_id = $1 AND c.scope_id = $2
            AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
            AND (
              setweight(to_tsvector('english', straylight.lexical_source_text(
                se.source_ref
              )), 'A') || c.search_vector
            ) @@ requested.terms
            AND (cardinality($6::uuid[]) = 0 OR c.id = ANY($6::uuid[])
              OR c.document_id = ANY($6::uuid[])
              OR dr.source_episode_id = ANY($6::uuid[]))
        ), scored AS (
          SELECT timestamped.*, max(term_coverage) OVER () AS relevance_ceiling
          FROM timestamped
          WHERE content_time IS NOT NULL
        )
        SELECT id, source_ref, native_locator ->> 'path' AS path,
               array_to_string(heading_path, ' / ') AS heading, content,
               content_hash, source_version, source_kind,source_metadata,
               described_asset,recorded_at, locator, content_time,
               score,
               ARRAY(
                 SELECT 'evidence:' || replace(ei.id::text, '-', '')
                 FROM straylight.evidence_items ei
                 JOIN straylight.corpus_members ecm
                   ON ecm.user_id = ei.user_id AND ecm.record_id = ei.id
                 WHERE ei.user_id = scored.user_id
                   AND ei.scope_id = scored.scope_id
                   AND ei.source_episode_id = scored.source_episode_id
                   AND ei.evidence_kind = 'source_native'
                   AND ecm.corpus_revision_id = $3 AND ecm.disposition = 'active'
                 ORDER BY ei.created_at, ei.id LIMIT 32
               ) AS evidence_refs
        FROM scored
        WHERE term_coverage >= greatest(1, ceil(relevance_ceiling * 0.6))
        ORDER BY content_time DESC, term_coverage DESC, score DESC, source_ref, ordinal
        LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(discovery_query)
    .bind(lane_limit(spec.limit))
    .bind(root_ids)
    .bind(discovery_terms)
    .fetch_all(&mut **tx)
    .await?;

    let mut candidates = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let content_time: String = row.try_get("content_time")?;
        let source_kind: String = row.try_get("source_kind")?;
        let source_metadata: Value = row.try_get("source_metadata")?;
        let described_asset: Option<Value> = row.try_get("described_asset")?;
        let (authority, canonicality, provenance) =
            source_provenance(&source_kind, &source_metadata, described_asset);
        candidates.push(RetrievalCandidate {
            reference: record_ref("chunk", id),
            source_ref: row.try_get("source_ref")?,
            path: row.try_get("path")?,
            heading: row.try_get("heading")?,
            content: row.try_get("content")?,
            content_hash: prefixed_hash(row.try_get("content_hash")?),
            source_version: row.try_get("source_version")?,
            authority,
            canonicality,
            provenance,
            recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
            valid_time: Some(json!({
                "kind": "source_text_timestamp",
                "value": content_time,
            })),
            evidence_refs: row.try_get("evidence_refs")?,
            why_selected: vec!["timestamped_source_currentness".to_owned()],
            lane_scores: HashMap::new(),
            score: row.try_get("score")?,
        });
    }
    Ok(candidates)
}

fn currentness_intent(query: &str) -> bool {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '-')
        .map(str::to_lowercase)
        .any(|term| {
            matches!(
                term.as_str(),
                "current"
                    | "latest"
                    | "newest"
                    | "now"
                    | "recent"
                    | "superseded"
                    | "superseding"
                    | "up-to-date"
            )
        })
}

async fn relation_lane_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    root_ids: &[Uuid],
) -> ApiResult<Vec<RetrievalCandidate>> {
    let query = spec.query.as_deref().unwrap_or("");
    let predicates = spec
        .expand
        .as_ref()
        .map(|expand| expand.relations.clone())
        .unwrap_or_default();
    let rows = sqlx::query(
        r#"
        SELECT r.id, rr.version, rr.predicate::text AS predicate, rr.qualifiers,
               rr.recorded_at, se.source_ref, se.source_version,
               jsonb_agg(jsonb_build_object(
                 'role', ep.role, 'position', ep.position,
                 'target_ref', rk.record_kind || ':' || replace(ep.target_record_id::text, '-', '')
               ) ORDER BY ep.role, ep.position) AS endpoints,
               ARRAY(SELECT 'evidence:' || replace(rel.evidence_id::text, '-', '')
                     FROM straylight.relation_evidence_links rel
                     WHERE rel.user_id = r.user_id AND rel.relation_id = r.id
                       AND rel.relation_version = rr.version) AS evidence_refs,
               (CASE WHEN rr.predicate::text = ANY($5::text[]) THEN 2.0
                    WHEN position(lower($4) in lower(rr.predicate::text || ' ' || rr.qualifiers::text)) > 0 THEN 1.5
                    ELSE 1.0 END)::float8 AS score
        FROM straylight.relations r
        JOIN straylight.corpus_members cm
          ON cm.user_id = r.user_id AND cm.record_id = r.id
        JOIN straylight.relation_revisions rr
          ON rr.user_id = r.user_id AND rr.relation_id = r.id
         AND rr.version = cm.record_version
        JOIN straylight.relation_endpoints ep
          ON ep.user_id = rr.user_id AND ep.relation_id = rr.relation_id
         AND ep.relation_version = rr.version
        JOIN straylight.record_keys rk
          ON rk.user_id = ep.user_id AND rk.record_id = ep.target_record_id
        JOIN straylight.source_episodes se
          ON se.user_id = rr.user_id AND se.id = rr.source_episode_id
        WHERE r.user_id = $1 AND r.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
          AND ($4 = '' OR cardinality($5::text[]) > 0
            OR position(lower($4) in lower(rr.predicate::text || ' ' || rr.qualifiers::text)) > 0)
          AND (cardinality($7::uuid[]) = 0 OR r.id = ANY($7::uuid[])
            OR EXISTS (
              SELECT 1 FROM straylight.relation_endpoints rooted
              WHERE rooted.user_id = r.user_id AND rooted.relation_id = r.id
                AND rooted.relation_version = rr.version
                AND rooted.target_record_id = ANY($7::uuid[])
            ))
        GROUP BY r.id, rr.version, rr.predicate, rr.qualifiers, rr.recorded_at,
                 se.source_ref, se.source_version
        ORDER BY score DESC, rr.recorded_at DESC LIMIT $6
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(query)
    .bind(predicates)
    .bind(lane_limit(spec.limit))
    .bind(root_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut candidates = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let payload = json!({
            "predicate": row.try_get::<String, _>("predicate")?,
            "qualifiers": row.try_get::<Value, _>("qualifiers")?,
            "endpoints": row.try_get::<Value, _>("endpoints")?
        });
        candidates.push(RetrievalCandidate {
            reference: record_ref("relation", id),
            source_ref: row.try_get("source_ref")?,
            path: None,
            heading: Some(row.try_get("predicate")?),
            content: serde_json::to_string(&payload)?,
            content_hash: sha256_prefixed_json(&payload)?,
            source_version: row.try_get("source_version")?,
            authority: None,
            canonicality: Some("canonical_at_revision".to_owned()),
            provenance: None,
            recorded_at: Some(row.try_get::<DateTime<Utc>, _>("recorded_at")?.to_rfc3339()),
            valid_time: None,
            evidence_refs: row.try_get("evidence_refs")?,
            why_selected: vec!["relationship_traversal".to_owned()],
            lane_scores: HashMap::new(),
            score: row.try_get("score")?,
        });
    }
    Ok(candidates)
}

async fn expand_selected_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    spec: &QuerySpec,
    selected: &mut Vec<RetrievalCandidate>,
) -> ApiResult<()> {
    let Some(expand) = &spec.expand else {
        return Ok(());
    };
    let chunk_ids: Vec<Uuid> = selected
        .iter()
        .filter_map(|item| {
            item.reference
                .strip_prefix("chunk:")
                .and_then(|value| Uuid::parse_str(value).ok())
        })
        .collect();
    if expand.neighbors > 0 && !chunk_ids.is_empty() {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT neighbor.id, se.source_ref,
                   dr.native_locator ->> 'path' AS path,
                   array_to_string(neighbor.heading_path, ' / ') AS heading,
                   neighbor.content, neighbor.content_hash, se.source_version,
                   se.source_kind,se.metadata AS source_metadata,
                   (
                     SELECT jsonb_build_object(
                       'asset_ref','asset:' || described.asset_id::text,
                       'version',described.asset_version,
                       'path',se.metadata->>'original_path'
                     )
                     FROM straylight.source_asset_links AS described
                     WHERE described.user_id=se.user_id
                       AND described.source_episode_id=se.id
                       AND described.role='describes'
                     ORDER BY described.asset_id,described.asset_version
                     LIMIT 1
                   ) AS described_asset,
                   dr.recorded_at, neighbor.locator,
                   ARRAY(
                     SELECT 'evidence:' || replace(ei.id::text, '-', '')
                     FROM straylight.evidence_items ei
                     JOIN straylight.corpus_members ecm
                       ON ecm.user_id = ei.user_id AND ecm.record_id = ei.id
                     WHERE ei.user_id = neighbor.user_id AND ei.scope_id = neighbor.scope_id
                       AND ei.source_episode_id = dr.source_episode_id
                       AND ei.evidence_kind = 'source_native'
                       AND ecm.corpus_revision_id = $3 AND ecm.disposition = 'active'
                     ORDER BY ei.created_at, ei.id LIMIT 32
                   ) AS evidence_refs,
                   0.25::float8 AS score
            FROM straylight.chunks seed
            JOIN straylight.chunks neighbor
              ON neighbor.user_id = seed.user_id AND neighbor.document_id = seed.document_id
             AND neighbor.document_version = seed.document_version
             AND abs(neighbor.ordinal - seed.ordinal) <= $5
            JOIN straylight.corpus_members cm
              ON cm.user_id = neighbor.user_id AND cm.record_id = neighbor.id
            JOIN straylight.document_revisions dr
              ON dr.user_id = neighbor.user_id AND dr.document_id = neighbor.document_id
             AND dr.version = neighbor.document_version
            JOIN straylight.source_episodes se
              ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
            WHERE seed.user_id = $1 AND seed.scope_id = $2
              AND seed.id = ANY($4::uuid[])
              AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
            ORDER BY neighbor.id LIMIT 100
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(session.corpus_revision_id)
        .bind(chunk_ids)
        .bind(i32::try_from(expand.neighbors.min(20)).unwrap_or(20))
        .fetch_all(&mut **tx)
        .await?;
        let existing: HashSet<String> =
            selected.iter().map(|item| item.reference.clone()).collect();
        for mut candidate in chunk_rows(rows, "neighbor")? {
            if !existing.contains(&candidate.reference) {
                candidate.why_selected = vec!["neighbor_expansion".to_owned()];
                selected.push(candidate);
            }
        }
    }
    Ok(())
}

async fn contradiction_summaries_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    selected: &[RetrievalCandidate],
) -> ApiResult<Vec<Value>> {
    let claim_ids: Vec<Uuid> = selected
        .iter()
        .filter_map(|item| {
            item.reference
                .strip_prefix("claim:")
                .and_then(|value| Uuid::parse_str(value).ok())
        })
        .collect();
    if claim_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'claim_ref', 'claim:' || replace(lineage.claim_id::text, '-', ''),
          'other_claim_ref', 'claim:' || replace(lineage.predecessor_claim_id::text, '-', ''),
          'kind', lineage.lineage_kind
        )
        FROM straylight.claim_lineage lineage
        JOIN straylight.corpus_members cm
          ON cm.user_id = lineage.user_id
         AND cm.record_id IN (lineage.claim_id, lineage.predecessor_claim_id)
        WHERE lineage.user_id = $1 AND cm.scope_id = $2
          AND cm.corpus_revision_id = $3
          AND lineage.lineage_kind IN ('contradicts', 'supersedes', 'corrects', 'retracts')
          AND (lineage.claim_id = ANY($4::uuid[])
            OR lineage.predecessor_claim_id = ANY($4::uuid[]))
        GROUP BY lineage.claim_id, lineage.predecessor_claim_id, lineage.lineage_kind
        ORDER BY lineage.lineage_kind, lineage.claim_id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(claim_ids)
    .fetch_all(&mut **tx)
    .await?)
}

fn chunk_rows(rows: Vec<sqlx::postgres::PgRow>, lane: &str) -> ApiResult<Vec<RetrievalCandidate>> {
    let mut candidates = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let source_kind: String = row.try_get("source_kind")?;
        let source_metadata: Value = row.try_get("source_metadata")?;
        let described_asset: Option<Value> = row.try_get("described_asset")?;
        let (authority, canonicality, provenance) =
            source_provenance(&source_kind, &source_metadata, described_asset);
        candidates.push(RetrievalCandidate {
            reference: record_ref("chunk", id),
            source_ref: row.try_get("source_ref")?,
            path: row.try_get("path")?,
            heading: row.try_get("heading")?,
            content: row.try_get("content")?,
            content_hash: prefixed_hash(row.try_get("content_hash")?),
            source_version: row.try_get("source_version")?,
            authority,
            canonicality,
            provenance,
            recorded_at: row
                .try_get::<Option<DateTime<Utc>>, _>("recorded_at")?
                .map(|value| value.to_rfc3339()),
            valid_time: Some(row.try_get::<Value, _>("locator").unwrap_or(Value::Null)),
            evidence_refs: row.try_get("evidence_refs")?,
            why_selected: vec![format!("{lane}_recall")],
            lane_scores: HashMap::new(),
            score: row.try_get("score")?,
        });
    }
    Ok(candidates)
}

fn source_provenance(
    source_kind: &str,
    metadata: &Value,
    described_asset: Option<Value>,
) -> (Option<String>, Option<String>, Option<Value>) {
    if source_kind != "derived_asset_description" {
        return (
            metadata
                .get("authoritative")
                .and_then(Value::as_bool)
                .filter(|value| *value)
                .map(|_| "source_authoritative".to_owned()),
            Some("source_text_at_revision".to_owned()),
            None,
        );
    }
    (
        Some("derived_non_authoritative".to_owned()),
        Some("non_authoritative_derivative".to_owned()),
        Some(json!({
            "source_kind": source_kind,
            "authoritative": false,
            "evidence_kind": "derived_non_authoritative",
            "description_status": metadata.get("description_status"),
            "description_method": metadata.get("description_method"),
            "prompt_version": metadata.get("prompt_version"),
            "limitations": [
                "Generated descriptions may contain extraction or model errors.",
                "Use the linked native asset or source-backed Markdown for consequential claims."
            ],
            "native_asset": described_asset
        })),
    )
}

fn generic_candidate_rows(
    rows: Vec<sqlx::postgres::PgRow>,
    lane: &str,
) -> ApiResult<Vec<RetrievalCandidate>> {
    let mut candidates = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let kind: String = row.try_get("record_kind")?;
        let source_kind: Option<String> = row.try_get("source_kind")?;
        let source_metadata: Option<Value> = row.try_get("source_metadata")?;
        let described_asset: Option<Value> = row.try_get("described_asset")?;
        let mut authority: Option<String> = row.try_get("authority")?;
        let mut canonicality: Option<String> = row.try_get("canonicality")?;
        let mut provenance = None;
        if let (Some(source_kind), Some(source_metadata)) =
            (source_kind.as_deref(), source_metadata.as_ref())
        {
            let source = source_provenance(source_kind, source_metadata, described_asset);
            if source.2.is_some() {
                authority = source.0;
                canonicality = source.1;
                provenance = source.2;
            }
        }
        candidates.push(RetrievalCandidate {
            reference: record_ref(&kind, id),
            source_ref: row.try_get("source_ref")?,
            path: row.try_get("path")?,
            heading: row.try_get("heading")?,
            content: row.try_get("content")?,
            content_hash: prefixed_hash(row.try_get("content_hash")?),
            source_version: row.try_get("source_version")?,
            authority,
            canonicality,
            provenance,
            recorded_at: row
                .try_get::<Option<DateTime<Utc>>, _>("recorded_at")?
                .map(|value| value.to_rfc3339()),
            valid_time: None,
            evidence_refs: row.try_get("evidence_refs")?,
            why_selected: vec![format!("{lane}_recall")],
            lane_scores: HashMap::new(),
            score: row.try_get("score")?,
        });
    }
    Ok(candidates)
}

fn normalize_selection_reasons(candidates: &mut [RetrievalCandidate]) {
    for candidate in candidates {
        let mut reasons = BTreeSet::new();
        for lane in candidate.lane_scores.keys() {
            reasons.insert(match lane.as_str() {
                "exact" => "exact_phrase_match",
                "structured" => "structured_filter_match",
                "lexical" => "lexical_recall",
                "semantic" => "semantic_recall",
                "temporal" => "temporal_match",
                "relations" => "relationship_traversal",
                other => other,
            });
        }
        if candidate.content.contains("state_machine") {
            reasons.insert("canonical_object_state");
        }
        candidate.why_selected = reasons.into_iter().map(str::to_owned).collect();
    }
}

fn exact_object_ambiguities(spec: &QuerySpec, candidates: &[RetrievalCandidate]) -> Vec<Value> {
    let Some(query) = spec
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    let matches: Vec<&RetrievalCandidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.reference.starts_with("object:")
                && candidate
                    .heading
                    .as_deref()
                    .is_some_and(|heading| heading.eq_ignore_ascii_case(query))
        })
        .collect();
    if matches.len() > 1 {
        vec![json!({
            "input": query,
            "kind": "object_name_ambiguity",
            "candidates": matches.iter().map(|candidate| &candidate.reference).collect::<Vec<_>>(),
            "rule": "matching names do not establish identity equivalence"
        })]
    } else {
        Vec::new()
    }
}

fn string_filter(filters: &Map<String, Value>, key: &str) -> Option<String> {
    filters.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn lane_limit(requested: usize) -> i64 {
    i64::try_from(requested.saturating_mul(4).clamp(12, 400)).unwrap_or(400)
}

fn parse_uuid_ref(value: &str, accepted_prefixes: &[&str]) -> ApiResult<Uuid> {
    let mut candidates = vec![value];
    if let Some((prefix, remainder)) = value.split_once(':')
        && (accepted_prefixes.is_empty() || accepted_prefixes.contains(&prefix))
    {
        candidates.push(remainder);
    }
    if let Some(remainder) = value.strip_prefix("ms_")
        && (accepted_prefixes.is_empty() || accepted_prefixes.contains(&"ms"))
    {
        candidates.push(remainder);
    }
    candidates
        .into_iter()
        .find_map(|candidate| Uuid::parse_str(candidate).ok())
        .ok_or_else(|| ApiError::invalid(format!("invalid record reference: {value}")))
}

fn record_ref(kind: &str, id: Uuid) -> String {
    format!("{kind}:{}", id.simple())
}

fn prefixed_hash(value: String) -> String {
    if value.starts_with("sha256:") {
        value
    } else {
        format!("sha256:{value}")
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_prefixed_json(value: &Value) -> ApiResult<String> {
    Ok(prefixed_hash(sha256_hex(&serde_json::to_vec(value)?)))
}

/// Read exact source-native or structured material in a bounded batch. Item
/// errors are isolated; a bad reference does not discard successful siblings.
pub async fn read(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    request: &ReadRequest,
) -> ApiResult<ReadBatchResult> {
    request.validate()?;
    if is_stage_ref(&request.session_id) {
        let stage = load_staged_corpus(state, auth, &request.session_id, Capability::Read).await?;
        return read_staged_corpus(state, auth, codec, &stage, request).await;
    }
    let session = load_session(state, auth, &request.session_id, Capability::Read).await?;
    let mut items = Vec::with_capacity(request.requests.len());
    for spec in &request.requests {
        let item_started = Instant::now();
        let view = read_view_name(spec.view);
        let mut tx = state.begin_read(auth).await?;
        match read_item_tx(&mut tx, state, codec, &session, spec).await {
            Ok(item) => {
                tx.commit().await?;
                metrics::histogram!(
                    "read.item.duration_ms",
                    "view" => view,
                    "status" => telemetry::item_status(item.status)
                )
                .record(telemetry::elapsed_ms(item_started));
                items.push(item);
            }
            Err(error) => {
                tx.rollback().await?;
                metrics::histogram!(
                    "read.item.duration_ms",
                    "view" => view,
                    "status" => "failed"
                )
                .record(telemetry::elapsed_ms(item_started));
                items.push(ReadItemResult {
                    reference: spec.reference.clone(),
                    view: read_view_name(spec.view).to_owned(),
                    status: ItemStatus::Failed,
                    data: None,
                    error: Some(item_error(error)),
                    truncation: Truncation::default(),
                });
            }
        }
    }
    let status = if items.iter().all(|item| item.status == ItemStatus::Complete) {
        ResponseStatus::Complete
    } else if items.iter().all(|item| item.status == ItemStatus::Failed) {
        ResponseStatus::Inconsistent
    } else {
        ResponseStatus::Partial
    };
    let successful = items
        .iter()
        .filter(|item| item.status != ItemStatus::Failed)
        .count();
    Ok(ReadBatchResult {
        session_id: session.session_ref.clone(),
        corpus_revision: session.corpus_revision_ref.clone(),
        status,
        items,
        freshness: session.freshness,
        coverage: Coverage {
            searched: vec![CoveragePartition {
                partition: "requested_records".to_owned(),
                lane: "read".to_owned(),
                completeness: if successful == request.requests.len() {
                    "complete".to_owned()
                } else {
                    "partial".to_owned()
                },
                index_revision: Some(session.corpus_revision_ref),
                searched_count: request.requests.len(),
                candidate_count: successful,
                failure_reason: None,
            }],
            unsearched: Vec::new(),
            absence_safe: false,
        },
    })
}

async fn read_item_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    codec: &ContinuationCodec,
    session: &PinnedSession,
    spec: &ReadSpec,
) -> ApiResult<ReadItemResult> {
    if spec.view == ReadView::MaterializeScope {
        if spec.reference != session.scope_ref && !spec.reference.starts_with("scope:") {
            return Err(ApiError::invalid(
                "materialize_scope requires the immutable session scope reference",
            ));
        }
        let value = materialize_scope_tx(tx, session, usize::MAX / 8).await?;
        let serialized = serde_json::to_string_pretty(&value)?;
        if serialized.chars().count() > spec.max_chars.unwrap_or(DEFAULT_READ_CHARS)
            || spec.continuation_token.is_some()
        {
            return text_read_result(
                codec,
                session,
                spec,
                LoadedText {
                    metadata: json!({
                        "scope_ref": session.scope_ref,
                        "corpus_revision": session.corpus_revision_ref,
                        "representation": "materialized_scope_json"
                    }),
                    text: serialized,
                },
            );
        }
        return Ok(complete_read_item(spec, value));
    }

    let resolved = resolve_record_tx(tx, session, &spec.reference).await?;
    match spec.view {
        ReadView::Full | ReadView::Range => {
            let loaded =
                load_text_tx(tx, state, session, &resolved, spec.version.as_deref()).await?;
            text_read_result(codec, session, spec, loaded)
        }
        ReadView::Neighbors => {
            let data = neighbors_tx(tx, session, &resolved, spec).await?;
            Ok(complete_read_item(spec, data))
        }
        ReadView::CurrentState | ReadView::Structured => {
            let data =
                structured_record_tx(tx, session, &resolved, spec.version.as_deref()).await?;
            Ok(complete_read_item(spec, data))
        }
        ReadView::Outline => {
            let data = outline_tx(tx, session, &resolved).await?;
            Ok(complete_read_item(spec, data))
        }
        ReadView::Relationships => {
            let data = relationships_tx(tx, session, &resolved).await?;
            Ok(complete_read_item(spec, data))
        }
        ReadView::History => {
            let data = history_tx(tx, session, &resolved).await?;
            Ok(complete_read_item(spec, data))
        }
        ReadView::Diff => {
            let data = diff_tx(tx, state, session, &resolved, spec).await?;
            let mut item = complete_read_item(spec, data.clone());
            if data.get("unsupported").and_then(Value::as_bool) == Some(true) {
                item.status = ItemStatus::Unsupported;
            }
            Ok(item)
        }
        ReadView::LastKnownGood => {
            let data = last_known_good_tx(tx, session, &resolved).await?;
            Ok(complete_read_item(spec, data))
        }
        ReadView::MaterializeScope => unreachable!("handled above"),
    }
}

fn complete_read_item(spec: &ReadSpec, data: Value) -> ReadItemResult {
    let serialized = serde_json::to_string(&data).unwrap_or_default();
    ReadItemResult {
        reference: spec.reference.clone(),
        view: read_view_name(spec.view).to_owned(),
        status: ItemStatus::Complete,
        data: Some(data),
        error: None,
        truncation: Truncation {
            truncated: false,
            returned_tokens: estimate_tokens(&serialized),
            limit_tokens: DEFAULT_TOKEN_BUDGET,
            continuation_token: None,
        },
    }
}

fn text_read_result(
    codec: &ContinuationCodec,
    session: &PinnedSession,
    spec: &ReadSpec,
    loaded: LoadedText,
) -> ApiResult<ReadItemResult> {
    text_read_result_for_snapshot(
        codec,
        &session.session_ref,
        &session.corpus_revision_ref,
        spec,
        loaded,
    )
}

fn text_read_result_for_snapshot(
    codec: &ContinuationCodec,
    snapshot_ref: &str,
    revision_ref: &str,
    spec: &ReadSpec,
    loaded: LoadedText,
) -> ApiResult<ReadItemResult> {
    let request_hash = read_request_hash(spec)?;
    let mut requested_start = 0usize;
    let mut requested_end = loaded.text.len();
    if spec.view == ReadView::Range {
        if let Some(anchor) = spec
            .anchor
            .as_deref()
            .and_then(|value| value.strip_prefix("char:"))
        {
            let char_start = anchor
                .parse::<usize>()
                .map_err(|_| ApiError::invalid("char anchor must be char:<zero-based offset>"))?;
            requested_start = byte_at_char(&loaded.text, char_start).unwrap_or(loaded.text.len());
            let char_end = spec
                .end
                .unwrap_or_else(|| char_start.saturating_add(DEFAULT_READ_CHARS));
            requested_end = byte_at_char(&loaded.text, char_end).unwrap_or(loaded.text.len());
        } else {
            let start_line = spec.start.unwrap_or(1).max(1);
            let end_line = spec.end.unwrap_or(usize::MAX);
            requested_start = byte_at_line(&loaded.text, start_line).unwrap_or(loaded.text.len());
            requested_end = byte_after_line(&loaded.text, end_line).unwrap_or(loaded.text.len());
        }
    }

    if let Some(token) = &spec.continuation_token {
        let continuation = codec.decode(token, snapshot_ref, revision_ref, "read")?;
        if continuation.request_hash != request_hash || continuation.item_id != spec.reference {
            return Err(ApiError::conflict(
                "continuation_mismatch",
                "continuation belongs to another read request",
                json!({"ref": spec.reference}),
            ));
        }
        requested_start = continuation
            .locator
            .get("next_byte")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                ApiError::conflict(
                    "continuation_mismatch",
                    "continuation has no exact next-byte locator",
                    json!({}),
                )
            })?;
        requested_end = continuation
            .locator
            .get("end_byte")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(loaded.text.len());
    }
    requested_start = requested_start.min(loaded.text.len());
    requested_end = requested_end.clamp(requested_start, loaded.text.len());
    if !loaded.text.is_char_boundary(requested_start)
        || !loaded.text.is_char_boundary(requested_end)
    {
        return Err(ApiError::Internal(
            "stored continuation locator is not a UTF-8 boundary".to_owned(),
        ));
    }

    let max_chars = spec.max_chars.unwrap_or(DEFAULT_READ_CHARS).max(1);
    let slice_end = bounded_char_end(&loaded.text, requested_start, requested_end, max_chars);
    let text = loaded.text[requested_start..slice_end].to_owned();
    let truncated = slice_end < requested_end;
    let continuation_token = if truncated {
        Some(codec.issue(
            snapshot_ref.to_owned(),
            revision_ref.to_owned(),
            "read".to_owned(),
            request_hash,
            spec.reference.clone(),
            json!({
                "next_byte": slice_end,
                "next_char": loaded.text[..slice_end].chars().count(),
                "next_line": line_at_byte(&loaded.text, slice_end),
                "end_byte": requested_end
            }),
            Some(format!("{:020}", slice_end)),
        )?)
    } else {
        None
    };
    let returned_chars = text.chars().count();
    Ok(ReadItemResult {
        reference: spec.reference.clone(),
        view: read_view_name(spec.view).to_owned(),
        status: if truncated {
            ItemStatus::Partial
        } else {
            ItemStatus::Complete
        },
        data: Some(json!({
            "metadata": loaded.metadata,
            "text": text,
            "range": {
                "start_byte": requested_start,
                "end_byte_exclusive": slice_end,
                "start_char": loaded.text[..requested_start].chars().count(),
                "end_char_exclusive": loaded.text[..slice_end].chars().count(),
                "start_line": line_at_byte(&loaded.text, requested_start),
                "end_line": line_at_byte(&loaded.text, slice_end)
            },
            "instruction_boundary": "evidence"
        })),
        error: None,
        truncation: Truncation {
            truncated,
            returned_tokens: returned_chars.div_ceil(4),
            limit_tokens: max_chars.div_ceil(4),
            continuation_token,
        },
    })
}

async fn resolve_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    reference: &str,
) -> ApiResult<ResolvedRecord> {
    if let Ok(id) = parse_uuid_ref(reference, &[])
        && let Some(row) = sqlx::query(
            r#"
            SELECT rk.record_kind, cm.record_version
            FROM straylight.record_keys rk
            JOIN straylight.corpus_members cm
              ON cm.user_id = rk.user_id AND cm.record_id = rk.record_id
            WHERE rk.user_id = $1 AND rk.scope_id = $2 AND rk.record_id = $3
              AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(id)
        .bind(session.corpus_revision_id)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok(ResolvedRecord {
            id,
            kind: row.try_get("record_kind")?,
            version: row.try_get("record_version")?,
        });
    }
    let candidates = resolve_reference_candidates_tx(tx, session, reference).await?;
    if candidates.len() > 1 {
        return Err(ApiError::with_details(
            http::StatusCode::CONFLICT,
            "ambiguous_reference",
            "the reference resolves to multiple records",
            json!({"ref": reference, "candidates": candidates}),
        ));
    }
    let candidate = candidates
        .first()
        .ok_or_else(|| ApiError::not_found("ref_not_found", reference))?;
    let id = candidate
        .get("ref")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::not_found("ref_not_found", reference))?;
    let id = parse_uuid_ref(id, &[])?;
    let kind = candidate
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("record")
        .to_owned();
    let version = sqlx::query_scalar(
        "SELECT record_version FROM straylight.corpus_members WHERE user_id = $1 AND scope_id = $2 AND corpus_revision_id = $3 AND record_id = $4",
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    Ok(ResolvedRecord { id, kind, version })
}

async fn load_text_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    session: &PinnedSession,
    record: &ResolvedRecord,
    requested_version: Option<&str>,
) -> ApiResult<LoadedText> {
    match record.kind.as_str() {
        "chunk" => {
            let row = sqlx::query(
                r#"
                SELECT c.content, c.content_hash, c.locator, c.heading_path,
                       c.document_id, c.document_version, c.ordinal,
                       se.source_ref, se.source_version
                FROM straylight.chunks c
                JOIN straylight.corpus_members cm
                  ON cm.user_id = c.user_id AND cm.record_id = c.id
                JOIN straylight.document_revisions dr
                  ON dr.user_id = c.user_id AND dr.document_id = c.document_id
                 AND dr.version = c.document_version
                JOIN straylight.source_episodes se
                  ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
                WHERE c.user_id = $1 AND c.scope_id = $2 AND c.id = $3
                  AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
                "#,
            )
            .bind(session.user_id)
            .bind(session.scope_id)
            .bind(record.id)
            .bind(session.corpus_revision_id)
            .fetch_one(&mut **tx)
            .await?;
            Ok(LoadedText {
                metadata: json!({
                    "ref": record_ref("chunk", record.id),
                    "document_ref": record_ref("document", row.try_get("document_id")?),
                    "document_version": row.try_get::<i32, _>("document_version")?,
                    "ordinal": row.try_get::<i32, _>("ordinal")?,
                    "heading_path": row.try_get::<Vec<String>, _>("heading_path")?,
                    "locator": row.try_get::<Value, _>("locator")?,
                    "source_ref": row.try_get::<String, _>("source_ref")?,
                    "source_version": row.try_get::<Option<String>, _>("source_version")?,
                    "content_hash": prefixed_hash(row.try_get::<String, _>("content_hash")?)
                }),
                text: row.try_get("content")?,
            })
        }
        "document" => document_text_tx(tx, state, session, record, requested_version).await,
        "source_episode" => source_text_tx(tx, state, session, record).await,
        "evidence" => evidence_text_tx(tx, state, session, record).await,
        "object" => {
            let document_id: Option<Uuid> = sqlx::query_scalar(
                r#"
                SELECT d.id FROM straylight.documents d
                JOIN straylight.corpus_members cm
                  ON cm.user_id = d.user_id AND cm.record_id = d.id
                WHERE d.user_id = $1 AND d.scope_id = $2 AND d.object_id = $3
                  AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
                ORDER BY d.id LIMIT 1
                "#,
            )
            .bind(session.user_id)
            .bind(session.scope_id)
            .bind(record.id)
            .bind(session.corpus_revision_id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(document_id) = document_id {
                let document = ResolvedRecord {
                    id: document_id,
                    kind: "document".to_owned(),
                    version: None,
                };
                document_text_tx(tx, state, session, &document, requested_version).await
            } else {
                let value = structured_record_tx(tx, session, record, requested_version).await?;
                Ok(LoadedText {
                    metadata: json!({"ref": record_ref("object", record.id), "representation": "structured_json"}),
                    text: serde_json::to_string_pretty(&value)?,
                })
            }
        }
        _ => {
            let value = structured_record_tx(tx, session, record, requested_version).await?;
            Ok(LoadedText {
                metadata: json!({"ref": record_ref(&record.kind, record.id), "representation": "structured_json"}),
                text: serde_json::to_string_pretty(&value)?,
            })
        }
    }
}

async fn document_text_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    session: &PinnedSession,
    record: &ResolvedRecord,
    requested_version: Option<&str>,
) -> ApiResult<LoadedText> {
    let version = parse_version(requested_version, record.version)?;
    let row = sqlx::query(
        r#"
        SELECT dr.version, dr.title, dr.media_type, dr.content_hash,
               dr.native_locator, dr.byte_size, se.source_ref, se.source_version,
               dr.asset_id,dr.asset_version,av.object_key,av.object_version_id,
               av.size_bytes AS asset_size,av.content_hash AS asset_hash
        FROM straylight.documents d
        JOIN straylight.corpus_members cm
          ON cm.user_id = d.user_id AND cm.record_id = d.id
        JOIN straylight.document_revisions dr
          ON dr.user_id = d.user_id AND dr.document_id = d.id
         AND dr.version = $5
        JOIN straylight.source_episodes se
          ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        LEFT JOIN straylight.asset_versions av
          ON av.user_id = dr.user_id AND av.asset_id = dr.asset_id
         AND av.version = dr.asset_version
        WHERE d.user_id = $1 AND d.scope_id = $2 AND d.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
          AND dr.version <= coalesce(cm.record_version, dr.version)
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            "version_not_found",
            &format!("{}@v{version}", record_ref("document", record.id)),
        )
    })?;
    let media_type: String = row.try_get("media_type")?;
    let object_key: Option<String> = row.try_get("object_key")?;
    let object_version_id: Option<String> = row.try_get("object_version_id")?;
    let asset_hash: Option<String> = row.try_get("asset_hash")?;
    let mut representation = "normalized_chunks";
    let mut integrity_warning = None;
    let text = if is_text_media_type(&media_type) {
        if let Some(key) = object_key.as_deref() {
            match state
                .object_store
                .get_version(key, object_version_id.as_deref())
                .await
            {
                Ok(bytes) => {
                    if !native_asset_hash_matches(asset_hash.as_deref(), &bytes) {
                        metrics::counter!(
                            "object_store.integrity_failures",
                            "failure" => "content_hash_mismatch"
                        )
                        .increment(1);
                        integrity_warning = Some("native_asset_hash_mismatch");
                        reconstruct_document_chunks_tx(tx, session, record.id, version).await?
                    } else {
                        match String::from_utf8(bytes.to_vec()) {
                            Ok(text) => {
                                representation = "native_asset";
                                text
                            }
                            Err(_) => {
                                metrics::counter!(
                                    "object_store.integrity_failures",
                                    "failure" => "invalid_utf8"
                                )
                                .increment(1);
                                integrity_warning = Some("native_asset_invalid_utf8");
                                reconstruct_document_chunks_tx(tx, session, record.id, version)
                                    .await?
                            }
                        }
                    }
                }
                Err(_) => {
                    metrics::counter!(
                        "object_store.integrity_failures",
                        "failure" => "unavailable"
                    )
                    .increment(1);
                    integrity_warning = Some("native_asset_unavailable");
                    reconstruct_document_chunks_tx(tx, session, record.id, version).await?
                }
            }
        } else {
            reconstruct_document_chunks_tx(tx, session, record.id, version).await?
        }
    } else {
        reconstruct_document_chunks_tx(tx, session, record.id, version).await?
    };
    let mut metadata = json!({
        "ref": record_ref("document", record.id),
        "version": version,
        "title": row.try_get::<Option<String>, _>("title")?,
        "media_type": media_type,
        "content_hash": prefixed_hash(row.try_get::<String, _>("content_hash")?),
        "native_locator": row.try_get::<Value, _>("native_locator")?,
        "byte_size": row.try_get::<i64, _>("byte_size")?,
        "source_ref": row.try_get::<String, _>("source_ref")?,
        "source_version": row.try_get::<Option<String>, _>("source_version")?,
        "representation": representation,
        "native_asset": row.try_get::<Option<Uuid>, _>("asset_id")?.map(|asset_id| json!({
            "asset_ref": record_ref("asset", asset_id),
            "version": row.try_get::<Option<i32>, _>("asset_version").ok().flatten(),
            "size_bytes": row.try_get::<Option<i64>, _>("asset_size").ok().flatten(),
            "content_hash": asset_hash.map(prefixed_hash),
            "inline": is_text_media_type(&media_type)
        }))
    });
    if let Some(warning) = integrity_warning {
        metadata
            .as_object_mut()
            .expect("document metadata is an object")
            .insert(
                "integrity_warning".to_owned(),
                Value::String(warning.to_owned()),
            );
    }
    Ok(LoadedText { metadata, text })
}

fn native_asset_hash_matches(expected_hash: Option<&str>, bytes: &[u8]) -> bool {
    let Some(expected_hash) = expected_hash else {
        return false;
    };
    let actual_hash = hex::encode(Sha256::digest(bytes));
    subtle::ConstantTimeEq::ct_eq(expected_hash.as_bytes(), actual_hash.as_bytes()).into()
}

async fn reconstruct_document_chunks_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    document_id: Uuid,
    version: i32,
) -> ApiResult<String> {
    let rows = sqlx::query(
        r#"
        SELECT c.ordinal, c.locator, c.content
        FROM straylight.chunks c
        JOIN straylight.corpus_members cm
          ON cm.user_id = c.user_id AND cm.record_id = c.id
        WHERE c.user_id = $1 AND c.scope_id = $2
          AND c.document_id = $3 AND c.document_version = $4
          AND cm.corpus_revision_id = $5 AND cm.disposition = 'active'
        ORDER BY c.ordinal
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(document_id)
    .bind(version)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut lines = BTreeMap::<usize, String>::new();
    let mut fallback = Vec::new();
    for row in rows {
        let content: String = row.try_get("content")?;
        let locator: Value = row.try_get("locator")?;
        if let Some(start) = locator.get("start_line").and_then(Value::as_u64) {
            let start = usize::try_from(start).unwrap_or(1).max(1);
            for (offset, line) in content.split('\n').enumerate() {
                lines
                    .entry(start + offset)
                    .or_insert_with(|| line.to_owned());
            }
        } else {
            fallback.push(content);
        }
    }
    if lines.is_empty() {
        Ok(fallback.join("\n"))
    } else {
        Ok(lines.into_values().collect::<Vec<_>>().join("\n"))
    }
}

async fn source_text_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    session: &PinnedSession,
    record: &ResolvedRecord,
) -> ApiResult<LoadedText> {
    let row = sqlx::query(
        r#"
        SELECT se.source_ref, se.source_version, se.title, se.source_kind,
               se.content_hash, se.native_locator, se.captured_at,
               av.object_key,av.object_version_id,av.media_type,
               (
                 SELECT coalesce(
                   jsonb_agg(
                     jsonb_build_object(
                       'role',link.role,
                       'asset_ref','asset:' || link.asset_id::text,
                       'version',link.asset_version
                     )
                     ORDER BY link.role,link.asset_id,link.asset_version
                   ),
                   '[]'::jsonb
                 )
                 FROM straylight.source_asset_links AS link
                 WHERE link.user_id=se.user_id
                   AND link.source_episode_id=se.id
               ) AS asset_handles
        FROM straylight.source_episodes se
        JOIN straylight.corpus_members cm
          ON cm.user_id = se.user_id AND cm.record_id = se.id
        LEFT JOIN LATERAL (
          SELECT av0.object_key,av0.object_version_id,av0.media_type
          FROM straylight.source_asset_links sal
          JOIN straylight.asset_versions av0
            ON av0.user_id = sal.user_id AND av0.asset_id = sal.asset_id
           AND av0.version = sal.asset_version
          WHERE sal.user_id = se.user_id AND sal.source_episode_id = se.id
            AND sal.role='source_native'
          ORDER BY sal.asset_id LIMIT 1
        ) av ON true
        WHERE se.user_id = $1 AND se.scope_id = $2 AND se.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .fetch_one(&mut **tx)
    .await?;
    let key: Option<String> = row.try_get("object_key")?;
    let object_version_id: Option<String> = row.try_get("object_version_id")?;
    let media_type: Option<String> = row.try_get("media_type")?;
    let text = if key.is_some() && media_type.as_deref().is_some_and(is_text_media_type) {
        state
            .object_store
            .get_version(
                key.as_deref().unwrap_or_default(),
                object_version_id.as_deref(),
            )
            .await
            .ok()
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
    } else {
        None
    };
    let text = if let Some(text) = text {
        text
    } else {
        let parts: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT ei.text_content FROM straylight.evidence_items ei
            JOIN straylight.corpus_members cm
              ON cm.user_id = ei.user_id AND cm.record_id = ei.id
            WHERE ei.user_id = $1 AND ei.scope_id = $2 AND ei.source_episode_id = $3
              AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
              AND ei.text_content IS NOT NULL
            ORDER BY ei.locator::text, ei.created_at, ei.id
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(record.id)
        .bind(session.corpus_revision_id)
        .fetch_all(&mut **tx)
        .await?;
        parts.join("\n")
    };
    Ok(LoadedText {
        metadata: json!({
            "ref": record_ref("source_episode", record.id),
            "source_ref": row.try_get::<String, _>("source_ref")?,
            "source_version": row.try_get::<Option<String>, _>("source_version")?,
            "title": row.try_get::<Option<String>, _>("title")?,
            "source_kind": row.try_get::<String, _>("source_kind")?,
            "content_hash": row.try_get::<Option<String>, _>("content_hash")?.map(prefixed_hash),
            "native_locator": row.try_get::<Value, _>("native_locator")?,
            "captured_at": row.try_get::<DateTime<Utc>, _>("captured_at")?,
            "native_assets": row.try_get::<Value, _>("asset_handles")?
        }),
        text,
    })
}

async fn evidence_text_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    session: &PinnedSession,
    record: &ResolvedRecord,
) -> ApiResult<LoadedText> {
    let row = sqlx::query(
        r#"
        SELECT ei.text_content, ei.content_hash, ei.locator, ei.evidence_kind,
               ei.observed_at, se.source_ref, se.source_version,
               ei.asset_id,ei.asset_version,av.object_key,av.object_version_id,
               av.media_type,
               av.size_bytes AS asset_size
        FROM straylight.evidence_items ei
        JOIN straylight.corpus_members cm
          ON cm.user_id = ei.user_id AND cm.record_id = ei.id
        JOIN straylight.source_episodes se
          ON se.user_id = ei.user_id AND se.id = ei.source_episode_id
        LEFT JOIN straylight.asset_versions av
          ON av.user_id = ei.user_id AND av.asset_id = ei.asset_id
         AND av.version = ei.asset_version
        WHERE ei.user_id = $1 AND ei.scope_id = $2 AND ei.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .fetch_one(&mut **tx)
    .await?;
    let mut text: Option<String> = row.try_get("text_content")?;
    let key: Option<String> = row.try_get("object_key")?;
    let object_version_id: Option<String> = row.try_get("object_version_id")?;
    let media_type: Option<String> = row.try_get("media_type")?;
    if text.is_none() && key.is_some() && media_type.as_deref().is_some_and(is_text_media_type) {
        text = state
            .object_store
            .get_version(
                key.as_deref().unwrap_or_default(),
                object_version_id.as_deref(),
            )
            .await
            .ok()
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok());
    }
    let text = text.ok_or_else(|| {
        ApiError::public(
            http::StatusCode::UNPROCESSABLE_ENTITY,
            "native_binary_required",
            "binary evidence cannot be inlined as model context; use its native asset handle",
        )
    })?;
    Ok(LoadedText {
        metadata: json!({
            "ref": record_ref("evidence", record.id),
            "evidence_kind": row.try_get::<String, _>("evidence_kind")?,
            "source_ref": row.try_get::<String, _>("source_ref")?,
            "source_version": row.try_get::<Option<String>, _>("source_version")?,
            "content_hash": prefixed_hash(row.try_get::<String, _>("content_hash")?),
            "locator": row.try_get::<Value, _>("locator")?,
            "observed_at": row.try_get::<Option<DateTime<Utc>>, _>("observed_at")?,
            "native_asset": row.try_get::<Option<Uuid>, _>("asset_id")?.map(|asset_id| json!({
                "asset_ref": record_ref("asset", asset_id),
                "version": row.try_get::<Option<i32>, _>("asset_version").ok().flatten(),
                "size_bytes": row.try_get::<Option<i64>, _>("asset_size").ok().flatten(),
                "inline": false
            }))
        }),
        text,
    })
}

async fn structured_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
    requested_version: Option<&str>,
) -> ApiResult<Value> {
    match record.kind.as_str() {
        "object" => object_state_tx(tx, session, record, requested_version).await,
        "claim" => claim_record_tx(tx, session, record.id).await,
        "relation" => relation_record_tx(tx, session, record, requested_version).await,
        "checkpoint" => checkpoint_tx(tx, session, record.id).await,
        "document" => document_record_tx(tx, session, record, requested_version).await,
        "source_episode" => source_record_tx(tx, session, record.id).await,
        "evidence" => evidence_record_tx(tx, session, record.id).await,
        "temporal_spec" => temporal_record_tx(tx, session, record.id).await,
        _ => Ok(json!({
            "ref": record_ref(&record.kind, record.id),
            "kind": record.kind,
            "version": record.version,
            "message": "record is addressable but has no richer structured projection"
        })),
    }
}

async fn object_state_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
    requested_version: Option<&str>,
) -> ApiResult<Value> {
    let version = parse_version(requested_version, record.version)?;
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'ref', 'object:' || replace(o.id::text, '-', ''),
          'version', orev.version, 'label', orev.label, 'properties', orev.properties,
          'recorded_at', orev.recorded_at,
          'type_profiles', ARRAY(
            SELECT p.profile_ref::text FROM straylight.object_revision_profiles p
            WHERE p.user_id = orev.user_id AND p.object_id = orev.object_id
              AND p.object_version = orev.version ORDER BY p.profile_ref
          ),
          'states', coalesce((
            SELECT jsonb_agg(state_row.payload ORDER BY state_row.machine)
            FROM (
              SELECT DISTINCT ON (sa.state_machine_ref)
                sa.state_machine_ref::text AS machine,
                jsonb_build_object(
                  'machine_ref', sa.state_machine_ref::text, 'value', sa.state_value,
                  'claim_ref', 'claim:' || replace(sa.claim_id::text, '-', ''),
                  'recorded_at', c.recorded_at
                ) AS payload
              FROM straylight.state_assignments sa
              JOIN straylight.claims c ON c.user_id = sa.user_id AND c.id = sa.claim_id
              JOIN straylight.corpus_members scm
                ON scm.user_id = sa.user_id AND scm.record_id = sa.claim_id
              WHERE sa.user_id = o.user_id AND sa.target_record_id = o.id
                AND scm.corpus_revision_id = $4 AND scm.disposition = 'active'
              ORDER BY sa.state_machine_ref, c.recorded_at DESC, c.id DESC
            ) state_row
          ), '[]'::jsonb),
          'claims', coalesce((
            SELECT jsonb_agg(jsonb_build_object(
              'claim_ref', 'claim:' || replace(c.id::text, '-', ''),
              'predicate', c.predicate::text, 'value', c.value,
              'authority', c.authority, 'canonicality', c.canonicality,
              'support_state', c.support_state, 'recorded_at', c.recorded_at
            ) ORDER BY c.recorded_at DESC)
            FROM straylight.claim_about_targets cat
            JOIN straylight.claims c ON c.user_id = cat.user_id AND c.id = cat.claim_id
            JOIN straylight.corpus_members ccm
              ON ccm.user_id = c.user_id AND ccm.record_id = c.id
            WHERE cat.user_id = o.user_id AND cat.target_record_id = o.id
              AND ccm.corpus_revision_id = $4 AND ccm.disposition = 'active'
              AND NOT EXISTS (
                SELECT 1 FROM straylight.claim_lineage successor
                JOIN straylight.corpus_members successor_cm
                  ON successor_cm.user_id = successor.user_id
                 AND successor_cm.record_id = successor.claim_id
                WHERE successor.user_id = c.user_id
                  AND successor.predecessor_claim_id = c.id
                  AND successor.lineage_kind IN ('corrects', 'supersedes', 'retracts')
                  AND successor_cm.corpus_revision_id = $4
                  AND successor_cm.disposition = 'active'
              )
          ), '[]'::jsonb)
        )
        FROM straylight.objects o
        JOIN straylight.corpus_members cm
          ON cm.user_id = o.user_id AND cm.record_id = o.id
        JOIN straylight.object_revisions orev
          ON orev.user_id = o.user_id AND orev.object_id = o.id AND orev.version = $5
        WHERE o.user_id = $1 AND o.scope_id = $2 AND o.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
          AND orev.version <= coalesce(cm.record_version, orev.version)
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            "version_not_found",
            &format!("{}@v{version}", record_ref("object", record.id)),
        )
    })
}

async fn claim_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    claim_id: Uuid,
) -> ApiResult<Value> {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'ref', 'claim:' || replace(c.id::text, '-', ''),
          'about_refs', ARRAY(SELECT rk.record_kind || ':' || replace(cat.target_record_id::text, '-', '')
            FROM straylight.claim_about_targets cat
            JOIN straylight.record_keys rk ON rk.user_id = cat.user_id AND rk.record_id = cat.target_record_id
            WHERE cat.user_id = c.user_id AND cat.claim_id = c.id ORDER BY cat.role, cat.target_record_id),
          'predicate', c.predicate::text, 'value', c.value,
          'formation_method', c.formation_method, 'claim_mode', c.claim_mode,
          'support_state', c.support_state, 'authority', c.authority,
          'canonicality', c.canonicality, 'confidence', c.confidence,
          'valid_time', to_jsonb(valid_time) - 'user_id' - 'scope_id',
          'observed_time', to_jsonb(observed_time) - 'user_id' - 'scope_id',
          'recorded_at', c.recorded_at,
          'evidence_refs', ARRAY(SELECT 'evidence:' || replace(link.evidence_id::text, '-', '')
            FROM straylight.claim_evidence_links link WHERE link.user_id = c.user_id AND link.claim_id = c.id
            ORDER BY link.support_kind, link.evidence_id),
          'lineage', coalesce((SELECT jsonb_agg(jsonb_build_object(
              'kind', lineage.lineage_kind,
              'predecessor_ref', 'claim:' || replace(lineage.predecessor_claim_id::text, '-', ''),
              'evidence_ref', CASE WHEN lineage.evidence_id IS NULL THEN NULL ELSE 'evidence:' || replace(lineage.evidence_id::text, '-', '') END
            ) ORDER BY lineage.lineage_kind, lineage.predecessor_claim_id)
            FROM straylight.claim_lineage lineage WHERE lineage.user_id = c.user_id AND lineage.claim_id = c.id), '[]'::jsonb)
        )
        FROM straylight.claims c
        JOIN straylight.corpus_members cm ON cm.user_id = c.user_id AND cm.record_id = c.id
        LEFT JOIN straylight.temporal_specs valid_time ON valid_time.user_id = c.user_id AND valid_time.id = c.valid_temporal_id
        LEFT JOIN straylight.temporal_specs observed_time ON observed_time.user_id = c.user_id AND observed_time.id = c.observed_temporal_id
        WHERE c.user_id = $1 AND c.scope_id = $2 AND c.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(claim_id)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("ref_not_found", &record_ref("claim", claim_id)))
}

async fn relation_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
    requested_version: Option<&str>,
) -> ApiResult<Value> {
    let version = parse_version(requested_version, record.version)?;
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'ref', 'relation:' || replace(r.id::text, '-', ''), 'version', rr.version,
          'predicate', rr.predicate::text, 'qualifiers', rr.qualifiers,
          'valid_time', to_jsonb(ts) - 'user_id' - 'scope_id',
          'recorded_at', rr.recorded_at,
          'endpoints', coalesce((SELECT jsonb_agg(jsonb_build_object(
            'role', ep.role, 'position', ep.position,
            'target_ref', rk.record_kind || ':' || replace(ep.target_record_id::text, '-', '')
          ) ORDER BY ep.role, ep.position)
          FROM straylight.relation_endpoints ep
          JOIN straylight.record_keys rk ON rk.user_id = ep.user_id AND rk.record_id = ep.target_record_id
          WHERE ep.user_id = rr.user_id AND ep.relation_id = rr.relation_id AND ep.relation_version = rr.version), '[]'::jsonb),
          'evidence_refs', ARRAY(SELECT 'evidence:' || replace(rel.evidence_id::text, '-', '')
            FROM straylight.relation_evidence_links rel
            WHERE rel.user_id = rr.user_id AND rel.relation_id = rr.relation_id AND rel.relation_version = rr.version
            ORDER BY rel.support_kind, rel.evidence_id)
        )
        FROM straylight.relations r
        JOIN straylight.corpus_members cm ON cm.user_id = r.user_id AND cm.record_id = r.id
        JOIN straylight.relation_revisions rr ON rr.user_id = r.user_id AND rr.relation_id = r.id AND rr.version = $5
        LEFT JOIN straylight.temporal_specs ts ON ts.user_id = rr.user_id AND ts.id = rr.valid_temporal_id
        WHERE r.user_id = $1 AND r.scope_id = $2 AND r.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
          AND rr.version <= coalesce(cm.record_version, rr.version)
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("version_not_found", &record_ref("relation", record.id)))
}

async fn document_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
    requested_version: Option<&str>,
) -> ApiResult<Value> {
    let version = parse_version(requested_version, record.version)?;
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'ref', 'document:' || replace(d.id::text, '-', ''), 'version', dr.version,
          'object_ref', CASE WHEN d.object_id IS NULL THEN NULL ELSE 'object:' || replace(d.object_id::text, '-', '') END,
          'title', dr.title, 'media_type', dr.media_type,
          'content_hash', 'sha256:' || dr.content_hash, 'native_locator', dr.native_locator,
          'byte_size', dr.byte_size, 'recorded_at', dr.recorded_at,
          'source_ref', se.source_ref, 'source_version', se.source_version,
          'chunks', coalesce((SELECT jsonb_agg(jsonb_build_object(
            'ref', 'chunk:' || replace(c.id::text, '-', ''), 'ordinal', c.ordinal,
            'heading_path', c.heading_path, 'locator', c.locator,
            'content_hash', 'sha256:' || c.content_hash, 'token_count', c.token_count
          ) ORDER BY c.ordinal) FROM straylight.chunks c
          JOIN straylight.corpus_members ccm ON ccm.user_id = c.user_id AND ccm.record_id = c.id
          WHERE c.user_id = dr.user_id AND c.document_id = dr.document_id AND c.document_version = dr.version
            AND ccm.corpus_revision_id = $4 AND ccm.disposition = 'active'), '[]'::jsonb)
        )
        FROM straylight.documents d
        JOIN straylight.corpus_members cm ON cm.user_id = d.user_id AND cm.record_id = d.id
        JOIN straylight.document_revisions dr ON dr.user_id = d.user_id AND dr.document_id = d.id AND dr.version = $5
        JOIN straylight.source_episodes se ON se.user_id = dr.user_id AND se.id = dr.source_episode_id
        WHERE d.user_id = $1 AND d.scope_id = $2 AND d.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
          AND dr.version <= coalesce(cm.record_version, dr.version)
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("version_not_found", &record_ref("document", record.id)))
}

async fn source_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    id: Uuid,
) -> ApiResult<Value> {
    sqlx::query_scalar(
        r#"
        SELECT to_jsonb(se) - 'user_id' - 'scope_id' - 'policy_id' || jsonb_build_object(
          'ref', 'source_episode:' || replace(se.id::text, '-', ''),
          'content_hash', CASE WHEN se.content_hash IS NULL THEN NULL ELSE 'sha256:' || se.content_hash END
        )
        FROM straylight.source_episodes se
        JOIN straylight.corpus_members cm ON cm.user_id = se.user_id AND cm.record_id = se.id
        WHERE se.user_id = $1 AND se.scope_id = $2 AND se.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(id)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("ref_not_found", &record_ref("source_episode", id)))
}

async fn evidence_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    id: Uuid,
) -> ApiResult<Value> {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'ref', 'evidence:' || replace(ei.id::text, '-', ''),
          'kind', ei.evidence_kind, 'locator', ei.locator,
          'text_content', ei.text_content, 'content_hash', 'sha256:' || ei.content_hash,
          'observed_at', ei.observed_at, 'created_at', ei.created_at,
          'source_ref', se.source_ref, 'source_version', se.source_version,
          'asset_ref', CASE WHEN ei.asset_id IS NULL THEN NULL ELSE 'asset:' || replace(ei.asset_id::text, '-', '') END,
          'asset_version', ei.asset_version, 'instruction_boundary', 'evidence'
        )
        FROM straylight.evidence_items ei
        JOIN straylight.corpus_members cm ON cm.user_id = ei.user_id AND cm.record_id = ei.id
        JOIN straylight.source_episodes se ON se.user_id = ei.user_id AND se.id = ei.source_episode_id
        WHERE ei.user_id = $1 AND ei.scope_id = $2 AND ei.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(id)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("ref_not_found", &record_ref("evidence", id)))
}

async fn temporal_record_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    id: Uuid,
) -> ApiResult<Value> {
    sqlx::query_scalar(
        r#"
        SELECT to_jsonb(ts) - 'user_id' - 'scope_id' ||
               jsonb_build_object('ref', 'temporal_spec:' || replace(ts.id::text, '-', ''))
        FROM straylight.temporal_specs ts
        JOIN straylight.corpus_members cm ON cm.user_id = ts.user_id AND cm.record_id = ts.id
        WHERE ts.user_id = $1 AND ts.scope_id = $2 AND ts.id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(id)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("ref_not_found", &record_ref("temporal_spec", id)))
}

async fn outline_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
) -> ApiResult<Value> {
    if record.kind == "document" {
        let version = record.version.ok_or_else(|| {
            ApiError::not_found("version_not_found", &record_ref("document", record.id))
        })?;
        let rows: Vec<Value> = sqlx::query_scalar(
            r#"
            SELECT jsonb_build_object(
              'chunk_ref', 'chunk:' || replace(c.id::text, '-', ''),
              'ordinal', c.ordinal, 'kind', c.chunk_kind,
              'heading_path', c.heading_path, 'locator', c.locator,
              'token_count', c.token_count
            )
            FROM straylight.chunks c
            JOIN straylight.corpus_members cm ON cm.user_id = c.user_id AND cm.record_id = c.id
            WHERE c.user_id = $1 AND c.scope_id = $2
              AND c.document_id = $3 AND c.document_version = $4
              AND cm.corpus_revision_id = $5 AND cm.disposition = 'active'
            ORDER BY c.ordinal
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(record.id)
        .bind(version)
        .bind(session.corpus_revision_id)
        .fetch_all(&mut **tx)
        .await?;
        Ok(json!({"ref": record_ref("document", record.id), "version": version, "outline": rows}))
    } else {
        let value = structured_record_tx(tx, session, record, None).await?;
        Ok(json!({
            "ref": record_ref(&record.kind, record.id),
            "keys": value.as_object().map(|object| object.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
        }))
    }
}

async fn neighbors_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
    spec: &ReadSpec,
) -> ApiResult<Value> {
    let anchor_id = spec
        .anchor
        .as_deref()
        .map(|value| parse_uuid_ref(value, &["chunk"]))
        .transpose()?
        .or((record.kind == "chunk").then_some(record.id))
        .ok_or_else(|| ApiError::invalid("neighbors requires a chunk ref or chunk anchor"))?;
    let before = i32::try_from(spec.before.unwrap_or(2).min(50)).unwrap_or(50);
    let after = i32::try_from(spec.after.unwrap_or(2).min(50)).unwrap_or(50);
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        WITH anchor AS (
          SELECT document_id, document_version, ordinal FROM straylight.chunks
          WHERE user_id = $1 AND scope_id = $2 AND id = $3
        )
        SELECT jsonb_build_object(
          'ref', 'chunk:' || replace(c.id::text, '-', ''), 'ordinal', c.ordinal,
          'heading_path', c.heading_path, 'locator', c.locator,
          'content', c.content, 'content_hash', 'sha256:' || c.content_hash,
          'is_anchor', c.id = $3
        )
        FROM anchor a JOIN straylight.chunks c
          ON c.user_id = $1 AND c.document_id = a.document_id AND c.document_version = a.document_version
        JOIN straylight.corpus_members cm ON cm.user_id = c.user_id AND cm.record_id = c.id
        WHERE c.ordinal BETWEEN a.ordinal - $5 AND a.ordinal + $6
          AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
        ORDER BY c.ordinal
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(anchor_id)
    .bind(session.corpus_revision_id)
    .bind(before)
    .bind(after)
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Err(ApiError::not_found(
            "ref_not_found",
            &record_ref("chunk", anchor_id),
        ));
    }
    Ok(json!({"anchor_ref": record_ref("chunk", anchor_id), "chunks": rows}))
}

async fn relationships_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
) -> ApiResult<Value> {
    let relations: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'relation_ref', 'relation:' || replace(r.id::text, '-', ''),
          'version', rr.version, 'predicate', rr.predicate::text,
          'qualifiers', rr.qualifiers,
          'endpoints', jsonb_agg(jsonb_build_object(
            'role', all_ep.role, 'position', all_ep.position,
            'target_ref', rk.record_kind || ':' || replace(all_ep.target_record_id::text, '-', '')
          ) ORDER BY all_ep.role, all_ep.position)
        )
        FROM straylight.relation_endpoints selected_ep
        JOIN straylight.relations r ON r.user_id = selected_ep.user_id AND r.id = selected_ep.relation_id
        JOIN straylight.corpus_members cm ON cm.user_id = r.user_id AND cm.record_id = r.id
        JOIN straylight.relation_revisions rr ON rr.user_id = r.user_id AND rr.relation_id = r.id AND rr.version = cm.record_version
        JOIN straylight.relation_endpoints all_ep ON all_ep.user_id = rr.user_id AND all_ep.relation_id = rr.relation_id AND all_ep.relation_version = rr.version
        JOIN straylight.record_keys rk ON rk.user_id = all_ep.user_id AND rk.record_id = all_ep.target_record_id
        WHERE selected_ep.user_id = $1 AND r.scope_id = $2 AND selected_ep.target_record_id = $3
          AND selected_ep.relation_version = rr.version
          AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
        GROUP BY r.id, rr.version, rr.predicate, rr.qualifiers, rr.recorded_at
        ORDER BY rr.recorded_at DESC, r.id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(record.id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(json!({"ref": record_ref(&record.kind, record.id), "relations": relations}))
}

async fn history_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
) -> ApiResult<Value> {
    let history: Vec<Value> = match record.kind.as_str() {
        "object" => sqlx::query_scalar(
            r#"SELECT jsonb_build_object('version', version, 'previous_version', previous_version, 'label', label, 'properties', properties, 'recorded_at', recorded_at)
               FROM straylight.object_revisions WHERE user_id = $1 AND object_id = $2 AND version <= $3 ORDER BY version"#,
        )
        .bind(session.user_id)
        .bind(record.id)
        .bind(record.version.unwrap_or(i32::MAX))
        .fetch_all(&mut **tx)
        .await?,
        "document" => sqlx::query_scalar(
            r#"SELECT jsonb_build_object('version', version, 'previous_version', previous_version, 'title', title, 'media_type', media_type, 'content_hash', 'sha256:' || content_hash, 'native_locator', native_locator, 'recorded_at', recorded_at)
               FROM straylight.document_revisions WHERE user_id = $1 AND document_id = $2 AND version <= $3 ORDER BY version"#,
        )
        .bind(session.user_id)
        .bind(record.id)
        .bind(record.version.unwrap_or(i32::MAX))
        .fetch_all(&mut **tx)
        .await?,
        "relation" => sqlx::query_scalar(
            r#"SELECT jsonb_build_object('version', version, 'previous_version', previous_version, 'predicate', predicate::text, 'qualifiers', qualifiers, 'recorded_at', recorded_at)
               FROM straylight.relation_revisions WHERE user_id = $1 AND relation_id = $2 AND version <= $3 ORDER BY version"#,
        )
        .bind(session.user_id)
        .bind(record.id)
        .bind(record.version.unwrap_or(i32::MAX))
        .fetch_all(&mut **tx)
        .await?,
        "claim" => sqlx::query_scalar(
            r#"
            SELECT jsonb_build_object(
              'claim_ref', 'claim:' || replace(c.id::text, '-', ''),
              'predicate', c.predicate::text, 'value', c.value,
              'support_state', c.support_state, 'recorded_at', c.recorded_at,
              'lineage_kind', lineage.lineage_kind
            )
            FROM straylight.claims c
            JOIN straylight.corpus_members cm ON cm.user_id = c.user_id AND cm.record_id = c.id
            LEFT JOIN straylight.claim_lineage lineage
              ON lineage.user_id = c.user_id AND lineage.claim_id = c.id
            WHERE c.user_id = $1 AND c.scope_id = $2
              AND cm.corpus_revision_id = $4
              AND (c.id = $3 OR lineage.predecessor_claim_id = $3)
            ORDER BY c.recorded_at, c.id
            "#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(record.id)
        .bind(session.corpus_revision_id)
        .fetch_all(&mut **tx)
        .await?,
        "checkpoint" => sqlx::query_scalar(
            r#"SELECT jsonb_build_object('checkpoint_ref', 'checkpoint:' || replace(id::text, '-', ''), 'parent_checkpoint_ref', CASE WHEN parent_checkpoint_id IS NULL THEN NULL ELSE 'checkpoint:' || replace(parent_checkpoint_id::text, '-', '') END, 'title', title, 'summary', summary, 'created_at', created_at)
               FROM straylight.checkpoints WHERE user_id = $1 AND scope_id = $2 AND (id = $3 OR parent_checkpoint_id = $3) ORDER BY created_at"#,
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(record.id)
        .fetch_all(&mut **tx)
        .await?,
        _ => Vec::new(),
    };
    Ok(json!({"ref": record_ref(&record.kind, record.id), "history": history}))
}

async fn diff_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    session: &PinnedSession,
    record: &ResolvedRecord,
    spec: &ReadSpec,
) -> ApiResult<Value> {
    if record.kind == "claim" {
        let predecessor: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT lineage.predecessor_claim_id
            FROM straylight.claim_lineage lineage
            WHERE lineage.user_id = $1 AND lineage.claim_id = $2
            ORDER BY CASE lineage.lineage_kind
              WHEN 'corrects' THEN 0 WHEN 'supersedes' THEN 1 WHEN 'retracts' THEN 2 ELSE 3 END,
              lineage.predecessor_claim_id
            LIMIT 1
            "#,
        )
        .bind(session.user_id)
        .bind(record.id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(predecessor) = predecessor else {
            return Ok(json!({
                "ref": record_ref("claim", record.id),
                "unsupported": true,
                "reason": "immutable claim has no correction, supersession, retraction, contradiction, or derivation predecessor"
            }));
        };
        let left = claim_record_tx(tx, session, predecessor).await?;
        let right = claim_record_tx(tx, session, record.id).await?;
        return Ok(json!({
            "from_ref": record_ref("claim", predecessor),
            "to_ref": record_ref("claim", record.id),
            "patch": json_patch::diff(&left, &right)
        }));
    }
    if record.kind == "checkpoint" {
        let parent: Option<Uuid> = sqlx::query_scalar(
            "SELECT parent_checkpoint_id FROM straylight.checkpoints WHERE user_id = $1 AND scope_id = $2 AND id = $3",
        )
        .bind(session.user_id)
        .bind(session.scope_id)
        .bind(record.id)
        .fetch_optional(&mut **tx)
        .await?
        .flatten();
        let Some(parent) = parent else {
            return Ok(json!({
                "ref": record_ref("checkpoint", record.id),
                "unsupported": true,
                "reason": "checkpoint has no parent"
            }));
        };
        let left = checkpoint_tx(tx, session, parent).await?;
        let right = checkpoint_tx(tx, session, record.id).await?;
        return Ok(json!({
            "from_ref": record_ref("checkpoint", parent),
            "to_ref": record_ref("checkpoint", record.id),
            "patch": json_patch::diff(&left, &right)
        }));
    }
    if record.version.is_none() {
        return Ok(json!({
            "ref": record_ref(&record.kind, record.id),
            "unsupported": true,
            "reason": "record kind is immutable and has no version or lineage diff surface"
        }));
    }
    let from = parse_version(
        spec.from_version.as_deref(),
        Some(record.version.unwrap_or(1).saturating_sub(1).max(1)),
    )?;
    let to = parse_version(spec.to_version.as_deref(), record.version)?;
    if from > to {
        return Err(ApiError::invalid(
            "diff from_version must not exceed to_version",
        ));
    }
    if record.kind == "document" {
        let from_record = ResolvedRecord {
            version: Some(from),
            ..record.clone()
        };
        let to_record = ResolvedRecord {
            version: Some(to),
            ..record.clone()
        };
        let left =
            document_text_tx(tx, state, session, &from_record, Some(&format!("v{from}"))).await?;
        let right =
            document_text_tx(tx, state, session, &to_record, Some(&format!("v{to}"))).await?;
        Ok(json!({
            "ref": record_ref("document", record.id), "from_version": from, "to_version": to,
            "line_changes": line_diff(&left.text, &right.text),
            "from_content_hash": left.metadata.get("content_hash"),
            "to_content_hash": right.metadata.get("content_hash")
        }))
    } else {
        let left = structured_record_tx(tx, session, record, Some(&format!("v{from}"))).await?;
        let right = structured_record_tx(tx, session, record, Some(&format!("v{to}"))).await?;
        let patch = json_patch::diff(&left, &right);
        Ok(json!({
            "ref": record_ref(&record.kind, record.id), "from_version": from,
            "to_version": to, "patch": patch
        }))
    }
}

async fn last_known_good_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    record: &ResolvedRecord,
) -> ApiResult<Value> {
    let version = record.version;
    let data = structured_record_tx(
        tx,
        session,
        record,
        version.map(|value| format!("v{value}")).as_deref(),
    )
    .await?;
    Ok(json!({
        "selection": "latest_integrity_valid_version_at_pinned_revision",
        "note": "The schema has immutable hashes and provenance but no independent last-known-good marker; this is not a quality certification.",
        "version": version,
        "data": data
    }))
}

fn parse_version(requested: Option<&str>, pinned: Option<i32>) -> ApiResult<i32> {
    let pinned =
        pinned.ok_or_else(|| ApiError::invalid("record has no version at this revision"))?;
    let Some(requested) = requested else {
        return Ok(pinned);
    };
    if matches!(requested, "canonical" | "current" | "pinned") {
        return Ok(pinned);
    }
    let value = requested.strip_prefix('v').unwrap_or(requested);
    let version = value
        .parse::<i32>()
        .map_err(|_| ApiError::invalid(format!("invalid version: {requested}")))?;
    if version < 1 || version > pinned {
        return Err(ApiError::not_found("version_not_found", requested));
    }
    Ok(version)
}

fn read_request_hash(spec: &ReadSpec) -> ApiResult<String> {
    let value = json!({
        "ref": spec.reference, "view": read_view_name(spec.view), "version": spec.version,
        "anchor": spec.anchor, "before": spec.before, "after": spec.after,
        "start": spec.start, "end": spec.end, "from_version": spec.from_version,
        "to_version": spec.to_version, "max_chars": spec.max_chars,
        "audience": spec.audience, "purpose": spec.purpose
    });
    Ok(sha256_hex(&serde_json::to_vec(&value)?))
}

fn read_view_name(view: ReadView) -> &'static str {
    match view {
        ReadView::CurrentState => "current_state",
        ReadView::Structured => "structured",
        ReadView::Outline => "outline",
        ReadView::Full => "full",
        ReadView::Range => "range",
        ReadView::Neighbors => "neighbors",
        ReadView::Relationships => "relationships",
        ReadView::History => "history",
        ReadView::Diff => "diff",
        ReadView::LastKnownGood => "last_known_good",
        ReadView::MaterializeScope => "materialize_scope",
    }
}

fn bounded_char_end(text: &str, start: usize, end: usize, max_chars: usize) -> usize {
    text[start..end]
        .char_indices()
        .nth(max_chars)
        .map(|(offset, _)| start + offset)
        .unwrap_or(end)
}

fn byte_at_char(text: &str, char_index: usize) -> Option<usize> {
    if char_index == text.chars().count() {
        Some(text.len())
    } else {
        text.char_indices().nth(char_index).map(|(byte, _)| byte)
    }
}

fn byte_at_line(text: &str, line: usize) -> Option<usize> {
    if line <= 1 {
        return Some(0);
    }
    text.match_indices('\n')
        .nth(line - 2)
        .map(|(byte, _)| byte + 1)
}

fn byte_after_line(text: &str, line: usize) -> Option<usize> {
    if line == usize::MAX {
        return Some(text.len());
    }
    text.match_indices('\n')
        .nth(line.saturating_sub(1))
        .map(|(byte, _)| byte + 1)
        .or(Some(text.len()))
}

fn line_at_byte(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())]
        .bytes()
        .filter(|value| *value == b'\n')
        .count()
        + 1
}

fn line_diff(left: &str, right: &str) -> Vec<Value> {
    let left: Vec<&str> = left.lines().collect();
    let right: Vec<&str> = right.lines().collect();
    let mut changes = Vec::new();
    for index in 0..left.len().max(right.len()) {
        if left.get(index) != right.get(index) {
            changes.push(json!({
                "line": index + 1,
                "before": left.get(index),
                "after": right.get(index)
            }));
        }
    }
    changes
}

fn is_text_media_type(value: &str) -> bool {
    value.starts_with("text/")
        || matches!(
            value,
            "application/json"
                | "application/xml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/javascript"
        )
}

fn item_error(error: ApiError) -> ItemError {
    match error {
        ApiError::Public {
            code,
            message,
            details,
            ..
        } => ItemError {
            code: code.to_owned(),
            message,
            details,
        },
        other => ItemError {
            code: "internal".to_owned(),
            message: other.to_string(),
            details: None,
        },
    }
}

/// Execute a deterministic, read-only DAG. No operator accepts source code or
/// SQL. Unsupported semantics are represented on the affected step.
pub async fn compute(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    request: &ComputeRequest,
) -> ApiResult<ComputeResult> {
    request.validate()?;
    if is_stage_ref(&request.session_id) {
        let stage =
            load_staged_corpus(state, auth, &request.session_id, Capability::Compute).await?;
        return tokio::time::timeout(
            Duration::from_secs(5),
            compute_staged_corpus(state, auth, codec, &stage, request),
        )
        .await
        .map_err(|_| {
            ApiError::public(
                http::StatusCode::UNPROCESSABLE_ENTITY,
                "compute_plan",
                "compute plan exceeded the five-second deterministic execution budget",
            )
        })?;
    }
    let session = load_session(state, auth, &request.session_id, Capability::Compute).await?;
    tokio::time::timeout(
        Duration::from_secs(5),
        compute_inner(state, auth, codec, &session, request),
    )
    .await
    .map_err(|_| {
        ApiError::public(
            http::StatusCode::UNPROCESSABLE_ENTITY,
            "compute_plan",
            "compute plan exceeded the five-second deterministic execution budget",
        )
    })?
}

async fn compute_inner(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    session: &PinnedSession,
    request: &ComputeRequest,
) -> ApiResult<ComputeResult> {
    let max_rows = request
        .max_rows
        .unwrap_or(MAX_COMPUTE_ROWS)
        .min(MAX_COMPUTE_ROWS);
    let token_budget = request.token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET).max(1);
    validate_compute_dag(&request.steps)?;
    let mut outputs = HashMap::<String, Value>::new();
    let mut steps = Vec::with_capacity(request.steps.len());
    let mut rows_returned = 0usize;
    let mut estimated_tokens = 0usize;

    for step in &request.steps {
        let step_started = Instant::now();
        let operator = compute_operator_name(step.op);
        let missing_dependency = step
            .depends_on
            .iter()
            .find(|dependency| !outputs.contains_key(*dependency));
        let mut execution = if let Some(dependency) = missing_dependency {
            StepExecution {
                status: ItemStatus::Failed,
                output: Value::Null,
                evidence_refs: Vec::new(),
                message: Some(format!("dependency {dependency} did not produce an output")),
            }
        } else {
            execute_compute_step(
                state,
                auth,
                codec,
                session,
                step,
                &outputs,
                max_rows.saturating_sub(rows_returned),
            )
            .await?
        };
        let step_rows = count_json_rows(&execution.output);
        if rows_returned.saturating_add(step_rows) > max_rows {
            truncate_json_rows(
                &mut execution.output,
                max_rows.saturating_sub(rows_returned),
            );
            execution.status = ItemStatus::Partial;
            execution.message = Some("output truncated at the 10,000-row compute limit".to_owned());
        }
        let serialized = serde_json::to_string(&execution.output)?;
        let step_tokens = estimate_tokens(&serialized);
        if estimated_tokens.saturating_add(step_tokens) > token_budget {
            execution.output = json!({
                "truncated": true,
                "reason": "compute token budget exhausted",
                "output_hash": prefixed_hash(sha256_hex(serialized.as_bytes()))
            });
            execution.status = ItemStatus::Partial;
            execution.message =
                Some("output replaced by a hash at the compute token budget".to_owned());
        } else {
            estimated_tokens += step_tokens;
        }
        rows_returned += count_json_rows(&execution.output);
        outputs.insert(step.id.clone(), execution.output.clone());
        metrics::histogram!(
            "compute.step.duration_ms",
            "operator" => operator,
            "status" => telemetry::item_status(execution.status)
        )
        .record(telemetry::elapsed_ms(step_started));
        steps.push(ComputeStepResult {
            id: step.id.clone(),
            operator: operator.to_owned(),
            status: execution.status,
            output: execution.output,
            evidence_refs: dedup_strings(execution.evidence_refs),
            message: execution.message,
        });
    }

    let status = if steps.iter().all(|step| step.status == ItemStatus::Complete) {
        ResponseStatus::Complete
    } else {
        ResponseStatus::Partial
    };
    Ok(ComputeResult {
        session_id: session.session_ref.clone(),
        corpus_revision: session.corpus_revision_ref.clone(),
        status,
        steps,
        rows_returned,
        estimated_tokens,
        freshness: session.freshness.clone(),
    })
}

async fn execute_compute_step(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    session: &PinnedSession,
    step: &ComputeStep,
    outputs: &HashMap<String, Value>,
    remaining_rows: usize,
) -> ApiResult<StepExecution> {
    match step.op {
        ComputeOperator::Catalog => {
            let mut tx = state.begin_read(auth).await?;
            let output = corpus_map_tx(&mut tx, session).await?;
            tx.commit().await?;
            Ok(complete_step(output))
        }
        ComputeOperator::Query | ComputeOperator::Search => {
            let mut spec: QuerySpec = serde_json::from_value(step.input.clone()).or_else(|_| {
                let query = step.input.as_str().map(str::to_owned).or_else(|| {
                    step.input
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
                serde_json::from_value(
                    json!({"id": step.id, "query": query, "limit": remaining_rows.clamp(1, 100)}),
                )
            })?;
            spec.limit = spec.limit.min(remaining_rows.max(1)).min(100);
            if step.op == ComputeOperator::Search && spec.modes.is_empty() {
                spec.modes = vec![
                    RetrievalMode::Exact,
                    RetrievalMode::Lexical,
                    RetrievalMode::Semantic,
                ];
            }
            let result = run_query_batch(
                state,
                auth,
                session,
                vec![spec],
                QuerySelection::Diversified,
            )
            .await?;
            let evidence = result
                .items
                .iter()
                .flat_map(|item| item.results.iter())
                .flat_map(|item| item.evidence_refs.clone())
                .collect();
            Ok(StepExecution {
                status: if result.status == ResponseStatus::Complete {
                    ItemStatus::Complete
                } else {
                    ItemStatus::Partial
                },
                output: serde_json::to_value(result.items)?,
                evidence_refs: evidence,
                message: None,
            })
        }
        ComputeOperator::Read
        | ComputeOperator::Neighbors
        | ComputeOperator::History
        | ComputeOperator::Diff => {
            let mut spec: ReadSpec = serde_json::from_value(step.input.clone())?;
            spec.view = match step.op {
                ComputeOperator::Neighbors => ReadView::Neighbors,
                ComputeOperator::History => ReadView::History,
                ComputeOperator::Diff => ReadView::Diff,
                _ => spec.view,
            };
            let mut tx = state.begin_read(auth).await?;
            let item = read_item_tx(&mut tx, state, codec, session, &spec).await?;
            tx.commit().await?;
            let evidence = item
                .data
                .as_ref()
                .map(collect_evidence_refs)
                .unwrap_or_default();
            Ok(StepExecution {
                status: item.status,
                output: item.data.unwrap_or(Value::Null),
                evidence_refs: evidence,
                message: item.error.map(|error| error.message),
            })
        }
        ComputeOperator::BatchRead => {
            let specs: Vec<ReadSpec> = serde_json::from_value(
                step.input
                    .get("requests")
                    .cloned()
                    .unwrap_or_else(|| step.input.clone()),
            )?;
            if specs.is_empty() || specs.len() > 64 {
                return Ok(unsupported_step(
                    "batchRead requires between 1 and 64 read specs",
                ));
            }
            let mut items = Vec::new();
            for spec in &specs {
                let mut tx = state.begin_read(auth).await?;
                match read_item_tx(&mut tx, state, codec, session, spec).await {
                    Ok(item) => {
                        tx.commit().await?;
                        items.push(item);
                    }
                    Err(error) => {
                        tx.rollback().await?;
                        items.push(ReadItemResult {
                            reference: spec.reference.clone(),
                            view: read_view_name(spec.view).to_owned(),
                            status: ItemStatus::Failed,
                            data: None,
                            error: Some(item_error(error)),
                            truncation: Truncation::default(),
                        });
                    }
                }
            }
            let output = serde_json::to_value(&items)?;
            Ok(StepExecution {
                status: if items.iter().all(|item| item.status == ItemStatus::Complete) {
                    ItemStatus::Complete
                } else {
                    ItemStatus::Partial
                },
                evidence_refs: collect_evidence_refs(&output),
                output,
                message: None,
            })
        }
        ComputeOperator::Timeline => {
            let mut tx = state.begin_read(auth).await?;
            let output = timeline_tx(&mut tx, session, &step.input, remaining_rows).await?;
            tx.commit().await?;
            Ok(complete_step(output))
        }
        ComputeOperator::Group => group_step(&step.input, outputs, &step.depends_on),
        ComputeOperator::Aggregate => aggregate_step(&step.input, outputs, &step.depends_on),
        ComputeOperator::Traverse => {
            let mut tx = state.begin_read(auth).await?;
            let output = traverse_tx(&mut tx, session, &step.input, remaining_rows).await?;
            tx.commit().await?;
            Ok(complete_step(output))
        }
        ComputeOperator::ResolveIdentity => {
            let mut tx = state.begin_read(auth).await?;
            let output = resolve_identity_tx(&mut tx, session, &step.input, remaining_rows).await?;
            tx.commit().await?;
            Ok(complete_step(output))
        }
        ComputeOperator::ExpandRecurrence => {
            let mut tx = state.begin_read(auth).await?;
            let execution =
                expand_recurrence_tx(&mut tx, session, &step.input, remaining_rows).await?;
            tx.commit().await?;
            Ok(execution)
        }
        ComputeOperator::StateHistory => {
            let mut tx = state.begin_read(auth).await?;
            let output = state_history_tx(&mut tx, session, &step.input, remaining_rows).await?;
            tx.commit().await?;
            Ok(complete_step(output))
        }
        ComputeOperator::Impact => {
            let mut tx = state.begin_read(auth).await?;
            let output = traverse_tx(&mut tx, session, &step.input, remaining_rows).await?;
            tx.commit().await?;
            Ok(StepExecution {
                status: ItemStatus::Partial,
                output,
                evidence_refs: Vec::new(),
                message: Some(
                    "impact is an undirected qualified-relation closure because the generic schema has no universal dependency direction"
                        .to_owned(),
                ),
            })
        }
        ComputeOperator::CompareApplicability => {
            compare_applicability_step(&step.input, outputs, &step.depends_on)
        }
        ComputeOperator::Proximity => proximity_step(&step.input),
        ComputeOperator::GateRollup => {
            let mut tx = state.begin_read(auth).await?;
            let output = gate_rollup_tx(&mut tx, session, &step.input).await?;
            tx.commit().await?;
            Ok(complete_step(output))
        }
    }
}

fn validate_compute_dag(steps: &[ComputeStep]) -> ApiResult<()> {
    let mut seen = HashSet::new();
    for step in steps {
        if step.id.trim().is_empty() || !seen.insert(step.id.clone()) {
            return Err(ApiError::public(
                http::StatusCode::UNPROCESSABLE_ENTITY,
                "compute_plan",
                "compute step IDs must be nonempty and unique",
            ));
        }
        if step
            .depends_on
            .iter()
            .any(|dependency| !seen.contains(dependency))
        {
            return Err(ApiError::public(
                http::StatusCode::UNPROCESSABLE_ENTITY,
                "compute_plan",
                format!("step {} has a missing or forward dependency", step.id),
            ));
        }
    }
    Ok(())
}

fn complete_step(output: Value) -> StepExecution {
    let evidence_refs = collect_evidence_refs(&output);
    StepExecution {
        status: ItemStatus::Complete,
        output,
        evidence_refs,
        message: None,
    }
}

fn unsupported_step(message: impl Into<String>) -> StepExecution {
    let message = message.into();
    StepExecution {
        status: ItemStatus::Unsupported,
        output: json!({"unsupported": true, "reason": message}),
        evidence_refs: Vec::new(),
        message: Some(message),
    }
}

fn group_step(
    input: &Value,
    outputs: &HashMap<String, Value>,
    dependencies: &[String],
) -> ApiResult<StepExecution> {
    let rows = compute_input_rows(input, outputs, dependencies);
    let pointer = input.get("by").and_then(Value::as_str).unwrap_or("");
    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    for row in rows {
        let key = if pointer.is_empty() {
            "all".to_owned()
        } else {
            row.pointer(pointer)
                .map(canonical_value_key)
                .unwrap_or_else(|| "null".to_owned())
        };
        groups.entry(key).or_default().push(row);
    }
    Ok(complete_step(json!({"by": pointer, "groups": groups})))
}

fn aggregate_step(
    input: &Value,
    outputs: &HashMap<String, Value>,
    dependencies: &[String],
) -> ApiResult<StepExecution> {
    let rows = compute_input_rows(input, outputs, dependencies);
    let pointer = input.get("field").and_then(Value::as_str).unwrap_or("");
    let op = input
        .get("aggregate")
        .and_then(Value::as_str)
        .unwrap_or("count");
    let numbers: Vec<f64> = rows
        .iter()
        .filter_map(|row| {
            if pointer.is_empty() {
                row.as_f64()
            } else {
                row.pointer(pointer).and_then(Value::as_f64)
            }
        })
        .collect();
    let value = match op {
        "count" => json!(rows.len()),
        "sum" => json!(numbers.iter().sum::<f64>()),
        "min" => numbers
            .iter()
            .copied()
            .reduce(f64::min)
            .map_or(Value::Null, |value| json!(value)),
        "max" => numbers
            .iter()
            .copied()
            .reduce(f64::max)
            .map_or(Value::Null, |value| json!(value)),
        "average" | "avg" if !numbers.is_empty() => {
            json!(numbers.iter().sum::<f64>() / numbers.len() as f64)
        }
        "average" | "avg" => Value::Null,
        other => {
            return Ok(unsupported_step(format!(
                "unknown aggregate operator: {other}"
            )));
        }
    };
    Ok(complete_step(
        json!({"aggregate": op, "field": pointer, "value": value, "input_rows": rows.len()}),
    ))
}

fn compute_input_rows(
    input: &Value,
    outputs: &HashMap<String, Value>,
    dependencies: &[String],
) -> Vec<Value> {
    if let Some(rows) = input.get("rows").and_then(Value::as_array) {
        return rows.clone();
    }
    dependencies
        .iter()
        .filter_map(|dependency| outputs.get(dependency))
        .flat_map(flatten_rows)
        .collect()
}

fn flatten_rows(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(rows) => rows.clone(),
        Value::Object(object) => {
            for key in ["results", "items", "rows", "history", "events", "paths"] {
                if let Some(rows) = object.get(key).and_then(Value::as_array) {
                    return rows.clone();
                }
            }
            vec![value.clone()]
        }
        _ => vec![value.clone()],
    }
}

async fn timeline_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    input: &Value,
    limit: usize,
) -> ApiResult<Value> {
    let about_id = input
        .get("ref")
        .and_then(Value::as_str)
        .map(|value| parse_uuid_ref(value, &[]))
        .transpose()?;
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT payload FROM (
          SELECT c.recorded_at AS sort_time, jsonb_build_object(
            'kind', 'claim', 'ref', 'claim:' || replace(c.id::text, '-', ''),
            'predicate', c.predicate::text, 'value', c.value,
            'recorded_at', c.recorded_at,
            'valid_time', to_jsonb(ts) - 'user_id' - 'scope_id'
          ) AS payload
          FROM straylight.claims c
          JOIN straylight.corpus_members cm ON cm.user_id = c.user_id AND cm.record_id = c.id
          LEFT JOIN straylight.temporal_specs ts ON ts.user_id = c.user_id AND ts.id = c.valid_temporal_id
          WHERE c.user_id = $1 AND c.scope_id = $2 AND cm.corpus_revision_id = $3
            AND cm.disposition <> 'tombstoned'
            AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM straylight.claim_about_targets cat
                WHERE cat.user_id = c.user_id AND cat.claim_id = c.id AND cat.target_record_id = $4))
          UNION ALL
          SELECT orev.recorded_at, jsonb_build_object(
            'kind', 'event_occurrence',
            'ref', 'object:' || replace(eor.occurrence_object_id::text, '-', ''),
            'series_ref', 'object:' || replace(eo.series_object_id::text, '-', ''),
            'exception_kind', eor.exception_kind,
            'current_time', to_jsonb(ts) - 'user_id' - 'scope_id',
            'recorded_at', orev.recorded_at
          )
          FROM straylight.event_occurrences eo
          JOIN straylight.corpus_members cm ON cm.user_id = eo.user_id AND cm.record_id = eo.occurrence_object_id
          JOIN straylight.event_occurrence_revisions eor ON eor.user_id = eo.user_id AND eor.occurrence_object_id = eo.occurrence_object_id AND eor.object_version = cm.record_version
          JOIN straylight.object_revisions orev ON orev.user_id = eor.user_id AND orev.object_id = eor.occurrence_object_id AND orev.version = eor.object_version
          JOIN straylight.temporal_specs ts ON ts.user_id = eor.user_id AND ts.id = eor.current_temporal_id
          WHERE eo.user_id = $1 AND eo.scope_id = $2 AND cm.corpus_revision_id = $3
            AND cm.disposition <> 'tombstoned'
            AND ($4::uuid IS NULL OR eo.series_object_id = $4 OR eo.occurrence_object_id = $4)
        ) timeline ORDER BY sort_time, payload ->> 'ref' LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(about_id)
    .bind(i64::try_from(limit.max(1)).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    Ok(json!({"events": rows, "corpus_revision": session.corpus_revision_ref}))
}

async fn traverse_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    input: &Value,
    limit: usize,
) -> ApiResult<Value> {
    let start_ref = input
        .get("ref")
        .or_else(|| input.get("start_ref"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("traverse requires ref or start_ref"))?;
    let start = parse_uuid_ref(start_ref, &[])?;
    let depth = input
        .get("depth")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(2)
        .clamp(1, MAX_GRAPH_DEPTH);
    let predicates: Vec<String> = input
        .get("predicates")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE active_edges AS (
          SELECT rr.predicate::text AS predicate, left_ep.target_record_id AS left_id,
                 right_ep.target_record_id AS right_id, r.id AS relation_id
          FROM straylight.relations r
          JOIN straylight.corpus_members cm ON cm.user_id = r.user_id AND cm.record_id = r.id
          JOIN straylight.relation_revisions rr ON rr.user_id = r.user_id AND rr.relation_id = r.id AND rr.version = cm.record_version
          JOIN straylight.relation_endpoints left_ep ON left_ep.user_id = rr.user_id AND left_ep.relation_id = rr.relation_id AND left_ep.relation_version = rr.version
          JOIN straylight.relation_endpoints right_ep ON right_ep.user_id = rr.user_id AND right_ep.relation_id = rr.relation_id AND right_ep.relation_version = rr.version AND right_ep.target_record_id <> left_ep.target_record_id
          WHERE r.user_id = $1 AND r.scope_id = $2 AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
            AND (cardinality($5::text[]) = 0 OR rr.predicate::text = ANY($5::text[]))
        ), walk(node_id, depth, path, relation_path) AS (
          SELECT $4::uuid, 0, ARRAY[$4::uuid], ARRAY[]::uuid[]
          UNION ALL
          SELECT edge.right_id, walk.depth + 1, walk.path || edge.right_id,
                 walk.relation_path || edge.relation_id
          FROM walk JOIN active_edges edge ON edge.left_id = walk.node_id
          WHERE walk.depth < $6 AND NOT edge.right_id = ANY(walk.path)
        )
        SELECT jsonb_build_object(
          'target_ref', rk.record_kind || ':' || replace(walk.node_id::text, '-', ''),
          'depth', walk.depth,
          'path', ARRAY(SELECT replace(item::text, '-', '') FROM unnest(walk.path) item),
          'relation_refs', ARRAY(SELECT 'relation:' || replace(item::text, '-', '') FROM unnest(walk.relation_path) item)
        )
        FROM walk JOIN straylight.record_keys rk ON rk.user_id = $1 AND rk.record_id = walk.node_id
        WHERE walk.depth > 0 ORDER BY walk.depth, walk.node_id LIMIT $7
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(start)
    .bind(predicates)
    .bind(depth)
    .bind(i64::try_from(limit.max(1)).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    Ok(json!({"start_ref": start_ref, "max_depth": depth, "paths": rows}))
}

async fn resolve_identity_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    input: &Value,
    limit: usize,
) -> ApiResult<Value> {
    let query = input
        .get("query")
        .or_else(|| input.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("resolveIdentity requires query or name"))?;
    let namespace = input.get("namespace").and_then(Value::as_str);
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT jsonb_build_object(
          'object_ref', 'object:' || replace(candidate.object_id::text, '-', ''),
          'label', orev.label, 'matched_by', candidate.matched_by,
          'match_value', candidate.match_value,
          'resolution', 'candidate_not_equivalence'
        )
        FROM (
          SELECT inc.object_id, 'source_bearing_name'::text AS matched_by, inc.name_value AS match_value
          FROM straylight.identity_name_claims inc
          JOIN straylight.corpus_members cm ON cm.user_id = inc.user_id AND cm.record_id = inc.claim_id
          WHERE inc.user_id = $1 AND cm.corpus_revision_id = $3
            AND lower(inc.name_value) = lower($4)
          UNION ALL
          SELECT h.object_id, 'handle', h.handle FROM straylight.object_handles h
          WHERE h.user_id = $1 AND h.scope_id = $2 AND h.valid_to IS NULL AND lower(h.handle) = lower($4)
          UNION ALL
          SELECT ext.object_id, 'external_identifier', ext.identifier_value
          FROM straylight.external_identifiers ext
          WHERE ext.user_id = $1 AND ext.scope_id = $2
            AND lower(ext.identifier_value) = lower($4)
            AND ($5::text IS NULL OR ext.namespace = $5)
        ) candidate
        JOIN straylight.corpus_members ocm ON ocm.user_id = $1 AND ocm.record_id = candidate.object_id AND ocm.corpus_revision_id = $3 AND ocm.disposition = 'active'
        JOIN straylight.object_revisions orev ON orev.user_id = $1 AND orev.object_id = candidate.object_id AND orev.version = ocm.record_version
        ORDER BY candidate.matched_by, orev.label, candidate.object_id LIMIT $6
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(query)
    .bind(namespace)
    .bind(i64::try_from(limit.max(1)).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    Ok(json!({
        "query": query,
        "candidates": rows,
        "ambiguous": rows.len() != 1,
        "identity_rule": "Names, handles, external identifiers, and similarity return candidates; only a reviewed core.same_as relation establishes equivalence."
    }))
}

async fn expand_recurrence_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    input: &Value,
    limit: usize,
) -> ApiResult<StepExecution> {
    let series_ref = input
        .get("series_ref")
        .or_else(|| input.get("ref"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("expandRecurrence requires series_ref"))?;
    let series_id = parse_uuid_ref(series_ref, &["object"])?;
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'occurrence_ref', 'object:' || replace(eo.occurrence_object_id::text, '-', ''),
          'series_ref', 'object:' || replace(eo.series_object_id::text, '-', ''),
          'canonical_recurrence_id', eo.canonical_recurrence_id,
          'original_time', to_jsonb(original_spec) - 'user_id' - 'scope_id',
          'current_time', to_jsonb(current_spec) - 'user_id' - 'scope_id',
          'actual_time', to_jsonb(actual_spec) - 'user_id' - 'scope_id',
          'exception_kind', eor.exception_kind,
          'evidence_ref', 'evidence:' || replace(eor.evidence_id::text, '-', '')
        )
        FROM straylight.event_occurrences eo
        JOIN straylight.corpus_members cm ON cm.user_id = eo.user_id AND cm.record_id = eo.occurrence_object_id
        JOIN straylight.event_occurrence_revisions eor ON eor.user_id = eo.user_id AND eor.occurrence_object_id = eo.occurrence_object_id AND eor.object_version = cm.record_version
        JOIN straylight.temporal_specs original_spec ON original_spec.user_id = eo.user_id AND original_spec.id = eo.original_temporal_id
        JOIN straylight.temporal_specs current_spec ON current_spec.user_id = eor.user_id AND current_spec.id = eor.current_temporal_id
        LEFT JOIN straylight.temporal_specs actual_spec ON actual_spec.user_id = eor.user_id AND actual_spec.id = eor.actual_temporal_id
        WHERE eo.user_id = $1 AND eo.scope_id = $2 AND eo.series_object_id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition = 'active'
        ORDER BY eo.canonical_recurrence_id LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(series_id)
    .bind(session.corpus_revision_id)
    .bind(i64::try_from(limit.max(1)).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    let recurrence_row = sqlx::query(
        r#"
        SELECT rs.id AS recurrence_spec_id, ts.*
        FROM straylight.corpus_members cm
        JOIN straylight.event_series_revisions esr
          ON esr.user_id = cm.user_id AND esr.object_id = cm.record_id
         AND esr.object_version = cm.record_version
        JOIN straylight.recurrence_specs rs
          ON rs.user_id = esr.user_id AND rs.id = esr.recurrence_spec_id
        JOIN straylight.temporal_specs ts
          ON ts.user_id = rs.user_id AND ts.id = rs.start_temporal_id
        WHERE cm.user_id = $1 AND cm.scope_id = $2 AND cm.corpus_revision_id = $3
          AND cm.record_id = $4 AND cm.disposition = 'active'
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(series_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(recurrence_row) = recurrence_row else {
        return Ok(StepExecution {
            status: ItemStatus::Unsupported,
            output: json!({
                "series_ref": series_ref,
                "rules": [],
                "generated_occurrences": [],
                "materialized_occurrences": rows
            }),
            evidence_refs: collect_evidence_refs(&Value::Array(rows)),
            message: Some(
                "the selected event series has no active recurrence specification".to_owned(),
            ),
        });
    };
    let recurrence_spec_id: Uuid = recurrence_row.try_get("recurrence_spec_id")?;
    let rules: Vec<String> = sqlx::query_scalar(
        "SELECT rrule FROM straylight.recurrence_rules WHERE user_id=$1 AND recurrence_spec_id=$2 ORDER BY rule_order",
    )
    .bind(session.user_id)
    .bind(recurrence_spec_id)
    .fetch_all(&mut **tx)
    .await?;
    if rules.is_empty() {
        return Ok(StepExecution {
            status: ItemStatus::Complete,
            output: json!({
                "series_ref": series_ref,
                "rules": [],
                "generated_occurrences": [],
                "materialized_occurrences": rows,
                "limited": false
            }),
            evidence_refs: collect_evidence_refs(&Value::Array(rows)),
            message: None,
        });
    }

    let start = recurrence::start_datetime(&temporal_spec_from_row(&recurrence_row)?)?;
    let exclusion_rows = sqlx::query(
        r#"
        SELECT ts.*
        FROM straylight.recurrence_exclusions exclusion
        JOIN straylight.temporal_specs ts
          ON ts.user_id=exclusion.user_id AND ts.id=exclusion.recurrence_id_temporal_id
        WHERE exclusion.user_id=$1 AND exclusion.recurrence_spec_id=$2
        ORDER BY ts.id
        "#,
    )
    .bind(session.user_id)
    .bind(recurrence_spec_id)
    .fetch_all(&mut **tx)
    .await?;
    let exclusions = exclusion_rows
        .iter()
        .map(temporal_spec_from_row)
        .map(|spec| spec.and_then(|spec| recurrence::start_datetime(&spec)))
        .collect::<ApiResult<Vec<_>>>()?;
    let window_start = recurrence_window(input, &["window_start", "start", "from"])?;
    let window_end = recurrence_window(input, &["window_end", "end", "to"])?;
    let expansion = recurrence::expand(
        start,
        &rules,
        exclusions,
        window_start,
        window_end,
        limit.max(1).min(u16::MAX as usize),
    )?;
    let materialized_instants: HashSet<DateTime<Utc>> = sqlx::query_scalar(
        r#"
        SELECT CASE original_spec.kind
          WHEN 'date' THEN original_spec.date_value::timestamp AT TIME ZONE 'UTC'
          WHEN 'date_interval' THEN original_spec.start_date::timestamp AT TIME ZONE 'UTC'
          WHEN 'local_datetime' THEN original_spec.local_value AT TIME ZONE original_spec.time_zone
          WHEN 'due' THEN original_spec.local_value AT TIME ZONE original_spec.time_zone
          WHEN 'floating_datetime' THEN original_spec.local_value AT TIME ZONE 'UTC'
          WHEN 'instant' THEN original_spec.instant_value
          WHEN 'instant_interval' THEN original_spec.start_instant
          WHEN 'local_interval' THEN original_spec.start_local AT TIME ZONE original_spec.time_zone
        END AS recurrence_instant
        FROM straylight.event_occurrences occurrence
        JOIN straylight.corpus_members cm
          ON cm.user_id=occurrence.user_id AND cm.record_id=occurrence.occurrence_object_id
        JOIN straylight.temporal_specs original_spec
          ON original_spec.user_id=occurrence.user_id AND original_spec.id=occurrence.original_temporal_id
        WHERE occurrence.user_id=$1 AND occurrence.scope_id=$2
          AND occurrence.series_object_id=$3 AND cm.corpus_revision_id=$4
          AND cm.disposition='active'
        LIMIT 10001
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(series_id)
    .bind(session.corpus_revision_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    if materialized_instants.len() > 10_000 {
        return Ok(unsupported_step(
            "recurrence expansion has more than 10,000 materialized exceptions",
        ));
    }
    let generated = expansion
        .dates
        .into_iter()
        .filter(|date| !materialized_instants.contains(&date.with_timezone(&Utc)))
        .map(|date| {
            json!({
                "occurrence_ref": null,
                "series_ref": series_ref,
                "canonical_recurrence_id": date.to_rfc3339(),
                "generated_time": {
                    "local": date.naive_local(),
                    "time_zone": date.timezone().name(),
                    "instant": date.with_timezone(&Utc)
                },
                "exception_kind": "generated_unmaterialized"
            })
        })
        .collect::<Vec<_>>();
    let mut evidence_refs = collect_evidence_refs(&Value::Array(rows.clone()));
    let series_evidence: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT 'evidence:' || replace(evidence.id::text, '-', '')
        FROM straylight.corpus_members cm
        JOIN straylight.object_revisions revision
          ON revision.user_id=cm.user_id AND revision.object_id=cm.record_id
         AND revision.version=cm.record_version
        JOIN straylight.evidence_items evidence
          ON evidence.user_id=revision.user_id
         AND evidence.source_episode_id=revision.source_episode_id
        WHERE cm.user_id=$1 AND cm.corpus_revision_id=$2 AND cm.record_id=$3
        ORDER BY 1
        "#,
    )
    .bind(session.user_id)
    .bind(session.corpus_revision_id)
    .bind(series_id)
    .fetch_all(&mut **tx)
    .await?;
    evidence_refs.extend(series_evidence);
    evidence_refs.sort();
    evidence_refs.dedup();
    Ok(StepExecution {
        status: ItemStatus::Complete,
        output: json!({
            "series_ref": series_ref,
            "rules": rules,
            "window": {"start": window_start, "end": window_end},
            "generated_occurrences": generated,
            "materialized_occurrences": rows,
            "limited": expansion.limited
        }),
        evidence_refs,
        message: None,
    })
}

fn recurrence_window(input: &Value, keys: &[&str]) -> ApiResult<Option<DateTime<Utc>>> {
    let Some((key, value)) = keys.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .map(|value| (*key, value))
    }) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|_| ApiError::invalid(format!("expandRecurrence {key} must be RFC 3339")))
}

fn temporal_spec_from_row(row: &PgRow) -> ApiResult<TemporalSpecV1> {
    let kind: String = row.try_get("kind")?;
    match kind.as_str() {
        "date" => Ok(TemporalSpecV1::Date {
            date: row.try_get::<NaiveDate, _>("date_value")?,
        }),
        "date_interval" => Ok(TemporalSpecV1::DateInterval {
            start: row.try_get::<NaiveDate, _>("start_date")?,
            end_exclusive: row.try_get("end_date_exclusive")?,
        }),
        "local_datetime" => Ok(TemporalSpecV1::LocalDatetime {
            local: row.try_get::<NaiveDateTime, _>("local_value")?,
            time_zone: row.try_get("time_zone")?,
        }),
        "floating_datetime" => Ok(TemporalSpecV1::FloatingDatetime {
            local: row.try_get::<NaiveDateTime, _>("local_value")?,
        }),
        "instant" => Ok(TemporalSpecV1::Instant {
            at: row.try_get::<DateTime<Utc>, _>("instant_value")?,
        }),
        "instant_interval" => Ok(TemporalSpecV1::InstantInterval {
            start: row.try_get::<DateTime<Utc>, _>("start_instant")?,
            end: row.try_get("end_instant")?,
        }),
        "local_interval" => Ok(TemporalSpecV1::LocalInterval {
            start_local: row.try_get::<NaiveDateTime, _>("start_local")?,
            end_local: row.try_get::<NaiveDateTime, _>("end_local")?,
            time_zone: row.try_get("time_zone")?,
        }),
        "due" => Ok(TemporalSpecV1::Due {
            local: row.try_get::<NaiveDateTime, _>("local_value")?,
            time_zone: row.try_get("time_zone")?,
        }),
        _ => Err(ApiError::invalid(format!(
            "unsupported recurrence temporal kind: {kind}"
        ))),
    }
}

async fn state_history_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    input: &Value,
    limit: usize,
) -> ApiResult<Value> {
    let target_ref = input
        .get("ref")
        .or_else(|| input.get("target_ref"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("stateHistory requires ref or target_ref"))?;
    let target = parse_uuid_ref(target_ref, &[])?;
    let machine = input.get("machine_ref").and_then(Value::as_str);
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'claim_ref', 'claim:' || replace(sa.claim_id::text, '-', ''),
          'machine_ref', sa.state_machine_ref::text, 'value', sa.state_value,
          'previous_claim_ref', CASE WHEN sa.previous_assignment_claim_id IS NULL THEN NULL ELSE 'claim:' || replace(sa.previous_assignment_claim_id::text, '-', '') END,
          'recorded_at', c.recorded_at,
          'evidence_refs', ARRAY(SELECT 'evidence:' || replace(link.evidence_id::text, '-', '') FROM straylight.claim_evidence_links link WHERE link.user_id = c.user_id AND link.claim_id = c.id)
        )
        FROM straylight.state_assignments sa
        JOIN straylight.claims c ON c.user_id = sa.user_id AND c.id = sa.claim_id
        JOIN straylight.corpus_members cm ON cm.user_id = sa.user_id AND cm.record_id = sa.claim_id
        WHERE sa.user_id = $1 AND sa.scope_id = $2 AND sa.target_record_id = $3
          AND cm.corpus_revision_id = $4 AND cm.disposition <> 'tombstoned'
          AND ($5::text IS NULL OR sa.state_machine_ref::text = $5)
        ORDER BY c.recorded_at, c.id LIMIT $6
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(target)
    .bind(session.corpus_revision_id)
    .bind(machine)
    .bind(i64::try_from(limit.max(1)).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await?;
    Ok(json!({"target_ref": target_ref, "machine_ref": machine, "history": rows}))
}

fn compare_applicability_step(
    input: &Value,
    outputs: &HashMap<String, Value>,
    dependencies: &[String],
) -> ApiResult<StepExecution> {
    let left = input
        .get("left")
        .cloned()
        .or_else(|| dependencies.first().and_then(|id| outputs.get(id)).cloned());
    let right = input
        .get("right")
        .cloned()
        .or_else(|| dependencies.get(1).and_then(|id| outputs.get(id)).cloned());
    let (Some(left), Some(right)) = (left, right) else {
        return Ok(unsupported_step(
            "compareApplicability requires left and right values or two dependencies",
        ));
    };
    let left_dimensions = left
        .pointer("/applicability")
        .cloned()
        .unwrap_or(Value::Null);
    let right_dimensions = right
        .pointer("/applicability")
        .cloned()
        .unwrap_or(Value::Null);
    if left_dimensions.is_null() || right_dimensions.is_null() {
        return Ok(unsupported_step(
            "both inputs must carry an explicit namespaced applicability object",
        ));
    }
    let compatible = left_dimensions == right_dimensions;
    Ok(complete_step(json!({
        "compatible": compatible,
        "left": left_dimensions,
        "right": right_dimensions,
        "rule": "missing applicability is not inferred"
    })))
}

fn proximity_step(input: &Value) -> ApiResult<StepExecution> {
    let Some(left) = input.get("left") else {
        return Ok(unsupported_step(
            "proximity requires left and right coordinates",
        ));
    };
    let Some(right) = input.get("right") else {
        return Ok(unsupported_step(
            "proximity requires left and right coordinates",
        ));
    };
    let left_frame = left.get("frame").and_then(Value::as_str);
    let right_frame = right.get("frame").and_then(Value::as_str);
    let left_unit = left.get("unit").and_then(Value::as_str);
    let right_unit = right.get("unit").and_then(Value::as_str);
    if left_frame.is_none() || left_frame != right_frame || left_unit != right_unit {
        return Ok(unsupported_step(
            "proximity requires one identical named frame and unit or an explicit evidenced transform",
        ));
    }
    let left_coordinates = number_array(left.get("coordinates"));
    let right_coordinates = number_array(right.get("coordinates"));
    if left_coordinates.len() != right_coordinates.len() || left_coordinates.is_empty() {
        return Ok(unsupported_step(
            "coordinate arrays must be nonempty and have equal dimensions",
        ));
    }
    let frame = left_frame.unwrap_or_default();
    let (distance, unit) = if frame == "EPSG:4326" && left_coordinates.len() == 2 {
        (
            haversine_km(
                left_coordinates[0],
                left_coordinates[1],
                right_coordinates[0],
                right_coordinates[1],
            ),
            "km",
        )
    } else {
        (
            left_coordinates
                .iter()
                .zip(&right_coordinates)
                .map(|(left, right)| (left - right).powi(2))
                .sum::<f64>()
                .sqrt(),
            left_unit.unwrap_or("unit"),
        )
    };
    Ok(complete_step(
        json!({"distance": distance, "unit": unit, "frame": frame}),
    ))
}

async fn gate_rollup_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    input: &Value,
) -> ApiResult<Value> {
    let checkpoint_ref = input
        .get("checkpoint_ref")
        .or_else(|| input.get("ref"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("gateRollup requires checkpoint_ref"))?;
    let checkpoint_id = parse_uuid_ref(checkpoint_ref, &["checkpoint"])?;
    let gates: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
          'position', gate.position, 'description', gate.description,
          'state', sa.state_value,
          'state_claim_ref', CASE WHEN gate.state_claim_id IS NULL THEN NULL ELSE 'claim:' || replace(gate.state_claim_id::text, '-', '') END,
          'evidence_refs', ARRAY(SELECT 'evidence:' || replace(ev.evidence_id::text, '-', '') FROM straylight.checkpoint_gate_evidence ev WHERE ev.user_id = gate.user_id AND ev.checkpoint_id = gate.checkpoint_id AND ev.gate_position = gate.position)
        )
        FROM straylight.checkpoint_gates gate
        LEFT JOIN straylight.state_assignments sa ON sa.user_id = gate.user_id AND sa.claim_id = gate.state_claim_id
        WHERE gate.user_id = $1 AND gate.checkpoint_id = $2
        ORDER BY gate.position
        "#,
    )
    .bind(session.user_id)
    .bind(checkpoint_id)
    .fetch_all(&mut **tx)
    .await?;
    let states: Vec<&str> = gates
        .iter()
        .filter_map(|gate| gate.get("state").and_then(Value::as_str))
        .collect();
    let rollup = if gates.is_empty() || states.len() != gates.len() {
        "incomplete"
    } else if states.iter().all(|state| *state == "passed") {
        "passed"
    } else if states.contains(&"failed") {
        "failed"
    } else if states.contains(&"blocked") {
        "blocked"
    } else {
        "not_passed"
    };
    Ok(json!({
        "checkpoint_ref": checkpoint_ref,
        "rollup": rollup,
        "gates": gates,
        "rule": "attempted work and missing evidence never count as passed"
    }))
}

fn compute_operator_name(operator: ComputeOperator) -> &'static str {
    match operator {
        ComputeOperator::Catalog => "catalog",
        ComputeOperator::Query => "query",
        ComputeOperator::Search => "search",
        ComputeOperator::Read => "read",
        ComputeOperator::BatchRead => "batchRead",
        ComputeOperator::Neighbors => "neighbors",
        ComputeOperator::Timeline => "timeline",
        ComputeOperator::History => "history",
        ComputeOperator::Diff => "diff",
        ComputeOperator::Group => "group",
        ComputeOperator::Aggregate => "aggregate",
        ComputeOperator::Traverse => "traverse",
        ComputeOperator::ResolveIdentity => "resolveIdentity",
        ComputeOperator::ExpandRecurrence => "expandRecurrence",
        ComputeOperator::StateHistory => "stateHistory",
        ComputeOperator::Impact => "impact",
        ComputeOperator::CompareApplicability => "compareApplicability",
        ComputeOperator::Proximity => "proximity",
        ComputeOperator::GateRollup => "gateRollup",
    }
}

fn count_json_rows(value: &Value) -> usize {
    match value {
        Value::Array(rows) => rows.len(),
        Value::Object(object) => object
            .values()
            .filter_map(Value::as_array)
            .map(Vec::len)
            .max()
            .unwrap_or(1),
        Value::Null => 0,
        _ => 1,
    }
}

fn truncate_json_rows(value: &mut Value, remaining: usize) {
    match value {
        Value::Array(rows) => rows.truncate(remaining),
        Value::Object(object) => {
            for value in object.values_mut() {
                if let Value::Array(rows) = value {
                    rows.truncate(remaining);
                }
            }
        }
        _ => {}
    }
}

fn collect_evidence_refs(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    fn visit(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(value) if value.starts_with("evidence:") => found.push(value.clone()),
            Value::Array(values) => values.iter().for_each(|value| visit(value, found)),
            Value::Object(object) => object.values().for_each(|value| visit(value, found)),
            _ => {}
        }
    }
    visit(value, &mut found);
    dedup_strings(found)
}

fn dedup_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn canonical_value_key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

fn number_array(value: Option<&Value>) -> Vec<f64> {
    value
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let radius = 6_371.008_8_f64;
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let delta_lat = lat2 - lat1;
    let delta_lon = (lon2 - lon1).to_radians();
    let a =
        (delta_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (delta_lon / 2.0).sin().powi(2);
    radius * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

/// Classify consequential claims using inspectable evidence, lineage, pinned
/// named state, and conservative structural checks.
pub async fn verify(
    state: &AppState,
    auth: &AuthContext,
    request: &VerifyRequest,
) -> ApiResult<VerifyResult> {
    request.validate()?;
    if is_stage_ref(&request.session_id) {
        let stage =
            load_staged_corpus(state, auth, &request.session_id, Capability::Verify).await?;
        return verify_staged_corpus(state, auth, &stage, request).await;
    }
    let session = load_session(state, auth, &request.session_id, Capability::Verify).await?;
    let mut claims = Vec::with_capacity(request.claims.len());
    for (index, claim) in request.claims.iter().enumerate() {
        let claim_started = Instant::now();
        let mut tx = state.begin_read(auth).await?;
        match verify_claim_tx(&mut tx, &session, claim, &request.check_for, index).await {
            Ok(result) => {
                tx.commit().await?;
                metrics::histogram!("verify.claim.duration_ms", "result" => "success")
                    .record(telemetry::elapsed_ms(claim_started));
                claims.push(result);
            }
            Err(error) => {
                tx.rollback().await?;
                metrics::histogram!("verify.claim.duration_ms", "result" => "failed")
                    .record(telemetry::elapsed_ms(claim_started));
                claims.push(VerifyItemResult {
                    id: claim.id.clone().unwrap_or_else(|| format!("claim_{index}")),
                    claim: claim.claim.clone(),
                    classification: VerificationClassification::InsufficientEvidence,
                    evidence: Vec::new(),
                    structural_checks: vec![StructuralCheck {
                        check: "verify_item".to_owned(),
                        status: "failed".to_owned(),
                        details: json!({"error": error.to_string()}),
                    }],
                    explanation: "The item could not be verified without violating the pinned or typed read contract.".to_owned(),
                });
            }
        }
    }
    let status = if claims
        .iter()
        .all(|claim| claim.classification == VerificationClassification::Supported)
    {
        ResponseStatus::Complete
    } else {
        ResponseStatus::Partial
    };
    Ok(VerifyResult {
        session_id: session.session_ref.clone(),
        corpus_revision: session.corpus_revision_ref.clone(),
        status,
        claims,
        freshness: session.freshness,
        coverage: Coverage {
            searched: vec![
                CoveragePartition {
                    partition: "pinned_claim_graph".to_owned(),
                    lane: "structured_verify".to_owned(),
                    completeness: "complete".to_owned(),
                    index_revision: Some(session.corpus_revision_ref.clone()),
                    searched_count: request.claims.len(),
                    candidate_count: request.claims.len(),
                    failure_reason: None,
                },
                CoveragePartition {
                    partition: "cited_and_lexical_evidence".to_owned(),
                    lane: "evidence_verify".to_owned(),
                    completeness: "best_effort".to_owned(),
                    index_revision: Some(session.corpus_revision_ref),
                    searched_count: request.claims.len(),
                    candidate_count: request.claims.len(),
                    failure_reason: None,
                },
            ],
            unsearched: vec![CoveragePartition {
                partition: "collection_completeness_registry".to_owned(),
                lane: "absence_verify".to_owned(),
                completeness: "unavailable".to_owned(),
                index_revision: None,
                searched_count: 0,
                candidate_count: 0,
                failure_reason: Some(
                    "the normalized schema does not yet declare complete collections".to_owned(),
                ),
            }],
            absence_safe: false,
        },
    })
}

async fn verify_claim_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    claim: &VerifyClaim,
    requested_checks: &[String],
    index: usize,
) -> ApiResult<VerifyItemResult> {
    if claim.claim.trim().is_empty() {
        return Err(ApiError::invalid("verify claim text is required"));
    }
    let explicit_evidence =
        resolve_verify_evidence_refs_tx(tx, session, &claim.evidence_refs).await?;

    let (about_id, identity_ambiguity) = if let Some(reference) = &claim.about_ref {
        match resolve_reference_candidates_tx(tx, session, reference).await? {
            candidates if candidates.len() == 1 => {
                let id = candidates[0]
                    .get("ref")
                    .and_then(Value::as_str)
                    .map(|value| parse_uuid_ref(value, &[]))
                    .transpose()?;
                (id, false)
            }
            candidates if candidates.len() > 1 => (None, true),
            _ => (parse_uuid_ref(reference, &[]).ok(), false),
        }
    } else {
        (None, false)
    };

    let claim_rows = matching_claim_rows_tx(tx, session, claim, about_id).await?;
    let claim_ids: Vec<Uuid> = claim_rows
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<Result<_, _>>()?;
    let mut evidence_ids = explicit_evidence;
    for row in &claim_rows {
        evidence_ids.extend(row.try_get::<Vec<Uuid>, _>("evidence_ids")?);
    }
    evidence_ids.sort();
    evidence_ids.dedup();
    let mut evidence = evidence_passages_tx(tx, session, &evidence_ids, &claim_ids).await?;
    if evidence.is_empty() {
        evidence = lexical_evidence_tx(tx, session, &claim.claim, 8).await?;
    }

    let requested_value_match = claim.value.as_ref().is_some_and(|expected| {
        claim_rows
            .iter()
            .filter_map(|row| row.try_get::<Value, _>("value").ok())
            .any(|actual| &actual == expected)
    });
    let structured_match = !claim_rows.is_empty()
        && (claim.value.is_none() || requested_value_match)
        && claim_rows.iter().any(|row| {
            row.try_get::<String, _>("support_state")
                .is_ok_and(|value| value != "refuted")
        });
    let contradicted =
        claim_rows.iter().any(|row| {
            row.try_get::<bool, _>("contradicted").unwrap_or(false)
                || row
                    .try_get::<String, _>("support_state")
                    .is_ok_and(|value| value == "refuted")
        }) || (claim.value.is_some() && !claim_rows.is_empty() && !requested_value_match)
            || evidence
                .iter()
                .any(|item| item.support_kind == "conflicting");
    let superseded = claim_rows
        .iter()
        .any(|row| row.try_get::<bool, _>("superseded").unwrap_or(false));
    let best_lexical_support = evidence
        .iter()
        .filter_map(|passage| passage.text.as_deref())
        .map(|text| lexical_support_ratio(&claim.claim, text))
        .fold(0.0_f64, f64::max);
    let lexical_support = best_lexical_support >= 0.55;
    let temporal_ambiguity = temporal_ambiguity(&claim_rows, claim, best_lexical_support >= 0.9);
    let absence_claim = looks_like_absence_claim(&claim.claim);

    let classification = if contradicted {
        VerificationClassification::Contradicted
    } else if superseded {
        VerificationClassification::Superseded
    } else if temporal_ambiguity {
        VerificationClassification::TemporallyAmbiguous
    } else if absence_claim {
        VerificationClassification::InsufficientEvidence
    } else if structured_match || lexical_support {
        VerificationClassification::Supported
    } else {
        VerificationClassification::InsufficientEvidence
    };
    let checks = effective_verify_checks(requested_checks);
    let structural_checks = structural_checks_tx(
        tx,
        session,
        claim,
        &checks,
        about_id,
        identity_ambiguity,
        structured_match,
        contradicted,
        superseded,
        temporal_ambiguity,
        absence_claim,
        &evidence,
    )
    .await?;
    let explanation = match classification {
        VerificationClassification::Supported => {
            "A current structured claim or lexically matching inspectable passage supports the claim at the pinned revision."
        }
        VerificationClassification::Contradicted => {
            "Current structured value, explicit contradiction lineage, refuted support state, or conflicting evidence contradicts the claim."
        }
        VerificationClassification::Superseded => {
            "The supporting claim has a correction, supersession, or retraction present in the pinned revision."
        }
        VerificationClassification::TemporallyAmbiguous => {
            "The pinned evidence has competing temporal values or lacks the temporal qualification required by this claim."
        }
        VerificationClassification::InsufficientEvidence => {
            "No current, inspectable, structurally valid evidence establishes this claim; absence is never inferred from incomplete coverage."
        }
    }
    .to_owned();

    Ok(VerifyItemResult {
        id: claim.id.clone().unwrap_or_else(|| format!("claim_{index}")),
        claim: claim.claim.clone(),
        classification,
        evidence,
        structural_checks,
        explanation,
    })
}

async fn matching_claim_rows_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    claim: &VerifyClaim,
    about_id: Option<Uuid>,
) -> ApiResult<Vec<sqlx::postgres::PgRow>> {
    if !verify_claim_has_structured_selector(claim, about_id) {
        return Ok(Vec::new());
    }
    Ok(sqlx::query(
        r#"
        SELECT c.id, c.predicate::text AS predicate, c.value, c.support_state,
               c.canonicality, c.authority, c.valid_temporal_id, c.recorded_at,
               ARRAY(SELECT link.evidence_id FROM straylight.claim_evidence_links link
                     WHERE link.user_id = c.user_id AND link.claim_id = c.id
                       AND link.support_kind = 'supporting') AS evidence_ids,
               EXISTS (
                 SELECT 1 FROM straylight.claim_lineage successor
                 JOIN straylight.corpus_members successor_cm
                   ON successor_cm.user_id = successor.user_id
                  AND successor_cm.record_id = successor.claim_id
                 WHERE successor.user_id = c.user_id
                   AND successor.predecessor_claim_id = c.id
                   AND successor.lineage_kind IN ('corrects', 'supersedes', 'retracts')
                   AND successor_cm.corpus_revision_id = $3
                   AND successor_cm.disposition = 'active'
               ) AS superseded,
               EXISTS (
                 SELECT 1 FROM straylight.claim_lineage conflict
                 JOIN straylight.corpus_members conflict_cm
                   ON conflict_cm.user_id = conflict.user_id
                  AND conflict_cm.record_id = conflict.claim_id
                 WHERE conflict.user_id = c.user_id
                   AND (conflict.claim_id = c.id OR conflict.predecessor_claim_id = c.id)
                   AND conflict.lineage_kind = 'contradicts'
                   AND conflict_cm.corpus_revision_id = $3
                   AND conflict_cm.disposition <> 'tombstoned'
               ) AS contradicted
        FROM straylight.claims c
        JOIN straylight.corpus_members cm ON cm.user_id = c.user_id AND cm.record_id = c.id
        WHERE c.user_id = $1 AND c.scope_id = $2
          AND cm.corpus_revision_id = $3 AND cm.disposition <> 'tombstoned'
          AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM straylight.claim_about_targets cat
              WHERE cat.user_id = c.user_id AND cat.claim_id = c.id AND cat.target_record_id = $4))
          AND ($5::text IS NULL OR c.predicate::text = $5)
          AND ($6::jsonb IS NULL OR c.value = $6)
        ORDER BY c.recorded_at DESC, c.id LIMIT 64
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(about_id)
    .bind(claim.predicate.as_deref())
    .bind(claim.value.as_ref())
    .fetch_all(&mut **tx)
    .await?)
}

fn verify_claim_has_structured_selector(claim: &VerifyClaim, about_id: Option<Uuid>) -> bool {
    if claim.about_ref.is_some() && about_id.is_none() {
        return false;
    }
    about_id.is_some() || claim.predicate.is_some() || claim.value.is_some()
}

fn parse_verify_evidence_reference(reference: &str) -> ApiResult<VerifyEvidenceReference> {
    if reference.starts_with("evidence:") {
        return parse_uuid_ref(reference, &["evidence"]).map(VerifyEvidenceReference::Evidence);
    }
    if reference.starts_with("chunk:") {
        return parse_uuid_ref(reference, &["chunk"]).map(VerifyEvidenceReference::Chunk);
    }
    Err(ApiError::invalid(format!(
        "verify evidence refs must be evidence:* or returned chunk:* refs: {reference}"
    )))
}

async fn resolve_verify_evidence_refs_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    references: &[String],
) -> ApiResult<Vec<Uuid>> {
    let mut evidence_ids = Vec::new();
    for reference in references {
        match parse_verify_evidence_reference(reference)? {
            VerifyEvidenceReference::Evidence(id) => evidence_ids.push(id),
            VerifyEvidenceReference::Chunk(chunk_id) => {
                let pinned = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT EXISTS(
                      SELECT 1 FROM straylight.chunks c
                      JOIN straylight.corpus_members cm
                        ON cm.user_id = c.user_id AND cm.record_id = c.id
                      WHERE c.user_id = $1 AND c.scope_id = $2 AND c.id = $4
                        AND cm.corpus_revision_id = $3 AND cm.disposition = 'active'
                    )
                    "#,
                )
                .bind(session.user_id)
                .bind(session.scope_id)
                .bind(session.corpus_revision_id)
                .bind(chunk_id)
                .fetch_one(&mut **tx)
                .await?;
                if !pinned {
                    return Err(ApiError::not_found("chunk_not_found", reference));
                }
                let resolved: Vec<Uuid> = sqlx::query_scalar(
                    r#"
                    SELECT ei.id
                    FROM straylight.chunks c
                    JOIN straylight.corpus_members chunk_cm
                      ON chunk_cm.user_id = c.user_id AND chunk_cm.record_id = c.id
                    JOIN straylight.document_revisions dr
                      ON dr.user_id = c.user_id AND dr.document_id = c.document_id
                     AND dr.version = c.document_version
                    JOIN straylight.evidence_items ei
                      ON ei.user_id = dr.user_id AND ei.scope_id = c.scope_id
                     AND ei.source_episode_id = dr.source_episode_id
                     AND ei.evidence_kind = 'source_native'
                    JOIN straylight.corpus_members evidence_cm
                      ON evidence_cm.user_id = ei.user_id AND evidence_cm.record_id = ei.id
                    WHERE c.user_id = $1 AND c.scope_id = $2 AND c.id = $4
                      AND chunk_cm.corpus_revision_id = $3
                      AND chunk_cm.disposition = 'active'
                      AND evidence_cm.corpus_revision_id = $3
                      AND evidence_cm.disposition = 'active'
                    ORDER BY ei.created_at, ei.id LIMIT 32
                    "#,
                )
                .bind(session.user_id)
                .bind(session.scope_id)
                .bind(session.corpus_revision_id)
                .bind(chunk_id)
                .fetch_all(&mut **tx)
                .await?;
                evidence_ids.extend(resolved);
            }
        }
    }
    evidence_ids.sort();
    evidence_ids.dedup();
    Ok(evidence_ids)
}

async fn evidence_passages_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    evidence_ids: &[Uuid],
    claim_ids: &[Uuid],
) -> ApiResult<Vec<EvidencePassage>> {
    if evidence_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT ei.id, ei.text_content, ei.content_hash, ei.locator, ei.created_at,
               se.source_ref, se.source_version,
               coalesce((SELECT link.support_kind FROM straylight.claim_evidence_links link
                 WHERE link.user_id = ei.user_id AND link.evidence_id = ei.id
                   AND link.claim_id = ANY($5::uuid[])
                 ORDER BY CASE link.support_kind WHEN 'conflicting' THEN 0 ELSE 1 END LIMIT 1), 'cited') AS support_kind
        FROM straylight.evidence_items ei
        JOIN straylight.corpus_members cm ON cm.user_id = ei.user_id AND cm.record_id = ei.id
        JOIN straylight.source_episodes se ON se.user_id = ei.user_id AND se.id = ei.source_episode_id
        WHERE ei.user_id = $1 AND ei.scope_id = $2 AND cm.corpus_revision_id = $3
          AND cm.disposition <> 'tombstoned' AND ei.id = ANY($4::uuid[])
          AND ei.evidence_kind <> 'derived_non_authoritative'
          AND se.source_kind <> 'derived_asset_description'
        ORDER BY ei.created_at, ei.id
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(evidence_ids)
    .bind(claim_ids)
    .fetch_all(&mut **tx)
    .await?;
    evidence_rows(rows)
}

async fn lexical_evidence_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    claim: &str,
    limit: i64,
) -> ApiResult<Vec<EvidencePassage>> {
    let rows = sqlx::query(
        r#"
        WITH requested AS (SELECT websearch_to_tsquery('english', $4) AS terms)
        SELECT ei.id, ei.text_content, ei.content_hash, ei.locator, ei.created_at,
               se.source_ref, se.source_version, 'lexical_candidate'::text AS support_kind
        FROM straylight.evidence_items ei
        JOIN straylight.corpus_members cm ON cm.user_id = ei.user_id AND cm.record_id = ei.id
        JOIN straylight.source_episodes se ON se.user_id = ei.user_id AND se.id = ei.source_episode_id
        CROSS JOIN requested
        WHERE ei.user_id = $1 AND ei.scope_id = $2 AND cm.corpus_revision_id = $3
          AND cm.disposition <> 'tombstoned' AND ei.text_content IS NOT NULL
          AND ei.evidence_kind <> 'derived_non_authoritative'
          AND se.source_kind <> 'derived_asset_description'
          AND to_tsvector('english', ei.text_content) @@ requested.terms
        ORDER BY ts_rank_cd(to_tsvector('english', ei.text_content), requested.terms) DESC,
                 ei.created_at DESC LIMIT $5
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(session.corpus_revision_id)
    .bind(claim)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    evidence_rows(rows)
}

fn evidence_rows(rows: Vec<sqlx::postgres::PgRow>) -> ApiResult<Vec<EvidencePassage>> {
    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            Ok(EvidencePassage {
                evidence_ref: record_ref("evidence", id),
                source_ref: row.try_get("source_ref")?,
                source_version: row.try_get("source_version")?,
                content_hash: prefixed_hash(row.try_get("content_hash")?),
                locator: row.try_get("locator")?,
                text: row.try_get("text_content")?,
                support_kind: row.try_get("support_kind")?,
                recorded_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()
        .map_err(ApiError::from)
}

#[allow(clippy::too_many_arguments)]
async fn structural_checks_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    claim: &VerifyClaim,
    checks: &[String],
    about_id: Option<Uuid>,
    identity_ambiguity: bool,
    structured_match: bool,
    contradicted: bool,
    superseded: bool,
    temporal_ambiguity: bool,
    absence_claim: bool,
    evidence: &[EvidencePassage],
) -> ApiResult<Vec<StructuralCheck>> {
    let mut results = Vec::new();
    for check in checks {
        let result = match check.as_str() {
            "newer_evidence" => StructuralCheck {
                check: check.clone(),
                status: "snapshot_bounded".to_owned(),
                details: json!({
                    "message": "Verification does not inspect revisions newer than the immutable session; refresh the session to check them.",
                    "corpus_revision": session.corpus_revision_ref
                }),
            },
            "contradictions" => StructuralCheck {
                check: check.clone(),
                status: if contradicted { "failed" } else { "passed" }.to_owned(),
                details: json!({"contradiction_present": contradicted}),
            },
            "superseded_sources" => StructuralCheck {
                check: check.clone(),
                status: if superseded { "failed" } else { "passed" }.to_owned(),
                details: json!({"superseded": superseded}),
            },
            "unsupported_claims" => StructuralCheck {
                check: check.clone(),
                status: if structured_match || !evidence.is_empty() {
                    "passed"
                } else {
                    "failed"
                }
                .to_owned(),
                details: json!({"structured_match": structured_match, "inspectable_passages": evidence.len()}),
            },
            "identity_ambiguity" => StructuralCheck {
                check: check.clone(),
                status: if identity_ambiguity {
                    "failed"
                } else {
                    "passed"
                }
                .to_owned(),
                details: json!({
                    "ambiguous": identity_ambiguity,
                    "rule": "name, contact, handle, and embedding matches never establish same_as"
                }),
            },
            "named_state_mismatch" => named_state_check_tx(tx, session, claim, about_id).await?,
            "recurrence_or_occurrence_loss" => recurrence_check_tx(tx, session, about_id).await?,
            "incomplete_collection_coverage" => StructuralCheck {
                check: check.clone(),
                status: if absence_claim {
                    "failed"
                } else {
                    "not_applicable"
                }
                .to_owned(),
                details: json!({
                    "absence_claim": absence_claim,
                    "absence_safe": false,
                    "reason": "No collection-completeness registry exists in the current normalized schema"
                }),
            },
            "temporal_ambiguity" => StructuralCheck {
                check: check.clone(),
                status: if temporal_ambiguity {
                    "failed"
                } else {
                    "passed"
                }
                .to_owned(),
                details: json!({"temporally_ambiguous": temporal_ambiguity}),
            },
            other => StructuralCheck {
                check: other.to_owned(),
                status: "unsupported".to_owned(),
                details: json!({"reason": "unknown verification check"}),
            },
        };
        results.push(result);
    }
    Ok(results)
}

async fn named_state_check_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    claim: &VerifyClaim,
    about_id: Option<Uuid>,
) -> ApiResult<StructuralCheck> {
    let Some(predicate) = claim
        .predicate
        .as_deref()
        .filter(|value| value.starts_with("state:"))
    else {
        return Ok(StructuralCheck {
            check: "named_state_mismatch".to_owned(),
            status: "not_applicable".to_owned(),
            details: json!({}),
        });
    };
    let Some(about_id) = about_id else {
        return Ok(StructuralCheck {
            check: "named_state_mismatch".to_owned(),
            status: "failed".to_owned(),
            details: json!({"reason": "state verification requires an unambiguous about_ref"}),
        });
    };
    let current: Option<String> = sqlx::query_scalar(
        r#"
        SELECT sa.state_value FROM straylight.state_assignments sa
        JOIN straylight.claims c ON c.user_id = sa.user_id AND c.id = sa.claim_id
        JOIN straylight.corpus_members cm ON cm.user_id = sa.user_id AND cm.record_id = sa.claim_id
        WHERE sa.user_id = $1 AND sa.scope_id = $2 AND sa.target_record_id = $3
          AND sa.state_machine_ref::text = $4 AND cm.corpus_revision_id = $5
          AND cm.disposition = 'active'
        ORDER BY c.recorded_at DESC, c.id DESC LIMIT 1
        "#,
    )
    .bind(session.user_id)
    .bind(session.scope_id)
    .bind(about_id)
    .bind(predicate)
    .bind(session.corpus_revision_id)
    .fetch_optional(&mut **tx)
    .await?;
    let expected = claim.value.as_ref().and_then(Value::as_str);
    Ok(StructuralCheck {
        check: "named_state_mismatch".to_owned(),
        status: if current.as_deref() == expected {
            "passed"
        } else {
            "failed"
        }
        .to_owned(),
        details: json!({
            "machine_ref": predicate,
            "expected": expected,
            "current": current,
            "rule": "one named state machine never implies another"
        }),
    })
}

async fn recurrence_check_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &PinnedSession,
    about_id: Option<Uuid>,
) -> ApiResult<StructuralCheck> {
    let Some(about_id) = about_id else {
        return Ok(StructuralCheck {
            check: "recurrence_or_occurrence_loss".to_owned(),
            status: "not_applicable".to_owned(),
            details: json!({}),
        });
    };
    let row = sqlx::query(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM straylight.event_series_revisions esr
          JOIN straylight.corpus_members cm ON cm.user_id = esr.user_id AND cm.record_id = esr.object_id AND cm.record_version = esr.object_version
          WHERE esr.user_id = $1 AND esr.object_id = $2 AND cm.corpus_revision_id = $3
        ) AS is_series,
        (SELECT count(*)::bigint FROM straylight.event_occurrences eo
          JOIN straylight.corpus_members cm ON cm.user_id = eo.user_id AND cm.record_id = eo.occurrence_object_id
          WHERE eo.user_id = $1 AND eo.series_object_id = $2 AND cm.corpus_revision_id = $3 AND cm.disposition <> 'tombstoned') AS occurrences,
        (SELECT count(*)::bigint FROM straylight.event_occurrences eo
          JOIN straylight.corpus_members cm ON cm.user_id = eo.user_id AND cm.record_id = eo.occurrence_object_id
          JOIN straylight.event_occurrence_revisions eor ON eor.user_id = eo.user_id AND eor.occurrence_object_id = eo.occurrence_object_id AND eor.object_version = cm.record_version
          WHERE eo.user_id = $1 AND eo.series_object_id = $2 AND cm.corpus_revision_id = $3 AND eor.exception_kind <> 'generated') AS exceptions
        "#,
    )
    .bind(session.user_id)
    .bind(about_id)
    .bind(session.corpus_revision_id)
    .fetch_one(&mut **tx)
    .await?;
    let is_series: bool = row.try_get("is_series")?;
    let occurrences: i64 = row.try_get("occurrences")?;
    let exceptions: i64 = row.try_get("exceptions")?;
    Ok(StructuralCheck {
        check: "recurrence_or_occurrence_loss".to_owned(),
        status: if !is_series {
            "not_applicable"
        } else if occurrences > 0 {
            "passed"
        } else {
            "failed"
        }
        .to_owned(),
        details: json!({
            "is_series": is_series,
            "materialized_occurrences": occurrences,
            "sparse_exceptions": exceptions
        }),
    })
}

fn effective_verify_checks(requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        [
            "newer_evidence",
            "contradictions",
            "superseded_sources",
            "unsupported_claims",
            "identity_ambiguity",
            "named_state_mismatch",
            "recurrence_or_occurrence_loss",
            "incomplete_collection_coverage",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    } else {
        requested.to_vec()
    }
}

fn temporal_ambiguity(
    rows: &[sqlx::postgres::PgRow],
    claim: &VerifyClaim,
    strong_lexical_support: bool,
) -> bool {
    let values: BTreeSet<String> = rows
        .iter()
        .filter_map(|row| row.try_get::<Value, _>("value").ok())
        .map(|value| canonical_value_key(&value))
        .collect();
    let has_typed_temporal_qualification = rows.iter().any(|row| {
        row.try_get::<Option<Uuid>, _>("valid_temporal_id")
            .ok()
            .flatten()
            .is_some()
    });
    temporal_ambiguity_from_facts(
        values.len(),
        has_typed_temporal_qualification,
        claim,
        strong_lexical_support,
    )
}

fn temporal_ambiguity_from_facts(
    distinct_structured_values: usize,
    has_typed_temporal_qualification: bool,
    claim: &VerifyClaim,
    strong_lexical_support: bool,
) -> bool {
    let temporal_predicate = claim.predicate.as_deref().is_some_and(|predicate| {
        predicate.contains("time") || predicate.contains("schedule") || predicate.contains("due")
    });
    let lower = claim.claim.to_ascii_lowercase();
    let temporal_language = [
        "today",
        "tomorrow",
        "yesterday",
        "currently",
        "scheduled",
        "due",
        " at ",
        " on ",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if distinct_structured_values > 1 {
        return true;
    }
    if !(temporal_predicate || temporal_language) || has_typed_temporal_qualification {
        return false;
    }
    let relative_language = [
        "today",
        "tomorrow",
        "yesterday",
        "currently",
        " right now",
        " next ",
        " last ",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    relative_language || !(strong_lexical_support && has_absolute_temporal_language(&lower))
}

fn has_absolute_temporal_language(lower: &str) -> bool {
    let has_clock = lower.as_bytes().windows(5).any(|window| {
        window[0].is_ascii_digit()
            && window[1].is_ascii_digit()
            && window[2] == b':'
            && window[3].is_ascii_digit()
            && window[4].is_ascii_digit()
    });
    let has_named_month = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ]
    .iter()
    .any(|month| lower.contains(month));
    let has_iso_date = lower.as_bytes().windows(10).any(|window| {
        window[0..4].iter().all(u8::is_ascii_digit)
            && window[4] == b'-'
            && window[5..7].iter().all(u8::is_ascii_digit)
            && window[7] == b'-'
            && window[8..10].iter().all(u8::is_ascii_digit)
    });
    has_clock || has_named_month || has_iso_date
}

fn lexical_support_ratio(claim: &str, passage: &str) -> f64 {
    let claim_tokens = meaningful_tokens(claim);
    if claim_tokens.is_empty() {
        return 0.0;
    }
    let passage_tokens = meaningful_tokens(passage);
    let overlap = claim_tokens.intersection(&passage_tokens).count();
    overlap as f64 / claim_tokens.len() as f64
}

fn meaningful_tokens(value: &str) -> HashSet<String> {
    const STOP: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "in", "is",
        "it", "of", "on", "or", "that", "the", "this", "to", "was", "were", "with",
    ];
    value
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '-'
        })
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() > 1 && !STOP.contains(&token.as_str()))
        .collect()
}

fn looks_like_absence_claim(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("no ")
        || value.starts_with("none ")
        || value.contains(" does not exist")
        || value.contains(" never ")
        || value.contains("nothing ")
        || value.contains("no one ")
}

fn is_stage_ref(value: &str) -> bool {
    value.starts_with("stage:")
}

fn parse_stage_ref(value: &str) -> ApiResult<(Uuid, Option<String>)> {
    let raw = value
        .strip_prefix("stage:")
        .ok_or_else(|| ApiError::invalid(format!("invalid stage reference: {value}")))?;
    let (id, revision) = raw
        .split_once('@')
        .map(|(id, revision)| (id, Some(revision.to_owned())))
        .unwrap_or((raw, None));
    if revision.as_deref() == Some("") {
        return Err(ApiError::invalid("stage revision suffix cannot be empty"));
    }
    let id = Uuid::parse_str(id)
        .map_err(|_| ApiError::invalid(format!("invalid stage reference: {value}")))?;
    Ok((id, revision))
}

async fn load_staged_corpus(
    state: &AppState,
    auth: &AuthContext,
    stage_ref: &str,
    capability: Capability,
) -> ApiResult<StagedCorpus> {
    auth.require(capability)?;
    let (stage_id, requested_revision) = parse_stage_ref(stage_ref)?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT stage.id,stage.scope_id,scope.scope_ref,stage.credential_id,
               stage.base_corpus_revision_id,stage.input_hash,stage.inventory_hash,
               stage.status,stage.created_at,stage.expires_at
        FROM straylight.stages AS stage
        JOIN straylight.scopes AS scope
          ON scope.user_id=stage.user_id AND scope.id=stage.scope_id
        WHERE stage.user_id=$1 AND stage.id=$2 AND stage.credential_id=$3
          AND scope.scope_ref=ANY($4::text[])
          AND stage.inventory_hash IS NOT NULL
          AND stage.status IN ('ready','promoted')
          AND stage.expires_at > clock_timestamp()
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id)
    .bind(auth.credential_id.0)
    .bind(&auth.scope_refs)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("stage_not_found", stage_ref))?;
    tx.commit().await?;
    let inventory_hash: String = row.try_get("inventory_hash")?;
    if let Some(requested) = requested_revision {
        let requested = requested.strip_prefix("sha256:").unwrap_or(&requested);
        if requested != inventory_hash {
            return Err(ApiError::conflict(
                "stage_revision_mismatch",
                "stage reference does not match the immutable inventory hash",
                json!({
                    "requested": requested,
                    "actual": format!("sha256:{inventory_hash}")
                }),
            ));
        }
    }
    Ok(StagedCorpus {
        id: stage_id,
        stage_ref: record_ref("stage", stage_id),
        scope_id: row.try_get("scope_id")?,
        scope_ref: row.try_get("scope_ref")?,
        credential_id: row.try_get("credential_id")?,
        base_corpus_revision_id: row.try_get("base_corpus_revision_id")?,
        input_hash: row.try_get("input_hash")?,
        inventory_hash,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}

async fn load_staged_entries(
    state: &AppState,
    auth: &AuthContext,
    stage: &StagedCorpus,
) -> ApiResult<Vec<StagedEntry>> {
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.entry_kind,entry.media_type,entry.size_bytes,
               entry.content_hash,entry.readability,entry.inspection,entry.asset_id,
               entry.asset_version,asset.object_key,asset.object_version_id,
               entry.created_at
        FROM straylight.staged_entries AS entry
        LEFT JOIN straylight.asset_versions AS asset
          ON asset.user_id=entry.user_id AND asset.asset_id=entry.asset_id
         AND asset.version=entry.asset_version
        WHERE entry.user_id=$1 AND entry.scope_id=$2 AND entry.stage_id=$3
        ORDER BY entry.path,entry.id
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage.scope_id)
    .bind(stage.id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    rows.into_iter()
        .map(|row| {
            Ok(StagedEntry {
                id: row.try_get("id")?,
                path: row.try_get("path")?,
                entry_kind: row.try_get("entry_kind")?,
                media_type: row.try_get("media_type")?,
                size_bytes: row.try_get("size_bytes")?,
                content_hash: row.try_get("content_hash")?,
                readability: row.try_get("readability")?,
                inspection: row.try_get("inspection")?,
                asset_id: row.try_get("asset_id")?,
                asset_version: row.try_get("asset_version")?,
                object_key: row.try_get("object_key")?,
                object_version_id: row.try_get("object_version_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()
        .map_err(ApiError::from)
}

fn public_staged_entry(entry: &StagedEntry) -> Value {
    let mut inspection = entry.inspection.clone();
    if let Some(object) = inspection.as_object_mut() {
        object.remove("search_text");
    }
    json!({
        "ref": record_ref("stage-entry", entry.id),
        "path": entry.path,
        "entry_kind": entry.entry_kind,
        "media_type": entry.media_type,
        "size_bytes": entry.size_bytes,
        "content_hash": entry.content_hash.clone().map(prefixed_hash),
        "readability": entry.readability,
        "inspection": inspection,
        "native_asset": entry.asset_id.map(|asset_id| json!({
            "asset_ref": record_ref("asset", asset_id),
            "version": entry.asset_version,
            "inline": false
        })),
        "created_at": entry.created_at
    })
}

fn staged_catalog(stage: &StagedCorpus, entries: &[StagedEntry]) -> Value {
    let readable = entries
        .iter()
        .filter(|entry| entry.readability == "readable")
        .count();
    let quarantined = entries
        .iter()
        .filter(|entry| entry.readability == "quarantined")
        .count();
    json!({
        "stage_ref": stage.stage_ref,
        "stage_revision": stage.revision_ref(),
        "scope": stage.scope_ref,
        "status": stage.status,
        "input_hash": prefixed_hash(stage.input_hash.clone()),
        "inventory_hash": prefixed_hash(stage.inventory_hash.clone()),
        "base_corpus_revision": record_ref("revision", stage.base_corpus_revision_id),
        "created_at": stage.created_at,
        "expires_at": stage.expires_at,
        "authorization": {
            "credential_bound": true,
            "credential_matches": stage.credential_id != Uuid::nil()
        },
        "inventory_summary": {
            "entries": entries.len(),
            "readable": readable,
            "opaque": entries.len().saturating_sub(readable + quarantined),
            "quarantined": quarantined
        },
        "entries": entries.iter().map(public_staged_entry).collect::<Vec<_>>()
    })
}

fn validate_staged_query_scope(stage: &StagedCorpus, scope: Option<&QueryScope>) -> ApiResult<()> {
    let requested = match scope {
        Some(QueryScope::Ref(value)) => Some(value.as_str()),
        Some(QueryScope::Structured {
            authorization_scope,
            ..
        }) => authorization_scope.as_deref(),
        None => None,
    };
    if requested.is_some_and(|value| value != stage.stage_ref && value != stage.scope_ref) {
        return Err(ApiError::public(
            http::StatusCode::FORBIDDEN,
            "capability_denied",
            "query scope cannot broaden the immutable staged corpus",
        ));
    }
    Ok(())
}

async fn run_staged_query_batch(
    state: &AppState,
    auth: &AuthContext,
    stage: &StagedCorpus,
    specs: Vec<QuerySpec>,
) -> ApiResult<QueryBatchResult> {
    let entries = load_staged_entries(state, auth, stage).await?;
    let mut items = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        validate_staged_query_scope(stage, spec.scope.as_ref())?;
        validate_staged_query_filters(stage, &entries, spec)?;
        let modes = effective_modes(spec);
        let mut results = Vec::new();
        let mut complete_text_search = true;
        for entry in &entries {
            match staged_query_candidate(state, stage, entry, spec, &modes).await {
                Ok((Some(candidate), complete)) => {
                    complete_text_search &= complete;
                    results.push(candidate);
                }
                Ok((None, complete)) => complete_text_search &= complete,
                Err(_) => complete_text_search = false,
            }
        }
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.reference.cmp(&right.reference))
        });
        results.truncate(spec.limit);

        let mut coverage = Coverage::default();
        let mut unsupported_requested = false;
        for mode in [
            RetrievalMode::Exact,
            RetrievalMode::Structured,
            RetrievalMode::Lexical,
            RetrievalMode::Semantic,
            RetrievalMode::Temporal,
            RetrievalMode::Relations,
        ] {
            if !modes.contains(&mode) {
                coverage.unsearched.push(CoveragePartition {
                    partition: mode_name(mode).to_owned(),
                    lane: mode_name(mode).to_owned(),
                    completeness: "not_requested".to_owned(),
                    index_revision: Some(stage.revision_ref()),
                    searched_count: 0,
                    candidate_count: 0,
                    failure_reason: None,
                });
            } else if matches!(
                mode,
                RetrievalMode::Exact | RetrievalMode::Structured | RetrievalMode::Lexical
            ) {
                coverage.searched.push(CoveragePartition {
                    partition: "staged_entries".to_owned(),
                    lane: mode_name(mode).to_owned(),
                    completeness: if complete_text_search {
                        "complete"
                    } else {
                        "best_effort"
                    }
                    .to_owned(),
                    index_revision: Some(stage.revision_ref()),
                    searched_count: entries.len(),
                    candidate_count: results.len(),
                    failure_reason: (!complete_text_search)
                        .then(|| "one or more staged text derivatives were unavailable".to_owned()),
                });
            } else {
                unsupported_requested = true;
                coverage.unsearched.push(CoveragePartition {
                    partition: mode_name(mode).to_owned(),
                    lane: mode_name(mode).to_owned(),
                    completeness: "unsupported_for_stage".to_owned(),
                    index_revision: Some(stage.revision_ref()),
                    searched_count: 0,
                    candidate_count: 0,
                    failure_reason: Some(
                        "temporary staged corpora expose exact, structured, and lexical lanes"
                            .to_owned(),
                    ),
                });
            }
        }
        coverage.absence_safe = complete_text_search && !unsupported_requested;
        items.push(QueryItemResult {
            id: spec.id.clone().unwrap_or_else(|| format!("q{index}")),
            status: if complete_text_search && !unsupported_requested {
                ResponseStatus::Complete
            } else {
                ResponseStatus::Partial
            },
            results,
            coverage,
            conflicts: Vec::new(),
            gaps: Vec::new(),
            ambiguities: Vec::new(),
        });
    }
    let status = if items
        .iter()
        .all(|item| item.status == ResponseStatus::Complete)
    {
        ResponseStatus::Complete
    } else {
        ResponseStatus::Partial
    };
    let mut coverage = Coverage::default();
    for item in &items {
        coverage.searched.extend(item.coverage.searched.clone());
        coverage.unsearched.extend(item.coverage.unsearched.clone());
    }
    coverage.absence_safe = items.iter().all(|item| item.coverage.absence_safe);
    Ok(QueryBatchResult {
        session_id: stage.stage_ref.clone(),
        corpus_revision: stage.revision_ref(),
        status,
        items,
        freshness: stage.freshness(),
        coverage,
    })
}

async fn staged_query_candidate(
    state: &AppState,
    stage: &StagedCorpus,
    entry: &StagedEntry,
    spec: &QuerySpec,
    modes: &[RetrievalMode],
) -> ApiResult<(Option<RetrievalCandidate>, bool)> {
    if !staged_entry_matches_filters(entry, &spec.r#where)
        || !staged_entry_matches_roots(stage, entry, spec)
    {
        return Ok((None, true));
    }
    let query = spec.query.as_deref().unwrap_or("").trim();
    let (text, complete) = staged_search_text(state, entry).await?;
    let path_lower = entry.path.to_ascii_lowercase();
    let text_lower = text.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    let mut score = 0.0_f64;
    let mut reasons = Vec::new();
    if modes.contains(&RetrievalMode::Exact) && !query.is_empty() {
        if path_lower == query_lower {
            score += 5.0;
            reasons.push("exact_path_match".to_owned());
        } else if path_lower.contains(&query_lower) || text_lower.contains(&query_lower) {
            score += 4.0;
            reasons.push("exact_phrase_match".to_owned());
        }
    }
    if modes.contains(&RetrievalMode::Lexical) && !query.is_empty() {
        let ratio = lexical_support_ratio(query, &format!("{}\n{text}", entry.path));
        if ratio > 0.0 {
            score += ratio * 2.0;
            reasons.push("lexical_recall".to_owned());
        }
    }
    if modes.contains(&RetrievalMode::Structured) && !spec.r#where.is_empty() {
        score += 1.0;
        reasons.push("structured_filter_match".to_owned());
    }
    if query.is_empty() && !spec.r#where.is_empty() && score == 0.0 {
        score = 1.0;
    }
    if score == 0.0 {
        return Ok((None, complete));
    }
    let metadata = public_staged_entry(entry);
    let content = if text.is_empty() {
        serde_json::to_string(&metadata)?
    } else {
        staged_query_snippet(&text, query, 4_000)
    };
    let content_hash = entry
        .content_hash
        .clone()
        .map(prefixed_hash)
        .unwrap_or(sha256_prefixed_json(&metadata)?);
    let reference = record_ref("stage-entry", entry.id);
    Ok((
        Some(RetrievalCandidate {
            reference: reference.clone(),
            source_ref: stage.stage_ref.clone(),
            path: Some(entry.path.clone()),
            heading: entry.path.rsplit('/').next().map(str::to_owned),
            content,
            content_hash: content_hash.clone(),
            source_version: Some(content_hash),
            authority: Some("staged_untrusted".to_owned()),
            canonicality: Some("temporary".to_owned()),
            provenance: None,
            recorded_at: Some(entry.created_at.to_rfc3339()),
            valid_time: None,
            evidence_refs: vec![reference],
            why_selected: reasons,
            lane_scores: HashMap::new(),
            score,
        }),
        complete,
    ))
}

fn staged_entry_matches_filters(entry: &StagedEntry, filters: &Map<String, Value>) -> bool {
    filters.iter().all(|(key, expected)| {
        let expected = expected.as_str().unwrap_or_default();
        match key.as_str() {
            "scope_root" => true,
            "record_kind" => entry.entry_kind == expected || expected == "stage_entry",
            _ => false,
        }
    })
}

fn validate_staged_query_filters(
    stage: &StagedCorpus,
    entries: &[StagedEntry],
    spec: &QuerySpec,
) -> ApiResult<()> {
    for key in spec.r#where.keys() {
        if !matches!(key.as_str(), "scope_root" | "record_kind") {
            return Err(ApiError::invalid(format!(
                "structured filter {key} is not supported for staged corpora"
            )));
        }
    }
    for root in query_root_references(spec) {
        if root != stage.stage_ref
            && !entries
                .iter()
                .any(|entry| staged_root_contains_entry(&root, entry))
        {
            return Err(ApiError::not_found("stage_root_not_found", &root));
        }
    }
    Ok(())
}

fn staged_entry_matches_roots(stage: &StagedCorpus, entry: &StagedEntry, spec: &QuerySpec) -> bool {
    let roots = query_root_references(spec);
    roots.is_empty()
        || roots
            .iter()
            .any(|root| root == &stage.stage_ref || staged_root_contains_entry(root, entry))
}

fn staged_root_contains_entry(root: &str, entry: &StagedEntry) -> bool {
    root == entry.path
        || root == record_ref("stage-entry", entry.id)
        || entry.path.starts_with(&format!("{root}/"))
        || entry.path.starts_with(&format!("{root}!/"))
}

async fn staged_search_text(state: &AppState, entry: &StagedEntry) -> ApiResult<(String, bool)> {
    if entry.readability != "readable" {
        return Ok((String::new(), true));
    }
    let preview = entry
        .inspection
        .get("search_text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let complete = entry
        .inspection
        .get("text_index_complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if complete {
        return Ok((preview.to_owned(), true));
    }
    match load_staged_text(state, entry).await {
        Ok(text) => Ok((text, true)),
        Err(_error) if !preview.is_empty() => Ok((preview.to_owned(), false)),
        Err(error) => Err(error),
    }
}

async fn load_staged_text(state: &AppState, entry: &StagedEntry) -> ApiResult<String> {
    let key = entry.object_key.as_deref().ok_or_else(|| {
        ApiError::not_found(
            "staged_asset_not_found",
            &record_ref("stage-entry", entry.id),
        )
    })?;
    let bytes = state
        .object_store
        .get_version(key, entry.object_version_id.as_deref())
        .await?;
    verify_staged_bytes(entry, &bytes)?;
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
        ApiError::public(
            http::StatusCode::UNPROCESSABLE_ENTITY,
            "native_binary_required",
            "binary staged content cannot be inlined as model context",
        )
    })?;
    if text.contains('\0') {
        return Err(ApiError::public(
            http::StatusCode::UNPROCESSABLE_ENTITY,
            "native_binary_required",
            "binary staged content cannot be inlined as model context",
        ));
    }
    Ok(text)
}

fn verify_staged_bytes(entry: &StagedEntry, bytes: &[u8]) -> ApiResult<()> {
    let expected = entry.content_hash.as_deref().ok_or_else(|| {
        ApiError::conflict(
            "content_hash_missing",
            "staged content has no immutable hash",
            json!({"path": entry.path}),
        )
    })?;
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(ApiError::conflict(
            "content_hash_mismatch",
            "staged content no longer matches its immutable inventory hash",
            json!({
                "path": entry.path,
                "expected": prefixed_hash(expected.to_owned()),
                "actual": prefixed_hash(actual)
            }),
        ));
    }
    Ok(())
}

fn staged_query_snippet(text: &str, query: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let query_lower = query.to_ascii_lowercase();
    let text_lower = text.to_ascii_lowercase();
    let match_byte = (!query.is_empty())
        .then(|| text_lower.find(&query_lower))
        .flatten()
        .unwrap_or(0);
    let start = text[..match_byte]
        .char_indices()
        .rev()
        .nth(max_chars / 4)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let end = bounded_char_end(text, start, text.len(), max_chars);
    text[start..end].to_owned()
}

async fn read_staged_corpus(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    stage: &StagedCorpus,
    request: &ReadRequest,
) -> ApiResult<ReadBatchResult> {
    let entries = load_staged_entries(state, auth, stage).await?;
    let mut items = Vec::with_capacity(request.requests.len());
    for spec in &request.requests {
        match read_staged_item(state, codec, stage, &entries, spec).await {
            Ok(item) => items.push(item),
            Err(error) => items.push(ReadItemResult {
                reference: spec.reference.clone(),
                view: read_view_name(spec.view).to_owned(),
                status: ItemStatus::Failed,
                data: None,
                error: Some(item_error(error)),
                truncation: Truncation::default(),
            }),
        }
    }
    let successful = items
        .iter()
        .filter(|item| item.status != ItemStatus::Failed)
        .count();
    let status = if items.iter().all(|item| item.status == ItemStatus::Complete) {
        ResponseStatus::Complete
    } else if successful == 0 {
        ResponseStatus::Inconsistent
    } else {
        ResponseStatus::Partial
    };
    Ok(ReadBatchResult {
        session_id: stage.stage_ref.clone(),
        corpus_revision: stage.revision_ref(),
        status,
        items,
        freshness: stage.freshness(),
        coverage: Coverage {
            searched: vec![CoveragePartition {
                partition: "staged_entries".to_owned(),
                lane: "read".to_owned(),
                completeness: if successful == request.requests.len() {
                    "complete"
                } else {
                    "partial"
                }
                .to_owned(),
                index_revision: Some(stage.revision_ref()),
                searched_count: request.requests.len(),
                candidate_count: successful,
                failure_reason: None,
            }],
            unsearched: Vec::new(),
            absence_safe: false,
        },
    })
}

async fn read_staged_item(
    state: &AppState,
    codec: &ContinuationCodec,
    stage: &StagedCorpus,
    entries: &[StagedEntry],
    spec: &ReadSpec,
) -> ApiResult<ReadItemResult> {
    if stage_reference_matches(stage, &spec.reference)
        || (spec.view == ReadView::MaterializeScope && spec.reference == stage.scope_ref)
    {
        return Ok(complete_read_item(spec, staged_catalog(stage, entries)));
    }
    let reference = spec
        .reference
        .strip_prefix(&format!("{}/", stage.stage_ref))
        .unwrap_or(&spec.reference);
    let entry = if reference.starts_with("stage-entry:") {
        let id = parse_uuid_ref(reference, &["stage-entry"])?;
        entries.iter().find(|entry| entry.id == id)
    } else {
        entries.iter().find(|entry| entry.path == reference)
    };
    let Some(entry) = entry else {
        let descendants = staged_descendants(entries, reference);
        if descendants.is_empty() {
            return Err(ApiError::not_found("stage_entry_not_found", reference));
        }
        return Ok(complete_read_item(
            spec,
            json!({
                "ref": reference,
                "entry_kind": "directory",
                "virtual": true,
                "children": descendants
            }),
        ));
    };

    let metadata = public_staged_entry(entry);
    match spec.view {
        ReadView::Full | ReadView::Range if entry.readability == "readable" => {
            let text = load_staged_text(state, entry).await?;
            text_read_result_for_snapshot(
                codec,
                &stage.stage_ref,
                &stage.revision_ref(),
                spec,
                LoadedText { metadata, text },
            )
        }
        ReadView::Full | ReadView::Range
            if matches!(entry.entry_kind.as_str(), "archive" | "directory") =>
        {
            Ok(complete_read_item(
                spec,
                json!({
                    "metadata": metadata,
                    "children": staged_entry_children(entries, entry)
                }),
            ))
        }
        ReadView::Full | ReadView::Range => Ok(ReadItemResult {
            reference: spec.reference.clone(),
            view: read_view_name(spec.view).to_owned(),
            status: ItemStatus::Unsupported,
            data: Some(metadata),
            error: Some(ItemError {
                code: if entry.readability == "quarantined" {
                    "staged_entry_quarantined"
                } else {
                    "native_binary_required"
                }
                .to_owned(),
                message: if entry.readability == "quarantined" {
                    "quarantined staged bytes are not readable or promotable"
                } else {
                    "binary staged content is available only through its native asset handle"
                }
                .to_owned(),
                details: Some(json!({"path": entry.path})),
            }),
            truncation: Truncation::default(),
        }),
        ReadView::CurrentState | ReadView::Structured => Ok(complete_read_item(spec, metadata)),
        ReadView::Outline => Ok(complete_read_item(
            spec,
            json!({
                "metadata": metadata,
                "children": staged_entry_children(entries, entry)
            }),
        )),
        ReadView::Neighbors => Ok(complete_read_item(
            spec,
            Value::Array(staged_siblings(entries, entry)),
        )),
        ReadView::Relationships | ReadView::History | ReadView::Diff | ReadView::LastKnownGood => {
            Ok(ReadItemResult {
                reference: spec.reference.clone(),
                view: read_view_name(spec.view).to_owned(),
                status: ItemStatus::Unsupported,
                data: Some(json!({
                    "ref": record_ref("stage-entry", entry.id),
                    "message": "temporary staged entries have no durable history or relation graph"
                })),
                error: None,
                truncation: Truncation::default(),
            })
        }
        ReadView::MaterializeScope => Err(ApiError::invalid(
            "materialize_scope requires the immutable stage_ref",
        )),
    }
}

fn stage_reference_matches(stage: &StagedCorpus, reference: &str) -> bool {
    parse_stage_ref(reference)
        .map(|(id, _)| id == stage.id)
        .unwrap_or(false)
}

fn staged_descendants(entries: &[StagedEntry], reference: &str) -> Vec<Value> {
    let slash_prefix = format!("{}/", reference.trim_end_matches('/'));
    let archive_prefix = format!("{}!/", reference.trim_end_matches("!/"));
    entries
        .iter()
        .filter(|entry| {
            entry.path.starts_with(&slash_prefix) || entry.path.starts_with(&archive_prefix)
        })
        .map(public_staged_entry)
        .collect()
}

fn staged_entry_children(entries: &[StagedEntry], parent: &StagedEntry) -> Vec<Value> {
    let prefix = if parent.entry_kind == "archive" {
        format!("{}!/", parent.path)
    } else {
        format!("{}/", parent.path.trim_end_matches('/'))
    };
    entries
        .iter()
        .filter(|entry| entry.path.starts_with(&prefix))
        .map(public_staged_entry)
        .collect()
}

fn staged_siblings(entries: &[StagedEntry], target: &StagedEntry) -> Vec<Value> {
    let parent = target
        .path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    entries
        .iter()
        .filter(|entry| {
            entry.id != target.id
                && entry
                    .path
                    .rsplit_once('/')
                    .map(|(candidate, _)| candidate)
                    .unwrap_or("")
                    == parent
        })
        .map(public_staged_entry)
        .collect()
}

async fn compute_staged_corpus(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    stage: &StagedCorpus,
    request: &ComputeRequest,
) -> ApiResult<ComputeResult> {
    validate_compute_dag(&request.steps)?;
    let entries = load_staged_entries(state, auth, stage).await?;
    let max_rows = request
        .max_rows
        .unwrap_or(MAX_COMPUTE_ROWS)
        .min(MAX_COMPUTE_ROWS);
    let token_budget = request.token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET).max(1);
    let mut outputs = HashMap::<String, Value>::new();
    let mut steps = Vec::with_capacity(request.steps.len());
    let mut rows_returned = 0usize;
    let mut estimated_tokens = 0usize;

    for step in &request.steps {
        let missing_dependency = step
            .depends_on
            .iter()
            .find(|dependency| !outputs.contains_key(*dependency));
        let mut execution = if let Some(dependency) = missing_dependency {
            StepExecution {
                status: ItemStatus::Failed,
                output: Value::Null,
                evidence_refs: Vec::new(),
                message: Some(format!("dependency {dependency} did not produce an output")),
            }
        } else {
            execute_staged_compute_step(
                state,
                auth,
                codec,
                stage,
                &entries,
                step,
                &outputs,
                max_rows.saturating_sub(rows_returned),
            )
            .await?
        };
        let step_rows = count_json_rows(&execution.output);
        if rows_returned.saturating_add(step_rows) > max_rows {
            truncate_json_rows(
                &mut execution.output,
                max_rows.saturating_sub(rows_returned),
            );
            execution.status = ItemStatus::Partial;
            execution.message = Some("output truncated at the 10,000-row compute limit".to_owned());
        }
        let serialized = serde_json::to_string(&execution.output)?;
        let step_tokens = estimate_tokens(&serialized);
        if estimated_tokens.saturating_add(step_tokens) > token_budget {
            execution.output = json!({
                "truncated": true,
                "reason": "compute token budget exhausted",
                "output_hash": prefixed_hash(sha256_hex(serialized.as_bytes()))
            });
            execution.status = ItemStatus::Partial;
            execution.message =
                Some("output replaced by a hash at the compute token budget".to_owned());
        } else {
            estimated_tokens += step_tokens;
        }
        rows_returned += count_json_rows(&execution.output);
        outputs.insert(step.id.clone(), execution.output.clone());
        steps.push(ComputeStepResult {
            id: step.id.clone(),
            operator: compute_operator_name(step.op).to_owned(),
            status: execution.status,
            output: execution.output,
            evidence_refs: dedup_strings(execution.evidence_refs),
            message: execution.message,
        });
    }
    let status = if steps.iter().all(|step| step.status == ItemStatus::Complete) {
        ResponseStatus::Complete
    } else {
        ResponseStatus::Partial
    };
    Ok(ComputeResult {
        session_id: stage.stage_ref.clone(),
        corpus_revision: stage.revision_ref(),
        status,
        steps,
        rows_returned,
        estimated_tokens,
        freshness: stage.freshness(),
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_staged_compute_step(
    state: &AppState,
    auth: &AuthContext,
    codec: &ContinuationCodec,
    stage: &StagedCorpus,
    entries: &[StagedEntry],
    step: &ComputeStep,
    outputs: &HashMap<String, Value>,
    remaining_rows: usize,
) -> ApiResult<StepExecution> {
    match step.op {
        ComputeOperator::Catalog => Ok(complete_step(staged_catalog(stage, entries))),
        ComputeOperator::Query | ComputeOperator::Search => {
            let mut spec: QuerySpec = serde_json::from_value(step.input.clone()).or_else(|_| {
                let query = step.input.as_str().map(str::to_owned).or_else(|| {
                    step.input
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
                serde_json::from_value(
                    json!({"id": step.id, "query": query, "limit": remaining_rows.clamp(1, 100)}),
                )
            })?;
            spec.limit = spec.limit.min(remaining_rows.max(1)).min(100);
            if step.op == ComputeOperator::Search && spec.modes.is_empty() {
                spec.modes = vec![
                    RetrievalMode::Exact,
                    RetrievalMode::Structured,
                    RetrievalMode::Lexical,
                ];
            }
            let result = run_staged_query_batch(state, auth, stage, vec![spec]).await?;
            let evidence_refs = result
                .items
                .iter()
                .flat_map(|item| item.results.iter())
                .flat_map(|candidate| candidate.evidence_refs.clone())
                .collect();
            Ok(StepExecution {
                status: if result.status == ResponseStatus::Complete {
                    ItemStatus::Complete
                } else {
                    ItemStatus::Partial
                },
                output: serde_json::to_value(result.items)?,
                evidence_refs,
                message: None,
            })
        }
        ComputeOperator::Read
        | ComputeOperator::Neighbors
        | ComputeOperator::History
        | ComputeOperator::Diff => {
            let mut spec: ReadSpec = serde_json::from_value(step.input.clone())?;
            spec.view = match step.op {
                ComputeOperator::Neighbors => ReadView::Neighbors,
                ComputeOperator::History => ReadView::History,
                ComputeOperator::Diff => ReadView::Diff,
                _ => spec.view,
            };
            let item = read_staged_item(state, codec, stage, entries, &spec).await?;
            let evidence_refs = item
                .data
                .as_ref()
                .map(collect_evidence_refs)
                .unwrap_or_default();
            Ok(StepExecution {
                status: item.status,
                output: item.data.unwrap_or(Value::Null),
                evidence_refs,
                message: item.error.map(|error| error.message),
            })
        }
        ComputeOperator::BatchRead => {
            let specs: Vec<ReadSpec> = serde_json::from_value(
                step.input
                    .get("requests")
                    .cloned()
                    .unwrap_or_else(|| step.input.clone()),
            )?;
            if specs.is_empty() || specs.len() > 64 {
                return Ok(unsupported_step(
                    "batchRead requires between 1 and 64 read specs",
                ));
            }
            let mut items = Vec::with_capacity(specs.len());
            for spec in &specs {
                match read_staged_item(state, codec, stage, entries, spec).await {
                    Ok(item) => items.push(item),
                    Err(error) => items.push(ReadItemResult {
                        reference: spec.reference.clone(),
                        view: read_view_name(spec.view).to_owned(),
                        status: ItemStatus::Failed,
                        data: None,
                        error: Some(item_error(error)),
                        truncation: Truncation::default(),
                    }),
                }
            }
            let output = serde_json::to_value(&items)?;
            Ok(StepExecution {
                status: if items.iter().all(|item| item.status == ItemStatus::Complete) {
                    ItemStatus::Complete
                } else {
                    ItemStatus::Partial
                },
                evidence_refs: collect_evidence_refs(&output),
                output,
                message: None,
            })
        }
        ComputeOperator::Group => group_step(&step.input, outputs, &step.depends_on),
        ComputeOperator::Aggregate => aggregate_step(&step.input, outputs, &step.depends_on),
        ComputeOperator::CompareApplicability => {
            compare_applicability_step(&step.input, outputs, &step.depends_on)
        }
        ComputeOperator::Proximity => proximity_step(&step.input),
        ComputeOperator::Timeline
        | ComputeOperator::Traverse
        | ComputeOperator::ResolveIdentity
        | ComputeOperator::ExpandRecurrence
        | ComputeOperator::StateHistory
        | ComputeOperator::Impact
        | ComputeOperator::GateRollup => Ok(unsupported_step(
            "this typed corpus operator is unavailable for temporary staged entries",
        )),
    }
}

async fn verify_staged_corpus(
    state: &AppState,
    auth: &AuthContext,
    stage: &StagedCorpus,
    request: &VerifyRequest,
) -> ApiResult<VerifyResult> {
    let entries = load_staged_entries(state, auth, stage).await?;
    let mut claims = Vec::with_capacity(request.claims.len());
    for (index, claim) in request.claims.iter().enumerate() {
        match verify_staged_claim(state, stage, &entries, claim, index).await {
            Ok(result) => claims.push(result),
            Err(error) => claims.push(VerifyItemResult {
                id: claim.id.clone().unwrap_or_else(|| format!("claim_{index}")),
                claim: claim.claim.clone(),
                classification: VerificationClassification::InsufficientEvidence,
                evidence: Vec::new(),
                structural_checks: vec![StructuralCheck {
                    check: "verify_item".to_owned(),
                    status: "failed".to_owned(),
                    details: json!({"error": error.to_string()}),
                }],
                explanation:
                    "The staged item could not be verified against immutable readable bytes."
                        .to_owned(),
            }),
        }
    }
    let status = if claims
        .iter()
        .all(|claim| claim.classification == VerificationClassification::Supported)
    {
        ResponseStatus::Complete
    } else {
        ResponseStatus::Partial
    };
    Ok(VerifyResult {
        session_id: stage.stage_ref.clone(),
        corpus_revision: stage.revision_ref(),
        status,
        claims,
        freshness: stage.freshness(),
        coverage: Coverage {
            searched: vec![CoveragePartition {
                partition: "staged_entries".to_owned(),
                lane: "evidence_verify".to_owned(),
                completeness: "best_effort".to_owned(),
                index_revision: Some(stage.revision_ref()),
                searched_count: request.claims.len(),
                candidate_count: request.claims.len(),
                failure_reason: None,
            }],
            unsearched: vec![CoveragePartition {
                partition: "structured_claim_graph".to_owned(),
                lane: "structured_verify".to_owned(),
                completeness: "unavailable_for_stage".to_owned(),
                index_revision: Some(stage.revision_ref()),
                searched_count: 0,
                candidate_count: 0,
                failure_reason: Some(
                    "temporary staged corpora have source evidence but no promoted claim graph"
                        .to_owned(),
                ),
            }],
            absence_safe: false,
        },
    })
}

async fn verify_staged_claim(
    state: &AppState,
    stage: &StagedCorpus,
    entries: &[StagedEntry],
    claim: &VerifyClaim,
    index: usize,
) -> ApiResult<VerifyItemResult> {
    if claim.claim.trim().is_empty() {
        return Err(ApiError::invalid("verify claim text is required"));
    }
    let mut candidates: Vec<&StagedEntry> = if claim.evidence_refs.is_empty() {
        let mut ranked: Vec<_> = entries
            .iter()
            .filter(|entry| entry.readability == "readable")
            .map(|entry| {
                let preview = entry
                    .inspection
                    .get("search_text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                (lexical_support_ratio(&claim.claim, preview), entry)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect();
        ranked.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.path.cmp(&right.1.path))
        });
        ranked.into_iter().take(8).map(|(_, entry)| entry).collect()
    } else {
        let mut selected = Vec::new();
        for reference in &claim.evidence_refs {
            let entry = if reference.starts_with("stage-entry:") {
                let id = parse_uuid_ref(reference, &["stage-entry"])?;
                entries.iter().find(|entry| entry.id == id)
            } else {
                entries.iter().find(|entry| entry.path == *reference)
            }
            .ok_or_else(|| ApiError::not_found("stage_entry_not_found", reference))?;
            selected.push(entry);
        }
        selected
    };
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by_key(|entry| entry.id);

    let mut evidence = Vec::new();
    let mut best_support = 0.0_f64;
    for entry in candidates.into_iter().take(8) {
        if entry.readability != "readable" {
            continue;
        }
        let text = load_staged_text(state, entry).await?;
        best_support = best_support.max(lexical_support_ratio(&claim.claim, &text));
        evidence.push(EvidencePassage {
            evidence_ref: record_ref("stage-entry", entry.id),
            source_ref: stage.stage_ref.clone(),
            source_version: entry.content_hash.clone().map(prefixed_hash),
            content_hash: prefixed_hash(entry.content_hash.clone().ok_or_else(|| {
                ApiError::conflict(
                    "content_hash_missing",
                    "readable staged evidence has no immutable hash",
                    json!({"path": entry.path}),
                )
            })?),
            locator: json!({
                "stage_ref": stage.stage_ref,
                "path": entry.path,
                "entry_kind": entry.entry_kind
            }),
            text: Some(staged_query_snippet(
                &text,
                &claim.claim,
                DEFAULT_READ_CHARS,
            )),
            support_kind: "staged_source".to_owned(),
            recorded_at: entry.created_at,
        });
    }
    let lexical_support = best_support >= 0.55;
    let temporal_ambiguity = temporal_ambiguity_from_facts(0, false, claim, best_support >= 0.9);
    let absence_claim = looks_like_absence_claim(&claim.claim);
    let classification = if temporal_ambiguity {
        VerificationClassification::TemporallyAmbiguous
    } else if absence_claim {
        VerificationClassification::InsufficientEvidence
    } else if lexical_support {
        VerificationClassification::Supported
    } else {
        VerificationClassification::InsufficientEvidence
    };
    Ok(VerifyItemResult {
        id: claim.id.clone().unwrap_or_else(|| format!("claim_{index}")),
        claim: claim.claim.clone(),
        classification,
        evidence,
        structural_checks: vec![
            StructuralCheck {
                check: "immutable_stage_hash".to_owned(),
                status: "passed".to_owned(),
                details: json!({
                    "stage_revision": stage.revision_ref(),
                    "bytes_rehashed": true
                }),
            },
            StructuralCheck {
                check: "promoted_claim_graph".to_owned(),
                status: "not_applicable".to_owned(),
                details: json!({
                    "message": "staged evidence remains temporary until memory.save promotion"
                }),
            },
        ],
        explanation: match classification {
            VerificationClassification::Supported => {
                "An immutable, hash-checked staged source passage supports the claim."
            }
            VerificationClassification::TemporallyAmbiguous => {
                "The staged passage does not safely qualify relative or incomplete temporal language."
            }
            _ => "No hash-checked staged source passage establishes this claim.",
        }
        .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_spec() -> ReadSpec {
        ReadSpec {
            reference: "document:018f0f3d8c2d7a2bb5077d5f6e5e0001".to_owned(),
            view: ReadView::Range,
            version: Some("v2".to_owned()),
            anchor: None,
            before: None,
            after: None,
            start: Some(2),
            end: Some(4),
            from_version: None,
            to_version: None,
            max_chars: Some(8),
            continuation_token: None,
            audience: Some("principal:self".to_owned()),
            purpose: Some("task_reasoning".to_owned()),
        }
    }

    #[test]
    fn reference_parser_accepts_prefixed_and_compact_uuids() {
        let id = Uuid::parse_str("018f0f3d-8c2d-7a2b-b507-7d5f6e5e0001").unwrap();
        assert_eq!(
            parse_uuid_ref(&format!("object:{}", id.simple()), &["object"]).unwrap(),
            id
        );
        assert_eq!(
            parse_uuid_ref(&format!("ms_{}", id.simple()), &["ms"]).unwrap(),
            id
        );
        assert!(parse_uuid_ref("object:not-a-uuid", &["object"]).is_err());
    }

    #[test]
    fn continuation_skips_initial_retrieval_only_for_complete_inline_delta() {
        let checkpoint = json!({"checkpoint_ref": "checkpoint:parent"});
        let complete = json!({"self_contained": true, "source_changes": [{"content": "delta"}]});
        let incomplete = json!({"self_contained": false, "incomplete_reasons": ["bounded"]});
        assert!(continuation_is_self_contained(
            Some(&checkpoint),
            Some(&complete)
        ));
        assert!(!continuation_is_self_contained(
            Some(&checkpoint),
            Some(&incomplete)
        ));
        assert!(!continuation_is_self_contained(None, Some(&complete)));
    }

    #[test]
    fn utf8_slices_end_on_exact_character_boundaries() {
        let text = "one élan 雪 end";
        let start = byte_at_char(text, 4).unwrap();
        let end = bounded_char_end(text, start, text.len(), 6);
        assert!(text.is_char_boundary(end));
        assert_eq!(&text[start..end], "élan 雪");
        assert_eq!(line_at_byte("a\nb\nc", 4), 3);
    }

    #[test]
    fn continuation_request_hash_ignores_the_token_itself() {
        let first = read_spec();
        let mut second = read_spec();
        second.continuation_token = Some("opaque".to_owned());
        assert_eq!(
            read_request_hash(&first).unwrap(),
            read_request_hash(&second).unwrap()
        );
    }

    #[test]
    fn compute_dag_rejects_forward_dependencies() {
        let steps = vec![ComputeStep {
            id: "later".to_owned(),
            op: ComputeOperator::Aggregate,
            input: json!({}),
            depends_on: vec!["missing".to_owned()],
        }];
        assert!(validate_compute_dag(&steps).is_err());
    }

    #[test]
    fn proximity_requires_compatible_frames() {
        let unsupported = proximity_step(&json!({
            "left": {"frame": "frame:a", "unit": "m", "coordinates": [0.0, 0.0]},
            "right": {"frame": "frame:b", "unit": "m", "coordinates": [3.0, 4.0]}
        }))
        .unwrap();
        assert_eq!(unsupported.status, ItemStatus::Unsupported);

        let complete = proximity_step(&json!({
            "left": {"frame": "frame:a", "unit": "m", "coordinates": [0.0, 0.0]},
            "right": {"frame": "frame:a", "unit": "m", "coordinates": [3.0, 4.0]}
        }))
        .unwrap();
        assert_eq!(complete.status, ItemStatus::Complete);
        assert_eq!(complete.output["distance"], json!(5.0));
    }

    #[test]
    fn absence_claims_are_never_treated_as_safe_by_lexical_silence() {
        assert!(looks_like_absence_claim("No lodging has been booked"));
        assert!(!looks_like_absence_claim("The lodging is booked"));
        assert!(lexical_support_ratio("lodging booked", "The lodging was booked yesterday") > 0.9);
    }

    fn verify_claim(text: &str) -> VerifyClaim {
        VerifyClaim {
            id: None,
            claim: text.to_owned(),
            evidence_refs: Vec::new(),
            about_ref: None,
            predicate: None,
            value: None,
            coverage_ref: None,
        }
    }

    #[test]
    fn stage_refs_pin_optional_inventory_hashes() {
        let id = Uuid::parse_str("018f0f3d-8c2d-7a2b-b507-7d5f6e5e0001").unwrap();
        let (parsed, revision) =
            parse_stage_ref(&format!("stage:{}@sha256:abc123", id.simple())).unwrap();
        assert_eq!(parsed, id);
        assert_eq!(revision.as_deref(), Some("sha256:abc123"));
        assert!(parse_stage_ref("session:018f0f3d8c2d7a2bb5077d5f6e5e0001").is_err());
    }

    #[test]
    fn verify_accepts_returned_chunk_refs_as_evidence_handles() {
        let id = Uuid::parse_str("018f0f3d-8c2d-7a2b-b507-7d5f6e5e0001").unwrap();
        assert_eq!(
            parse_verify_evidence_reference(&format!("chunk:{}", id.simple())).unwrap(),
            VerifyEvidenceReference::Chunk(id)
        );
        assert!(parse_verify_evidence_reference(&format!("object:{}", id.simple())).is_err());
    }

    #[test]
    fn exact_absolute_time_passage_can_qualify_temporal_claim() {
        let claim = verify_claim("The Alder transfer departs Zurich at 09:40 on February 14");
        assert!(!temporal_ambiguity_from_facts(0, false, &claim, true));

        let relative = verify_claim("The Alder transfer departs Zurich tomorrow at 09:40");
        assert!(temporal_ambiguity_from_facts(0, false, &relative, true));
        assert!(temporal_ambiguity_from_facts(2, true, &claim, true));
    }

    #[test]
    fn text_only_verify_cannot_match_unrelated_structured_claims() {
        let claim = verify_claim("An unrelated arbitrary statement");
        assert!(!verify_claim_has_structured_selector(&claim, None));

        let mut constrained = claim.clone();
        constrained.predicate = Some("travel.departure_time".to_owned());
        assert!(verify_claim_has_structured_selector(&constrained, None));

        let mut unresolved = claim;
        unresolved.about_ref = Some("object:missing".to_owned());
        assert!(!verify_claim_has_structured_selector(&unresolved, None));
    }

    #[test]
    fn retrieval_case_scores_are_explicit_float8() {
        let source = include_str!("read_service.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("END AS score"));
        assert!(production.matches("END)::float8 AS score").count() >= 4);
    }

    #[test]
    fn lexical_discovery_uses_or_recall_and_expands_currentness() {
        let query = lexical_discovery_query(
            "What is the latest recorded target in the final parser validation handoff?",
        );
        assert!(query.contains("\"latest\" OR \"newest\" OR \"current\""));
        assert!(query.contains(" OR \"target\""));
        assert!(query.contains(" OR \"parser\""));
        assert!(!query.contains("\"the\""));
    }

    #[test]
    fn lexical_discovery_splits_compound_identifiers() {
        let terms = lexical_discovery_terms("StarRupture RuptureOps PGCRArchive");
        assert!(terms.contains(&"star".to_owned()));
        assert!(terms.contains(&"rupture".to_owned()));
        assert!(terms.contains(&"ops".to_owned()));
        assert!(terms.contains(&"pgcr".to_owned()));
        assert!(terms.contains(&"archive".to_owned()));
        assert!(!terms.contains(&"starrupture".to_owned()));
    }

    #[test]
    fn exact_substring_patterns_keep_like_metacharacters_literal() {
        assert_eq!(
            sql_like_literal_contains(r"50%_complete\path"),
            r"%50\%\_complete\\path%"
        );
    }

    fn retrieval_candidate(path: &str, content: &str) -> RetrievalCandidate {
        RetrievalCandidate {
            reference: "chunk:018f0f3d8c2d7a2bb5077d5f6e5e0001".to_owned(),
            source_ref: path.to_owned(),
            path: Some(path.to_owned()),
            heading: Some("Current schedule".to_owned()),
            content: content.to_owned(),
            content_hash: "sha256:chunk".to_owned(),
            source_version: Some("source-v1".to_owned()),
            authority: Some("owner_confirmed".to_owned()),
            canonicality: Some("canonical_at_revision".to_owned()),
            provenance: None,
            recorded_at: Some("2026-07-22T00:00:00Z".to_owned()),
            valid_time: Some(json!({"path": path, "start_line": 1, "end_line": 3})),
            evidence_refs: Vec::new(),
            why_selected: vec!["exact_phrase_match".to_owned()],
            lane_scores: HashMap::new(),
            score: 1.0,
        }
    }

    #[test]
    fn complete_open_hydration_preserves_source_identity_and_exact_text() {
        let candidate = retrieval_candidate(
            "Schedule/current.md",
            "Practice starts at 09:00 America/Los_Angeles.",
        );
        let loaded = LoadedText {
            metadata: json!({
                "title": "Current schedule",
                "content_hash": "sha256:source",
                "source_ref": "Schedule/current.md",
                "source_version": "source-v2"
            }),
            text: "# Current schedule\n\nPractice starts at 09:00 America/Los_Angeles.\n"
                .to_owned(),
        };
        let hydrated = complete_hydrated_source("Schedule/current.md", &[&candidate], loaded);
        assert!(hydrated.complete);
        assert_eq!(hydrated.source_version.as_deref(), Some("source-v2"));
        assert_eq!(hydrated.content_hash.as_deref(), Some("sha256:source"));
        assert!(hydrated.text.contains("America/Los_Angeles"));
        assert!(
            hydrated
                .why_selected
                .contains(&"complete_source_hydration".to_owned())
        );
    }

    #[test]
    fn native_assets_must_match_the_committed_content_hash() {
        let bytes = b"durable source evidence";
        let expected = hex::encode(Sha256::digest(bytes));
        assert!(native_asset_hash_matches(Some(&expected), bytes));
        assert!(!native_asset_hash_matches(None, bytes));
        assert!(!native_asset_hash_matches(
            Some(&expected),
            b"altered evidence"
        ));
    }

    #[test]
    fn open_revision_matching_accepts_hyphenated_and_compact_refs() {
        let revision_id = Uuid::now_v7();
        assert!(open_revision_matches(
            &format!("revision:{revision_id}"),
            revision_id,
            7
        ));
        assert!(open_revision_matches(
            &format!("revision:{}", revision_id.simple()),
            revision_id,
            7
        ));
        assert!(open_revision_matches("latest", revision_id, 7));
        assert!(open_revision_matches("7", revision_id, 7));
        assert!(!open_revision_matches("8", revision_id, 7));
    }

    #[test]
    fn retrieval_sufficiency_is_deterministic_and_reports_missing_anchors() {
        let candidate = retrieval_candidate(
            "Schedule/current.md",
            "Practice starts at 09:00 America/Los_Angeles.",
        );
        let source = section_hydrated_source("Schedule/current.md", &[&candidate]);
        let sufficient = open_retrieval_sufficiency(
            "State the current practice schedule and timezone",
            std::slice::from_ref(&candidate),
            &[HydratedSource {
                complete: true,
                ..source.clone()
            }],
        );
        assert_eq!(sufficient["status"], "likely_sufficient");

        let complete_secondary = HydratedSource {
            path: Some("Schedule/archive.md".to_owned()),
            complete: true,
            ..source.clone()
        };
        let partial_primary = open_retrieval_sufficiency(
            "State the current practice schedule and timezone",
            std::slice::from_ref(&candidate),
            &[source.clone(), complete_secondary],
        );
        assert_eq!(
            partial_primary["status"], "inspect_then_query_gaps",
            "a complete secondary source must not mask a partial primary source"
        );

        let incomplete = open_retrieval_sufficiency(
            "State the current practice schedule and missing delegation",
            &[candidate],
            &[source],
        );
        assert_eq!(incomplete["status"], "inspect_then_query_gaps");
        assert!(
            incomplete["unresolved_anchors"]
                .as_array()
                .is_some_and(|items| items.contains(&json!("delegation")))
        );
    }

    #[test]
    fn table_superlatives_request_source_order_tie_breaking() {
        assert!(source_start_intent(
            "Show the three largest rows from the top of the table"
        ));
        assert!(!source_start_intent("Explain the current storage state"));
    }

    #[test]
    fn currentness_intent_is_explicit_and_does_not_trigger_on_generic_state() {
        assert!(currentness_intent("What is the latest recorded target?"));
        assert!(currentness_intent("Show the superseding handoff"));
        assert!(currentness_intent("Is this up-to-date?"));
        assert!(!currentness_intent("Which tables receive active writes?"));
        assert!(!currentness_intent("Explain the validation state"));
    }

    #[test]
    fn semantic_lane_limits_vector_ids_before_representation_joins() {
        let source = include_str!("read_service.rs");
        let semantic = source
            .split_once("async fn semantic_lane_tx")
            .unwrap()
            .1
            .split_once("async fn structured_lane_tx")
            .unwrap()
            .0;
        let ranked_limit = semantic.find("LIMIT $6").unwrap();
        let representation_join = semantic.find("FROM ranked e").unwrap();
        assert!(ranked_limit < representation_join);
        assert!(semantic.contains("WITH ranked AS MATERIALIZED"));
    }

    #[test]
    fn materialization_is_complete_only_when_every_active_family_is_returned() {
        let available = BTreeMap::from([
            ("document".to_owned(), 2),
            ("chunk".to_owned(), 4),
            ("object".to_owned(), 1),
            ("claim".to_owned(), 3),
            ("relation".to_owned(), 1),
            ("checkpoint".to_owned(), 1),
            ("source_episode".to_owned(), 2),
        ]);
        assert!(materialization_omissions(&available, &available).is_empty());

        let mut returned = available.clone();
        returned.insert("claim".to_owned(), 2);
        returned.remove("checkpoint");
        let omitted = materialization_omissions(&available, &returned);
        assert_eq!(omitted.get("claim"), Some(&1));
        assert_eq!(omitted.get("checkpoint"), Some(&1));
    }

    #[test]
    fn structured_scope_roots_include_query_scope_and_scope_root_filter() {
        let mut filters = Map::new();
        filters.insert("scope_root".to_owned(), json!("object:filter-root"));
        let spec = QuerySpec {
            id: None,
            goal: None,
            query: None,
            scope: Some(QueryScope::Structured {
                authorization_scope: Some("scope:test".to_owned()),
                root_refs: vec!["object:scope-root".to_owned()],
            }),
            modes: vec![RetrievalMode::Structured],
            r#where: filters,
            state_filter: None,
            expand: None,
            limit: 12,
        };
        assert_eq!(
            query_root_references(&spec),
            vec![
                "object:filter-root".to_owned(),
                "object:scope-root".to_owned()
            ]
        );
    }

    #[test]
    fn latest_state_head_is_selected_before_requested_values_are_filtered() {
        let (head_selection, requested_filters) = STATE_LANE_SQL
            .split_once("), filtered_states AS (")
            .expect("state query has distinct head and filter phases");
        assert!(head_selection.contains("DISTINCT ON"));
        assert!(!head_selection.contains("state_value = ANY"));
        assert!(requested_filters.contains("head.state_value = ANY"));
        assert!(requested_filters.contains("head.authority = $6"));
        assert!(requested_filters.contains("head.canonicality = $7"));
    }
}

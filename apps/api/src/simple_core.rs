use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Extension, Json,
    body::Body,
    extract::{Multipart, Path, Query, State, multipart::Field},
    http::{HeaderValue, Response, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use hmac::{Hmac, Mac};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder, Row, Transaction, postgres::PgRow};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};
use tokio_util::io::ReaderStream;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::{
    auth::{AuthContext, hash_token},
    db::{AppState, set_context},
    error::{ApiError, ApiResult},
    eval_service::EvalImportRequest,
    ingest::{DocumentChunk, normalize_document},
    models::{Capability, CheckpointRequest, CredentialId, ResponseStatus, UserId},
    usage::UsageOperation,
    workspace_features::{
        DerivedFrontmatter, SupersessionAnnotation, WorkspaceFeatureDocument,
        WorkspaceFeatureSnapshot, parse_frontmatter, supersession_warnings,
    },
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceEnvelope<T> {
    #[serde(skip_serializing)]
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corpus_revision: Option<String>,
    pub status: ResponseStatus,
    pub data: T,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings_ms: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_count: Option<usize>,
}

impl<T> WorkspaceEnvelope<T> {
    fn complete(data: T) -> Self {
        Self {
            request_id: crate::request_context::current_request_id(),
            session_id: None,
            corpus_revision: None,
            status: ResponseStatus::Complete,
            data,
            gaps: Vec::new(),
            timings_ms: None,
            query_count: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RetrievalTimings {
    exact: f64,
    lexical: f64,
    semantic_ready: f64,
    semantic: f64,
    embed: f64,
    semantic_db: f64,
    merge: f64,
    total: f64,
}

impl RetrievalTimings {
    fn as_value(&self) -> Value {
        json!({
            "exact": round_ms(self.exact),
            "lexical": round_ms(self.lexical),
            "semantic_ready": round_ms(self.semantic_ready),
            "semantic": round_ms(self.semantic),
            "embed": round_ms(self.embed),
            "semantic_db": round_ms(self.semantic_db),
            "merge": round_ms(self.merge),
            "total": round_ms(self.total),
        })
    }
}

#[derive(Clone, Debug)]
struct SemanticCandidates {
    candidates: Vec<Candidate>,
    embed_ms: f64,
    database_ms: f64,
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn round_ms(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

const DEFAULT_TOKEN_BUDGET: usize = 12_000;
const MAX_TOKEN_BUDGET: usize = 64_000;
const MAX_WRITE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EXACT_READ_CHARS: usize = 4 * 1024 * 1024;
const MAX_READ_RESPONSE_CHARS: usize = MAX_EXACT_READ_CHARS;
const MAX_SEARCH_LIMIT: usize = 50;
const MAX_SEARCH_RESPONSE_CANDIDATES: usize = 128;
const MAX_SEARCH_RESPONSE_CHARS: usize = 96_000;
const MAX_VERBATIM_MATCHES_PER_CANDIDATE: usize = 3;
const MAX_VERBATIM_LINE_CHARS: usize = 2_400;
const MAX_VERBATIM_RESPONSE_CHARS: usize = 9_600;
const OPEN_CANDIDATE_LIMIT: usize = 32;
const HYDRATED_DOCUMENT_LIMIT: usize = 8;
const MAX_OPEN_COMPLETE_SOURCE_CHARS: usize = 24_000;
const MAX_STREAMED_BINARY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const RETRIEVAL_LANE_TIMEOUT: Duration = Duration::from_millis(2_500);
const CHUNK_INSERT_BATCH_SIZE: usize = 256;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OpenHints {
    pub authorization_scope: Option<String>,
    #[serde(default)]
    pub root_refs: Vec<String>,
    #[serde(default)]
    pub open_object_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OpenRequest {
    pub task: String,
    #[serde(default)]
    pub hints: OpenHints,
    pub resume_checkpoint_ref: Option<String>,
    pub token_budget: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchQuery {
    pub id: Option<String>,
    pub query: String,
    pub goal: Option<String>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub modes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchRequest {
    pub session_id: Option<String>,
    #[serde(default)]
    pub queries: Vec<SearchQuery>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub token_budget: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadItem {
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub path: Option<String>,
    pub view: Option<String>,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub max_chars: Option<usize>,
    pub version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadRequest {
    pub session_id: Option<String>,
    pub requests: Vec<ReadItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WriteRequest {
    pub path: String,
    pub content: String,
    #[serde(default = "markdown_media_type")]
    pub media_type: String,
    pub expected_version: Option<i64>,
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CaptureRequest {
    pub content: String,
    #[serde(default)]
    pub source: Value,
    pub intent: Option<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChangesQuery {
    #[serde(default)]
    pub since_generation: i64,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BinaryListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManifestQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub after_path: Option<String>,
    pub after_entry_ref: Option<String>,
    pub after_version: Option<i64>,
    #[serde(default)]
    pub history: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UsageQuery {
    pub sort: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BinaryVersionQuery {
    pub version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StreamingBinaryQuery {
    pub path: String,
    pub media_type: Option<String>,
    pub expected_content_hash: String,
    pub mtime_ns: Option<i64>,
    pub mode: Option<u32>,
    pub provenance: Option<String>,
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeleteQuery {
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DreamRequest {
    pub since_generation: Option<i64>,
    pub focus: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JobsQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
struct CandidateSection {
    heading: String,
    excerpt: String,
    #[serde(skip)]
    score: f64,
}

#[derive(Clone, Debug, Serialize)]
struct VerbatimMatch {
    line_no: usize,
    byte_start: usize,
    byte_end: usize,
    text: String,
    version: i64,
    content_hash: String,
    #[serde(skip_serializing_if = "is_false")]
    truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
struct Candidate {
    entry_id: Uuid,
    path: String,
    title: String,
    version: i64,
    content_sha256: String,
    heading: String,
    excerpt: String,
    score: f64,
    lanes: Vec<String>,
    sections: Vec<CandidateSection>,
    verbatim_matches: Vec<VerbatimMatch>,
    superseded_by: Option<SupersessionAnnotation>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchRepresentation {
    CompleteSource,
    Excerpt,
    PointerLead,
}

#[derive(Clone, Debug)]
struct SearchCandidateView {
    candidate: Candidate,
    representation: SearchRepresentation,
    complete_text: Option<String>,
    demote_additional_sections: bool,
}

#[derive(Clone, Debug)]
struct SearchHydration {
    size_bytes: usize,
    content: String,
}

#[derive(Clone, Copy, Debug)]
struct SearchBudgetOptions {
    fair_share: bool,
    top1_hydration: bool,
    char_cap: bool,
    section_demotion_top_n: Option<usize>,
    max_chars: usize,
}

impl SearchBudgetOptions {
    fn from_request(config: &crate::config::Config, token_budget: Option<usize>) -> Self {
        let char_cap = config.search_char_cap;
        let max_chars = if char_cap {
            token_budget
                .unwrap_or(DEFAULT_TOKEN_BUDGET)
                .clamp(1_000, MAX_TOKEN_BUDGET)
                .saturating_mul(4)
                .min(MAX_SEARCH_RESPONSE_CHARS)
        } else {
            MAX_SEARCH_RESPONSE_CHARS
        };
        Self {
            fair_share: config.search_fair_share,
            top1_hydration: config.search_top1_hydration,
            char_cap,
            section_demotion_top_n: char_cap
                .then_some(config.search_section_demotion_top_n)
                .flatten(),
            max_chars,
        }
    }

    fn active(self) -> bool {
        self.fair_share || self.top1_hydration || self.char_cap
    }
}

#[derive(Clone, Debug)]
struct EntryRow {
    id: Uuid,
    path: String,
    title: String,
    kind: String,
    media_type: String,
    version: i64,
    version_id: Uuid,
    content_sha256: String,
    content: Option<String>,
    object_key: Option<String>,
    object_version_id: Option<String>,
    size_bytes: i64,
    metadata: Value,
    updated_at: DateTime<Utc>,
}

struct ChangePage {
    changes: Vec<Value>,
    truncated: bool,
    next_generation: Option<i64>,
}

#[derive(Clone, Debug)]
struct PreparedMarkdown {
    entry_id_hint: Option<Uuid>,
    path: String,
    title: String,
    content: String,
    content_sha256: String,
    media_type: String,
    metadata: Value,
    chunks: Vec<DocumentChunk>,
    embeddings: Vec<Option<Vector>>,
    expected_version: Option<i64>,
    frontmatter: DerivedFrontmatter,
}

#[derive(Clone, Debug)]
struct MarkdownUpsertResult {
    entry_id: Uuid,
    version: i64,
    version_id: Option<Uuid>,
    generation: Option<i64>,
    no_op: bool,
    metadata_only: bool,
}

#[derive(Clone, Debug)]
struct BulkMarkdown {
    entry_id: Uuid,
    version_id: Uuid,
    path: String,
    title: String,
    content: String,
    content_sha256: String,
    media_type: String,
    metadata: Value,
    chunks: Vec<DocumentChunk>,
    embeddings: Vec<Option<Vector>>,
    frontmatter: DerivedFrontmatter,
}

fn markdown_media_type() -> String {
    "text/markdown".to_owned()
}

pub async fn open(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<OpenRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    let total_started = Instant::now();
    auth.require(Capability::Open)?;
    if request.task.trim().is_empty() {
        return Err(ApiError::invalid("task is required"));
    }
    let budget = request
        .token_budget
        .unwrap_or(DEFAULT_TOKEN_BUDGET)
        .clamp(1_000, MAX_TOKEN_BUDGET);
    let session_id = format!("session:{}", Uuid::now_v7());
    let generation_started = Instant::now();
    let generation = current_generation(&state, &auth).await?;
    let generation_ms = elapsed_ms(generation_started);
    let features_started = Instant::now();
    let features = feature_snapshot(&state, &auth, generation).await?;
    let features_ms = elapsed_ms(features_started);
    let checkpoint_started = Instant::now();
    let mut checkpoint = match request.resume_checkpoint_ref.as_deref() {
        Some(reference) => Some(read_checkpoint(&state, &auth, reference).await?),
        None => None,
    };
    let checkpoint_read_ms = elapsed_ms(checkpoint_started);
    let checkpoint_generation = checkpoint
        .as_ref()
        .and_then(|value| value.get("workspace_generation"))
        .and_then(Value::as_i64);
    let changes_started = Instant::now();
    let change_page = match checkpoint_generation {
        Some(since) => changes_since(&state, &auth, since, 200).await?,
        None => ChangePage {
            changes: vec![],
            truncated: false,
            next_generation: None,
        },
    };
    let changes_ms = elapsed_ms(changes_started);

    let (checkpoint_text_truncated, evidence_budget) =
        apply_checkpoint_budget(&mut checkpoint, budget);
    let retrieval_started = Instant::now();
    let (search, hinted) = tokio::join!(
        search_one(
            &state,
            &auth,
            &request.task,
            OPEN_CANDIDATE_LIMIT,
            &[],
            features.as_deref(),
        ),
        open_hint_candidates(&state, &auth, &request.hints)
    );
    let retrieval_wall_ms = elapsed_ms(retrieval_started);
    let (search_candidates, mut lane_failures, search_timings) = match search {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(?error, "simple workspace initial search failed");
            (vec![], vec!["search"], RetrievalTimings::default())
        }
    };
    let (hint_candidates, hint_gaps) = hinted?;
    let (continuation_paths, changed_path_keys) =
        continuation_paths(checkpoint.as_ref(), &change_page.changes);
    let continuation_candidates = if continuation_paths.is_empty() {
        vec![]
    } else {
        match bounded_retrieval_lane(
            "continuation_exact",
            exact_candidates(&state, &auth, &continuation_paths, None),
        )
        .await
        {
            Ok(mut candidates) => {
                for candidate in &mut candidates {
                    candidate.score =
                        if changed_path_keys.contains(&portable_path_key(&candidate.path)) {
                            30.0
                        } else {
                            20.0
                        };
                    candidate.lanes.push("continuation".to_owned());
                }
                candidates
            }
            Err(error) => {
                tracing::warn!(?error, "simple continuation source lookup failed");
                lane_failures.push("continuation_exact");
                vec![]
            }
        }
    };
    let mut merged = HashMap::new();
    for candidate in continuation_candidates
        .into_iter()
        .chain(hint_candidates)
        .chain(search_candidates)
    {
        merge_candidate(&mut merged, candidate);
    }
    let mut candidates = merged.into_values().collect::<Vec<_>>();
    annotate_candidates(
        &mut candidates,
        features.as_deref(),
        state.config.supersession_demotion,
    );
    sort_candidates(&mut candidates);
    candidates.truncate(OPEN_CANDIDATE_LIMIT);
    let continuation_evidence = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .lanes
                .iter()
                .any(|lane| lane == "continuation" || lane == "hint")
        })
        .cloned()
        .collect::<Vec<_>>();
    let evidence_candidates = if continuation_evidence.is_empty() {
        &candidates
    } else {
        &continuation_evidence
    };
    let hydrate_started = Instant::now();
    let evidence = hydrate_candidates(&state, &auth, evidence_candidates, evidence_budget).await?;
    let hydrate_ms = elapsed_ms(hydrate_started);
    let hydrated_refs = evidence
        .iter()
        .filter_map(|item| item.get("reference").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    let evidence_leads = candidates
        .iter()
        .filter(|candidate| {
            !hydrated_refs.contains(format!("entry:{}", candidate.entry_id).as_str())
        })
        .map(render_evidence_lead)
        .collect::<Vec<_>>();
    record_candidate_usage(&state, &auth, &candidates);

    let mut response_data = json!({
        "workspace_generation": generation,
        "authorization_scope": request.hints.authorization_scope,
        "evidence": evidence,
        "evidence_leads": evidence_leads,
        "checkpoint": checkpoint,
        "changes_since_checkpoint": change_page.changes,
        "changes_truncated": change_page.truncated,
        "next_changes_generation": change_page.next_generation,
        "checkpoint_text_truncated": checkpoint_text_truncated,
        "retrieval_sufficiency": {
            "status": if candidates.is_empty() { "no_evidence" } else { "bounded_evidence" },
            "complete_source_count": evidence.iter()
                .filter(|item| item.get("representation").and_then(Value::as_str) == Some("complete_source"))
                .count(),
            "selected_source_count": evidence.len(),
            "pointer_source_count": evidence_leads.len()
        }
    });
    if state.config.intention_ledger {
        response_data["pending_intentions"] = json!(
            features
                .as_deref()
                .map(|snapshot| snapshot.pending_intentions(&request.task))
                .unwrap_or_default()
        );
    }
    let mut envelope = WorkspaceEnvelope::complete(response_data);
    envelope.session_id = Some(session_id);
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if checkpoint_text_truncated {
        envelope.gaps.push(json!({
            "kind": "checkpoint_text_budget",
            "message": "checkpoint text was truncated to the requested open budget"
        }));
    }
    envelope.gaps.extend(hint_gaps);
    if !lane_failures.is_empty() {
        envelope.status = if candidates.is_empty() {
            ResponseStatus::Degraded
        } else {
            ResponseStatus::Partial
        };
        envelope.gaps.extend(lane_failures.into_iter().map(|lane| {
            if lane == "semantic_unavailable" {
                json!({
                    "kind": "retrieval_lane_unavailable",
                    "lane": "semantic",
                    "message": "no indexed semantic evidence exists for this user yet; exact and lexical evidence was retained"
                })
            } else {
                json!({
                    "kind": "retrieval_lane_failed",
                    "lane": lane,
                    "message": "this lane failed; evidence from other lanes was retained"
                })
            }
        }));
    } else if !envelope.gaps.is_empty() {
        envelope.status = ResponseStatus::Partial;
    }
    let total_ms = elapsed_ms(total_started);
    if state.config.observability_timings_ms {
        let attributed_ms = generation_ms
            + features_ms
            + checkpoint_read_ms
            + changes_ms
            + retrieval_wall_ms
            + hydrate_ms;
        envelope.timings_ms = Some(json!({
            "generation": round_ms(generation_ms),
            "features": round_ms(features_ms),
            "checkpoint_read": round_ms(checkpoint_read_ms),
            "changes": round_ms(changes_ms),
            "retrieval_wall": round_ms(retrieval_wall_ms),
            "lanes": search_timings.as_value(),
            "hydrate": round_ms(hydrate_ms),
            "unattributed": round_ms((total_ms - attributed_ms).max(0.0)),
            "total": round_ms(total_ms),
        }));
    }
    metrics::histogram!("simple.open.duration_ms").record(total_ms);
    metrics::histogram!("simple.open.evidence_sources").record(evidence.len() as f64);
    metrics::histogram!("simple.open.evidence_leads").record(evidence_leads.len() as f64);
    Ok(Json(envelope))
}

pub async fn search(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<SearchRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Query)?;
    let started = Instant::now();
    let budget_options = SearchBudgetOptions::from_request(&state.config, request.token_budget);
    let generation_started = Instant::now();
    let generation = current_generation(&state, &auth).await?;
    let generation_ms = elapsed_ms(generation_started);
    let features_started = Instant::now();
    let features = feature_snapshot(&state, &auth, generation).await?;
    let features_ms = elapsed_ms(features_started);
    let queries = if request.queries.is_empty() {
        vec![SearchQuery {
            id: Some("q0".to_owned()),
            query: request
                .query
                .ok_or_else(|| ApiError::invalid("query is required"))?,
            goal: None,
            limit: request.limit,
            modes: vec![],
        }]
    } else {
        request.queries
    };
    if queries.len() > 16 {
        return Err(ApiError::invalid("at most 16 queries may be batched"));
    }
    let query_execution_started = Instant::now();
    let mut completed =
        futures::stream::iter(queries.into_iter().enumerate().map(|(index, query)| {
            let state = state.clone();
            let auth = auth.clone();
            let features = features.clone();
            async move {
                let limit = query.limit.unwrap_or(8).clamp(1, MAX_SEARCH_LIMIT);
                let result = search_one(
                    &state,
                    &auth,
                    &query.query,
                    limit,
                    &query.modes,
                    features.as_deref(),
                )
                .await?;
                Ok::<_, ApiError>((index, query, result))
            }
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<ApiResult<Vec<_>>>()?;
    let query_execution_ms = elapsed_ms(query_execution_started);
    completed.sort_by_key(|(index, _, _)| *index);

    let budget_started = Instant::now();
    let (budgeted_views, budgeted_truncated) = if budget_options.active() {
        let candidate_sets = completed
            .iter()
            .map(|(_, _, (candidates, _, _))| candidates.clone())
            .collect::<Vec<_>>();
        let (selected, selection_truncated) = select_search_candidate_sets(
            &candidate_sets,
            budget_options.fair_share,
            MAX_SEARCH_RESPONSE_CANDIDATES,
        );
        let hydration = if budget_options.top1_hydration {
            fetch_search_top1_hydration(&state, &auth, &selected).await?
        } else {
            HashMap::new()
        };
        let (views, evidence_truncated) =
            assemble_search_candidate_views(selected, &hydration, budget_options);
        (Some(views), selection_truncated || evidence_truncated)
    } else {
        (None, false)
    };
    let mut result_sets = Vec::with_capacity(completed.len());
    let mut query_timings = Vec::with_capacity(completed.len());
    let mut any_candidates = false;
    let mut any_failures = false;
    let mut all_candidates = Vec::new();
    let mut remaining_candidates = MAX_SEARCH_RESPONSE_CANDIDATES;
    let mut remaining_excerpt_chars = MAX_SEARCH_RESPONSE_CHARS;
    let mut remaining_verbatim_chars = MAX_VERBATIM_RESPONSE_CHARS;
    let mut response_truncated = budgeted_truncated;
    for (position, (index, query, (mut candidates, failures, timings))) in
        completed.into_iter().enumerate()
    {
        query_timings.push(json!({
            "id": query.id.clone().unwrap_or_else(|| format!("q{index}")),
            "phases": timings.as_value(),
        }));
        let rendered = if let Some(views) = &budgeted_views {
            let query_views = &views[position];
            any_candidates |= !query_views.is_empty();
            all_candidates.extend(query_views.iter().map(|view| view.candidate.clone()));
            query_views
                .iter()
                .map(|view| render_budgeted_search_candidate(view, &mut remaining_verbatim_chars))
                .collect::<Vec<_>>()
        } else {
            if candidates.len() > remaining_candidates {
                candidates.truncate(remaining_candidates);
                response_truncated = true;
            }
            remaining_candidates = remaining_candidates.saturating_sub(candidates.len());
            for candidate in &mut candidates {
                if truncate_candidate_evidence(candidate, &mut remaining_excerpt_chars) {
                    response_truncated = true;
                }
            }
            any_candidates |= !candidates.is_empty();
            all_candidates.extend(candidates.iter().cloned());
            candidates
                .iter()
                .map(|candidate| render_search_candidate(candidate, &mut remaining_verbatim_chars))
                .collect::<Vec<_>>()
        };
        any_failures |= !failures.is_empty();
        let mut result = serde_json::Map::from_iter([
            (
                "id".to_owned(),
                Value::String(query.id.unwrap_or_else(|| format!("q{index}"))),
            ),
            ("candidates".to_owned(), Value::Array(rendered)),
        ]);
        if let Some(goal) = query.goal {
            result.insert("goal".to_owned(), Value::String(goal));
        }
        if !failures.is_empty() {
            result.insert(
                "query_status".to_owned(),
                Value::String("partial".to_owned()),
            );
            result.insert("lane_failures".to_owned(), json!(failures));
        }
        result_sets.push(Value::Object(result));
    }
    let budget_ms = elapsed_ms(budget_started);
    record_candidate_usage(&state, &auth, &all_candidates);
    let mut response_data = serde_json::Map::from_iter([
        ("workspace_generation".to_owned(), json!(generation)),
        ("results".to_owned(), Value::Array(result_sets)),
    ]);
    if response_truncated {
        response_data.insert("response_truncated".to_owned(), Value::Bool(true));
    }
    let mut envelope = WorkspaceEnvelope::complete(Value::Object(response_data));
    envelope.session_id = request.session_id;
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if any_failures {
        envelope.status = if any_candidates {
            ResponseStatus::Partial
        } else {
            ResponseStatus::Degraded
        };
    }
    let total_ms = elapsed_ms(started);
    if state.config.observability_timings_ms {
        let attributed_ms = query_execution_ms + budget_ms + generation_ms + features_ms;
        envelope.timings_ms = Some(json!({
            "queries": query_timings,
            "retrieval_wall": round_ms(query_execution_ms),
            "budget": round_ms(budget_ms),
            "generation": round_ms(generation_ms),
            "features": round_ms(features_ms),
            "unattributed": round_ms((total_ms - attributed_ms).max(0.0)),
            "total": round_ms(total_ms),
        }));
    }
    metrics::histogram!("simple.search.duration_ms").record(total_ms);
    metrics::histogram!("simple.search.candidates").record(all_candidates.len() as f64);
    Ok(Json(envelope))
}

pub async fn read(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ReadRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let started = Instant::now();
    if request.requests.is_empty() || request.requests.len() > 32 {
        return Err(ApiError::invalid(
            "read requires between 1 and 32 exact requests",
        ));
    }
    let requested_count = request.requests.len();
    let generation = current_generation(&state, &auth).await?;
    let features = feature_snapshot(&state, &auth, generation).await?;
    let mut items = Vec::with_capacity(requested_count);
    let mut used_entries = Vec::new();
    let mut remaining_chars = MAX_READ_RESPONSE_CHARS;
    let mut skipped_requests = 0_usize;
    let mut missing_requests = 0_usize;
    let mut resolved = futures::stream::iter(request.requests.into_iter().enumerate().map(
        |(index, item)| {
            let state = state.clone();
            let auth = auth.clone();
            let features = features.clone();
            async move {
                let mut entry = resolve_entry_version(
                    &state,
                    &auth,
                    item.path.as_deref(),
                    item.reference.as_deref(),
                    item.version,
                )
                .await;
                let mut current_truth = None;
                let mut disabled_notice = false;
                if item.view.as_deref() == Some("current_truth") {
                    if state.config.supersession_demotion {
                        if let (Ok(requested), Some(snapshot)) = (&entry, features.as_deref()) {
                            let resolution = snapshot.resolve_current_truth(&requested.path);
                            if resolution.warning.is_none()
                                && resolution.head_path != requested.path
                            {
                                entry = resolve_entry_version(
                                    &state,
                                    &auth,
                                    Some(&resolution.head_path),
                                    None,
                                    None,
                                )
                                .await;
                            }
                            current_truth = Some(resolution);
                        }
                    } else {
                        disabled_notice = true;
                    }
                }
                (index, item, entry, current_truth, disabled_notice)
            }
        },
    ))
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await;
    resolved.sort_by_key(|(index, _, _, _, _)| *index);
    for (_, item, resolved_entry, current_truth, disabled_notice) in resolved {
        if remaining_chars == 0 {
            skipped_requests += 1;
            continue;
        }
        let entry = match resolved_entry {
            Ok(entry) => entry,
            Err(ApiError::Public {
                status: StatusCode::NOT_FOUND,
                code,
                message,
                details,
            }) => {
                missing_requests += 1;
                items.push(json!({
                    "status": "not_found",
                    "path": item.path,
                    "reference": item.reference,
                    "error": {
                        "code": code,
                        "message": message,
                        "details": details
                    }
                }));
                continue;
            }
            Err(error) => return Err(error),
        };
        used_entries.push(entry.id);
        let max_chars = item
            .max_chars
            .unwrap_or(256_000)
            .clamp(1, MAX_EXACT_READ_CHARS)
            .min(remaining_chars);
        let mut rendered = render_read(&entry, &item, max_chars)?;
        if let Some(resolution) = current_truth {
            rendered["supersession_chain"] = json!(resolution.chain);
            if let Some(warning) = resolution.warning {
                rendered["supersession_warning"] = json!(warning);
            }
        } else if disabled_notice {
            rendered["current_truth_notice"] = Value::String(
                "supersession_demotion is disabled; the requested document was returned unchanged"
                    .to_owned(),
            );
        }
        remaining_chars = remaining_chars.saturating_sub(
            rendered
                .get("text")
                .and_then(Value::as_str)
                .map(|text| text.chars().count())
                .unwrap_or(0),
        );
        items.push(rendered);
    }
    record_entry_usage(&state, &auth, &used_entries, UsageOperation::Read);
    let returned_entries = used_entries.len();
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "workspace_generation": generation,
        "items": items,
        "response_truncated": skipped_requests > 0,
        "missing_requests": missing_requests,
        "requested_count": requested_count
    }));
    envelope.session_id = request.session_id;
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if skipped_requests > 0 || missing_requests > 0 {
        envelope.status = if returned_entries == 0 {
            ResponseStatus::Degraded
        } else {
            ResponseStatus::Partial
        };
    }
    if skipped_requests > 0 {
        envelope.gaps.push(json!({
            "kind": "read_response_budget",
            "skipped_requests": skipped_requests,
            "message": "request the remaining exact entries in a follow-up read"
        }));
    }
    if missing_requests > 0 {
        envelope.gaps.push(json!({
            "kind": "read_entries_not_found",
            "missing_requests": missing_requests,
            "message": "missing entries were reported per item; valid exact reads were retained"
        }));
    }
    metrics::histogram!("simple.read.duration_ms")
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!("simple.read.entries").record(returned_entries as f64);
    Ok(Json(envelope))
}

pub async fn write(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<WriteRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    require_write_capabilities(&auth, &request.path)?;
    validate_write_path(&request)?;
    let prepared = prepare_markdown(&state, request).await?;
    let receipt = commit_markdown(&state, &auth, prepared).await?;
    let mut envelope = WorkspaceEnvelope::complete(receipt.clone());
    envelope.status = if receipt.get("no_op") == Some(&Value::Bool(true)) {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = receipt
        .get("workspace_generation")
        .and_then(Value::as_i64)
        .map(|generation| format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn capture(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CaptureRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Save)?;
    if request.content.trim().is_empty() {
        return Err(ApiError::invalid("capture content is required"));
    }
    let stable_capture = request.idempotency_key.is_some();
    let path = match request.idempotency_key.as_deref() {
        Some(key) => {
            validate_idempotency_key(key)?;
            let digest = hex::encode(Sha256::digest(
                format!("{}\0{key}", auth.user_id.0).as_bytes(),
            ));
            format!("Inbox/Captures/Stable/{digest}.md")
        }
        None => {
            let date = Utc::now().format("%Y/%m/%d");
            format!("Inbox/Captures/{date}/{}.md", Uuid::now_v7())
        }
    };
    let source = serde_json::to_string_pretty(&request.source)?;
    let content = format!(
        "# Captured context\n\n{}\n\n## Source\n\n```json\n{}\n```\n",
        request.content.trim(),
        source
    );
    let prepared = prepare_markdown(
        &state,
        WriteRequest {
            path,
            content,
            media_type: markdown_media_type(),
            expected_version: stable_capture.then_some(0),
            idempotency_key: request.idempotency_key,
            metadata: json!({
                "kind": "capture",
                "intent": request.intent
            }),
        },
    )
    .await?;
    let receipt = commit_markdown(&state, &auth, prepared).await?;
    let mut envelope = WorkspaceEnvelope::complete(receipt.clone());
    envelope.status = if receipt.get("no_op") == Some(&Value::Bool(true)) {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = receipt
        .get("workspace_generation")
        .and_then(Value::as_i64)
        .map(|generation| format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn checkpoint(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CheckpointRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Checkpoint)?;
    if let Some(parent_checkpoint_ref) = request.parent_checkpoint_id.as_deref() {
        read_checkpoint(&state, &auth, parent_checkpoint_ref).await?;
    }
    let checkpoint_uuid = deterministic_checkpoint_id(&request);
    let checkpoint_ref = format!("checkpoint:{checkpoint_uuid}");
    let path = format!(".straylight/checkpoints/{checkpoint_uuid}.md");
    match resolve_entry(&state, &auth, Some(&path), None).await {
        Ok(existing) => {
            let metadata = existing
                .metadata
                .get("client")
                .filter(|value| value.is_object())
                .unwrap_or(&existing.metadata);
            if metadata.get("kind").and_then(Value::as_str) != Some("checkpoint") {
                return Err(ApiError::conflict(
                    "checkpoint_path_conflict",
                    "the deterministic checkpoint path is occupied by another entry",
                    json!({"path": path}),
                ));
            }
            let generation = metadata
                .get("workspace_generation")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let mut envelope = WorkspaceEnvelope::complete(json!({
                "checkpoint_id": checkpoint_ref,
                "checkpoint_ref": checkpoint_ref,
                "path": existing.path,
                "workspace_generation": generation,
                "source_entries": metadata
                    .get("source_entries")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                "write": {
                    "entry_ref": format!("entry:{}", existing.id),
                    "version_ref": format!("entry-version:{}", existing.version_id),
                    "path": existing.path,
                    "version": existing.version,
                    "content_hash": format!("sha256:{}", existing.content_sha256),
                    "workspace_generation": generation,
                    "no_op": true
                }
            }));
            envelope.status = ResponseStatus::NoOp;
            envelope.session_id = Some(request.session_id);
            envelope.corpus_revision = Some(format!("generation:{generation}"));
            return Ok(Json(envelope));
        }
        Err(ApiError::Public {
            status: StatusCode::NOT_FOUND,
            ..
        }) => {}
        Err(error) => return Err(error),
    }
    let generation = current_generation(&state, &auth).await?;
    let source_entries =
        resolve_checkpoint_sources(&state, &auth, &request.state, &request.source_refs).await?;
    let content =
        render_checkpoint_markdown(checkpoint_uuid, generation, &request, &source_entries)?;
    let mut prepared = prepare_markdown(
        &state,
        WriteRequest {
            path: path.clone(),
            content,
            media_type: markdown_media_type(),
            expected_version: Some(0),
            idempotency_key: request.idempotency_key.clone(),
            metadata: json!({
                "kind": "checkpoint",
                "checkpoint_ref": checkpoint_ref,
                "workspace_generation": generation,
                "session_id": request.session_id,
                "parent_checkpoint_ref": request.parent_checkpoint_id,
                "source_refs": request.source_refs,
                "source_entries": source_entries.iter().map(entry_reference).collect::<Vec<_>>()
            }),
        },
    )
    .await?;
    prepared.entry_id_hint = Some(checkpoint_uuid);
    let receipt = commit_markdown(&state, &auth, prepared).await?;
    let resulting_generation = receipt
        .get("workspace_generation")
        .and_then(Value::as_i64)
        .unwrap_or(generation);
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "checkpoint_id": checkpoint_ref,
        "checkpoint_ref": checkpoint_ref,
        "path": path,
        "workspace_generation": resulting_generation,
        "source_entries": source_entries.iter().map(entry_reference).collect::<Vec<_>>(),
        "write": receipt
    }));
    envelope.status = ResponseStatus::Committed;
    envelope.session_id = Some(request.session_id);
    envelope.corpus_revision = Some(format!("generation:{resulting_generation}"));
    Ok(Json(envelope))
}

pub async fn changes(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ChangesQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let limit = query.limit.unwrap_or(200).clamp(1, 2_000);
    let page = changes_since(&state, &auth, query.since_generation, limit).await?;
    let generation = current_generation(&state, &auth).await?;
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "since_generation": query.since_generation,
        "workspace_generation": generation,
        "changes": page.changes,
        "truncated": page.truncated,
        "next_generation": page.next_generation
    }));
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn delete_entry(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(entry_ref): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Delete)?;
    let entry_id = entry_ref
        .strip_prefix("entry:")
        .unwrap_or(&entry_ref)
        .parse::<Uuid>()
        .map_err(|_| ApiError::invalid("delete requires an entry:<uuid> reference"))?;
    let mut tx = state.begin_write(&auth).await?;
    let row = sqlx::query(
        r#"
        SELECT entry.path,entry.kind,entry.current_version,entry.deleted_at,
               version.content_sha256,version.metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1 AND entry.id=$2
        FOR UPDATE OF entry
        "#,
    )
    .bind(auth.user_id.0)
    .bind(entry_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("entry_not_found", &entry_ref))?;
    let path: String = row.get("path");
    validate_public_path(&path)?;
    let version: i64 = row.get("current_version");
    if let Some(expected_version) = query.expected_version
        && expected_version != version
    {
        return Err(ApiError::conflict(
            "entry_version_conflict",
            "the entry changed since it was read",
            json!({
                "entry_ref": format!("entry:{entry_id}"),
                "expected_version": expected_version,
                "actual_version": version
            }),
        ));
    }
    if row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some() {
        tx.commit().await?;
        let mut envelope = WorkspaceEnvelope::complete(json!({
            "entry_ref": format!("entry:{entry_id}"),
            "path": path,
            "version": version,
            "no_op": true
        }));
        envelope.status = ResponseStatus::NoOp;
        return Ok(Json(envelope));
    }
    sqlx::query("DELETE FROM straylight.search_chunks WHERE user_id=$1 AND entry_id=$2")
        .bind(auth.user_id.0)
        .bind(entry_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        UPDATE straylight.entries
        SET deleted_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(entry_id)
    .execute(&mut *tx)
    .await?;
    let mut generation = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO straylight.workspace_changes (
          user_id,entry_id,entry_version,operation,path,content_sha256
        ) VALUES ($1,$2,$3,'delete',$4,$5)
        RETURNING generation
        "#,
    )
    .bind(auth.user_id.0)
    .bind(entry_id)
    .bind(version)
    .bind(&path)
    .bind(row.get::<String, _>("content_sha256"))
    .fetch_one(&mut *tx)
    .await?;
    let entry_metadata: Value = row.get("metadata");
    if row.get::<String, _>("kind") == "binary"
        && let Some(companion_path) = entry_metadata.get("companion_path").and_then(Value::as_str)
    {
        let companion = sqlx::query(
            r#"
            SELECT entry.id,entry.current_version,version.content_sha256
            FROM straylight.entries AS entry
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            WHERE entry.user_id=$1
              AND lower(normalize(entry.path, NFC))=$2
              AND entry.deleted_at IS NULL
            FOR UPDATE OF entry
            "#,
        )
        .bind(auth.user_id.0)
        .bind(portable_path_key(companion_path))
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(companion) = companion {
            let companion_id: Uuid = companion.get("id");
            let companion_version: i64 = companion.get("current_version");
            sqlx::query("DELETE FROM straylight.search_chunks WHERE user_id=$1 AND entry_id=$2")
                .bind(auth.user_id.0)
                .bind(companion_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                r#"
                UPDATE straylight.entries
                SET deleted_at=clock_timestamp(),updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                "#,
            )
            .bind(auth.user_id.0)
            .bind(companion_id)
            .execute(&mut *tx)
            .await?;
            let companion_generation = sqlx::query_scalar::<_, i64>(
                r#"
                INSERT INTO straylight.workspace_changes (
                  user_id,entry_id,entry_version,operation,path,content_sha256
                ) VALUES ($1,$2,$3,'delete',$4,$5)
                RETURNING generation
                "#,
            )
            .bind(auth.user_id.0)
            .bind(companion_id)
            .bind(companion_version)
            .bind(companion_path)
            .bind(companion.get::<String, _>("content_sha256"))
            .fetch_one(&mut *tx)
            .await?;
            generation = generation.max(companion_generation);
        }
    }
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "entry_ref": format!("entry:{entry_id}"),
        "path": path,
        "version": version,
        "workspace_generation": generation,
        "no_op": false
    }));
    envelope.status = ResponseStatus::Committed;
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn queue_dream(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DreamRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    if !state.config.dream_scheduler_enabled {
        return Err(ApiError::public(
            StatusCode::SERVICE_UNAVAILABLE,
            "dreaming_disabled",
            "dreaming is disabled by the operator",
        ));
    }
    auth.require(Capability::Dream)?;
    let mut tx = state.begin_write(&auth).await?;
    let current_generation = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(generation) FROM straylight.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);
    let previous_watermark = match request.since_generation {
        Some(value) => value.max(0),
        None => sqlx::query_scalar::<_, Option<i64>>(
            r#"
            SELECT max(watermark)
            FROM straylight.jobs
            WHERE user_id=$1 AND kind='dream_workspace' AND status='complete'
            "#,
        )
        .bind(auth.user_id.0)
        .fetch_one(&mut *tx)
        .await?
        .unwrap_or(0),
    };
    if current_generation <= previous_watermark {
        tx.commit().await?;
        let mut envelope = WorkspaceEnvelope::complete(json!({
            "workspace_generation": current_generation,
            "watermark": previous_watermark,
            "no_op": true,
            "reason": "no workspace changes after the dream watermark"
        }));
        envelope.status = ResponseStatus::NoOp;
        return Ok(Json(envelope));
    }
    if let Some(existing) = sqlx::query(
        r#"
        SELECT id,status,created_at
        FROM straylight.jobs
        WHERE user_id=$1 AND kind='dream_workspace'
          AND status IN ('queued','running')
        ORDER BY created_at
        LIMIT 1
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        let id: Uuid = existing.get("id");
        let mut envelope = WorkspaceEnvelope::complete(json!({
            "job_ref": format!("job:{id}"),
            "status": existing.get::<String, _>("status"),
            "workspace_generation": current_generation,
            "watermark": previous_watermark,
            "no_op": true,
            "reason": "a dream job is already queued or running"
        }));
        envelope.status = ResponseStatus::NoOp;
        return Ok(Json(envelope));
    }
    let job_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.jobs (
          id,user_id,kind,status,payload,watermark
        ) VALUES ($1,$2,'dream_workspace','queued',$3,$4)
        "#,
    )
    .bind(job_id)
    .bind(auth.user_id.0)
    .bind(json!({
        "from_generation": previous_watermark,
        "to_generation": current_generation,
        "focus": request.focus
    }))
    .bind(previous_watermark)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "job_ref": format!("job:{job_id}"),
        "status": "queued",
        "workspace_generation": current_generation,
        "watermark": previous_watermark,
        "no_op": false
    }));
    envelope.status = ResponseStatus::Committed;
    Ok(Json(envelope))
}

pub async fn list_jobs(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<JobsQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    if let Some(status) = query.status.as_deref()
        && !matches!(status, "queued" | "running" | "complete" | "failed")
    {
        return Err(ApiError::invalid("unsupported job status"));
    }
    let limit = query.limit.unwrap_or(100).clamp(1, 500) as i64;
    let offset = query.offset.unwrap_or(0).min(100_000) as i64;
    let mut tx = state.begin_read(&auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT id,kind,status,payload,watermark,attempts,available_at,
               started_at,finished_at,last_error,created_at
        FROM straylight.jobs
        WHERE user_id=$1
          AND ($2::text IS NULL OR status=$2)
        ORDER BY created_at DESC,id DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.status.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let jobs = rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.get("id");
            json!({
                "job_ref": format!("job:{id}"),
                "kind": row.get::<String, _>("kind"),
                "status": row.get::<String, _>("status"),
                "payload": row.get::<Value, _>("payload"),
                "watermark": row.get::<Option<i64>, _>("watermark"),
                "attempts": row.get::<i32, _>("attempts"),
                "available_at": row.get::<DateTime<Utc>, _>("available_at"),
                "started_at": row.get::<Option<DateTime<Utc>>, _>("started_at"),
                "finished_at": row.get::<Option<DateTime<Utc>>, _>("finished_at"),
                "last_error": row.get::<Option<String>, _>("last_error"),
                "created_at": row.get::<DateTime<Utc>, _>("created_at")
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(WorkspaceEnvelope::complete(json!({
        "jobs": jobs,
        "offset": offset,
        "limit": limit,
        "truncated": jobs.len() == usize::try_from(limit).unwrap_or(usize::MAX)
    }))))
}

pub async fn list_binaries(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<BinaryListQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500) as i64;
    let offset = query.offset.unwrap_or(0).min(100_000) as i64;
    let mut tx = state.begin_read(&auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.title,entry.media_type,
               entry.current_version,version.content_sha256,
               version.size_bytes,version.metadata,entry.updated_at,
               companion_version.metadata AS companion_metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        LEFT JOIN straylight.entries AS companion
          ON companion.user_id=entry.user_id
         AND lower(normalize(companion.path, NFC))
             =lower(normalize(version.metadata->>'companion_path', NFC))
         AND companion.kind='markdown'
         AND companion.deleted_at IS NULL
        LEFT JOIN straylight.entry_versions AS companion_version
          ON companion_version.user_id=companion.user_id
         AND companion_version.entry_id=companion.id
         AND companion_version.version=companion.current_version
        WHERE entry.user_id=$1
          AND entry.kind='binary'
          AND entry.deleted_at IS NULL
        ORDER BY entry.path
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let binaries = rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.get("id");
            let metadata = merge_binary_description_metadata(
                row.get("metadata"),
                row.get("companion_metadata"),
            );
            json!({
                "entry_ref": format!("entry:{id}"),
                "path": row.get::<String, _>("path"),
                "title": row.get::<String, _>("title"),
                "media_type": row.get::<String, _>("media_type"),
                "version": row.get::<i64, _>("current_version"),
                "content_hash": format!("sha256:{}", row.get::<String, _>("content_sha256")),
                "size_bytes": row.get::<i64, _>("size_bytes"),
                "metadata": metadata,
                "updated_at": row.get::<DateTime<Utc>, _>("updated_at")
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(WorkspaceEnvelope::complete(
        json!({"binaries": binaries}),
    )))
}

pub async fn manifest(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ManifestQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let limit = query.limit.unwrap_or(1_000).clamp(1, 5_000) as i64;
    let offset = query.offset.unwrap_or(0).min(10_000_000) as i64;
    let cursor_supplied = query.after_path.is_some()
        || query.after_entry_ref.is_some()
        || query.after_version.is_some();
    if cursor_supplied && offset > 0 {
        return Err(ApiError::invalid(
            "manifest cursor and offset cannot be used together",
        ));
    }
    let cursor_entry_id = match (
        query.after_path.as_deref(),
        query.after_entry_ref.as_deref(),
    ) {
        (None, None) => None,
        (Some(_), Some(reference)) => Some(
            reference
                .strip_prefix("entry:")
                .unwrap_or(reference)
                .parse::<Uuid>()
                .map_err(|_| ApiError::invalid("after_entry_ref must be an entry:<uuid>"))?,
        ),
        _ => {
            return Err(ApiError::invalid(
                "after_path and after_entry_ref must be provided together",
            ));
        }
    };
    if query.history {
        if cursor_entry_id.is_some() != query.after_version.is_some() {
            return Err(ApiError::invalid(
                "history cursors require after_path, after_entry_ref, and after_version",
            ));
        }
        if query.after_version.is_some_and(|version| version <= 0) {
            return Err(ApiError::invalid("after_version must be positive"));
        }
    } else if query.after_version.is_some() {
        return Err(ApiError::invalid(
            "after_version is only valid for history manifests",
        ));
    }

    let mut tx = state.begin_read(&auth).await?;
    let mut statement = if query.history {
        QueryBuilder::<Postgres>::new(
            r#"
            SELECT entry.id,entry.path,entry.title,entry.kind,entry.media_type,
                   entry.current_version,entry.deleted_at,
                   version.id AS version_id,version.version AS version_number,
                   version.content_sha256,version.size_bytes,version.metadata,
                   version.created_at AS version_created_at
            FROM straylight.entries AS entry
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
            WHERE entry.user_id=
            "#,
        )
    } else {
        QueryBuilder::<Postgres>::new(
            r#"
            SELECT entry.id,entry.path,entry.title,entry.kind,entry.media_type,
                   entry.current_version,entry.deleted_at,
                   version.id AS version_id,version.version AS version_number,
                   version.content_sha256,version.size_bytes,version.metadata,
                   version.created_at AS version_created_at
            FROM straylight.entries AS entry
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            WHERE entry.user_id=
            "#,
        )
    };
    statement.push_bind(auth.user_id.0);
    if !query.history {
        statement.push(" AND entry.deleted_at IS NULL");
    }
    if let (Some(after_path), Some(entry_id)) = (query.after_path.as_deref(), cursor_entry_id) {
        if query.history {
            statement
                .push(" AND (entry.path,entry.id,version.version) > (")
                .push_bind(after_path)
                .push(",")
                .push_bind(entry_id)
                .push(",")
                .push_bind(query.after_version.expect("validated above"))
                .push(")");
        } else {
            statement
                .push(" AND (entry.path,entry.id) > (")
                .push_bind(after_path)
                .push(",")
                .push_bind(entry_id)
                .push(")");
        }
    }
    statement.push(" ORDER BY entry.path,entry.id");
    if query.history {
        statement.push(",version.version");
    }
    statement.push(" LIMIT ").push_bind(limit + 1);
    if !cursor_supplied && offset > 0 {
        statement.push(" OFFSET ").push_bind(offset);
    }
    let mut rows = statement.build().fetch_all(&mut *tx).await?;
    let generation = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(generation) FROM straylight.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);
    tx.commit().await?;
    let truncated = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if truncated {
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    let next = if truncated {
        rows.last().map(|row| {
            let id: Uuid = row.get("id");
            let mut cursor = serde_json::Map::from_iter([
                (
                    "after_path".to_owned(),
                    Value::String(row.get::<String, _>("path")),
                ),
                (
                    "after_entry_ref".to_owned(),
                    Value::String(format!("entry:{id}")),
                ),
            ]);
            if query.history {
                cursor.insert(
                    "after_version".to_owned(),
                    json!(row.get::<i64, _>("version_number")),
                );
            }
            Value::Object(cursor)
        })
    } else {
        None
    };
    let entries = rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.get("id");
            json!({
                "entry_ref": format!("entry:{id}"),
                "path": row.get::<String, _>("path"),
                "title": row.get::<String, _>("title"),
                "kind": row.get::<String, _>("kind"),
                "media_type": row.get::<String, _>("media_type"),
                "version": row.get::<i64, _>("version_number"),
                "version_ref": format!(
                    "entry-version:{}",
                    row.get::<Uuid, _>("version_id")
                ),
                "current": row.get::<i64, _>("version_number")
                    == row.get::<i64, _>("current_version"),
                "deleted": row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some(),
                "content_hash": format!("sha256:{}", row.get::<String, _>("content_sha256")),
                "size_bytes": row.get::<i64, _>("size_bytes"),
                "metadata": row.get::<Value, _>("metadata"),
                "created_at": row.get::<DateTime<Utc>, _>("version_created_at")
            })
        })
        .collect::<Vec<_>>();
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "workspace_generation": generation,
        "entries": entries,
        "history": query.history,
        "offset": offset,
        "limit": limit,
        "truncated": truncated,
        "next": next
    }));
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn usage(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<UsageQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let sort = query.sort.as_deref().unwrap_or("most_used");
    let ordering = match sort {
        "most_used" => {
            "(coalesce(usage.read_count,0)+coalesce(usage.search_count,0)) DESC, entry.path"
        }
        "least_used" => "(coalesce(usage.read_count,0)+coalesce(usage.search_count,0)), entry.path",
        "least_recently_used" => "usage.last_used_at NULLS FIRST, entry.path",
        "most_recently_used" => "usage.last_used_at DESC NULLS LAST, entry.path",
        _ => {
            return Err(ApiError::invalid(
                "usage sort must be most_used, least_used, least_recently_used, or most_recently_used",
            ));
        }
    };
    let limit = query.limit.unwrap_or(100).clamp(1, 500) as i64;
    let offset = query.offset.unwrap_or(0).min(10_000_000) as i64;
    let mut statement = QueryBuilder::<Postgres>::new(
        r#"
        SELECT entry.id,entry.path,entry.title,entry.kind,
               coalesce(usage.read_count,0) AS read_count,
               coalesce(usage.search_count,0) AS search_count,
               usage.first_used_at,usage.last_used_at,
               usage.last_read_at,usage.last_search_at
        FROM straylight.entries AS entry
        LEFT JOIN straylight.entry_usage AS usage
          ON usage.user_id=entry.user_id AND usage.entry_id=entry.id
        WHERE entry.user_id=
        "#,
    );
    statement
        .push_bind(auth.user_id.0)
        .push(" AND entry.deleted_at IS NULL ORDER BY ")
        .push(ordering)
        .push(" LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    let mut tx = state.begin_read(&auth).await?;
    let rows = statement.build().fetch_all(&mut *tx).await?;
    tx.commit().await?;
    let entries = rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.get("id");
            let reads: i64 = row.get("read_count");
            let searches: i64 = row.get("search_count");
            json!({
                "entry_ref": format!("entry:{id}"),
                "path": row.get::<String, _>("path"),
                "title": row.get::<String, _>("title"),
                "kind": row.get::<String, _>("kind"),
                "read_count": reads,
                "search_count": searches,
                "total_uses": reads.saturating_add(searches),
                "first_used_at": row.get::<Option<DateTime<Utc>>, _>("first_used_at"),
                "last_used_at": row.get::<Option<DateTime<Utc>>, _>("last_used_at"),
                "last_read_at": row.get::<Option<DateTime<Utc>>, _>("last_read_at"),
                "last_search_at": row.get::<Option<DateTime<Utc>>, _>("last_search_at")
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(WorkspaceEnvelope::complete(json!({
        "sort": sort,
        "entries": entries,
        "offset": offset,
        "limit": limit
    }))))
}

pub async fn upload_binary(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    mut multipart: Multipart,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Stage)?;
    let mut path = None;
    let mut media_type = None;
    let mut description = None;
    let mut provenance = None;
    let mut limitations = None;
    let mut mtime_ns = None;
    let mut mode = None;
    let mut expected_version = None;
    let mut expected_content_hash = None;
    let mut bytes: Option<Bytes> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::invalid(format!("invalid binary upload: {error}")))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "file" => {
                if bytes.is_some() {
                    return Err(ApiError::invalid(
                        "workspace binary upload accepts exactly one file",
                    ));
                }
                if media_type.is_none() {
                    media_type = field.content_type().map(ToOwned::to_owned);
                }
                bytes = Some(field.bytes().await.map_err(|error| {
                    ApiError::invalid(format!("invalid binary bytes: {error}"))
                })?);
            }
            "path" => path = Some(read_small_field(field, "path", 1_024).await?),
            "media_type" => media_type = Some(read_small_field(field, "media_type", 255).await?),
            "description" => {
                description = Some(read_small_field(field, "description", 256_000).await?)
            }
            "provenance" => provenance = Some(read_small_field(field, "provenance", 32_000).await?),
            "limitations" => {
                limitations = Some(read_small_field(field, "limitations", 32_000).await?)
            }
            "mtime_ns" => {
                let value = read_small_field(field, "mtime_ns", 32).await?;
                mtime_ns = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| ApiError::invalid("mtime_ns must be an integer"))?,
                );
            }
            "mode" => {
                let value = read_small_field(field, "mode", 16).await?;
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| ApiError::invalid("mode must be an integer"))?;
                if parsed > 0o777 {
                    return Err(ApiError::invalid(
                        "mode may contain only portable permission bits",
                    ));
                }
                mode = Some(parsed);
            }
            "expected_version" => {
                let value = read_small_field(field, "expected_version", 32).await?;
                expected_version = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| ApiError::invalid("expected_version must be an integer"))?,
                );
            }
            "expected_content_hash" => {
                expected_content_hash =
                    Some(read_small_field(field, "expected_content_hash", 80).await?);
            }
            _ => {
                return Err(ApiError::invalid(format!(
                    "unsupported binary upload field {name}"
                )));
            }
        }
    }
    let path = path.ok_or_else(|| ApiError::invalid("binary path is required"))?;
    validate_public_path(&path)?;
    let expected_content_hash = validate_sha256(
        expected_content_hash
            .as_deref()
            .ok_or_else(|| ApiError::invalid("expected_content_hash is required"))?,
    )?;
    let bytes = bytes.ok_or_else(|| ApiError::invalid("binary file is required"))?;
    let media_type = media_type
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            mime_guess::from_path(&path)
                .first_or_octet_stream()
                .essence_str()
                .to_owned()
        });
    let stored = state
        .object_store
        .put_user_blob(auth.user_id, Some(&media_type), bytes)
        .await?;
    if validate_sha256(&stored.sha256)? != expected_content_hash {
        return Err(ApiError::conflict(
            "content_hash_mismatch",
            "uploaded binary bytes changed after the caller calculated their hash",
            json!({"path": path}),
        ));
    }
    let portable_metadata = (mtime_ns.is_some() || mode.is_some()).then(|| {
        json!({
            "modified_unix_ns": mtime_ns,
            "mode": mode
        })
    });
    let receipt = commit_binary_with_companion(
        &state,
        &auth,
        &path,
        &media_type,
        &stored.sha256,
        u64::try_from(stored.size_bytes).unwrap_or(u64::MAX),
        &stored.object_key,
        stored.object_version_id.as_deref(),
        description.as_deref(),
        provenance.as_deref(),
        limitations.as_deref(),
        portable_metadata,
        expected_version,
    )
    .await?;
    let mut envelope = WorkspaceEnvelope::complete(receipt.clone());
    envelope.status = if receipt.get("no_op") == Some(&Value::Bool(true)) {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = receipt
        .get("workspace_generation")
        .and_then(Value::as_i64)
        .map(|generation| format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn upload_binary_stream(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<StreamingBinaryQuery>,
    body: Body,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Stage)?;
    validate_public_path(&query.path)?;
    let expected_content_hash = validate_sha256(&query.expected_content_hash)?;
    let media_type = query
        .media_type
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            mime_guess::from_path(&query.path)
                .first_or_octet_stream()
                .essence_str()
                .to_owned()
        });
    if media_type.len() > 255 {
        return Err(ApiError::invalid("media_type is limited to 255 characters"));
    }
    if query.mode.is_some_and(|value| value > 0o777) {
        return Err(ApiError::invalid(
            "mode may contain only portable permission bits",
        ));
    }
    let temporary_path =
        std::env::temp_dir().join(format!("straylight-binary-upload-{}", Uuid::now_v7()));
    let transfer = async {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .await
            .map_err(|error| {
                ApiError::Internal(format!("could not create upload buffer: {error}"))
            })?;
        let mut stream = body.into_data_stream();
        let mut size_bytes = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                ApiError::invalid(format!("binary upload stream failed: {error}"))
            })?;
            size_bytes = size_bytes
                .checked_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| ApiError::invalid("binary upload size overflow"))?;
            if size_bytes > MAX_STREAMED_BINARY_BYTES {
                return Err(ApiError::public(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "binary_too_large",
                    "workspace binary uploads are limited to 4 GiB",
                ));
            }
            file.write_all(&chunk).await.map_err(|error| {
                ApiError::Internal(format!("could not buffer binary upload: {error}"))
            })?;
        }
        file.flush().await.map_err(|error| {
            ApiError::Internal(format!("could not flush binary upload: {error}"))
        })?;
        drop(file);
        state
            .object_store
            .put_user_file_blob(auth.user_id, &media_type, &temporary_path)
            .await
    }
    .await;
    let _ = tokio::fs::remove_file(&temporary_path).await;
    let stored = transfer?;
    if validate_sha256(&stored.sha256)? != expected_content_hash {
        return Err(ApiError::conflict(
            "content_hash_mismatch",
            "uploaded binary bytes changed after the caller calculated their hash",
            json!({"path": query.path}),
        ));
    }
    let portable_metadata = json!({
        "modified_unix_ns": query.mtime_ns,
        "mode": query.mode
    });
    let receipt = commit_binary_with_companion(
        &state,
        &auth,
        &query.path,
        &media_type,
        &stored.sha256,
        stored.size_bytes,
        &stored.object_key,
        stored.object_version_id.as_deref(),
        None,
        query.provenance.as_deref(),
        None,
        Some(portable_metadata),
        query.expected_version,
    )
    .await?;
    let mut envelope = WorkspaceEnvelope::complete(receipt.clone());
    envelope.status = if receipt.get("no_op") == Some(&Value::Bool(true)) {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = receipt
        .get("workspace_generation")
        .and_then(Value::as_i64)
        .map(|generation| format!("generation:{generation}"));
    Ok(Json(envelope))
}

pub async fn fetch_binary(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(entry_ref): Path<String>,
    Query(query): Query<BinaryVersionQuery>,
) -> ApiResult<Response<Body>> {
    auth.require(Capability::Read)?;
    let entry = resolve_entry_version(&state, &auth, None, Some(&entry_ref), query.version).await?;
    if entry.kind != "binary" {
        return Err(ApiError::invalid("the requested entry is not binary"));
    }
    let key = entry
        .object_key
        .as_deref()
        .ok_or_else(|| ApiError::Internal("binary entry has no object key".to_owned()))?;
    let stream = state
        .object_store
        .get_stream_version(key, entry.object_version_id.as_deref())
        .await?;
    if stream.content_length != Some(entry.size_bytes) {
        return Err(ApiError::conflict(
            "content_size_mismatch",
            "stored binary size no longer matches its immutable metadata",
            json!({"entry_ref": format!("entry:{}", entry.id)}),
        ));
    }
    let mut response = Response::new(Body::from_stream(ReaderStream::new(
        stream.body.into_async_read(),
    )));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&entry.media_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&entry.size_bytes.to_string())
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    );
    response.headers_mut().insert(
        "x-carrystate-sha256",
        HeaderValue::from_str(&format!("sha256:{}", entry.content_sha256))
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    );
    response.headers_mut().insert(
        "x-carrystate-asset-ref",
        HeaderValue::from_str(&format!("entry:{}", entry.id))
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    );
    response.headers_mut().insert(
        "x-carrystate-asset-version",
        HeaderValue::from_str(&entry.version.to_string())
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    );
    response.headers_mut().insert(
        "x-carrystate-integrity",
        HeaderValue::from_static("client-verify-sha256"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    record_entry_usage(&state, &auth, &[entry.id], UsageOperation::Read);
    Ok(response)
}

pub async fn binary_metadata(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(entry_ref): Path<String>,
    Query(query): Query<BinaryVersionQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let entry = resolve_entry_version(&state, &auth, None, Some(&entry_ref), query.version).await?;
    if entry.kind != "binary" {
        return Err(ApiError::invalid("the requested entry is not binary"));
    }
    let metadata = load_binary_description_metadata(&state, &auth, &entry).await?;
    record_entry_usage(&state, &auth, &[entry.id], UsageOperation::Read);
    Ok(Json(WorkspaceEnvelope::complete(json!({
        "entry_ref": format!("entry:{}", entry.id),
        "path": entry.path,
        "title": entry.title,
        "media_type": entry.media_type,
        "version": entry.version,
        "content_hash": format!("sha256:{}", entry.content_sha256),
        "size_bytes": entry.size_bytes,
        "metadata": metadata,
        "updated_at": entry.updated_at
    }))))
}

async fn load_binary_description_metadata(
    state: &AppState,
    auth: &AuthContext,
    entry: &EntryRow,
) -> ApiResult<Value> {
    let Some(companion_path) = entry.metadata.get("companion_path").and_then(Value::as_str) else {
        return Ok(entry.metadata.clone());
    };
    let mut tx = state.begin_read(auth).await?;
    let companion_metadata = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT version.metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND lower(normalize(entry.path, NFC))=$2
          AND entry.kind='markdown'
          AND entry.deleted_at IS NULL
        "#,
    )
    .bind(auth.user_id.0)
    .bind(portable_path_key(companion_path))
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(merge_binary_description_metadata(
        entry.metadata.clone(),
        companion_metadata,
    ))
}

fn merge_binary_description_metadata(mut binary: Value, companion: Option<Value>) -> Value {
    let Some(companion) = companion else {
        return binary;
    };
    if let Value::Object(values) = &mut binary {
        if let Some(status) = companion.get("description_status") {
            values.insert("description_status".to_owned(), status.clone());
        }
        values.insert("description".to_owned(), companion);
    }
    binary
}

async fn read_small_field(field: Field<'_>, name: &str, limit: usize) -> ApiResult<String> {
    let bytes = field
        .bytes()
        .await
        .map_err(|error| ApiError::invalid(format!("invalid {name}: {error}")))?;
    if bytes.len() > limit {
        return Err(ApiError::public(
            StatusCode::PAYLOAD_TOO_LARGE,
            "binary_metadata_too_large",
            format!("{name} is limited to {limit} bytes"),
        ));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|_| ApiError::invalid(format!("{name} must be UTF-8")))
}

#[allow(clippy::too_many_arguments)]
async fn commit_binary_with_companion(
    state: &AppState,
    auth: &AuthContext,
    path: &str,
    media_type: &str,
    stored_hash: &str,
    size_bytes: u64,
    object_key: &str,
    object_version_id: Option<&str>,
    description: Option<&str>,
    provenance: Option<&str>,
    limitations: Option<&str>,
    portable_metadata: Option<Value>,
    expected_version: Option<i64>,
) -> ApiResult<Value> {
    let object_version_id = object_version_id.ok_or_else(|| {
        ApiError::Internal("versioned object upload returned no exact version ID".to_owned())
    })?;
    let content_sha256 = stored_hash.trim_start_matches("sha256:").to_owned();
    let companion_key = hex::encode(Sha256::digest(portable_path_key(path).as_bytes()));
    let companion_path = format!(".straylight/binaries/{}.md", &companion_key[..32]);
    validate_path(&companion_path)?;
    let title = std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path);
    let supplied_description = description.map(str::trim).filter(|value| !value.is_empty());
    let description_text = supplied_description
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            format!(
                "Binary file `{path}` ({media_type}, {size_bytes} bytes). \
                 Content-specific description is pending background inspection."
            )
        });
    let provenance_text = provenance
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Uploaded through the Straylight workspace binary API.");
    let limitations_text = limitations
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(
            "The immutable binary bytes are authoritative. This Markdown companion is for retrieval and may be incomplete.",
        );
    let companion_content = format!(
        "---\nstraylight_kind: binary_description\nbinary_path: {}\n\
         content_hash: {}\nmedia_type: {}\nsize_bytes: {}\n\
         description_status: {}\n---\n\n# Binary: {}\n\n\
         ## Description\n\n{}\n\n## Provenance\n\n{}\n\n\
         ## Limitations\n\n{}\n",
        serde_json::to_string(path)?,
        serde_json::to_string(&format!("sha256:{content_sha256}"))?,
        serde_json::to_string(media_type)?,
        size_bytes,
        if supplied_description.is_some() {
            "provided"
        } else {
            "pending"
        },
        title,
        description_text,
        provenance_text,
        limitations_text
    );
    let companion = prepare_markdown(
        state,
        WriteRequest {
            path: companion_path.clone(),
            content: companion_content,
            media_type: markdown_media_type(),
            expected_version: None,
            idempotency_key: None,
            metadata: json!({
                "kind": "binary_description",
                "binary_path": path,
                "content_hash": format!("sha256:{content_sha256}"),
                "portable": portable_metadata.clone(),
                "description_status": if supplied_description.is_some() {
                    "provided"
                } else {
                    "pending"
                }
            }),
        },
    )
    .await?;

    let mut tx = state.begin_write(auth).await?;
    let mut lock_paths = [portable_path_key(path), portable_path_key(&companion_path)];
    lock_paths.sort();
    for lock_path in lock_paths {
        require_local_publish_lock(
            &mut tx,
            format!("simple-entry:{}:{lock_path}", auth.user_id.0),
        )
        .await?;
    }

    let existing = sqlx::query(
        r#"
        SELECT entry.id,entry.kind,entry.media_type,entry.current_version,entry.deleted_at,
               version.content_sha256,version.metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND lower(normalize(entry.path, NFC))=$2
        FOR UPDATE OF entry
        "#,
    )
    .bind(auth.user_id.0)
    .bind(portable_path_key(path))
    .fetch_optional(&mut *tx)
    .await?;
    if existing
        .as_ref()
        .is_some_and(|row| row.get::<String, _>("kind") != "binary")
    {
        return Err(ApiError::conflict(
            "entry_kind_conflict",
            "a Markdown entry already uses this binary path",
            json!({"path": path}),
        ));
    }
    let binary_no_op = existing
        .as_ref()
        .is_some_and(|row| row.get::<String, _>("content_sha256") == content_sha256);
    if !binary_no_op && let Some(expected) = expected_version {
        let actual = existing
            .as_ref()
            .map(|row| row.get::<i64, _>("current_version"))
            .unwrap_or(0);
        let restoring_deleted = existing.as_ref().is_some_and(|row| {
            row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some() && expected == 0
        });
        if actual != expected && !restoring_deleted {
            return Err(ApiError::conflict(
                "entry_version_conflict",
                "the entry changed since it was read",
                json!({
                    "path": path,
                    "expected_version": expected,
                    "actual_version": actual
                }),
            ));
        }
    }
    let mut binary_annotation_changed = false;
    let (binary_entry_id, binary_version, binary_generation) = if binary_no_op {
        let row = existing.as_ref().expect("checked above");
        let was_deleted = row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some();
        let mut metadata = row.get::<Value, _>("metadata");
        let previous_metadata = metadata.clone();
        if let Value::Object(values) = &mut metadata {
            values.insert(
                "companion_path".to_owned(),
                Value::String(companion_path.clone()),
            );
            if let Some(portable) = &portable_metadata {
                values.insert("portable".to_owned(), portable.clone());
            }
            if provenance.is_some() {
                values.insert(
                    "provenance".to_owned(),
                    Value::String(provenance_text.to_owned()),
                );
            }
            if limitations.is_some() {
                values.insert(
                    "limitations".to_owned(),
                    Value::String(limitations_text.to_owned()),
                );
            }
        }
        binary_annotation_changed = was_deleted
            || metadata != previous_metadata
            || row.get::<String, _>("media_type") != media_type;
        let generation = if binary_annotation_changed {
            sqlx::query(
                r#"
                UPDATE straylight.entry_versions
                SET metadata=$4
                WHERE user_id=$1 AND entry_id=$2 AND version=$3
                "#,
            )
            .bind(auth.user_id.0)
            .bind(row.get::<Uuid, _>("id"))
            .bind(row.get::<i64, _>("current_version"))
            .bind(&metadata)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE straylight.entries
                SET path=$3,media_type=$4,deleted_at=NULL,
                    updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                "#,
            )
            .bind(auth.user_id.0)
            .bind(row.get::<Uuid, _>("id"))
            .bind(path)
            .bind(media_type)
            .execute(&mut *tx)
            .await?;
            Some(
                sqlx::query_scalar::<_, i64>(
                    r#"
                    INSERT INTO straylight.workspace_changes (
                      user_id,entry_id,entry_version,operation,path,content_sha256
                    ) VALUES ($1,$2,$3,'update',$4,$5)
                    RETURNING generation
                    "#,
                )
                .bind(auth.user_id.0)
                .bind(row.get::<Uuid, _>("id"))
                .bind(row.get::<i64, _>("current_version"))
                .bind(path)
                .bind(&content_sha256)
                .fetch_one(&mut *tx)
                .await?,
            )
        } else {
            None
        };
        (
            row.get::<Uuid, _>("id"),
            row.get::<i64, _>("current_version"),
            generation,
        )
    } else {
        let (entry_id, version, operation) = match existing {
            Some(row) => (
                row.get::<Uuid, _>("id"),
                row.get::<i64, _>("current_version") + 1,
                "update",
            ),
            None => (Uuid::now_v7(), 1_i64, "create"),
        };
        if operation == "create" {
            sqlx::query(
                r#"
                INSERT INTO straylight.entries (
                  id,user_id,path,title,kind,media_type,current_version
                ) VALUES ($1,$2,$3,$4,'binary',$5,0)
                "#,
            )
            .bind(entry_id)
            .bind(auth.user_id.0)
            .bind(path)
            .bind(title)
            .bind(media_type)
            .execute(&mut *tx)
            .await?;
        }
        let version_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO straylight.entry_versions (
              id,user_id,entry_id,version,content_sha256,object_key,
              object_version_id,size_bytes,metadata,created_by_credential_id
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            "#,
        )
        .bind(version_id)
        .bind(auth.user_id.0)
        .bind(entry_id)
        .bind(version)
        .bind(&content_sha256)
        .bind(object_key)
        .bind(object_version_id)
        .bind(i64::try_from(size_bytes).unwrap_or(i64::MAX))
        .bind(json!({
            "companion_path": companion_path,
            "description_status": if supplied_description.is_some() {
                "provided"
            } else {
                "pending"
            },
            "portable": portable_metadata,
            "provenance": provenance_text,
            "limitations": limitations_text
        }))
        .bind(auth.credential_id.0)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE straylight.entries
            SET path=$3,title=$4,media_type=$5,current_version=$6,
                updated_at=clock_timestamp(),deleted_at=NULL
            WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(entry_id)
        .bind(path)
        .bind(title)
        .bind(media_type)
        .bind(version)
        .execute(&mut *tx)
        .await?;
        let generation = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO straylight.workspace_changes (
              user_id,entry_id,entry_version,operation,path,content_sha256
            ) VALUES ($1,$2,$3,$4,$5,$6)
            RETURNING generation
            "#,
        )
        .bind(auth.user_id.0)
        .bind(entry_id)
        .bind(version)
        .bind(operation)
        .bind(path)
        .bind(&content_sha256)
        .fetch_one(&mut *tx)
        .await?;
        (entry_id, version, Some(generation))
    };

    let retained_companion = if binary_no_op && supplied_description.is_none() {
        sqlx::query(
            r#"
            SELECT entry.id,entry.current_version,version.id AS version_id
            FROM straylight.entries AS entry
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            WHERE entry.user_id=$1
              AND lower(normalize(entry.path, NFC))=$2
              AND entry.kind='markdown'
              AND entry.deleted_at IS NULL
            "#,
        )
        .bind(auth.user_id.0)
        .bind(portable_path_key(&companion_path))
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| MarkdownUpsertResult {
            entry_id: row.get("id"),
            version: row.get("current_version"),
            version_id: Some(row.get("version_id")),
            generation: None,
            no_op: true,
            metadata_only: false,
        })
    } else {
        None
    };
    let should_queue_description =
        supplied_description.is_none() && (!binary_no_op || retained_companion.is_none());
    let companion_result = match retained_companion {
        Some(result) => result,
        None => {
            upsert_markdown_in_tx(
                &mut tx,
                auth.user_id.0,
                Some(auth.credential_id.0),
                companion,
            )
            .await?
        }
    };
    if should_queue_description {
        sqlx::query(
            r#"
            INSERT INTO straylight.jobs (user_id,kind,payload)
            VALUES ($1,'describe_binary',$2)
            "#,
        )
        .bind(auth.user_id.0)
        .bind(json!({
            "entry_id": binary_entry_id,
            "version": binary_version,
            "companion_path": companion_path
        }))
        .execute(&mut *tx)
        .await?;
    }
    let generation = match (binary_generation, companion_result.generation) {
        (Some(left), Some(right)) => left.max(right),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => max_generation_in_tx(&mut tx, auth.user_id.0).await?,
    };
    tx.commit().await?;
    Ok(json!({
        "entry_ref": format!("entry:{binary_entry_id}"),
        "path": path,
        "version": binary_version,
        "content_hash": format!("sha256:{content_sha256}"),
        "size_bytes": size_bytes,
        "media_type": media_type,
        "companion": {
            "entry_ref": format!("entry:{}", companion_result.entry_id),
            "path": companion_path,
            "version": companion_result.version
        },
        "workspace_generation": generation,
        "description_status": if supplied_description.is_some() {
            "provided"
        } else {
            "pending"
        },
        "metadata_only": binary_no_op && binary_annotation_changed,
        "no_op": binary_no_op && !binary_annotation_changed && companion_result.no_op
    }))
}

pub async fn import_evaluation(
    State(state): State<AppState>,
    Extension(caller): Extension<AuthContext>,
    Json(request): Json<EvalImportRequest>,
) -> ApiResult<Json<Value>> {
    if !state.config.evaluation_api_enabled {
        return Err(ApiError::not_found(
            "route_not_found",
            "/v1/workspace/admin/eval/import",
        ));
    }
    caller.require(Capability::Admin)?;
    validate_eval_import(&request)?;
    let access_mode = request.access_mode.as_str();
    let capabilities = if access_mode == "read_only" {
        vec!["open", "query", "read", "status"]
    } else {
        vec![
            "open",
            "query",
            "read",
            "status",
            "checkpoint",
            "save",
            "stage",
            "correct",
            "delete",
            "dream",
        ]
    };
    let evaluation_batch = evaluation_batch(&request)?;
    let evaluation_identity = format!(
        "{}\0{}\0{}\0{}\0{}",
        if evaluation_batch.is_some() {
            "simple-workspace-eval-batch-v1"
        } else {
            "simple-workspace-eval-v1"
        },
        request.run_id,
        request.case_id,
        request.authorization_scope,
        request.idempotency_key
    );
    let external_ref = format!(
        "eval-user:{}",
        hex::encode(Sha256::digest(evaluation_identity.as_bytes()))
    );
    let display_name = format!(
        "Straylight evaluation: {}",
        request.case_id.chars().take(160).collect::<String>()
    );
    let token = derive_eval_token(
        &state.config.continuation_secret,
        &external_ref,
        &request.idempotency_key,
    )?;
    let prepared = prepare_bulk_documents(&state, &request.documents).await?;
    let deltas = prepare_bulk_documents(&state, &request.delta_documents).await?;
    let mut tx = state.rw_pool.begin().await?;
    require_local_publish_lock(&mut tx, format!("simple-eval:{external_ref}")).await?;
    set_context(&mut tx, &caller).await?;
    let provisioning_capabilities = vec![
        "open",
        "query",
        "read",
        "status",
        "checkpoint",
        "save",
        "stage",
        "correct",
        "delete",
        "dream",
    ];
    let row = sqlx::query(
        r#"
        SELECT *
        FROM straylight_auth.bootstrap_evaluation_user($1,$2,$3,$4,$5)
        "#,
    )
    .bind(&external_ref)
    .bind(&display_name)
    .bind("Evaluation provisioning")
    .bind(hash_token(&token))
    .bind(&provisioning_capabilities)
    .fetch_one(&mut *tx)
    .await?;
    let user_id: Uuid = row.get("user_id");
    let credential_id: Uuid = row.get("credential_id");
    let scope_id: Uuid = row.get("scope_id");
    let provisioning_auth = AuthContext {
        credential_id: CredentialId(credential_id),
        user_id: UserId(user_id),
        capabilities: provisioning_capabilities
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        scope_refs: vec!["scope:root".to_owned()],
        read_only: false,
    };
    set_context(&mut tx, &provisioning_auth).await?;
    let existing = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM straylight.entries WHERE user_id=$1)",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let batch_index = evaluation_batch.map(|(index, _)| index);
    if existing && batch_index.is_none_or(|index| index == 0) {
        return Err(ApiError::conflict(
            "eval_token_unavailable_on_replay",
            "the evaluation import already committed; use a new run ID",
            json!({"run_id": request.run_id, "case_id": request.case_id}),
        ));
    }
    if !existing && batch_index.is_some_and(|index| index > 0) {
        return Err(ApiError::conflict(
            "eval_batch_out_of_order",
            "evaluation import batches must begin with batch zero",
            json!({"batch_index": batch_index}),
        ));
    }
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await?;
    insert_bulk_documents(&mut tx, user_id, credential_id, &prepared, "create").await?;
    let base_generation = max_generation_in_tx(&mut tx, user_id).await?;
    let checkpoint_id = if let Some(seed) = &request.seed_checkpoint {
        let checkpoint_id = Uuid::now_v7();
        let checkpoint_path = format!(".straylight/checkpoints/{checkpoint_id}.md");
        let checkpoint_content = render_seed_checkpoint(
            checkpoint_id,
            base_generation,
            &seed.state,
            &seed.source_refs,
        );
        let mut checkpoint_document = prepare_one_bulk_document(
            &state,
            checkpoint_path,
            checkpoint_content,
            "text/markdown".to_owned(),
        )
        .await?;
        checkpoint_document.entry_id = checkpoint_id;
        checkpoint_document.metadata = json!({
            "kind": "checkpoint",
            "checkpoint_ref": format!("checkpoint:{checkpoint_id}"),
            "workspace_generation": base_generation,
            "source_entries": seed.source_refs
        });
        insert_bulk_documents(
            &mut tx,
            user_id,
            credential_id,
            &[checkpoint_document],
            "create",
        )
        .await?;
        Some(format!("checkpoint:{checkpoint_id}"))
    } else {
        None
    };
    apply_bulk_deltas(&mut tx, user_id, credential_id, &deltas).await?;
    let generation = max_generation_in_tx(&mut tx, user_id).await?;
    let _ = sqlx::query(
        r#"
        SELECT *
        FROM straylight_auth.bootstrap_evaluation_user($1,$2,$3,$4,$5)
        "#,
    )
    .bind(&external_ref)
    .bind(&display_name)
    .bind(if access_mode == "read_only" {
        "Evaluation read-only"
    } else {
        "Evaluation read/write"
    })
    .bind(hash_token(&token))
    .bind(&capabilities)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    state.workspace_features.invalidate(user_id).await;
    metrics::histogram!("simple.import.feature_declarations").record(
        prepared
            .iter()
            .chain(&deltas)
            .filter(|document| {
                !document.frontmatter.supersedes.is_empty()
                    || document.frontmatter.kind.as_deref() == Some("intention")
            })
            .count() as f64,
    );
    let chunk_count = prepared
        .iter()
        .chain(&deltas)
        .map(|document| document.chunks.len())
        .sum::<usize>();
    let embedded_count = prepared
        .iter()
        .chain(&deltas)
        .flat_map(|document| &document.embeddings)
        .filter(|embedding| embedding.is_some())
        .count();
    let semantic_ready = embedded_count == chunk_count;
    let import_id = format!("simple-import:{user_id}");
    let status_url = format!("/v1/workspace/admin/eval/imports/{import_id}");
    Ok(Json(json!({
        "status": if semantic_ready { "ready" } else { "indexing" },
        "ready_for_evaluation": semantic_ready,
        "import_id": import_id,
        "status_url": status_url,
        "authorization_scope": request.authorization_scope,
        "requested_authorization_scope": request.authorization_scope,
        "display_scope": request.display_scope,
        "access_mode": access_mode,
        "user_id": format!("user:{user_id}"),
        "scope_id": format!("scope:{scope_id}"),
        "credential_id": format!("credential:{credential_id}"),
        "base_corpus_revision": format!("generation:{base_generation}"),
        "corpus_revision": format!("generation:{generation}"),
        "checkpoint_id": checkpoint_id,
        "seed_session_id": null,
        "index_status": {
            "exact": "ready",
            "lexical": "ready",
            "semantic": if semantic_ready { "ready" } else { "pending" }
        },
        "index_counts": {
            "documents": request.documents.len() + request.delta_documents.len(),
            "chunks": chunk_count,
            "embeddings": embedded_count
        },
        "embedding_provider": state.embedder.provider(),
        "embedding_model": state.embedder.model(),
        "embedding_degraded": state.embedder.is_degraded(),
        "documents": request.documents.len(),
        "delta_documents": request.delta_documents.len(),
        "batch_index": evaluation_batch.map(|(index, _)| index),
        "batch_count": evaluation_batch.map(|(_, count)| count),
        "replayed": false,
        "token_status": "issued_once",
        "credential_token": token
    })))
}

pub async fn evaluation_status(
    State(state): State<AppState>,
    Extension(caller): Extension<AuthContext>,
    Path(import_id): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require(Capability::Status)?;
    let user_id = import_id
        .strip_prefix("simple-import:")
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ApiError::invalid("invalid simple evaluation import ID"))?;
    if caller.user_id.0 != user_id {
        return Err(ApiError::public(
            StatusCode::FORBIDDEN,
            "evaluation_scope_mismatch",
            "evaluation status can only be read by its scoped credential",
        ));
    }
    let mut tx = state.begin_read(&caller).await?;
    let row = sqlx::query(
        r#"
        SELECT
          count(*) FILTER (WHERE status IN ('queued','running')) AS pending_jobs,
          count(*) FILTER (WHERE status='failed') AS failed_jobs,
          (SELECT coalesce(max(generation),0)
           FROM straylight.workspace_changes
           WHERE user_id=$1) AS generation
        FROM straylight.jobs
        WHERE user_id=$1 AND kind='embed_entry'
        "#,
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let pending_jobs: i64 = row.get("pending_jobs");
    let failed_jobs: i64 = row.get("failed_jobs");
    let ready = pending_jobs == 0 && failed_jobs == 0;
    Ok(Json(json!({
        "status": if ready {
            "ready"
        } else if failed_jobs > 0 {
            "failed"
        } else {
            "indexing"
        },
        "ready_for_evaluation": ready,
        "import_id": import_id,
        "corpus_revision": format!("generation:{}", row.get::<i64, _>("generation")),
        "index_status": {
            "exact": "ready",
            "lexical": "ready",
            "semantic": if ready { "ready" } else if failed_jobs > 0 { "failed" } else { "pending" }
        },
        "index_counts": {
            "pending_jobs": pending_jobs,
            "failed_jobs": failed_jobs
        }
    })))
}

async fn current_generation(state: &AppState, auth: &AuthContext) -> ApiResult<i64> {
    let mut tx = state.begin_read(auth).await?;
    let generation = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(generation) FROM straylight.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0);
    tx.commit().await?;
    Ok(generation)
}

async fn feature_snapshot(
    state: &AppState,
    auth: &AuthContext,
    generation: i64,
) -> ApiResult<Option<Arc<WorkspaceFeatureSnapshot>>> {
    if !state.config.supersession_demotion && !state.config.intention_ledger {
        return Ok(None);
    }
    if let Some(snapshot) = state
        .workspace_features
        .get(auth.user_id.0, generation)
        .await
    {
        return Ok(Some(snapshot));
    }
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.title,coalesce(version.content,'') AS content
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.deleted_at IS NULL
          AND entry.kind='markdown'
        ORDER BY entry.path
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let documents = rows
        .into_iter()
        .map(|row| WorkspaceFeatureDocument {
            entry_id: row.get("id"),
            path: row.get("path"),
            title: row.get("title"),
            content: row.get("content"),
        })
        .collect();
    Ok(Some(
        state
            .workspace_features
            .put(
                auth.user_id.0,
                WorkspaceFeatureSnapshot::build(generation, documents),
            )
            .await,
    ))
}

async fn search_one(
    state: &AppState,
    auth: &AuthContext,
    query: &str,
    limit: usize,
    requested_modes: &[String],
    features: Option<&WorkspaceFeatureSnapshot>,
) -> ApiResult<(Vec<Candidate>, Vec<&'static str>, RetrievalTimings)> {
    let total_started = Instant::now();
    let mut timings = RetrievalTimings::default();
    if query.trim().is_empty() {
        return Err(ApiError::invalid("search query is required"));
    }
    let modes: HashSet<&str> = requested_modes.iter().map(String::as_str).collect();
    let lexical_enabled = modes.is_empty() || modes.contains("lexical");
    let semantic_enabled = modes.is_empty() || modes.contains("semantic");
    let exact_enabled = modes.is_empty() || modes.contains("exact");
    let exact_paths = if exact_enabled {
        path_hints(query)
    } else {
        vec![]
    };
    let exact_future = async {
        let started = Instant::now();
        if exact_enabled && !exact_paths.is_empty() {
            (
                bounded_retrieval_lane(
                    "exact",
                    exact_candidates(state, auth, &exact_paths, Some(query)),
                )
                .await,
                elapsed_ms(started),
            )
        } else {
            (Ok(vec![]), elapsed_ms(started))
        }
    };
    let lexical_future = async {
        let started = Instant::now();
        if lexical_enabled {
            (
                bounded_retrieval_lane("lexical", lexical_candidates(state, auth, query, features))
                    .await,
                elapsed_ms(started),
            )
        } else {
            (Ok(vec![]), elapsed_ms(started))
        }
    };
    let ((exact, exact_ms), (lexical, lexical_ms)) = tokio::join!(exact_future, lexical_future);
    timings.exact = exact_ms;
    timings.lexical = lexical_ms;
    let mut failures = Vec::new();
    let mut merged: HashMap<Uuid, Candidate> = HashMap::new();
    let merge_started = Instant::now();
    for (lane, result) in [("exact", exact), ("lexical", lexical)] {
        match result {
            Ok(candidates) => {
                for candidate in candidates {
                    merge_candidate(&mut merged, candidate);
                }
            }
            Err(error) => {
                tracing::warn!(lane, ?error, "simple retrieval lane failed");
                failures.push(lane);
            }
        }
    }
    timings.merge += elapsed_ms(merge_started);
    let semantic_forced = modes.len() == 1 && modes.contains("semantic");
    let semantic_ready_started = Instant::now();
    let semantic_ready = if semantic_enabled && !semantic_forced {
        match semantic_search_allowed(state, auth).await {
            Ok(ready) => ready,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "could not inspect semantic index status; using exact and lexical lanes"
                );
                false
            }
        }
    } else {
        semantic_forced
    };
    timings.semantic_ready = elapsed_ms(semantic_ready_started);
    if semantic_enabled && semantic_ready {
        let semantic_started = Instant::now();
        match bounded_semantic_lane(semantic_candidates(state, auth, query, features)).await {
            Ok(result) => {
                timings.embed = result.embed_ms;
                timings.semantic_db = result.database_ms;
                let merge_started = Instant::now();
                for candidate in result.candidates {
                    merge_candidate(&mut merged, candidate);
                }
                timings.merge += elapsed_ms(merge_started);
            }
            Err(error) => {
                tracing::warn!(lane = "semantic", ?error, "simple retrieval lane failed");
                failures.push("semantic");
            }
        }
        timings.semantic = elapsed_ms(semantic_started);
    } else if semantic_enabled {
        failures.push("semantic_unavailable");
    }
    let mut candidates = merged.into_values().collect::<Vec<_>>();
    annotate_candidates(
        &mut candidates,
        features,
        state.config.supersession_demotion,
    );
    sort_candidates(&mut candidates);
    candidates.truncate(limit);
    timings.total = elapsed_ms(total_started);
    Ok((candidates, failures, timings))
}

async fn semantic_search_allowed(state: &AppState, auth: &AuthContext) -> ApiResult<bool> {
    let mut tx = state.begin_read(auth).await?;
    let has_semantic_coverage = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1
          FROM straylight.search_chunks
          WHERE user_id=$1 AND embedding IS NOT NULL
          LIMIT 1
        )
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(has_semantic_coverage)
}

fn sort_candidates(candidates: &mut [Candidate]) {
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });
}

async fn bounded_retrieval_lane<F>(lane: &'static str, future: F) -> ApiResult<Vec<Candidate>>
where
    F: std::future::Future<Output = ApiResult<Vec<Candidate>>>,
{
    let started = Instant::now();
    match tokio::time::timeout(RETRIEVAL_LANE_TIMEOUT, future).await {
        Ok(Ok(candidates)) => {
            metrics::histogram!(
                "simple.retrieval.lane_duration_ms",
                "lane" => lane,
                "result" => "success"
            )
            .record(started.elapsed().as_secs_f64() * 1_000.0);
            metrics::histogram!(
                "simple.retrieval.lane_candidates",
                "lane" => lane
            )
            .record(candidates.len() as f64);
            Ok(candidates)
        }
        Ok(Err(error)) => {
            metrics::histogram!(
                "simple.retrieval.lane_duration_ms",
                "lane" => lane,
                "result" => "failure"
            )
            .record(started.elapsed().as_secs_f64() * 1_000.0);
            metrics::counter!(
                "simple.retrieval.lane_failure",
                "lane" => lane
            )
            .increment(1);
            Err(error)
        }
        Err(_) => {
            metrics::histogram!(
                "simple.retrieval.lane_duration_ms",
                "lane" => lane,
                "result" => "timeout"
            )
            .record(started.elapsed().as_secs_f64() * 1_000.0);
            metrics::counter!("simple.retrieval.lane_timeout", "lane" => lane).increment(1);
            Err(ApiError::public(
                StatusCode::SERVICE_UNAVAILABLE,
                "retrieval_lane_timeout",
                format!("{lane} retrieval exceeded its bounded time budget"),
            ))
        }
    }
}

async fn bounded_semantic_lane<F>(future: F) -> ApiResult<SemanticCandidates>
where
    F: std::future::Future<Output = ApiResult<SemanticCandidates>>,
{
    let started = Instant::now();
    match tokio::time::timeout(RETRIEVAL_LANE_TIMEOUT, future).await {
        Ok(Ok(result)) => {
            metrics::histogram!(
                "simple.retrieval.lane_duration_ms",
                "lane" => "semantic",
                "result" => "success"
            )
            .record(elapsed_ms(started));
            metrics::histogram!(
                "simple.retrieval.lane_candidates",
                "lane" => "semantic"
            )
            .record(result.candidates.len() as f64);
            Ok(result)
        }
        Ok(Err(error)) => {
            metrics::histogram!(
                "simple.retrieval.lane_duration_ms",
                "lane" => "semantic",
                "result" => "failure"
            )
            .record(elapsed_ms(started));
            metrics::counter!(
                "simple.retrieval.lane_failure",
                "lane" => "semantic"
            )
            .increment(1);
            Err(error)
        }
        Err(_) => {
            metrics::histogram!(
                "simple.retrieval.lane_duration_ms",
                "lane" => "semantic",
                "result" => "timeout"
            )
            .record(elapsed_ms(started));
            metrics::counter!(
                "simple.retrieval.lane_timeout",
                "lane" => "semantic"
            )
            .increment(1);
            Err(ApiError::public(
                StatusCode::SERVICE_UNAVAILABLE,
                "retrieval_lane_timeout",
                "semantic retrieval exceeded its bounded time budget",
            ))
        }
    }
}

async fn exact_candidates(
    state: &AppState,
    auth: &AuthContext,
    paths: &[String],
    verbatim_query: Option<&str>,
) -> ApiResult<Vec<Candidate>> {
    let verbatim_terms = if state.config.verbatim_spans {
        verbatim_query.map(verbatim_match_terms).unwrap_or_default()
    } else {
        Vec::new()
    };
    let include_verbatim_source = !verbatim_terms.is_empty();
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.title,entry.current_version,
               version.content_sha256,
               CASE
                 WHEN $3 THEN coalesce(version.content,'')
                 ELSE left(coalesce(version.content,''),2400)
               END AS content
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.deleted_at IS NULL
          AND entry.path=ANY($2)
        ORDER BY entry.path
        LIMIT 32
        "#,
    )
    .bind(auth.user_id.0)
    .bind(paths)
    .bind(include_verbatim_source)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let content = row.get::<String, _>("content");
            let excerpt = truncate_chars(&content, 2_400);
            let version = row.get("current_version");
            let content_sha256 = row.get::<String, _>("content_sha256");
            let verbatim_matches =
                extract_verbatim_matches(&content, &verbatim_terms, version, &content_sha256);
            Candidate {
                entry_id: row.get("id"),
                path: row.get("path"),
                title: row.get("title"),
                version,
                content_sha256,
                heading: String::new(),
                excerpt: excerpt.clone(),
                score: 10.0,
                lanes: vec!["exact".to_owned()],
                sections: vec![CandidateSection {
                    heading: String::new(),
                    excerpt,
                    score: 10.0,
                }],
                verbatim_matches,
                superseded_by: None,
            }
        })
        .collect())
}

async fn lexical_candidates(
    state: &AppState,
    auth: &AuthContext,
    query: &str,
    features: Option<&WorkspaceFeatureSnapshot>,
) -> ApiResult<Vec<Candidate>> {
    let mut tx = state.begin_read(auth).await?;
    let anchors = search_anchors(query);
    let mut candidates = Vec::new();
    let mut anchor_hit = false;
    for anchor in &anchors {
        let anchor_candidates = fetch_lexical_candidates(
            &mut tx,
            anchor,
            query,
            features,
            state.config.supersession_demotion,
            state.config.supersession_demotion_weight,
        )
        .await?;
        anchor_hit |= !anchor_candidates.is_empty();
        candidates.extend(anchor_candidates);
    }
    if !anchor_hit {
        let focused = focused_lexical_query(query).unwrap_or_else(|| query.trim().to_owned());
        candidates.extend(
            fetch_lexical_candidates(
                &mut tx,
                &focused,
                query,
                features,
                state.config.supersession_demotion,
                state.config.supersession_demotion_weight,
            )
            .await?,
        );
    }
    tx.commit().await?;
    let mut merged = HashMap::new();
    for candidate in candidates {
        merge_candidate(&mut merged, candidate);
    }
    Ok(merged.into_values().collect())
}

async fn fetch_lexical_candidates(
    tx: &mut Transaction<'_, Postgres>,
    retrieval_query: &str,
    scoring_query: &str,
    features: Option<&WorkspaceFeatureSnapshot>,
    supersession_enabled: bool,
    supersession_weight: f64,
) -> ApiResult<Vec<Candidate>> {
    let rows = sqlx::query(
        r#"
        SELECT *
        FROM straylight.workspace_lexical_candidates($1)
        "#,
    )
    .bind(retrieval_query)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let path: String = row.get("path");
            let title: String = row.get("title");
            let heading: String = row.get("heading");
            let excerpt = truncate_chars(&row.get::<String, _>("content"), 2_400);
            let score = 3.0
                + row.get::<f64, _>("score")
                + lexical_candidate_bonus(scoring_query, &path, &title, &heading, &excerpt)
                - derived_penalty(&path)
                - supersession_penalty(&path, features, supersession_enabled, supersession_weight);
            Candidate {
                entry_id: row.get("entry_id"),
                score,
                path,
                title,
                version: row.get("current_version"),
                content_sha256: row.get("content_sha256"),
                heading: heading.clone(),
                excerpt: excerpt.clone(),
                lanes: vec!["lexical".to_owned()],
                sections: vec![CandidateSection {
                    heading,
                    excerpt,
                    score,
                }],
                verbatim_matches: vec![],
                superseded_by: None,
            }
        })
        .collect())
}

async fn semantic_candidates(
    state: &AppState,
    auth: &AuthContext,
    query: &str,
    features: Option<&WorkspaceFeatureSnapshot>,
) -> ApiResult<SemanticCandidates> {
    let embed_started = Instant::now();
    let vectors = state.embedder.embed(&[query.to_owned()]).await?;
    let embed_ms = elapsed_ms(embed_started);
    let vector = vectors.into_iter().next().ok_or_else(|| {
        ApiError::Internal("embedding provider returned no query vector".to_owned())
    })?;
    let database_started = Instant::now();
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT *
        FROM straylight.workspace_semantic_candidates($1)
        "#,
    )
    .bind(Vector::from(vector))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let database_ms = elapsed_ms(database_started);
    let candidates = rows
        .into_iter()
        .map(|row| {
            let distance = row.get::<f64, _>("distance");
            let path: String = row.get("path");
            let title: String = row.get("title");
            let heading: String = row.get("heading");
            let excerpt = truncate_chars(&row.get::<String, _>("content"), 2_400);
            let score = 2.0
                + (1.0 - distance).max(0.0)
                + lexical_candidate_bonus(query, &path, &title, &heading, &excerpt)
                - derived_penalty(&path)
                - supersession_penalty(
                    &path,
                    features,
                    state.config.supersession_demotion,
                    state.config.supersession_demotion_weight,
                );
            Candidate {
                entry_id: row.get("entry_id"),
                score,
                path,
                title,
                version: row.get("current_version"),
                content_sha256: row.get("content_sha256"),
                heading: heading.clone(),
                excerpt: excerpt.clone(),
                lanes: vec!["semantic".to_owned()],
                sections: vec![CandidateSection {
                    heading,
                    excerpt,
                    score,
                }],
                verbatim_matches: vec![],
                superseded_by: None,
            }
        })
        .collect();
    Ok(SemanticCandidates {
        candidates,
        embed_ms,
        database_ms,
    })
}

fn derived_penalty(path: &str) -> f64 {
    if path.starts_with(".straylight/proposals/") {
        2.0
    } else if path.starts_with(".straylight/derived/") || path.starts_with(".straylight/dreams/") {
        1.0
    } else {
        0.0
    }
}

fn supersession_penalty(
    path: &str,
    features: Option<&WorkspaceFeatureSnapshot>,
    enabled: bool,
    weight: f64,
) -> f64 {
    if enabled && features.is_some_and(|snapshot| snapshot.superseded_by(path).is_some()) {
        weight
    } else {
        0.0
    }
}

fn annotate_candidates(
    candidates: &mut [Candidate],
    features: Option<&WorkspaceFeatureSnapshot>,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let Some(features) = features else {
        return;
    };
    for candidate in candidates {
        candidate.superseded_by = features.superseded_by(&candidate.path);
    }
}

fn merge_candidate(merged: &mut HashMap<Uuid, Candidate>, mut candidate: Candidate) {
    normalize_candidate_sections(&mut candidate);
    if let Some(existing) = merged.get_mut(&candidate.entry_id) {
        let candidate_score = candidate.score;
        let adds_lane = candidate
            .lanes
            .iter()
            .any(|lane| !existing.lanes.contains(lane));
        existing.sections.extend(candidate.sections);
        normalize_candidate_sections(existing);
        existing.score = existing.score.max(candidate_score);
        if adds_lane {
            existing.score += 0.15;
        }
        for lane in candidate.lanes {
            if !existing.lanes.contains(&lane) {
                existing.lanes.push(lane);
            }
        }
        existing.verbatim_matches.extend(candidate.verbatim_matches);
        existing
            .verbatim_matches
            .sort_by_key(|source_match| source_match.line_no);
        existing.verbatim_matches.dedup_by(|left, right| {
            left.line_no == right.line_no
                && left.byte_start == right.byte_start
                && left.byte_end == right.byte_end
        });
        existing
            .verbatim_matches
            .truncate(MAX_VERBATIM_MATCHES_PER_CANDIDATE);
    } else {
        merged.insert(candidate.entry_id, candidate);
    }
}

fn normalize_candidate_sections(candidate: &mut Candidate) {
    candidate.sections.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.heading.cmp(&right.heading))
            .then_with(|| left.excerpt.cmp(&right.excerpt))
    });
    let mut seen = HashSet::new();
    candidate
        .sections
        .retain(|section| seen.insert((section.heading.clone(), section.excerpt.clone())));
    candidate.sections.truncate(3);
    if let Some(section) = candidate.sections.first() {
        candidate.heading.clone_from(&section.heading);
        candidate.excerpt.clone_from(&section.excerpt);
    }
}

fn select_search_candidate_sets(
    candidate_sets: &[Vec<Candidate>],
    fair_share: bool,
    max_candidates: usize,
) -> (Vec<Vec<Candidate>>, bool) {
    let available = candidate_sets.iter().map(Vec::len).sum::<usize>();
    let mut selected = vec![Vec::new(); candidate_sets.len()];
    if fair_share {
        let max_rank = candidate_sets.iter().map(Vec::len).max().unwrap_or(0);
        let mut selected_count = 0_usize;
        'ranks: for rank in 0..max_rank {
            for (query_index, candidates) in candidate_sets.iter().enumerate() {
                if selected_count == max_candidates {
                    break 'ranks;
                }
                if let Some(candidate) = candidates.get(rank) {
                    selected[query_index].push(candidate.clone());
                    selected_count += 1;
                }
            }
        }
    } else {
        let mut remaining = max_candidates;
        for (query_index, candidates) in candidate_sets.iter().enumerate() {
            let retain = candidates.len().min(remaining);
            selected[query_index].extend(candidates.iter().take(retain).cloned());
            remaining = remaining.saturating_sub(retain);
        }
    }
    let retained = selected.iter().map(Vec::len).sum::<usize>();
    (selected, retained < available)
}

async fn fetch_search_top1_hydration(
    state: &AppState,
    auth: &AuthContext,
    candidate_sets: &[Vec<Candidate>],
) -> ApiResult<HashMap<Uuid, SearchHydration>> {
    let mut seen = HashSet::new();
    let ids = candidate_sets
        .iter()
        .filter_map(|candidates| candidates.first())
        .map(|candidate| candidate.entry_id)
        .filter(|entry_id| seen.insert(*entry_id))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT entry.id,version.size_bytes,
               CASE WHEN version.size_bytes <= $3 THEN version.content ELSE NULL END AS content
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.id=ANY($2)
          AND entry.deleted_at IS NULL
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&ids)
    .bind(i64::try_from(MAX_OPEN_COMPLETE_SOURCE_CHARS).unwrap_or(i64::MAX))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let size_bytes = usize::try_from(row.get::<i64, _>("size_bytes")).ok()?;
            let content = row.get::<Option<String>, _>("content")?;
            Some((
                row.get::<Uuid, _>("id"),
                SearchHydration {
                    size_bytes,
                    content,
                },
            ))
        })
        .collect())
}

fn assemble_search_candidate_views(
    candidate_sets: Vec<Vec<Candidate>>,
    hydration: &HashMap<Uuid, SearchHydration>,
    options: SearchBudgetOptions,
) -> (Vec<Vec<SearchCandidateView>>, bool) {
    let mut views = candidate_sets
        .into_iter()
        .map(|candidates| {
            candidates
                .into_iter()
                .enumerate()
                .map(|(rank, candidate)| SearchCandidateView {
                    candidate,
                    representation: SearchRepresentation::Excerpt,
                    complete_text: None,
                    demote_additional_sections: options
                        .section_demotion_top_n
                        .is_some_and(|top_n| rank >= top_n),
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut remaining_chars = options.max_chars;
    let mut truncated = false;

    if options.top1_hydration {
        for query_views in &mut views {
            let Some(top) = query_views.first_mut() else {
                continue;
            };
            let Some(loaded) = hydration.get(&top.candidate.entry_id) else {
                continue;
            };
            let content_chars = loaded.content.chars().count();
            if loaded.size_bytes > MAX_OPEN_COMPLETE_SOURCE_CHARS
                || loaded.size_bytes > remaining_chars
                || content_chars > remaining_chars
            {
                continue;
            }
            top.representation = SearchRepresentation::CompleteSource;
            top.complete_text = Some(loaded.content.clone());
            remaining_chars = remaining_chars.saturating_sub(content_chars);
            for view in query_views.iter_mut().skip(1) {
                if desired_search_evidence_chars(view) > 0 {
                    truncated = true;
                }
                view.representation = SearchRepresentation::PointerLead;
            }
        }
    }

    if options.fair_share {
        let quotas = fair_search_char_quotas(&views, remaining_chars);
        for (query_views, quota) in views.iter_mut().zip(quotas) {
            let mut query_remaining = quota;
            for view in query_views {
                let (used, view_truncated) = apply_search_view_budget(view, query_remaining);
                query_remaining = query_remaining.saturating_sub(used);
                truncated |= view_truncated;
            }
        }
    } else {
        for query_views in &mut views {
            for view in query_views {
                let (used, view_truncated) = apply_search_view_budget(view, remaining_chars);
                remaining_chars = remaining_chars.saturating_sub(used);
                truncated |= view_truncated;
            }
        }
    }

    (views, truncated)
}

fn fair_search_char_quotas(views: &[Vec<SearchCandidateView>], max_chars: usize) -> Vec<usize> {
    if views.is_empty() {
        return Vec::new();
    }
    let desired = views
        .iter()
        .map(|query_views| {
            query_views
                .iter()
                .map(desired_search_evidence_chars)
                .sum::<usize>()
        })
        .collect::<Vec<_>>();
    let floor = max_chars / views.len();
    let mut quotas = desired
        .iter()
        .map(|desired_chars| (*desired_chars).min(floor))
        .collect::<Vec<_>>();
    let mut remaining = max_chars.saturating_sub(quotas.iter().sum());
    while remaining > 0 {
        let mut progressed = false;
        for (query_index, desired_chars) in desired.iter().enumerate() {
            let unmet = desired_chars.saturating_sub(quotas[query_index]);
            let granted = unmet.min(2_400).min(remaining);
            if granted > 0 {
                quotas[query_index] += granted;
                remaining -= granted;
                progressed = true;
            }
            if remaining == 0 {
                break;
            }
        }
        if !progressed {
            break;
        }
    }
    quotas
}

fn desired_search_evidence_chars(view: &SearchCandidateView) -> usize {
    if view.representation != SearchRepresentation::Excerpt {
        return 0;
    }
    let primary = view.candidate.excerpt.chars().count();
    if view.demote_additional_sections {
        primary
    } else {
        primary.saturating_add(
            view.candidate
                .sections
                .iter()
                .skip(1)
                .map(|section| section.excerpt.chars().count())
                .sum::<usize>(),
        )
    }
}

fn apply_search_view_budget(
    view: &mut SearchCandidateView,
    available_chars: usize,
) -> (usize, bool) {
    if view.representation != SearchRepresentation::Excerpt {
        return (0, false);
    }
    let original_chars = desired_search_evidence_chars(view);
    let mut remaining = available_chars.min(original_chars);
    let starting = remaining;
    let truncated = if view.demote_additional_sections {
        let original_primary = view.candidate.excerpt.chars().count();
        let retained = truncate_chars(&view.candidate.excerpt, remaining.min(2_400));
        remaining = remaining.saturating_sub(retained.chars().count());
        let truncated = retained.chars().count() < original_primary
            || view
                .candidate
                .sections
                .iter()
                .skip(1)
                .any(|section| !section.excerpt.is_empty());
        view.candidate.excerpt = retained;
        if let Some(primary) = view.candidate.sections.first_mut() {
            primary.excerpt.clone_from(&view.candidate.excerpt);
        }
        truncated
    } else {
        truncate_candidate_evidence(&mut view.candidate, &mut remaining)
    };
    if view.candidate.excerpt.is_empty() {
        view.representation = SearchRepresentation::PointerLead;
    }
    (starting.saturating_sub(remaining), truncated)
}

fn render_budgeted_search_candidate(
    view: &SearchCandidateView,
    remaining_verbatim_chars: &mut usize,
) -> Value {
    let candidate = &view.candidate;
    let representation = match view.representation {
        SearchRepresentation::CompleteSource => "complete_source",
        SearchRepresentation::Excerpt => "excerpt",
        SearchRepresentation::PointerLead => "pointer_lead",
    };
    let mut rendered = serde_json::Map::from_iter([
        (
            "reference".to_owned(),
            Value::String(format!("entry:{}", candidate.entry_id)),
        ),
        ("path".to_owned(), Value::String(candidate.path.clone())),
        ("title".to_owned(), Value::String(candidate.title.clone())),
        ("version".to_owned(), json!(candidate.version)),
        (
            "representation".to_owned(),
            Value::String(representation.to_owned()),
        ),
    ]);
    if !candidate.heading.is_empty() {
        rendered.insert(
            "heading".to_owned(),
            Value::String(candidate.heading.clone()),
        );
    }
    match view.representation {
        SearchRepresentation::CompleteSource => {
            rendered.insert(
                "content_hash".to_owned(),
                Value::String(format!("sha256:{}", candidate.content_sha256)),
            );
            rendered.insert(
                "text".to_owned(),
                Value::String(view.complete_text.clone().unwrap_or_default()),
            );
        }
        SearchRepresentation::PointerLead => {
            rendered.insert(
                "content_hash".to_owned(),
                Value::String(format!("sha256:{}", candidate.content_sha256)),
            );
            rendered.insert("score".to_owned(), json!(candidate.score));
        }
        SearchRepresentation::Excerpt => {
            rendered.insert(
                "excerpt".to_owned(),
                Value::String(candidate.excerpt.clone()),
            );
            let additional_sections = candidate
                .sections
                .iter()
                .skip(1)
                .filter_map(|section| {
                    if view.demote_additional_sections {
                        if section.heading.is_empty() {
                            return None;
                        }
                        Some(json!({
                            "representation": "heading_lead",
                            "heading": section.heading,
                            "path": candidate.path,
                            "version": candidate.version
                        }))
                    } else if section.excerpt.is_empty() {
                        None
                    } else {
                        let mut rendered = serde_json::Map::from_iter([(
                            "excerpt".to_owned(),
                            Value::String(section.excerpt.clone()),
                        )]);
                        if !section.heading.is_empty() {
                            rendered.insert(
                                "heading".to_owned(),
                                Value::String(section.heading.clone()),
                            );
                        }
                        Some(Value::Object(rendered))
                    }
                })
                .collect::<Vec<_>>();
            if !additional_sections.is_empty() {
                rendered.insert(
                    "additional_sections".to_owned(),
                    Value::Array(additional_sections),
                );
            }
        }
    }
    let verbatim_matches = render_verbatim_matches(candidate, remaining_verbatim_chars);
    if !verbatim_matches.is_empty() {
        rendered.insert(
            "verbatim_matches".to_owned(),
            Value::Array(verbatim_matches),
        );
    }
    if let Some(superseded_by) = &candidate.superseded_by {
        rendered.insert("superseded_by".to_owned(), json!(superseded_by));
    }
    Value::Object(rendered)
}

fn verbatim_match_terms(query: &str) -> Vec<String> {
    search_anchors(query)
        .into_iter()
        .filter(|term| !looks_like_path(term))
        .take(MAX_VERBATIM_MATCHES_PER_CANDIDATE)
        .collect()
}

fn extract_verbatim_matches(
    content: &str,
    terms: &[String],
    version: i64,
    content_sha256: &str,
) -> Vec<VerbatimMatch> {
    if terms.is_empty() {
        return vec![];
    }
    let mut byte_start = 0_usize;
    let mut matches = Vec::new();
    for (index, source_line) in content.split_inclusive('\n').enumerate() {
        let line = source_line.strip_suffix('\n').unwrap_or(source_line);
        if terms.iter().any(|term| line.contains(term)) {
            let text = truncate_chars(line, MAX_VERBATIM_LINE_CHARS);
            let truncated = text.len() < line.len();
            matches.push(VerbatimMatch {
                line_no: index + 1,
                byte_start,
                byte_end: byte_start + text.len(),
                text,
                version,
                content_hash: format!("sha256:{content_sha256}"),
                truncated,
            });
            if matches.len() >= MAX_VERBATIM_MATCHES_PER_CANDIDATE {
                break;
            }
        }
        byte_start += source_line.len();
    }
    matches
}

fn render_verbatim_matches(candidate: &Candidate, remaining_chars: &mut usize) -> Vec<Value> {
    let mut rendered = Vec::new();
    for source_match in &candidate.verbatim_matches {
        if *remaining_chars == 0 {
            break;
        }
        let mut retained = source_match.clone();
        retained.text = truncate_chars(
            &source_match.text,
            (*remaining_chars).min(MAX_VERBATIM_LINE_CHARS),
        );
        let retained_chars = retained.text.chars().count();
        if retained_chars == 0 {
            break;
        }
        *remaining_chars = remaining_chars.saturating_sub(retained_chars);
        retained.byte_end = retained.byte_start + retained.text.len();
        retained.truncated |= retained.text.len() < source_match.text.len();
        rendered.push(json!(retained));
    }
    rendered
}

fn render_search_candidate(candidate: &Candidate, remaining_verbatim_chars: &mut usize) -> Value {
    let mut rendered = serde_json::Map::from_iter([
        (
            "reference".to_owned(),
            Value::String(format!("entry:{}", candidate.entry_id)),
        ),
        ("path".to_owned(), Value::String(candidate.path.clone())),
        ("title".to_owned(), Value::String(candidate.title.clone())),
        ("version".to_owned(), json!(candidate.version)),
        (
            "excerpt".to_owned(),
            Value::String(candidate.excerpt.clone()),
        ),
    ]);
    if !candidate.heading.is_empty() {
        rendered.insert(
            "heading".to_owned(),
            Value::String(candidate.heading.clone()),
        );
    }
    let additional_sections = candidate
        .sections
        .iter()
        .skip(1)
        .map(|section| {
            let mut rendered = serde_json::Map::from_iter([(
                "excerpt".to_owned(),
                Value::String(section.excerpt.clone()),
            )]);
            if !section.heading.is_empty() {
                rendered.insert("heading".to_owned(), Value::String(section.heading.clone()));
            }
            Value::Object(rendered)
        })
        .collect::<Vec<_>>();
    if !additional_sections.is_empty() {
        rendered.insert(
            "additional_sections".to_owned(),
            Value::Array(additional_sections),
        );
    }
    let verbatim_matches = render_verbatim_matches(candidate, remaining_verbatim_chars);
    if !verbatim_matches.is_empty() {
        rendered.insert(
            "verbatim_matches".to_owned(),
            Value::Array(verbatim_matches),
        );
    }
    if let Some(superseded_by) = &candidate.superseded_by {
        rendered.insert("superseded_by".to_owned(), json!(superseded_by));
    }
    Value::Object(rendered)
}

fn render_evidence_lead(candidate: &Candidate) -> Value {
    let mut rendered = serde_json::Map::from_iter([
        (
            "reference".to_owned(),
            Value::String(format!("entry:{}", candidate.entry_id)),
        ),
        ("path".to_owned(), Value::String(candidate.path.clone())),
        ("title".to_owned(), Value::String(candidate.title.clone())),
        ("version".to_owned(), json!(candidate.version)),
    ]);
    if !candidate.heading.is_empty() {
        rendered.insert(
            "heading".to_owned(),
            Value::String(candidate.heading.clone()),
        );
    }
    if let Some(superseded_by) = &candidate.superseded_by {
        rendered.insert("superseded_by".to_owned(), json!(superseded_by));
    }
    Value::Object(rendered)
}

fn apply_checkpoint_budget(checkpoint: &mut Option<Value>, token_budget: usize) -> (bool, usize) {
    let total_chars = token_budget.saturating_mul(4);
    let Some(checkpoint) = checkpoint.as_mut() else {
        return (false, token_budget);
    };
    let Some(text) = checkpoint.get("text").and_then(Value::as_str) else {
        return (false, token_budget);
    };
    let text_chars = text.chars().count();
    let retained_chars = text_chars.min(total_chars);
    let truncated = text_chars > retained_chars;
    if truncated {
        checkpoint["text"] = Value::String(truncate_chars(text, retained_chars));
        checkpoint["text_truncated"] = Value::Bool(true);
    }
    (truncated, total_chars.saturating_sub(retained_chars) / 4)
}

async fn open_hint_candidates(
    state: &AppState,
    auth: &AuthContext,
    hints: &OpenHints,
) -> ApiResult<(Vec<Candidate>, Vec<Value>)> {
    let mut requested = Vec::new();
    let mut seen = HashSet::new();
    for hint in hints.root_refs.iter().chain(&hints.open_object_refs) {
        let normalized = hint.trim();
        if !normalized.is_empty() && seen.insert(normalized.to_lowercase()) {
            requested.push(normalized.to_owned());
        }
    }
    if requested.len() > 32 {
        return Err(ApiError::invalid(
            "open hints are limited to 32 exact paths or entry references",
        ));
    }
    let mut results =
        futures::stream::iter(requested.into_iter().enumerate().map(|(index, hint)| {
            let state = state.clone();
            let auth = auth.clone();
            async move {
                let result = if hint.starts_with("entry:")
                    || hint.starts_with("checkpoint:")
                    || Uuid::parse_str(&hint).is_ok()
                {
                    resolve_entry_summary(&state, &auth, None, Some(&hint)).await
                } else if validate_path(&hint).is_ok() {
                    resolve_entry_summary(&state, &auth, Some(&hint), None).await
                } else {
                    Err(ApiError::invalid(
                        "open hints must be exact paths, entry refs, or checkpoint refs",
                    ))
                };
                (index, hint, result)
            }
        }))
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    results.sort_by_key(|(index, _, _)| *index);
    let mut candidates = Vec::new();
    let mut gaps = Vec::new();
    for (_, hint, result) in results {
        match result {
            Ok(entry) => candidates.push(Candidate {
                entry_id: entry.id,
                path: entry.path,
                title: entry.title,
                version: entry.version,
                content_sha256: entry.content_sha256,
                heading: String::new(),
                excerpt: String::new(),
                score: 40.0,
                lanes: vec!["hint".to_owned()],
                sections: vec![],
                verbatim_matches: vec![],
                superseded_by: None,
            }),
            Err(ApiError::Public {
                status: StatusCode::NOT_FOUND,
                ..
            }) => gaps.push(json!({
                "kind": "open_hint_not_found",
                "hint": hint,
                "message": "the exact hinted source was not found; other evidence was retained"
            })),
            Err(ApiError::Public { code, message, .. }) => gaps.push(json!({
                "kind": "open_hint_invalid",
                "hint": hint,
                "code": code,
                "message": message
            })),
            Err(error) => return Err(error),
        }
    }
    Ok((candidates, gaps))
}

async fn hydrate_candidates(
    state: &AppState,
    auth: &AuthContext,
    candidates: &[Candidate],
    token_budget: usize,
) -> ApiResult<Vec<Value>> {
    let ids = candidates
        .iter()
        .take(HYDRATED_DOCUMENT_LIMIT)
        .map(|candidate| candidate.entry_id)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT entry.id,version.size_bytes
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.id=ANY($2)
          AND entry.deleted_at IS NULL
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await?;
    let sizes = rows
        .into_iter()
        .filter_map(|row| {
            usize::try_from(row.get::<i64, _>("size_bytes"))
                .ok()
                .map(|size| (row.get::<Uuid, _>("id"), size))
        })
        .collect::<HashMap<_, _>>();
    let mut remaining_chars = token_budget.saturating_mul(4);
    let mut selections = Vec::new();
    let mut complete_ids = Vec::new();
    for (index, candidate) in candidates.iter().take(HYDRATED_DOCUMENT_LIMIT).enumerate() {
        if let Some(annotation) = &candidate.superseded_by {
            let annotation_chars = serde_json::to_string(annotation)
                .map(|value| value.chars().count())
                .unwrap_or(0);
            remaining_chars = remaining_chars.saturating_sub(annotation_chars);
        }
        if remaining_chars == 0 {
            break;
        }
        let Some(size_bytes) = sizes.get(&candidate.entry_id) else {
            continue;
        };
        if *size_bytes <= remaining_chars && *size_bytes <= MAX_OPEN_COMPLETE_SOURCE_CHARS {
            remaining_chars -= *size_bytes;
            complete_ids.push(candidate.entry_id);
            selections.push((index, true, 0_usize));
        } else {
            let excerpt_chars = remaining_chars.min(2_400);
            remaining_chars -= excerpt_chars;
            selections.push((index, false, excerpt_chars));
        }
    }
    let complete_rows = if complete_ids.is_empty() {
        vec![]
    } else {
        sqlx::query(
            r#"
            SELECT entry.id,version.content
            FROM straylight.entries AS entry
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            WHERE entry.user_id=$1
              AND entry.id=ANY($2)
              AND entry.deleted_at IS NULL
            "#,
        )
        .bind(auth.user_id.0)
        .bind(&complete_ids)
        .fetch_all(&mut *tx)
        .await?
    };
    tx.commit().await?;
    let complete_content = complete_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<Uuid, _>("id"),
                row.get::<Option<String>, _>("content").unwrap_or_default(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut evidence = Vec::new();
    for (index, complete, excerpt_chars) in selections {
        let candidate = &candidates[index];
        let text = if complete {
            complete_content
                .get(&candidate.entry_id)
                .cloned()
                .unwrap_or_default()
        } else {
            candidate_excerpt(candidate, excerpt_chars)
        };
        if text.is_empty() {
            continue;
        }
        let mut item = serde_json::Map::from_iter([
            (
                "reference".to_owned(),
                Value::String(format!("entry:{}", candidate.entry_id)),
            ),
            ("path".to_owned(), Value::String(candidate.path.clone())),
            ("title".to_owned(), Value::String(candidate.title.clone())),
            ("version".to_owned(), json!(candidate.version)),
            (
                "representation".to_owned(),
                Value::String(
                    if complete {
                        "complete_source"
                    } else {
                        "source_excerpt"
                    }
                    .to_owned(),
                ),
            ),
            ("text".to_owned(), Value::String(text)),
        ]);
        if !candidate.heading.is_empty() {
            item.insert(
                "heading".to_owned(),
                Value::String(candidate.heading.clone()),
            );
        }
        if let Some(superseded_by) = &candidate.superseded_by {
            item.insert("superseded_by".to_owned(), json!(superseded_by));
        }
        evidence.push(Value::Object(item));
    }
    Ok(evidence)
}

fn truncate_candidate_evidence(candidate: &mut Candidate, remaining_chars: &mut usize) -> bool {
    if let Some(annotation) = &candidate.superseded_by {
        let annotation_chars = serde_json::to_string(annotation)
            .map(|value| value.chars().count())
            .unwrap_or(0);
        *remaining_chars = remaining_chars.saturating_sub(annotation_chars);
    }
    let original_excerpt_chars = candidate.excerpt.chars().count();
    let retained_excerpt = truncate_chars(&candidate.excerpt, (*remaining_chars).min(2_400));
    *remaining_chars = remaining_chars.saturating_sub(retained_excerpt.chars().count());
    let mut truncated = retained_excerpt.chars().count() < original_excerpt_chars;
    candidate.excerpt = retained_excerpt;
    if let Some(primary) = candidate.sections.first_mut() {
        primary.excerpt = candidate.excerpt.clone();
    }
    for section in candidate.sections.iter_mut().skip(1) {
        let original_chars = section.excerpt.chars().count();
        let retained = truncate_chars(&section.excerpt, (*remaining_chars).min(2_400));
        *remaining_chars = remaining_chars.saturating_sub(retained.chars().count());
        truncated |= retained.chars().count() < original_chars;
        section.excerpt = retained;
    }
    let mut index = 0_usize;
    candidate.sections.retain(|section| {
        let retain = index == 0 || !section.excerpt.is_empty();
        index += 1;
        retain
    });
    truncated
}

fn candidate_excerpt(candidate: &Candidate, max_chars: usize) -> String {
    let combined = candidate
        .sections
        .iter()
        .map(|section| section.excerpt.as_str())
        .filter(|excerpt| !excerpt.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    truncate_chars(
        if combined.is_empty() {
            &candidate.excerpt
        } else {
            &combined
        },
        max_chars,
    )
}

async fn resolve_entry(
    state: &AppState,
    auth: &AuthContext,
    path: Option<&str>,
    reference: Option<&str>,
) -> ApiResult<EntryRow> {
    resolve_entry_version(state, auth, path, reference, None).await
}

async fn fetch_entry_lookup(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    requested_version: Option<i64>,
    path: Option<&str>,
    entry_id: Option<Uuid>,
    normalized_path: bool,
    include_content: bool,
) -> Result<Option<PgRow>, sqlx::Error> {
    let mut statement = QueryBuilder::<Postgres>::new(
        r#"
        SELECT entry.id,entry.path,entry.title,entry.kind,entry.media_type,
               entry.current_version,entry.updated_at,
               version.id AS version_id,version.content_sha256,
        "#,
    );
    if include_content {
        statement.push("version.content,");
    } else {
        statement.push("NULL::text AS content,");
    }
    statement.push(
        r#"
               version.object_key,version.object_version_id,version.size_bytes,
               version.metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
        WHERE entry.user_id=
        "#,
    );
    statement.push_bind(user_id);
    if let Some(version) = requested_version {
        statement.push(" AND version.version=").push_bind(version);
    } else {
        statement.push(" AND version.version=entry.current_version AND entry.deleted_at IS NULL");
    }
    if let Some(path) = path {
        if normalized_path {
            statement
                .push(" AND lower(normalize(entry.path, NFC))=")
                .push_bind(portable_path_key(path));
        } else {
            statement
                .push(" AND entry.path=")
                .push_bind(path.to_owned());
        }
    } else {
        statement
            .push(" AND entry.id=")
            .push_bind(entry_id.expect("validated exact entry reference"));
    }
    statement.push(" LIMIT 1");
    statement.build().fetch_optional(&mut **transaction).await
}

async fn resolve_entry_version(
    state: &AppState,
    auth: &AuthContext,
    path: Option<&str>,
    reference: Option<&str>,
    requested_version: Option<i64>,
) -> ApiResult<EntryRow> {
    if requested_version.is_some_and(|version| version <= 0) {
        return Err(ApiError::invalid("entry version must be positive"));
    }
    let checkpoint_path = reference
        .and_then(|value| value.strip_prefix("checkpoint:"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(|checkpoint_id| format!(".straylight/checkpoints/{checkpoint_id}.md"));
    let effective_path = path.or(checkpoint_path.as_deref());
    let entry_id = reference
        .and_then(|value| value.strip_prefix("entry:").or(Some(value)))
        .and_then(|value| Uuid::parse_str(value).ok());
    if effective_path.is_none() && entry_id.is_none() {
        return Err(ApiError::invalid(
            "read requires an exact path or entry ref",
        ));
    }
    let mut tx = state.begin_read(auth).await?;
    let row = fetch_entry_lookup(
        &mut tx,
        auth.user_id.0,
        requested_version,
        effective_path,
        entry_id,
        false,
        true,
    )
    .await?;
    let row =
        if row.is_none() && effective_path.is_some_and(|path| !path.starts_with(".straylight/")) {
            fetch_entry_lookup(
                &mut tx,
                auth.user_id.0,
                requested_version,
                effective_path,
                entry_id,
                true,
                true,
            )
            .await?
        } else {
            row
        };
    let row = row.ok_or_else(|| {
        ApiError::not_found("entry_not_found", path.or(reference).unwrap_or("entry"))
    })?;
    tx.commit().await?;
    Ok(EntryRow {
        id: row.get("id"),
        path: row.get("path"),
        title: row.get("title"),
        kind: row.get("kind"),
        media_type: row.get("media_type"),
        version: requested_version.unwrap_or_else(|| row.get("current_version")),
        version_id: row.get("version_id"),
        content_sha256: row.get("content_sha256"),
        content: row.get("content"),
        object_key: row.get("object_key"),
        object_version_id: row.get("object_version_id"),
        size_bytes: row.get("size_bytes"),
        metadata: row.get("metadata"),
        updated_at: row.get("updated_at"),
    })
}

async fn resolve_entry_summary(
    state: &AppState,
    auth: &AuthContext,
    path: Option<&str>,
    reference: Option<&str>,
) -> ApiResult<EntryRow> {
    let checkpoint_path = reference
        .and_then(|value| value.strip_prefix("checkpoint:"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(|checkpoint_id| format!(".straylight/checkpoints/{checkpoint_id}.md"));
    let effective_path = path.or(checkpoint_path.as_deref());
    let entry_id = reference
        .and_then(|value| value.strip_prefix("entry:").or(Some(value)))
        .and_then(|value| Uuid::parse_str(value).ok());
    if effective_path.is_none() && entry_id.is_none() {
        return Err(ApiError::invalid(
            "checkpoint sources require an exact path or entry ref",
        ));
    }
    let mut tx = state.begin_read(auth).await?;
    let row = fetch_entry_lookup(
        &mut tx,
        auth.user_id.0,
        None,
        effective_path,
        entry_id,
        false,
        false,
    )
    .await?;
    let row =
        if row.is_none() && effective_path.is_some_and(|path| !path.starts_with(".straylight/")) {
            fetch_entry_lookup(
                &mut tx,
                auth.user_id.0,
                None,
                effective_path,
                entry_id,
                true,
                false,
            )
            .await?
        } else {
            row
        };
    let row = row.ok_or_else(|| {
        ApiError::not_found(
            "entry_not_found",
            effective_path.or(reference).unwrap_or("checkpoint source"),
        )
    })?;
    tx.commit().await?;
    Ok(EntryRow {
        id: row.get("id"),
        path: row.get("path"),
        title: row.get("title"),
        kind: row.get("kind"),
        media_type: row.get("media_type"),
        version: row.get("current_version"),
        version_id: row.get("version_id"),
        content_sha256: row.get("content_sha256"),
        content: None,
        object_key: row.get("object_key"),
        object_version_id: row.get("object_version_id"),
        size_bytes: row.get("size_bytes"),
        metadata: row.get("metadata"),
        updated_at: row.get("updated_at"),
    })
}

fn render_read(entry: &EntryRow, request: &ReadItem, max_chars: usize) -> ApiResult<Value> {
    let content = entry.content.as_deref().unwrap_or("");
    let view = request.view.as_deref().unwrap_or("full");
    let selected = match view {
        "full" | "current_state" | "current_truth" => content.to_owned(),
        "range" => {
            let start = request.start.unwrap_or(1).max(1);
            let end = request.end.unwrap_or(start + 199).max(start);
            content
                .lines()
                .skip(start - 1)
                .take(end - start + 1)
                .collect::<Vec<_>>()
                .join("\n")
        }
        "outline" => content
            .lines()
            .filter(|line| line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n"),
        other => {
            return Err(ApiError::invalid(format!(
                "simple read does not support view {other}"
            )));
        }
    };
    let selected_chars = selected.chars().count();
    let mut rendered = serde_json::Map::from_iter([
        (
            "reference".to_owned(),
            Value::String(format!("entry:{}", entry.id)),
        ),
        ("path".to_owned(), Value::String(entry.path.clone())),
        ("title".to_owned(), Value::String(entry.title.clone())),
        ("version".to_owned(), json!(entry.version)),
        (
            "content_hash".to_owned(),
            Value::String(format!("sha256:{}", entry.content_sha256)),
        ),
        (
            "media_type".to_owned(),
            Value::String(entry.media_type.clone()),
        ),
        ("view".to_owned(), Value::String(view.to_owned())),
        (
            "text".to_owned(),
            Value::String(truncate_chars(&selected, max_chars)),
        ),
        ("updated_at".to_owned(), json!(entry.updated_at)),
    ]);
    if selected_chars > max_chars {
        rendered.insert("truncated".to_owned(), Value::Bool(true));
    }
    if entry.metadata != json!({}) && !entry.metadata.is_null() {
        rendered.insert("metadata".to_owned(), entry.metadata.clone());
    }
    Ok(Value::Object(rendered))
}

async fn prepare_markdown(state: &AppState, request: WriteRequest) -> ApiResult<PreparedMarkdown> {
    validate_path(&request.path)?;
    if request.content.len() > MAX_WRITE_BYTES {
        return Err(ApiError::public(
            StatusCode::PAYLOAD_TOO_LARGE,
            "entry_too_large",
            "Markdown writes are limited to 4 MiB; use a binary entry and companion Markdown",
        ));
    }
    if request.media_type != "text/markdown" && request.media_type != "text/plain" {
        return Err(ApiError::invalid(
            "workspace.write accepts Markdown or plain text; upload other files as binaries",
        ));
    }
    let normalized = normalize_document(&request.path, &request.content);
    let frontmatter = if state.config.supersession_demotion || state.config.intention_ledger {
        parse_frontmatter(&request.content)
    } else {
        DerivedFrontmatter::default()
    };
    let embeddings = vec![None; normalized.chunks.len()];
    let mut metadata = match request.metadata {
        Value::Object(values) => values,
        Value::Null => serde_json::Map::new(),
        value => serde_json::Map::from_iter([("value".to_owned(), value)]),
    };
    if let Some(idempotency_key) = request.idempotency_key {
        validate_idempotency_key(&idempotency_key)?;
        metadata.insert(
            "_straylight_idempotency_hash".to_owned(),
            Value::String(hex::encode(Sha256::digest(idempotency_key.as_bytes()))),
        );
    }
    Ok(PreparedMarkdown {
        entry_id_hint: None,
        path: request.path,
        title: normalized.title,
        content_sha256: normalized
            .content_hash
            .trim_start_matches("sha256:")
            .to_owned(),
        content: request.content,
        media_type: request.media_type,
        metadata: Value::Object(metadata),
        chunks: normalized.chunks,
        embeddings,
        expected_version: request.expected_version,
        frontmatter,
    })
}

async fn commit_markdown(
    state: &AppState,
    auth: &AuthContext,
    prepared: PreparedMarkdown,
) -> ApiResult<Value> {
    let started = Instant::now();
    let path = prepared.path.clone();
    let content_sha256 = prepared.content_sha256.clone();
    let frontmatter = if state.config.supersession_demotion {
        prepared.frontmatter.clone()
    } else {
        DerivedFrontmatter::default()
    };
    let warnings = if frontmatter.supersedes.is_empty() {
        Vec::new()
    } else {
        let generation = current_generation(state, auth).await?;
        match feature_snapshot(state, auth, generation).await? {
            Some(snapshot) => supersession_warnings(&frontmatter, &snapshot),
            None => Vec::new(),
        }
    };
    let mut tx = state.begin_write(auth).await?;
    require_local_publish_lock(
        &mut tx,
        format!(
            "simple-entry:{}:{}",
            auth.user_id.0,
            portable_path_key(&prepared.path)
        ),
    )
    .await?;
    let result = upsert_markdown_in_tx(
        &mut tx,
        auth.user_id.0,
        Some(auth.credential_id.0),
        prepared,
    )
    .await?;
    let generation = match result.generation {
        Some(value) => value,
        None => max_generation_in_tx(&mut tx, auth.user_id.0).await?,
    };
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    metrics::histogram!("simple.write.duration_ms")
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!("simple.write.changed_entries").record(if result.no_op {
        0.0
    } else {
        1.0
    });
    let mut receipt = json!({
        "entry_ref": format!("entry:{}", result.entry_id),
        "version_ref": result.version_id.map(|id| format!("entry-version:{id}")),
        "path": path,
        "version": result.version,
        "content_hash": format!("sha256:{content_sha256}"),
        "workspace_generation": generation,
        "search_status": if result.no_op {
            "unchanged"
        } else if result.metadata_only {
            "content_unchanged"
        } else {
            "lexical_ready_semantic_queued"
        },
        "metadata_only": result.metadata_only,
        "no_op": result.no_op
    });
    if !warnings.is_empty() {
        receipt["supersession_warnings"] = json!(warnings);
    }
    Ok(receipt)
}

pub(crate) async fn write_markdown_as_worker(
    state: &AppState,
    user_id: Uuid,
    path: String,
    content: String,
    metadata: Value,
    expected_version: Option<i64>,
    guard_entry: Option<(Uuid, i64)>,
) -> ApiResult<Option<Value>> {
    let prepared = prepare_markdown(
        state,
        WriteRequest {
            path,
            content,
            media_type: markdown_media_type(),
            expected_version,
            idempotency_key: None,
            metadata,
        },
    )
    .await?;
    let result_path = prepared.path.clone();
    let content_sha256 = prepared.content_sha256.clone();
    let pool = state
        .admin_pool
        .as_ref()
        .ok_or_else(|| ApiError::configuration("the worker requires DATABASE_URL_ADMIN"))?;
    let mut tx = pool.begin().await?;
    require_local_publish_lock(
        &mut tx,
        format!(
            "simple-entry:{user_id}:{}",
            portable_path_key(&prepared.path)
        ),
    )
    .await?;
    if let Some((entry_id, expected_entry_version)) = guard_entry {
        let current_entry_version = sqlx::query_scalar::<_, Option<i64>>(
            r#"
            SELECT current_version
            FROM straylight.entries
            WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL
            FOR SHARE
            "#,
        )
        .bind(user_id)
        .bind(entry_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        if current_entry_version != Some(expected_entry_version) {
            tx.commit().await?;
            return Ok(None);
        }
    }
    let result = upsert_markdown_in_tx(&mut tx, user_id, None, prepared).await?;
    let generation = match result.generation {
        Some(value) => value,
        None => max_generation_in_tx(&mut tx, user_id).await?,
    };
    tx.commit().await?;
    state.workspace_features.invalidate(user_id).await;
    Ok(Some(json!({
        "entry_ref": format!("entry:{}", result.entry_id),
        "version_ref": result.version_id.map(|id| format!("entry-version:{id}")),
        "path": result_path,
        "version": result.version,
        "content_hash": format!("sha256:{content_sha256}"),
        "workspace_generation": generation,
        "metadata_only": result.metadata_only,
        "no_op": result.no_op
    })))
}

async fn fetch_locked_markdown_entry(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    path: &str,
) -> ApiResult<Option<PgRow>> {
    Ok(sqlx::query(
        r#"
        SELECT entry.id,entry.kind,entry.current_version,entry.deleted_at,
               version.id AS version_id,version.content_sha256,version.metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND lower(normalize(entry.path, NFC))=$2
        FOR UPDATE OF entry
        "#,
    )
    .bind(user_id)
    .bind(portable_path_key(path))
    .fetch_optional(&mut **tx)
    .await?)
}

async fn upsert_markdown_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    created_by_credential_id: Option<Uuid>,
    prepared: PreparedMarkdown,
) -> ApiResult<MarkdownUpsertResult> {
    let checkpoint_import = is_portable_checkpoint_import(&prepared.path, &prepared.metadata);
    let proposed_entry_id = prepared.entry_id_hint.unwrap_or_else(Uuid::now_v7);
    let inserted_entry_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,$4,'markdown',$5,0)
        ON CONFLICT (user_id,(lower(normalize(path, NFC)))) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(proposed_entry_id)
    .bind(user_id)
    .bind(&prepared.path)
    .bind(&prepared.title)
    .bind(&prepared.media_type)
    .fetch_optional(&mut **tx)
    .await?;
    let existing = if inserted_entry_id.is_some() {
        None
    } else {
        fetch_locked_markdown_entry(tx, user_id, &prepared.path).await?
    };
    if existing
        .as_ref()
        .is_some_and(|row| row.get::<String, _>("kind") != "markdown")
    {
        return Err(ApiError::conflict(
            "entry_kind_conflict",
            "a binary entry already uses this Markdown path",
            json!({"path": prepared.path}),
        ));
    }
    if let Some(row) = &existing
        && is_checkpoint_path(&prepared.path)
    {
        if row.get::<String, _>("content_sha256") == prepared.content_sha256
            && row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_none()
        {
            return Ok(MarkdownUpsertResult {
                entry_id: row.get("id"),
                version: row.get("current_version"),
                version_id: Some(row.get("version_id")),
                generation: None,
                no_op: true,
                metadata_only: false,
            });
        }
        return Err(ApiError::conflict(
            "checkpoint_immutable",
            "checkpoint entries are immutable; create a new checkpoint instead",
            json!({"path": prepared.path}),
        ));
    }
    if let Some(row) = &existing
        && row.get::<String, _>("content_sha256") == prepared.content_sha256
    {
        let was_deleted = row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some();
        let current_metadata = row.get::<Value, _>("metadata");
        let metadata_only = (prepared.metadata.get("_straylight_import").is_some()
            || prepared.metadata.get("portable").is_some())
            && current_metadata != prepared.metadata;
        if metadata_only {
            sqlx::query(
                r#"
                UPDATE straylight.entry_versions
                SET metadata=$4
                WHERE user_id=$1 AND entry_id=$2 AND version=$3
                "#,
            )
            .bind(user_id)
            .bind(row.get::<Uuid, _>("id"))
            .bind(row.get::<i64, _>("current_version"))
            .bind(&prepared.metadata)
            .execute(&mut **tx)
            .await?;
        }
        if was_deleted {
            sqlx::query(
                r#"
                UPDATE straylight.entries
                SET path=$3,title=$4,media_type=$5,deleted_at=NULL,
                    updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                "#,
            )
            .bind(user_id)
            .bind(row.get::<Uuid, _>("id"))
            .bind(&prepared.path)
            .bind(&prepared.title)
            .bind(&prepared.media_type)
            .execute(&mut **tx)
            .await?;
            sqlx::query("DELETE FROM straylight.search_chunks WHERE user_id=$1 AND entry_id=$2")
                .bind(user_id)
                .bind(row.get::<Uuid, _>("id"))
                .execute(&mut **tx)
                .await?;
            insert_chunks(
                tx,
                user_id,
                row.get("id"),
                row.get("version_id"),
                &prepared.path,
                &prepared.chunks,
                &prepared.embeddings,
            )
            .await?;
            if prepared.embeddings.iter().any(Option::is_none) {
                sqlx::query(
                    r#"
                    INSERT INTO straylight.jobs (user_id,kind,payload)
                    VALUES ($1,'embed_entry',$2)
                    "#,
                )
                .bind(user_id)
                .bind(json!({
                    "entry_id": row.get::<Uuid, _>("id"),
                    "version": row.get::<i64, _>("current_version")
                }))
                .execute(&mut **tx)
                .await?;
            }
        } else if metadata_only {
            sqlx::query(
                r#"
                UPDATE straylight.entries
                SET updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                "#,
            )
            .bind(user_id)
            .bind(row.get::<Uuid, _>("id"))
            .execute(&mut **tx)
            .await?;
        }
        let changed = was_deleted || metadata_only;
        let generation = if changed {
            Some(
                sqlx::query_scalar::<_, i64>(
                    r#"
                    INSERT INTO straylight.workspace_changes (
                      user_id,entry_id,entry_version,operation,path,content_sha256
                    ) VALUES ($1,$2,$3,'update',$4,$5)
                    RETURNING generation
                    "#,
                )
                .bind(user_id)
                .bind(row.get::<Uuid, _>("id"))
                .bind(row.get::<i64, _>("current_version"))
                .bind(&prepared.path)
                .bind(&prepared.content_sha256)
                .fetch_one(&mut **tx)
                .await?,
            )
        } else {
            None
        };
        return Ok(MarkdownUpsertResult {
            entry_id: row.get("id"),
            version: row.get("current_version"),
            version_id: Some(row.get("version_id")),
            generation,
            no_op: !changed,
            metadata_only,
        });
    }
    if let Some(expected) = prepared.expected_version {
        let actual = existing
            .as_ref()
            .map(|row| row.get::<i64, _>("current_version"))
            .unwrap_or(0);
        let restoring_deleted = existing.as_ref().is_some_and(|row| {
            row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some() && expected == 0
        });
        if actual != expected && !restoring_deleted {
            return Err(ApiError::conflict(
                "entry_version_conflict",
                "the entry changed since it was read",
                json!({
                    "path": prepared.path,
                    "expected_version": expected,
                    "actual_version": actual
                }),
            ));
        }
    }
    let (entry_id, version, operation) = match existing {
        Some(row) => (
            row.get::<Uuid, _>("id"),
            row.get::<i64, _>("current_version") + 1,
            "update",
        ),
        None => (proposed_entry_id, 1, "create"),
    };
    let version_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,
          metadata,created_by_credential_id
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind(version_id)
    .bind(user_id)
    .bind(entry_id)
    .bind(version)
    .bind(&prepared.content_sha256)
    .bind(&prepared.content)
    .bind(i64::try_from(prepared.content.len()).unwrap_or(i64::MAX))
    .bind(&prepared.metadata)
    .bind(created_by_credential_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.entries
        SET path=$3,title=$4,media_type=$5,current_version=$6,
            updated_at=clock_timestamp(),deleted_at=NULL
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .bind(&prepared.path)
    .bind(&prepared.title)
    .bind(&prepared.media_type)
    .bind(version)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM straylight.search_chunks WHERE user_id=$1 AND entry_id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&mut **tx)
        .await?;
    insert_chunks(
        tx,
        user_id,
        entry_id,
        version_id,
        &prepared.path,
        &prepared.chunks,
        &prepared.embeddings,
    )
    .await?;
    let generation = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO straylight.workspace_changes (
          user_id,entry_id,entry_version,operation,path,content_sha256
        ) VALUES ($1,$2,$3,$4,$5,$6)
        RETURNING generation
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .bind(version)
    .bind(operation)
    .bind(&prepared.path)
    .bind(&prepared.content_sha256)
    .fetch_one(&mut **tx)
    .await?;
    if checkpoint_import {
        let metadata = rebase_imported_checkpoint_metadata(prepared.metadata.clone(), generation);
        sqlx::query(
            r#"
            UPDATE straylight.entry_versions
            SET metadata=$4
            WHERE user_id=$1 AND entry_id=$2 AND version=$3
            "#,
        )
        .bind(user_id)
        .bind(entry_id)
        .bind(version)
        .bind(metadata)
        .execute(&mut **tx)
        .await?;
    }
    if prepared.embeddings.iter().any(Option::is_none) {
        sqlx::query(
            r#"
            INSERT INTO straylight.jobs (user_id,kind,payload)
            VALUES ($1,'embed_entry',$2)
            "#,
        )
        .bind(user_id)
        .bind(json!({"entry_id": entry_id, "version": version}))
        .execute(&mut **tx)
        .await?;
    }
    Ok(MarkdownUpsertResult {
        entry_id,
        version,
        version_id: Some(version_id),
        generation: Some(generation),
        no_op: false,
        metadata_only: false,
    })
}

async fn insert_chunks(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entry_id: Uuid,
    version_id: Uuid,
    path: &str,
    chunks: &[DocumentChunk],
    embeddings: &[Option<Vector>],
) -> ApiResult<()> {
    if chunks.is_empty() {
        return Ok(());
    }
    let pairs = chunks.iter().zip(embeddings).collect::<Vec<_>>();
    for batch in pairs.chunks(CHUNK_INSERT_BATCH_SIZE) {
        let mut builder = QueryBuilder::<Postgres>::new(
            "INSERT INTO straylight.search_chunks \
             (id,user_id,entry_id,entry_version_id,chunk_index,path,heading,content,token_estimate,embedding) ",
        );
        builder.push_values(batch.iter().copied(), |mut row, (chunk, embedding)| {
            row.push_bind(Uuid::now_v7())
                .push_bind(user_id)
                .push_bind(entry_id)
                .push_bind(version_id)
                .push_bind(i32::try_from(chunk.ordinal).unwrap_or(i32::MAX))
                .push_bind(path)
                .push_bind(&chunk.heading)
                .push_bind(&chunk.content)
                .push_bind(i32::try_from(chunk.estimated_tokens).unwrap_or(i32::MAX))
                .push_bind(embedding.clone());
        });
        builder.build().execute(&mut **tx).await?;
    }
    Ok(())
}

async fn changes_since(
    state: &AppState,
    auth: &AuthContext,
    since: i64,
    limit: usize,
) -> ApiResult<ChangePage> {
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT generation,operation,path,entry_version,content_sha256,recorded_at
        FROM straylight.workspace_changes
        WHERE user_id=$1 AND generation>$2
        ORDER BY generation
        LIMIT $3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(since)
    .bind(i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let truncated = rows.len() > limit;
    let mut changes = rows
        .into_iter()
        .map(|row| {
            json!({
                "generation": row.get::<i64, _>("generation"),
                "operation": row.get::<String, _>("operation"),
                "path": row.get::<String, _>("path"),
                "version": row.get::<i64, _>("entry_version"),
                "content_hash": format!("sha256:{}", row.get::<String, _>("content_sha256")),
                "recorded_at": row.get::<DateTime<Utc>, _>("recorded_at")
            })
        })
        .collect::<Vec<_>>();
    if truncated {
        changes.truncate(limit);
    }
    let next_generation = truncated
        .then(|| changes.last().and_then(|value| value.get("generation")))
        .flatten()
        .and_then(Value::as_i64);
    Ok(ChangePage {
        changes,
        truncated,
        next_generation,
    })
}

async fn read_checkpoint(
    state: &AppState,
    auth: &AuthContext,
    checkpoint_ref: &str,
) -> ApiResult<Value> {
    let entry = resolve_entry(state, auth, None, Some(checkpoint_ref)).await?;
    let metadata = entry
        .metadata
        .get("client")
        .filter(|value| value.is_object())
        .unwrap_or(&entry.metadata);
    if metadata.get("kind").and_then(Value::as_str) != Some("checkpoint") {
        return Err(ApiError::invalid(
            "resume_checkpoint_ref does not identify a checkpoint entry",
        ));
    }
    Ok(json!({
        "checkpoint_id": checkpoint_ref,
        "path": entry.path,
        "workspace_generation": metadata.get("workspace_generation"),
        "text": entry.content,
        "source_entries": metadata.get("source_entries")
    }))
}

async fn resolve_checkpoint_sources(
    state: &AppState,
    auth: &AuthContext,
    checkpoint_state: &Value,
    source_refs: &[String],
) -> ApiResult<Vec<EntryRow>> {
    if source_refs.len() > 64 {
        return Err(ApiError::invalid(
            "checkpoint source_refs are limited to 64 exact references",
        ));
    }
    let mut resolved = Vec::new();
    let mut seen = HashSet::new();
    let explicit = source_refs
        .iter()
        .filter(|candidate| {
            !candidate.starts_with("source_episode:") && !candidate.starts_with("evidence:")
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut explicit_results =
        futures::stream::iter(explicit.into_iter().enumerate().map(|(index, candidate)| {
            let state = state.clone();
            let auth = auth.clone();
            async move {
                let entry = if candidate.starts_with("entry:") {
                    resolve_entry_summary(&state, &auth, None, Some(&candidate)).await
                } else {
                    resolve_entry_summary(&state, &auth, Some(&candidate), None).await
                };
                (index, entry)
            }
        }))
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    explicit_results.sort_by_key(|(index, _)| *index);
    for (_, entry) in explicit_results {
        let entry = entry?;
        if seen.insert(entry.id) {
            resolved.push(entry);
        }
    }
    let mut inferred = Vec::new();
    collect_markdown_paths(checkpoint_state, &mut inferred);
    inferred.sort();
    inferred.dedup();
    let remaining = 64_usize.saturating_sub(resolved.len());
    let mut inferred_results =
        futures::stream::iter(inferred.into_iter().take(remaining).enumerate().map(
            |(index, candidate)| {
                let state = state.clone();
                let auth = auth.clone();
                async move {
                    (
                        index,
                        resolve_entry_summary(&state, &auth, Some(&candidate), None).await,
                    )
                }
            },
        ))
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    inferred_results.sort_by_key(|(index, _)| *index);
    for (_, entry) in inferred_results {
        if let Ok(entry) = entry
            && seen.insert(entry.id)
        {
            resolved.push(entry);
        }
    }
    Ok(resolved)
}

fn collect_markdown_paths(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if looks_like_path(text) {
                output.push(text.to_owned());
            }
            for path in path_hints(text) {
                output.push(path);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_markdown_paths(item, output);
            }
        }
        Value::Object(object) => {
            for item in object.values() {
                collect_markdown_paths(item, output);
            }
        }
        _ => {}
    }
}

fn render_checkpoint_markdown(
    checkpoint_id: Uuid,
    generation: i64,
    request: &CheckpointRequest,
    source_entries: &[EntryRow],
) -> ApiResult<String> {
    let objective = request
        .state
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("Resume durable work");
    let mut output = format!(
        "---\nstraylight_kind: checkpoint\ncheckpoint_id: {checkpoint_id}\n\
         workspace_generation: {generation}\nsession_id: {}\nparent_checkpoint_id: {}\n---\n\n\
         # Checkpoint: {}\n\n",
        request.session_id,
        request.parent_checkpoint_id.as_deref().unwrap_or(""),
        objective
    );
    for (field, heading) in [
        ("current_state", "Current state"),
        ("decisions", "Decisions"),
        ("open_questions", "Open questions"),
        ("next_actions", "Next actions"),
        ("artifacts", "Artifacts"),
    ] {
        output.push_str(&format!("## {heading}\n\n"));
        match request.state.get(field) {
            Some(Value::Array(items)) => {
                for item in items {
                    output.push_str(&format!(
                        "- {}\n",
                        item.as_str()
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| item.to_string())
                    ));
                }
            }
            Some(Value::String(text)) => output.push_str(&format!("- {text}\n")),
            _ => output.push_str("- None recorded.\n"),
        }
        output.push('\n');
    }
    output.push_str("## Exact source references\n\n");
    if source_entries.is_empty() {
        output.push_str("- None recorded.\n");
    } else {
        for entry in source_entries {
            output.push_str(&format!(
                "- `{}` | version {} | `sha256:{}`\n",
                entry.path, entry.version, entry.content_sha256
            ));
        }
    }
    let external_refs = request
        .source_refs
        .iter()
        .filter(|value| value.starts_with("source_episode:") || value.starts_with("evidence:"))
        .collect::<Vec<_>>();
    if !external_refs.is_empty() {
        output.push_str("\n## External source references\n\n");
        for reference in external_refs {
            output.push_str(&format!("- `{reference}`\n"));
        }
    }
    Ok(output)
}

fn deterministic_checkpoint_id(request: &CheckpointRequest) -> Uuid {
    let identity = json!({
        "session_id": request.session_id,
        "parent_checkpoint_id": request.parent_checkpoint_id,
        "state": request.state,
        "source_refs": request.source_refs,
        "idempotency_key": request.idempotency_key
    });
    let digest = Sha256::digest(
        serde_json::to_vec(&identity).unwrap_or_else(|_| Uuid::now_v7().as_bytes().to_vec()),
    );
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn entry_reference(entry: &EntryRow) -> Value {
    json!({
        "entry_ref": format!("entry:{}", entry.id),
        "path": entry.path,
        "version": entry.version,
        "content_hash": format!("sha256:{}", entry.content_sha256)
    })
}

fn path_hints(query: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut unquoted = query.to_owned();
    for delimiter in ['`', '"'] {
        let mut remainder = query;
        while let Some(start) = remainder.find(delimiter) {
            remainder = &remainder[start + delimiter.len_utf8()..];
            let Some(end) = remainder.find(delimiter) else {
                break;
            };
            let candidate = &remainder[..end];
            if looks_like_path(candidate) {
                paths.push(candidate.to_owned());
            }
            remainder = &remainder[end + delimiter.len_utf8()..];
        }
        let mut inside = false;
        unquoted = unquoted
            .chars()
            .map(|character| {
                if character == delimiter {
                    inside = !inside;
                    ' '
                } else if inside {
                    ' '
                } else {
                    character
                }
            })
            .collect();
    }
    paths.extend(
        unquoted
            .split_whitespace()
            .map(|token| {
                token.trim_matches(|character: char| {
                    matches!(
                        character,
                        '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
                    )
                })
            })
            .filter(|token| looks_like_path(token))
            .map(ToOwned::to_owned),
    );
    paths.sort();
    paths.dedup();
    paths.truncate(16);
    paths
}

fn search_anchors(query: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    for delimiter in ['`', '"'] {
        let mut remainder = query;
        while let Some(start) = remainder.find(delimiter) {
            remainder = &remainder[start + delimiter.len_utf8()..];
            let Some(end) = remainder.find(delimiter) else {
                break;
            };
            let candidate = remainder[..end].trim();
            if (3..=256).contains(&candidate.chars().count()) {
                anchors.push(candidate.to_owned());
            }
            remainder = &remainder[end + delimiter.len_utf8()..];
        }
    }
    anchors.extend(
        query
            .split_whitespace()
            .map(|token| {
                token.trim_matches(|character: char| {
                    !character.is_alphanumeric() && !matches!(character, '-' | '_' | '/' | '.')
                })
            })
            .filter(|token| {
                (6..=256).contains(&token.chars().count())
                    && looks_like_unquoted_search_anchor(token)
            })
            .map(ToOwned::to_owned),
    );
    anchors.retain(|anchor| !anchor.eq_ignore_ascii_case(query.trim()));
    let mut deduplicated = Vec::new();
    for anchor in anchors {
        if !deduplicated
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&anchor))
        {
            deduplicated.push(anchor);
        }
    }
    deduplicated.truncate(2);
    deduplicated
}

fn looks_like_unquoted_search_anchor(value: &str) -> bool {
    let has_digit = value.chars().any(|character| character.is_ascii_digit());
    value.contains('_')
        || (has_digit
            && value
                .chars()
                .any(|character| matches!(character, '-' | '/')))
}

fn lexical_fallback_queries(query: &str) -> Vec<String> {
    lexical_terms(query, 16)
        .windows(2)
        .take(8)
        .map(|terms| terms.join(" "))
        .collect()
}

fn focused_lexical_query(query: &str) -> Option<String> {
    let mut pairs = lexical_fallback_queries(query)
        .into_iter()
        .enumerate()
        .map(|(index, pair)| {
            let score = pair
                .chars()
                .filter(|character| character.is_alphanumeric())
                .count()
                + if pair.chars().any(|character| character.is_ascii_digit()) {
                    40
                } else {
                    0
                };
            (score, index, pair)
        })
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let selected = pairs
        .into_iter()
        .take(4)
        .map(|(_, _, pair)| pair)
        .collect::<Vec<_>>();
    (!selected.is_empty()).then(|| selected.join(" OR "))
}

fn lexical_terms(query: &str, limit: usize) -> Vec<String> {
    let mut distinctive = Vec::new();
    let mut ordinary = Vec::new();
    let mut seen = HashSet::new();
    for raw in query.split(|character: char| !character.is_alphanumeric()) {
        let term = raw.to_lowercase();
        let has_digit = term.chars().any(|character| character.is_ascii_digit());
        if (!has_digit && term.chars().count() < 3)
            || is_lexical_noise(&term)
            || !seen.insert(term.clone())
        {
            continue;
        }
        let letters = raw
            .chars()
            .filter(|character| character.is_alphabetic())
            .collect::<Vec<_>>();
        let is_acronym =
            letters.len() >= 2 && letters.iter().all(|character| character.is_uppercase());
        if has_digit || is_acronym {
            distinctive.push(term);
        } else {
            ordinary.push(term);
        }
    }
    distinctive
        .into_iter()
        .chain(ordinary)
        .take(limit)
        .collect()
}

fn lexical_candidate_bonus(
    query: &str,
    path: &str,
    title: &str,
    heading: &str,
    excerpt: &str,
) -> f64 {
    let path_title = format!("{path} {title}");
    let path_title_words = normalized_search_words(&path_title);
    let path_title_collapsed = collapsed_search_text(&path_title);
    let heading_words = normalized_search_words(heading);
    let heading_collapsed = collapsed_search_text(heading);
    let excerpt_words = normalized_search_words(excerpt);
    let excerpt_collapsed = collapsed_search_text(excerpt);

    lexical_terms(query, 32)
        .into_iter()
        .map(|term| {
            let weight = if term.chars().any(|character| character.is_ascii_digit())
                || term.chars().count() >= 10
            {
                1.4
            } else if term.chars().count() >= 6 {
                1.0
            } else {
                0.7
            };
            if search_text_contains(&path_title_words, &path_title_collapsed, &term) {
                0.8 * weight
            } else if search_text_contains(&heading_words, &heading_collapsed, &term) {
                0.45 * weight
            } else if search_text_contains(&excerpt_words, &excerpt_collapsed, &term) {
                0.1 * weight
            } else {
                0.0
            }
        })
        .sum::<f64>()
        .min(8.0)
}

fn normalized_search_words(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len() + 2);
    normalized.push(' ');
    let mut separated = true;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            normalized.push(character);
            separated = false;
        } else if !separated {
            normalized.push(' ');
            separated = true;
        }
    }
    if !separated {
        normalized.push(' ');
    }
    normalized
}

fn collapsed_search_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn search_text_contains(words: &str, collapsed: &str, term: &str) -> bool {
    words.contains(&format!(" {term} ")) || (term.chars().count() >= 5 && collapsed.contains(term))
}

fn is_lexical_noise(term: &str) -> bool {
    matches!(
        term,
        "and"
            | "are"
            | "about"
            | "after"
            | "against"
            | "also"
            | "before"
            | "being"
            | "between"
            | "could"
            | "current"
            | "does"
            | "each"
            | "for"
            | "inspect"
            | "explain"
            | "from"
            | "give"
            | "handle"
            | "have"
            | "identify"
            | "into"
            | "leave"
            | "needed"
            | "only"
            | "please"
            | "reconcile"
            | "report"
            | "request"
            | "resume"
            | "should"
            | "state"
            | "task"
            | "that"
            | "the"
            | "their"
            | "them"
            | "then"
            | "there"
            | "these"
            | "this"
            | "through"
            | "using"
            | "was"
            | "were"
            | "what"
            | "when"
            | "where"
            | "whether"
            | "which"
            | "while"
            | "with"
            | "without"
            | "would"
            | "will"
            | "work"
    )
}

fn continuation_paths(
    checkpoint: Option<&Value>,
    changes: &[Value],
) -> (Vec<String>, HashSet<String>) {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut changed_keys = HashSet::new();
    for path in changes
        .iter()
        .rev()
        .filter_map(|change| change.get("path").and_then(Value::as_str))
    {
        if path.starts_with(".straylight/") {
            continue;
        }
        let key = portable_path_key(path);
        changed_keys.insert(key.clone());
        if seen.insert(key) {
            paths.push(path.to_owned());
        }
        if paths.len() == 16 {
            break;
        }
    }
    if let Some(entries) = checkpoint
        .and_then(|value| value.get("source_entries"))
        .and_then(Value::as_array)
    {
        for path in entries
            .iter()
            .filter_map(|entry| entry.get("path").and_then(Value::as_str))
        {
            if path.starts_with(".straylight/") {
                continue;
            }
            let key = portable_path_key(path);
            if seen.insert(key) {
                paths.push(path.to_owned());
            }
            if paths.len() == 32 {
                break;
            }
        }
    }
    (paths, changed_keys)
}

fn looks_like_path(value: &str) -> bool {
    value.contains('/')
        && !value.starts_with('/')
        && ["md", "txt", "sql", "json", "jsonl", "csv"]
            .iter()
            .any(|extension| value.ends_with(&format!(".{extension}")))
}

fn validate_path(path: &str) -> ApiResult<()> {
    if path.is_empty()
        || path.len() > 1_024
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.chars().any(char::is_control)
    {
        return Err(ApiError::invalid(
            "path must be a safe relative workspace path",
        ));
    }
    Ok(())
}

fn validate_public_path(path: &str) -> ApiResult<()> {
    validate_path(path)?;
    if path.starts_with(".straylight/") {
        return Err(ApiError::invalid(
            "the .straylight namespace is reserved for workspace-managed entries",
        ));
    }
    Ok(())
}

fn require_write_capabilities(auth: &AuthContext, path: &str) -> ApiResult<()> {
    auth.require(Capability::Save)?;
    if is_checkpoint_path(path) {
        auth.require(Capability::Checkpoint)?;
    } else if path.starts_with(".straylight/binaries/") {
        auth.require(Capability::Stage)?;
    }
    Ok(())
}

fn validate_write_path(request: &WriteRequest) -> ApiResult<()> {
    validate_path(&request.path)?;
    if !request.path.starts_with(".straylight/") {
        return Ok(());
    }
    let portable_restore = request
        .metadata
        .get("_straylight_import")
        .and_then(|value| value.get("format"))
        .and_then(Value::as_str)
        == Some("straylight-workspace-import-manifest@v1");
    let client_metadata = request
        .metadata
        .get("client")
        .filter(|value| value.is_object())
        .unwrap_or(&request.metadata);
    let checkpoint_restore = request
        .path
        .strip_prefix(".straylight/checkpoints/")
        .and_then(|value| value.strip_suffix(".md"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .is_some_and(|checkpoint_id| {
            client_metadata.get("kind").and_then(Value::as_str) == Some("checkpoint")
                && client_metadata
                    .get("checkpoint_ref")
                    .and_then(Value::as_str)
                    .is_none_or(|reference| reference == format!("checkpoint:{checkpoint_id}"))
        });
    let binary_companion_restore = request.path.starts_with(".straylight/binaries/")
        && request.path.ends_with(".md")
        && client_metadata.get("kind").and_then(Value::as_str) == Some("binary_description")
        && client_metadata
            .get("binary_path")
            .and_then(Value::as_str)
            .is_some();
    if portable_restore
        && request.expected_version.is_some()
        && (checkpoint_restore || binary_companion_restore)
    {
        return Ok(());
    }
    Err(ApiError::invalid(
        "the .straylight namespace is reserved for workspace-managed entries",
    ))
}

fn is_checkpoint_path(path: &str) -> bool {
    path.strip_prefix(".straylight/checkpoints/")
        .and_then(|value| value.strip_suffix(".md"))
        .and_then(|value| Uuid::parse_str(value).ok())
        .is_some()
}

fn is_portable_checkpoint_import(path: &str, metadata: &Value) -> bool {
    if !is_checkpoint_path(path)
        || metadata
            .get("_straylight_import")
            .and_then(|value| value.get("format"))
            .and_then(Value::as_str)
            != Some("straylight-workspace-import-manifest@v1")
    {
        return false;
    }
    metadata
        .get("client")
        .filter(|value| value.is_object())
        .unwrap_or(metadata)
        .get("kind")
        .and_then(Value::as_str)
        == Some("checkpoint")
}

fn rebase_imported_checkpoint_metadata(mut metadata: Value, generation: i64) -> Value {
    let target = if metadata.get("client").is_some_and(Value::is_object) {
        metadata
            .get_mut("client")
            .and_then(Value::as_object_mut)
            .expect("checked checkpoint client metadata")
    } else {
        metadata
            .as_object_mut()
            .expect("prepared Markdown metadata is always an object")
    };
    if let Some(origin) = target.get("workspace_generation").cloned() {
        target
            .entry("origin_workspace_generation".to_owned())
            .or_insert(origin);
    }
    target.insert("workspace_generation".to_owned(), json!(generation));
    if let Some(source_entries) = target
        .get_mut("source_entries")
        .and_then(Value::as_array_mut)
    {
        for source in source_entries {
            if let Some(source) = source.as_object_mut() {
                source.remove("entry_ref");
            }
        }
    }
    metadata
}

fn portable_path_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

fn validate_idempotency_key(key: &str) -> ApiResult<()> {
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            "idempotency_key must contain 1 to 256 printable characters",
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> ApiResult<String> {
    let normalized = value.trim().strip_prefix("sha256:").unwrap_or(value.trim());
    if normalized.len() != 64 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::invalid(
            "expected_content_hash must be a SHA-256 value",
        ));
    }
    Ok(normalized.to_ascii_lowercase())
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    text.chars().take(limit).collect()
}

fn record_candidate_usage(state: &AppState, auth: &AuthContext, candidates: &[Candidate]) {
    let ids = candidates
        .iter()
        .map(|candidate| candidate.entry_id)
        .collect::<Vec<_>>();
    record_entry_usage(state, auth, &ids, UsageOperation::Search);
}

fn record_entry_usage(
    state: &AppState,
    auth: &AuthContext,
    entry_ids: &[Uuid],
    operation: UsageOperation,
) {
    state
        .usage_tracker
        .record(auth.user_id.0, entry_ids.iter().copied(), operation);
}

fn validate_eval_import(request: &EvalImportRequest) -> ApiResult<()> {
    if request.schema != "straylight-eval-import@v1" {
        return Err(ApiError::invalid(
            "evaluation import schema must be straylight-eval-import@v1",
        ));
    }
    if !matches!(request.access_mode.as_str(), "read_only" | "read_write") {
        return Err(ApiError::invalid(
            "access_mode must be read_only or read_write",
        ));
    }
    if request.documents.is_empty() {
        return Err(ApiError::invalid(
            "evaluation import requires at least one document",
        ));
    }
    if request.documents.len() + request.delta_documents.len() > 700_000 {
        return Err(ApiError::invalid(
            "simple evaluation imports are limited to 700,000 documents",
        ));
    }
    if let Some((index, count)) = evaluation_batch(request)? {
        if request.documents.len() + request.delta_documents.len() > 10_000 {
            return Err(ApiError::invalid(
                "batched simple evaluation imports are limited to 10,000 documents per request",
            ));
        }
        if index + 1 < count
            && (!request.delta_documents.is_empty() || request.seed_checkpoint.is_some())
        {
            return Err(ApiError::invalid(
                "evaluation deltas and seed checkpoints belong only in the final batch",
            ));
        }
    }
    let total_bytes = request
        .documents
        .iter()
        .chain(&request.delta_documents)
        .map(|document| document.content.len())
        .sum::<usize>();
    if total_bytes > 512 * 1024 * 1024 {
        return Err(ApiError::public(
            StatusCode::PAYLOAD_TOO_LARGE,
            "eval_import_too_large",
            "evaluation text is limited to 512 MiB",
        ));
    }
    for document in request.documents.iter().chain(&request.delta_documents) {
        validate_path(&document.path)?;
        let digest = hex::encode(Sha256::digest(document.content.as_bytes()));
        if digest != document.content_sha256.to_lowercase() {
            return Err(ApiError::invalid(format!(
                "content_sha256 does not match {}",
                document.path
            )));
        }
    }
    Ok(())
}

fn evaluation_batch(request: &EvalImportRequest) -> ApiResult<Option<(usize, usize)>> {
    match (request.batch_index, request.batch_count) {
        (None, None) => Ok(None),
        (Some(index), Some(count)) if count > 0 && index < count => Ok(Some((index, count))),
        (Some(_), Some(_)) => Err(ApiError::invalid(
            "evaluation batch_index must be less than a positive batch_count",
        )),
        _ => Err(ApiError::invalid(
            "evaluation batch_index and batch_count must be provided together",
        )),
    }
}

async fn require_local_publish_lock(
    tx: &mut Transaction<'_, Postgres>,
    key: String,
) -> ApiResult<()> {
    let acquired =
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key)
            .fetch_one(&mut **tx)
            .await?;
    if !acquired {
        return Err(ApiError::conflict(
            "entry_busy",
            "another write is publishing the same entry; retry",
            json!({"retryable": true}),
        ));
    }
    Ok(())
}

fn derive_eval_token(secret: &str, external_ref: &str, idempotency_key: &str) -> ApiResult<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|error| ApiError::Internal(format!("invalid evaluation key: {error}")))?;
    mac.update(external_ref.as_bytes());
    mac.update(b"\0");
    mac.update(idempotency_key.as_bytes());
    Ok(format!(
        "straylight_eval_{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}

async fn prepare_bulk_documents(
    state: &AppState,
    documents: &[crate::eval_service::EvalDocument],
) -> ApiResult<Vec<BulkMarkdown>> {
    let normalized = documents
        .iter()
        .map(|document| {
            (
                document,
                normalize_document(&document.path, &document.content),
            )
        })
        .collect::<Vec<_>>();
    let mut prepared = Vec::with_capacity(normalized.len());
    for (source, document) in normalized {
        let embeddings = vec![None; document.chunks.len()];
        prepared.push(BulkMarkdown {
            entry_id: Uuid::now_v7(),
            version_id: Uuid::now_v7(),
            path: source.path.clone(),
            title: document.title,
            content: source.content.clone(),
            content_sha256: source.content_sha256.to_lowercase(),
            media_type: source.media_type.clone(),
            metadata: json!({"kind": "evaluation_import"}),
            chunks: document.chunks,
            embeddings,
            frontmatter: if state.config.supersession_demotion || state.config.intention_ledger {
                parse_frontmatter(&source.content)
            } else {
                DerivedFrontmatter::default()
            },
        });
    }
    Ok(prepared)
}

async fn prepare_one_bulk_document(
    state: &AppState,
    path: String,
    content: String,
    media_type: String,
) -> ApiResult<BulkMarkdown> {
    let digest = hex::encode(Sha256::digest(content.as_bytes()));
    let source = crate::eval_service::EvalDocument {
        path,
        content,
        content_sha256: digest,
        media_type,
    };
    prepare_bulk_documents(state, &[source])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::Internal("checkpoint preparation returned no document".to_owned()))
}

async fn insert_bulk_documents(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    documents: &[BulkMarkdown],
    operation: &str,
) -> ApiResult<()> {
    for batch in documents.chunks(1_000) {
        let mut entries = QueryBuilder::<Postgres>::new(
            "INSERT INTO straylight.entries \
             (id,user_id,path,title,kind,media_type,current_version) ",
        );
        entries.push_values(batch, |mut row, document| {
            row.push_bind(document.entry_id)
                .push_bind(user_id)
                .push_bind(&document.path)
                .push_bind(&document.title)
                .push_bind("markdown")
                .push_bind(&document.media_type)
                .push_bind(1_i64);
        });
        entries.build().execute(&mut **tx).await?;
    }
    for batch in documents.chunks(700) {
        let mut versions = QueryBuilder::<Postgres>::new(
            "INSERT INTO straylight.entry_versions \
             (id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) ",
        );
        versions.push_values(batch, |mut row, document| {
            row.push_bind(document.version_id)
                .push_bind(user_id)
                .push_bind(document.entry_id)
                .push_bind(1_i64)
                .push_bind(&document.content_sha256)
                .push_bind(&document.content)
                .push_bind(i64::try_from(document.content.len()).unwrap_or(i64::MAX))
                .push_bind(&document.metadata)
                .push_bind(credential_id);
        });
        versions.build().execute(&mut **tx).await?;
    }
    insert_bulk_chunks(tx, user_id, documents).await?;
    for batch in documents.chunks(1_000) {
        let mut changes = QueryBuilder::<Postgres>::new(
            "INSERT INTO straylight.workspace_changes \
             (user_id,entry_id,entry_version,operation,path,content_sha256) ",
        );
        changes.push_values(batch, |mut row, document| {
            row.push_bind(user_id)
                .push_bind(document.entry_id)
                .push_bind(1_i64)
                .push_bind(operation)
                .push_bind(&document.path)
                .push_bind(&document.content_sha256);
        });
        changes.build().execute(&mut **tx).await?;
    }
    for batch in documents.chunks(1_000) {
        let mut jobs =
            QueryBuilder::<Postgres>::new("INSERT INTO straylight.jobs (user_id,kind,payload) ");
        jobs.push_values(batch, |mut row, document| {
            row.push_bind(user_id)
                .push_bind("embed_entry")
                .push_bind(json!({
                    "entry_id": document.entry_id,
                    "version": 1_i64
                }));
        });
        jobs.build().execute(&mut **tx).await?;
    }
    Ok(())
}

async fn insert_bulk_chunks(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    documents: &[BulkMarkdown],
) -> ApiResult<()> {
    let rows = documents
        .iter()
        .flat_map(|document| {
            document
                .chunks
                .iter()
                .zip(&document.embeddings)
                .map(move |(chunk, embedding)| (document, chunk, embedding))
        })
        .collect::<Vec<_>>();
    for batch in rows.chunks(500) {
        let mut builder = QueryBuilder::<Postgres>::new(
            "INSERT INTO straylight.search_chunks \
             (id,user_id,entry_id,entry_version_id,chunk_index,path,heading,content,token_estimate,embedding) ",
        );
        builder.push_values(batch, |mut row, (document, chunk, embedding)| {
            row.push_bind(Uuid::now_v7())
                .push_bind(user_id)
                .push_bind(document.entry_id)
                .push_bind(document.version_id)
                .push_bind(i32::try_from(chunk.ordinal).unwrap_or(i32::MAX))
                .push_bind(&document.path)
                .push_bind(&chunk.heading)
                .push_bind(&chunk.content)
                .push_bind(i32::try_from(chunk.estimated_tokens).unwrap_or(i32::MAX))
                .push_bind((*embedding).clone());
        });
        builder.build().execute(&mut **tx).await?;
    }
    Ok(())
}

async fn apply_bulk_deltas(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    deltas: &[BulkMarkdown],
) -> ApiResult<()> {
    for delta in deltas {
        let row = sqlx::query(
            "SELECT id,current_version FROM straylight.entries \
             WHERE user_id=$1 AND lower(normalize(path, NFC))=$2 FOR UPDATE",
        )
        .bind(user_id)
        .bind(portable_path_key(&delta.path))
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else {
            insert_bulk_documents(
                tx,
                user_id,
                credential_id,
                std::slice::from_ref(delta),
                "create",
            )
            .await?;
            continue;
        };
        let entry_id: Uuid = row.get("id");
        let version = row.get::<i64, _>("current_version") + 1;
        sqlx::query(
            r#"
            INSERT INTO straylight.entry_versions (
              id,user_id,entry_id,version,content_sha256,content,size_bytes,
              metadata,created_by_credential_id
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            "#,
        )
        .bind(delta.version_id)
        .bind(user_id)
        .bind(entry_id)
        .bind(version)
        .bind(&delta.content_sha256)
        .bind(&delta.content)
        .bind(i64::try_from(delta.content.len()).unwrap_or(i64::MAX))
        .bind(json!({"kind": "evaluation_delta"}))
        .bind(credential_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE straylight.entries
            SET title=$3,media_type=$4,current_version=$5,
                updated_at=clock_timestamp(),deleted_at=NULL
            WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(user_id)
        .bind(entry_id)
        .bind(&delta.title)
        .bind(&delta.media_type)
        .bind(version)
        .execute(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM straylight.search_chunks WHERE user_id=$1 AND entry_id=$2")
            .bind(user_id)
            .bind(entry_id)
            .execute(&mut **tx)
            .await?;
        insert_chunks(
            tx,
            user_id,
            entry_id,
            delta.version_id,
            &delta.path,
            &delta.chunks,
            &delta.embeddings,
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.workspace_changes (
              user_id,entry_id,entry_version,operation,path,content_sha256
            ) VALUES ($1,$2,$3,'update',$4,$5)
            "#,
        )
        .bind(user_id)
        .bind(entry_id)
        .bind(version)
        .bind(&delta.path)
        .bind(&delta.content_sha256)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.jobs (user_id,kind,payload)
            VALUES ($1,'embed_entry',$2)
            "#,
        )
        .bind(user_id)
        .bind(json!({"entry_id": entry_id, "version": version}))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn max_generation_in_tx(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> ApiResult<i64> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(generation) FROM straylight.workspace_changes WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?
    .unwrap_or(0))
}

fn render_seed_checkpoint(
    checkpoint_id: Uuid,
    generation: i64,
    state: &Value,
    source_refs: &[String],
) -> String {
    format!(
        "---\nstraylight_kind: checkpoint\ncheckpoint_id: {checkpoint_id}\n\
         workspace_generation: {generation}\n---\n\n\
         # Seed checkpoint\n\n## State\n\n```json\n{}\n```\n\n\
         ## Source references\n\n{}\n",
        serde_json::to_string_pretty(state).unwrap_or_else(|_| "{}".to_owned()),
        if source_refs.is_empty() {
            "- None recorded.".to_owned()
        } else {
            source_refs
                .iter()
                .map(|reference| format!("- `{reference}`"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate_with_sections(id: u128, section_chars: &[usize]) -> Candidate {
        let sections = section_chars
            .iter()
            .enumerate()
            .map(|(index, chars)| CandidateSection {
                heading: format!("Section {index}"),
                excerpt: char::from(b'a' + u8::try_from(index).unwrap_or(0))
                    .to_string()
                    .repeat(*chars),
                score: 10.0 - index as f64,
            })
            .collect::<Vec<_>>();
        Candidate {
            entry_id: Uuid::from_u128(id),
            path: format!("Sources/{id}.md"),
            title: format!("Source {id}"),
            version: 1,
            content_sha256: format!("{id:064x}"),
            heading: sections
                .first()
                .map(|section| section.heading.clone())
                .unwrap_or_default(),
            excerpt: sections
                .first()
                .map(|section| section.excerpt.clone())
                .unwrap_or_default(),
            score: 10.0,
            lanes: vec!["lexical".to_owned()],
            sections,
            verbatim_matches: vec![],
            superseded_by: None,
        }
    }

    #[test]
    fn path_hints_only_return_explicit_relative_files() {
        assert_eq!(
            path_hints("Read `Trips/Europe 2026/Plan.md` and explain it"),
            ["Trips/Europe 2026/Plan.md"]
        );
        assert!(path_hints("Find the current trip plan").is_empty());
    }

    #[test]
    fn search_anchors_keep_explicit_clues_without_replaying_the_whole_task() {
        assert_eq!(
            search_anchors(
                "What is recorded for `terminal-corpus-64000-current-answer` without a path?"
            ),
            ["terminal-corpus-64000-current-answer"]
        );
        assert_eq!(
            search_anchors("Find terminal-corpus-64000-current-answer without a path"),
            ["terminal-corpus-64000-current-answer"]
        );
        assert!(
            search_anchors("Reconcile non-negotiable recovery rules and success/failure criteria")
                .is_empty()
        );
        assert!(search_anchors("Find the current trip plan").is_empty());
    }

    #[test]
    fn lexical_fallback_turns_natural_tasks_into_focused_term_pairs() {
        let fallbacks = lexical_fallback_queries(
            "Handle the request to add every confirmed Europe flight to the family calendar. \
             State what the bounded search found, whether any write is needed, and report it.",
        );
        assert!(fallbacks.contains(&"confirmed europe".to_owned()));
        assert!(fallbacks.contains(&"family calendar".to_owned()));
        assert!(
            fallbacks
                .iter()
                .all(|fallback| !fallback.contains("handle"))
        );
        assert!(
            fallbacks
                .iter()
                .all(|fallback| !fallback.contains("request"))
        );
        let focused = focused_lexical_query(
            "Handle the request to add every confirmed Europe flight to the family calendar. \
             State what the bounded search found, whether any write is needed, and report it.",
        )
        .expect("natural task should produce a focused query");
        assert!(focused.contains("confirmed europe"));
        assert!(focused.contains("family calendar"));
    }

    #[test]
    fn lexical_fallback_keeps_distinctive_terms_from_long_tasks() {
        let fallbacks = lexical_fallback_queries(
            "Resume the D1 parser performance and autonomy work. Reconcile the older 10M Nyx \
             tuning checkpoint with the later durable autonomy summary. Explain what the PVE34 \
             result established and leave a checkpoint.",
        );
        assert_eq!(fallbacks.first().map(String::as_str), Some("d1 10m"));
        assert!(
            fallbacks.contains(&"pve34 parser".to_owned()),
            "{fallbacks:?}"
        );
        assert!(fallbacks.contains(&"performance autonomy".to_owned()));
        assert!(
            fallbacks
                .iter()
                .all(|fallback| !fallback.contains("resume"))
        );
        assert!(
            fallbacks
                .iter()
                .all(|fallback| !fallback.contains("reconcile"))
        );
        let focused = focused_lexical_query(
            "Resume the D1 parser performance and autonomy work. Reconcile the older 10M Nyx \
             tuning checkpoint with the later durable autonomy summary. Explain what the PVE34 \
             result established and leave a checkpoint.",
        )
        .expect("identifier task should produce a focused query");
        assert!(focused.contains("pve34 parser"));
        assert!(focused.contains("performance autonomy"));
    }

    #[test]
    fn lexical_bonus_prefers_domain_paths_over_generic_matching_prose() {
        let task = "Resume the D1 parser performance and autonomy work. Reconcile the older 10M \
                    Nyx tuning checkpoint with the later durable autonomy summary and PVE34.";
        let warmind = lexical_candidate_bonus(
            task,
            "Projects/Warmind/D1 parser autonomy and autotuning context summary - 2026-05-25.md",
            "D1 parser autonomy and autotuning context summary",
            "Current operating model",
            "PVE34 was historical evidence rather than a fixed identity.",
        );
        let generic = lexical_candidate_bonus(
            task,
            "Projects/Straylight/Portable Personal Context Layer.md",
            "Portable Personal Context Layer",
            "Durable agent work",
            "Resume durable work from a checkpoint and preserve an operating model.",
        );
        assert!(warmind > generic + 2.0, "{warmind} should beat {generic}");

        let rupture = lexical_candidate_bonus(
            "Extend the StarRupture factory plan from current source authority.",
            "Topics/Star Rupture/Star Rupture production.md",
            "Star Rupture production",
            "Source authority",
            "Current factory planning evidence.",
        );
        assert!(rupture >= 2.0);
    }

    #[test]
    fn continuation_paths_put_recent_changes_before_checkpoint_sources() {
        let checkpoint = json!({
            "source_entries": [
                {"path": "Projects/Warmind/Rules.md"},
                {"path": "Projects/Warmind/Plan.md"}
            ]
        });
        let changes = vec![
            json!({"path": "Projects/Warmind/Rules.md"}),
            json!({"path": ".straylight/checkpoints/ignored.md"}),
            json!({"path": "Projects/Warmind/New evidence.md"}),
        ];
        let (paths, changed) = continuation_paths(Some(&checkpoint), &changes);
        assert_eq!(
            paths,
            [
                "Projects/Warmind/New evidence.md",
                "Projects/Warmind/Rules.md",
                "Projects/Warmind/Plan.md"
            ]
        );
        assert!(changed.contains(&portable_path_key("Projects/Warmind/New evidence.md")));
        assert!(changed.contains(&portable_path_key("Projects/Warmind/Rules.md")));
    }

    #[test]
    fn safe_paths_reject_traversal_and_absolute_paths() {
        assert!(validate_path("Trips/Plan.md").is_ok());
        assert!(validate_path("../Plan.md").is_err());
        assert!(validate_path("/Trips/Plan.md").is_err());
        assert!(validate_path("Trips//Plan.md").is_err());
        assert!(validate_path(r"Trips\Plan.md").is_err());
    }

    #[test]
    fn portable_path_keys_fold_unicode_and_case_without_rewriting_paths() {
        assert_eq!(
            portable_path_key("Trips/Caf\u{e9}.md"),
            portable_path_key("trips/Cafe\u{301}.md")
        );
    }

    #[test]
    fn ordinary_writes_cannot_mutate_workspace_managed_paths() {
        let ordinary = WriteRequest {
            path: ".straylight/checkpoints/not-a-checkpoint.md".to_owned(),
            content: "no".to_owned(),
            media_type: markdown_media_type(),
            expected_version: None,
            idempotency_key: None,
            metadata: json!({}),
        };
        assert!(validate_write_path(&ordinary).is_err());

        let checkpoint_id = Uuid::now_v7();
        let portable_restore = WriteRequest {
            path: format!(".straylight/checkpoints/{checkpoint_id}.md"),
            content: "checkpoint".to_owned(),
            media_type: markdown_media_type(),
            expected_version: Some(0),
            idempotency_key: None,
            metadata: json!({
                "kind": "checkpoint",
                "checkpoint_ref": format!("checkpoint:{checkpoint_id}"),
                "_straylight_import": {
                    "format": "straylight-workspace-import-manifest@v1"
                }
            }),
        };
        assert!(validate_write_path(&portable_restore).is_ok());
    }

    #[test]
    fn portable_restore_paths_require_their_own_capabilities() {
        let checkpoint_id = Uuid::now_v7();
        let auth = |capabilities: &[Capability]| AuthContext {
            credential_id: CredentialId(Uuid::now_v7()),
            user_id: UserId(Uuid::now_v7()),
            capabilities: capabilities
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect(),
            scope_refs: vec!["scope:root".to_owned()],
            read_only: false,
        };
        let save_only = auth(&[Capability::Save]);
        assert!(require_write_capabilities(&save_only, "Notes/ordinary.md").is_ok());
        assert!(
            require_write_capabilities(
                &save_only,
                &format!(".straylight/checkpoints/{checkpoint_id}.md")
            )
            .is_err()
        );
        assert!(require_write_capabilities(&save_only, ".straylight/binaries/receipt.md").is_err());
        assert!(
            require_write_capabilities(
                &auth(&[Capability::Save, Capability::Checkpoint]),
                &format!(".straylight/checkpoints/{checkpoint_id}.md")
            )
            .is_ok()
        );
        assert!(
            require_write_capabilities(
                &auth(&[Capability::Save, Capability::Stage]),
                ".straylight/binaries/receipt.md"
            )
            .is_ok()
        );
    }

    #[test]
    fn expected_binary_hashes_are_strict() {
        let hash = "a".repeat(64);
        assert_eq!(validate_sha256(&format!("sha256:{hash}")).unwrap(), hash);
        assert!(validate_sha256("not-a-hash").is_err());
    }

    #[test]
    fn checkpoint_ids_are_idempotent() {
        let request = CheckpointRequest {
            session_id: "session:one".to_owned(),
            parent_checkpoint_id: None,
            state: json!({"objective": "Continue"}),
            source_refs: vec![],
            idempotency_key: Some("same".to_owned()),
        };
        assert_eq!(
            deterministic_checkpoint_id(&request),
            deterministic_checkpoint_id(&request)
        );
    }

    #[test]
    fn workspace_envelope_omits_empty_legacy_metadata() {
        let envelope = WorkspaceEnvelope::complete(json!({"evidence": []}));
        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(
            value.get("status").and_then(Value::as_str),
            Some("complete")
        );
        for omitted in [
            "request_id",
            "session_id",
            "corpus_revision",
            "gaps",
            "freshness",
            "coverage",
            "conflicts",
            "ambiguities",
            "truncation",
        ] {
            assert!(value.get(omitted).is_none(), "{omitted} should be omitted");
        }
    }

    #[test]
    fn verbatim_matches_preserve_exact_lines_beyond_excerpt_window() {
        let identifier = "STRAYID-64000-07-deadbeef";
        let prefix = format!("{}\n", "x".repeat(2_500));
        let line = format!("literal identifier: {identifier}");
        let content = format!("{prefix}{line}\ntrailing material\n");
        let terms = verbatim_match_terms(&format!("Synthetic/records/0000007.md {identifier}"));

        assert_eq!(terms, vec![identifier]);
        let matches = extract_verbatim_matches(&content, &terms, 9, &"a".repeat(64));

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_no, 2);
        assert_eq!(matches[0].byte_start, prefix.len());
        assert_eq!(matches[0].byte_end, prefix.len() + line.len());
        assert_eq!(matches[0].text, line);
        assert_eq!(matches[0].version, 9);
        assert_eq!(
            matches[0].content_hash,
            format!("sha256:{}", "a".repeat(64))
        );
        assert!(!matches[0].truncated);
    }

    #[test]
    fn verbatim_response_budget_truncates_text_and_offsets_together() {
        let candidate = Candidate {
            entry_id: Uuid::nil(),
            path: "Synthetic/record.md".to_owned(),
            title: "Record".to_owned(),
            version: 1,
            content_sha256: "a".repeat(64),
            heading: String::new(),
            excerpt: "prefix".to_owned(),
            score: 10.0,
            lanes: vec!["exact".to_owned()],
            sections: vec![],
            verbatim_matches: vec![VerbatimMatch {
                line_no: 3,
                byte_start: 100,
                byte_end: 112,
                text: "abcdefghijkl".to_owned(),
                version: 1,
                content_hash: format!("sha256:{}", "a".repeat(64)),
                truncated: false,
            }],
            superseded_by: None,
        };
        let mut remaining = 5;

        let rendered = render_search_candidate(&candidate, &mut remaining);
        let source_match = rendered
            .get("verbatim_matches")
            .and_then(Value::as_array)
            .and_then(|matches| matches.first())
            .expect("verbatim match");

        assert_eq!(
            source_match.get("text").and_then(Value::as_str),
            Some("abcde")
        );
        assert_eq!(
            source_match.get("byte_start").and_then(Value::as_u64),
            Some(100)
        );
        assert_eq!(
            source_match.get("byte_end").and_then(Value::as_u64),
            Some(105)
        );
        assert_eq!(
            source_match.get("truncated").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(remaining, 0);
    }

    #[test]
    fn search_candidates_expose_identity_and_evidence_without_ranking_internals() {
        let candidate = Candidate {
            entry_id: Uuid::nil(),
            path: "Trips/Plan.md".to_owned(),
            title: "Trip plan".to_owned(),
            version: 4,
            content_sha256: "a".repeat(64),
            heading: "Rail".to_owned(),
            excerpt: "Take the train.".to_owned(),
            score: 12.5,
            lanes: vec!["lexical".to_owned(), "semantic".to_owned()],
            sections: vec![
                CandidateSection {
                    heading: "Rail".to_owned(),
                    excerpt: "Take the train.".to_owned(),
                    score: 12.5,
                },
                CandidateSection {
                    heading: "Backup".to_owned(),
                    excerpt: "Keep the bus as a fallback.".to_owned(),
                    score: 11.0,
                },
            ],
            verbatim_matches: vec![],
            superseded_by: None,
        };
        let mut remaining_verbatim_chars = MAX_VERBATIM_RESPONSE_CHARS;
        let rendered = render_search_candidate(&candidate, &mut remaining_verbatim_chars);
        assert_eq!(
            rendered.get("reference").and_then(Value::as_str),
            Some("entry:00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            rendered.get("excerpt").and_then(Value::as_str),
            Some("Take the train.")
        );
        assert_eq!(
            rendered
                .get("additional_sections")
                .and_then(Value::as_array)
                .and_then(|sections| sections.first())
                .and_then(|section| section.get("excerpt"))
                .and_then(Value::as_str),
            Some("Keep the bus as a fallback.")
        );
        for omitted in ["entry_id", "content_sha256", "score", "lanes"] {
            assert!(
                rendered.get(omitted).is_none(),
                "{omitted} should be omitted"
            );
        }
        let lead = render_evidence_lead(&candidate);
        assert_eq!(
            lead.get("path").and_then(Value::as_str),
            Some("Trips/Plan.md")
        );
        for omitted in ["entry_id", "content_sha256", "score", "lanes", "excerpt"] {
            assert!(
                lead.get(omitted).is_none(),
                "{omitted} should be omitted from evidence leads"
            );
        }
    }

    #[test]
    fn search_text_budget_counts_additional_sections() {
        let mut candidate = Candidate {
            entry_id: Uuid::nil(),
            path: "Trips/Plan.md".to_owned(),
            title: "Trip plan".to_owned(),
            version: 1,
            content_sha256: "a".repeat(64),
            heading: "Primary".to_owned(),
            excerpt: "1234".to_owned(),
            score: 1.0,
            lanes: vec!["lexical".to_owned()],
            sections: vec![
                CandidateSection {
                    heading: "Primary".to_owned(),
                    excerpt: "1234".to_owned(),
                    score: 1.0,
                },
                CandidateSection {
                    heading: "Second".to_owned(),
                    excerpt: "5678".to_owned(),
                    score: 0.9,
                },
                CandidateSection {
                    heading: "Third".to_owned(),
                    excerpt: "90".to_owned(),
                    score: 0.8,
                },
            ],
            verbatim_matches: vec![],
            superseded_by: None,
        };
        let mut remaining = 5;
        assert!(truncate_candidate_evidence(&mut candidate, &mut remaining));
        assert_eq!(remaining, 0);
        assert_eq!(candidate.excerpt, "1234");
        assert_eq!(candidate.sections.len(), 2);
        assert_eq!(candidate.sections[1].excerpt, "5");
    }

    #[test]
    fn fair_share_candidate_allocation_preserves_the_last_query_floor() {
        let candidate_sets = (0_u128..16)
            .map(|query| {
                (0_u128..50)
                    .map(|rank| candidate_with_sections(query * 100 + rank + 1, &[16]))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let (selected, truncated) = select_search_candidate_sets(&candidate_sets, true, 128);
        assert!(truncated);
        assert_eq!(
            selected.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![8; 16]
        );
        assert_eq!(selected[15][0].entry_id, candidate_sets[15][0].entry_id,);
    }

    #[test]
    fn fair_share_character_allocation_preserves_every_query_floor() {
        let candidate_sets = (0_u128..16)
            .map(|query| vec![candidate_with_sections(query + 1, &[2_400, 2_400, 2_400])])
            .collect::<Vec<_>>();
        let options = SearchBudgetOptions {
            fair_share: true,
            top1_hydration: false,
            char_cap: true,
            section_demotion_top_n: None,
            max_chars: 48_000,
        };
        let (views, truncated) =
            assemble_search_candidate_views(candidate_sets, &HashMap::new(), options);
        assert!(truncated);
        assert!(
            views
                .iter()
                .all(|query| { desired_search_evidence_chars(&query[0]) == 3_000 })
        );
        assert_eq!(
            views
                .iter()
                .flat_map(|query| query.iter())
                .map(desired_search_evidence_chars)
                .sum::<usize>(),
            48_000,
        );
    }

    #[test]
    fn hydration_is_batched_budgeted_and_degrades_in_request_order() {
        let candidate_sets = (0_u128..16)
            .map(|query| {
                vec![
                    candidate_with_sections(query * 10 + 1, &[2_400]),
                    candidate_with_sections(query * 10 + 2, &[2_400]),
                ]
            })
            .collect::<Vec<_>>();
        let hydration = candidate_sets
            .iter()
            .map(|query| {
                (
                    query[0].entry_id,
                    SearchHydration {
                        size_bytes: 24_000,
                        content: "x".repeat(24_000),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let options = SearchBudgetOptions {
            fair_share: true,
            top1_hydration: true,
            char_cap: true,
            section_demotion_top_n: Some(8),
            max_chars: 48_000,
        };
        let (views, truncated) =
            assemble_search_candidate_views(candidate_sets, &hydration, options);
        assert!(truncated);
        assert_eq!(
            views[0][0].representation,
            SearchRepresentation::CompleteSource,
        );
        assert_eq!(
            views[1][0].representation,
            SearchRepresentation::CompleteSource,
        );
        assert_eq!(
            views[2][0].representation,
            SearchRepresentation::PointerLead,
        );
        assert_eq!(
            views
                .iter()
                .flat_map(|query| query.iter())
                .map(|view| {
                    view.complete_text
                        .as_deref()
                        .map(str::chars)
                        .map(Iterator::count)
                        .unwrap_or_else(|| desired_search_evidence_chars(view))
                })
                .sum::<usize>(),
            48_000,
        );
        assert_eq!(
            views[0][1].representation,
            SearchRepresentation::PointerLead,
        );
    }

    #[test]
    fn section_demotion_renders_heading_leads_without_body_text() {
        let candidate_sets = vec![vec![
            candidate_with_sections(1, &[8, 8]),
            candidate_with_sections(2, &[8, 8]),
        ]];
        let options = SearchBudgetOptions {
            fair_share: false,
            top1_hydration: false,
            char_cap: true,
            section_demotion_top_n: Some(1),
            max_chars: 48_000,
        };
        let (views, truncated) =
            assemble_search_candidate_views(candidate_sets, &HashMap::new(), options);
        assert!(truncated);
        let mut remaining_verbatim_chars = MAX_VERBATIM_RESPONSE_CHARS;
        let rendered =
            render_budgeted_search_candidate(&views[0][1], &mut remaining_verbatim_chars);
        let lead = rendered["additional_sections"][0].as_object().unwrap();
        assert_eq!(
            lead.get("representation").and_then(Value::as_str),
            Some("heading_lead"),
        );
        assert!(lead.get("excerpt").is_none());
        assert_eq!(
            rendered.get("representation").and_then(Value::as_str),
            Some("excerpt"),
        );
    }

    #[test]
    fn checkpoint_budget_precedes_related_evidence() {
        let mut checkpoint = Some(json!({"text": "abcdefgh"}));
        let (truncated, evidence_tokens) = apply_checkpoint_budget(&mut checkpoint, 1);
        assert!(truncated);
        assert_eq!(evidence_tokens, 0);
        assert_eq!(
            checkpoint
                .as_ref()
                .and_then(|value| value.get("text"))
                .and_then(Value::as_str),
            Some("abcd")
        );
    }

    #[test]
    fn imported_checkpoint_metadata_rebases_and_drops_origin_entry_ids() {
        let metadata = json!({
            "_straylight_import": {
                "format": "straylight-workspace-import-manifest@v1"
            },
            "client": {
                "kind": "checkpoint",
                "workspace_generation": 1_000_000,
                "source_entries": [{
                    "entry_ref": "entry:00000000-0000-0000-0000-000000000001",
                    "path": "Trips/Plan.md"
                }]
            }
        });
        let rebased = rebase_imported_checkpoint_metadata(metadata, 42);
        assert_eq!(rebased["client"]["workspace_generation"], 42);
        assert_eq!(rebased["client"]["origin_workspace_generation"], 1_000_000);
        assert!(
            rebased["client"]["source_entries"][0]
                .get("entry_ref")
                .is_none()
        );
    }

    #[test]
    fn evaluation_batches_are_bounded_and_ordered() {
        let content = "# Fixture";
        let document = crate::eval_service::EvalDocument {
            path: "Fixture.md".to_owned(),
            content: content.to_owned(),
            content_sha256: hex::encode(Sha256::digest(content.as_bytes())),
            media_type: markdown_media_type(),
        };
        let mut request = EvalImportRequest {
            schema: "straylight-eval-import@v1".to_owned(),
            run_id: "run".to_owned(),
            case_id: "case".to_owned(),
            authorization_scope: "eval:run/case".to_owned(),
            display_scope: "Fixture".to_owned(),
            access_mode: "read_write".to_owned(),
            documents: vec![document],
            delta_documents: vec![],
            seed_checkpoint: None,
            idempotency_key: "import".to_owned(),
            batch_index: Some(1),
            batch_count: Some(3),
        };
        assert_eq!(evaluation_batch(&request).unwrap(), Some((1, 3)));
        assert!(validate_eval_import(&request).is_ok());
        request.batch_count = Some(1);
        assert!(validate_eval_import(&request).is_err());
        request.batch_index = Some(0);
        request.batch_count = None;
        assert!(validate_eval_import(&request).is_err());
    }

    #[test]
    fn idempotency_keys_are_bounded_and_printable() {
        assert!(validate_idempotency_key("capture:stable").is_ok());
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key(&"x".repeat(257)).is_err());
        assert!(validate_idempotency_key("bad\nkey").is_err());
    }
}

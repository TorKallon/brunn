//! Production Dreamer state, owner review and publication over the ordinary
//! immutable entry/version store. The runner remains an HTTP client.
use crate::{
    auth::AuthContext,
    db::AppState,
    dreamer::{
        control::{self, ControlState},
        decisions,
    },
    error::{ApiError, ApiResult},
    models::Capability,
    simple_core::{self, WriteRequest},
};
use axum::{
    Extension, Json, Router,
    extract::State,
    routing::{get, post},
};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::America::Los_Angeles;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

const STATE_PATH: &str = "dreams/state.md";
mod location_discovery;
mod narrative_discovery;
mod project_status;
mod research;
mod research_comparison;
const MAX_INPUTS: usize = 128;
const MAX_ITEMS: usize = 96;
const MAX_LEGACY_ITEMS: usize = 96;
const LEGACY_REASON: &str =
    "Retained from an earlier run; a concrete candidate is required before application.";
const MAX_STATE_BYTES: usize = 256 * 1024;
const MAX_CANDIDATE_BYTES: usize = 32 * 1024;
pub(crate) const SENSITIVE_INPUT_PATH: &str = r"(^|[/[:space:]_.-])(api[[:space:]_-]*keys?|access[[:space:]_-]*tokens?|credentials?|passwords?|secrets?|private[[:space:]_-]*keys?)([/[:space:]_.-]|$)";

pub(crate) fn sensitive_input_path(path: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::RegexBuilder::new(SENSITIVE_INPUT_PATH)
            .case_insensitive(true)
            .build()
            .expect("fixed sensitive path pattern")
    });
    PATTERN.is_match(path)
}

/// Keep historical research projections under the same source policy as live
/// research admission, including generated and evaluation-only material.
pub(crate) fn research_source_excluded(path: &str, metadata: &Value) -> bool {
    location_discovery::excluded(path, metadata)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Input {
    pub entry_ref: String,
    pub path: String,
    pub version: i64,
    pub generation: i64,
    pub operation: String,
    pub content_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub entry_ref: String,
    pub version: i64,
    pub start_line: usize,
    pub end_line: usize,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub excerpt: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub expected_version: Option<i64>,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub uncertainty: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub revises_item_id: Option<String>,
    #[serde(default)]
    pub evidence_scope: Option<Value>,
    #[serde(default)]
    pub raw_sources: Vec<crate::location::summary::RawCitation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subject_scope: Option<crate::dreamer_subject::SubjectScope>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Item {
    id: String,
    run_id: String,
    run_entry_ref: String,
    run_version: i64,
    candidate_hash: String,
    candidate: Candidate,
    status: String,
    reviewable: bool,
    #[serde(default)]
    before_md: String,
    #[serde(default)]
    published: Option<Value>,
    #[serde(default)]
    frozen_generation: i64,
    created_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Attempt {
    attempt_id: String,
    fence: String,
    date: String,
    producer_credential_id: String,
    started_at: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    frozen_generation: i64,
    mode: String,
    #[serde(default)]
    admission_hash: String,
    #[serde(default)]
    admission_version: i64,
    #[serde(default)]
    location_work: Option<Value>,
    /// Nightly project-status summary recorded by the apply endpoint under
    /// this attempt; the run record and receipt read it from here.
    #[serde(default)]
    project_status: Value,
    #[serde(default)]
    narrative_context: Vec<Input>,
    #[serde(default)]
    narrative_discovery: Option<Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct RunState {
    #[serde(default)]
    scanned_generation: i64,
    #[serde(default)]
    processed_generation: i64,
    #[serde(default)]
    inputs: Vec<Input>,
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    legacy_items: Vec<Item>,
    #[serde(default)]
    history: Vec<Value>,
    #[serde(default)]
    active: Option<Attempt>,
    #[serde(default)]
    last_attempt: Option<Value>,
    #[serde(default)]
    last_successful_run: Option<Value>,
    #[serde(default)]
    finish_response: Option<Value>,
    #[serde(default)]
    pending_notifications: Vec<Value>,
    #[serde(default)]
    next_item: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    processed_count: usize,
    #[serde(default)]
    legacy_scan_after: String,
    #[serde(default)]
    legacy_complete: bool,
    #[serde(default)]
    finish_identity: Option<Value>,
    #[serde(default)]
    candidate_submission: Option<Value>,
    #[serde(default)]
    source_dispositions: Vec<Value>,
    #[serde(default)]
    candidate_dispositions: Vec<Value>,
    #[serde(default)]
    location_work: Vec<Value>,
    #[serde(default)]
    location_scopes: Vec<Value>,
    #[serde(default)]
    location_check_cursor: usize,
    #[serde(default)]
    location_dispositions: Vec<Value>,
    #[serde(default)]
    research: research::Scheduler,
}
#[derive(Clone)]
struct Entry {
    version: i64,
    content: String,
    metadata: Value,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dreamer/review", get(review))
        .route("/dreamer/review/decisions", post(decide))
        .route("/dreamer/review/location-pilot", post(queue_location))
        .route("/workspace/dreamer/admit", post(admit))
        .route("/workspace/dreamer/checkpoint", post(checkpoint))
        .route(
            "/workspace/dreamer/location-discover",
            post(location_discovery::discover),
        )
        .route("/workspace/dreamer/candidates", post(candidates))
        .route("/workspace/dreamer/research-next", post(research::next))
        .route(
            "/workspace/dreamer/research-progress",
            post(research::progress),
        )
        .route(
            "/workspace/dreamer/narrative-discover",
            post(narrative_discovery::discover),
        )
        .route(
            "/workspace/dreamer/project-status-packet",
            post(project_status::packet),
        )
        .route(
            "/workspace/dreamer/project-status",
            post(project_status::apply),
        )
        .route("/workspace/dreamer/finish", post(finish))
}

fn runner_auth(auth: &AuthContext) -> ApiResult<AuthContext> {
    if !auth.can(Capability::DreamerRun) && !auth.can(Capability::CredentialManage) {
        return Err(ApiError::capability("dreamer:run"));
    }
    Ok(auth.clone())
}
async fn begin_runner_write<'a>(
    state: &'a AppState,
    auth: &AuthContext,
) -> ApiResult<Transaction<'a, Postgres>> {
    // Authenticate the stored capabilities and scope grants first. Additional
    // workspace authority exists only in this server-owned transaction, after
    // the Dreamer endpoint has checked its dedicated capability. HTTP callers
    // cannot request this context through general workspace endpoints.
    runner_auth(auth)?;
    let mut tx = state.begin_write(auth).await?;
    let mut internal = auth.clone();
    internal.capabilities.insert("read".into());
    internal.capabilities.insert("save".into());
    sqlx::query("SELECT set_config('app.capabilities',$1,true)")
        .bind(internal.capability_guc())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
fn conflict(message: &str, version: i64) -> ApiError {
    ApiError::conflict(
        "dreamer_state_conflict",
        message,
        json!({"actual_version":version}),
    )
}
fn string<'a>(v: &'a Value, key: &str) -> ApiResult<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::invalid(format!("{key} is required")))
}
fn integer(v: &Value, key: &str) -> ApiResult<i64> {
    v.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::invalid(format!("{key} is required")))
}
fn entry_id(reference: &str) -> ApiResult<Uuid> {
    reference
        .strip_prefix("entry:")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError::invalid("an exact entry reference is required"))
}
fn digest(value: &impl Serialize) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(value).expect("serializable contract"),
    ))
}
async fn load_entry(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    path: &str,
) -> ApiResult<Option<Entry>> {
    let row = sqlx::query("SELECT e.id,e.path,e.current_version,v.content,v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.path=$2 AND e.deleted_at IS NULL")
        .bind(user).bind(path).fetch_optional(&mut **tx).await?;
    Ok(row.map(|r| Entry {
        version: r.get("current_version"),
        content: r.get("content"),
        metadata: r.get("metadata"),
    }))
}
async fn load_state(tx: &mut Transaction<'_, Postgres>, user: Uuid) -> ApiResult<(RunState, i64)> {
    match load_entry(tx, user, STATE_PATH).await? {
        None => Ok((RunState::default(), 0)),
        Some(e) => {
            let value = e.metadata.get("dreamer_state").ok_or_else(|| {
                ApiError::invalid(
                    "Dreamer state metadata is missing; retained state requires recovery",
                )
            })?;
            let mut data: RunState = serde_json::from_value(value.clone()).map_err(|_| {
                ApiError::invalid("Dreamer state is invalid; refusing to reset progress")
            })?;
            separate_legacy_history(&mut data)?;
            let mut audits = std::collections::BTreeMap::<(String, i64), Vec<Item>>::new();
            for item in &mut data.items {
                if !item.reviewable || item.run_version == 0 {
                    continue;
                }
                let key = (item.run_entry_ref.clone(), item.run_version);
                if !audits.contains_key(&key) {
                    let metadata:Value=sqlx::query_scalar("SELECT v.metadata FROM brunn.entry_versions v JOIN brunn.entries e ON e.user_id=v.user_id AND e.id=v.entry_id WHERE v.user_id=$1 AND v.entry_id=$2 AND v.version=$3 AND e.deleted_at IS NULL")
                        .bind(user).bind(entry_id(&key.0)?).bind(key.1).fetch_optional(&mut **tx).await?
                        .ok_or_else(||ApiError::invalid("Retained proposal audit is unavailable; refusing to discard it"))?;
                    let items: Vec<Item> =
                        serde_json::from_value(metadata["dreamer_run"]["items"].clone())
                            .map_err(|_| ApiError::invalid("Retained proposal audit is invalid"))?;
                    audits.insert(key.clone(), items);
                }
                let audit = audits[&key]
                    .iter()
                    .find(|a| a.id == item.id && a.candidate_hash == item.candidate_hash)
                    .ok_or_else(|| {
                        ApiError::invalid(
                            "Retained proposal identity is missing from its exact audit version",
                        )
                    })?;
                item.candidate = audit.candidate.clone();
                item.before_md = audit.before_md.clone();
            }
            Ok((data, e.version))
        }
    }
}
async fn put_entry(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    path: &str,
    content: String,
    metadata: Value,
    expected: i64,
) -> ApiResult<Value> {
    let mut prepared = simple_core::prepare_dreamer_markdown(
        state,
        WriteRequest {
            path: path.into(),
            content,
            media_type: "text/markdown".into(),
            expected_version: Some(expected),
            idempotency_key: None,
            metadata: Value::Null,
        },
    )
    .await?;
    prepared.metadata = metadata;
    prepared.force_new_version = true;
    let hash = prepared.content_sha256.clone();
    let written = simple_core::upsert_markdown_in_tx(
        tx,
        auth.user_id.0,
        Some(auth.credential_id.0),
        prepared,
    )
    .await?;
    Ok(
        json!({"entry_ref":format!("entry:{}",written.entry_id),"path":path,"version":written.version,"content_hash":format!("sha256:{hash}"),"generation":written.generation}),
    )
}
async fn save_state(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    version: i64,
) -> ApiResult<i64> {
    let mut compact = data.clone();
    for item in &mut compact.items {
        if item.reviewable && item.run_version > 0 {
            item.candidate.content = None;
            item.candidate.raw_sources.clear();
            item.before_md.clear();
            item.candidate.sources.clear();
            item.candidate.subject_scope = None;
            item.candidate.summary.clear();
            item.candidate.reason.clear();
            item.candidate.uncertainty.clear();
            item.candidate.question.clear();
        }
    }
    // Original untyped run versions remain the text source. Review restores
    // their full text only after checking exact-version ownership/visibility.
    for item in &mut compact.legacy_items {
        item.candidate.summary.clear();
        item.candidate.question.clear();
    }
    let metadata = json!({"kind":"dreamer_state","dreamer_state":compact});
    if serde_json::to_vec(&metadata)?.len() > MAX_STATE_BYTES {
        return Err(ApiError::invalid(
            "Dreamer retained state is full; no work or cursor was discarded",
        ));
    }
    let text = format!(
        "# Dreamer progress\n\nScanned generation: {}\nProcessed generation: {}\nRetained inputs: {}\nReview items: {}\nHistorical notes: {}\n\n[Open Review](https://brunn.ai/dreams) for decisions. Immutable run versions retain the audit.\n",
        data.scanned_generation,
        data.processed_generation,
        data.inputs.len(),
        data.items.len(),
        data.legacy_items.len()
    );
    let receipt = put_entry(state, tx, auth, STATE_PATH, text, metadata, version).await?;
    Ok(receipt["version"].as_i64().expect("version"))
}
async fn mode(tx: &mut Transaction<'_, Postgres>, user: Uuid) -> ApiResult<Option<String>> {
    let control = load_entry(tx, user, "dreams/CONTROL.md").await?;
    Ok(
        match control::parse(control.as_ref().map(|e| e.content.as_str())) {
            ControlState::Enabled(c) => Some(c.mode.as_str().into()),
            _ => None,
        },
    )
}
fn active(data: &RunState, body: &Value, auth: &AuthContext, version: i64) -> ApiResult<Attempt> {
    if integer(body, "expected_state_version")? != version {
        return Err(conflict(
            "Review or run state changed; reload before retrying",
            version,
        ));
    }
    let a = data
        .active
        .as_ref()
        .ok_or_else(|| conflict("No active Dreamer attempt", version))?;
    if a.attempt_id != string(body, "attempt_id")?
        || a.fence != string(body, "fence")?
        || a.producer_credential_id != auth.credential_id.0.to_string()
        || a.lease_until < Utc::now()
    {
        return Err(conflict(
            "The Dreamer attempt was superseded or its lease expired",
            version,
        ));
    }
    Ok(a.clone())
}
fn pending(item: &Item) -> bool {
    !matches!(item.status.as_str(), "rejected" | "applied" | "superseded")
}
fn is_legacy_history(item: &Item) -> bool {
    !item.reviewable
        && item.frozen_generation == 0
        && item.candidate.reason == LEGACY_REASON
        && matches!(item.candidate.kind.as_str(), "legacy" | "question")
}

fn separate_legacy_history(data: &mut RunState) -> ApiResult<()> {
    let migrating = data
        .items
        .iter()
        .filter(|item| is_legacy_history(item))
        .count();
    let mut identities = std::collections::BTreeSet::new();
    if data.legacy_items.len() + migrating > MAX_LEGACY_ITEMS
        || data.items.len() - migrating > MAX_ITEMS
        || data
            .legacy_items
            .iter()
            .any(|item| !is_legacy_history(item))
        || data
            .items
            .iter()
            .chain(&data.legacy_items)
            .any(|item| !identities.insert(&item.id))
    {
        return Err(ApiError::invalid(
            "Dreamer retained history is invalid or full; no item or progress was discarded",
        ));
    }
    let (historical, current) = std::mem::take(&mut data.items)
        .into_iter()
        .partition(is_legacy_history);
    data.items = current;
    data.legacy_items.extend(historical);
    Ok(())
}
fn counts(data: &RunState) -> Value {
    json!({"retained":data.inputs.len(),"processed":data.processed_count,"processed_generation":data.processed_generation,"pending":data.items.iter().filter(|i|pending(i)).count(),"proposals":data.items.iter().filter(|i|pending(i)&&i.candidate.kind!="question").count(),"questions":data.items.iter().filter(|i|pending(i)&&i.candidate.kind=="question").count(),"approved_held":data.items.iter().filter(|i|i.status=="approved_held").count(),"applied":data.items.iter().filter(|i|i.status=="applied").count(),"published":data.items.iter().filter(|i|i.status=="applied").count(),"legacy":data.legacy_items.len(),"legacy_backlog":!data.legacy_complete,"retained_location_days":data.location_work.len(),"research_turns":data.research.service_sequence,"research_completed":data.research.completed})
}
fn next_run(now: DateTime<Utc>) -> DateTime<Utc> {
    let local = now.with_timezone(&Los_Angeles);
    let mut date = local.date_naive();
    let hour = crate::dreamer::http::NIGHTLY_HOUR;
    let time = date.and_hms_opt(hour, 0, 0).expect("nightly hour");
    let today = Los_Angeles
        .from_local_datetime(&time)
        .earliest()
        .expect("nightly hour exists");
    if today <= local {
        date = date.succ_opt().expect("next date");
    }
    Los_Angeles
        .from_local_datetime(&date.and_hms_opt(hour, 0, 0).expect("nightly hour"))
        .earliest()
        .expect("nightly hour exists")
        .with_timezone(&Utc)
}

enum SourceSelectorPolicy {
    Strict,
    CheckpointEndOfDocument,
}

pub(crate) async fn source_versions(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    sources: &mut [Source],
    frozen: i64,
    require_current: bool,
) -> ApiResult<()> {
    source_versions_with_policy(
        tx,
        user,
        sources,
        frozen,
        require_current,
        SourceSelectorPolicy::Strict,
    )
    .await
}

async fn source_versions_with_policy(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    sources: &mut [Source],
    frozen: i64,
    require_current: bool,
    policy: SourceSelectorPolicy,
) -> ApiResult<()> {
    if sources.len() > 64 {
        return Err(ApiError::invalid(
            "a candidate may reference at most 64 source versions",
        ));
    }
    for source in sources {
        let id = entry_id(&source.entry_ref)?;
        if source.version < 1 || source.start_line < 1 || source.end_line < source.start_line {
            return Err(ApiError::invalid(
                "source versions and line selectors must be positive and ordered",
            ));
        }
        let row=sqlx::query("SELECT e.path,e.current_version,v.content,v.metadata,head.metadata AS current_metadata,EXISTS(SELECT 1 FROM brunn.workspace_changes c WHERE c.user_id=e.user_id AND c.entry_id=e.id AND c.entry_version=v.version AND c.generation<=$4) AS in_snapshot FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3 LEFT JOIN brunn.entry_versions head ON head.user_id=e.user_id AND head.entry_id=e.id AND head.version=e.current_version WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL")
            .bind(user).bind(id).bind(source.version).bind(frozen).fetch_optional(&mut **tx).await?
            .ok_or_else(|| ApiError::invalid("a candidate source is missing or inaccessible"))?;
        if !row.get::<bool, _>("in_snapshot") {
            return Err(ApiError::invalid(
                "candidate source was not in the frozen input snapshot",
            ));
        }
        if require_current && row.get::<i64, _>("current_version") != source.version {
            return Err(ApiError::conflict(
                "dreamer_source_changed",
                "candidate evidence changed; retain and recompile it",
                json!({"entry_ref":source.entry_ref}),
            ));
        }
        if crate::dreamer_summary::generated_briefing_metadata(&row.get::<Value, _>("metadata"))
            || row
                .get::<Option<Value>, _>("current_metadata")
                .as_ref()
                .is_some_and(crate::dreamer_summary::generated_briefing_metadata)
        {
            return Err(ApiError::invalid(
                "generated briefing editions cannot be source evidence",
            ));
        }
        let content: Option<String> = row.get("content");
        let content = content
            .ok_or_else(|| ApiError::invalid("candidate sources must be readable Markdown"))?;
        let lines: Vec<_> = content.lines().collect();
        // Check the original request before normalization: an EOF overshoot
        // cannot turn an empty, out-of-range, or oversized request into evidence.
        if source.start_line > lines.len()
            || source.end_line - source.start_line > 400
            || matches!(policy, SourceSelectorPolicy::Strict) && source.end_line > lines.len()
        {
            return Err(ApiError::invalid(
                "source selector is outside its exact source version or exceeds 400 lines",
            ));
        }
        if matches!(policy, SourceSelectorPolicy::CheckpointEndOfDocument) {
            source.end_line = source.end_line.min(lines.len());
        }
        source.path = row.get("path");
        if source.path.starts_with("dreams/")
            || source.path.starts_with("derived/")
            || source.path.starts_with(".brunn/")
            || source.path == "private/dreamer.md"
            || source.path.starts_with("agent-memory/")
        {
            return Err(ApiError::invalid(
                "generated Dreamer output cannot be its own source evidence",
            ));
        }
        source.excerpt = lines[source.start_line - 1..source.end_line].join("\n");
        // A coherent source (for example a small CSV) may use most of the
        // candidate budget. The complete hydrated candidate is checked below;
        // an extra 12 KB per-source limit rejected otherwise valid summaries.
        if source.excerpt.len() > MAX_CANDIDATE_BYTES {
            return Err(ApiError::invalid(
                "source excerpt exceeds the 32 KiB candidate budget",
            ));
        }
    }
    Ok(())
}

fn related_block_range(text: &str) -> ApiResult<Option<std::ops::Range<usize>>> {
    let mut found = None;
    let mut start = None;
    let mut offset = 0;
    let mut fence = None;
    let mut frontmatter = None;
    let mut comment = false;
    for line in text.split_inclusive('\n') {
        let raw = line.trim_end_matches(['\r', '\n']);
        if offset == 0 && matches!(raw, "---" | "+++") {
            frontmatter = Some(raw);
            offset += line.len();
            continue;
        }
        if frontmatter.is_some() {
            if frontmatter == Some(raw) {
                frontmatter = None;
            }
            offset += line.len();
            continue;
        }
        if fence.is_none() && (comment || raw.contains("<!--")) {
            comment = raw
                .rfind("-->")
                .is_none_or(|end| raw.rfind("<!--").is_some_and(|start| start > end));
            offset += line.len();
            continue;
        }
        let heading = raw.trim_start_matches(' ');
        let marker = legacy_fence_opening(raw);
        if let Some((character, length)) = fence {
            if marker.is_some_and(|(c, n)| c == character && n >= length)
                && raw.trim().chars().all(|c| c == character)
            {
                fence = None;
            }
        } else if marker.is_some() {
            fence = marker;
        } else if raw.len() - heading.len() <= 3
            && (heading.starts_with("# ") || heading.starts_with("## "))
        {
            if let Some(begin) = start.take() {
                found = Some(begin..offset);
            }
            if heading.trim_end() == "## Related" {
                if found.is_some() {
                    return Err(ApiError::invalid(
                        "multiple managed Related sections are ambiguous",
                    ));
                }
                start = Some(offset);
            }
        }
        offset += line.len();
    }
    if frontmatter.is_some() || comment || fence.is_some() {
        return Err(ApiError::invalid(
            "unclosed source markup prevents a managed Related edit",
        ));
    }
    Ok(start.map(|begin| begin..text.len()).or(found))
}

fn related_without_block(text: &str) -> ApiResult<String> {
    Ok(match related_block_range(text)? {
        Some(range) => format!("{}{}", &text[..range.start], &text[range.end..]),
        None => text.to_owned(),
    }
    .trim_end()
    .to_owned())
}

/// The model selects links. The server assembles the reviewable source-note
/// preview from its exact current version, preserving all owner prose.
fn compile_related_candidate(candidate: &mut Candidate, before: &str) -> ApiResult<()> {
    if candidate.kind != "related" {
        return Ok(());
    }
    let content = candidate.content.as_deref().unwrap_or("").trim();
    if content.is_empty()
        || !content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .all(|line| line.trim().starts_with("- [[") && line.trim().ends_with("]]"))
    {
        // Existing complete-note candidates still pass the same body guard.
        return Ok(());
    }
    if !candidate.sources.iter().any(|source| {
        Some(source.path.as_str()) == candidate.path.as_deref()
            && Some(source.version) == candidate.expected_version
    }) {
        return Err(ApiError::invalid(
            "Related changes require the exact destination source in their evidence",
        ));
    }
    let block = format!("## Related\n\n{content}\n\n");
    candidate.content = Some(match related_block_range(before)? {
        Some(range) => format!(
            "{}{}{}",
            &before[..range.start],
            block,
            &before[range.end..]
        ),
        None => format!(
            "{before}{}{block}",
            if before.ends_with("\n\n") {
                ""
            } else if before.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            }
        ),
    });
    Ok(())
}

#[cfg(test)]
mod related_candidate_tests {
    use super::*;

    fn links() -> Candidate {
        serde_json::from_value(json!({
            "kind":"related","title":"Connect the project notes",
            "path":"sources/Original.md","expected_version":1,
            "content":"- [[sources/Target.md]]",
            "sources":[
                {"entry_ref":format!("entry:{}",Uuid::now_v7()),"version":1,"start_line":1,"end_line":1,"path":"sources/Original.md"},
                {"entry_ref":format!("entry:{}",Uuid::now_v7()),"version":1,"start_line":1,"end_line":1,"path":"sources/Target.md"}
            ]
        })).unwrap()
    }

    #[test]
    fn compact_links_preserve_prose_and_fenced_examples_and_accept_md_paths() {
        let prefix = "---\nexample: |\n  ## Related\n  Preserve metadata.\n---\n# Original\n\nOwner’s exact text.\n\n<!--\n## Related\nPreserve comment.\n-->\n\n```markdown\n## Related\nExample only.\n```\n\n";
        let suffix = "# Another section\n\nMore owner text.\n";
        let before = format!("{prefix}## Related\n\n- [[Old]]\n\n{suffix}");
        let mut candidate = links();
        compile_related_candidate(&mut candidate, &before).unwrap();
        assert_eq!(
            candidate.content.as_deref(),
            Some(format!("{prefix}## Related\n\n- [[sources/Target.md]]\n\n{suffix}").as_str())
        );
        validate_candidate(&candidate, &before).unwrap();
        candidate.content = candidate
            .content
            .map(|body| body.replace("Owner’s exact text.", "Changed text."));
        assert!(validate_candidate(&candidate, &before).is_err());
    }

    #[test]
    fn adding_a_block_preserves_a_note_without_a_trailing_newline() {
        let before = "# Original\n\nOriginal text without a final newline.";
        let mut candidate = links();
        compile_related_candidate(&mut candidate, before).unwrap();
        assert!(candidate.content.as_ref().unwrap().starts_with(before));
        validate_candidate(&candidate, before).unwrap();
    }

    #[test]
    fn related_links_cannot_bypass_exact_targets_or_ambiguous_sections() {
        let mut missing_destination = links();
        missing_destination.sources.remove(0);
        assert!(compile_related_candidate(&mut missing_destination, "# Original").is_err());
        let mut undeclared = links();
        undeclared.content = Some("- [[sources/Unadmitted.md]]".into());
        compile_related_candidate(&mut undeclared, "# Original").unwrap();
        assert!(validate_candidate(&undeclared, "# Original").is_err());
        let mut ambiguous = links();
        assert!(
            compile_related_candidate(
                &mut ambiguous,
                "# Original\n## Related\n- [[First]]\n## Related\n- [[Second]]"
            )
            .is_err()
        );
    }
}

pub(crate) fn validate_candidate(candidate: &Candidate, before: &str) -> ApiResult<()> {
    if candidate.title.is_empty()
        || candidate.title.len() > 300
        || candidate.summary.len() > 1000
        || candidate.reason.len() > 2000
        || candidate.uncertainty.len() > 2000
    {
        return Err(ApiError::invalid(
            "candidate title, summary or explanation exceeds its bound",
        ));
    }
    let mut claim_candidate = candidate.clone();
    claim_candidate.subject_scope = None;
    if candidate.subject_scope.as_ref().is_some_and(|scope| {
        serde_json::to_vec(scope).map_or(true, |bytes| bytes.len() > 128 * 1024)
    }) {
        return Err(ApiError::invalid(
            "subject dependency manifest exceeds 128 KiB",
        ));
    }
    if serde_json::to_vec(&claim_candidate)?.len() > MAX_CANDIDATE_BYTES {
        return Err(ApiError::invalid(
            "candidate exceeds 32 KiB; split its scope without discarding evidence",
        ));
    }
    if candidate.evidence_scope.is_none() && !candidate.raw_sources.is_empty() {
        return Err(ApiError::invalid(
            "raw citations require closed-day location evidence",
        ));
    }
    if candidate.kind == "question" {
        return Ok(());
    }
    let path = candidate
        .path
        .as_deref()
        .ok_or_else(|| ApiError::invalid("candidate output path is required"))?;
    let content = candidate
        .content
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::invalid("review requires actual candidate content"))?;
    if candidate.expected_version.is_none()
        || (candidate.sources.is_empty() && candidate.raw_sources.is_empty())
    {
        return Err(ApiError::invalid(
            "candidate needs expected output version and exact supporting sources",
        ));
    }
    if path.contains("..") || !path.ends_with(".md") || path.contains('\\') {
        return Err(ApiError::invalid("invalid candidate destination"));
    }
    match candidate.kind.as_str() {
        "summary" => {
            let markers = regex::Regex::new(r"\[\^([sr])([0-9]+)\]").expect("fixed citation regex");
            if markers.captures_iter(content).any(|capture| {
                let index = capture[2].parse::<usize>().unwrap_or(usize::MAX);
                let count = if &capture[1] == "s" {
                    candidate.sources.len()
                } else {
                    candidate.raw_sources.len()
                };
                index == 0 || index > count || capture[2] != index.to_string()
            }) {
                return Err(ApiError::invalid(
                    "summary contains an undeclared citation marker",
                ));
            }
            let cited_inline = |marker: &str| {
                content.lines().map(str::trim).any(|line| {
                    !line.starts_with('#')
                        && !line.starts_with("[^s")
                        && !line.starts_with("[^r")
                        && line.contains(marker)
                })
            };
            if !candidate.uncertainty.trim().is_empty()
                && !content.contains(candidate.uncertainty.trim())
            {
                return Err(ApiError::invalid(
                    "summary uncertainty must appear verbatim in the proposed content so publication preserves its caveats",
                ));
            }
            if candidate
                .raw_sources
                .iter()
                .enumerate()
                .any(|(index, _)| !cited_inline(&format!("[^r{}]", index + 1)))
            {
                return Err(ApiError::invalid(
                    "every declared raw source must be cited in the proposed summary content",
                ));
            }
            if candidate.evidence_scope.is_some()
                && candidate.sources.iter().enumerate().any(|(index, source)| {
                    !source.path.starts_with("Location/Visits/")
                        && !cited_inline(&format!("[^s{}]", index + 1))
                })
            {
                return Err(ApiError::invalid(
                    "every non-inventory canonical location source must be cited in the proposed summary content",
                ));
            }
            if path.starts_with("derived/location/") && candidate.evidence_scope.is_none() {
                return Err(ApiError::invalid(
                    "historical location summaries require a validated closed-day evidence scope",
                ));
            }
            if !path.starts_with("derived/entities/") && !path.starts_with("derived/location/") {
                return Err(ApiError::invalid(
                    "summaries may publish only to managed derived summary paths",
                ));
            }
            let lines: Vec<_> = content.lines().map(str::trim).collect();
            for (index, line) in lines.iter().enumerate().filter(|(index, line)| {
                !(line.is_empty()
                    || line.starts_with('#')
                    || summary_table_structure(&lines, *index)
                    || candidate.evidence_scope.is_some()
                        && **line == "Times are approximate observation windows.")
            }) {
                if line.starts_with("[^s")
                    || line.starts_with("[^r")
                    || !candidate
                        .sources
                        .iter()
                        .enumerate()
                        .any(|(i, _)| line.contains(&format!("[^s{}]", i + 1)))
                        && !candidate
                            .raw_sources
                            .iter()
                            .enumerate()
                            .any(|(i, _)| line.contains(&format!("[^r{}]", i + 1)))
                {
                    return Err(ApiError::invalid(format!(
                        "summary line {} needs a declared [^sN] or [^rN] citation; footnotes are rendered by the server",
                        index + 1
                    )));
                }
            }
        }
        "related" => {
            let lower = path.to_ascii_lowercase();
            if !path.starts_with("sources/")
                || lower.ends_with("agents.md")
                || lower.ends_with("soul.md")
                || lower.contains("preferences")
                || related_block_range(content)?.is_none()
                || related_without_block(before)? != related_without_block(content)?
            {
                return Err(ApiError::invalid(
                    "Related changes may alter only the managed ## Related block of a source note",
                ));
            }
            let range = related_block_range(content)?.expect("validated Related section");
            let block = content[range]
                .split_once('\n')
                .map_or("", |(_, block)| block);
            if block.matches("[[").count() > 8 {
                return Err(ApiError::invalid(
                    "Related blocks are limited to eight links",
                ));
            }
            for line in block.lines().map(str::trim).filter(|s| !s.is_empty()) {
                let target = line
                    .strip_prefix("- [[")
                    .and_then(|s| s.strip_suffix("]]"))
                    .ok_or_else(|| {
                        ApiError::invalid(
                            "Related blocks contain only evidence-backed wiki-link bullets",
                        )
                    })?;
                let target = target
                    .split('|')
                    .next()
                    .unwrap_or(target)
                    .trim_end_matches(".md");
                if !candidate.sources.iter().any(|s| {
                    s.path.trim_end_matches(".md") == target
                        || s.path
                            .trim_start_matches("sources/")
                            .trim_end_matches(".md")
                            == target
                }) {
                    return Err(ApiError::invalid(
                        "Related link target is absent from the exact supporting sources",
                    ));
                }
            }
        }
        _ => return Err(ApiError::invalid("unsupported candidate kind")),
    }
    Ok(())
}
fn summary_table_structure(lines: &[&str], index: usize) -> bool {
    fn cells(line: &str) -> Option<Vec<&str>> {
        (line.starts_with('|') && line.ends_with('|'))
            .then(|| line.trim_matches('|').split('|').map(str::trim).collect())
    }
    // Like Markdown headings, table headings are presentation. Only a header
    // directly followed by its matching delimiter row gets this exemption;
    // every data row still needs evidence, for ordinary and location summaries.
    let pair = |header: &str, delimiter: &str| {
        let (Some(header), Some(delimiter)) = (cells(header), cells(delimiter)) else {
            return false;
        };
        header.len() >= 2
            && header.len() == delimiter.len()
            && header.iter().all(|cell| !cell.is_empty())
            && delimiter.iter().all(|cell| {
                cell.trim_matches(':').len() >= 3
                    && cell.trim_matches(':').chars().all(|c| c == '-')
            })
    };
    lines
        .get(index + 1)
        .is_some_and(|next| pair(lines[index], next))
        || index > 0 && pair(lines[index - 1], lines[index])
}

#[cfg(test)]
mod summary_structure_tests {
    use super::*;

    #[test]
    fn ordinary_tables_allow_headings_but_require_cited_data_rows() {
        let mut candidate: Candidate = serde_json::from_value(json!({
            "kind":"summary","title":"Compare recorded experiments",
            "path":"derived/entities/experiments.md","expected_version":0,
            "sources":[{"entry_ref":format!("entry:{}",Uuid::now_v7()),
                "version":1,"start_line":1,"end_line":4}],
            "content":"# Recorded experiments\n\n| Element | First experiment | Second experiment |\n| --- | :---: | ---: |\n| Material | Oak.[^s1] | Birch.[^s1] |"
        })).unwrap();
        validate_candidate(&candidate, "").unwrap();
        candidate.content = candidate.content.map(|c| c.replace("[^s1]", ""));
        assert!(validate_candidate(&candidate, "").is_err());
    }

    #[test]
    fn unmatched_or_detached_table_rows_do_not_exempt_uncited_claims() {
        for body in [
            "| Fact | Value |\n| Recorded | Uncited |",
            "| Fact | Value |\n\n| --- | --- |",
            "| Fact | Value |\n| --- | --- | --- |",
            "| Fact | Value |\n| --- | --- |\n| Recorded | Uncited |",
        ] {
            let candidate: Candidate = serde_json::from_value(json!({
                "kind":"summary","title":"Unsupported table",
                "path":"derived/entities/unsupported.md","expected_version":0,
                "sources":[{"entry_ref":format!("entry:{}",Uuid::now_v7()),
                    "version":1,"start_line":1,"end_line":4}],"content":body
            }))
            .unwrap();
            assert!(validate_candidate(&candidate, "").is_err(), "{body}");
        }
    }
}

/// Citation selectors remain on the immutable candidate and in the published
/// manifest. The primary location representation contains only readable prose.
fn location_primary_content(candidate: &Candidate) -> (String, Value) {
    let marker = regex::Regex::new(r"\[\^([sr])([1-9][0-9]*)\]").expect("fixed citation regex");
    let mut claims = Vec::new();
    let content = candidate.content.as_deref().unwrap_or("");
    let body = content
        .lines()
        .enumerate()
        .map(|(line_number, line)| {
            let mut sources = std::collections::BTreeSet::new();
            let rendered = marker.replace_all(line, |capture: &regex::Captures<'_>| {
                let index = capture[2].parse::<usize>().unwrap_or(usize::MAX);
                let count = if &capture[1] == "s" {
                    candidate.sources.len()
                } else {
                    candidate.raw_sources.len()
                };
                if index <= count {
                    sources.insert(format!("{}{}", &capture[1], index));
                    String::new()
                } else {
                    capture[0].to_owned()
                }
            });
            if !sources.is_empty() {
                claims.push(json!({"line":line_number + 1,"sources":sources}));
            }
            rendered.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (body, json!(claims))
}

fn candidate_body(candidate: &Candidate) -> String {
    if candidate.kind == "summary" && candidate.evidence_scope.is_some() {
        return location_primary_content(candidate).0;
    }
    let mut body = candidate
        .content
        .clone()
        .unwrap_or_else(|| candidate.question.clone());
    if candidate.kind == "summary" {
        body.push_str("\n\n");
        for (i, s) in candidate.sources.iter().enumerate() {
            body.push_str(&format!(
                "[^s{}]: {} — {} v{}, lines {}–{}.\n",
                i + 1,
                s.path,
                s.entry_ref,
                s.version,
                s.start_line,
                s.end_line
            ));
        }
    }
    if candidate.kind == "summary" {
        for (n, source) in candidate.raw_sources.iter().enumerate() {
            body.push_str(&format!("[^r{}]: Retained location report {} — fields {}. Raw evidence expires after 30 days.\n",n+1,source.natural_key,source.fields.join(", ")));
        }
    }
    body
}
fn input_excluded(path: &str, metadata: &Value) -> bool {
    sensitive_input_path(path)
        || path.starts_with("dreams/")
        || path.starts_with("derived/")
        || path.starts_with(".brunn/")
        || path == "private/dreamer.md"
        || path.starts_with("agent-memory/")
        || path.starts_with("Evidence/Location/")
        || path == "Location/Places.md"
        || path.starts_with("Location/Visits/")
        || matches!(
            metadata["kind"].as_str(),
            Some("location-places" | "location-visits")
        )
        || crate::dreamer_summary::generated_briefing_metadata(metadata)
}
async fn retain_inputs(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    data: &mut RunState,
    upper: i64,
) -> ApiResult<()> {
    // Re-evaluate retained headers after policy upgrades without reading the
    // excluded bodies or pretending the model reasoned about them.
    if data.inputs.len() > MAX_INPUTS {
        return Err(ApiError::invalid(
            "retained input count exceeds its supported bound",
        ));
    }
    let mut generated_briefings = std::collections::BTreeSet::new();
    if !data.inputs.is_empty() {
        let ids = data
            .inputs
            .iter()
            .map(|input| entry_id(&input.entry_ref))
            .collect::<ApiResult<Vec<_>>>()?;
        let versions = data
            .inputs
            .iter()
            .map(|input| input.version)
            .collect::<Vec<_>>();
        let rows = sqlx::query(r#"
            SELECT retained.entry_id,exact.metadata,head.metadata AS current_metadata
            FROM unnest($2::uuid[],$3::bigint[]) AS retained(entry_id,version)
            LEFT JOIN LATERAL (
                SELECT jsonb_build_object('kind',metadata->'kind') AS metadata FROM brunn.entry_versions
                WHERE user_id=$1 AND entry_id=retained.entry_id AND version=retained.version LIMIT 1
            ) exact ON true
            LEFT JOIN brunn.entries e ON e.user_id=$1 AND e.id=retained.entry_id
            LEFT JOIN LATERAL (
                SELECT jsonb_build_object('kind',metadata->'kind') AS metadata FROM brunn.entry_versions
                WHERE user_id=$1 AND entry_id=retained.entry_id AND version=e.current_version LIMIT 1
            ) head ON true
        "#).bind(user).bind(ids).bind(versions).fetch_all(&mut **tx).await?;
        for row in rows {
            if ["metadata", "current_metadata"].iter().any(|column| {
                row.get::<Option<Value>, _>(*column)
                    .as_ref()
                    .is_some_and(crate::dreamer_summary::generated_briefing_metadata)
            }) {
                generated_briefings.insert(format!("entry:{}", row.get::<Uuid, _>("entry_id")));
            }
        }
    }
    data.inputs.retain(|input| {
        if sensitive_input_path(&input.path) {
            data.source_dispositions.push(json!({"entry_ref":input.entry_ref,"version":input.version,"generation":input.generation,
                "disposition":"excluded_credential_record","detail":"Credential records are excluded from automatic synthesis."}));
            false
        } else if generated_briefings.contains(&input.entry_ref) {
            data.source_dispositions.push(json!({"entry_ref":input.entry_ref,"version":input.version,"generation":input.generation,
                "disposition":"excluded_generated_briefing","detail":"Generated briefing editions are excluded from automatic synthesis; this is not model processing."}));
            false
        } else { true }
    });
    // RLS can substantially underestimate rows. Bound the ordered page before
    // any version joins, then resolve each distinct entry only once. Without
    // these fences the planner can put LIMIT after whole-corpus nested loops.
    // Keep the final exact version lookup lateral as well: with cold import
    // statistics RLS estimates one visible row and otherwise repeatedly scans
    // every version for the owner before applying the entry/version join filter.
    let rows = sqlx::query(
        r#"
        WITH change_page AS MATERIALIZED (
            SELECT generation, entry_id
            FROM brunn.workspace_changes
            WHERE user_id=$1 AND generation>$2 AND generation<=$3
            ORDER BY generation LIMIT 2000
        ), snapshots AS MATERIALIZED (
            SELECT ids.entry_id, latest.*
            FROM (SELECT DISTINCT entry_id FROM change_page) ids
            CROSS JOIN LATERAL (
                SELECT generation, entry_version, path, operation, content_sha256
                FROM brunn.workspace_changes
                WHERE user_id=$1 AND entry_id=ids.entry_id AND generation<=$3
                ORDER BY generation DESC LIMIT 1
            ) latest
        )
        SELECT c.generation, snapshot.generation AS snapshot_generation,
               c.entry_id, snapshot.entry_version, snapshot.path,
               snapshot.operation, snapshot.content_sha256, v.metadata,
               (v.version IS NOT NULL) AS source_available,
               (v.content IS NOT NULL) AS is_text
        FROM change_page c JOIN snapshots snapshot ON snapshot.entry_id=c.entry_id
        LEFT JOIN LATERAL (
            SELECT version,metadata,content
            FROM brunn.entry_versions
            WHERE user_id=$1 AND entry_id=c.entry_id AND version=snapshot.entry_version
            LIMIT 1
        ) v ON true
        ORDER BY c.generation
    "#,
    )
    .bind(user)
    .bind(data.scanned_generation)
    .bind(upper)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let path: String = row.get("path");
        let generation: i64 = row.get("generation");
        if !row.get::<bool, _>("source_available") {
            // A visible historical change can outlive access to its version.
            // Emit an explicit bounded disposition instead of dropping every
            // row of the page at the join and retrying that empty page forever.
            if data.source_dispositions.len() >= MAX_INPUTS {
                break;
            }
            data.source_dispositions.push(json!({"entry_ref":format!("entry:{}",row.get::<Uuid,_>("entry_id")),"version":row.get::<i64,_>("entry_version"),"generation":row.get::<i64,_>("snapshot_generation"),"disposition":"source_unavailable","detail":"The change is visible but its exact source version is unavailable. No model read or summary publication is allowed; any previously retained input remains pending."}));
            data.scanned_generation = generation;
            continue;
        }
        let metadata: Value = row.get("metadata");
        if !row.get::<bool, _>("is_text") || input_excluded(&path, &metadata) {
            data.scanned_generation = generation;
            continue;
        }
        let reference = format!("entry:{}", row.get::<Uuid, _>("entry_id"));
        if row.get::<String, _>("operation") == "delete" {
            if data.source_dispositions.len() >= MAX_INPUTS {
                break;
            }
            data.inputs.retain(|i| i.entry_ref != reference);
            if !data
                .source_dispositions
                .iter()
                .any(|v| v["entry_ref"] == reference)
            {
                data.source_dispositions.push(json!({"entry_ref":reference,"version":row.get::<i64,_>("entry_version"),"generation":row.get::<i64,_>("snapshot_generation"),"disposition":"deleted_source","detail":"Source is deleted; no model read or summary publication is allowed."}));
            }
            data.scanned_generation = generation;
            continue;
        }

        if let Some(input) = data.inputs.iter_mut().find(|i| i.entry_ref == reference) {
            input.version = row.get("entry_version");
            input.path = path;
            input.operation = row.get("operation");
            input.content_hash = format!("sha256:{}", row.get::<String, _>("content_sha256"));
            data.scanned_generation = generation;
            continue;
        }
        if data.inputs.len() >= MAX_INPUTS {
            break;
        }
        let input = Input {
            entry_ref: format!("entry:{}", row.get::<Uuid, _>("entry_id")),
            version: row.get("entry_version"),
            path,
            generation,
            operation: row.get("operation"),
            content_hash: format!("sha256:{}", row.get::<String, _>("content_sha256")),
        };
        data.inputs.push(input);
        data.scanned_generation = generation;
    }
    data.processed_generation = data
        .inputs
        .iter()
        .map(|i| i.generation - 1)
        .min()
        .unwrap_or(data.scanned_generation)
        .min(data.scanned_generation);
    Ok(())
}

fn record_location_disposition(data: &mut RunState, disposition: Value) {
    if data.location_dispositions.contains(&disposition) {
        return;
    }
    // Older dispositions remain in immutable state and run versions.
    if data.location_dispositions.len() >= 31 {
        data.location_dispositions.remove(0);
    }
    data.location_dispositions.push(disposition);
}

fn same_location_window(left: &Value, right: &Value) -> bool {
    left["timezone"] == right["timezone"]
        && ["from", "to"].iter().all(|key| {
            // The accepted scope serializes FixedOffset (+00:00), while a
            // queued UTC window uses Z. Compare instants, not JSON spelling.
            let instant = |value: &Value| {
                value[*key]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            };
            matches!((instant(left), instant(right)), (Some(a), Some(b)) if a == b)
        })
}

fn expire_location_work(data: &mut RunState, now: DateTime<Utc>) -> ApiResult<bool> {
    let mut retained = Vec::new();
    let mut expired = Vec::new();
    for work in &data.location_work {
        let from = DateTime::parse_from_rfc3339(string(work, "from")?)
            .map_err(|_| ApiError::invalid("Retained location window is invalid"))?
            .with_timezone(&Utc);
        let expires_at = from + Duration::days(30);
        if expires_at < now {
            expired.push(json!({"disposition":"raw_retention_expired","work":work,"expired_at":expires_at,"detail":"The original window no longer has complete retained raw evidence; no summary was compiled or published."}));
        } else {
            retained.push(work.clone());
        }
    }
    let changed = !expired.is_empty();
    data.location_work = retained;
    for disposition in expired {
        record_location_disposition(data, disposition);
    }
    Ok(changed)
}

pub async fn queue_location(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    auth.require(Capability::Save)?;
    let date = NaiveDate::parse_from_str(string(&body, "date")?, "%Y-%m-%d")
        .map_err(|_| ApiError::invalid("date must be an ISO date"))?;
    let zone: chrono_tz::Tz = string(&body, "timezone")?
        .parse()
        .map_err(|_| ApiError::invalid("timezone must be IANA"))?;
    let from = zone
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .ok_or_else(|| ApiError::invalid("day starts at an unavailable local time"))?
        .with_timezone(&Utc);
    let to = zone
        .from_local_datetime(
            &date
                .succ_opt()
                .ok_or_else(|| ApiError::invalid("invalid date"))?
                .and_hms_opt(0, 0, 0)
                .expect("midnight"),
        )
        .earliest()
        .ok_or_else(|| ApiError::invalid("day ends at an unavailable local time"))?
        .with_timezone(&Utc);
    if to > Utc::now() || from < Utc::now() - Duration::days(30) {
        return Err(ApiError::invalid(
            "pilot requires a completed day within retained raw evidence",
        ));
    }
    let work = json!({"date":date.to_string(),"timezone":zone.name(),"from":from,"to":to});
    let mut tx = state.begin_write(&auth).await?;
    if body.get("context_sources").is_some() {
        return Err(ApiError::invalid(
            "Location context is discovered automatically; queue the date without source selections",
        ));
    }
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    if version == 0 {
        import_legacy(&mut tx, auth.user_id.0, &mut data).await?;
    }
    let expired = expire_location_work(&mut data, Utc::now())?;
    if !data.location_work.is_empty() {
        if data.location_work[0] == work {
            let version = if expired {
                let version = save_state(&state, &mut tx, &auth, &data, version).await?;
                tx.commit().await?;
                version
            } else {
                version
            };
            return Ok(Json(
                json!({"status":"complete","data":{"queued":true,"work":work,"state_version":version,"no_op":true}}),
            ));
        }
        if same_location_window(&data.location_work[0], &work)
            && data.active.is_none()
            && body["expected_state_version"].as_i64() == Some(version)
        {
            data.location_work[0] = work.clone();
            let version = save_state(&state, &mut tx, &auth, &data, version).await?;
            tx.commit().await?;
            return Ok(Json(
                json!({"status":"complete","data":{"queued":true,"work":work,"state_version":version}}),
            ));
        }
        return Err(conflict(
            "A historical day is already retained; finish that bounded pilot first",
            version,
        ));
    }
    data.location_work.push(work.clone());
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"status":"complete","data":{"queued":true,"work":work,"state_version":version}}),
    ))
}
async fn validate_location_candidate(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    candidate: &Candidate,
) -> ApiResult<Value> {
    let scope = candidate
        .evidence_scope
        .as_ref()
        .ok_or_else(|| ApiError::invalid("location evidence scope required"))?;
    let query = serde_json::from_value(
        json!({"from":scope["from"],"to":scope["to"],"timezone":scope["timezone"]}),
    )?;
    let mut internal = auth.clone();
    internal.capabilities.insert("save".into());
    let (validated_scope, packet) =
        crate::location::summary::validate_candidate_with_clock_evidence_in_tx(
            tx,
            &internal,
            &query,
            string(scope, "fingerprint")?,
            &candidate.sources,
            &candidate.raw_sources,
            scope.get("context_sources"),
        )
        .await?;
    let output = json!({"candidates":[candidate]});
    let admission =
        json!({"location_work":{"timezone":scope["timezone"]},"location_evidence":packet});
    if let Some(issue) =
        crate::dreamer::prompt::location_content_issues(&output, &admission).first()
    {
        return Err(ApiError::invalid(format!(
            "location content validation failed: {issue}"
        )));
    }
    Ok(validated_scope)
}
pub async fn admit(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let user = auth.user_id.0;
    let attempt_id = string(&body, "attempt_id")?.to_owned();
    if Uuid::parse_str(&attempt_id).is_err() {
        return Err(ApiError::invalid("attempt_id must be a UUID"));
    }
    let date = string(&body, "date")?.to_owned();
    NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .map_err(|_| ApiError::invalid("date must be an owner-local ISO date"))?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    let Some(current_mode) = mode(&mut tx, user).await? else {
        return Ok(Json(
            json!({"admitted":false,"reason":"CONTROL is disabled, missing or invalid"}),
        ));
    };
    let (mut data, version) = load_state(&mut tx, user).await?;
    let mut recovered = false;
    if let Some(a) = data.active.clone() {
        if a.lease_until > Utc::now() {
            if a.attempt_id == attempt_id
                && a.producer_credential_id == auth.credential_id.0.to_string()
                && a.admission_hash == digest(&body)
            {
                let response = admission_response(&mut tx, &auth, &data, version).await?;
                tx.commit().await?;
                return Ok(Json(response));
            }
            return Err(conflict("A Dreamer attempt is already active", version));
        }
        let at = Utc::now();
        let detail = "The attempt lease expired without an accepted terminal result. Retained work is available for retry.";
        data.last_attempt = Some(
            json!({"attempt_id":a.attempt_id,"producer_credential_id":a.producer_credential_id,"date":a.date,"outcome":"partial","detail":detail,"started_at":a.started_at,"finished_at":at,"recovered":true,"counts":counts(&data),"project_status":a.project_status}),
        );
        let run = write_run(&state, &mut tx, &auth, &mut data, &a, "partial", detail).await?;
        write_projection(
            &state,
            &mut tx,
            &auth,
            &projection(&data, &a, &run, "partial", at),
        )
        .await?;
        data.active = None;
        recovered = true;
    }
    let expired_location = expire_location_work(&mut data, Utc::now())?;
    if body.get("kind").and_then(Value::as_str) != Some("manual")
        && data
            .last_successful_run
            .as_ref()
            .and_then(|v| v.get("run_id"))
            .and_then(Value::as_str)
            == Some(&date)
    {
        if recovered || expired_location {
            save_state(&state, &mut tx, &auth, &data, version).await?;
            tx.commit().await?;
        }
        return Ok(Json(
            json!({"admitted":false,"reason":"scheduled date already completed"}),
        ));
    }
    let upper: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(generation),0) FROM brunn.workspace_changes WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    // Terminal candidates have immutable run and review audit versions. Their
    // live slots can be reclaimed; pending work is never evicted to make room.
    data.items.retain(pending);
    if !data.legacy_complete {
        import_legacy(&mut tx, user, &mut data).await?;
    }
    data.source_dispositions.clear();
    data.candidate_dispositions.clear();
    retain_inputs(&mut tx, user, &mut data, upper).await?;
    research_comparison::retire_excluded(&state, &mut tx, &auth, &mut data).await?;
    research::enqueue_requested(&mut tx, &auth, &mut data, &body, upper).await?;
    let lease = body
        .get("lease_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(2100)
        .clamp(30, 14_400);
    let now = Utc::now();
    let fence = Uuid::now_v7().to_string();
    data.processed_count = 0;
    data.candidate_submission = None;
    data.active = Some(Attempt {
        attempt_id: attempt_id.clone(),
        fence: fence.clone(),
        date: date.clone(),
        producer_credential_id: auth.credential_id.0.to_string(),
        started_at: now,
        lease_until: now + Duration::seconds(lease),
        frozen_generation: upper,
        mode: current_mode.clone(),
        admission_hash: digest(&body),
        admission_version: version + 1,
        location_work: None,
        project_status: Value::Null,
        narrative_context: Vec::new(),
        narrative_discovery: None,
    });
    if data.location_work.is_empty() && !data.location_scopes.is_empty() {
        let mut internal = auth.clone();
        internal.capabilities.insert("save".into());
        crate::location::store::lock_location_user(&mut tx, user).await?;
        for _ in 0..data.location_scopes.len().min(3) {
            let index = data.location_check_cursor % data.location_scopes.len();
            data.location_check_cursor = (index + 1) % data.location_scopes.len();
            let known = data.location_scopes[index].clone();
            let query: crate::location::evidence::EvidenceQuery = serde_json::from_value(
                json!({"from":known["from"],"to":known["to"],"timezone":known["timezone"]}),
            )?;
            if query.from.to_utc() < Utc::now() - Duration::days(30) {
                continue;
            }
            let packet =
                crate::location::evidence::evidence_in_tx(&mut tx, &internal, &query).await?;
            let deferred = data.items.iter().any(|i| {
                i.status == "deferred"
                    && i.candidate.path.as_deref()
                        == Some(
                            format!(
                                "derived/location/{}.md",
                                known["date"].as_str().unwrap_or("")
                            )
                            .as_str(),
                        )
            });
            let context_current =
                crate::location::summary::context_sources_current_in_tx(&mut tx, &internal, &known)
                    .await?;
            if packet["fingerprint_complete"] == true
                && (packet["evidence_fingerprint"] != known["fingerprint"] || !context_current)
                && !deferred
            {
                let mut work = known.clone();
                work.as_object_mut().expect("scope").remove("fingerprint");
                data.location_work.push(work);
                break;
            }
        }
    }
    location_discovery::queue_latest(&mut tx, user, &mut data, &date).await?;
    // A fresh attempt rediscovers context. Old corrected prose and old lookup
    // sources cannot enter the new discovery prompt via a retained scope.
    for work in &mut data.location_work {
        if let Some(fields) = work.as_object_mut() {
            fields.remove("context_sources");
            fields.remove("discovery");
        }
    }
    if let Some(work) = data.location_work.first().cloned() {
        if crate::location::summary::context_sources_current_in_tx(&mut tx, &auth, &work).await? {
            let query: crate::location::evidence::EvidenceQuery = serde_json::from_value(
                json!({"from":work["from"],"to":work["to"],"timezone":work["timezone"]}),
            )?;
            let mut internal = auth.clone();
            internal.capabilities.insert("save".into());
            crate::location::store::lock_location_user(&mut tx, user).await?;
            let packet =
                crate::location::evidence::evidence_in_tx(&mut tx, &internal, &query).await?;
            // Owner approval was for the exact old evidence. Both automatic
            // refresh and explicit requeue need a fresh candidate and decision.
            if packet["fingerprint_complete"] == true {
                let mut invalidated = Vec::new();
                for item in &mut data.items {
                    if item.status == "approved_held"
                        && item.candidate.evidence_scope.as_ref().is_some_and(|scope| {
                            same_location_window(scope, &work)
                                && (scope["fingerprint"] != packet["evidence_fingerprint"]
                                    || scope.get("context_sources") != work.get("context_sources"))
                        })
                    {
                        let mut original = work.clone();
                        original["fingerprint"] =
                            item.candidate.evidence_scope.as_ref().expect("scope")["fingerprint"]
                                .clone();
                        item.status = "needs_changes".into();
                        invalidated.push(json!({"disposition":"held_approval_invalidated","work":original,"new_fingerprint":packet["evidence_fingerprint"],"item_id":item.id,"candidate_hash":item.candidate_hash,"run_entry_ref":item.run_entry_ref,"run_version":item.run_version}));
                    }
                }
                for disposition in invalidated {
                    record_location_disposition(&mut data, disposition);
                }
            }
            let mut scope = work.clone();
            scope["fingerprint"] = packet["evidence_fingerprint"].clone();
            data.active.as_mut().expect("active").location_work = Some(scope);
        } else {
            for item in &mut data.items {
                if item.status == "approved_held"
                    && item
                        .candidate
                        .evidence_scope
                        .as_ref()
                        .is_some_and(|scope| same_location_window(scope, &work))
                {
                    item.status = "needs_changes".into();
                }
            }
            record_location_disposition(
                &mut data,
                json!({"disposition":"context_sources_changed","work":work,"detail":"Explicit location context changed or is inaccessible. The day remains retained until the owner supplies current exact sources."}),
            );
        }
    }
    research::invalidate_held(&mut tx, &auth, &mut data).await?;
    if current_mode == "full" {
        let mut changed = false;
        for index in data
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.status == "approved_held")
            .take(8)
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
        {
            let mut item = data.items[index].clone();
            if item_stale(&mut tx, &auth, &item).await? {
                item.status = "needs_changes".into();
            } else {
                let validation = async {
                    validate_candidate(&item.candidate, &item.before_md)?;
                    if item.candidate.evidence_scope.is_some() {
                        validate_location_candidate(&mut tx, &auth, &item.candidate).await?;
                    }
                    Ok::<(), ApiError>(())
                }
                .await;
                match validation {
                    Ok(()) => {
                        publish_item(&state, &mut tx, &auth, &mut item, Some(&attempt_id)).await?;
                    }
                    Err(ApiError::Public {
                        status,
                        code,
                        message,
                        ..
                    }) if status == axum::http::StatusCode::BAD_REQUEST
                        || status == axum::http::StatusCode::CONFLICT =>
                    {
                        // A prior approval is for immutable bytes. A changed
                        // contract requires a new proposal and owner decision;
                        // it must not wedge unrelated work on every admission.
                        item.status = "needs_changes".into();
                        data.candidate_dispositions.push(json!({
                            "disposition":"held_approval_invalidated",
                            "item_id":item.id,"candidate_hash":item.candidate_hash,
                            "run_entry_ref":item.run_entry_ref,"run_version":item.run_version,
                            "code":code,"reason":message,
                        }));
                        if let Some(scope) = &item.candidate.evidence_scope
                            && !data
                                .location_work
                                .iter()
                                .any(|work| same_location_window(scope, work))
                            && let Some(known) = data
                                .location_scopes
                                .iter()
                                .find(|known| same_location_window(scope, known))
                        {
                            let mut work = known.clone();
                            work.as_object_mut()
                                .expect("retained location scope")
                                .remove("fingerprint");
                            data.location_work.push(work);
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            data.items[index] = item;
            changed = true;
        }
        if changed {
            let attempt = data.active.clone().expect("active");
            write_run(&state,&mut tx,&auth,&mut data,&attempt,"running","Previously approved candidates were checked for publication. Invalid candidates require changes and a new owner decision; model execution has not completed.").await?;
        }
    }
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    let response = admission_response(&mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(response))
}
async fn admission_response(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    version: i64,
) -> ApiResult<Value> {
    let user = auth.user_id.0;
    let a = data.active.as_ref().expect("active admission");
    let mut pending_items = Vec::new();
    for item in data.items.iter().filter(|i| pending(i)) {
        pending_items.push(if item_available(tx, user, item).await? {
            item.clone()
        } else {
            withhold_item(item)
        });
    }
    let mut location_evidence = Value::Null;
    if let Some(work) = &a.location_work {
        if !crate::location::summary::context_sources_current_in_tx(tx, auth, work).await? {
            return Err(conflict(
                "Location context changed; retain this day for a fresh admission",
                version,
            ));
        }
        let query = serde_json::from_value(
            json!({"from":work["from"],"to":work["to"],"timezone":work["timezone"]}),
        )?;
        let mut internal = auth.clone();
        internal.capabilities.insert("save".into());
        crate::location::store::lock_location_user(tx, user).await?;
        location_evidence =
            crate::location::evidence::evidence_in_tx(tx, &internal, &query).await?;
        if location_evidence["evidence_fingerprint"] != work["fingerprint"] {
            return Err(conflict(
                "Location evidence changed; retain this day for a fresh admission",
                version,
            ));
        }
    }
    let research = research::view_active(tx, auth, data).await?;
    let comparison_proposals = if let Some(reference) = research["subject_ref"].as_str() {
        if let Some((job, _)) = research::load(tx, auth, reference).await? {
            research_comparison::proposals(tx, auth, data, &job).await?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let decisions = load_entry(tx, user, "dreams/decisions.md").await?;
    let rows=sqlx::query("SELECT path,current_version FROM brunn.entries WHERE user_id=$1 AND deleted_at IS NULL AND (starts_with(path,'derived/entities/') OR starts_with(path,'derived/location/')) ORDER BY path LIMIT 256")
        .bind(user).fetch_all(&mut **tx).await?;
    let outputs=rows.iter().map(|r|json!({"path":r.get::<String,_>("path"),"version":r.get::<i64,_>("current_version")})).collect::<Vec<_>>();
    Ok(
        json!({"research_protocol":1,"research":research,"comparison_proposals":comparison_proposals,"research_progress":{"turns":data.research.service_sequence,"completed":data.research.completed},"admitted":true,"session_id":format!("session:{}",a.attempt_id),"attempt_id":a.attempt_id,"fence":a.fence,"state_version":version,"mode":a.mode,"frozen_generation":a.frozen_generation,"scanned_generation":data.scanned_generation,"processed_generation":data.processed_generation,"inputs":data.inputs,"outputs":outputs,"location_work":a.location_work,"location_evidence":location_evidence,"location_context":a.location_work.as_ref().and_then(|work|work.get("context_sources")).cloned().unwrap_or_else(||json!([])),"narrative_context":a.narrative_context,"narrative_discovery":a.narrative_discovery,"pending":pending_items,"pending_notifications":data.pending_notifications,"decisions":decisions.as_ref().map(|e|e.content.as_str()).unwrap_or(""),"decisions_version":decisions.map_or(0,|e|e.version)}),
    )
}
pub async fn checkpoint(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid(
            "CONTROL is paused; no state write was made",
        ));
    }
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    active(&data, &body, &auth, version)?;
    let lease = body
        .get("lease_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(2100)
        .clamp(30, 14_400);
    data.active.as_mut().expect("active").lease_until = Utc::now() + Duration::seconds(lease);
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"state_version":version,"pending":data.items.iter().filter(|i|pending(i)).count()}),
    ))
}

fn vetoed(raw: &str, date: &str, number: usize) -> bool {
    if decisions::is_vetoed(&decisions::parse(raw), date, number) {
        return true;
    }
    // Preserve the owner's existing prose adjudications as well as the strict
    // line grammar; never interpret an old veto as an unreviewed new proposal.
    let mut in_date = false;
    for line in raw.lines() {
        if line.starts_with("## ") {
            in_date = line.contains(date);
        }
        let lower = line.to_ascii_lowercase();
        if in_date && lower.contains(&format!("proposal {number}:")) && lower.contains("vetoed") {
            return true;
        }
    }
    false
}
async fn import_legacy(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    data: &mut RunState,
) -> ApiResult<()> {
    if data.legacy_items.len() >= MAX_LEGACY_ITEMS {
        // The bounded History projection can be full while current work runs.
        // Preserve the scan cursor and the original reports for later access.
        return Ok(());
    }
    let decisions = load_entry(tx, user, "dreams/decisions.md")
        .await?
        .map(|e| e.content)
        .unwrap_or_default();
    let rows=sqlx::query("SELECT e.id,e.path,e.current_version,v.content,v.metadata,v.created_at FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.deleted_at IS NULL AND e.path LIKE 'dreams/runs/%.md' AND e.path>$2 ORDER BY e.path LIMIT 128")
        .bind(user).bind(&data.legacy_scan_after).fetch_all(&mut **tx).await?;
    let exhausted = rows.len() < 128;
    for row in rows {
        let metadata: Value = row.get("metadata");
        if metadata.get("dreamer_run").is_some() {
            data.legacy_scan_after = row.get("path");
            continue;
        }
        let path: String = row.get("path");
        let Some(date) = path
            .strip_prefix("dreams/runs/")
            .and_then(|s| s.strip_suffix(".md"))
        else {
            continue;
        };
        if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
            continue;
        }
        let content: String = row.get("content");
        let mut section = "";
        let mut number = 0usize;
        let mut paragraphs: Vec<(usize, String, String)> = Vec::new();
        for line in content.lines() {
            if line.starts_with("## ") {
                section = line.trim_start_matches("## ");
                continue;
            }
            if !matches!(section, "Proposed" | "Needs your call") {
                continue;
            }
            if let Some((n, text)) = line.split_once(". ")
                && let Ok(n) = n.parse::<usize>()
            {
                number = number.max(n);
                paragraphs.push((n, section.to_owned(), text.to_owned()));
                continue;
            }
            if !line.trim().is_empty()
                && let Some((_, _, text)) = paragraphs.last_mut()
            {
                text.push('\n');
                text.push_str(line);
            }
        }
        data.next_item
            .entry(date.to_owned())
            .and_modify(|n| *n = (*n).max(number as u64))
            .or_insert(number as u64);
        for (n, section, text) in paragraphs {
            let id = if section == "Needs your call" {
                format!("{date}/question-{n}")
            } else {
                format!("{date}/{n}")
            };
            if data
                .items
                .iter()
                .chain(&data.legacy_items)
                .any(|i| i.id == id)
                || vetoed(&decisions, date, n)
            {
                continue;
            }
            if data.legacy_items.len() >= MAX_LEGACY_ITEMS {
                return Ok(());
            }
            let title = text
                .lines()
                .next()
                .unwrap_or("Legacy proposal")
                .chars()
                .take(150)
                .collect::<String>();
            let candidate = Candidate {
                kind: if section == "Needs your call" {
                    "question"
                } else {
                    "legacy"
                }
                .into(),
                title,
                summary: text.chars().take(1000).collect(),
                reason: LEGACY_REASON.into(),
                path: None,
                content: None,
                expected_version: None,
                sources: vec![],
                uncertainty: String::new(),
                question: if section == "Needs your call" {
                    text
                } else {
                    String::new()
                },
                revises_item_id: None,
                evidence_scope: None,
                raw_sources: vec![],
                subject_ref: None,
                subject_scope: None,
            };
            data.legacy_items.push(Item {
                id,
                run_id: date.into(),
                run_entry_ref: format!("entry:{}", row.get::<Uuid, _>("id")),
                run_version: row.get("current_version"),
                candidate_hash: digest(&candidate),
                candidate,
                status: "pending".into(),
                reviewable: false,
                before_md: String::new(),
                published: None,
                frozen_generation: 0,
                created_at: row.get("created_at"),
            });
        }
        data.legacy_scan_after = path;
    }
    data.legacy_complete = exhausted;
    Ok(())
}
fn render_run(data: &RunState, a: &Attempt, outcome: &str, detail: &str) -> String {
    let mut body = format!(
        "Run result: {outcome}. Mode: {}.\n{detail}\nRetained inputs: {}; pending review items: {}.\nScanned generation: {}; processed generation: {}.\n[Open Review](https://brunn.ai/dreams) to inspect evidence and record a decision.\n\n## Applied\n",
        a.mode,
        data.inputs.len(),
        data.items.iter().filter(|i| pending(i)).count(),
        data.scanned_generation,
        data.processed_generation
    );
    let applied: Vec<_> = data
        .items
        .iter()
        .filter_map(|i| {
            i.published
                .as_ref()
                .filter(|p| p["attempt_id"] == a.attempt_id)
                .map(|p| (i, p))
        })
        .collect();
    if applied.is_empty() {
        body.push_str("None.\n");
    }
    for (i, p) in applied {
        body.push_str(&format!(
            "- {}@{} — {}\n",
            p["path"].as_str().unwrap_or(""),
            p["version"],
            i.candidate.title
        ));
    }
    if !a.project_status.is_null() {
        body.push_str(&format!(
            "\n## Project status\n{}\n",
            crate::dreamer::receipt::project_status_line(&a.project_status)
        ));
    }
    for (heading, question) in [("Proposed", false), ("Needs your call", true)] {
        body.push_str(&format!("\n## {heading}\n"));
        for i in data
            .items
            .iter()
            .filter(|i| pending(i) && (i.candidate.kind == "question") == question)
        {
            body.push_str(&format!(
                "\n### {} — {}\n\nStatus: {}\n\n{}\n\n",
                i.id, i.candidate.title, i.status, i.candidate.reason
            ));
            if i.run_version == 0 {
                body.push_str(&candidate_body(&i.candidate));
            } else {
                body.push_str(&format!(
                    "Candidate evidence: {} v{}. Open Review for its exact content.\n",
                    i.run_entry_ref, i.run_version
                ));
            }
            if !i.reviewable {
                body.push_str(&format!(
                    "\n{}\n\nA generated candidate is required before approval.\n",
                    i.candidate.summary
                ));
            }
        }
    }
    body.push_str(&format!("\n## Findings\n{detail}\n\n## Watermark\nscanned_generation: {}\nprocessed_generation: {}\nfrozen_generation: {}\n",data.scanned_generation,data.processed_generation,a.frozen_generation));
    body
}
async fn write_run(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    a: &Attempt,
    outcome: &str,
    detail: &str,
) -> ApiResult<Value> {
    let path = format!("dreams/runs/{}.md", a.date);
    let existing = load_entry(tx, auth.user_id.0, &path).await?;
    let metadata = json!({"kind":"dreamer_run","dreamer_run":{"schema":"dream.run.v1","accepted":true,"attempt_id":a.attempt_id,"date":a.date,"producer_credential_id":auth.credential_id.0,"frozen_generation":a.frozen_generation,"outcome":outcome,"items":data.items.iter().filter(|i|i.run_version==0).collect::<Vec<_>>(),"pending_refs":data.items.iter().filter(|i|pending(i)&&i.run_version>0).map(|i|json!({"id":i.id,"entry_ref":i.run_entry_ref,"version":i.run_version,"candidate_hash":i.candidate_hash,"status":i.status})).collect::<Vec<_>>(),"attempt":data.last_attempt,"history":data.history,"source_dispositions":data.source_dispositions,"candidate_dispositions":data.candidate_dispositions,"location_dispositions":data.location_dispositions}});
    let receipt = put_entry(
        state,
        tx,
        auth,
        &path,
        render_run(data, a, outcome, detail),
        metadata,
        existing.map_or(0, |e| e.version),
    )
    .await?;
    for i in &mut data.items {
        if i.reviewable && i.run_version == 0 {
            i.run_entry_ref = receipt["entry_ref"].as_str().expect("ref").into();
            i.run_version = receipt["version"].as_i64().expect("version");
        }
    }
    Ok(receipt)
}
fn projection(
    data: &RunState,
    a: &Attempt,
    run: &Value,
    outcome: &str,
    completed: DateTime<Utc>,
) -> Value {
    let mut owner:Vec<Value>=data.items.iter().filter(|i|pending(i)).map(|i|json!({"recommendation_id":crate::dreamer::receipt::recommendation_id(&i.id),"summary":format!("{} — {}",i.id,i.candidate.title),"reason":if i.status=="approved_held"{"Approved; held by report-only mode."}else if !i.reviewable{"Needs a concrete candidate or owner clarification before application."}else{"Review the proposed change and its supporting evidence."},"published_at":crate::dreamer::receipt::format_timestamp(i.created_at),"age_days":(completed-i.created_at).num_days().max(0)})).collect();
    owner.sort_by(|a, b| {
        a["recommendation_id"]
            .as_str()
            .cmp(&b["recommendation_id"].as_str())
    });
    let mut applied:Vec<Value>=data.items.iter().filter_map(|i|i.published.as_ref().filter(|p|p["attempt_id"]==a.attempt_id).map(|p|json!({"path":p["path"],"version":p["version"],"summary":i.candidate.title}))).collect();
    applied.sort_by(|a, b| {
        a["path"]
            .as_str()
            .cmp(&b["path"].as_str())
            .then(a["version"].as_i64().cmp(&b["version"].as_i64()))
    });
    applied.dedup_by(|a, b| a["path"] == b["path"] && a["version"] == b["version"]);
    let mut latest = json!({"schema":"dream.latest-receipt.v2","run_id":a.date,"receipt_ref":run["entry_ref"],"receipt_version":run["version"],"receipt_path":run["path"],"status":outcome,"completed_at":crate::dreamer::receipt::format_timestamp(completed),"mode":a.mode,"runner":"brunn-rust-dreamer","mode_flip":false,"probe_monitoring":null,"applied_writes":applied,"entering_veto_window_today":[],"pending_owner":owner,"pending_review_surfaces":[],"next_run_at":crate::dreamer::receipt::format_timestamp(next_run(completed))});
    if !a.project_status.is_null() {
        latest["project_status"] = a.project_status.clone();
    }
    latest
}
async fn write_projection(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    value: &Value,
) -> ApiResult<()> {
    let body = crate::dreamer::receipt::render_latest(value).map_err(ApiError::invalid)?;
    let old = load_entry(tx, auth.user_id.0, "dreams/latest-receipt.md").await?;
    put_entry(
        state,
        tx,
        auth,
        "dreams/latest-receipt.md",
        body,
        json!({"kind":"dreamer_receipt","dreamer_receipt":value}),
        old.map_or(0, |e| e.version),
    )
    .await?;
    Ok(())
}

pub async fn candidates(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    let user = auth.user_id.0;
    if mode(&mut tx, user).await?.is_none() {
        return Err(ApiError::invalid(
            "CONTROL is paused; candidates were not published",
        ));
    }
    let (mut data, version) = load_state(&mut tx, user).await?;
    research_comparison::reject_candidate_fields(&body)?;
    if body.get("repair_feedback").is_some()
        || body["research_progress"].get("repair_feedback").is_some()
    {
        return Err(ApiError::invalid(
            "repair feedback requires an operational-only research-progress request",
        ));
    }
    let request_hash = digest(&body);
    let mut research_job = None;
    let mut research_operation = None;
    if let Some(reference) = body.get("subject_ref").and_then(Value::as_str) {
        let (operation_id, hash) = research::operation(&body, "candidates")?;
        let (job, job_version) = research::load(&mut tx, &auth, reference)
            .await?
            .ok_or_else(|| ApiError::invalid("research subject is not retained"))?;
        let replay = research::receipt(
            &mut tx,
            &auth,
            &research::path(reference)?,
            "dreamer_research",
            &operation_id,
            &hash,
        )
        .await?;
        research::checked_attempt(&data, &body, &auth, version, replay.is_some())?;
        if let Some(receipt) = replay {
            let mut response = admission_response(&mut tx, &auth, &data, version).await?;
            for (key, value) in receipt["result"].as_object().into_iter().flatten() {
                response[key] = value.clone();
            }
            response["state_version"] = json!(version);
            response["no_op"] = json!(true);
            research::checkpoints::replay(&mut response["checkpoint_receipt"]);
            research_comparison::replay_resolution(&mut response["follow_up_receipt"]);
            research::drafts::replay(&mut response["draft_receipt"]);
            return Ok(Json(response));
        }
        research::check_job(&data, &body, &job, job_version)?;
        research_operation = Some((operation_id, hash));
        research_job = Some((job, job_version));
    }
    if research_job.is_none()
        && let Some(old) = &data.candidate_submission
        && old["attempt_id"] == body["attempt_id"]
        && old["producer"] == auth.credential_id.0.to_string()
        && old["request_hash"] == request_hash
    {
        return Ok(Json(old["response"].clone()));
    }
    let subject_submission = research_job.is_some();
    let a = active(&data, &body, &auth, version)?;
    let draft = if let Some((job, _)) = &research_job {
        research::drafts::validate_submission(&mut tx, &auth, job, &body).await?
    } else {
        if body.get("draft_pointer").is_some() || body.get("draft_protocol").is_some() {
            return Err(ApiError::invalid(
                "draft submission requires the selected research subject",
            ));
        }
        None
    };
    let source_resolutions = if let Some((job, _)) = &research_job {
        research_comparison::prepare_resolutions(
            &mut tx,
            &auth,
            &data,
            job,
            body.get("research_progress").unwrap_or(&Value::Null),
            true,
        )
        .await?
    } else {
        if body["research_progress"]
            .get("resolved_follow_ups")
            .is_some()
            || body["research_progress"]
                .get("follow_up_protocol")
                .is_some()
        {
            return Err(ApiError::invalid(
                "source routes require the selected research subject",
            ));
        }
        Vec::new()
    };
    let list: Vec<Candidate> =
        serde_json::from_value(body.get("candidates").cloned().unwrap_or(json!([])))?;
    if list.len() > 16 {
        return Err(ApiError::invalid(
            "review inbox is full; admitted input and existing proposals remain retained",
        ));
    }
    let is_location = |candidate: &Candidate| {
        candidate.evidence_scope.is_some()
            || candidate
                .path
                .as_deref()
                .is_some_and(|path| path.starts_with("derived/location/"))
    };
    if list
        .iter()
        .filter(|candidate| is_location(candidate))
        .count()
        > 1
    {
        return Err(ApiError::invalid(
            "at most one location candidate is allowed per submission",
        ));
    }
    let mut revisions = std::collections::BTreeSet::new();
    let mut destinations = std::collections::BTreeMap::new();
    for candidate in &list {
        if let Some(id) = &candidate.revises_item_id {
            if let Some((job, _)) = &research_job {
                let old = data
                    .items
                    .iter()
                    .find(|item| &item.id == id)
                    .ok_or_else(|| {
                        ApiError::invalid("revised item is not in the retained inbox")
                    })?;
                if old.candidate.evidence_scope.is_some()
                    || old.candidate.kind != candidate.kind
                    || old.candidate.path != candidate.path
                    || old
                        .candidate
                        .subject_ref
                        .as_deref()
                        .is_some_and(|reference| reference != job.subject_ref)
                    || old.candidate.subject_ref.is_none()
                        && old.candidate.path.as_deref() != Some(job.output_path.as_str())
                {
                    return Err(ApiError::invalid(
                        "subject revision must preserve its canonical identity, destination and kind",
                    ));
                }
            }
            if !revisions.insert(id) {
                return Err(ApiError::invalid(
                    "a retained item can be revised only once per submission",
                ));
            }
            if data
                .items
                .iter()
                .find(|item| &item.id == id)
                .is_some_and(|item| is_location(&item.candidate))
                && !is_location(candidate)
            {
                return Err(ApiError::invalid(
                    "location revision must preserve the original destination, kind and evidence window",
                ));
            }
            if let Some(old) = data.items.iter().find(|item| &item.id == id)
                && old.candidate.kind != "question"
                && !is_location(&old.candidate)
                && !is_location(candidate)
                && (old.candidate.kind != candidate.kind || old.candidate.path != candidate.path)
            {
                return Err(ApiError::invalid(
                    "a revision must preserve its original destination and kind",
                ));
            }
        }
        if let Some(path) = &candidate.path {
            let hash = digest(candidate);
            if let Some(previous) = destinations.insert(path, hash.clone()) {
                if previous == hash {
                    // Identical new candidates are deduplicated after validation;
                    // repeated revisions still fail the identity check above.
                    continue;
                }
                return Err(ApiError::invalid(
                    "a destination can have only one distinct candidate per submission",
                ));
            }
        }
        if !is_location(candidate) && candidate.kind != "question" {
            let same_target: Vec<_> = data
                .items
                .iter()
                .filter(|item| {
                    pending(item)
                        && item.candidate.path.is_some()
                        && item.candidate.path == candidate.path
                })
                .collect();
            if same_target.len() > 1 {
                return Err(ApiError::invalid(
                    "multiple retained candidates share this destination; owner resolution is required",
                ));
            }
            if let Some(old) = same_target.first() {
                if old.status == "deferred" || old.status == "approved_held" {
                    return Err(ApiError::invalid(
                        "a deferred or approved destination cannot be duplicated",
                    ));
                }
                if candidate.revises_item_id.as_deref() != Some(old.id.as_str()) {
                    return Err(ApiError::invalid(
                        "candidate must revise the existing item for this destination",
                    ));
                }
            }
        }
        if is_location(candidate) {
            if candidate.kind != "summary" {
                return Err(ApiError::invalid(
                    "location candidates must have kind summary",
                ));
            }
            let scope = candidate
                .evidence_scope
                .as_ref()
                .ok_or_else(|| ApiError::invalid("location evidence scope required"))?;
            let path = candidate
                .path
                .as_deref()
                .ok_or_else(|| ApiError::invalid("location candidate destination required"))?;
            if let Some(id) = &candidate.revises_item_id {
                let old = data
                    .items
                    .iter()
                    .find(|item| &item.id == id)
                    .ok_or_else(|| {
                        ApiError::invalid("revised item is not in the retained inbox")
                    })?;
                if !pending(old) || old.status == "deferred" || old.status == "approved_held" {
                    return Err(ApiError::invalid(
                        "a rejected, deferred or approved candidate cannot be silently replaced",
                    ));
                }
                if old.candidate.kind != candidate.kind
                    || old.candidate.path.as_deref() != Some(path)
                    || !old
                        .candidate
                        .evidence_scope
                        .as_ref()
                        .is_some_and(|old_scope| same_location_window(old_scope, scope))
                {
                    return Err(ApiError::invalid(
                        "location revision must preserve the original destination, kind and evidence window",
                    ));
                }
            }
            let same_target: Vec<_> = data
                .items
                .iter()
                .filter(|item| pending(item) && item.candidate.path.as_deref() == Some(path))
                .collect();
            if same_target
                .iter()
                .any(|item| item.status == "deferred" || item.status == "approved_held")
            {
                return Err(ApiError::invalid(
                    "a deferred or approved location destination cannot be duplicated",
                ));
            }
            if same_target.len() > 1 {
                return Err(ApiError::invalid(
                    "multiple retained location candidates share this destination; owner resolution is required",
                ));
            }
            if let Some(old) = same_target.first()
                && candidate.revises_item_id.as_deref() != Some(old.id.as_str())
            {
                return Err(ApiError::invalid(
                    "location candidate must revise the existing item for this destination",
                ));
            }
        }
    }
    let mut ids = Vec::new();
    let decisions = load_entry(&mut tx, user, "dreams/decisions.md")
        .await?
        .map(|e| e.content)
        .unwrap_or_default();
    for mut candidate in list {
        if candidate.subject_scope.is_some() {
            return Err(ApiError::invalid(
                "subject_scope is server-owned; submit subject_ref only",
            ));
        }
        let candidate_generation = if let Some((job, _)) = &research_job {
            if candidate.evidence_scope.is_some()
                || !candidate.raw_sources.is_empty()
                || candidate.subject_ref.as_deref() != Some(job.subject_ref.as_str())
            {
                return Err(ApiError::invalid(
                    "research candidates must identify their subject and cannot contain location evidence",
                ));
            }
            if candidate.sources.iter().any(|source| {
                !job.sources.iter().any(|input| {
                    input.entry_ref == source.entry_ref && input.version == source.version
                })
            }) {
                return Err(ApiError::invalid(
                    "candidate source was not admitted to this research subject",
                ));
            }
            if data.items.iter().any(|item| {
                item.candidate.path == candidate.path
                    && candidate.path.is_some()
                    && matches!(
                        item.status.as_str(),
                        "rejected" | "deferred" | "approved_held"
                    )
            }) {
                return Err(ApiError::invalid(
                    "a rejected, deferred or approved subject destination requires an owner decision before replacement",
                ));
            }
            if candidate.kind == "summary"
                && (candidate.path.as_deref() != Some(job.output_path.as_str())
                    || !candidate
                        .sources
                        .iter()
                        .any(|source| source.entry_ref == job.subject_ref))
            {
                return Err(ApiError::invalid(
                    "subject summary must use its stable output_path and cite its canonical source",
                ));
            }
            if candidate.kind == "summary" {
                let index = candidate
                    .sources
                    .iter()
                    .position(|source| source.entry_ref == job.subject_ref)
                    .expect("canonical citation checked")
                    + 1;
                let marker = format!("[^s{index}]");
                if !candidate
                    .content
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .any(|line| {
                        !line.trim_start().starts_with('#')
                            && !line.trim_start().starts_with("[^s")
                            && !line.trim_start().starts_with("[^r")
                            && line.contains(&marker)
                    })
                {
                    return Err(ApiError::invalid(
                        "subject summary must cite the canonical source inline",
                    ));
                }
            }
            if !research::fresh(&mut tx, &auth, job).await? {
                return Err(ApiError::public(
                    axum::http::StatusCode::BAD_REQUEST,
                    "research_refresh_required",
                    "research evidence is unavailable or its change scan is unfinished; rediscover before submitting",
                ));
            }
            // Dependencies are the reliance set at this pass's evidence
            // cutoff: canonical, cited and reviewed sources at their exact
            // pinned versions. Admitted but unreviewed leads are not bound.
            let mut scope = job.scope.clone();
            scope.checked_generation = research::cutoff(job);
            let progress_reviewed: Vec<Source> = serde_json::from_value(
                body["research_progress"]
                    .get("reviewed_sources")
                    .cloned()
                    .unwrap_or(json!([])),
            )
            .unwrap_or_default();
            let mut dependencies = std::collections::BTreeMap::new();
            for source in &job.sources {
                let relied = source.entry_ref == job.subject_ref
                    || candidate
                        .sources
                        .iter()
                        .chain(&job.reviewed_sources)
                        .chain(&progress_reviewed)
                        .any(|cite| {
                            cite.entry_ref == source.entry_ref && cite.version == source.version
                        });
                if relied {
                    dependencies.insert(entry_id(&source.entry_ref)?, source.version);
                }
            }
            let dependencies = dependencies.into_iter().collect::<Vec<_>>();
            crate::dreamer_subject::bind_dependencies(&mut tx, &auth, &mut scope, &dependencies)
                .await?;
            candidate.subject_scope = Some(scope);
            job.snapshot_generation
        } else {
            if candidate.subject_ref.is_some() {
                return Err(ApiError::invalid(
                    "subject candidates require the research request envelope",
                ));
            }
            a.frozen_generation
        };
        if research_job.is_none()
            && candidate.evidence_scope.is_none()
            && candidate.sources.iter().any(|s| {
                !data
                    .inputs
                    .iter()
                    .any(|i| i.entry_ref == s.entry_ref && i.version == s.version)
                    && !a
                        .narrative_context
                        .iter()
                        .any(|i| i.entry_ref == s.entry_ref && i.version == s.version)
                    && !data
                        .items
                        .iter()
                        .filter(|i| pending(i))
                        .flat_map(|i| &i.candidate.sources)
                        .any(|old| old.entry_ref == s.entry_ref && old.version == s.version)
            })
        {
            return Err(ApiError::invalid(
                "candidate source was not admitted or retained for this attempt",
            ));
        }
        let location = candidate.evidence_scope.is_some();
        source_versions(
            &mut tx,
            user,
            &mut candidate.sources,
            candidate_generation,
            !location && research_job.is_none(),
        )
        .await?;
        if location {
            let work = a
                .location_work
                .as_ref()
                .ok_or_else(|| ApiError::invalid("no historical day was admitted"))?;
            let scope = candidate.evidence_scope.as_ref().expect("scope");
            if ["from", "to", "timezone", "fingerprint"]
                .iter()
                .any(|key| scope[*key] != work[*key])
                || scope.get("context_sources") != work.get("context_sources")
                || candidate.path.as_deref()
                    != Some(
                        format!(
                            "derived/location/{}.md",
                            work["date"].as_str().unwrap_or("")
                        )
                        .as_str(),
                    )
            {
                return Err(ApiError::invalid(
                    "location candidate must match the admitted closed day and fingerprint",
                ));
            }
            candidate.evidence_scope =
                Some(validate_location_candidate(&mut tx, &auth, &candidate).await?);
        }
        let before = match candidate.path.as_deref() {
            Some(path) => load_entry(&mut tx, user, path).await?,
            None => None,
        };
        if candidate.kind != "question"
            && candidate.expected_version != Some(before.as_ref().map_or(0, |e| e.version))
        {
            return Err(ApiError::conflict(
                "dreamer_output_changed",
                "candidate output changed; recompile before review",
                json!({"path":candidate.path}),
            ));
        }
        let before_md = before.map(|e| e.content).unwrap_or_default();
        compile_related_candidate(&mut candidate, &before_md)?;
        validate_candidate(&candidate, &before_md).map_err(|error| {
            if subject_submission {
                research::repair_error(
                    error,
                    crate::dreamer::research::RepairPhase::CandidateValidation,
                )
            } else {
                error
            }
        })?;
        let hash = digest(&candidate);
        if data.items.iter().any(|i| i.candidate_hash == hash) {
            continue;
        }
        if decisions
            .lines()
            .any(|line| line.contains("veto") && line.contains(&hash))
        {
            continue;
        }
        if location {
            let known = a.location_work.clone().expect("validated location work");
            data.location_scopes
                .retain(|old| old["date"] != known["date"] || old["timezone"] != known["timezone"]);
            if data.location_scopes.len() >= 31 {
                data.location_scopes.remove(0);
            }
            data.location_scopes.push(known.clone());
            data.location_work.retain(|work| {
                work["date"] != known["date"] || work["timezone"] != known["timezone"]
            });
        }
        if let Some(id) = &candidate.revises_item_id {
            let index =
                data.items.iter().position(|i| &i.id == id).ok_or_else(|| {
                    ApiError::invalid("revised item is not in the retained inbox")
                })?;
            let old = &data.items[index];
            if !pending(old) || old.status == "deferred" || old.status == "approved_held" {
                return Err(ApiError::invalid(
                    "a rejected, deferred or approved candidate cannot be silently replaced",
                ));
            }
            let id = old.id.clone();
            let created_at = old.created_at;
            let run_id = old.run_id.clone();
            data.items[index] = Item {
                id: id.clone(),
                run_id,
                run_entry_ref: String::new(),
                run_version: 0,
                candidate_hash: hash,
                candidate,
                status: "pending".into(),
                reviewable: true,
                before_md,
                published: None,
                frozen_generation: candidate_generation,
                created_at,
            };
            ids.push(id);
            continue;
        }
        if data.items.len() >= MAX_ITEMS {
            return Err(ApiError::invalid(
                "review inbox is full; admitted input and existing proposals remain retained",
            ));
        }
        let n = data.next_item.entry(a.date.clone()).or_default();
        *n += 1;
        let id = format!("{}/{}", a.date, n);
        ids.push(id.clone());
        data.items.push(Item {
            id,
            run_id: a.date.clone(),
            run_entry_ref: String::new(),
            run_version: 0,
            candidate_hash: hash,
            candidate,
            status: "pending".into(),
            reviewable: true,
            before_md,
            published: None,
            frozen_generation: candidate_generation,
            created_at: Utc::now(),
        });
    }
    let processed = body
        .get("processed_inputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !processed.is_empty()
        && ids.is_empty()
        && body
            .get("findings")
            .and_then(Value::as_array)
            .is_none_or(|a| a.is_empty())
    {
        return Err(ApiError::invalid(
            "processed sources require an accepted candidate or explicit disposition finding",
        ));
    }
    for p in &processed {
        let r = string(p, "entry_ref")?;
        let v = integer(p, "version")?;
        let g = integer(p, "generation")?;
        if !data
            .inputs
            .iter()
            .any(|i| i.entry_ref == r && i.version == v && i.generation == g)
        {
            return Err(ApiError::invalid(
                "processed input was not durably admitted",
            ));
        }
    }
    if let Some((job, _)) = &research_job {
        research::validate_processed(job, &processed, body.get("research_progress"))?;
        research_comparison::resolve_processed(
            &mut tx,
            &auth,
            &mut data,
            job,
            &processed,
            body.get("research_progress"),
            Some(&ids),
        )
        .await?;
    } else {
        research_comparison::validate_legacy_processing(&data, &processed)?;
    }
    let before_count = data.inputs.len();
    data.inputs.retain(|i| {
        !processed.iter().any(|p| {
            p["entry_ref"] == i.entry_ref
                && p["version"] == i.version
                && p["generation"] == i.generation
        })
    });
    data.processed_count += before_count - data.inputs.len();
    data.processed_generation = data
        .inputs
        .iter()
        .map(|i| i.generation - 1)
        .min()
        .unwrap_or(data.scanned_generation)
        .min(data.scanned_generation);
    let detail = body
        .get("findings")
        .and_then(Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if detail.len() > 16000 {
        return Err(ApiError::invalid("findings exceed 16 KiB"));
    }
    let run = write_run(
        &state,
        &mut tx,
        &auth,
        &mut data,
        &a,
        "review_ready",
        &detail,
    )
    .await?;
    if !ids.is_empty() {
        let key = format!("dreaming-review-{}", a.attempt_id);
        if let Some(prior) = data
            .pending_notifications
            .iter_mut()
            .find(|n| n["event_key"] == key)
        {
            prior["run_entry_ref"] = run["entry_ref"].clone();
            prior["run_version"] = run["version"].clone();
            prior["count"] = json!(prior["count"].as_u64().unwrap_or(0) + ids.len() as u64);
        } else {
            data.pending_notifications.push(json!({"status":"pending","event_key":key,"target_kind":"review","run_entry_ref":run["entry_ref"],"run_version":run["version"],"count":ids.len()}));
        }
    }
    let mut response = json!({"state_version":version+1,"run_entry_ref":run["entry_ref"],"run_version":run["version"],"accepted_candidate_ids":ids,"pending_count":data.items.iter().filter(|i|pending(i)).count()});
    if let Some((mut job, job_version)) = research_job {
        let before_job = job.clone();
        if let Some(progress) = body.get("research_progress") {
            research::apply_progress(&mut tx, &auth, &mut job, job_version, progress).await?;
        } else {
            job.status = "waiting".into();
            job.retry_at = Utc::now() + Duration::hours(24);
        }
        let cited = data
            .items
            .iter()
            .filter(|item| ids.contains(&item.id))
            .flat_map(|item| item.candidate.sources.clone())
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            research::require_changed_reviewed(&job, &cited)?;
            let hash = data
                .items
                .iter()
                .find(|item| ids.contains(&item.id) && item.candidate.kind == "summary")
                .map(|item| item.candidate_hash.clone());
            research::complete_pass(&mut job, "accepted", hash, &cited);
        }
        job.accepted_candidate_ids = ids.clone();
        let mut progress = body.get("research_progress").cloned().unwrap_or(json!({}));
        if progress.get("findings").is_none() {
            progress["findings"] = body["findings"].clone();
        }
        let mut checkpoint_receipt = research::checkpoints::apply(
            &mut tx,
            &auth,
            &before_job,
            &mut job,
            job_version,
            &progress,
            !ids.is_empty(),
            !ids.is_empty(),
        )
        .await?;
        let (operation_id, hash) = research_operation.expect("research operation");
        if let Some(ack) = research_comparison::resolve_sources(
            &state,
            &mut tx,
            &auth,
            &mut data,
            &job,
            source_resolutions,
            &progress,
            Some(&ids),
            &operation_id,
        )
        .await?
        {
            response["follow_up_receipt"] = ack;
        }
        if research_comparison::retains_source_work(&data, &job.subject_ref) {
            job.status = "researching".into();
            job.retry_at = Utc::now();
            if let Some(ack) = &mut checkpoint_receipt {
                ack["subject_complete"] = json!(false);
            }
        }
        if let Some(ack) =
            research::drafts::accepted(&state, &mut tx, &auth, draft, &ids, &operation_id).await?
        {
            response["draft_receipt"] = ack;
        }
        let subject_complete = !ids.is_empty()
            && !research_comparison::retains_source_work(&data, &job.subject_ref)
            && (job.schema != "dream.research.v2" || job.status != "researching");
        if !ids.is_empty() {
            job.repair_feedback = None;
            research::clear_revalidation(&mut job);
        }
        if subject_complete && !research_comparison::retains_priority(&data, &job.subject_ref) {
            data.research
                .requested_subject_refs
                .retain(|reference| reference != &job.subject_ref);
            data.research
                .follow_up_priorities
                .retain(|reference| reference != &job.subject_ref);
        }
        data.research.completed += usize::from(subject_complete);
        if let Some(ack) = &mut checkpoint_receipt {
            ack["operation_id"] = json!(operation_id);
            response["checkpoint_receipt"] = ack.clone();
        }
        research::remember(
            &mut job.receipts,
            &auth,
            &operation_id,
            &hash,
            response.clone(),
        );
        research::save(&state, &mut tx, &auth, &job, job_version).await?;
        let mut refreshed = admission_response(&mut tx, &auth, &data, version + 1).await?;
        for (key, value) in response.as_object().expect("candidate response") {
            refreshed[key] = value.clone();
        }
        response = refreshed;
    }
    if !subject_submission {
        data.candidate_submission = Some(
            json!({"attempt_id":a.attempt_id,"producer":auth.credential_id.0,"request_hash":request_hash,"response":response}),
        );
    }
    save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(response))
}
/// Keep retry authority flat. A failed result may contain prior retry results;
/// persisting that entire envelope recursively grows the retained outbox.
fn notification_replay_spec(value: &Value) -> Value {
    let mut result = serde_json::Map::new();
    for key in [
        "status",
        "event_key",
        "target_kind",
        "title",
        "body",
        "run_entry_ref",
        "run_version",
        "count",
    ] {
        if let Some(field) = value.get(key) {
            result.insert(key.into(), field.clone());
        }
    }
    Value::Object(result)
}

fn notification_fact(value: &Value) -> Value {
    let mut fact = notification_replay_spec(value);
    if let Some(detail) = value.get("detail").and_then(Value::as_str) {
        fact["detail"] = json!(detail.chars().take(2000).collect::<String>());
    }
    if let Some(ack) = value.get("ack") {
        let mut fields = serde_json::Map::new();
        for key in [
            "notification_ref",
            "created",
            "delivery_status",
            "delivery_count",
            "replayed",
            "queued_deliveries",
            "status",
        ] {
            if let Some(field) = ack.get(key) {
                fields.insert(key.into(), field.clone());
            }
        }
        fact["ack"] = Value::Object(fields);
    }
    fact
}

#[cfg(test)]
mod notification_retention_tests {
    use super::*;

    #[test]
    fn repeated_failed_events_stay_flat_fair_and_replayable() {
        let mut data = RunState::default();
        for index in 0..64 {
            let retries = data
                .pending_notifications
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>();
            let notification = json!({"status":"failed","event_key":format!("review-{index}"),"target_kind":"review","run_entry_ref":"entry:fixture","run_version":index+1,"count":1,"retry_results":retries});
            let fact = record_notifications(&mut data, &notification).unwrap();
            assert!(serde_json::to_vec(&fact).unwrap().len() < 4000);
            assert_eq!(data.pending_notifications.len(), index + 1);
            assert!(
                data.pending_notifications
                    .iter()
                    .all(|item| item.get("retry_results").is_none() && item.get("ack").is_none())
            );
        }
        assert!(
            serde_json::to_vec(&data.pending_notifications)
                .unwrap()
                .len()
                < 24 * 1024
        );
        let before = data.pending_notifications.clone();
        let retries = before.iter().take(8).cloned().collect::<Vec<_>>();
        record_notifications(
            &mut data,
            &json!({"status":"not_needed","retry_results":retries}),
        )
        .unwrap();
        assert_eq!(
            data.pending_notifications[0], before[8],
            "permanent failures cannot starve later notifications"
        );
        let mut accepted = before[0].clone();
        accepted["status"] = json!("accepted");
        accepted["ack"] = json!({"notification_ref":"notification:fixture","delivery_status":"no_installations","retry_results":[{"private":"not an ack field"}]});
        let fact = record_notifications(&mut data, &accepted).unwrap();
        assert_eq!(data.pending_notifications.len(), 63);
        assert!(
            !data
                .pending_notifications
                .iter()
                .any(|item| item["event_key"] == before[0]["event_key"])
        );
        assert_eq!(fact["ack"]["delivery_status"], "no_installations");
        assert!(fact["ack"].get("retry_results").is_none());
    }
}

fn record_notifications(data: &mut RunState, notification: &Value) -> ApiResult<Value> {
    let retries = notification
        .get("retry_results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if retries.len() > 8 {
        return Err(ApiError::invalid(
            "notification retry results exceed the attempt bound",
        ));
    }
    let mut fact = notification_fact(notification);
    fact["retry_results"] = json!(retries.iter().map(notification_fact).collect::<Vec<_>>());
    if serde_json::to_vec(&fact)?.len() > 24 * 1024 {
        return Err(ApiError::invalid(
            "notification facts exceed the attempt byte bound",
        ));
    }
    // Also normalize any previously retained envelope during recovery.
    data.pending_notifications = data
        .pending_notifications
        .iter()
        .map(notification_replay_spec)
        .collect();
    for value in retries.iter().chain(std::iter::once(notification)) {
        let Some(key) = value
            .get("event_key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
        else {
            continue;
        };
        if value["status"] == "accepted" {
            data.pending_notifications
                .retain(|item| item["event_key"] != key);
        } else if value["status"] == "failed" || value["status"] == "pending" {
            let retained = data
                .pending_notifications
                .iter()
                .find(|item| item["event_key"] == key)
                .cloned();
            data.pending_notifications
                .retain(|item| item["event_key"] != key);
            // Failed retries move to the tail so one unavailable producer cannot
            // indefinitely prevent later accepted runs from being notified.
            data.pending_notifications
                .push(retained.unwrap_or_else(|| notification_replay_spec(value)));
        }
    }
    Ok(fact)
}

pub async fn finish(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(mut body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    let user = auth.user_id.0;
    let (mut data, version) = load_state(&mut tx, user).await?;
    let identity = json!({"attempt_id":body["attempt_id"],"fence":body["fence"],"producer":auth.credential_id.0,"request_hash":digest(&body)});
    if data.last_attempt.as_ref().and_then(|v| v.get("attempt_id")) == body.get("attempt_id")
        && data.active.is_none()
    {
        if data.finish_identity.as_ref() != Some(&identity) {
            return Err(conflict(
                "Terminal retry must preserve the original request, fence and producer",
                version,
            ));
        }
        if let Some(response) = data.finish_response {
            return Ok(Json(response));
        }
    }
    if mode(&mut tx, user).await?.is_none() {
        return Err(ApiError::invalid(
            "CONTROL is paused; terminal workspace persistence is held",
        ));
    }
    // Owner decisions may change state while the model runs. Merge the terminal
    // facts into the locked current state; the active run fence is still required.
    body["expected_state_version"] = json!(version);
    let a = active(&data, &body, &auth, version)?;
    let mut outcome = string(&body, "outcome")?.to_owned();
    if outcome == "completed" && (!data.inputs.is_empty() || !data.location_work.is_empty()) {
        outcome = "partial".into();
    }
    if !matches!(
        outcome.as_str(),
        "completed" | "partial" | "skipped" | "failed"
    ) {
        return Err(ApiError::invalid("invalid terminal outcome"));
    }
    let mut detail = body
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(2000)
        .collect::<String>();
    if body["outcome"] == "completed" && outcome == "partial" && body.get("research").is_some() {
        detail.push_str(&format!(" Selected research completed; {} historical inputs and {} location days remain retained.", data.inputs.len(), data.location_work.len()));
    }
    let mut research_progress = serde_json::Map::new();
    for key in [
        "rounds",
        "subjects_completed",
        "subjects_yielded",
        "processed_inputs",
        "new_review_items",
        "change_pages",
        "routed_discoveries",
    ] {
        if let Some(value) = body["research"][key].as_u64() {
            research_progress.insert(key.into(), json!(value));
        }
    }
    if let Some(reason) = body["research"]["stop_reason"].as_str() {
        research_progress.insert(
            "stop_reason".into(),
            json!(reason.chars().take(300).collect::<String>()),
        );
    }
    let completed = Utc::now();
    let notification = record_notifications(&mut data, &body["notification"])?;
    data.last_attempt = Some(
        json!({"attempt_id":a.attempt_id,"producer_credential_id":a.producer_credential_id,"date":a.date,"outcome":outcome,"detail":detail,"started_at":a.started_at,"finished_at":completed,"auth_persistence":body["auth_persistence"],"notification":notification,"execution_outcome":body["execution_outcome"],"model":body["model"],"codex_version":body["codex_version"],"counts":counts(&data),"research":research_progress,"project_status":if a.project_status.is_null(){body.get("project_status").filter(|value|crate::dreamer::receipt::project_status(value).is_ok()).cloned().unwrap_or(Value::Null)}else{a.project_status.clone()}}),
    );
    let run = write_run(&state, &mut tx, &auth, &mut data, &a, &outcome, &detail).await?;
    if outcome == "completed" {
        data.last_successful_run = Some(
            json!({"run_id":a.date,"entry_ref":run["entry_ref"],"version":run["version"],"at":completed}),
        );
    }
    let latest = projection(&data, &a, &run, &outcome, completed);
    write_projection(&state, &mut tx, &auth, &latest).await?;
    data.active = None;
    let response = json!({"state_version":version+1,"run_entry_ref":run["entry_ref"],"run_version":run["version"],"latest_receipt":latest,"counts":counts(&data)});
    data.finish_identity = Some(identity);
    data.finish_response = Some(response.clone());
    save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    state.workspace_features.invalidate(user).await;
    Ok(Json(response))
}

async fn item_available(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    item: &Item,
) -> ApiResult<bool> {
    let ids = item
        .candidate
        .sources
        .iter()
        .map(|s| s.entry_ref.as_str())
        .chain(
            item.candidate
                .subject_scope
                .iter()
                .flat_map(|scope| scope.dependencies.iter().map(|s| s.entry_ref.as_str())),
        )
        .map(entry_id)
        .collect::<ApiResult<std::collections::BTreeSet<_>>>()?
        .into_iter()
        .collect::<Vec<_>>();
    if !ids.is_empty() {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM brunn.entries WHERE user_id=$1 AND id=ANY($2) AND deleted_at IS NULL")
            .bind(user).bind(&ids).fetch_one(&mut **tx).await?;
        if count != ids.len() as i64 {
            return Ok(false);
        }
    }
    if item.candidate.expected_version.unwrap_or(0) > 0
        && let Some(path) = &item.candidate.path
        && load_entry(tx, user, path).await?.is_none()
    {
        return Ok(false);
    }
    Ok(true)
}
fn withhold_item(item: &Item) -> Item {
    let mut shown = item.clone();
    shown.candidate.title = "Evidence unavailable".into();
    shown.candidate.content = None;
    shown.candidate.summary.clear();
    shown.candidate.reason.clear();
    shown.candidate.uncertainty.clear();
    shown.candidate.question.clear();
    shown.candidate.sources.clear();
    shown.candidate.raw_sources.clear();
    shown.candidate.evidence_scope = None;
    shown.candidate.subject_ref = None;
    shown.candidate.subject_scope = None;
    shown.candidate.path = None;
    shown.before_md.clear();
    shown.reviewable = false;
    shown.status = "stale".into();
    shown
}
pub(crate) struct ItemFreshness {
    /// Evidence is unavailable, the identity changed or the target moved. A
    /// stale item cannot be published as-is.
    pub stale: bool,
    /// Pinned reliance evidence has newer versions. The dated proposal stays
    /// reviewable; refresh is background work for the next research pass.
    pub refresh_pending: bool,
}

async fn item_stale(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    item: &Item,
) -> ApiResult<bool> {
    Ok(item_freshness(tx, auth, item).await?.stale)
}

#[tracing::instrument(skip_all, fields(item_id = %item.id, candidate_hash = %item.candidate_hash))]
async fn item_freshness(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    item: &Item,
) -> ApiResult<ItemFreshness> {
    let stale = item_stale_inner(tx, auth, item).await?;
    let mut refresh_pending = false;
    if !stale && let Some(scope) = &item.candidate.subject_scope {
        let dependencies = item
            .candidate
            .sources
            .iter()
            .map(|source| Ok((entry_id(&source.entry_ref)?, source.version)))
            .collect::<ApiResult<Vec<_>>>()?;
        refresh_pending = !crate::dreamer_subject::check_scope(tx, auth, scope, &dependencies)
            .await?
            .changed
            .is_empty();
    }
    Ok(ItemFreshness {
        stale,
        refresh_pending,
    })
}

async fn item_stale_inner(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    item: &Item,
) -> ApiResult<bool> {
    let user = auth.user_id.0;
    if !item.reviewable {
        return Ok(false);
    }
    // Apply source-class changes before location/subject branches can return
    // early. Old immutable proposals retain every dependency and need review.
    if item.candidate.sources.len() > 64 {
        tracing::info!(
            reason = "candidate_source_limit",
            "review freshness rejected"
        );
        return Ok(true);
    }
    if !item.candidate.sources.is_empty() {
        let ids = item
            .candidate
            .sources
            .iter()
            .map(|source| entry_id(&source.entry_ref))
            .collect::<ApiResult<Vec<_>>>()?;
        let versions = item
            .candidate
            .sources
            .iter()
            .map(|source| source.version)
            .collect::<Vec<_>>();
        let generated: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM unnest($2::uuid[],$3::bigint[]) AS selected(id,version)
                LEFT JOIN brunn.entries e ON e.user_id=$1 AND e.id=selected.id
                LEFT JOIN LATERAL (
                    SELECT metadata->>'kind' AS kind FROM brunn.entry_versions
                    WHERE user_id=$1 AND entry_id=selected.id AND version=selected.version LIMIT 1
                ) exact ON true
                LEFT JOIN LATERAL (
                    SELECT metadata->>'kind' AS kind FROM brunn.entry_versions
                    WHERE user_id=$1 AND entry_id=selected.id AND version=e.current_version LIMIT 1
                ) current ON true
                WHERE exact.kind='briefing_edition' OR current.kind='briefing_edition'
            )
        "#,
        )
        .bind(user)
        .bind(ids)
        .bind(versions)
        .fetch_one(&mut **tx)
        .await?;
        if generated {
            tracing::info!(
                reason = "generated_briefing_source",
                "review freshness rejected"
            );
            return Ok(true);
        }
    }
    if let Some(scope) = &item.candidate.evidence_scope {
        if !auth.can(Capability::Save) && !auth.can(Capability::DreamerRun) {
            return Ok(true);
        }
        let query = serde_json::from_value(
            json!({"from":scope["from"],"to":scope["to"],"timezone":scope["timezone"]}),
        )?;
        let mut internal = auth.clone();
        internal.capabilities.insert("save".into());
        let packet = crate::location::evidence::evidence_in_tx(tx, &internal, &query).await?;
        if packet["fingerprint_complete"] != true
            || packet["evidence_fingerprint"] != scope["fingerprint"]
            || !crate::location::summary::context_sources_current_in_tx(tx, &internal, scope)
                .await?
        {
            return Ok(true);
        }
        if let Some(path) = &item.candidate.path {
            return Ok(item.candidate.expected_version
                != Some(load_entry(tx, user, path).await?.map_or(0, |e| e.version)));
        }
        return Ok(false);
    }
    // Subject proposals cite evidence pinned at their pass cutoff: a newer
    // head is refresh work, not invalidation. Only lost access stales them.
    let pinned = item.candidate.subject_scope.is_some();
    for (citation_index, source) in item.candidate.sources.iter().enumerate() {
        let current:Option<i64>=sqlx::query_scalar("SELECT current_version FROM brunn.entries WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL")
            .bind(user).bind(entry_id(&source.entry_ref)?).fetch_optional(&mut **tx).await?;
        if current.is_none() || (!pinned && current != Some(source.version)) {
            // Full source authority has not passed yet. Identify the existing
            // citation by index, without disclosing a revoked head or identity.
            tracing::info!(
                citation_index,
                reason = "cited_source_changed_or_unavailable",
                "review freshness rejected"
            );
            return Ok(true);
        }
    }
    if let Some(path) = &item.candidate.path
        && item.candidate.expected_version
            != Some(load_entry(tx, user, path).await?.map_or(0, |e| e.version))
    {
        tracing::info!(
            reason = "candidate_target_changed",
            "review freshness rejected"
        );
        return Ok(true);
    }
    if let Some(scope) = &item.candidate.subject_scope {
        let dependencies = item
            .candidate
            .sources
            .iter()
            .map(|source| Ok((entry_id(&source.entry_ref)?, source.version)))
            .collect::<ApiResult<Vec<_>>>()?;
        return Ok(
            crate::dreamer_subject::check_scope(tx, auth, scope, &dependencies)
                .await?
                .status
                != "fresh",
        );
    }
    if item.candidate.kind == "summary" {
        for prefix in item
            .candidate
            .sources
            .iter()
            .filter_map(|s| s.path.rsplit_once('/').map(|(p, _)| format!("{p}/")))
            .collect::<std::collections::BTreeSet<_>>()
        {
            let changed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM brunn.workspace_changes change LEFT JOIN LATERAL (SELECT old.entry_version FROM brunn.workspace_changes old WHERE old.user_id=change.user_id AND old.entry_id=change.entry_id AND old.generation<change.generation ORDER BY old.generation DESC LIMIT 1) previous ON true LEFT JOIN brunn.entry_versions change_v ON change_v.user_id=change.user_id AND change_v.entry_id=change.entry_id AND change_v.version=change.entry_version LEFT JOIN brunn.entry_versions previous_v ON previous_v.user_id=change.user_id AND previous_v.entry_id=change.entry_id AND previous_v.version=previous.entry_version WHERE change.user_id=$1 AND change.generation>$2 AND starts_with(change.path,$3) AND (coalesce(change_v.metadata->>'kind','')<>'briefing_edition' OR previous.entry_version IS NOT NULL AND coalesce(previous_v.metadata->>'kind','')<>'briefing_edition'))")
                .bind(user).bind(item.frozen_generation).bind(prefix).fetch_one(&mut **tx).await?;
            if changed {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Legacy state intentionally stores compact previews. Read the complete proposal
/// from the immutable run it came from; never rewrite its decision identity/hash
/// or substitute a newer run. Both tables remain subject to the caller's RLS.
async fn legacy_review_texts(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    data: &RunState,
) -> ApiResult<std::collections::BTreeMap<String, Option<String>>> {
    let mut runs = std::collections::BTreeMap::new();
    let mut texts = std::collections::BTreeMap::new();
    for item in &data.legacy_items {
        let key = (entry_id(&item.run_entry_ref)?, item.run_version);
        if let std::collections::btree_map::Entry::Vacant(slot) = runs.entry(key) {
            let row = sqlx::query("SELECT e.path,v.content,v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3 WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL")
                .bind(user).bind(key.0).bind(key.1).fetch_optional(&mut **tx).await?;
            slot.insert(row.map(|row| {
                (
                    row.get::<String, _>("path"),
                    row.get::<Value, _>("metadata"),
                    legacy_proposal_texts(&row.get::<String, _>("content")),
                )
            }));
        }
        let text =
            runs.get(&key)
                .and_then(Option::as_ref)
                .and_then(|(path, metadata, paragraphs)| {
                    if path != &format!("dreams/runs/{}.md", item.run_id)
                        || metadata.get("dreamer_run").is_some()
                    {
                        return None;
                    }
                    let number = item.id.strip_prefix(&format!("{}/", item.run_id))?;
                    let (section, number) = if item.candidate.kind == "question" {
                        ("Needs your call", number.strip_prefix("question-")?)
                    } else {
                        ("Proposed", number)
                    };
                    paragraphs
                        .get(&(section.to_owned(), number.parse::<usize>().ok()?))?
                        .clone()
                });
        texts.insert(item.id.clone(), text);
    }
    Ok(texts)
}

fn legacy_fence_opening(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let character = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = trimmed.chars().take_while(|c| *c == character).count();
    // Backticks in an info string make this ordinary inline text, not a fence.
    (length >= 3 && (character != '`' || !trimmed[length..].contains('`')))
        .then_some((character, length))
}

fn legacy_proposal_texts(
    content: &str,
) -> std::collections::BTreeMap<(String, usize), Option<String>> {
    let mut result = std::collections::BTreeMap::new();
    let mut section = String::new();
    let mut current: Option<(String, usize)> = None;
    let mut fence: Option<(char, usize)> = None;
    for line in content.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = marker.map_or(0, |c| trimmed.chars().take_while(|x| *x == c).count());
        let in_fence = fence.is_some();
        if let Some((character, length)) = fence {
            if marker == Some(character) && run >= length && trimmed[run..].trim().is_empty() {
                fence = None;
            }
        } else {
            fence = legacy_fence_opening(line);
        }
        if !in_fence && fence.is_none() {
            if line.starts_with("# ") {
                section.clear();
                current = None;
                continue;
            }
            if let Some(heading) = line.strip_prefix("## ") {
                section = heading.trim().to_owned();
                current = None;
                continue;
            }
            if matches!(section.as_str(), "Proposed" | "Needs your call")
                && let Some((number, text)) = line.split_once(". ")
                && let Ok(number) = number.parse::<usize>()
            {
                let key = (section.clone(), number);
                result
                    .entry(key.clone())
                    .and_modify(|value| *value = None)
                    .or_insert_with(|| Some(text.to_owned()));
                current = Some(key);
                fence = legacy_fence_opening(text);
                continue;
            }
        }
        if let Some(key) = &current
            && let Some(Some(text)) = result.get_mut(key)
        {
            text.push('\n');
            text.push_str(line);
        }
    }
    for text in result.values_mut().flatten() {
        *text = text.trim_end().to_owned();
    }
    result
}

#[cfg(test)]
mod legacy_review_tests {
    use super::*;

    #[test]
    fn history_migration_preserves_progress_and_requires_exact_import_provenance() {
        let imported: Item = serde_json::from_value(json!({
            "id":"2020-01-01/1","run_id":"2020-01-01",
            "run_entry_ref":format!("entry:{}",Uuid::now_v7()),"run_version":1,
            "candidate_hash":"original-immutable-hash",
            "candidate":{"kind":"legacy","title":"Old promise","summary":"Original cached text","reason":LEGACY_REASON},
            "status":"deferred","reviewable":false,"frozen_generation":0,"created_at":Utc::now()
        })).unwrap();
        let mut current_question = imported.clone();
        current_question.id = "current-question".into();
        current_question.candidate.kind = "question".into();
        current_question.reviewable = true;
        let mut other_prose = imported.clone();
        other_prose.id = "other-prose".into();
        other_prose.candidate.reason = "A different provenance".into();
        let mut terminal = imported.clone();
        terminal.id = "2020-01-01/question-2".into();
        terminal.candidate.kind = "question".into();
        terminal.status = "rejected".into();
        let mut data = RunState {
            items: vec![
                imported.clone(),
                current_question.clone(),
                other_prose.clone(),
                terminal.clone(),
            ],
            inputs: vec![Input {
                entry_ref: "entry:source".into(),
                path: "sources/Unprocessed.md".into(),
                version: 3,
                generation: 42,
                operation: "update".into(),
                content_hash: "source-hash".into(),
            }],
            scanned_generation: 100,
            processed_generation: 41,
            processed_count: 5,
            next_item: std::collections::BTreeMap::from([("2020-01-01".into(), 2)]),
            history: vec![json!({"decision":"reject","item_id":terminal.id})],
            legacy_scan_after: "dreams/runs/2020-01-01.md".into(),
            legacy_complete: true,
            ..RunState::default()
        };
        let mut expected = serde_json::to_value(&data).unwrap();
        expected["items"] = json!([current_question, other_prose]);
        expected["legacy_items"] = json!([imported, terminal]);
        separate_legacy_history(&mut data).unwrap();
        assert_eq!(serde_json::to_value(&data).unwrap(), expected);
        separate_legacy_history(&mut data).unwrap();
        assert_eq!(
            serde_json::to_value(&data).unwrap(),
            expected,
            "repeat loading is lossless"
        );
    }

    #[test]
    fn inline_backticks_cannot_swallow_the_next_proposal() {
        let items = legacy_proposal_texts("## Proposed\n1. Original\n\n```literal```\n\n2. Other");
        assert_eq!(
            items[&("Proposed".into(), 1)].as_deref(),
            Some("Original\n\n```literal```")
        );
        assert_eq!(items[&("Proposed".into(), 2)].as_deref(), Some("Other"));
    }

    #[test]
    fn first_line_fence_preserves_code_and_real_item_boundaries() {
        let items = legacy_proposal_texts(
            "## Proposed\n1. ```markdown\n   2. Code, not an item\n   ## Needs your call\n   ```\n\n2. Actual next item\n\n## Needs your call\n1. Actual question",
        );
        assert_eq!(
            items[&("Proposed".into(), 1)].as_deref(),
            Some("```markdown\n   2. Code, not an item\n   ## Needs your call\n   ```")
        );
        assert_eq!(
            items[&("Proposed".into(), 2)].as_deref(),
            Some("Actual next item")
        );
        assert_eq!(
            items[&("Needs your call".into(), 1)].as_deref(),
            Some("Actual question")
        );
    }

    #[test]
    fn repeated_numbers_are_unavailable_instead_of_silently_picking_one() {
        let items = legacy_proposal_texts("## Proposed\n1. First\n\n1. Conflicting second\n");
        assert_eq!(items[&("Proposed".into(), 1)], None);
    }

    #[test]
    fn new_report_heading_ends_the_proposal_but_subheadings_do_not() {
        let items = legacy_proposal_texts(
            "## Proposed\n1. First\n\n### Details\nKeep these.\n\n# Another report\nWithhold this.",
        );
        assert_eq!(
            items[&("Proposed".into(), 1)].as_deref(),
            Some("First\n\n### Details\nKeep these.")
        );
    }
}

async fn review_view(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    version: i64,
    current_mode: Option<String>,
) -> ApiResult<Value> {
    let user = auth.user_id.0;
    let legacy_texts = legacy_review_texts(tx, user, data).await?;
    let mut items = Vec::new();
    let mut legacy_items = Vec::new();
    for original in data
        .items
        .iter()
        .filter(|i| pending(i))
        .chain(&data.legacy_items)
    {
        let legacy = is_legacy_history(original);
        let legacy_text = legacy_texts.get(&original.id);
        let available = !matches!(legacy_text, Some(None))
            && (original.candidate.raw_sources.is_empty() || auth.can(Capability::Save))
            && item_available(tx, user, original).await?;
        let shown = if available {
            original.clone()
        } else {
            withhold_item(original)
        };
        let i = &shown;
        let freshness = if available {
            item_freshness(tx, auth, original).await?
        } else {
            ItemFreshness {
                stale: true,
                refresh_pending: false,
            }
        };
        let stale = freshness.stale;
        let block = if !available {
            Some("Evidence is no longer available. Cached proposal text has been withheld.")
        } else if legacy {
            Some("Historical note; no generated candidate or decision is pending.")
        } else if stale {
            Some("Sources or the target changed. A new candidate must be reviewed.")
        } else if !i.reviewable {
            Some("This older proposal describes work but has no generated candidate to approve.")
        } else {
            None
        };
        let preview = if i.candidate.kind == "question" || legacy {
            Value::Null
        } else {
            json!({"body_md":candidate_body(&i.candidate),"before_md":i.before_md,"after_md":candidate_body(&i.candidate),"target_path":i.candidate.path})
        };
        let mut sources=i.candidate.sources.iter().map(|s|json!({"entry_ref":s.entry_ref,"path":s.path,"version":s.version,"label":format!("{} v{} · lines {}–{}",s.path,s.version,s.start_line,s.end_line),"excerpt":s.excerpt})).collect::<Vec<_>>();
        if available && legacy_text.is_some() {
            sources.push(json!({"entry_ref":i.run_entry_ref,"path":format!("dreams/runs/{}.md",i.run_id),"version":i.run_version,"label":format!("Original proposal · {} v{}",i.run_id,i.run_version)}));
        }
        let raw = if !stale && !i.candidate.raw_sources.is_empty() && auth.can(Capability::Save) {
            crate::location::summary::citation_previews_in_tx(
                tx,
                auth,
                i.candidate
                    .evidence_scope
                    .as_ref()
                    .expect("validated raw scope"),
                &i.candidate.raw_sources,
            )
            .await?
        } else {
            vec![]
        };
        if raw.is_empty() {
            sources.extend(i.candidate.raw_sources.iter().map(|s|json!({"entry_ref":format!("location-report:{}",digest(&s.natural_key)),"label":format!("Raw location report {} ({})",s.natural_key["at"].as_str().unwrap_or(""),s.natural_key["type"].as_str().unwrap_or("")),"excerpt":format!("Exact key: {}. Fields: {}. Retained raw evidence expires after 30 days.",s.natural_key,s.fields.join(", "))})));
        } else {
            sources.extend(raw);
        }

        let full_text = legacy_text.and_then(Option::as_deref).filter(|_| available);
        let title = full_text
            .and_then(|text| text.lines().next())
            .unwrap_or(&i.candidate.title);
        let body = full_text.unwrap_or(if i.candidate.kind == "question" {
            &i.candidate.question
        } else {
            &i.candidate.summary
        });
        let destination = if legacy {
            &mut legacy_items
        } else {
            &mut items
        };
        destination.push(json!({"id":i.id,"kind":if i.candidate.kind=="question"{"question"}else{"proposal"},"legacy":legacy,"title":title,"body_md":body,"why_md":i.candidate.reason,"uncertainty_md":i.candidate.uncertainty,"run_id":i.run_id,"run_entry_ref":i.run_entry_ref,"run_version":i.run_version,"candidate_hash":i.candidate_hash,"candidate":preview,"sources":sources,"status":if stale{"stale"}else{&i.status},"reviewable":i.reviewable&&i.candidate.kind!="question","stale":stale,"refresh_pending":freshness.refresh_pending,"evidence_cutoff":i.frozen_generation,"blocked_reason":block}));
    }
    // Put actionable candidates within reach before other current items;
    // stable sorting preserves the existing order and every decision identity.
    items.sort_by_key(|item| !(item["reviewable"] == true && item["stale"] == false));
    Ok(
        json!({"available":true,"mode":current_mode,"paused":current_mode.is_none(),"last_attempt":data.last_attempt,"last_successful_run":data.last_successful_run,"counts":counts(data),"items":items,"legacy_items":legacy_items,"history":data.history,"decision_version":version}),
    )
}
pub async fn review(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::Read)?;
    let pool = if auth.can(Capability::Save) {
        &state.rw_pool
    } else {
        &state.ro_pool
    };
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    crate::db::set_context(&mut tx, &auth).await?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true)")
        .bind(format!("{}ms", state.config.request_timeout.as_millis()))
        .execute(&mut *tx)
        .await?;
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    if version == 0 {
        import_legacy(&mut tx, auth.user_id.0, &mut data).await?;
    }
    let current_mode = mode(&mut tx, auth.user_id.0).await?;
    let value = review_view(&mut tx, &auth, &data, version, current_mode).await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"complete","data":value})))
}

async fn publish_item(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    item: &mut Item,
    attempt_id: Option<&str>,
) -> ApiResult<()> {
    let user = auth.user_id.0;
    let location = item.candidate.evidence_scope.is_some();
    source_versions(
        tx,
        user,
        &mut item.candidate.sources,
        item.frozen_generation,
        !location && item.candidate.subject_scope.is_none(),
    )
    .await?;
    if location {
        item.candidate.evidence_scope =
            Some(validate_location_candidate(tx, auth, &item.candidate).await?);
    }
    if let Some(scope) = &item.candidate.subject_scope {
        let dependencies = item
            .candidate
            .sources
            .iter()
            .map(|source| Ok((entry_id(&source.entry_ref)?, source.version)))
            .collect::<ApiResult<Vec<_>>>()?;
        if item.candidate.subject_ref.as_deref() != Some(scope.subject_ref.as_str())
            || crate::dreamer_subject::check_scope(tx, auth, scope, &dependencies)
                .await?
                .status
                != "fresh"
        {
            return Err(ApiError::invalid(
                "subject summary evidence changed; retain its review identity and research it again",
            ));
        }
    }
    validate_candidate(&item.candidate, &item.before_md)?;
    let path = item.candidate.path.clone().expect("validated path");
    let mut metadata = if item.candidate.kind == "summary" {
        let prefixes: std::collections::BTreeSet<_> = item
            .candidate
            .sources
            .iter()
            .filter_map(|s| {
                s.path
                    .rsplit_once('/')
                    .map(|(prefix, _)| format!("{prefix}/"))
            })
            .collect();
        json!({"kind":"derived_summary","dreamer_summary":{"schema":"dream.summary.v1","compiler":"brunn-rust-v1","state":"published","candidate_id":item.id,"candidate_hash":item.candidate_hash,"run_entry_ref":item.run_entry_ref,"run_version":item.run_version,"compiled_at":item.created_at,"published_at":Utc::now(),"frozen_generation":item.frozen_generation,"sources":item.candidate.sources,"scope_prefixes":prefixes,"raw_sources":item.candidate.raw_sources,"evidence_scope":item.candidate.evidence_scope,"subject_ref":item.candidate.subject_ref,"subject_scope":item.candidate.subject_scope}})
    } else {
        load_entry(tx, user, &path)
            .await?
            .map(|e| e.metadata)
            .unwrap_or(json!({}))
    };
    if location && item.candidate.kind == "summary" {
        metadata["dreamer_summary"]["claim_sources"] = location_primary_content(&item.candidate).1;
        metadata["dreamer_summary"]["presentation"] = json!("location-timeline.v1");
    }
    let mut published = put_entry(
        state,
        tx,
        auth,
        &path,
        candidate_body(&item.candidate),
        metadata,
        item.candidate.expected_version.expect("validated CAS"),
    )
    .await?;
    published["attempt_id"] = json!(attempt_id);
    published["actor_credential_id"] = json!(auth.credential_id.0);
    item.status = "applied".into();
    item.published = Some(published);
    Ok(())
}

pub async fn decide(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    auth.require(Capability::Save)?;
    let user = auth.user_id.0;
    let mut tx = state.begin_write(&auth).await?;
    let (mut data, version) = load_state(&mut tx, user).await?;
    if version == 0 {
        import_legacy(&mut tx, user, &mut data).await?;
    }
    let key = string(&body, "idempotency_key")?;
    if key.len() > 200 {
        return Err(ApiError::invalid("idempotency key is too long"));
    }
    let request_hash = digest(&body);
    let audit_path = format!("dreams/reviews/{}.md", digest(&key));
    if let Some(audit) = load_entry(&mut tx, user, &audit_path).await? {
        let old = &audit.metadata["dreamer_review"]["decision"];
        if old["request_hash"] != request_hash {
            return Err(conflict(
                "Decision key already belongs to another request or its audit is unavailable",
                version,
            ));
        }
        return Ok(Json(
            json!({"status":"complete","data":{"saved":true,"decision":old["decision"],"application_status":old["application_status"],"message":"Decision already recorded.","state_version":version}}),
        ));
    }
    if let Some(old) = data.history.iter().find(|h| h["idempotency_key"] == key) {
        if old["request_hash"] != request_hash {
            return Err(ApiError::conflict(
                "idempotency_conflict",
                "this decision key was already used for a different request",
                json!({}),
            ));
        }
        return Ok(Json(
            json!({"status":"complete","data":{"saved":true,"decision":old["decision"],"application_status":old["application_status"],"message":"Decision already recorded.","state_version":version}}),
        ));
    }
    if integer(&body, "expected_decisions_version")? != version {
        return Err(conflict(
            "Review state changed; inspect the latest candidate before deciding",
            version,
        ));
    }
    let id = string(&body, "item_id")?;
    if data.legacy_items.iter().any(|item| item.id == id) {
        return Err(conflict(
            "Historical notes are read-only; no generated candidate or decision is pending",
            version,
        ));
    }
    let index = data
        .items
        .iter()
        .position(|i| i.id == id)
        .ok_or_else(|| ApiError::not_found("review_item", id))?;
    let mut item = data.items[index].clone();
    if item.candidate_hash != string(&body, "candidate_hash")?
        || item.run_entry_ref != string(&body, "run_entry_ref")?
        || item.run_version != integer(&body, "run_version")?
    {
        return Err(conflict(
            "The candidate changed; inspect its current evidence before deciding",
            version,
        ));
    }
    let decision = string(&body, "decision")?;
    if !matches!(decision, "approve" | "reject" | "defer" | "correct") {
        return Err(ApiError::invalid("invalid review decision"));
    }
    if matches!(item.status.as_str(), "rejected" | "applied" | "superseded") {
        return Err(conflict(
            "This item already has a terminal disposition",
            version,
        ));
    }
    let comment = body.get("comment").and_then(Value::as_str).unwrap_or("");
    let correction = body.get("correction").and_then(Value::as_str).unwrap_or("");
    if comment.len() > 4000 || correction.len() > 4000 {
        return Err(ApiError::invalid(
            "review comments are limited to 4000 bytes",
        ));
    }
    if decision == "correct" && correction.trim().is_empty() {
        return Err(ApiError::invalid("a correction is required"));
    }
    let current_mode = mode(&mut tx, user).await?;
    if decision == "approve" {
        if !item.reviewable
            || item.candidate.kind == "question"
            || item_stale(&mut tx, &auth, &item).await?
        {
            return Err(conflict(
                "Only a concrete candidate with unchanged sources can be approved",
                version,
            ));
        }
        if matches!(item.status.as_str(), "approved_held" | "needs_changes") {
            return Err(conflict(
                "This item is held or requires a new candidate",
                version,
            ));
        }
        // Report-only approval is durable too: old candidates must satisfy the
        // current contract before they can be held for later publication.
        validate_candidate(&item.candidate, &item.before_md)?;
        if item.candidate.evidence_scope.is_some() {
            validate_location_candidate(&mut tx, &auth, &item.candidate).await?;
        }
        if current_mode.as_deref() == Some("full") {
            publish_item(&state, &mut tx, &auth, &mut item, None).await?;
        } else {
            item.status = "approved_held".into();
        }
    } else {
        item.status = match decision {
            "reject" => "rejected",
            "defer" => "deferred",
            _ => "needs_changes",
        }
        .into();
    }
    let now = Utc::now();
    let history = json!({"id":format!("decision:{}",Uuid::now_v7()),"item_id":id,"decision":decision,"comment":comment,"correction":correction,"at":now,"application_status":item.status,"idempotency_key":key,"request_hash":request_hash,"candidate_hash":item.candidate_hash,"run_entry_ref":item.run_entry_ref,"run_version":item.run_version});
    let audit=put_entry(&state,&mut tx,&auth,&audit_path,
        format!("# Review decision\n\n{} — {}\n\n{}\n\n{}\n",item.id,decision,comment,correction),
        json!({"kind":"dreamer_review","dreamer_review":{"schema":"dream.review.v1","decision":history,"item":item,"actor_credential_id":auth.credential_id.0}}),0).await?;
    let mut compact_history = history.clone();
    compact_history["comment"] = json!(comment.chars().take(500).collect::<String>());
    compact_history["correction"] = json!(correction.chars().take(500).collect::<String>());
    compact_history["audit"] = audit;
    data.history.push(compact_history);
    if data.history.len() > 32 {
        data.history.drain(..data.history.len() - 32);
    }
    data.items[index] = item.clone();
    let old_decisions = load_entry(&mut tx, user, "dreams/decisions.md").await?;
    let mut text = old_decisions
        .as_ref()
        .map(|e| e.content.clone())
        .unwrap_or_else(|| "# Dreaming decisions\n".into());
    text.push_str(&format!(
        "\n- {} {} {} — candidate {}. {} {}\n",
        now.to_rfc3339(),
        if decision == "reject" {
            "veto"
        } else {
            decision
        },
        id,
        item.candidate_hash,
        comment.replace('\n', " "),
        correction.replace('\n', " ")
    ));
    put_entry(
        &state,
        &mut tx,
        &auth,
        "dreams/decisions.md",
        text,
        json!({"kind":"dreamer_decisions"}),
        old_decisions.map_or(0, |e| e.version),
    )
    .await?;
    // A review is an owner action, never a synthetic nightly execution. Keep
    // the last real attempt, timestamp, outcome and exact run receipt identity.
    if current_mode.is_some()
        && let Some(old) = load_entry(&mut tx, user, "dreams/latest-receipt.md").await?
        && let Ok(mut latest) = crate::dreamer::receipt::parse_latest(&old.content)
    {
        let a = Attempt {
            attempt_id: String::new(),
            fence: String::new(),
            date: item.run_id.clone(),
            producer_credential_id: String::new(),
            started_at: now,
            lease_until: now,
            frozen_generation: item.frozen_generation,
            mode: current_mode.clone().unwrap_or_default(),
            admission_hash: String::new(),
            admission_version: 0,
            location_work: None,
            project_status: Value::Null,
            narrative_context: Vec::new(),
            narrative_discovery: None,
        };
        latest["pending_owner"] =
            projection(&data, &a, &json!({}), "completed", now)["pending_owner"].clone();
        write_projection(&state, &mut tx, &auth, &latest).await?;
    }
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    state.workspace_features.invalidate(user).await;
    let message = match item.status.as_str() {
        "approved_held" => {
            "Approved and held. It is written at the first run after Dreaming is switched to full mode, after its sources are re-checked; if they change first it comes back for another look."
        }
        "applied" => "Approved and written now; its sources were re-checked first.",
        "rejected" => "Rejected. Final; nothing is written and this proposal does not come back.",
        "deferred" => "Deferred. It stays in this inbox; nothing is written.",
        _ => {
            "Correction sent. The next run drafts a new version for you to review; nothing is written until you approve it."
        }
    };
    Ok(Json(
        json!({"status":"complete","data":{"saved":true,"decision":decision,"application_status":item.status,"message":message,"state_version":version}}),
    ))
}

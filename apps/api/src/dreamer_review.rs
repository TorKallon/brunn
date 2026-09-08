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
const MAX_INPUTS: usize = 128;
const MAX_ITEMS: usize = 96;
const MAX_STATE_BYTES: usize = 256 * 1024;

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
        .route("/workspace/dreamer/candidates", post(candidates))
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
            item.candidate.summary.clear();
            item.candidate.reason.clear();
            item.candidate.uncertainty.clear();
            item.candidate.question.clear();
        }
    }
    let metadata = json!({"kind":"dreamer_state","dreamer_state":compact});
    if serde_json::to_vec(&metadata)?.len() > MAX_STATE_BYTES {
        return Err(ApiError::invalid(
            "Dreamer retained state is full; no work or cursor was discarded",
        ));
    }
    let text = format!(
        "# Dreamer progress\n\nScanned generation: {}\nProcessed generation: {}\nRetained inputs: {}\nReview items: {}\n\n[Open Review](https://brunn.ai/dreams) for decisions. Immutable run versions retain the audit.\n",
        data.scanned_generation,
        data.processed_generation,
        data.inputs.len(),
        data.items.len()
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
fn counts(data: &RunState) -> Value {
    json!({"retained":data.inputs.len(),"processed":data.processed_count,"processed_generation":data.processed_generation,"pending":data.items.iter().filter(|i|pending(i)).count(),"proposals":data.items.iter().filter(|i|pending(i)&&i.candidate.kind!="question").count(),"questions":data.items.iter().filter(|i|pending(i)&&i.candidate.kind=="question").count(),"approved_held":data.items.iter().filter(|i|i.status=="approved_held").count(),"applied":data.items.iter().filter(|i|i.status=="applied").count(),"published":data.items.iter().filter(|i|i.status=="applied").count(),"legacy_backlog":!data.legacy_complete,"retained_location_days":data.location_work.len()})
}
fn next_run(now: DateTime<Utc>) -> DateTime<Utc> {
    let local = now.with_timezone(&Los_Angeles);
    let mut date = local.date_naive();
    let time = date.and_hms_opt(3, 0, 0).expect("3am");
    let today = Los_Angeles
        .from_local_datetime(&time)
        .earliest()
        .expect("3am exists");
    if today <= local {
        date = date.succ_opt().expect("next date");
    }
    Los_Angeles
        .from_local_datetime(&date.and_hms_opt(3, 0, 0).expect("3am"))
        .earliest()
        .expect("3am exists")
        .with_timezone(&Utc)
}

async fn source_versions(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    candidate: &mut Candidate,
    frozen: i64,
    require_current: bool,
) -> ApiResult<()> {
    if candidate.sources.len() > 64 {
        return Err(ApiError::invalid(
            "a candidate may reference at most 64 source versions",
        ));
    }
    for source in &mut candidate.sources {
        let id = entry_id(&source.entry_ref)?;
        if source.version < 1 || source.start_line < 1 || source.end_line < source.start_line {
            return Err(ApiError::invalid(
                "source versions and line selectors must be positive and ordered",
            ));
        }
        let row=sqlx::query("SELECT e.path,e.current_version,v.content,EXISTS(SELECT 1 FROM brunn.workspace_changes c WHERE c.user_id=e.user_id AND c.entry_id=e.id AND c.entry_version=v.version AND c.generation<=$4) AS in_snapshot FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3 WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL")
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
        let content: Option<String> = row.get("content");
        let content = content
            .ok_or_else(|| ApiError::invalid("candidate sources must be readable Markdown"))?;
        let lines: Vec<_> = content.lines().collect();
        if source.end_line > lines.len() || source.end_line - source.start_line > 400 {
            return Err(ApiError::invalid(
                "source selector is outside its exact source version or exceeds 400 lines",
            ));
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
        if source.excerpt.len() > 12000 {
            return Err(ApiError::invalid(
                "source excerpt exceeds the candidate evidence budget",
            ));
        }
    }
    Ok(())
}
fn related_without_block(text: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim() == "## Related" {
            inside = true;
            continue;
        }
        if inside && line.starts_with("## ") {
            inside = false;
        }
        if !inside {
            out.push(line);
        }
    }
    out.join("\n").trim_end().to_owned()
}
fn validate_candidate(candidate: &Candidate, before: &str) -> ApiResult<()> {
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
    if serde_json::to_vec(candidate)?.len() > 32768 {
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
                && candidate
                    .sources
                    .iter()
                    .enumerate()
                    .any(|(index, _)| !cited_inline(&format!("[^s{}]", index + 1)))
            {
                return Err(ApiError::invalid(
                    "every declared canonical location source must be cited in the proposed summary content",
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
            for line in content
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
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
                    return Err(ApiError::invalid(
                        "every summary fact/uncertainty line needs a declared [^sN] or [^rN] citation; footnotes are rendered by the server",
                    ));
                }
            }
        }
        "related" => {
            let lower = path.to_ascii_lowercase();
            if !path.starts_with("sources/")
                || lower.ends_with("agents.md")
                || lower.ends_with("soul.md")
                || lower.contains("preferences")
                || content.matches("## Related").count() != 1
                || related_without_block(before) != related_without_block(content)
            {
                return Err(ApiError::invalid(
                    "Related changes may alter only the managed ## Related block of a source note",
                ));
            }
            let block = content
                .split("## Related")
                .nth(1)
                .unwrap_or("")
                .split("\n## ")
                .next()
                .unwrap_or("");
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
                let target = target.split('|').next().unwrap_or(target);
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
fn candidate_body(candidate: &Candidate) -> String {
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
fn input_excluded(path: &str, kind: Option<&str>) -> bool {
    path.starts_with("dreams/")
        || path.starts_with("derived/")
        || path.starts_with(".brunn/")
        || path == "private/dreamer.md"
        || path.starts_with("agent-memory/")
        || path == "Location/Places.md"
        || path.starts_with("Location/Visits/")
        || matches!(kind, Some("location-places" | "location-visits"))
}
async fn retain_inputs(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    data: &mut RunState,
    upper: i64,
) -> ApiResult<()> {
    // RLS can substantially underestimate rows. Bound the ordered page before
    // any version joins, then resolve each distinct entry only once. Without
    // these fences the planner can put LIMIT after whole-corpus nested loops.
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
        LEFT JOIN brunn.entry_versions v
          ON v.user_id=$1 AND v.entry_id=c.entry_id AND v.version=snapshot.entry_version
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
        if !row.get::<bool, _>("is_text")
            || input_excluded(&path, metadata.get("kind").and_then(Value::as_str))
        {
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
        )
        .await?;
    let output = json!({"candidates":[candidate]});
    let admission =
        json!({"location_work":{"timezone":scope["timezone"]},"location_evidence":packet});
    if let Some(issue) = crate::dreamer::prompt::location_clock_issues(&output, &admission).first()
    {
        return Err(ApiError::invalid(format!(
            "location clock citation validation failed: {issue}"
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
            json!({"attempt_id":a.attempt_id,"producer_credential_id":a.producer_credential_id,"date":a.date,"outcome":"partial","detail":detail,"started_at":a.started_at,"finished_at":at,"recovered":true,"counts":counts(&data)}),
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
    let lease = body
        .get("lease_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(2100)
        .clamp(30, 7800);
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
            if packet["fingerprint_complete"] == true
                && packet["evidence_fingerprint"] != known["fingerprint"]
                && !deferred
            {
                let mut work = known.clone();
                work.as_object_mut().expect("scope").remove("fingerprint");
                data.location_work.push(work);
                break;
            }
        }
    }
    if let Some(work) = data.location_work.first().cloned() {
        let query: crate::location::evidence::EvidenceQuery = serde_json::from_value(
            json!({"from":work["from"],"to":work["to"],"timezone":work["timezone"]}),
        )?;
        let mut internal = auth.clone();
        internal.capabilities.insert("save".into());
        crate::location::store::lock_location_user(&mut tx, user).await?;
        let packet = crate::location::evidence::evidence_in_tx(&mut tx, &internal, &query).await?;
        // Owner approval was for the exact old evidence. Both automatic
        // refresh and explicit requeue need a fresh candidate and decision.
        if packet["fingerprint_complete"] == true {
            let mut invalidated = Vec::new();
            for item in &mut data.items {
                if item.status == "approved_held"
                    && item.candidate.evidence_scope.as_ref().is_some_and(|scope| {
                        same_location_window(scope, &work)
                            && scope["fingerprint"] != packet["evidence_fingerprint"]
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
    }
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
                        if let Some(scope) = &item.candidate.evidence_scope {
                            if !data
                                .location_work
                                .iter()
                                .any(|work| same_location_window(scope, work))
                            {
                                if let Some(known) = data
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
    let decisions = load_entry(tx, user, "dreams/decisions.md").await?;
    let rows=sqlx::query("SELECT path,current_version FROM brunn.entries WHERE user_id=$1 AND deleted_at IS NULL AND (starts_with(path,'derived/entities/') OR starts_with(path,'derived/location/')) ORDER BY path LIMIT 256")
        .bind(user).fetch_all(&mut **tx).await?;
    let outputs=rows.iter().map(|r|json!({"path":r.get::<String,_>("path"),"version":r.get::<i64,_>("current_version")})).collect::<Vec<_>>();
    Ok(
        json!({"admitted":true,"attempt_id":a.attempt_id,"fence":a.fence,"state_version":version,"mode":a.mode,"frozen_generation":a.frozen_generation,"scanned_generation":data.scanned_generation,"processed_generation":data.processed_generation,"inputs":data.inputs,"outputs":outputs,"location_work":a.location_work,"location_evidence":location_evidence,"pending":pending_items,"pending_notifications":data.pending_notifications,"decisions":decisions.as_ref().map(|e|e.content.as_str()).unwrap_or(""),"decisions_version":decisions.map_or(0,|e|e.version)}),
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
        .clamp(30, 7800);
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
            if let Some((n, text)) = line.split_once(". ") {
                if let Ok(n) = n.parse::<usize>() {
                    number = number.max(n);
                    paragraphs.push((n, section.to_owned(), text.to_owned()));
                    continue;
                }
            }
            if !line.trim().is_empty() {
                if let Some((_, _, text)) = paragraphs.last_mut() {
                    text.push('\n');
                    text.push_str(line);
                }
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
            if data.items.iter().any(|i| i.id == id) || vetoed(&decisions, date, n) {
                continue;
            }
            if data.items.len() >= MAX_ITEMS {
                return Ok(());
            }
            let title = text
                .lines()
                .next()
                .unwrap_or("Legacy proposal")
                .chars()
                .take(150)
                .collect::<String>();
            let candidate=Candidate {kind:if section=="Needs your call"{"question"}else{"legacy"}.into(),title,summary:text.chars().take(1000).collect(),reason:"Retained from an earlier run; a concrete candidate is required before application.".into(),path:None,content:None,expected_version:None,sources:vec![],uncertainty:String::new(),question:if section=="Needs your call"{text}else{String::new()},revises_item_id:None,evidence_scope:None,raw_sources:vec![]};
            data.items.push(Item {
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
    json!({"schema":"dream.latest-receipt.v2","run_id":a.date,"receipt_ref":run["entry_ref"],"receipt_version":run["version"],"receipt_path":run["path"],"status":outcome,"completed_at":crate::dreamer::receipt::format_timestamp(completed),"mode":a.mode,"runner":"brunn-rust-dreamer","mode_flip":false,"probe_monitoring":null,"applied_writes":applied,"entering_veto_window_today":[],"pending_owner":owner,"pending_review_surfaces":[],"next_run_at":crate::dreamer::receipt::format_timestamp(next_run(completed))})
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
    let request_hash = digest(&body);
    if let Some(old) = &data.candidate_submission {
        if old["attempt_id"] == body["attempt_id"]
            && old["producer"] == auth.credential_id.0.to_string()
            && old["request_hash"] == request_hash
        {
            return Ok(Json(old["response"].clone()));
        }
    }
    let a = active(&data, &body, &auth, version)?;
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
    for candidate in &list {
        if let Some(id) = &candidate.revises_item_id {
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
        if candidate.evidence_scope.is_none()
            && candidate.sources.iter().any(|s| {
                !data
                    .inputs
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
            &mut candidate,
            a.frozen_generation,
            !location,
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
        validate_candidate(&candidate, &before_md)?;
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
                frozen_generation: a.frozen_generation,
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
            frozen_generation: a.frozen_generation,
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
        data.pending_notifications.push(json!({"status":"pending","event_key":format!("dreaming-review-{}",a.attempt_id),"target_kind":"review","run_entry_ref":run["entry_ref"],"run_version":run["version"],"count":ids.len()}));
    }
    let response = json!({"state_version":version+1,"run_entry_ref":run["entry_ref"],"run_version":run["version"],"accepted_candidate_ids":ids,"pending_count":data.items.iter().filter(|i|pending(i)).count()});
    data.candidate_submission = Some(
        json!({"attempt_id":a.attempt_id,"producer":auth.credential_id.0,"request_hash":request_hash,"response":response}),
    );
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
    let detail = body
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(2000)
        .collect::<String>();
    let completed = Utc::now();
    let notification = record_notifications(&mut data, &body["notification"])?;
    data.last_attempt = Some(
        json!({"attempt_id":a.attempt_id,"producer_credential_id":a.producer_credential_id,"date":a.date,"outcome":outcome,"detail":detail,"started_at":a.started_at,"finished_at":completed,"auth_persistence":body["auth_persistence"],"notification":notification,"execution_outcome":body["execution_outcome"],"model":body["model"],"codex_version":body["codex_version"],"counts":counts(&data)}),
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
        .map(|s| entry_id(&s.entry_ref))
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
    if item.candidate.expected_version.unwrap_or(0) > 0 {
        if let Some(path) = &item.candidate.path {
            if load_entry(tx, user, path).await?.is_none() {
                return Ok(false);
            }
        }
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
    shown.candidate.path = None;
    shown.before_md.clear();
    shown.reviewable = false;
    shown.status = "stale".into();
    shown
}
async fn item_stale(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    item: &Item,
) -> ApiResult<bool> {
    let user = auth.user_id.0;
    if !item.reviewable {
        return Ok(false);
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
        {
            return Ok(true);
        }
        if let Some(path) = &item.candidate.path {
            return Ok(item.candidate.expected_version
                != Some(load_entry(tx, user, path).await?.map_or(0, |e| e.version)));
        }
        return Ok(false);
    }
    for source in &item.candidate.sources {
        let current:Option<i64>=sqlx::query_scalar("SELECT current_version FROM brunn.entries WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL")
            .bind(user).bind(entry_id(&source.entry_ref)?).fetch_optional(&mut **tx).await?;
        if current != Some(source.version) {
            return Ok(true);
        }
    }
    if let Some(path) = &item.candidate.path {
        if item.candidate.expected_version
            != Some(load_entry(tx, user, path).await?.map_or(0, |e| e.version))
        {
            return Ok(true);
        }
    }
    if item.candidate.kind == "summary" {
        for prefix in item
            .candidate
            .sources
            .iter()
            .filter_map(|s| s.path.rsplit_once('/').map(|(p, _)| format!("{p}/")))
            .collect::<std::collections::BTreeSet<_>>()
        {
            let changed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM brunn.workspace_changes WHERE user_id=$1 AND generation>$2 AND starts_with(path,$3))")
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
    let legacy = data.items.iter().filter(|item| {
        pending(item)
            && (item.candidate.kind == "legacy"
                || (item.candidate.kind == "question"
                    && item.frozen_generation == 0
                    && item.candidate.reason
                        == "Retained from an earlier run; a concrete candidate is required before application."))
    }).collect::<Vec<_>>();
    let mut runs = std::collections::BTreeMap::new();
    let mut texts = std::collections::BTreeMap::new();
    for item in legacy {
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
            if matches!(section.as_str(), "Proposed" | "Needs your call") {
                if let Some((number, text)) = line.split_once(". ") {
                    if let Ok(number) = number.parse::<usize>() {
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
            }
        }
        if let Some(key) = &current {
            if let Some(Some(text)) = result.get_mut(key) {
                text.push('\n');
                text.push_str(line);
            }
        }
    }
    for text in result.values_mut().flatten() {
        *text = text.trim_end().to_owned();
    }
    result
}

#[cfg(test)]
mod legacy_review_tests {
    use super::legacy_proposal_texts;

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
    for original in data.items.iter().filter(|i| pending(i)) {
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
        let stale = !available || item_stale(tx, auth, original).await?;
        let block = if !available {
            Some("Evidence is no longer available. Cached proposal text has been withheld.")
        } else if stale {
            Some("Sources or the target changed. A new candidate must be reviewed.")
        } else if !i.reviewable {
            Some("This older proposal describes work but has no generated candidate to approve.")
        } else {
            None
        };
        let preview = if i.candidate.kind == "question" || legacy_text.is_some() {
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
        items.push(json!({"id":i.id,"kind":if i.candidate.kind=="question"{"question"}else{"proposal"},"title":title,"body_md":body,"why_md":i.candidate.reason,"uncertainty_md":i.candidate.uncertainty,"run_id":i.run_id,"run_entry_ref":i.run_entry_ref,"run_version":i.run_version,"candidate_hash":i.candidate_hash,"candidate":preview,"sources":sources,"status":if stale{"stale"}else{&i.status},"reviewable":i.reviewable&&i.candidate.kind!="question","stale":stale,"blocked_reason":block}));
    }
    // Put actionable candidates within reach before the retained legacy backlog;
    // stable sorting preserves the existing order and every decision identity.
    items.sort_by_key(|item| !(item["reviewable"] == true && item["stale"] == false));
    Ok(
        json!({"available":true,"mode":current_mode,"paused":current_mode.is_none(),"last_attempt":data.last_attempt,"last_successful_run":data.last_successful_run,"counts":counts(data),"items":items,"history":data.history,"decision_version":version}),
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
        &mut item.candidate,
        item.frozen_generation,
        !location,
    )
    .await?;
    if location {
        item.candidate.evidence_scope =
            Some(validate_location_candidate(tx, auth, &item.candidate).await?);
    }
    validate_candidate(&item.candidate, &item.before_md)?;
    let path = item.candidate.path.clone().expect("validated path");
    let metadata = if item.candidate.kind == "summary" {
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
        json!({"kind":"derived_summary","dreamer_summary":{"schema":"dream.summary.v1","compiler":"brunn-rust-v1","state":"published","candidate_id":item.id,"candidate_hash":item.candidate_hash,"run_entry_ref":item.run_entry_ref,"run_version":item.run_version,"compiled_at":item.created_at,"published_at":Utc::now(),"frozen_generation":item.frozen_generation,"sources":item.candidate.sources,"scope_prefixes":prefixes,"raw_sources":item.candidate.raw_sources,"evidence_scope":item.candidate.evidence_scope}})
    } else {
        load_entry(tx, user, &path)
            .await?
            .map(|e| e.metadata)
            .unwrap_or(json!({}))
    };
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
    if matches!(item.status.as_str(), "rejected" | "applied") {
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
    if current_mode.is_some() {
        if let Some(old) = load_entry(&mut tx, user, "dreams/latest-receipt.md").await? {
            if let Ok(mut latest) = crate::dreamer::receipt::parse_latest(&old.content) {
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
                };
                latest["pending_owner"] =
                    projection(&data, &a, &json!({}), "completed", now)["pending_owner"].clone();
                write_projection(&state, &mut tx, &auth, &latest).await?;
            }
        }
    }
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    state.workspace_features.invalidate(user).await;
    let message = match item.status.as_str() {
        "approved_held" => "Approved. Application is held until publication is explicitly enabled.",
        "applied" => "Approved and applied after validating the source versions.",
        "rejected" => "Rejected. This proposal remains in decision history.",
        "deferred" => "Deferred. The proposal remains in your inbox.",
        _ => "Correction recorded. A revised candidate is needed before approval.",
    };
    Ok(Json(
        json!({"status":"complete","data":{"saved":true,"decision":decision,"application_status":item.status,"message":message,"state_version":version}}),
    ))
}

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use chrono::{DateTime, FixedOffset, NaiveDate, SecondsFormat, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use url::Url;
use url::form_urlencoded;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{Capability, ResponseStatus},
    simple_core::{self, WorkspaceEnvelope, WriteRequest},
    usage::ProductActivityOperation,
    workspace_features,
};

pub const SUMMARY_LINE_LIMIT: usize = 12;
pub const SECTION_LIMIT: usize = 24;
pub const SECTION_ITEM_LIMIT: usize = 32;
pub const OMISSION_LIMIT: usize = 64;
pub const STORY_URL_LIMIT: usize = 8;
pub const HEADLINE_LIMIT_CHARS: usize = 500;
pub const BODY_LIMIT_CHARS: usize = 4_000;
pub const WHY_IT_MATTERS_LIMIT_CHARS: usize = 1_000;
pub const DETAIL_LIMIT_CHARS: usize = 16_000;
pub const WHAT_CHANGED_LIMIT_CHARS: usize = 1_000;
pub const URL_LIMIT_CHARS: usize = 2_048;
pub const SECTION_TOPIC_LIMIT_CHARS: usize = 80;
pub const SECTION_TITLE_LIMIT_CHARS: usize = 200;
pub const STORY_TITLE_LIMIT_CHARS: usize = 500;
pub const ENTITY_LIMIT_CHARS: usize = 120;
pub const ENTITY_LIMIT: usize = 16;
pub const OMISSION_REASON_LIMIT_CHARS: usize = 1_000;
pub const SUMMARY_LINE_LIMIT_CHARS: usize = 1_000;
pub const TIMEZONE_LIMIT_CHARS: usize = 64;
pub const GENERATED_AT_LIMIT_CHARS: usize = 64;

const TRACKING_QUERY_PARAMS: [&str; 8] = [
    "fbclid", "gclid", "mc_cid", "mc_eid", "ref", "ref_src", "cmpid", "smid",
];

pub const ITEM_KINDS: [&str; 7] = [
    "news", "metric", "health", "ops", "digest", "tracker", "schedule",
];
pub const ITEM_DELTAS: [&str; 3] = ["new", "update", "corroboration"];

static EDITION_SLUG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{1,31}$").expect("edition slug regex"));
static ITEM_ID: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{1,63}$").expect("item id regex"));
static STORY_KEY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{2,79}$").expect("story key regex"));

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BriefingPublishRequest {
    pub date: String,
    pub edition: String,
    pub timezone: Option<String>,
    pub generated_at: Option<String>,
    #[serde(default)]
    pub summary_md: Vec<String>,
    #[serde(default)]
    pub sections: Vec<BriefingSection>,
    #[serde(default)]
    pub omitted: Vec<BriefingOmission>,
    pub idempotency_key: Option<String>,
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BriefingSection {
    pub topic: String,
    pub title: String,
    pub items: Vec<BriefingItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BriefingItem {
    pub id: String,
    pub kind: String,
    pub headline_md: String,
    #[serde(default)]
    pub body_md: String,
    #[serde(default)]
    pub why_it_matters: String,
    #[serde(default)]
    pub detail_md: String,
    #[serde(default)]
    pub what_changed: String,
    #[serde(default = "default_delta")]
    pub delta: String,
    pub story: Option<BriefingStoryRef>,
    pub times: Option<BriefingTimes>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BriefingStoryRef {
    pub key: String,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub entities: Vec<String>,
    pub event_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BriefingTimes {
    pub published_at: Option<String>,
    pub event_at: Option<String>,
    pub first_seen_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BriefingOmission {
    pub story_key: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    pub reason: String,
}

fn default_delta() -> String {
    "new".to_owned()
}

pub fn validate_publish_request(request: &BriefingPublishRequest) -> ApiResult<()> {
    let round_trips = NaiveDate::parse_from_str(&request.date, "%Y-%m-%d")
        .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == request.date);
    if !round_trips {
        return Err(ApiError::invalid("date must be a YYYY-MM-DD calendar date"));
    }
    if !EDITION_SLUG.is_match(&request.edition) {
        return Err(ApiError::invalid(
            "edition must be a lowercase slug of 2 to 32 characters",
        ));
    }
    if let Some(timezone) = &request.timezone {
        require_char_limit("timezone", timezone, TIMEZONE_LIMIT_CHARS)?;
    }
    if let Some(generated_at) = &request.generated_at {
        require_char_limit("generated_at", generated_at, GENERATED_AT_LIMIT_CHARS)?;
    }
    require_collection_limit("summary_md", request.summary_md.len(), SUMMARY_LINE_LIMIT)?;
    for line in &request.summary_md {
        require_char_limit("summary_md line", line, SUMMARY_LINE_LIMIT_CHARS)?;
    }
    require_collection_limit("sections", request.sections.len(), SECTION_LIMIT)?;
    require_collection_limit("omitted", request.omitted.len(), OMISSION_LIMIT)?;
    let mut seen_ids = HashSet::new();
    for section in &request.sections {
        require_char_limit("section topic", &section.topic, SECTION_TOPIC_LIMIT_CHARS)?;
        require_char_limit("section title", &section.title, SECTION_TITLE_LIMIT_CHARS)?;
        require_collection_limit("section items", section.items.len(), SECTION_ITEM_LIMIT)?;
        for item in &section.items {
            if !ITEM_ID.is_match(&item.id) {
                return Err(ApiError::invalid(
                    "item id must be a lowercase slug of 2 to 64 characters",
                ));
            }
            if !seen_ids.insert(item.id.as_str()) {
                return Err(ApiError::invalid(format!(
                    "item id {} appears more than once in the edition",
                    item.id,
                )));
            }
            if !ITEM_KINDS.contains(&item.kind.as_str()) {
                return Err(ApiError::invalid(format!(
                    "item kind must be one of: {}",
                    ITEM_KINDS.join(", "),
                )));
            }
            if !ITEM_DELTAS.contains(&item.delta.as_str()) {
                return Err(ApiError::invalid(format!(
                    "item delta must be one of: {}",
                    ITEM_DELTAS.join(", "),
                )));
            }
            require_char_limit("headline_md", &item.headline_md, HEADLINE_LIMIT_CHARS)?;
            require_char_limit("body_md", &item.body_md, BODY_LIMIT_CHARS)?;
            require_char_limit(
                "why_it_matters",
                &item.why_it_matters,
                WHY_IT_MATTERS_LIMIT_CHARS,
            )?;
            require_char_limit("detail_md", &item.detail_md, DETAIL_LIMIT_CHARS)?;
            require_char_limit("what_changed", &item.what_changed, WHAT_CHANGED_LIMIT_CHARS)?;
            if item.kind == "news" && item.story.is_none() {
                return Err(ApiError::invalid(format!(
                    "news item {} requires a story with a key for dedupe bookkeeping",
                    item.id,
                )));
            }
            if let Some(story) = &item.story {
                require_story_key(&story.key)?;
                require_char_limit("story title", &story.title, STORY_TITLE_LIMIT_CHARS)?;
                require_collection_limit("story entities", story.entities.len(), ENTITY_LIMIT)?;
                for entity in &story.entities {
                    require_char_limit("story entity", entity, ENTITY_LIMIT_CHARS)?;
                }
                require_collection_limit("story urls", story.urls.len(), STORY_URL_LIMIT)?;
                for url in &story.urls {
                    canonicalize_url(url)?;
                }
            }
        }
    }
    for omission in &request.omitted {
        if let Some(story_key) = &omission.story_key {
            require_story_key(story_key)?;
        }
        require_char_limit(
            "omission reason",
            &omission.reason,
            OMISSION_REASON_LIMIT_CHARS,
        )?;
        require_collection_limit("omission urls", omission.urls.len(), STORY_URL_LIMIT)?;
        for url in &omission.urls {
            canonicalize_url(url)?;
        }
    }
    Ok(())
}

fn require_collection_limit(field: &'static str, length: usize, limit: usize) -> ApiResult<()> {
    if length > limit {
        return Err(ApiError::invalid(format!(
            "{field} exceeds the limit of {limit} entries",
        )));
    }
    Ok(())
}

fn require_char_limit(field: &'static str, value: &str, limit: usize) -> ApiResult<()> {
    if value.chars().count() > limit {
        return Err(ApiError::invalid(format!(
            "{field} exceeds the limit of {limit} characters",
        )));
    }
    Ok(())
}

fn require_story_key(story_key: &str) -> ApiResult<()> {
    if !STORY_KEY.is_match(story_key) {
        return Err(ApiError::invalid(
            "story key must be a lowercase slug of 3 to 80 characters",
        ));
    }
    Ok(())
}

pub fn render_edition_markdown(request: &BriefingPublishRequest) -> String {
    let generated = request
        .generated_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|instant| localize(instant, request.timezone.as_deref()));
    let mut blocks = Vec::new();
    let updated = generated
        .as_ref()
        .map_or_else(|| request.date.clone(), |stamp| stamp.local.clone());
    blocks.push(format!("Created: {}\nUpdated: {updated}", request.date));
    blocks.push(format!(
        "# {} briefing - {}",
        capitalize_edition(&request.edition),
        request.date,
    ));
    if let Some(stamp) = &generated {
        blocks.push(format!("Generated at {} {}.", stamp.local, stamp.zone));
    }
    if !request.summary_md.is_empty() {
        let bullets = request
            .summary_md
            .iter()
            .map(|line| format!("- {}", line.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(format!("## 30-second version\n\n{bullets}"));
    }
    for section in &request.sections {
        blocks.push(format!("## {}", section.title.trim()));
        for item in &section.items {
            let mut paragraph: Vec<String> = [item.headline_md.trim(), item.body_md.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect();
            let why_it_matters = item.why_it_matters.trim();
            if !why_it_matters.is_empty() {
                paragraph.push(format!("**Why this matters:** {why_it_matters}"));
            }
            if !paragraph.is_empty() {
                blocks.push(paragraph.join(" "));
            }
            let what_changed = item.what_changed.trim();
            if !what_changed.is_empty() {
                blocks.push(format!("*What changed:* {what_changed}"));
            }
            let detail = item.detail_md.trim();
            if !detail.is_empty() {
                blocks.push(format!("**Details.** {detail}"));
            }
        }
    }
    let mut rendered = blocks.join("\n\n");
    rendered.push('\n');
    rendered
}

struct LocalizedStamp {
    local: String,
    zone: String,
}

fn localize(instant: DateTime<FixedOffset>, timezone: Option<&str>) -> LocalizedStamp {
    match timezone.and_then(|name| name.parse::<chrono_tz::Tz>().ok()) {
        Some(zone) => {
            let local = instant.with_timezone(&zone);
            LocalizedStamp {
                local: local.format("%Y-%m-%d %H:%M").to_string(),
                zone: local.format("%Z").to_string(),
            }
        }
        None => LocalizedStamp {
            local: instant.format("%Y-%m-%d %H:%M").to_string(),
            zone: instant.format("%:z").to_string(),
        },
    }
}

fn capitalize_edition(edition: &str) -> String {
    let mut chars = edition.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

pub fn canonicalize_url(raw: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > URL_LIMIT_CHARS {
        return Err(ApiError::invalid(format!(
            "url exceeds the limit of {URL_LIMIT_CHARS} characters",
        )));
    }
    let parsed =
        Url::parse(trimmed).map_err(|_| ApiError::invalid("url must be an absolute URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::invalid("url scheme must be http or https"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ApiError::invalid("url must include a host"))?;
    // Url::parse already lowercases the scheme and host and drops default ports.
    let mut canonical = format!("{}://{host}", parsed.scheme());
    if let Some(port) = parsed.port() {
        write!(canonical, ":{port}").expect("write port into canonical url");
    }
    if parsed.path() != "/" {
        canonical.push_str(parsed.path());
    }
    let mut pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !is_tracking_query_param(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if !pairs.is_empty() {
        pairs.sort();
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        canonical.push('?');
        canonical.push_str(&query);
    }
    Ok(canonical)
}

fn is_tracking_query_param(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_") || TRACKING_QUERY_PARAMS.contains(&key.as_str())
}

pub fn story_url_hash(canonical_url: &str) -> String {
    hex::encode(Sha256::digest(canonical_url.as_bytes()))
}

pub fn edition_entry_path(date: &str, edition: &str) -> String {
    let year = date.get(..4).unwrap_or(date);
    format!(
        "Briefings/{year}/{} briefing - {date}.md",
        capitalize_edition(edition),
    )
}

fn edition_metadata(request: &BriefingPublishRequest) -> ApiResult<Value> {
    let mut briefing = match serde_json::to_value(request)? {
        Value::Object(map) => map,
        _ => return Err(ApiError::invalid("the briefing payload must be an object")),
    };
    briefing.remove("idempotency_key");
    briefing.remove("expected_version");
    briefing.insert("schema".to_owned(), Value::String("briefing.v1".to_owned()));
    Ok(json!({"kind": "briefing_edition", "briefing": Value::Object(briefing)}))
}

pub fn compute_edition_delta(
    previous_briefing: Option<&Value>,
    request: &BriefingPublishRequest,
) -> Value {
    let mut previous_items = BTreeMap::new();
    let previous_sections = previous_briefing
        .and_then(|briefing| briefing.get("sections"))
        .and_then(Value::as_array);
    for section in previous_sections.into_iter().flatten() {
        let items = section.get("items").and_then(Value::as_array);
        for item in items.into_iter().flatten() {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                previous_items.insert(id.to_owned(), item.clone());
            }
        }
    }
    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut current_ids = HashSet::new();
    for section in &request.sections {
        for item in &section.items {
            current_ids.insert(item.id.as_str());
            match previous_items.get(&item.id) {
                None => added.push(item.id.clone()),
                Some(previous_item) if item_changed(previous_item, item) => {
                    changed.push(item.id.clone());
                }
                Some(_) => {}
            }
        }
    }
    let removed: Vec<String> = previous_items
        .keys()
        .filter(|id| !current_ids.contains(id.as_str()))
        .cloned()
        .collect();
    added.sort();
    changed.sort();
    json!({"added": added, "changed": changed, "removed": removed})
}

fn briefing_without_delta(briefing: &Value) -> Value {
    let mut briefing = briefing.clone();
    if let Some(map) = briefing.as_object_mut() {
        map.remove("delta");
    }
    briefing
}

/// True when an existing entry must gain a new version even though the
/// rendered markdown is byte-identical: the briefing metadata (ignoring the
/// computed delta block) changed, or the current version predates structured
/// briefing metadata. Without this, ledger-facing fields that never render
/// (story refs, urls, omissions, item kinds) would be silently dropped by the
/// content-hash NoOp rule.
pub(crate) fn briefing_requires_new_version(
    previous_entry_exists: bool,
    previous_briefing: Option<&Value>,
    next_briefing: Option<&Value>,
) -> bool {
    if !previous_entry_exists {
        return false;
    }
    match (previous_briefing, next_briefing) {
        (Some(previous), Some(next)) => {
            briefing_without_delta(previous) != briefing_without_delta(next)
        }
        (None, Some(_)) => true,
        _ => false,
    }
}

fn item_changed(previous_item: &Value, item: &BriefingItem) -> bool {
    let Ok(previous) = serde_json::from_value::<BriefingItem>(previous_item.clone()) else {
        return true;
    };
    match (
        serde_json::to_string(&previous),
        serde_json::to_string(item),
    ) {
        (Ok(previous), Ok(current)) => previous != current,
        _ => true,
    }
}

pub async fn publish(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<BriefingPublishRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Save)?;
    validate_publish_request(&request)?;
    let date = NaiveDate::parse_from_str(&request.date, "%Y-%m-%d")
        .map_err(|_| ApiError::invalid("date must be a YYYY-MM-DD calendar date"))?;
    let path = edition_entry_path(&request.date, &request.edition);
    let content = render_edition_markdown(&request);
    let committed_bytes = u64::try_from(content.len()).unwrap_or(u64::MAX);
    let mut prepared = simple_core::prepare_markdown(
        &state,
        WriteRequest {
            path: path.clone(),
            content,
            media_type: "text/markdown".to_owned(),
            expected_version: request.expected_version,
            idempotency_key: request.idempotency_key.clone(),
            metadata: edition_metadata(&request)?,
        },
    )
    .await?;
    let content_sha256 = prepared.content_sha256.clone();
    let mut tx = state.begin_write(&auth).await?;
    simple_core::require_local_publish_lock(
        &mut tx,
        entry_lock_key(auth.user_id.0, &path),
        state.config.read_path_roundtrip_v1,
    )
    .await?;
    let previous_row = simple_core::fetch_locked_markdown_entry(&mut tx, auth.user_id.0, &path)
        .await?
        .filter(|row| row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_none());
    let previous_briefing = previous_row
        .as_ref()
        .and_then(|row| row.get::<Value, _>("metadata").get("briefing").cloned());
    let delta = compute_edition_delta(previous_briefing.as_ref(), &request);
    if let Some(briefing) = prepared
        .metadata
        .get_mut("briefing")
        .and_then(Value::as_object_mut)
    {
        briefing.insert("delta".to_owned(), delta.clone());
    }
    prepared.force_new_version = briefing_requires_new_version(
        previous_row.is_some(),
        previous_briefing.as_ref(),
        prepared.metadata.get("briefing"),
    );
    let result = simple_core::upsert_markdown_in_tx(
        &mut tx,
        auth.user_id.0,
        Some(auth.credential_id.0),
        prepared,
    )
    .await?;
    let skipped_invalid_urls = if result.no_op {
        0
    } else {
        apply_edition_to_ledger(
            &mut tx,
            auth.user_id.0,
            &format!("entry:{}", result.entry_id),
            date,
            &request.sections,
            &request.omitted,
        )
        .await?
    };
    let generation = match result.generation {
        Some(value) => value,
        None => simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?,
    };
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    let response_delta = if result.no_op {
        json!({"added": [], "changed": [], "removed": []})
    } else {
        delta
    };
    let mut data = json!({
        "path": path,
        "entry_ref": format!("entry:{}", result.entry_id),
        "version_ref": result.version_id.map(|id| format!("entry-version:{id}")),
        "version": result.version,
        "content_hash": format!("sha256:{content_sha256}"),
        "delta": response_delta,
    });
    if skipped_invalid_urls > 0 {
        data["skipped_invalid_urls"] = json!(skipped_invalid_urls);
    }
    let mut envelope = WorkspaceEnvelope::complete(data);
    envelope.status = if result.no_op {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if !result.no_op {
        record_product_activity(
            &state,
            &auth,
            ProductActivityOperation::BriefingPublish,
            committed_bytes,
        );
    }
    Ok(Json(envelope))
}

enum LedgerOp<'a> {
    Deliver {
        topic: &'a str,
        item: &'a BriefingItem,
        story: &'a BriefingStoryRef,
    },
    Suppress {
        story_key: String,
        urls: &'a [String],
    },
}

impl LedgerOp<'_> {
    fn story_key(&self) -> &str {
        match self {
            Self::Deliver { story, .. } => &story.key,
            Self::Suppress { story_key, .. } => story_key,
        }
    }
}

/// One deterministic, story-key-sorted operation list per edition so that
/// concurrent publishes acquire briefing_stories row locks in the same order
/// (the sort is stable, so repeated keys keep their in-edition order).
fn ordered_ledger_ops<'a>(
    sections: &'a [BriefingSection],
    omitted: &'a [BriefingOmission],
) -> Vec<LedgerOp<'a>> {
    let mut ops = Vec::new();
    for section in sections {
        for item in &section.items {
            if let Some(story) = &item.story {
                ops.push(LedgerOp::Deliver {
                    topic: &section.topic,
                    item,
                    story,
                });
            }
        }
    }
    for omission in omitted {
        let story_key = omission
            .story_key
            .clone()
            .or_else(|| synthetic_story_key(&omission.urls));
        if let Some(story_key) = story_key {
            ops.push(LedgerOp::Suppress {
                story_key,
                urls: &omission.urls,
            });
        }
    }
    ops.sort_by(|left, right| left.story_key().cmp(right.story_key()));
    ops
}

/// Deterministic story key for an omission that carries URLs but no key, so
/// its suppression history and URL hashes are still recorded and rebuilds
/// reproduce it: "url-" plus the first 16 hex of the first canonical URL hash.
pub(crate) fn synthetic_story_key(urls: &[String]) -> Option<String> {
    urls.iter()
        .find_map(|url| canonicalize_url(url).ok())
        .map(|canonical| format!("url-{}", &story_url_hash(&canonical)[..16]))
}

/// Canonicalize and hash URLs, dropping invalid ones (defense for rebuilds of
/// pre-validation data) and sorting by hash for deterministic insert order.
fn canonical_url_rows(urls: &[String]) -> (Vec<(String, String)>, usize) {
    let mut rows = Vec::new();
    let mut skipped = 0;
    for url in urls {
        match canonicalize_url(url) {
            Ok(canonical) => rows.push((story_url_hash(&canonical), canonical)),
            Err(_) => skipped += 1,
        }
    }
    rows.sort();
    rows.dedup();
    (rows, skipped)
}

pub async fn apply_edition_to_ledger(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    edition_ref: &str,
    date: NaiveDate,
    sections: &[BriefingSection],
    omitted: &[BriefingOmission],
) -> ApiResult<usize> {
    let mut skipped_invalid_urls = 0;
    for op in ordered_ledger_ops(sections, omitted) {
        match op {
            LedgerOp::Deliver { topic, item, story } => {
                let delivered = item.delta != "corroboration";
                let event_at = story
                    .event_at
                    .as_deref()
                    .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
                sqlx::query(
                    r#"
                    INSERT INTO brunn.briefing_stories (
                      user_id,story_key,title,topic,entities,event_at,
                      last_delivered_date,last_delivered_edition_ref,
                      last_delivered_headline,delivery_count
                    ) VALUES (
                      $1,$2,$3,$4,$5,$6,
                      CASE WHEN $7 THEN $8 END,
                      CASE WHEN $7 THEN $9 END,
                      CASE WHEN $7 THEN $10 END,
                      CASE WHEN $7 THEN 1 ELSE 0 END
                    )
                    ON CONFLICT (user_id,story_key) DO UPDATE SET
                      title=CASE WHEN EXCLUDED.title <> '' THEN EXCLUDED.title
                            ELSE briefing_stories.title END,
                      topic=CASE WHEN EXCLUDED.topic <> '' THEN EXCLUDED.topic
                            ELSE briefing_stories.topic END,
                      entities=CASE WHEN cardinality(EXCLUDED.entities) > 0 THEN EXCLUDED.entities
                               ELSE briefing_stories.entities END,
                      event_at=COALESCE(EXCLUDED.event_at,briefing_stories.event_at),
                      last_seen_at=clock_timestamp(),
                      last_delivered_date=CASE
                        WHEN $7 AND (briefing_stories.last_delivered_date IS NULL
                                     OR $8 >= briefing_stories.last_delivered_date)
                        THEN $8 ELSE briefing_stories.last_delivered_date END,
                      last_delivered_edition_ref=CASE
                        WHEN $7 AND (briefing_stories.last_delivered_date IS NULL
                                     OR $8 >= briefing_stories.last_delivered_date)
                        THEN $9 ELSE briefing_stories.last_delivered_edition_ref END,
                      last_delivered_headline=CASE
                        WHEN $7 AND (briefing_stories.last_delivered_date IS NULL
                                     OR $8 >= briefing_stories.last_delivered_date)
                        THEN $10 ELSE briefing_stories.last_delivered_headline END,
                      delivery_count=briefing_stories.delivery_count
                                     + CASE WHEN $7 THEN 1 ELSE 0 END
                    "#,
                )
                .bind(user_id)
                .bind(&story.key)
                .bind(&story.title)
                .bind(topic)
                .bind(&story.entities)
                .bind(event_at)
                .bind(delivered)
                .bind(date)
                .bind(edition_ref)
                .bind(&item.headline_md)
                .execute(&mut **tx)
                .await?;
                skipped_invalid_urls +=
                    insert_story_urls(tx, user_id, &story.key, &story.urls).await?;
            }
            LedgerOp::Suppress { story_key, urls } => {
                sqlx::query(
                    r#"
                    INSERT INTO brunn.briefing_stories (user_id,story_key,suppression_count)
                    VALUES ($1,$2,1)
                    ON CONFLICT (user_id,story_key) DO UPDATE SET
                      suppression_count=briefing_stories.suppression_count + 1,
                      last_seen_at=clock_timestamp()
                    "#,
                )
                .bind(user_id)
                .bind(&story_key)
                .execute(&mut **tx)
                .await?;
                skipped_invalid_urls += insert_story_urls(tx, user_id, &story_key, urls).await?;
            }
        }
    }
    Ok(skipped_invalid_urls)
}

#[derive(Clone, Debug, Deserialize)]
struct BriefingEditionReplay {
    date: String,
    #[serde(default)]
    sections: Vec<BriefingSection>,
    #[serde(default)]
    omitted: Vec<BriefingOmission>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct BriefingLedgerRebuild {
    pub replayed_versions: usize,
    pub skipped_versions: usize,
    pub skipped_invalid_urls: usize,
}

pub async fn rebuild_briefing_ledger(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<BriefingLedgerRebuild> {
    sqlx::query("DELETE FROM brunn.briefing_story_urls WHERE user_id=$1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM brunn.briefing_stories WHERE user_id=$1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    let versions = sqlx::query(
        r#"
        SELECT entry.id AS entry_id,version.metadata->'briefing' AS briefing
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
        WHERE entry.user_id=$1
          AND entry.path LIKE 'Briefings/%'
          AND entry.deleted_at IS NULL
          AND version.metadata->>'kind'='briefing_edition'
        ORDER BY version.created_at ASC,version.version ASC,entry.path ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut rebuild = BriefingLedgerRebuild::default();
    for row in versions {
        let entry_id: Uuid = row.get("entry_id");
        let replay = row
            .get::<Option<Value>, _>("briefing")
            .and_then(|briefing| serde_json::from_value::<BriefingEditionReplay>(briefing).ok());
        let date = replay
            .as_ref()
            .and_then(|replay| NaiveDate::parse_from_str(&replay.date, "%Y-%m-%d").ok());
        let (Some(replay), Some(date)) = (replay, date) else {
            rebuild.skipped_versions += 1;
            continue;
        };
        rebuild.skipped_invalid_urls += apply_edition_to_ledger(
            tx,
            user_id,
            &format!("entry:{entry_id}"),
            date,
            &replay.sections,
            &replay.omitted,
        )
        .await?;
        rebuild.replayed_versions += 1;
    }
    Ok(rebuild)
}

pub const DEDUPE_CANDIDATE_LIMIT: usize = 64;
pub const DEDUPE_NEAR_LIMIT: usize = 5;
pub const CANDIDATE_TOPIC_LIMIT_CHARS: usize = 80;

#[derive(Clone, Debug, Deserialize)]
pub struct DedupeCheckRequest {
    #[serde(default)]
    pub candidates: Vec<DedupeCandidate>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DedupeCandidate {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub event_at: Option<String>,
    pub topic: Option<String>,
    pub story_key: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExactStoryHit {
    pub story_key: String,
    pub title: String,
    pub last_delivered_date: Option<NaiveDate>,
    pub last_delivered_edition_ref: Option<String>,
    pub last_delivered_headline: Option<String>,
    pub delivery_count: i32,
    pub suppression_count: i32,
    pub matched_by: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DedupeCandidateReport {
    pub exact: Vec<ExactStoryHit>,
    pub near: Vec<Value>,
    pub verdict_hint: &'static str,
}

pub fn validate_dedupe_request(request: &DedupeCheckRequest) -> ApiResult<()> {
    if request.candidates.is_empty() || request.candidates.len() > DEDUPE_CANDIDATE_LIMIT {
        return Err(ApiError::invalid(format!(
            "dedupe-check requires between 1 and {DEDUPE_CANDIDATE_LIMIT} candidates",
        )));
    }
    for candidate in &request.candidates {
        require_collection_limit("candidate urls", candidate.urls.len(), STORY_URL_LIMIT)?;
        require_char_limit("candidate title", &candidate.title, HEADLINE_LIMIT_CHARS)?;
        require_char_limit("candidate summary", &candidate.summary, BODY_LIMIT_CHARS)?;
        if let Some(topic) = &candidate.topic {
            require_char_limit("candidate topic", topic, CANDIDATE_TOPIC_LIMIT_CHARS)?;
        }
        if let Some(story_key) = &candidate.story_key {
            require_story_key(story_key)?;
        }
        if let Some(event_at) = &candidate.event_at {
            parse_calendar_date(event_at)
                .ok_or_else(|| ApiError::invalid("event_at must be a YYYY-MM-DD calendar date"))?;
        }
    }
    Ok(())
}

fn parse_calendar_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .filter(|date| date.format("%Y-%m-%d").to_string() == value)
}

/// Verdict hints only; the agent adjudicates `near` and `possible_update`.
/// An exact URL hit on a delivered story is a duplicate; a story that was
/// seen but never delivered, or whose candidate event is newer than the last
/// delivery, is a possible update; everything else is unseen.
pub fn classify_candidate(exact: &[ExactStoryHit], event_at: Option<NaiveDate>) -> &'static str {
    let delivered_url_hit = exact
        .iter()
        .any(|hit| hit.matched_by.iter().any(|lane| lane == "url") && hit.delivery_count > 0);
    if delivered_url_hit {
        return "duplicate";
    }
    let possible_update = exact.iter().any(|hit| match hit.last_delivered_date {
        None => true,
        Some(delivered) => event_at.is_some_and(|event| event > delivered),
    });
    if possible_update {
        "possible_update"
    } else {
        "unseen"
    }
}

fn exact_hit_from_row(row: &sqlx::postgres::PgRow, matched_by: &str) -> ExactStoryHit {
    ExactStoryHit {
        story_key: row.get("story_key"),
        title: row.get("title"),
        last_delivered_date: row.get("last_delivered_date"),
        last_delivered_edition_ref: row.get("last_delivered_edition_ref"),
        last_delivered_headline: row.get("last_delivered_headline"),
        delivery_count: row.get("delivery_count"),
        suppression_count: row.get("suppression_count"),
        matched_by: vec![matched_by.to_owned()],
    }
}

pub async fn dedupe_candidate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    candidate: &DedupeCandidate,
) -> ApiResult<DedupeCandidateReport> {
    let mut url_hashes = Vec::new();
    for url in &candidate.urls {
        if let Ok(canonical) = canonicalize_url(url) {
            let hash = story_url_hash(&canonical);
            if !url_hashes.contains(&hash) {
                url_hashes.push(hash);
            }
        }
    }
    let mut exact = Vec::new();
    if !url_hashes.is_empty() {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT story.story_key,story.title,story.last_delivered_date,
                   story.last_delivered_edition_ref,story.last_delivered_headline,
                   story.delivery_count,story.suppression_count
            FROM brunn.briefing_story_urls AS url
            JOIN brunn.briefing_stories AS story
              ON story.user_id=url.user_id AND story.story_key=url.story_key
            WHERE url.user_id=$1 AND url.url_hash=ANY($2)
            ORDER BY story.story_key
            "#,
        )
        .bind(user_id)
        .bind(&url_hashes)
        .fetch_all(&mut **tx)
        .await?;
        exact.extend(rows.iter().map(|row| exact_hit_from_row(row, "url")));
    }
    if let Some(story_key) = candidate.story_key.as_deref() {
        let row = sqlx::query(
            r#"
            SELECT story_key,title,last_delivered_date,last_delivered_edition_ref,
                   last_delivered_headline,delivery_count,suppression_count
            FROM brunn.briefing_stories
            WHERE user_id=$1 AND story_key=$2
            "#,
        )
        .bind(user_id)
        .bind(story_key)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(row) = row {
            match exact.iter_mut().find(|hit| hit.story_key == story_key) {
                Some(hit) => hit.matched_by.push("story_key".to_owned()),
                None => exact.push(exact_hit_from_row(&row, "story_key")),
            }
        }
    }
    let mut near = Vec::new();
    let title = candidate.title.trim();
    if !title.is_empty() {
        let rows = sqlx::query(
            r#"
            SELECT story_key,title,last_delivered_date,last_delivered_edition_ref,
                   last_delivered_headline,delivery_count,suppression_count
            FROM brunn.briefing_stories
            WHERE user_id=$1
              AND (title <> '' OR cardinality(entities) > 0)
              AND to_tsvector('english',title || ' ' || array_to_string(entities,' '))
                  @@ plainto_tsquery('english',$2)
            ORDER BY last_seen_at DESC,story_key
            LIMIT $3
            "#,
        )
        .bind(user_id)
        .bind(title)
        .bind((DEDUPE_NEAR_LIMIT + exact.len()) as i64)
        .fetch_all(&mut **tx)
        .await?;
        near.extend(
            rows.iter()
                .filter(|row| {
                    let story_key: String = row.get("story_key");
                    !exact.iter().any(|hit| hit.story_key == story_key)
                })
                .take(DEDUPE_NEAR_LIMIT)
                .map(|row| {
                    json!({
                        "lane": "ledger_titles",
                        "story_key": row.get::<String, _>("story_key"),
                        "title": row.get::<String, _>("title"),
                        "last_delivered_date": row.get::<Option<NaiveDate>, _>("last_delivered_date"),
                        "delivery_count": row.get::<i32, _>("delivery_count"),
                    })
                }),
        );
        let lexical = sqlx::query(crate::retrieval_sql::SIMPLE_LEXICAL_CANDIDATES_SQL)
            .bind(title)
            .bind("best_match")
            .fetch_all(&mut **tx)
            .await?;
        let mut seen_paths = HashSet::new();
        for row in lexical {
            let path: String = row.get("path");
            if !path.starts_with("Briefings/") || !seen_paths.insert(path.clone()) {
                continue;
            }
            near.push(json!({
                "lane": "workspace_lexical",
                "path": path,
                "title": row.get::<String, _>("title"),
                "heading": row.get::<String, _>("heading"),
                "score": row.get::<f64, _>("score"),
            }));
            if seen_paths.len() >= DEDUPE_NEAR_LIMIT {
                break;
            }
        }
    }
    let event_at = candidate
        .event_at
        .as_deref()
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    let verdict_hint = classify_candidate(&exact, event_at);
    Ok(DedupeCandidateReport {
        exact,
        near,
        verdict_hint,
    })
}

pub async fn dedupe_check(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DedupeCheckRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    validate_dedupe_request(&request)?;
    let mut tx = state.begin_read(&auth).await?;
    let mut candidates = Vec::with_capacity(request.candidates.len());
    for candidate in &request.candidates {
        candidates.push(dedupe_candidate_in_tx(&mut tx, auth.user_id.0, candidate).await?);
    }
    let generation = simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?;
    tx.commit().await?;
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "candidates": candidates,
        "workspace_generation": generation,
    }));
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    record_serialized_product_read(&state, &auth, ProductActivityOperation::Search, &envelope);
    Ok(Json(envelope))
}

pub const BRIEFING_LIST_DEFAULT_LIMIT: usize = 14;
pub const BRIEFING_LIST_MAX_LIMIT: usize = 60;
pub const FEEDBACK_TAIL_LINES: usize = 50;
pub const NOTE_LIMIT_CHARS: usize = 1_000;
pub const TOPIC_BODY_LIMIT_CHARS: usize = 16_000;
pub const PENDING_REQUEST_LIMIT: usize = 50;
pub const REQUEST_NOTE_LIMIT_CHARS: usize = 2_000;
pub const DEFAULT_TOPIC_SECTION_ORDER: i64 = 1_000;
pub const DEFAULT_TOPIC_FRESHNESS_HOURS: i64 = 48;
pub const TOPIC_MODES: [&str; 5] = [
    "every_briefing",
    "on_material_delta",
    "scheduled",
    "paused",
    "muted",
];
pub const FEEDBACK_VERDICTS: [&str; 6] = [
    "useful",
    "not_important",
    "already_knew",
    "repeated",
    "wrong",
    "follow_closer",
];

/// Split a document into (frontmatter, body-after-closing-fence). The fence
/// must open on the first line and close within the same 4 KiB bound the
/// workspace-features parser applies; the returned body is byte-verbatim.
pub(crate) fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let bounded =
        workspace_features::bounded_prefix(content, workspace_features::FRONTMATTER_LIMIT_BYTES);
    let mut offset = 0;
    let mut frontmatter_start = None;
    for line in bounded.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r').trim();
        match frontmatter_start {
            None => {
                if trimmed != "---" {
                    return None;
                }
                frontmatter_start = Some(offset + line.len());
            }
            Some(start) => {
                if trimmed == "---" {
                    return Some((&content[start..offset], &content[offset + line.len()..]));
                }
            }
        }
        offset += line.len();
    }
    None
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BriefingTopic {
    pub slug: String,
    pub name: String,
    pub section_order: i64,
    pub mode: String,
    pub editions: Vec<String>,
    pub schedule: Option<String>,
    pub entities: Vec<String>,
    pub symbols: Vec<String>,
    pub suppress_unchanged: bool,
    pub freshness_hours: i64,
    pub body: String,
    /// True when the body exceeded `TOPIC_BODY_LIMIT_CHARS` and was cut.
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
}

fn note_parse_error(slot: &mut Option<String>, message: String) {
    if slot.is_none() {
        *slot = Some(message);
    }
}

fn cap_chars(value: &str, limit: usize) -> (String, bool) {
    if value.chars().count() <= limit {
        (value.to_owned(), false)
    } else {
        (value.chars().take(limit).collect(), true)
    }
}

pub fn parse_topic_document(slug: &str, content: &str) -> BriefingTopic {
    let mut topic = BriefingTopic {
        slug: slug.to_owned(),
        name: slug.to_owned(),
        section_order: DEFAULT_TOPIC_SECTION_ORDER,
        mode: "every_briefing".to_owned(),
        editions: vec!["morning".to_owned()],
        schedule: None,
        entities: Vec::new(),
        symbols: Vec::new(),
        suppress_unchanged: true,
        freshness_hours: DEFAULT_TOPIC_FRESHNESS_HOURS,
        body: String::new(),
        truncated: false,
        parse_error: None,
    };
    let Some((frontmatter, body)) = split_frontmatter(content) else {
        (topic.body, topic.truncated) = cap_chars(content, TOPIC_BODY_LIMIT_CHARS);
        topic.parse_error =
            Some("frontmatter is missing or not closed; raw content preserved as body".to_owned());
        return topic;
    };
    (topic.body, topic.truncated) = cap_chars(
        body.strip_prefix("\r\n")
            .or_else(|| body.strip_prefix('\n'))
            .unwrap_or(body),
        TOPIC_BODY_LIMIT_CHARS,
    );
    let mut active_list: Option<&'static str> = None;
    for raw in frontmatter.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(value) = line.trim().strip_prefix("- ")
                && let Some(value) = workspace_features::bounded_scalar(value, 80)
            {
                match active_list {
                    Some("editions") => topic.editions.push(value),
                    Some("entities") => topic.entities.push(value),
                    Some("symbols") => topic.symbols.push(value),
                    _ => {}
                }
            }
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            active_list = None;
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        active_list = match key {
            "editions" | "entities" | "symbols" if value.is_empty() => {
                if key == "editions" {
                    topic.editions.clear();
                }
                Some(match key {
                    "editions" => "editions",
                    "entities" => "entities",
                    _ => "symbols",
                })
            }
            _ => None,
        };
        match key {
            "name" => {
                if let Some(name) = workspace_features::bounded_scalar(value, 120) {
                    topic.name = name;
                }
            }
            "section_order" => {
                match workspace_features::bounded_scalar(value, 32)
                    .and_then(|order| order.parse::<i64>().ok())
                {
                    Some(order) => topic.section_order = order,
                    None => note_parse_error(
                        &mut topic.parse_error,
                        format!("section_order must be an integer, got {value:?}"),
                    ),
                }
            }
            "mode" => {
                let mode = workspace_features::bounded_scalar(value, 32)
                    .map(|mode| mode.to_lowercase())
                    .unwrap_or_default();
                if TOPIC_MODES.contains(&mode.as_str()) {
                    topic.mode = mode;
                } else {
                    note_parse_error(
                        &mut topic.parse_error,
                        format!("mode must be one of: {}", TOPIC_MODES.join(", ")),
                    );
                }
            }
            "editions" if !value.is_empty() => {
                let editions = workspace_features::inline_list(value);
                if !editions.is_empty() {
                    topic.editions = editions;
                }
            }
            "entities" if !value.is_empty() => {
                topic.entities = workspace_features::inline_list(value);
            }
            "symbols" if !value.is_empty() => {
                topic.symbols = workspace_features::inline_list(value);
            }
            "schedule" => {
                topic.schedule = workspace_features::bounded_scalar(value, 80)
                    .filter(|schedule| schedule != "null" && schedule != "~");
            }
            "suppress_unchanged" => match workspace_features::bounded_scalar(value, 8).as_deref() {
                Some("true") => topic.suppress_unchanged = true,
                Some("false") => topic.suppress_unchanged = false,
                _ => note_parse_error(
                    &mut topic.parse_error,
                    format!("suppress_unchanged must be true or false, got {value:?}"),
                ),
            },
            "freshness_hours" => {
                match workspace_features::bounded_scalar(value, 32)
                    .and_then(|hours| hours.parse::<i64>().ok())
                    .filter(|hours| *hours > 0)
                {
                    Some(hours) => topic.freshness_hours = hours,
                    None => note_parse_error(
                        &mut topic.parse_error,
                        format!("freshness_hours must be a positive integer, got {value:?}"),
                    ),
                }
            }
            _ => {}
        }
    }
    topic
}

/// Replace (or insert) one top-level scalar frontmatter key, preserving every
/// other frontmatter line in order and the body byte-for-byte. A document
/// without well-formed frontmatter gains a new block above its content.
pub fn patch_topic_frontmatter(content: &str, key: &str, value: &str) -> String {
    let Some((frontmatter, body)) = split_frontmatter(content) else {
        return format!("---\n{key}: {value}\n---\n{content}");
    };
    let mut lines = Vec::new();
    let mut replaced = false;
    for raw in frontmatter.lines() {
        let is_key_line = !raw.starts_with(' ')
            && !raw.starts_with('\t')
            && raw
                .split_once(':')
                .is_some_and(|(candidate, _)| candidate.trim() == key);
        if is_key_line && !replaced {
            lines.push(format!("{key}: {value}"));
            replaced = true;
        } else {
            lines.push(raw.to_owned());
        }
    }
    if !replaced {
        lines.push(format!("{key}: {value}"));
    }
    let mut rebuilt = String::from("---\n");
    for line in &lines {
        rebuilt.push_str(line);
        rebuilt.push('\n');
    }
    rebuilt.push_str("---\n");
    rebuilt.push_str(body);
    rebuilt
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BriefingRequestFields {
    pub status: Option<String>,
    pub date: Option<String>,
    pub item_id: Option<String>,
    pub edition_ref: Option<String>,
    pub topic: Option<String>,
    pub note: String,
}

pub fn parse_request_document(content: &str) -> BriefingRequestFields {
    let mut fields = BriefingRequestFields::default();
    let Some((frontmatter, body)) = split_frontmatter(content) else {
        return fields;
    };
    fields.note = body.trim().to_owned();
    for raw in frontmatter.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let value = workspace_features::bounded_scalar(raw_value, 256);
        match raw_key.trim() {
            "status" => fields.status = value.map(|status| status.to_lowercase()),
            "date" => fields.date = value,
            "item_id" => fields.item_id = value,
            "edition_ref" => fields.edition_ref = value,
            "topic" => fields.topic = value,
            _ => {}
        }
    }
    fields
}

pub fn request_entry_path(date: &str, item_id: &str) -> String {
    format!("Briefings/Requests/{date} - {item_id}.md")
}

pub fn render_request_document(
    date: &str,
    edition_ref: &str,
    item_id: &str,
    topic_slug: Option<&str>,
    note: Option<&str>,
) -> String {
    let mut document = String::from("---\nkind: briefing_request\nstatus: pending\n");
    let _ = writeln!(document, "date: {date}");
    let _ = writeln!(document, "edition_ref: {edition_ref}");
    let _ = writeln!(document, "item_id: {item_id}");
    if let Some(topic) = topic_slug {
        let _ = writeln!(document, "topic: {topic}");
    }
    document.push_str("---\n\n");
    document.push_str(
        note.map(str::trim)
            .filter(|note| !note.is_empty())
            .unwrap_or("Go deeper on this item."),
    );
    document.push('\n');
    document
}

pub fn feedback_log_path(now: DateTime<Utc>) -> String {
    format!("Briefings/Feedback/{}.md", now.format("%Y-%m"))
}

pub fn feedback_log_line(
    now: DateTime<Utc>,
    edition_ref: &str,
    item_id: &str,
    action: &str,
    verdict: Option<&str>,
) -> String {
    let mut line = format!(
        "- {} {edition_ref} {item_id} {action}",
        now.to_rfc3339_opts(SecondsFormat::Secs, true),
    );
    if let Some(verdict) = verdict {
        line.push(' ');
        line.push_str(verdict);
    }
    line
}

pub fn append_log_line(existing: Option<&str>, line: &str) -> String {
    match existing {
        None => format!("{line}\n"),
        Some("") => format!("{line}\n"),
        Some(content) if content.ends_with('\n') => format!("{content}{line}\n"),
        Some(content) => format!("{content}\n{line}\n"),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BriefingItemActionRequest {
    pub action: String,
    pub edition_ref: Option<String>,
    pub item_id: Option<String>,
    pub topic_slug: Option<String>,
    pub verdict: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidatedItemAction<'a> {
    Log {
        action: &'a str,
        edition_ref: &'a str,
        item_id: &'a str,
        verdict: Option<&'a str>,
    },
    Expand {
        edition_ref: &'a str,
        item_id: &'a str,
        topic_slug: Option<&'a str>,
        note: Option<&'a str>,
    },
    MuteTopic {
        topic_slug: &'a str,
    },
}

fn parse_entry_ref(reference: &str) -> Option<Uuid> {
    reference.strip_prefix("entry:")?.parse().ok()
}

fn optional_topic_slug(value: Option<&str>) -> ApiResult<Option<&str>> {
    match value {
        None => Ok(None),
        Some(slug) if ITEM_ID.is_match(slug) => Ok(Some(slug)),
        Some(_) => Err(ApiError::invalid(
            "topic_slug must be a lowercase slug of 2 to 64 characters",
        )),
    }
}

pub fn validate_item_action(
    request: &BriefingItemActionRequest,
) -> ApiResult<ValidatedItemAction<'_>> {
    let require_edition_ref = || {
        request
            .edition_ref
            .as_deref()
            .filter(|reference| parse_entry_ref(reference).is_some())
            .ok_or_else(|| ApiError::invalid("edition_ref must be an entry:<uuid> reference"))
    };
    let require_item_id = || {
        request
            .item_id
            .as_deref()
            .filter(|item_id| ITEM_ID.is_match(item_id))
            .ok_or_else(|| {
                ApiError::invalid("item_id must be a lowercase slug of 2 to 64 characters")
            })
    };
    match request.action.as_str() {
        action @ ("read" | "feedback") => {
            let edition_ref = require_edition_ref()?;
            let item_id = require_item_id()?;
            let verdict = match (action, request.verdict.as_deref()) {
                ("read", None) => None,
                ("read", Some(_)) => {
                    return Err(ApiError::invalid(
                        "verdict is only valid for the feedback action",
                    ));
                }
                (_, Some(verdict)) if FEEDBACK_VERDICTS.contains(&verdict) => Some(verdict),
                _ => {
                    return Err(ApiError::invalid(format!(
                        "feedback requires a verdict, one of: {}",
                        FEEDBACK_VERDICTS.join(", "),
                    )));
                }
            };
            Ok(ValidatedItemAction::Log {
                action,
                edition_ref,
                item_id,
                verdict,
            })
        }
        "expand" => {
            let edition_ref = require_edition_ref()?;
            let item_id = require_item_id()?;
            let topic_slug = optional_topic_slug(request.topic_slug.as_deref())?;
            let note = request
                .note
                .as_deref()
                .map(str::trim)
                .filter(|note| !note.is_empty());
            if let Some(note) = note {
                require_char_limit("note", note, NOTE_LIMIT_CHARS)?;
            }
            Ok(ValidatedItemAction::Expand {
                edition_ref,
                item_id,
                topic_slug,
                note,
            })
        }
        "mute_topic" => {
            let topic_slug = optional_topic_slug(request.topic_slug.as_deref())?
                .ok_or_else(|| ApiError::invalid("mute_topic requires a topic_slug"))?;
            Ok(ValidatedItemAction::MuteTopic { topic_slug })
        }
        _ => Err(ApiError::invalid(
            "action must be one of: read, feedback, expand, mute_topic",
        )),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BriefingListQuery {
    pub limit: Option<usize>,
    pub after_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BriefingEditionQuery {
    pub version: Option<i64>,
}

/// Edition slug derived from the canonical filename convention
/// `<Edition> briefing - <date>.md`, for interim/legacy editions whose
/// metadata carries no edition field.
fn edition_from_path(path: &str) -> Option<String> {
    let filename = path.rsplit('/').next()?;
    let prefix = filename.split(" briefing - ").next()?;
    (prefix != filename && !prefix.is_empty()).then(|| prefix.to_lowercase())
}

pub async fn list_editions_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    limit: i64,
    after_path: Option<&str>,
) -> ApiResult<(Vec<Value>, bool)> {
    let cursor = match after_path {
        None => None,
        Some(path) => {
            let date = sqlx::query_scalar::<_, Option<String>>(
                r#"
                SELECT coalesce(
                         version.metadata->'briefing'->>'date',
                         version.metadata->>'date',
                         substring(entry.path from ' - (\d{4}-\d{2}-\d{2})[.]md$')
                       )
                FROM brunn.entries AS entry
                JOIN brunn.entry_versions AS version
                  ON version.user_id=entry.user_id
                 AND version.entry_id=entry.id
                 AND version.version=entry.current_version
                WHERE entry.user_id=$1
                  AND entry.path=$2
                  AND entry.deleted_at IS NULL
                  AND version.metadata->>'kind'='briefing_edition'
                "#,
            )
            .bind(user_id)
            .bind(path)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
            match date {
                Some(date) => Some((date, path.to_owned())),
                None => {
                    return Err(ApiError::invalid(
                        "after_path must name an existing briefing edition",
                    ));
                }
            }
        }
    };
    // Interim-contract editions carry the kind marker but no briefing
    // payload; their date falls back to top-level metadata, then the path.
    let rows = sqlx::query(
        r#"
        SELECT listing.id,listing.path,listing.current_version,
               listing.briefing,listing.metadata,listing.order_date
        FROM (
          SELECT entry.id,entry.path,entry.current_version,
                 version.metadata->'briefing' AS briefing,
                 version.metadata AS metadata,
                 coalesce(
                   version.metadata->'briefing'->>'date',
                   version.metadata->>'date',
                   substring(entry.path from ' - (\d{4}-\d{2}-\d{2})[.]md$')
                 ) AS order_date
          FROM brunn.entries AS entry
          JOIN brunn.entry_versions AS version
            ON version.user_id=entry.user_id
           AND version.entry_id=entry.id
           AND version.version=entry.current_version
          WHERE entry.user_id=$1
            AND entry.deleted_at IS NULL
            AND entry.path LIKE 'Briefings/%'
            AND version.metadata->>'kind'='briefing_edition'
        ) AS listing
        WHERE listing.order_date IS NOT NULL
          AND ($2::text IS NULL OR (listing.order_date,listing.path) < ($2,$3))
        ORDER BY listing.order_date DESC,listing.path DESC
        LIMIT $4
        "#,
    )
    .bind(user_id)
    .bind(cursor.as_ref().map(|(date, _)| date.clone()))
    .bind(cursor.as_ref().map(|(_, path)| path.clone()))
    .bind(limit + 1)
    .fetch_all(&mut **tx)
    .await?;
    let truncated = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    let editions = rows
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(|row| {
            let entry_id: Uuid = row.get("id");
            let path: String = row.get("path");
            let metadata: Value = row.get("metadata");
            let order_date: String = row.get("order_date");
            let briefing = row
                .get::<Option<Value>, _>("briefing")
                .unwrap_or_else(|| json!({}));
            let date = briefing
                .get("date")
                .or_else(|| metadata.get("date"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(order_date);
            let edition = briefing
                .get("edition")
                .or_else(|| metadata.get("edition"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| edition_from_path(&path));
            let sections = briefing.get("sections").and_then(Value::as_array);
            let section_titles: Vec<Value> = sections
                .into_iter()
                .flatten()
                .filter_map(|section| section.get("title").cloned())
                .collect();
            let item_count: usize = sections
                .into_iter()
                .flatten()
                .map(|section| {
                    section
                        .get("items")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len)
                })
                .sum();
            json!({
                "date": date,
                "edition": edition,
                "path": path,
                "entry_ref": format!("entry:{entry_id}"),
                "version": row.get::<i64, _>("current_version"),
                "generated_at": briefing.get("generated_at").cloned().unwrap_or(Value::Null),
                "summary_md": briefing.get("summary_md").cloned().unwrap_or_else(|| json!([])),
                "section_titles": section_titles,
                "item_count": item_count,
            })
        })
        .collect();
    Ok((editions, truncated))
}

pub async fn list_editions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<BriefingListQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let limit = query
        .limit
        .unwrap_or(BRIEFING_LIST_DEFAULT_LIMIT)
        .clamp(1, BRIEFING_LIST_MAX_LIMIT) as i64;
    let mut tx = state.begin_read(&auth).await?;
    let (editions, truncated) =
        list_editions_in_tx(&mut tx, auth.user_id.0, limit, query.after_path.as_deref()).await?;
    let generation = simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?;
    tx.commit().await?;
    let next = truncated
        .then(|| {
            editions
                .last()
                .and_then(|edition| edition.get("path").cloned())
        })
        .flatten()
        .map(|path| json!({"after_path": path}));
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "editions": editions,
        "limit": limit,
        "truncated": truncated,
        "next": next,
        "workspace_generation": generation,
    }));
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    record_serialized_product_read(
        &state,
        &auth,
        ProductActivityOperation::BriefingList,
        &envelope,
    );
    Ok(Json(envelope))
}

pub async fn get_edition_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    date: &str,
    edition: &str,
    version: Option<i64>,
) -> ApiResult<Value> {
    let path = edition_entry_path(date, edition);
    let row = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.current_version,
               version.version,version.content,version.metadata,version.created_at
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=coalesce($3,entry.current_version)
        WHERE entry.user_id=$1
          AND lower(normalize(entry.path, NFC))=$2
          AND entry.deleted_at IS NULL
          AND version.metadata->>'kind'='briefing_edition'
        "#,
    )
    .bind(user_id)
    .bind(simple_core::portable_path_key(&path))
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("briefing_not_found", &path));
    };
    let entry_id: Uuid = row.get("id");
    let versions = sqlx::query(
        r#"
        SELECT version,created_at
        FROM brunn.entry_versions
        WHERE user_id=$1 AND entry_id=$2
        ORDER BY version
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|version_row| {
        json!({
            "version": version_row.get::<i64, _>("version"),
            "created_at": version_row.get::<DateTime<Utc>, _>("created_at"),
        })
    })
    .collect::<Vec<_>>();
    Ok(json!({
        "path": row.get::<String, _>("path"),
        "entry_ref": format!("entry:{entry_id}"),
        "version": row.get::<i64, _>("version"),
        "current_version": row.get::<i64, _>("current_version"),
        "date": date,
        "edition": edition,
        "briefing": row
            .get::<Value, _>("metadata")
            .get("briefing")
            .cloned()
            .unwrap_or(Value::Null),
        "markdown": row.get::<String, _>("content"),
        "created_at": row.get::<DateTime<Utc>, _>("created_at"),
        "versions": versions,
    }))
}

pub async fn get_edition(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((date, edition)): Path<(String, String)>,
    Query(query): Query<BriefingEditionQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    parse_calendar_date(&date)
        .ok_or_else(|| ApiError::invalid("date must be a YYYY-MM-DD calendar date"))?;
    if !EDITION_SLUG.is_match(&edition) {
        return Err(ApiError::invalid(
            "edition must be a lowercase slug of 2 to 32 characters",
        ));
    }
    if query.version.is_some_and(|version| version < 1) {
        return Err(ApiError::invalid("version must be a positive integer"));
    }
    let mut tx = state.begin_read(&auth).await?;
    let mut data =
        get_edition_in_tx(&mut tx, auth.user_id.0, &date, &edition, query.version).await?;
    let generation = simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?;
    tx.commit().await?;
    data["workspace_generation"] = json!(generation);
    let mut envelope = WorkspaceEnvelope::complete(data);
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    record_serialized_product_read(
        &state,
        &auth,
        ProductActivityOperation::BriefingRead,
        &envelope,
    );
    Ok(Json(envelope))
}

fn topic_slug_from_path(path: &str) -> Option<String> {
    let name = path
        .strip_prefix("Briefings/Topics/")?
        .strip_suffix(".md")?;
    (!name.is_empty() && !name.contains('/')).then(|| name.to_owned())
}

fn request_date_from_path(path: &str) -> Option<String> {
    let date = path.strip_prefix("Briefings/Requests/")?.get(..10)?;
    parse_calendar_date(date).map(|_| date.to_owned())
}

pub async fn topics_snapshot_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> ApiResult<Value> {
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.current_version,version.content
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.deleted_at IS NULL
          AND entry.path LIKE 'Briefings/Topics/%'
        ORDER BY entry.path
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut loaded = Vec::new();
    for row in rows {
        let path: String = row.get("path");
        let Some(slug) = topic_slug_from_path(&path) else {
            continue;
        };
        let topic = parse_topic_document(&slug, &row.get::<String, _>("content"));
        loaded.push((
            topic,
            path,
            row.get::<Uuid, _>("id"),
            row.get::<i64, _>("current_version"),
        ));
    }
    loaded.sort_by(|left, right| {
        left.0
            .section_order
            .cmp(&right.0.section_order)
            .then_with(|| left.0.slug.cmp(&right.0.slug))
    });
    let mut topics = Vec::new();
    for (topic, path, entry_id, version) in loaded {
        let mut value = serde_json::to_value(&topic)?;
        value["path"] = json!(path);
        value["entry_ref"] = json!(format!("entry:{entry_id}"));
        value["version"] = json!(version);
        topics.push(value);
    }
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,version.content
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.deleted_at IS NULL
          AND entry.path LIKE 'Briefings/Requests/%'
        ORDER BY entry.path DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut pending_requests = Vec::new();
    let mut pending_total = 0_usize;
    for row in rows {
        let path: String = row.get("path");
        let fields = parse_request_document(&row.get::<String, _>("content"));
        if fields.status.as_deref() != Some("pending") {
            continue;
        }
        pending_total += 1;
        if pending_requests.len() >= PENDING_REQUEST_LIMIT {
            continue;
        }
        let entry_id: Uuid = row.get("id");
        let date = fields
            .date
            .clone()
            .or_else(|| request_date_from_path(&path));
        let (note, _) = cap_chars(&fields.note, REQUEST_NOTE_LIMIT_CHARS);
        pending_requests.push(json!({
            "path": path,
            "entry_ref": format!("entry:{entry_id}"),
            "date": date,
            "item_id": fields.item_id,
            "edition_ref": fields.edition_ref,
            "topic": fields.topic,
            "note": note,
        }));
    }
    let pending_requests_truncated = pending_total > PENDING_REQUEST_LIMIT;
    let feedback_path = feedback_log_path(now);
    let feedback = sqlx::query_scalar::<_, String>(
        r#"
        SELECT version.content
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND entry.deleted_at IS NULL
          AND lower(normalize(entry.path, NFC))=$2
        "#,
    )
    .bind(user_id)
    .bind(simple_core::portable_path_key(&feedback_path))
    .fetch_optional(&mut **tx)
    .await?;
    let feedback_tail: Vec<String> = feedback
        .map(|content| {
            let lines: Vec<&str> = content.lines().collect();
            lines[lines.len().saturating_sub(FEEDBACK_TAIL_LINES)..]
                .iter()
                .map(|line| (*line).to_owned())
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "topics": topics,
        "pending_requests": pending_requests,
        "pending_requests_truncated": pending_requests_truncated,
        "feedback_path": feedback_path,
        "feedback_tail": feedback_tail,
    }))
}

pub async fn topics_snapshot(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    let mut tx = state.begin_read(&auth).await?;
    let mut data = topics_snapshot_in_tx(&mut tx, auth.user_id.0, Utc::now()).await?;
    let generation = simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?;
    tx.commit().await?;
    data["workspace_generation"] = json!(generation);
    let mut envelope = WorkspaceEnvelope::complete(data);
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    record_serialized_product_read(
        &state,
        &auth,
        ProductActivityOperation::BriefingTopics,
        &envelope,
    );
    Ok(Json(envelope))
}

struct LockedDocument {
    content: String,
    metadata: Value,
}

async fn fetch_locked_document(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    path: &str,
) -> ApiResult<Option<LockedDocument>> {
    let row = sqlx::query(
        r#"
        SELECT entry.deleted_at,version.content,version.metadata
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1
          AND lower(normalize(entry.path, NFC))=$2
        FOR UPDATE OF entry
        "#,
    )
    .bind(user_id)
    .bind(simple_core::portable_path_key(path))
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row
        .filter(|row| row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_none())
        .map(|row| LockedDocument {
            content: row.get("content"),
            metadata: row.get("metadata"),
        }))
}

fn entry_lock_key(user_id: Uuid, path: &str) -> String {
    format!(
        "simple-entry:{user_id}:{}",
        simple_core::portable_path_key(path)
    )
}

async fn write_briefing_document(
    state: &AppState,
    auth: &AuthContext,
    mut tx: Transaction<'_, Postgres>,
    path: String,
    content: String,
    metadata: Value,
    mut data: Value,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    let committed_bytes = u64::try_from(content.len()).unwrap_or(u64::MAX);
    let prepared = simple_core::prepare_markdown(
        state,
        WriteRequest {
            path: path.clone(),
            content,
            media_type: "text/markdown".to_owned(),
            expected_version: None,
            idempotency_key: None,
            metadata,
        },
    )
    .await?;
    let content_sha256 = prepared.content_sha256.clone();
    let result = simple_core::upsert_markdown_in_tx(
        &mut tx,
        auth.user_id.0,
        Some(auth.credential_id.0),
        prepared,
    )
    .await?;
    let generation = match result.generation {
        Some(value) => value,
        None => simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?,
    };
    tx.commit().await?;
    state.workspace_features.invalidate(auth.user_id.0).await;
    data["path"] = json!(path);
    data["entry_ref"] = json!(format!("entry:{}", result.entry_id));
    data["version_ref"] = json!(result.version_id.map(|id| format!("entry-version:{id}")));
    data["version"] = json!(result.version);
    data["content_hash"] = json!(format!("sha256:{content_sha256}"));
    let mut envelope = WorkspaceEnvelope::complete(data);
    envelope.status = if result.no_op {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if !result.no_op {
        record_product_activity(
            state,
            auth,
            ProductActivityOperation::BriefingAction,
            committed_bytes,
        );
    }
    Ok(Json(envelope))
}

fn record_serialized_product_read<T: Serialize>(
    state: &AppState,
    auth: &AuthContext,
    operation: ProductActivityOperation,
    response: &T,
) {
    match serde_json::to_vec(response) {
        Ok(bytes) => record_product_activity(
            state,
            auth,
            operation,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        ),
        Err(error) => tracing::warn!(
            ?error,
            operation = operation.as_str(),
            "briefing activity response sizing failed"
        ),
    }
}

fn record_product_activity(
    state: &AppState,
    auth: &AuthContext,
    operation: ProductActivityOperation,
    bytes: u64,
) {
    state
        .usage_tracker
        .record_product_activity(auth, operation, bytes);
}

pub async fn item_action(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<BriefingItemActionRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Save)?;
    match validate_item_action(&request)? {
        ValidatedItemAction::Log {
            action,
            edition_ref,
            item_id,
            verdict,
        } => {
            let now = Utc::now();
            let path = feedback_log_path(now);
            let line = feedback_log_line(now, edition_ref, item_id, action, verdict);
            let mut tx = state.begin_write(&auth).await?;
            simple_core::require_local_publish_lock(
                &mut tx,
                entry_lock_key(auth.user_id.0, &path),
                state.config.read_path_roundtrip_v1,
            )
            .await?;
            let existing = fetch_locked_document(&mut tx, auth.user_id.0, &path).await?;
            let content = append_log_line(
                existing.as_ref().map(|document| document.content.as_str()),
                &line,
            );
            write_briefing_document(
                &state,
                &auth,
                tx,
                path,
                content,
                json!({"kind": "briefing_feedback_log"}),
                json!({"action": action, "line": line}),
            )
            .await
        }
        ValidatedItemAction::Expand {
            edition_ref,
            item_id,
            topic_slug,
            note,
        } => {
            let edition_entry_id = parse_entry_ref(edition_ref).expect("validated edition_ref");
            let mut tx = state.begin_write(&auth).await?;
            let date = sqlx::query_scalar::<_, Option<String>>(
                r#"
                SELECT version.metadata->'briefing'->>'date'
                FROM brunn.entries AS entry
                JOIN brunn.entry_versions AS version
                  ON version.user_id=entry.user_id
                 AND version.entry_id=entry.id
                 AND version.version=entry.current_version
                WHERE entry.user_id=$1
                  AND entry.id=$2
                  AND entry.deleted_at IS NULL
                  AND version.metadata->>'kind'='briefing_edition'
                "#,
            )
            .bind(auth.user_id.0)
            .bind(edition_entry_id)
            .fetch_optional(&mut *tx)
            .await?
            .flatten()
            .filter(|date| parse_calendar_date(date).is_some())
            .ok_or_else(|| {
                ApiError::invalid("edition_ref does not resolve to a briefing edition with a date")
            })?;
            let path = request_entry_path(&date, item_id);
            simple_core::require_local_publish_lock(
                &mut tx,
                entry_lock_key(auth.user_id.0, &path),
                state.config.read_path_roundtrip_v1,
            )
            .await?;
            let existing = fetch_locked_document(&mut tx, auth.user_id.0, &path).await?;
            if let Some(document) = &existing
                && parse_request_document(&document.content).status.as_deref() == Some("pending")
            {
                return Err(ApiError::conflict(
                    "request_exists",
                    "a pending expansion request already exists for this item",
                    json!({"path": path}),
                ));
            }
            let content = render_request_document(&date, edition_ref, item_id, topic_slug, note);
            write_briefing_document(
                &state,
                &auth,
                tx,
                path,
                content,
                json!({"kind": "briefing_request"}),
                json!({
                    "action": "expand",
                    "date": date,
                    "item_id": item_id,
                    "status": "pending",
                }),
            )
            .await
        }
        ValidatedItemAction::MuteTopic { topic_slug } => {
            let path = format!("Briefings/Topics/{topic_slug}.md");
            let mut tx = state.begin_write(&auth).await?;
            simple_core::require_local_publish_lock(
                &mut tx,
                entry_lock_key(auth.user_id.0, &path),
                state.config.read_path_roundtrip_v1,
            )
            .await?;
            let Some(document) = fetch_locked_document(&mut tx, auth.user_id.0, &path).await?
            else {
                return Err(ApiError::not_found("briefing_topic_not_found", &path));
            };
            let content = patch_topic_frontmatter(&document.content, "mode", "muted");
            write_briefing_document(
                &state,
                &auth,
                tx,
                path,
                content,
                document.metadata,
                json!({"action": "mute_topic", "slug": topic_slug, "mode": "muted"}),
            )
            .await
        }
    }
}

async fn insert_story_urls(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    story_key: &str,
    urls: &[String],
) -> ApiResult<usize> {
    let (rows, skipped) = canonical_url_rows(urls);
    for (url_hash, canonical) in rows {
        sqlx::query(
            r#"
            INSERT INTO brunn.briefing_story_urls (user_id,url_hash,story_key,url)
            VALUES ($1,$2,$3,$4)
            ON CONFLICT (user_id,url_hash) DO NOTHING
            "#,
        )
        .bind(user_id)
        .bind(url_hash)
        .bind(story_key)
        .bind(&canonical)
        .execute(&mut **tx)
        .await?;
    }
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    type PublishFieldCapCase = (&'static str, Box<dyn Fn(&mut BriefingPublishRequest)>);

    fn fixture_request() -> BriefingPublishRequest {
        BriefingPublishRequest {
            date: "2026-08-01".to_owned(),
            edition: "morning".to_owned(),
            timezone: Some("America/Los_Angeles".to_owned()),
            generated_at: Some("2026-08-01T06:30:00-07:00".to_owned()),
            summary_md: vec![
                "OpenAI disclosed an evaluation-agent incident.".to_owned(),
                "AMZN opened up $32.85.".to_owned(),
            ],
            sections: vec![
                BriefingSection {
                    topic: "ai".to_owned(),
                    title: "AI".to_owned(),
                    items: vec![BriefingItem {
                        id: "openai-hf-incident".to_owned(),
                        kind: "news".to_owned(),
                        headline_md: "**[OpenAI incident disclosed](https://example.com/openai)**"
                            .to_owned(),
                        body_md: "OpenAI described the failure in a postmortem.".to_owned(),
                        why_it_matters: "Agent sandboxing is now a procurement question."
                            .to_owned(),
                        detail_md: "Fuller context with measurements and links.".to_owned(),
                        what_changed: "The postmortem added a timeline.".to_owned(),
                        delta: "update".to_owned(),
                        story: Some(BriefingStoryRef {
                            key: "openai-hf-eval-agent-incident".to_owned(),
                            urls: vec!["https://example.com/openai".to_owned()],
                            title: "OpenAI Hugging Face evaluation incident".to_owned(),
                            entities: vec!["OpenAI".to_owned(), "Hugging Face".to_owned()],
                            event_at: Some("2026-07-28".to_owned()),
                        }),
                        times: Some(BriefingTimes {
                            published_at: Some("2026-07-28T14:00:00Z".to_owned()),
                            event_at: Some("2026-07-28".to_owned()),
                            first_seen_at: Some("2026-08-01T06:12:00-07:00".to_owned()),
                        }),
                    }],
                },
                BriefingSection {
                    topic: "markets".to_owned(),
                    title: "Markets".to_owned(),
                    items: vec![BriefingItem {
                        id: "amzn-open".to_owned(),
                        kind: "metric".to_owned(),
                        headline_md: "**AMZN opened up $32.85.**".to_owned(),
                        body_md: String::new(),
                        why_it_matters: String::new(),
                        detail_md: String::new(),
                        what_changed: String::new(),
                        delta: default_delta(),
                        story: None,
                        times: None,
                    }],
                },
            ],
            omitted: vec![BriefingOmission {
                story_key: Some("kimi-k3-weights".to_owned()),
                urls: vec!["https://example.com/kimi".to_owned()],
                reason: "already delivered 2026-07-28; no material delta".to_owned(),
            }],
            idempotency_key: Some("briefing-2026-08-01-morning-1".to_owned()),
            expected_version: None,
        }
    }

    #[test]
    fn render_matches_the_expected_edition_markdown() {
        let expected = "Created: 2026-08-01\n\
            Updated: 2026-08-01 06:30\n\
            \n\
            # Morning briefing - 2026-08-01\n\
            \n\
            Generated at 2026-08-01 06:30 PDT.\n\
            \n\
            ## 30-second version\n\
            \n\
            - OpenAI disclosed an evaluation-agent incident.\n\
            - AMZN opened up $32.85.\n\
            \n\
            ## AI\n\
            \n\
            **[OpenAI incident disclosed](https://example.com/openai)** \
            OpenAI described the failure in a postmortem. \
            **Why this matters:** Agent sandboxing is now a procurement question.\n\
            \n\
            *What changed:* The postmortem added a timeline.\n\
            \n\
            **Details.** Fuller context with measurements and links.\n\
            \n\
            ## Markets\n\
            \n\
            **AMZN opened up $32.85.**\n";
        assert_eq!(render_edition_markdown(&fixture_request()), expected);
    }

    #[test]
    fn render_is_byte_identical_across_calls() {
        let request = fixture_request();
        assert_eq!(
            render_edition_markdown(&request),
            render_edition_markdown(&request),
        );
    }

    #[test]
    fn render_falls_back_to_verbatim_offset_for_unknown_timezones() {
        let mut request = fixture_request();
        request.timezone = Some("Mars/Olympus_Mons".to_owned());
        let rendered = render_edition_markdown(&request);
        assert!(rendered.contains("Generated at 2026-08-01 06:30 -07:00.\n"));
    }

    #[test]
    fn item_defaults_apply_on_deserialization() {
        let item: BriefingItem = serde_json::from_str(
            r#"{"id":"amzn-open","kind":"metric","headline_md":"**AMZN opened up $32.85.**"}"#,
        )
        .expect("minimal item deserializes");
        assert_eq!(item.delta, "new");
        assert!(item.body_md.is_empty());
        assert!(item.story.is_none());
        assert!(item.times.is_none());
    }

    #[test]
    fn validation_accepts_the_fixture_request() {
        assert!(validate_publish_request(&fixture_request()).is_ok());
    }

    #[test]
    fn validation_rejects_malformed_dates() {
        for date in ["2026-8-1", "2026-02-30", "yesterday", ""] {
            let mut request = fixture_request();
            request.date = date.to_owned();
            assert!(
                validate_publish_request(&request).is_err(),
                "date {date:?} must be rejected",
            );
        }
    }

    #[test]
    fn validation_rejects_malformed_edition_slugs() {
        for edition in ["Morning", "-morning", "m", "morning briefing"] {
            let mut request = fixture_request();
            request.edition = edition.to_owned();
            assert!(
                validate_publish_request(&request).is_err(),
                "edition {edition:?} must be rejected",
            );
        }
    }

    #[test]
    fn validation_rejects_duplicate_item_ids_across_sections() {
        let mut request = fixture_request();
        request.sections[1].items[0].id = "openai-hf-incident".to_owned();
        assert!(validate_publish_request(&request).is_err());
    }

    #[test]
    fn validation_rejects_unknown_kind_and_delta_vocabulary() {
        let mut request = fixture_request();
        request.sections[0].items[0].kind = "opinion".to_owned();
        assert!(validate_publish_request(&request).is_err());

        let mut request = fixture_request();
        request.sections[0].items[0].delta = "changed".to_owned();
        assert!(validate_publish_request(&request).is_err());
    }

    #[test]
    fn validation_rejects_oversize_fields_and_collections() {
        let mut request = fixture_request();
        request.sections[0].items[0].headline_md = "h".repeat(HEADLINE_LIMIT_CHARS + 1);
        assert!(validate_publish_request(&request).is_err());

        let mut request = fixture_request();
        request.summary_md = vec!["line".to_owned(); SUMMARY_LINE_LIMIT + 1];
        assert!(validate_publish_request(&request).is_err());

        let mut request = fixture_request();
        request.sections[0].items[0]
            .story
            .as_mut()
            .expect("story")
            .urls = vec!["https://example.com/".to_owned(); STORY_URL_LIMIT + 1];
        assert!(validate_publish_request(&request).is_err());
    }

    #[test]
    fn canonicalize_normalizes_scheme_host_ports_params_and_fragments() {
        for (raw, canonical) in [
            (
                "HTTPS://Example.COM/Path/To?utm_source=x&b=2&a=1&fbclid=z#frag",
                "https://example.com/Path/To?a=1&b=2",
            ),
            ("http://example.com:80/x", "http://example.com/x"),
            ("https://example.com:443/", "https://example.com"),
            ("https://example.com:8443/x", "https://example.com:8443/x"),
            ("https://example.com/", "https://example.com"),
            ("https://example.com", "https://example.com"),
            ("https://example.com/a/", "https://example.com/a/"),
            (
                "https://example.com/a?ref=nl&ref_src=tw&gclid=1&mc_cid=2&mc_eid=3&cmpid=4&smid=5&keep=1",
                "https://example.com/a?keep=1",
            ),
        ] {
            assert_eq!(
                canonicalize_url(raw).expect(raw).as_str(),
                canonical,
                "canonical form of {raw}",
            );
        }
    }

    #[test]
    fn canonicalize_sorts_remaining_query_pairs_deterministically() {
        assert_eq!(
            canonicalize_url("https://example.com/a?b=2&a=2&a=1").expect("sortable url"),
            "https://example.com/a?a=1&a=2&b=2",
        );
    }

    #[test]
    fn canonicalize_rejects_invalid_urls() {
        assert!(canonicalize_url("example.com/no-scheme").is_err());
        assert!(canonicalize_url("ftp://example.com/x").is_err());
        assert!(canonicalize_url("mailto:owner@example.com").is_err());
        let oversized = format!("https://example.com/{}", "a".repeat(2_048));
        assert!(canonicalize_url(&oversized).is_err());
    }

    #[test]
    fn story_url_hash_is_lowercase_hex_sha256_of_the_canonical_string() {
        assert_eq!(
            story_url_hash("https://example.com"),
            "100680ad546ce6a577f42f52df33b4cfdca756859e664b8d7de329b150d09ce9",
        );
        assert_eq!(
            story_url_hash("https://example.com/a?a=1&b=2"),
            "051029b6a13fc6686e4523427e03b3a177e6970f9bfe03b026a9a023819b902a",
        );
    }

    #[test]
    fn edition_entry_path_follows_the_canonical_convention() {
        assert_eq!(
            edition_entry_path("2026-08-01", "morning"),
            "Briefings/2026/Morning briefing - 2026-08-01.md",
        );
        assert_eq!(
            edition_entry_path("2027-01-02", "health-update"),
            "Briefings/2027/Health-update briefing - 2027-01-02.md",
        );
    }

    #[test]
    fn edition_metadata_echoes_the_request_without_transport_fields() {
        let metadata = edition_metadata(&fixture_request()).expect("metadata renders");
        assert_eq!(
            metadata.get("kind").and_then(Value::as_str),
            Some("briefing_edition"),
        );
        let briefing = metadata.get("briefing").expect("briefing payload");
        assert_eq!(
            briefing.get("schema").and_then(Value::as_str),
            Some("briefing.v1"),
        );
        assert_eq!(
            briefing.get("date").and_then(Value::as_str),
            Some("2026-08-01"),
        );
        assert_eq!(
            briefing
                .get("sections")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2),
        );
        assert_eq!(
            briefing
                .get("omitted")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
        );
        assert!(briefing.get("idempotency_key").is_none());
        assert!(briefing.get("expected_version").is_none());
        assert!(briefing.get("delta").is_none(), "publish injects the delta");
    }

    #[test]
    fn delta_reports_everything_added_without_a_previous_version() {
        let delta = compute_edition_delta(None, &fixture_request());
        assert_eq!(
            delta,
            json!({
                "added": ["amzn-open", "openai-hf-incident"],
                "changed": [],
                "removed": []
            }),
        );
    }

    #[test]
    fn delta_diffs_item_ids_and_content_across_the_whole_edition() {
        let previous_metadata = edition_metadata(&fixture_request()).expect("metadata renders");
        let previous = previous_metadata.get("briefing").expect("briefing payload");
        let mut request = fixture_request();
        request.sections[0].items[0].body_md = "The postmortem gained a root cause.".to_owned();
        request.sections[1].items[0].id = "amzn-close".to_owned();
        let delta = compute_edition_delta(Some(previous), &request);
        assert_eq!(
            delta,
            json!({
                "added": ["amzn-close"],
                "changed": ["openai-hf-incident"],
                "removed": ["amzn-open"]
            }),
        );
    }

    #[test]
    fn delta_ignores_section_regrouping_of_unchanged_items() {
        let previous_metadata = edition_metadata(&fixture_request()).expect("metadata renders");
        let previous = previous_metadata.get("briefing").expect("briefing payload");
        let mut request = fixture_request();
        let moved = request.sections[1].items.remove(0);
        request.sections[0].items.push(moved);
        request.sections.remove(1);
        let delta = compute_edition_delta(Some(previous), &request);
        assert_eq!(delta, json!({"added": [], "changed": [], "removed": []}));
    }

    #[test]
    fn field_caps_do_not_subsume_the_rendered_size_cap() {
        let mut request = fixture_request();
        request.sections = (0..SECTION_LIMIT)
            .map(|section_index| BriefingSection {
                topic: format!("topic-{section_index}"),
                title: format!("Section {section_index}"),
                items: (0..SECTION_ITEM_LIMIT)
                    .map(|item_index| BriefingItem {
                        id: format!("item-{section_index}-{item_index}"),
                        kind: "digest".to_owned(),
                        headline_md: "**Headline.**".to_owned(),
                        body_md: String::new(),
                        why_it_matters: String::new(),
                        detail_md: "d".repeat(DETAIL_LIMIT_CHARS),
                        what_changed: String::new(),
                        delta: default_delta(),
                        story: None,
                        times: None,
                    })
                    .collect(),
            })
            .collect();
        assert!(validate_publish_request(&request).is_ok());
        assert!(
            render_edition_markdown(&request).len() > crate::simple_core::MAX_WRITE_BYTES,
            "a validation-passing edition can exceed the 4 MiB write cap, \
             so the prepare-stage entry_too_large guard remains load-bearing",
        );
    }

    fn exact_hit(
        matched_by: &[&str],
        delivery_count: i32,
        last_delivered_date: Option<&str>,
    ) -> ExactStoryHit {
        ExactStoryHit {
            story_key: "story-alpha".to_owned(),
            title: "Alpha story".to_owned(),
            last_delivered_date: last_delivered_date
                .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date")),
            last_delivered_edition_ref: last_delivered_date.map(|_| "entry:test".to_owned()),
            last_delivered_headline: last_delivered_date.map(|_| "**Alpha.**".to_owned()),
            delivery_count,
            suppression_count: 0,
            matched_by: matched_by.iter().map(|lane| (*lane).to_owned()).collect(),
        }
    }

    fn date(value: &str) -> Option<NaiveDate> {
        Some(NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date"))
    }

    #[test]
    fn classify_marks_delivered_url_hits_as_duplicates() {
        let exact = [exact_hit(&["url"], 1, Some("2026-07-28"))];
        assert_eq!(classify_candidate(&exact, None), "duplicate");
        assert_eq!(
            classify_candidate(&exact, date("2026-07-30")),
            "duplicate",
            "the URL identity dominates a newer candidate event date",
        );
        assert_eq!(
            classify_candidate(
                &[exact_hit(&["url", "story_key"], 3, Some("2026-07-28"))],
                None
            ),
            "duplicate",
        );
    }

    #[test]
    fn classify_marks_newer_events_on_seen_stories_as_possible_updates() {
        let story_key_hit = [exact_hit(&["story_key"], 1, Some("2026-07-28"))];
        assert_eq!(
            classify_candidate(&story_key_hit, date("2026-07-30")),
            "possible_update",
        );
        assert_eq!(
            classify_candidate(&story_key_hit, date("2026-07-28")),
            "unseen",
            "an event no newer than the last delivery is not an update hint",
        );
        assert_eq!(classify_candidate(&story_key_hit, None), "unseen");
    }

    #[test]
    fn classify_marks_seen_but_never_delivered_stories_as_possible_updates() {
        assert_eq!(
            classify_candidate(&[exact_hit(&["story_key"], 0, None)], None),
            "possible_update",
        );
        assert_eq!(
            classify_candidate(&[exact_hit(&["url"], 0, None)], None),
            "possible_update",
            "a URL hit on an undelivered story is an update hint, not a duplicate",
        );
    }

    #[test]
    fn classify_marks_unmatched_candidates_as_unseen() {
        assert_eq!(classify_candidate(&[], None), "unseen");
        assert_eq!(classify_candidate(&[], date("2026-08-01")), "unseen");
    }

    fn dedupe_fixture() -> DedupeCheckRequest {
        DedupeCheckRequest {
            candidates: vec![DedupeCandidate {
                urls: vec!["https://example.com/alpha".to_owned()],
                title: "Alpha story".to_owned(),
                summary: "Alpha resurfaced.".to_owned(),
                event_at: Some("2026-07-30".to_owned()),
                topic: Some("ai".to_owned()),
                story_key: Some("story-alpha".to_owned()),
            }],
        }
    }

    #[test]
    fn dedupe_validation_accepts_the_fixture_and_rejects_bad_shapes() {
        assert!(validate_dedupe_request(&dedupe_fixture()).is_ok());

        let empty = DedupeCheckRequest { candidates: vec![] };
        assert!(validate_dedupe_request(&empty).is_err());

        let mut oversize = dedupe_fixture();
        oversize.candidates = vec![oversize.candidates[0].clone(); DEDUPE_CANDIDATE_LIMIT + 1];
        assert!(validate_dedupe_request(&oversize).is_err());

        let mut too_many_urls = dedupe_fixture();
        too_many_urls.candidates[0].urls =
            vec!["https://example.com/".to_owned(); STORY_URL_LIMIT + 1];
        assert!(validate_dedupe_request(&too_many_urls).is_err());

        let mut bad_story_key = dedupe_fixture();
        bad_story_key.candidates[0].story_key = Some("Bad Key".to_owned());
        assert!(validate_dedupe_request(&bad_story_key).is_err());

        let mut bad_event = dedupe_fixture();
        bad_event.candidates[0].event_at = Some("2026-7-30".to_owned());
        assert!(validate_dedupe_request(&bad_event).is_err());

        let mut long_title = dedupe_fixture();
        long_title.candidates[0].title = "t".repeat(HEADLINE_LIMIT_CHARS + 1);
        assert!(validate_dedupe_request(&long_title).is_err());
    }

    #[test]
    fn dedupe_candidate_defaults_apply_on_deserialization() {
        let candidate: DedupeCandidate =
            serde_json::from_str(r#"{"title":"Alpha"}"#).expect("minimal candidate deserializes");
        assert!(candidate.urls.is_empty());
        assert!(candidate.summary.is_empty());
        assert!(candidate.event_at.is_none());
        assert!(candidate.story_key.is_none());
    }

    #[test]
    fn validation_rejects_malformed_story_keys() {
        let mut request = fixture_request();
        request.sections[0].items[0]
            .story
            .as_mut()
            .expect("story")
            .key = "AI".to_owned();
        assert!(validate_publish_request(&request).is_err());

        let mut request = fixture_request();
        request.omitted[0].story_key = Some("-bad".to_owned());
        assert!(validate_publish_request(&request).is_err());
    }

    #[test]
    fn validation_requires_a_story_for_news_items() {
        let mut request = fixture_request();
        request.sections[0].items[0].story = None;
        assert!(
            validate_publish_request(&request).is_err(),
            "news items without a story lose all bookkeeping and must be rejected",
        );

        let mut request = fixture_request();
        request.sections[0].items[0].kind = "digest".to_owned();
        request.sections[0].items[0].story = None;
        assert!(
            validate_publish_request(&request).is_ok(),
            "non-news items remain storyless",
        );
    }

    #[test]
    fn validation_requires_canonicalizable_story_and_omission_urls() {
        for url in [
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://example.com/x",
            "example.com/no-scheme",
        ] {
            let mut request = fixture_request();
            request.sections[0].items[0]
                .story
                .as_mut()
                .expect("story")
                .urls = vec![url.to_owned()];
            assert!(
                validate_publish_request(&request).is_err(),
                "story url {url:?} must be rejected",
            );

            let mut request = fixture_request();
            request.omitted[0].urls = vec![url.to_owned()];
            assert!(
                validate_publish_request(&request).is_err(),
                "omission url {url:?} must be rejected",
            );
        }
    }

    #[test]
    fn validation_enforces_publish_field_caps() {
        let cases: Vec<PublishFieldCapCase> = vec![
            (
                "section topic",
                Box::new(|request| {
                    request.sections[0].topic = "t".repeat(SECTION_TOPIC_LIMIT_CHARS + 1);
                }),
            ),
            (
                "section title",
                Box::new(|request| {
                    request.sections[0].title = "t".repeat(SECTION_TITLE_LIMIT_CHARS + 1);
                }),
            ),
            (
                "story title",
                Box::new(|request| {
                    request.sections[0].items[0]
                        .story
                        .as_mut()
                        .expect("story")
                        .title = "t".repeat(STORY_TITLE_LIMIT_CHARS + 1);
                }),
            ),
            (
                "entity length",
                Box::new(|request| {
                    request.sections[0].items[0]
                        .story
                        .as_mut()
                        .expect("story")
                        .entities = vec!["e".repeat(ENTITY_LIMIT_CHARS + 1)];
                }),
            ),
            (
                "entity count",
                Box::new(|request| {
                    request.sections[0].items[0]
                        .story
                        .as_mut()
                        .expect("story")
                        .entities = vec!["Entity".to_owned(); ENTITY_LIMIT + 1];
                }),
            ),
            (
                "omission reason",
                Box::new(|request| {
                    request.omitted[0].reason = "r".repeat(OMISSION_REASON_LIMIT_CHARS + 1);
                }),
            ),
            (
                "summary line",
                Box::new(|request| {
                    request.summary_md = vec!["s".repeat(SUMMARY_LINE_LIMIT_CHARS + 1)];
                }),
            ),
            (
                "timezone",
                Box::new(|request| {
                    request.timezone = Some("z".repeat(TIMEZONE_LIMIT_CHARS + 1));
                }),
            ),
            (
                "generated_at",
                Box::new(|request| {
                    request.generated_at = Some("g".repeat(GENERATED_AT_LIMIT_CHARS + 1));
                }),
            ),
        ];
        for (field, mutate) in cases {
            let mut request = fixture_request();
            mutate(&mut request);
            assert!(
                validate_publish_request(&request).is_err(),
                "oversize {field} must be rejected",
            );
        }
    }

    #[test]
    fn ledger_ops_apply_in_story_key_order_with_urls_by_hash() {
        let sections = vec![BriefingSection {
            topic: "ai".to_owned(),
            title: "AI".to_owned(),
            items: vec![
                BriefingItem {
                    id: "zeta-item".to_owned(),
                    kind: "news".to_owned(),
                    headline_md: "**Zeta.**".to_owned(),
                    body_md: String::new(),
                    why_it_matters: String::new(),
                    detail_md: String::new(),
                    what_changed: String::new(),
                    delta: default_delta(),
                    story: Some(BriefingStoryRef {
                        key: "story-zeta".to_owned(),
                        urls: vec![],
                        title: String::new(),
                        entities: vec![],
                        event_at: None,
                    }),
                    times: None,
                },
                BriefingItem {
                    id: "alpha-item".to_owned(),
                    kind: "news".to_owned(),
                    headline_md: "**Alpha.**".to_owned(),
                    body_md: String::new(),
                    why_it_matters: String::new(),
                    detail_md: String::new(),
                    what_changed: String::new(),
                    delta: default_delta(),
                    story: Some(BriefingStoryRef {
                        key: "story-alpha".to_owned(),
                        urls: vec![],
                        title: String::new(),
                        entities: vec![],
                        event_at: None,
                    }),
                    times: None,
                },
            ],
        }];
        let omitted = vec![
            BriefingOmission {
                story_key: Some("story-mango".to_owned()),
                urls: vec![],
                reason: "seen".to_owned(),
            },
            BriefingOmission {
                story_key: None,
                urls: vec!["https://example.com/keyless".to_owned()],
                reason: "keyless".to_owned(),
            },
        ];
        let keys: Vec<String> = ordered_ledger_ops(&sections, &omitted)
            .iter()
            .map(|op| op.story_key().to_owned())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "ops must apply in sorted story-key order");
        assert_eq!(keys.len(), 4, "the keyless omission still produces an op");

        let (rows, skipped) = canonical_url_rows(&[
            "https://example.com/zzz".to_owned(),
            "https://example.com/aaa".to_owned(),
            "not a url".to_owned(),
            "https://example.com/zzz?utm_source=x".to_owned(),
        ]);
        assert_eq!(skipped, 1);
        assert_eq!(rows.len(), 2, "canonical duplicates collapse");
        assert!(rows[0].0 < rows[1].0, "url inserts are ordered by hash");
    }

    #[test]
    fn keyless_omissions_derive_a_synthetic_url_key() {
        let key = synthetic_story_key(&[
            "not a url".to_owned(),
            "HTTPS://Example.COM/keyless?utm_source=x".to_owned(),
        ])
        .expect("synthetic key derives from the first canonical url");
        let canonical = canonicalize_url("https://example.com/keyless").expect("canonical");
        assert_eq!(key, format!("url-{}", &story_url_hash(&canonical)[..16]));
        assert!(
            STORY_KEY.is_match(&key),
            "synthetic keys fit the story-key regex"
        );
        assert!(synthetic_story_key(&["not a url".to_owned()]).is_none());
        assert!(synthetic_story_key(&[]).is_none());
    }

    #[test]
    fn briefing_requires_new_version_ignores_only_the_delta_block() {
        let previous_metadata = edition_metadata(&fixture_request()).expect("metadata renders");
        let previous = previous_metadata.get("briefing").expect("briefing payload");
        let mut previous_with_delta = previous.clone();
        previous_with_delta["delta"] =
            json!({"added": ["openai-hf-incident"], "changed": [], "removed": []});

        let next_metadata = edition_metadata(&fixture_request()).expect("metadata renders");
        let mut next = next_metadata
            .get("briefing")
            .expect("briefing payload")
            .clone();
        next["delta"] = json!({"added": [], "changed": [], "removed": []});
        assert!(
            !briefing_requires_new_version(true, Some(&previous_with_delta), Some(&next)),
            "a differing delta block alone is not a metadata change",
        );

        let mut changed_request = fixture_request();
        changed_request.sections[0].items[0]
            .story
            .as_mut()
            .expect("story")
            .urls
            .push("https://example.com/openai-postmortem".to_owned());
        let changed_metadata = edition_metadata(&changed_request).expect("metadata renders");
        let changed = changed_metadata.get("briefing").expect("briefing payload");
        assert!(
            briefing_requires_new_version(true, Some(&previous_with_delta), Some(changed)),
            "a story url change is a metadata change even when the markdown is identical",
        );

        assert!(
            briefing_requires_new_version(true, None, Some(&next)),
            "an existing entry without briefing metadata must be upgraded",
        );
        assert!(
            !briefing_requires_new_version(false, None, Some(&next)),
            "a fresh entry needs no forced version",
        );
    }

    const TOPIC_DOCUMENT: &str = "---\n\
        kind: briefing_topic\n\
        name: Stock watchlist\n\
        section_order: 70\n\
        mode: on_material_delta\n\
        editions: [morning, health-update]\n\
        schedule: 10:00 America/Los_Angeles\n\
        entities:\n  - Alphabet\n  - Microsoft\n\
        symbols: [GOOGL, MSFT]\n\
        suppress_unchanged: false\n\
        freshness_hours: 24\n\
        ---\n\
        \n\
        Absolute dollar changes, not percentages.\n";

    #[test]
    fn topic_parse_reads_every_configured_field() {
        let topic = parse_topic_document("stocks", TOPIC_DOCUMENT);
        assert_eq!(topic.slug, "stocks");
        assert_eq!(topic.name, "Stock watchlist");
        assert_eq!(topic.section_order, 70);
        assert_eq!(topic.mode, "on_material_delta");
        assert_eq!(topic.editions, ["morning", "health-update"]);
        assert_eq!(topic.schedule.as_deref(), Some("10:00 America/Los_Angeles"));
        assert_eq!(topic.entities, ["Alphabet", "Microsoft"]);
        assert_eq!(topic.symbols, ["GOOGL", "MSFT"]);
        assert!(!topic.suppress_unchanged);
        assert_eq!(topic.freshness_hours, 24);
        assert_eq!(topic.body, "Absolute dollar changes, not percentages.\n");
        assert!(topic.parse_error.is_none());
    }

    #[test]
    fn topic_parse_applies_defaults_for_missing_fields() {
        let topic = parse_topic_document("ai", "---\nkind: briefing_topic\n---\nWatch labs.\n");
        assert_eq!(topic.name, "ai", "name defaults to the slug");
        assert_eq!(topic.section_order, DEFAULT_TOPIC_SECTION_ORDER);
        assert_eq!(topic.mode, "every_briefing");
        assert_eq!(topic.editions, ["morning"]);
        assert!(topic.schedule.is_none());
        assert!(topic.entities.is_empty());
        assert!(topic.symbols.is_empty());
        assert!(topic.suppress_unchanged);
        assert_eq!(topic.freshness_hours, DEFAULT_TOPIC_FRESHNESS_HOURS);
        assert_eq!(topic.body, "Watch labs.\n");
        assert!(topic.parse_error.is_none());
    }

    #[test]
    fn topic_parse_preserves_raw_body_on_malformed_frontmatter() {
        for content in [
            "---\nkind: briefing_topic\nno closing fence\n",
            "no frontmatter at all\n",
            "",
        ] {
            let topic = parse_topic_document("stocks", content);
            assert!(
                topic.parse_error.is_some(),
                "content {content:?} must error"
            );
            assert_eq!(topic.body, content, "raw body preserved for {content:?}");
            assert_eq!(topic.mode, "every_briefing");
            assert_eq!(topic.section_order, DEFAULT_TOPIC_SECTION_ORDER);
        }
    }

    #[test]
    fn topic_parse_flags_unknown_mode_and_bad_numbers_but_keeps_parsing() {
        let topic = parse_topic_document(
            "stocks",
            "---\nmode: sometimes\nsection_order: 70\n---\nBody.\n",
        );
        assert_eq!(
            topic.mode, "every_briefing",
            "unknown mode keeps the default"
        );
        assert!(
            topic
                .parse_error
                .as_deref()
                .is_some_and(|error| error.contains("mode must be one of")),
        );
        assert_eq!(topic.section_order, 70, "later fields still parse");

        let topic = parse_topic_document("stocks", "---\nsection_order: soon\n---\nBody.\n");
        assert!(
            topic
                .parse_error
                .as_deref()
                .is_some_and(|error| error.contains("section_order")),
        );
        assert_eq!(topic.section_order, DEFAULT_TOPIC_SECTION_ORDER);
    }

    #[test]
    fn topic_parse_caps_oversized_bodies_with_a_truncated_flag() {
        let small = parse_topic_document("ai", "---\nkind: briefing_topic\n---\nShort body.\n");
        assert!(!small.truncated);
        assert_eq!(small.body, "Short body.\n");

        let oversized_body = "b".repeat(TOPIC_BODY_LIMIT_CHARS + 100);
        let oversized = parse_topic_document(
            "ai",
            &format!("---\nkind: briefing_topic\n---\n{oversized_body}"),
        );
        assert!(oversized.truncated);
        assert_eq!(oversized.body.chars().count(), TOPIC_BODY_LIMIT_CHARS);

        let malformed = parse_topic_document("ai", &oversized_body);
        assert!(
            malformed.truncated,
            "the malformed raw-body path is capped too"
        );
        assert_eq!(malformed.body.chars().count(), TOPIC_BODY_LIMIT_CHARS);
        assert!(malformed.parse_error.is_some());
    }

    #[test]
    fn patch_preserves_body_bytes_and_key_order_when_replacing_mode() {
        let patched = patch_topic_frontmatter(TOPIC_DOCUMENT, "mode", "muted");
        let expected = TOPIC_DOCUMENT.replace("mode: on_material_delta", "mode: muted");
        assert_eq!(patched, expected);
        let (_, original_body) = split_frontmatter(TOPIC_DOCUMENT).expect("fixture splits");
        let (_, patched_body) = split_frontmatter(&patched).expect("patched splits");
        assert_eq!(patched_body.as_bytes(), original_body.as_bytes());
    }

    #[test]
    fn patch_inserts_a_missing_key_before_the_closing_fence() {
        let patched = patch_topic_frontmatter(
            "---\nkind: briefing_topic\n---\nBody stays.\n",
            "mode",
            "muted",
        );
        assert_eq!(
            patched,
            "---\nkind: briefing_topic\nmode: muted\n---\nBody stays.\n",
        );
    }

    #[test]
    fn patch_wraps_documents_without_frontmatter() {
        let patched = patch_topic_frontmatter("Just prose.\n", "mode", "muted");
        assert_eq!(patched, "---\nmode: muted\n---\nJust prose.\n");
    }

    #[test]
    fn split_frontmatter_requires_a_closed_leading_fence() {
        assert!(split_frontmatter("no fence").is_none());
        assert!(split_frontmatter("---\nunclosed: true\n").is_none());
        let (frontmatter, body) =
            split_frontmatter("---\nkind: x\n---\nBody.").expect("closed fence splits");
        assert_eq!(frontmatter, "kind: x\n");
        assert_eq!(body, "Body.");
    }

    fn action_request(value: Value) -> BriefingItemActionRequest {
        serde_json::from_value(value).expect("action request deserializes")
    }

    const EDITION_REF: &str = "entry:018f4c1e-0000-7000-8000-000000000001";

    #[test]
    fn item_action_validation_rejects_unknown_actions_and_missing_fields() {
        assert!(
            validate_item_action(&action_request(json!({"action": "promote"}))).is_err(),
            "unknown actions are rejected",
        );
        assert!(
            validate_item_action(&action_request(json!({"action": "read"}))).is_err(),
            "read requires edition_ref and item_id",
        );
        assert!(
            validate_item_action(&action_request(
                json!({"action": "expand", "edition_ref": EDITION_REF})
            ))
            .is_err(),
            "expand requires item_id",
        );
        assert!(
            validate_item_action(&action_request(
                json!({"action": "expand", "item_id": "alpha-item"})
            ))
            .is_err(),
            "expand requires edition_ref",
        );
        assert!(
            validate_item_action(&action_request(
                json!({"action": "expand", "edition_ref": "not-a-ref", "item_id": "alpha-item"})
            ))
            .is_err(),
            "edition_ref must be an entry:<uuid> reference",
        );
        assert!(
            validate_item_action(&action_request(json!({"action": "mute_topic"}))).is_err(),
            "mute_topic requires topic_slug",
        );
        assert!(
            validate_item_action(&action_request(
                json!({"action": "mute_topic", "topic_slug": "Bad Slug"})
            ))
            .is_err(),
        );
    }

    #[test]
    fn item_action_validation_enforces_the_verdict_vocabulary() {
        let useful = action_request(json!({
            "action": "feedback",
            "edition_ref": EDITION_REF,
            "item_id": "alpha-item",
            "verdict": "useful",
        }));
        assert!(matches!(
            validate_item_action(&useful),
            Ok(ValidatedItemAction::Log {
                verdict: Some("useful"),
                ..
            }),
        ));
        let unknown_verdict = action_request(json!({
            "action": "feedback",
            "edition_ref": EDITION_REF,
            "item_id": "alpha-item",
            "verdict": "meh",
        }));
        assert!(
            validate_item_action(&unknown_verdict).is_err(),
            "unknown verdicts are rejected",
        );
        assert!(
            validate_item_action(&action_request(json!({
                "action": "feedback",
                "edition_ref": EDITION_REF,
                "item_id": "alpha-item",
            })))
            .is_err(),
            "feedback requires a verdict",
        );
        assert!(
            validate_item_action(&action_request(json!({
                "action": "read",
                "edition_ref": EDITION_REF,
                "item_id": "alpha-item",
                "verdict": "useful",
            })))
            .is_err(),
            "read rejects a verdict",
        );
        assert!(matches!(
            validate_item_action(&action_request(json!({
                "action": "read",
                "edition_ref": EDITION_REF,
                "item_id": "alpha-item",
            }))),
            Ok(ValidatedItemAction::Log {
                action: "read",
                verdict: None,
                ..
            }),
        ));
    }

    #[test]
    fn feedback_lines_append_deterministically() {
        let now = DateTime::parse_from_rfc3339("2026-08-01T17:03:09Z")
            .expect("test timestamp")
            .with_timezone(&Utc);
        let line = feedback_log_line(now, EDITION_REF, "alpha-item", "feedback", Some("useful"));
        assert_eq!(
            line,
            format!("- 2026-08-01T17:03:09Z {EDITION_REF} alpha-item feedback useful"),
        );
        let read_line = feedback_log_line(now, EDITION_REF, "alpha-item", "read", None);
        assert_eq!(
            read_line,
            format!("- 2026-08-01T17:03:09Z {EDITION_REF} alpha-item read"),
        );
        assert_eq!(feedback_log_path(now), "Briefings/Feedback/2026-08.md");

        assert_eq!(append_log_line(None, &read_line), format!("{read_line}\n"));
        assert_eq!(
            append_log_line(Some("- earlier\n"), &read_line),
            format!("- earlier\n{read_line}\n"),
        );
        assert_eq!(
            append_log_line(Some("- earlier"), &read_line),
            format!("- earlier\n{read_line}\n"),
            "a missing trailing newline is repaired before appending",
        );
    }

    #[test]
    fn request_documents_render_and_parse_round_trip() {
        assert_eq!(
            request_entry_path("2026-08-01", "openai-hf-incident"),
            "Briefings/Requests/2026-08-01 - openai-hf-incident.md",
        );
        let document = render_request_document(
            "2026-08-01",
            EDITION_REF,
            "openai-hf-incident",
            Some("ai"),
            Some("Compare with the Anthropic incident."),
        );
        assert_eq!(
            document,
            format!(
                "---\nkind: briefing_request\nstatus: pending\ndate: 2026-08-01\n\
                 edition_ref: {EDITION_REF}\nitem_id: openai-hf-incident\ntopic: ai\n---\n\n\
                 Compare with the Anthropic incident.\n",
            ),
        );
        let fields = parse_request_document(&document);
        assert_eq!(fields.status.as_deref(), Some("pending"));
        assert_eq!(fields.date.as_deref(), Some("2026-08-01"));
        assert_eq!(fields.item_id.as_deref(), Some("openai-hf-incident"));
        assert_eq!(fields.edition_ref.as_deref(), Some(EDITION_REF));
        assert_eq!(fields.topic.as_deref(), Some("ai"));
        assert_eq!(fields.note, "Compare with the Anthropic incident.");

        let minimal =
            render_request_document("2026-08-01", EDITION_REF, "openai-hf-incident", None, None);
        assert!(minimal.ends_with("---\n\nGo deeper on this item.\n"));
        assert!(!minimal.contains("topic:"));
        let fields = parse_request_document(&minimal);
        assert!(fields.topic.is_none());
        assert_eq!(fields.note, "Go deeper on this item.");
    }
}

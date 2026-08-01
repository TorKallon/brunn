use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use axum::{Extension, Json, extract::State};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
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
    require_collection_limit("summary_md", request.summary_md.len(), SUMMARY_LINE_LIMIT)?;
    require_collection_limit("sections", request.sections.len(), SECTION_LIMIT)?;
    require_collection_limit("omitted", request.omitted.len(), OMISSION_LIMIT)?;
    let mut seen_ids = HashSet::new();
    for section in &request.sections {
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
            if let Some(story) = &item.story {
                require_story_key(&story.key)?;
                require_collection_limit("story urls", story.urls.len(), STORY_URL_LIMIT)?;
            }
        }
    }
    for omission in &request.omitted {
        if let Some(story_key) = &omission.story_key {
            require_story_key(story_key)?;
        }
        require_collection_limit("omission urls", omission.urls.len(), STORY_URL_LIMIT)?;
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

fn item_changed(previous_item: &Value, item: &BriefingItem) -> bool {
    let Ok(previous) = serde_json::from_value::<BriefingItem>(previous_item.clone()) else {
        return true;
    };
    match (serde_json::to_string(&previous), serde_json::to_string(item)) {
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
    let mut prepared = simple_core::prepare_markdown(
        &state,
        WriteRequest {
            path: path.clone(),
            content: render_edition_markdown(&request),
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
        format!(
            "simple-entry:{}:{}",
            auth.user_id.0,
            simple_core::portable_path_key(&path)
        ),
        state.config.read_path_roundtrip_v1,
    )
    .await?;
    let previous_briefing = simple_core::fetch_locked_markdown_entry(&mut tx, auth.user_id.0, &path)
        .await?
        .filter(|row| row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_none())
        .and_then(|row| row.get::<Value, _>("metadata").get("briefing").cloned());
    let delta = compute_edition_delta(previous_briefing.as_ref(), &request);
    if let Some(briefing) = prepared
        .metadata
        .get_mut("briefing")
        .and_then(Value::as_object_mut)
    {
        briefing.insert("delta".to_owned(), delta.clone());
    }
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
    let mut data = json!({
        "path": path,
        "entry_ref": format!("entry:{}", result.entry_id),
        "version_ref": result.version_id.map(|id| format!("entry-version:{id}")),
        "version": result.version,
        "content_hash": format!("sha256:{content_sha256}"),
        "delta": delta,
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
    Ok(Json(envelope))
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
    for section in sections {
        for item in &section.items {
            let Some(story) = &item.story else { continue };
            let delivered = item.delta != "corroboration";
            let event_at = story
                .event_at
                .as_deref()
                .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
            sqlx::query(
                r#"
                INSERT INTO straylight.briefing_stories (
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
                  last_delivered_date=CASE WHEN $7 THEN $8
                                      ELSE briefing_stories.last_delivered_date END,
                  last_delivered_edition_ref=CASE WHEN $7 THEN $9
                                             ELSE briefing_stories.last_delivered_edition_ref END,
                  last_delivered_headline=CASE WHEN $7 THEN $10
                                          ELSE briefing_stories.last_delivered_headline END,
                  delivery_count=briefing_stories.delivery_count
                                 + CASE WHEN $7 THEN 1 ELSE 0 END
                "#,
            )
            .bind(user_id)
            .bind(&story.key)
            .bind(&story.title)
            .bind(&section.topic)
            .bind(&story.entities)
            .bind(event_at)
            .bind(delivered)
            .bind(date)
            .bind(edition_ref)
            .bind(&item.headline_md)
            .execute(&mut **tx)
            .await?;
            skipped_invalid_urls += insert_story_urls(tx, user_id, &story.key, &story.urls).await?;
        }
    }
    for omission in omitted {
        let Some(story_key) = &omission.story_key else {
            continue;
        };
        sqlx::query(
            r#"
            INSERT INTO straylight.briefing_stories (user_id,story_key,suppression_count)
            VALUES ($1,$2,1)
            ON CONFLICT (user_id,story_key) DO UPDATE SET
              suppression_count=briefing_stories.suppression_count + 1,
              last_seen_at=clock_timestamp()
            "#,
        )
        .bind(user_id)
        .bind(story_key)
        .execute(&mut **tx)
        .await?;
        skipped_invalid_urls += insert_story_urls(tx, user_id, story_key, &omission.urls).await?;
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
    sqlx::query("DELETE FROM straylight.briefing_story_urls WHERE user_id=$1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM straylight.briefing_stories WHERE user_id=$1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    let versions = sqlx::query(
        r#"
        SELECT entry.id AS entry_id,version.metadata->'briefing' AS briefing
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
        WHERE entry.user_id=$1
          AND entry.path LIKE 'Briefings/%'
          AND entry.deleted_at IS NULL
          AND version.metadata->>'kind'='briefing_edition'
        ORDER BY version.metadata->'briefing'->>'date' ASC,entry.path ASC,version.version ASC
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
            parse_calendar_date(event_at)?;
        }
    }
    Ok(())
}

fn parse_calendar_date(value: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .filter(|date| date.format("%Y-%m-%d").to_string() == value)
        .ok_or_else(|| ApiError::invalid("event_at must be a YYYY-MM-DD calendar date"))
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
            FROM straylight.briefing_story_urls AS url
            JOIN straylight.briefing_stories AS story
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
            FROM straylight.briefing_stories
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
            FROM straylight.briefing_stories
            WHERE user_id=$1
              AND title <> ''
              AND to_tsvector('english',title) @@ plainto_tsquery('english',$2)
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
    Ok(Json(envelope))
}

async fn insert_story_urls(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    story_key: &str,
    urls: &[String],
) -> ApiResult<usize> {
    let mut skipped = 0;
    for url in urls {
        let Ok(canonical) = canonicalize_url(url) else {
            skipped += 1;
            continue;
        };
        sqlx::query(
            r#"
            INSERT INTO straylight.briefing_story_urls (user_id,url_hash,story_key,url)
            VALUES ($1,$2,$3,$4)
            ON CONFLICT (user_id,url_hash) DO NOTHING
            "#,
        )
        .bind(user_id)
        .bind(story_url_hash(&canonical))
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
                        headline_md:
                            "**[OpenAI incident disclosed](https://example.com/openai)**".to_owned(),
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
        request.sections[0].items[0].story.as_mut().expect("story").urls =
            vec!["https://example.com/".to_owned(); STORY_URL_LIMIT + 1];
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
                        kind: "news".to_owned(),
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
            classify_candidate(&[exact_hit(&["url", "story_key"], 3, Some("2026-07-28"))], None),
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
        oversize.candidates =
            vec![oversize.candidates[0].clone(); DEDUPE_CANDIDATE_LIMIT + 1];
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
        let candidate: DedupeCandidate = serde_json::from_str(r#"{"title":"Alpha"}"#)
            .expect("minimal candidate deserializes");
        assert!(candidate.urls.is_empty());
        assert!(candidate.summary.is_empty());
        assert!(candidate.event_at.is_none());
        assert!(candidate.story_key.is_none());
    }

    #[test]
    fn validation_rejects_malformed_story_keys() {
        let mut request = fixture_request();
        request.sections[0].items[0].story.as_mut().expect("story").key = "AI".to_owned();
        assert!(validate_publish_request(&request).is_err());

        let mut request = fixture_request();
        request.omitted[0].story_key = Some("-bad".to_owned());
        assert!(validate_publish_request(&request).is_err());
    }
}

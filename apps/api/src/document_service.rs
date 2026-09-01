use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use url::Url;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{Capability, ResponseStatus},
    simple_core::{self, WorkspaceEnvelope, WriteRequest},
    usage::ProductActivityOperation,
};

const TITLE_LIMIT_CHARS: usize = 240;
const SUMMARY_LIMIT_CHARS: usize = 1_000;
const SOURCE_LIMIT: usize = 32;
const SOURCE_LABEL_LIMIT_CHARS: usize = 240;
const SOURCE_URL_LIMIT_CHARS: usize = 2_048;

static DOCUMENT_SLUG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^[a-z0-9](?:[a-z0-9-]{0,78}[a-z0-9])$").expect("human document slug regex")
});

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSource {
    pub label: String,
    pub entry_ref: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPublishRequest {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub body_md: String,
    #[serde(default)]
    pub sources: Vec<DocumentSource>,
    pub expected_version: Option<i64>,
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DocumentVersionQuery {
    pub version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DocumentMetadata {
    schema: String,
    slug: String,
    title: String,
    summary: String,
    sources: Vec<DocumentSource>,
}

pub fn validate_publish_request(request: &DocumentPublishRequest) -> ApiResult<()> {
    validate_slug(&request.slug)?;
    validate_single_line("title", &request.title, TITLE_LIMIT_CHARS)?;
    validate_optional_single_line("summary", &request.summary, SUMMARY_LIMIT_CHARS)?;
    if request.body_md.trim().is_empty() {
        return Err(ApiError::invalid("body_md must not be empty"));
    }
    if request.sources.len() > SOURCE_LIMIT {
        return Err(ApiError::invalid(format!(
            "sources exceeds the limit of {SOURCE_LIMIT} entries",
        )));
    }

    let mut identities = HashSet::new();
    for source in &request.sources {
        validate_single_line("source label", &source.label, SOURCE_LABEL_LIMIT_CHARS)?;
        let identity = normalized_source_identity(source)?;
        if !identities.insert(identity) {
            return Err(ApiError::invalid(
                "sources must not contain duplicate entry refs or URLs",
            ));
        }
    }
    Ok(())
}

fn validate_slug(slug: &str) -> ApiResult<()> {
    if !DOCUMENT_SLUG.is_match(slug) {
        return Err(ApiError::invalid(
            "slug must contain 2 to 80 lowercase letters, numbers, or hyphens, and must begin and end with a letter or number",
        ));
    }
    Ok(())
}

fn validate_optional_single_line(field: &'static str, value: &str, limit: usize) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Ok(());
    }
    validate_single_line(field, value, limit)
}

fn validate_single_line(field: &'static str, value: &str, limit: usize) -> ApiResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::invalid(format!("{field} must not be empty")));
    }
    if trimmed.chars().count() > limit {
        return Err(ApiError::invalid(format!(
            "{field} exceeds the limit of {limit} characters",
        )));
    }
    if trimmed.chars().any(|character| character.is_control()) {
        return Err(ApiError::invalid(format!(
            "{field} must be a single printable line",
        )));
    }
    Ok(())
}

fn normalized_source_identity(source: &DocumentSource) -> ApiResult<String> {
    match (source.entry_ref.as_deref(), source.url.as_deref()) {
        (Some(entry_ref), None) => {
            let entry_id = parse_entry_ref(entry_ref)?;
            Ok(format!("entry:{entry_id}"))
        }
        (None, Some(url)) => Ok(normalize_source_url(url)?),
        _ => Err(ApiError::invalid(
            "each source must contain exactly one of entry_ref or url",
        )),
    }
}

fn parse_entry_ref(reference: &str) -> ApiResult<Uuid> {
    let raw = reference
        .trim()
        .strip_prefix("entry:")
        .ok_or_else(|| ApiError::invalid("source entry_ref must use the entry:<uuid> format"))?;
    Uuid::parse_str(raw)
        .map_err(|_| ApiError::invalid("source entry_ref must use the entry:<uuid> format"))
}

fn normalize_source_url(raw: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > SOURCE_URL_LIMIT_CHARS {
        return Err(ApiError::invalid(format!(
            "source url exceeds the limit of {SOURCE_URL_LIMIT_CHARS} characters",
        )));
    }
    let parsed = Url::parse(trimmed)
        .map_err(|_| ApiError::invalid("source url must be an absolute HTTP(S) URL"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ApiError::invalid(
            "source url must be an absolute HTTP(S) URL without credentials",
        ));
    }
    Ok(parsed.into())
}

fn normalized_sources(sources: &[DocumentSource]) -> ApiResult<Vec<DocumentSource>> {
    sources
        .iter()
        .map(
            |source| match (source.entry_ref.as_deref(), source.url.as_deref()) {
                (Some(entry_ref), None) => Ok(DocumentSource {
                    label: source.label.trim().to_owned(),
                    entry_ref: Some(format!("entry:{}", parse_entry_ref(entry_ref)?)),
                    url: None,
                }),
                (None, Some(url)) => Ok(DocumentSource {
                    label: source.label.trim().to_owned(),
                    entry_ref: None,
                    url: Some(normalize_source_url(url)?),
                }),
                _ => Err(ApiError::invalid(
                    "each source must contain exactly one of entry_ref or url",
                )),
            },
        )
        .collect()
}

pub fn document_entry_path(slug: &str) -> String {
    format!("Documents/{slug}.md")
}

pub fn render_document_markdown(title: &str, body_md: &str) -> String {
    format!("# {}\n\n{}\n", title.trim(), body_md.trim())
}

pub fn document_body(title: &str, markdown: &str) -> ApiResult<String> {
    let prefix = format!("# {}\n\n", title.trim());
    let body = markdown
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix('\n'))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::invalid("stored human document is not canonical Markdown"))?;
    if render_document_markdown(title, body) != markdown {
        return Err(ApiError::invalid(
            "stored human document is not canonical Markdown",
        ));
    }
    Ok(body.to_owned())
}

pub fn document_url(public_url: &str, slug: &str, version: Option<i64>) -> String {
    let stable = format!("{}/documents/{slug}", public_url.trim_end_matches('/'),);
    match version {
        Some(version) => format!("{stable}?version={version}"),
        None => stable,
    }
}

fn document_metadata(request: &DocumentPublishRequest) -> ApiResult<Value> {
    Ok(json!({
        "kind": "human_document",
        "document": DocumentMetadata {
            schema: "document.v1".to_owned(),
            slug: request.slug.clone(),
            title: request.title.trim().to_owned(),
            summary: request.summary.trim().to_owned(),
            sources: normalized_sources(&request.sources)?,
        }
    }))
}

fn document_requires_new_version(
    previous_entry_exists: bool,
    previous_document: Option<&Value>,
    next_document: Option<&Value>,
) -> bool {
    previous_entry_exists && previous_document != next_document
}

pub async fn publish(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DocumentPublishRequest>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Save)?;
    validate_publish_request(&request)?;

    let path = document_entry_path(&request.slug);
    let content = render_document_markdown(&request.title, &request.body_md);
    let committed_bytes = u64::try_from(content.len()).unwrap_or(u64::MAX);
    let mut prepared = simple_core::prepare_markdown(
        &state,
        WriteRequest {
            path: path.clone(),
            content,
            media_type: "text/markdown".to_owned(),
            expected_version: request.expected_version,
            idempotency_key: request.idempotency_key.clone(),
            metadata: document_metadata(&request)?,
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
    validate_source_entries_in_tx(&mut tx, auth.user_id.0, &request.sources).await?;
    let previous_row =
        simple_core::fetch_locked_markdown_entry(&mut tx, auth.user_id.0, &path).await?;
    let previous_document = previous_row.as_ref().and_then(|row| {
        let metadata = row.get::<Value, _>("metadata");
        (metadata.get("kind").and_then(Value::as_str) == Some("human_document"))
            .then(|| metadata.get("document").cloned())
            .flatten()
    });
    prepared.force_new_version = document_requires_new_version(
        previous_row.is_some(),
        previous_document.as_ref(),
        prepared.metadata.get("document"),
    );
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

    let stable_url = document_url(&state.config.public_url, &request.slug, None);
    let version_url = document_url(
        &state.config.public_url,
        &request.slug,
        Some(result.version),
    );
    let mut envelope = WorkspaceEnvelope::complete(json!({
        "slug": request.slug,
        "title": request.title.trim(),
        "summary": request.summary.trim(),
        "sources": normalized_sources(&request.sources)?,
        "path": path,
        "entry_ref": format!("entry:{}", result.entry_id),
        "version_ref": result.version_id.map(|id| format!("entry-version:{id}")),
        "version": result.version,
        "content_hash": format!("sha256:{content_sha256}"),
        "url": stable_url,
        "version_url": version_url,
    }));
    envelope.status = if result.no_op {
        ResponseStatus::NoOp
    } else {
        ResponseStatus::Committed
    };
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if !result.no_op {
        state.usage_tracker.record_product_activity(
            &auth,
            ProductActivityOperation::HUMAN_DOCUMENT_PUBLISH,
            committed_bytes,
        );
    }
    Ok(Json(envelope))
}

fn entry_lock_key(user_id: Uuid, path: &str) -> String {
    format!(
        "simple-entry:{user_id}:{}",
        simple_core::portable_path_key(path),
    )
}

pub async fn validate_source_entries_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    sources: &[DocumentSource],
) -> ApiResult<()> {
    let requested = sources
        .iter()
        .filter_map(|source| source.entry_ref.as_deref())
        .map(parse_entry_ref)
        .collect::<ApiResult<HashSet<_>>>()?;
    if requested.is_empty() {
        return Ok(());
    }
    let ids = requested.iter().copied().collect::<Vec<_>>();
    let resolved = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM brunn.entries
        WHERE user_id=$1 AND id=ANY($2) AND deleted_at IS NULL
        FOR SHARE
        "#,
    )
    .bind(user_id)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    if resolved != requested {
        return Err(ApiError::invalid(
            "every source entry_ref must identify a current entry owned by the authenticated user",
        ));
    }
    Ok(())
}

pub async fn get_document_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    public_url: &str,
    slug: &str,
    requested_version: Option<i64>,
) -> ApiResult<Value> {
    let path = document_entry_path(slug);
    let version_rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.current_version,
               version.version,version.created_at
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
        WHERE entry.user_id=$1
          AND lower(normalize(entry.path, NFC))=$2
          AND entry.deleted_at IS NULL
          AND entry.kind='markdown'
          AND version.content IS NOT NULL
          AND version.metadata->>'kind'='human_document'
          AND version.metadata->'document'->>'schema'='document.v1'
          AND version.metadata->'document'->>'slug'=$3
        ORDER BY version.version
        "#,
    )
    .bind(user_id)
    .bind(simple_core::portable_path_key(&path))
    .bind(slug)
    .fetch_all(&mut **tx)
    .await?;
    let Some(first_version) = version_rows.first() else {
        return Err(ApiError::not_found("document_not_found", slug));
    };
    let current_version = first_version.get::<i64, _>("current_version");
    if !version_rows
        .iter()
        .any(|row| row.get::<i64, _>("version") == current_version)
    {
        return Err(ApiError::not_found("document_not_found", slug));
    }
    let selected_version = requested_version.unwrap_or(current_version);
    if !version_rows
        .iter()
        .any(|row| row.get::<i64, _>("version") == selected_version)
    {
        return Err(ApiError::not_found("document_not_found", slug));
    }
    let entry_id = first_version.get::<Uuid, _>("id");
    let entry_path = first_version.get::<String, _>("path");
    let published_at = first_version.get::<DateTime<Utc>, _>("created_at");
    let selected = sqlx::query(
        r#"
        SELECT id,content,metadata,created_at
        FROM brunn.entry_versions
        WHERE user_id=$1 AND entry_id=$2 AND version=$3
          AND content IS NOT NULL
          AND metadata->>'kind'='human_document'
          AND metadata->'document'->>'schema'='document.v1'
          AND metadata->'document'->>'slug'=$4
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .bind(selected_version)
    .bind(slug)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("document_not_found", slug))?;
    let metadata = selected.get::<Value, _>("metadata");
    let document: DocumentMetadata = serde_json::from_value(
        metadata
            .get("document")
            .cloned()
            .ok_or_else(|| ApiError::not_found("document_not_found", slug))?,
    )
    .map_err(|_| ApiError::not_found("document_not_found", slug))?;
    validate_stored_document(slug, &document)?;
    let markdown = selected.get::<String, _>("content");
    let body_md = document_body(&document.title, &markdown)
        .map_err(|_| ApiError::not_found("document_not_found", slug))?;

    let updated_at = selected.get::<DateTime<Utc>, _>("created_at");
    let stable_url = document_url(public_url, slug, None);
    let versions = version_rows
        .into_iter()
        .map(|row| {
            let version = row.get::<i64, _>("version");
            json!({
                "version": version,
                "created_at": row.get::<DateTime<Utc>, _>("created_at"),
                "version_url": document_url(public_url, slug, Some(version)),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "slug": document.slug,
        "title": document.title,
        "summary": document.summary,
        "sources": document.sources,
        "body_md": body_md,
        "markdown": markdown,
        "path": entry_path,
        "entry_ref": format!("entry:{entry_id}"),
        "version_ref": format!("entry-version:{}", selected.get::<Uuid, _>("id")),
        "version": selected_version,
        "current_version": current_version,
        "published_at": published_at,
        "updated_at": updated_at,
        "versions": versions,
        "url": stable_url,
        "version_url": document_url(public_url, slug, Some(selected_version)),
    }))
}

fn validate_stored_document(slug: &str, document: &DocumentMetadata) -> ApiResult<()> {
    if document.schema != "document.v1" || document.slug != slug {
        return Err(ApiError::not_found("document_not_found", slug));
    }
    validate_slug(&document.slug).map_err(|_| ApiError::not_found("document_not_found", slug))?;
    validate_single_line("title", &document.title, TITLE_LIMIT_CHARS)
        .map_err(|_| ApiError::not_found("document_not_found", slug))?;
    validate_optional_single_line("summary", &document.summary, SUMMARY_LIMIT_CHARS)
        .map_err(|_| ApiError::not_found("document_not_found", slug))?;
    if document.sources.len() > SOURCE_LIMIT {
        return Err(ApiError::not_found("document_not_found", slug));
    }
    for source in &document.sources {
        validate_single_line("source label", &source.label, SOURCE_LABEL_LIMIT_CHARS)
            .map_err(|_| ApiError::not_found("document_not_found", slug))?;
        normalized_source_identity(source)
            .map_err(|_| ApiError::not_found("document_not_found", slug))?;
    }
    Ok(())
}

pub async fn get_document(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(slug): Path<String>,
    Query(query): Query<DocumentVersionQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<Value>>> {
    auth.require(Capability::Read)?;
    validate_slug(&slug)?;
    if query.version.is_some_and(|version| version < 1) {
        return Err(ApiError::invalid("version must be a positive integer"));
    }
    let mut tx = state.begin_read(&auth).await?;
    let mut data = get_document_in_tx(
        &mut tx,
        auth.user_id.0,
        &state.config.public_url,
        &slug,
        query.version,
    )
    .await?;
    let generation = simple_core::max_generation_in_tx(&mut tx, auth.user_id.0).await?;
    tx.commit().await?;
    data["workspace_generation"] = json!(generation);
    let mut envelope = WorkspaceEnvelope::complete(data);
    envelope.corpus_revision = Some(format!("generation:{generation}"));
    if let Ok(bytes) = serde_json::to_vec(&envelope) {
        state.usage_tracker.record_product_activity(
            &auth,
            ProductActivityOperation::HUMAN_DOCUMENT_READ,
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        );
    }
    Ok(Json(envelope))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> DocumentPublishRequest {
        DocumentPublishRequest {
            slug: "switzerland-vacation-plan".to_owned(),
            title: "Switzerland vacation plan".to_owned(),
            summary: "A current, human-readable trip plan.".to_owned(),
            body_md: "## Itinerary\n\nTake the train to Zermatt.".to_owned(),
            sources: vec![
                DocumentSource {
                    label: "Rail timetable".to_owned(),
                    entry_ref: None,
                    url: Some("https://example.com/trains".to_owned()),
                },
                DocumentSource {
                    label: "Saved constraints".to_owned(),
                    entry_ref: Some("entry:018f4c1e-0000-7000-8000-000000000001".to_owned()),
                    url: None,
                },
            ],
            expected_version: Some(2),
            idempotency_key: Some("show-trip-plan".to_owned()),
        }
    }

    #[test]
    fn validates_a_curated_document_request() {
        validate_publish_request(&fixture()).expect("fixture validates");
    }

    #[test]
    fn rejects_unsafe_slugs_and_ambiguous_sources() {
        for slug in ["Trip Plan", "-trip", "trip-", "trip/plan", "", "x"] {
            let mut request = fixture();
            request.slug = slug.to_owned();
            assert!(
                validate_publish_request(&request).is_err(),
                "accepted {slug:?}"
            );
        }

        let mut request = fixture();
        request.slug = "x".repeat(81);
        assert!(validate_publish_request(&request).is_err());

        let mut request = fixture();
        request.sources[0].entry_ref =
            Some("entry:018f4c1e-0000-7000-8000-000000000002".to_owned());
        assert!(validate_publish_request(&request).is_err());
        request.sources[0].url = None;
        request.sources[0].entry_ref = None;
        assert!(validate_publish_request(&request).is_err());
    }

    #[test]
    fn rejects_non_http_source_urls_and_credentials() {
        for url in [
            "javascript:alert(1)",
            "mailto:owner@example.com",
            "https://user:secret@example.com/private",
            "not a url",
        ] {
            let mut request = fixture();
            request.sources[0].url = Some(url.to_owned());
            assert!(
                validate_publish_request(&request).is_err(),
                "accepted {url:?}"
            );
        }
    }

    #[test]
    fn renders_canonical_markdown_and_deep_links() {
        let markdown = render_document_markdown("  Trip plan  ", "\n## Day one\n\nArrive.\n");
        assert_eq!(markdown, "# Trip plan\n\n## Day one\n\nArrive.\n",);
        assert_eq!(
            document_body("Trip plan", &markdown).expect("canonical body extracts"),
            "## Day one\n\nArrive.",
        );
        assert_eq!(document_entry_path("trip-plan"), "Documents/trip-plan.md",);
        assert_eq!(
            document_url("https://brunn.example/", "trip-plan", None),
            "https://brunn.example/documents/trip-plan",
        );
        assert_eq!(
            document_url("https://brunn.example", "trip-plan", Some(3)),
            "https://brunn.example/documents/trip-plan?version=3",
        );
    }

    #[test]
    fn rejects_noncanonical_stored_markdown() {
        for markdown in [
            "## Trip plan\n\nBody.\n",
            "# Wrong title\n\nBody.\n",
            "# Trip plan\n\n",
            "# Trip plan\n\n Body. \n",
        ] {
            assert!(document_body("Trip plan", markdown).is_err());
        }
    }

    #[test]
    fn summary_is_optional_at_the_json_boundary() {
        let value = json!({
            "slug": "trip-plan",
            "title": "Trip plan",
            "body_md": "Body."
        });
        let request: DocumentPublishRequest =
            serde_json::from_value(value).expect("summary defaults to empty");
        assert_eq!(request.summary, "");
        validate_publish_request(&request).expect("an empty summary is valid");
    }

    #[test]
    fn metadata_is_versioned_without_transport_fields() {
        let request = fixture();
        let metadata = document_metadata(&request).expect("metadata renders");
        assert_eq!(metadata["kind"], "human_document");
        assert_eq!(metadata["document"]["schema"], "document.v1");
        assert_eq!(metadata["document"]["slug"], request.slug);
        assert_eq!(metadata["document"]["title"], request.title);
        assert_eq!(metadata["document"]["summary"], request.summary);
        assert!(metadata["document"].get("expected_version").is_none());
        assert!(metadata["document"].get("idempotency_key").is_none());
    }

    #[test]
    fn metadata_changes_force_history_but_first_publication_does_not() {
        let current = document_metadata(&fixture()).expect("metadata renders");
        let current = current.get("document").expect("document metadata");
        assert!(!document_requires_new_version(false, None, Some(current)));
        assert!(!document_requires_new_version(
            true,
            Some(current),
            Some(current)
        ));

        let mut changed = fixture();
        changed.summary = "A corrected summary.".to_owned();
        let changed = document_metadata(&changed).expect("metadata renders");
        assert!(document_requires_new_version(
            true,
            Some(current),
            changed.get("document"),
        ));
        assert!(document_requires_new_version(true, None, Some(current)));
    }
}

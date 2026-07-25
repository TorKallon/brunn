use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

use axum::{
    body::Body,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
            RANGE,
        },
    },
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
    session_service,
};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AssetListQuery {
    pub session_id: Option<String>,
    #[serde(default)]
    pub include_derivatives: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AssetQuery {
    pub session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthorizedAsset {
    pub(crate) asset_id: Uuid,
    pub(crate) scope_id: Uuid,
    pub(crate) revision_id: Uuid,
    pub(crate) version: i32,
    pub(crate) object_key: String,
    pub(crate) object_version_id: Option<String>,
    pub(crate) content_hash: String,
    pub(crate) size_bytes: i64,
    pub(crate) media_type: String,
    pub(crate) etag: Option<String>,
    pub(crate) metadata: Value,
    pub(crate) path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    start: u64,
    end: u64,
}

struct IntegrityReader<R> {
    inner: R,
    hasher: Sha256,
    expected_hash: String,
    expected_size: u64,
    bytes_read: u64,
    terminal: bool,
}

impl<R> IntegrityReader<R> {
    fn new(inner: R, expected_hash: String, expected_size: u64) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            expected_hash,
            expected_size,
            bytes_read: 0,
            terminal: false,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for IntegrityReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        match Pin::new(&mut self.inner).poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                let after = buffer.filled().len();
                if after > before {
                    let bytes = &buffer.filled()[before..after];
                    self.hasher.update(bytes);
                    self.bytes_read = self
                        .bytes_read
                        .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                    return Poll::Ready(Ok(()));
                }
                if self.terminal {
                    return Poll::Ready(Ok(()));
                }
                self.terminal = true;
                let actual_hash = hex::encode(self.hasher.clone().finalize());
                if self.bytes_read != self.expected_size || actual_hash != self.expected_hash {
                    metrics::counter!(
                        "object_store.integrity_failures",
                        "failure" => "stream_sha256_mismatch"
                    )
                    .increment(1);
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "asset stream failed size or SHA-256 verification",
                    )));
                }
                metrics::counter!(
                    "asset.access",
                    "operation" => "download",
                    "result" => "stream_verified",
                    "ranged" => "false"
                )
                .increment(1);
                metrics::histogram!("asset.access.bytes", "operation" => "download_verified")
                    .record(self.bytes_read as f64);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

struct DownloadAuditReader<R> {
    inner: R,
    state: AppState,
    auth: AuthContext,
    asset: AuthorizedAsset,
    requested_bytes: u64,
    bytes_read: u64,
    finished: bool,
    _transfer_permit: OwnedSemaphorePermit,
}

impl<R> DownloadAuditReader<R> {
    fn new(
        inner: R,
        state: AppState,
        auth: AuthContext,
        asset: AuthorizedAsset,
        requested_bytes: u64,
        transfer_permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            inner,
            state,
            auth,
            asset,
            requested_bytes,
            bytes_read: 0,
            finished: false,
            _transfer_permit: transfer_permit,
        }
    }

    fn finish(&mut self, outcome: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let state = self.state.clone();
        let auth = self.auth.clone();
        let asset = self.asset.clone();
        let bytes_read = self.bytes_read;
        let requested_bytes = self.requested_bytes;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                record_download_outcome(
                    &state,
                    &auth,
                    &asset,
                    outcome,
                    bytes_read,
                    requested_bytes,
                )
                .await;
            });
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for DownloadAuditReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        match Pin::new(&mut self.inner).poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                let after = buffer.filled().len();
                let count = after.saturating_sub(before);
                self.bytes_read = self
                    .bytes_read
                    .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
                if count == 0 {
                    self.finish("completed");
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => {
                self.finish("integrity_failed");
                Poll::Ready(Err(error))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<R> Drop for DownloadAuditReader<R> {
    fn drop(&mut self) {
        if !self.finished {
            self.finish("aborted");
        }
    }
}

pub async fn list(
    state: &AppState,
    auth: &AuthContext,
    query: &AssetListQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let offset = query.offset.unwrap_or(0).max(0);
    let session_ref = query
        .session_id
        .as_deref()
        .ok_or_else(|| ApiError::invalid("asset listing requires session_id"))?;
    let session_id = parse_ref(session_ref, "session")?;
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        WITH projected AS (
        SELECT asset.id,member.record_version AS version,version.content_hash,
               version.size_bytes,version.media_type,version.stored_at,
               native_source.path,native_source.stable_import_id,
               native_source.source_kind,
               description.description_path,description.description_status,
               description.description_method,
               coalesce(access.access_count,0) AS access_count,
               access.first_accessed_at,access.last_accessed_at
        FROM straylight.sessions AS session
        JOIN straylight.corpus_members AS member
          ON member.user_id=session.user_id
         AND member.scope_id=session.scope_id
         AND member.corpus_revision_id=session.corpus_revision_id
         AND member.record_kind='asset'
         AND member.disposition='active'
        JOIN straylight.assets AS asset
          ON asset.user_id=member.user_id AND asset.id=member.record_id
        JOIN straylight.asset_versions AS version
          ON version.user_id=asset.user_id AND version.asset_id=asset.id
         AND version.version=member.record_version
        LEFT JOIN LATERAL (
          SELECT source.native_locator->>'path' AS path,
                 source.metadata->>'stable_import_id' AS stable_import_id,
                 source.source_kind
          FROM straylight.source_asset_links AS link
          JOIN straylight.source_episodes AS source
            ON source.user_id=link.user_id AND source.id=link.source_episode_id
          JOIN straylight.corpus_members AS source_member
            ON source_member.user_id=source.user_id
           AND source_member.corpus_revision_id=session.corpus_revision_id
           AND source_member.record_id=source.id
           AND source_member.record_kind='source_episode'
           AND source_member.disposition='active'
          WHERE link.user_id=asset.user_id AND link.asset_id=asset.id
            AND link.asset_version=member.record_version
            AND link.role='source_native'
          ORDER BY source.captured_at DESC,source.id DESC
          LIMIT 1
        ) AS native_source ON true
        LEFT JOIN LATERAL (
          SELECT source.native_locator->>'path' AS description_path,
                 source.metadata->>'description_status' AS description_status,
                 source.metadata->>'description_method' AS description_method
          FROM straylight.source_asset_links AS link
          JOIN straylight.source_episodes AS source
            ON source.user_id=link.user_id AND source.id=link.source_episode_id
          JOIN straylight.corpus_members AS source_member
            ON source_member.user_id=source.user_id
           AND source_member.corpus_revision_id=session.corpus_revision_id
           AND source_member.record_id=source.id
           AND source_member.record_kind='source_episode'
           AND source_member.disposition='active'
          WHERE link.user_id=asset.user_id AND link.asset_id=asset.id
            AND link.asset_version=member.record_version
            AND link.role='describes'
          ORDER BY source.captured_at DESC,source.id DESC
          LIMIT 1
        ) AS description ON true
        LEFT JOIN LATERAL (
          SELECT count(*)::bigint AS access_count,
                 min(event.recorded_at) AS first_accessed_at,
                 max(event.recorded_at) AS last_accessed_at
          FROM straylight.audit_events AS event
          WHERE event.user_id=asset.user_id
            AND event.target_record_id=asset.id
            AND event.action IN ('asset.metadata','asset.download')
            AND event.details->>'asset_version'=member.record_version::text
        ) AS access ON true
        WHERE session.user_id=$1 AND session.id=$2
          AND session.credential_id=$3
          AND session.expires_at > clock_timestamp()
          AND session.scope_id=ANY(
            SELECT scope.id
            FROM straylight.scopes AS scope
            WHERE scope.user_id=session.user_id
              AND scope.scope_ref=ANY($4::text[])
          )
          AND EXISTS (
            SELECT 1
            FROM straylight.source_asset_links AS reachable_link
            JOIN straylight.source_episodes AS reachable_source
              ON reachable_source.user_id=reachable_link.user_id
             AND reachable_source.id=reachable_link.source_episode_id
             AND reachable_source.scope_id=session.scope_id
            JOIN straylight.corpus_members AS reachable_member
              ON reachable_member.user_id=reachable_source.user_id
             AND reachable_member.scope_id=session.scope_id
             AND reachable_member.corpus_revision_id=session.corpus_revision_id
             AND reachable_member.record_id=reachable_source.id
             AND reachable_member.record_kind='source_episode'
             AND reachable_member.disposition='active'
            WHERE reachable_link.user_id=asset.user_id
              AND reachable_link.asset_id=asset.id
              AND reachable_link.asset_version=member.record_version
          )
          AND (
            $5::boolean
            OR native_source.source_kind IS DISTINCT FROM 'derived_asset_description'
          )
        )
        SELECT projected.*,
               count(*) OVER ()::bigint AS total,
               rank() OVER (
                 ORDER BY projected.access_count DESC
               )::bigint AS most_used_rank,
               rank() OVER (
                 ORDER BY projected.access_count ASC
               )::bigint AS least_used_rank,
               rank() OVER (
                 ORDER BY projected.last_accessed_at ASC NULLS FIRST
               )::bigint AS least_recently_used_rank
        FROM projected
        ORDER BY coalesce(projected.path,''),projected.id
        LIMIT $6 OFFSET $7
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(auth.credential_id.0)
    .bind(&auth.scope_refs)
    .bind(query.include_derivatives)
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let total = rows
        .first()
        .map(|row| row.try_get::<i64, _>("total"))
        .transpose()?
        .unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|row| {
            let asset_id: Uuid = row.try_get("id")?;
            Ok(json!({
                "asset_ref": format!("asset:{asset_id}"),
                "version": row.try_get::<i32,_>("version")?,
                "path": row.try_get::<Option<String>,_>("path")?,
                "stable_import_id": row.try_get::<Option<String>,_>("stable_import_id")?,
                "source_kind": row.try_get::<Option<String>,_>("source_kind")?,
                "content_hash": format!("sha256:{}", row.try_get::<String,_>("content_hash")?),
                "size_bytes": row.try_get::<i64,_>("size_bytes")?,
                "media_type": row.try_get::<String,_>("media_type")?,
                "stored_at": row.try_get::<chrono::DateTime<chrono::Utc>,_>("stored_at")?,
                "description_available": row.try_get::<Option<String>,_>("description_path")?.is_some(),
                "description_path": row.try_get::<Option<String>,_>("description_path")?,
                "description_status": row.try_get::<Option<String>,_>("description_status")?,
                "description_method": row.try_get::<Option<String>,_>("description_method")?,
                "access_count": row.try_get::<i64,_>("access_count")?,
                "first_accessed_at": row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("first_accessed_at")?,
                "last_accessed_at": row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_accessed_at")?,
                "last_used_at": row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_accessed_at")?,
                "never_used": row.try_get::<i64,_>("access_count")? == 0,
                "most_used_rank": row.try_get::<i64,_>("most_used_rank")?,
                "least_used_rank": row.try_get::<i64,_>("least_used_rank")?,
                "least_recently_used_rank": row.try_get::<i64,_>("least_recently_used_rank")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let has_more = offset.saturating_add(i64::try_from(items.len()).unwrap_or(i64::MAX)) < total;
    let result = json!({
        "items": items,
        "limit": limit,
        "offset": offset,
        "has_more": has_more,
        "total": total
    });
    session_service::project_response(state, auth, session_ref, "asset.list", "read", result).await
}

pub async fn metadata(
    state: &AppState,
    auth: &AuthContext,
    asset_ref: &str,
    query: &AssetQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let session_ref = query
        .session_id
        .as_deref()
        .ok_or_else(|| ApiError::invalid("asset metadata requires session_id"))?;
    let asset_id = parse_ref(asset_ref, "asset")?;
    let mut tx = state.begin_read(auth).await?;
    let asset = authorize_asset(&mut tx, auth, asset_id, None, query.session_id.as_deref()).await?;
    let links = load_links(&mut tx, auth.user_id.0, &asset).await?;
    tx.commit().await?;
    let result = json!({
        "ref": format!("asset:{}", asset.asset_id),
        "asset_ref": format!("asset:{}", asset.asset_id),
        "version": asset.version,
        "corpus_revision": format!("revision:{}", asset.revision_id),
        "path": asset.path,
        "content_hash": format!("sha256:{}", asset.content_hash),
        "size_bytes": asset.size_bytes,
        "media_type": asset.media_type,
        "etag": asset.etag,
        "metadata": asset.metadata,
        "sources": links,
        "download": {
            "path": format!("/v1/assets/asset:{}/versions/{}/content", asset.asset_id, asset.version),
            "supports_ranges": true,
            "integrity_header": "x-carrystate-sha256"
        }
    });
    let result = session_service::project_response(
        state,
        auth,
        session_ref,
        "asset.metadata",
        "read",
        result,
    )
    .await?;
    record_access(state, auth, &asset, "asset.metadata", None).await;
    Ok(result)
}

pub async fn content(
    state: &AppState,
    auth: &AuthContext,
    asset_ref: &str,
    version: i32,
    query: &AssetQuery,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    auth.require(Capability::Read)?;
    if version <= 0 {
        return Err(ApiError::invalid("asset version must be positive"));
    }
    let session_ref = query
        .session_id
        .as_deref()
        .ok_or_else(|| ApiError::invalid("asset download requires session_id"))?;
    let started = Instant::now();
    let asset_id = parse_ref(asset_ref, "asset")?;
    let mut tx = state.begin_read(auth).await?;
    let asset = authorize_asset(
        &mut tx,
        auth,
        asset_id,
        Some(version),
        query.session_id.as_deref(),
    )
    .await?;
    tx.commit().await?;
    let projection = session_service::project_response(
        state,
        auth,
        session_ref,
        "asset.download",
        "read",
        json!({
            "ref": format!("asset:{}", asset.asset_id),
            "content": true,
            "version": asset.version,
            "content_hash": format!("sha256:{}", asset.content_hash)
        }),
    )
    .await?;
    if projection.get("content").and_then(Value::as_bool) != Some(true) {
        return Err(ApiError::public(
            StatusCode::FORBIDDEN,
            "policy_denied",
            "the effective policy does not allow atomic binary download",
        ));
    }
    stream_authorized_content(state, auth, asset, headers, started).await
}

pub(crate) async fn stream_authorized_content(
    state: &AppState,
    auth: &AuthContext,
    asset: AuthorizedAsset,
    headers: &HeaderMap,
    started: Instant,
) -> ApiResult<Response> {
    let requested_range = headers
        .get(RANGE)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError::invalid("Range must contain valid ASCII"))
                .and_then(|value| parse_range(value, asset.size_bytes))
        })
        .transpose()?;
    let object_range = requested_range.map(|range| format!("bytes={}-{}", range.start, range.end));
    let transfer_permit = state
        .transfer_limiter
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::Internal("binary transfer limiter is unavailable".to_owned()))?;
    let expected_size = u64::try_from(asset.size_bytes)
        .map_err(|_| ApiError::Internal("asset size is negative".to_owned()))?;
    state
        .object_store
        .verify_object_identity_version(
            &asset.object_key,
            asset.object_version_id.as_deref(),
            &asset.content_hash,
            expected_size,
        )
        .await?;
    let object = state
        .object_store
        .get_stream_range_version(
            &asset.object_key,
            asset.object_version_id.as_deref(),
            object_range.as_deref(),
        )
        .await?;
    let response_size = requested_range
        .map(|range| range.end - range.start + 1)
        .unwrap_or_else(|| u64::try_from(asset.size_bytes).unwrap_or_default());
    if object.content_length != Some(i64::try_from(response_size).unwrap_or(i64::MAX)) {
        metrics::counter!(
            "object_store.integrity_failures",
            "failure" => "content_length_mismatch"
        )
        .increment(1);
        return Err(ApiError::conflict(
            "content_length_mismatch",
            "object storage returned an unexpected byte count",
            json!({"asset_ref": format!("asset:{}", asset.asset_id)}),
        ));
    }
    if let Some(range) = requested_range {
        let expected = format!("bytes {}-{}/{}", range.start, range.end, asset.size_bytes);
        if object.content_range.as_deref() != Some(expected.as_str()) {
            metrics::counter!(
                "object_store.integrity_failures",
                "failure" => "content_range_mismatch"
            )
            .increment(1);
            return Err(ApiError::conflict(
                "content_range_mismatch",
                "object storage did not honor the requested byte range",
                json!({"asset_ref": format!("asset:{}", asset.asset_id)}),
            ));
        }
    }
    let reader: Pin<Box<dyn AsyncRead + Send>> = if requested_range.is_none() {
        Box::pin(IntegrityReader::new(
            object.body.into_async_read(),
            asset.content_hash.clone(),
            expected_size,
        ))
    } else {
        Box::pin(object.body.into_async_read())
    };
    let reader = DownloadAuditReader::new(
        reader,
        state.clone(),
        auth.clone(),
        asset.clone(),
        response_size,
        transfer_permit,
    );
    let stream = ReaderStream::new(reader);
    let status = if requested_range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, &asset.media_type)
        .header(CONTENT_LENGTH, response_size)
        .header(ACCEPT_RANGES, "bytes")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", download_name(&asset)),
        )
        .header("cache-control", "private, no-store")
        .header("x-content-type-options", "nosniff")
        .header("x-carrystate-sha256", &asset.content_hash)
        .header(
            "x-carrystate-integrity",
            if requested_range.is_none() {
                "expected-sha256-verified-on-complete"
            } else {
                "range-from-verified-object-identity"
            },
        )
        .header(
            "x-carrystate-asset-ref",
            format!("asset:{}", asset.asset_id),
        )
        .header("x-carrystate-asset-version", asset.version);
    if let Some(range) = requested_range {
        builder = builder.header(
            CONTENT_RANGE,
            format!("bytes {}-{}/{}", range.start, range.end, asset.size_bytes),
        );
    }
    if let Some(etag) = object.etag.as_deref().or(asset.etag.as_deref())
        && let Ok(value) = HeaderValue::from_str(etag)
    {
        builder = builder.header(ETAG, value);
    }
    let response = builder
        .body(Body::from_stream(stream))
        .map_err(|error| ApiError::Internal(format!("could not build asset response: {error}")))?;
    metrics::counter!(
        "asset.access",
        "operation" => "download",
        "result" => "stream_started",
        "ranged" => if requested_range.is_some() { "true" } else { "false" }
    )
    .increment(1);
    metrics::histogram!(
        "asset.access.duration_ms",
        "operation" => "download"
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!("asset.access.bytes", "operation" => "download_requested")
        .record(response_size as f64);
    Ok(response)
}

async fn authorize_asset(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auth: &AuthContext,
    asset_id: Uuid,
    requested_version: Option<i32>,
    session_ref: Option<&str>,
) -> ApiResult<AuthorizedAsset> {
    let session_id = parse_optional_ref(session_ref, "session")?;
    let row = if let Some(session_id) = session_id {
        sqlx::query(
            r#"
            SELECT asset.id AS asset_id,session.scope_id,session.corpus_revision_id,
                   member.record_version AS version,version.object_key,
                   version.object_version_id,
                   version.content_hash,version.size_bytes,version.media_type,
                   version.etag,version.metadata,
                   source.native_locator->>'path' AS path
            FROM straylight.sessions AS session
            JOIN straylight.corpus_members AS member
              ON member.user_id=session.user_id
             AND member.scope_id=session.scope_id
             AND member.corpus_revision_id=session.corpus_revision_id
             AND member.record_kind='asset'
             AND member.disposition='active'
            JOIN straylight.assets AS asset
              ON asset.user_id=member.user_id AND asset.id=member.record_id
            JOIN straylight.asset_versions AS version
              ON version.user_id=asset.user_id AND version.asset_id=asset.id
             AND version.version=member.record_version
            LEFT JOIN straylight.source_asset_links AS native_link
              ON native_link.user_id=asset.user_id
             AND native_link.asset_id=asset.id
             AND native_link.asset_version=member.record_version
             AND native_link.role='source_native'
            LEFT JOIN straylight.source_episodes AS source
              ON source.user_id=native_link.user_id
             AND source.id=native_link.source_episode_id
            WHERE session.user_id=$1 AND session.id=$2
              AND session.credential_id=$3
              AND session.expires_at > clock_timestamp()
              AND asset.id=$4
              AND ($5::integer IS NULL OR member.record_version=$5)
              AND EXISTS (
                SELECT 1
                FROM straylight.source_asset_links AS reachable_link
                JOIN straylight.source_episodes AS reachable_source
                  ON reachable_source.user_id=reachable_link.user_id
                 AND reachable_source.id=reachable_link.source_episode_id
                 AND reachable_source.scope_id=session.scope_id
                JOIN straylight.corpus_members AS reachable_member
                  ON reachable_member.user_id=reachable_source.user_id
                 AND reachable_member.scope_id=session.scope_id
                 AND reachable_member.corpus_revision_id=session.corpus_revision_id
                 AND reachable_member.record_id=reachable_source.id
                 AND reachable_member.record_kind='source_episode'
                 AND reachable_member.disposition='active'
                WHERE reachable_link.user_id=asset.user_id
                  AND reachable_link.asset_id=asset.id
                  AND reachable_link.asset_version=member.record_version
              )
              AND session.scope_id=ANY(
                SELECT scope.id
                FROM straylight.scopes AS scope
                WHERE scope.user_id=session.user_id
                  AND scope.scope_ref=ANY($6::text[])
              )
            ORDER BY source.created_at NULLS LAST
            LIMIT 1
            "#,
        )
        .bind(auth.user_id.0)
        .bind(session_id)
        .bind(auth.credential_id.0)
        .bind(asset_id)
        .bind(requested_version)
        .bind(&auth.scope_refs)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT asset.id AS asset_id,manifest.scope_id,
                   manifest.active_corpus_revision_id AS corpus_revision_id,
                   member.record_version AS version,version.object_key,
                   version.object_version_id,
                   version.content_hash,version.size_bytes,version.media_type,
                   version.etag,version.metadata,
                   source.native_locator->>'path' AS path
            FROM straylight.active_manifests AS manifest
            JOIN straylight.scopes AS scope
              ON scope.user_id=manifest.user_id AND scope.id=manifest.scope_id
            JOIN straylight.corpus_members AS member
              ON member.user_id=manifest.user_id
             AND member.scope_id=manifest.scope_id
             AND member.corpus_revision_id=manifest.active_corpus_revision_id
             AND member.record_kind='asset'
             AND member.disposition='active'
            JOIN straylight.assets AS asset
              ON asset.user_id=member.user_id AND asset.id=member.record_id
            JOIN straylight.asset_versions AS version
              ON version.user_id=asset.user_id AND version.asset_id=asset.id
             AND version.version=member.record_version
            LEFT JOIN straylight.source_asset_links AS native_link
              ON native_link.user_id=asset.user_id
             AND native_link.asset_id=asset.id
             AND native_link.asset_version=member.record_version
             AND native_link.role='source_native'
            LEFT JOIN straylight.source_episodes AS source
              ON source.user_id=native_link.user_id
             AND source.id=native_link.source_episode_id
            WHERE manifest.user_id=$1 AND scope.scope_ref=ANY($2::text[])
              AND asset.id=$3
              AND ($4::integer IS NULL OR member.record_version=$4)
              AND EXISTS (
                SELECT 1
                FROM straylight.source_asset_links AS reachable_link
                JOIN straylight.source_episodes AS reachable_source
                  ON reachable_source.user_id=reachable_link.user_id
                 AND reachable_source.id=reachable_link.source_episode_id
                 AND reachable_source.scope_id=manifest.scope_id
                JOIN straylight.corpus_members AS reachable_member
                  ON reachable_member.user_id=reachable_source.user_id
                 AND reachable_member.scope_id=manifest.scope_id
                 AND reachable_member.corpus_revision_id=manifest.active_corpus_revision_id
                 AND reachable_member.record_id=reachable_source.id
                 AND reachable_member.record_kind='source_episode'
                 AND reachable_member.disposition='active'
                WHERE reachable_link.user_id=asset.user_id
                  AND reachable_link.asset_id=asset.id
                  AND reachable_link.asset_version=member.record_version
              )
            ORDER BY source.created_at NULLS LAST
            LIMIT 1
            "#,
        )
        .bind(auth.user_id.0)
        .bind(&auth.scope_refs)
        .bind(asset_id)
        .bind(requested_version)
        .fetch_optional(&mut **tx)
        .await?
    }
    .ok_or_else(|| ApiError::not_found("asset_not_found", &format!("asset:{asset_id}")))?;
    Ok(AuthorizedAsset {
        asset_id: row.try_get("asset_id")?,
        scope_id: row.try_get("scope_id")?,
        revision_id: row.try_get("corpus_revision_id")?,
        version: row.try_get("version")?,
        object_key: row.try_get("object_key")?,
        object_version_id: row.try_get("object_version_id")?,
        content_hash: row.try_get("content_hash")?,
        size_bytes: row.try_get("size_bytes")?,
        media_type: row.try_get("media_type")?,
        etag: row.try_get("etag")?,
        metadata: row.try_get("metadata")?,
        path: row.try_get("path")?,
    })
}

async fn load_links(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    asset: &AuthorizedAsset,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT link.role,source.id,source.title,source.source_kind,
               source.native_locator->>'path' AS path,source.content_hash
        FROM straylight.source_asset_links AS link
        JOIN straylight.source_episodes AS source
          ON source.user_id=link.user_id AND source.id=link.source_episode_id
        JOIN straylight.corpus_members AS member
          ON member.user_id=source.user_id
         AND member.corpus_revision_id=$4
         AND member.record_id=source.id
         AND member.record_kind='source_episode'
         AND member.disposition='active'
        WHERE link.user_id=$1 AND link.asset_id=$2 AND link.asset_version=$3
        ORDER BY link.role,source.created_at,source.id
        "#,
    )
    .bind(user_id)
    .bind(asset.asset_id)
    .bind(asset.version)
    .bind(asset.revision_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let source_id: Uuid = row.try_get("id")?;
            Ok(json!({
                "role": row.try_get::<String,_>("role")?,
                "source_ref": format!("source:{source_id}"),
                "title": row.try_get::<Option<String>,_>("title")?,
                "source_kind": row.try_get::<String,_>("source_kind")?,
                "path": row.try_get::<Option<String>,_>("path")?,
                "content_hash": row.try_get::<Option<String>,_>("content_hash")?
                    .map(|hash| format!("sha256:{hash}"))
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::from)
}

async fn record_access(
    state: &AppState,
    auth: &AuthContext,
    asset: &AuthorizedAsset,
    action: &'static str,
    bytes_served: Option<u64>,
) {
    let result = async {
        let mut tx = state.begin_write(auth).await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.audit_events (
              id,user_id,scope_id,credential_id,action,target_record_id,
              details,content_free
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,true)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(auth.user_id.0)
        .bind(asset.scope_id)
        .bind(auth.credential_id.0)
        .bind(action)
        .bind(asset.asset_id)
        .bind(json!({
            "asset_version": asset.version,
            "corpus_revision": format!("revision:{}", asset.revision_id),
            "bytes_served": bytes_served
        }))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        ApiResult::Ok(())
    }
    .await;
    if let Err(error) = result {
        metrics::counter!(
            "asset.access.audit",
            "operation" => action,
            "result" => "error"
        )
        .increment(1);
        tracing::warn!(?error, action, "asset access audit could not be persisted");
    } else {
        metrics::counter!(
            "asset.access.audit",
            "operation" => action,
            "result" => "success"
        )
        .increment(1);
    }
}

async fn record_download_outcome(
    state: &AppState,
    auth: &AuthContext,
    asset: &AuthorizedAsset,
    outcome: &'static str,
    bytes_read: u64,
    requested_bytes: u64,
) {
    let action = match outcome {
        "completed" => "asset.download",
        "integrity_failed" => "asset.download_failed",
        _ => "asset.download_aborted",
    };
    let result = async {
        let mut tx = state.begin_write(auth).await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.audit_events (
              id,user_id,scope_id,credential_id,action,target_record_id,
              details,content_free
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,true)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(auth.user_id.0)
        .bind(asset.scope_id)
        .bind(auth.credential_id.0)
        .bind(action)
        .bind(asset.asset_id)
        .bind(json!({
            "asset_version": asset.version,
            "corpus_revision": format!("revision:{}", asset.revision_id),
            "outcome": outcome,
            "bytes_served": bytes_read,
            "bytes_requested": requested_bytes
        }))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        ApiResult::Ok(())
    }
    .await;
    let result_label = if result.is_ok() { "success" } else { "error" };
    metrics::counter!(
        "asset.access.audit",
        "operation" => action,
        "result" => result_label
    )
    .increment(1);
    metrics::counter!(
        "asset.access",
        "operation" => "download",
        "result" => outcome,
        "ranged" => if requested_bytes
            != u64::try_from(asset.size_bytes).unwrap_or_default()
        {
            "true"
        } else {
            "false"
        }
    )
    .increment(1);
    metrics::histogram!(
        "asset.access.bytes",
        "operation" => "download_actual",
        "result" => outcome
    )
    .record(bytes_read as f64);
    if let Err(error) = result {
        tracing::warn!(
            ?error,
            action,
            "asset download outcome audit could not be persisted"
        );
    }
}

fn parse_ref(value: &str, prefix: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.trim_start_matches(&format!("{prefix}:")))
        .map_err(|_| ApiError::invalid(format!("{prefix} reference is invalid")))
}

fn parse_optional_ref(value: Option<&str>, prefix: &str) -> ApiResult<Option<Uuid>> {
    value.map(|value| parse_ref(value, prefix)).transpose()
}

fn parse_range(value: &str, size_bytes: i64) -> ApiResult<ByteRange> {
    let size = u64::try_from(size_bytes)
        .map_err(|_| ApiError::Internal("asset size is negative".to_owned()))?;
    let raw = value
        .strip_prefix("bytes=")
        .ok_or_else(|| ApiError::invalid("only byte ranges are supported"))?;
    if raw.contains(',') || raw.is_empty() || size == 0 {
        return Err(ApiError::public(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "range_not_satisfiable",
            "the requested byte range is not satisfiable",
        ));
    }
    let (start, end) = raw
        .split_once('-')
        .ok_or_else(|| ApiError::invalid("Range must use bytes=start-end"))?;
    let range = if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| {
            ApiError::invalid("a suffix byte range must contain a positive integer")
        })?;
        if suffix == 0 {
            return Err(ApiError::public(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "range_not_satisfiable",
                "the requested byte range is not satisfiable",
            ));
        }
        ByteRange {
            start: size.saturating_sub(suffix),
            end: size - 1,
        }
    } else {
        let start = start
            .parse::<u64>()
            .map_err(|_| ApiError::invalid("byte range start is invalid"))?;
        let end = if end.is_empty() {
            size.saturating_sub(1)
        } else {
            end.parse::<u64>()
                .map_err(|_| ApiError::invalid("byte range end is invalid"))?
        };
        ByteRange {
            start,
            end: end.min(size.saturating_sub(1)),
        }
    };
    if range.start >= size || range.end < range.start {
        return Err(ApiError::public(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "range_not_satisfiable",
            "the requested byte range is not satisfiable",
        ));
    }
    Ok(range)
}

fn download_name(asset: &AuthorizedAsset) -> String {
    let candidate = asset
        .path
        .as_deref()
        .and_then(|path| path.rsplit('/').next())
        .unwrap_or("asset.bin");
    let safe = candidate
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(180)
        .collect::<String>();
    if safe.is_empty() {
        format!("asset-{}.bin", asset.asset_id)
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn byte_ranges_are_bounded_to_the_asset() {
        assert_eq!(
            parse_range("bytes=2-5", 10).unwrap(),
            ByteRange { start: 2, end: 5 }
        );
        assert_eq!(
            parse_range("bytes=8-", 10).unwrap(),
            ByteRange { start: 8, end: 9 }
        );
        assert_eq!(
            parse_range("bytes=-3", 10).unwrap(),
            ByteRange { start: 7, end: 9 }
        );
        assert_eq!(
            parse_range("bytes=4-100", 10).unwrap(),
            ByteRange { start: 4, end: 9 }
        );
    }

    #[test]
    fn malformed_or_multiple_ranges_are_rejected() {
        assert!(parse_range("items=1-2", 10).is_err());
        assert!(parse_range("bytes=1-2,4-5", 10).is_err());
        assert!(parse_range("bytes=10-", 10).is_err());
        assert!(parse_range("bytes=-0", 10).is_err());
    }

    #[test]
    fn content_disposition_never_uses_untrusted_path_characters() {
        let asset = AuthorizedAsset {
            asset_id: Uuid::nil(),
            scope_id: Uuid::nil(),
            revision_id: Uuid::nil(),
            version: 1,
            object_key: String::new(),
            object_version_id: Some("version-1".to_owned()),
            content_hash: "0".repeat(64),
            size_bytes: 0,
            media_type: "application/octet-stream".to_owned(),
            etag: None,
            metadata: json!({}),
            path: Some("../../bad\"name\r\n.sql".to_owned()),
        };
        assert_eq!(download_name(&asset), "bad_name__.sql");
    }

    #[tokio::test]
    async fn full_stream_integrity_reader_accepts_exact_bytes_and_rejects_same_size_corruption() {
        let expected = b"exact native bytes";
        let digest = hex::encode(Sha256::digest(expected));
        let mut valid = IntegrityReader::new(
            std::io::Cursor::new(expected),
            digest.clone(),
            expected.len() as u64,
        );
        let mut output = Vec::new();
        valid.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, expected);

        let corrupt = b"exact native bytex";
        assert_eq!(corrupt.len(), expected.len());
        let mut invalid =
            IntegrityReader::new(std::io::Cursor::new(corrupt), digest, expected.len() as u64);
        let mut output = Vec::new();
        assert!(invalid.read_to_end(&mut output).await.is_err());
    }
}

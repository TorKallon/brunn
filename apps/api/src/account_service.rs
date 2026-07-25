use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    http::{
        Response, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
    },
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
};

struct AccountExportIntegrityReader<R> {
    inner: R,
    hasher: Sha256,
    expected_hash: String,
    expected_size: u64,
    bytes_read: u64,
    terminal: bool,
    mismatch_pending: bool,
}

impl<R> AccountExportIntegrityReader<R> {
    fn new(inner: R, expected_hash: String, expected_size: u64) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            expected_hash,
            expected_size,
            bytes_read: 0,
            terminal: false,
            mismatch_pending: false,
        }
    }

    fn mismatch(&self) -> io::Error {
        metrics::counter!(
            "object_store.integrity_failures",
            "failure" => "account_export_stream_mismatch"
        )
        .increment(1);
        io::Error::new(
            io::ErrorKind::InvalidData,
            "account export stream failed size or SHA-256 verification",
        )
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AccountExportIntegrityReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.mismatch_pending {
            self.mismatch_pending = false;
            self.terminal = true;
            return Poll::Ready(Err(self.mismatch()));
        }
        if self.terminal {
            return Poll::Ready(Ok(()));
        }
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
                    if self.bytes_read > self.expected_size {
                        self.mismatch_pending = true;
                    }
                    return Poll::Ready(Ok(()));
                }
                self.terminal = true;
                let actual_hash = hex::encode(self.hasher.clone().finalize());
                if self.bytes_read != self.expected_size || actual_hash != self.expected_hash {
                    return Poll::Ready(Err(self.mismatch()));
                }
                metrics::counter!("account.exports", "result" => "download_stream_verified")
                    .increment(1);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteAccountRequest {
    confirmation: String,
    reason: String,
}

pub async fn request_export(state: &AppState, auth: &AuthContext) -> ApiResult<Value> {
    auth.require(Capability::CredentialManage)?;
    let export_id = Uuid::now_v7();
    let ttl_seconds = i64::try_from(state.config.account_export_ttl.as_secs()).unwrap_or(i64::MAX);
    let mut tx = state.begin_write(auth).await?;
    let row = sqlx::query(
        r#"
        INSERT INTO straylight.account_exports (
          id,user_id,requested_by_credential_id,status,expires_at
        ) VALUES (
          $1,$2,$3,'queued',clock_timestamp()+make_interval(secs => $4)
        )
        RETURNING created_at,expires_at
        "#,
    )
    .bind(export_id)
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(ttl_seconds)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_export_database_error)?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("export:{export_id}"),
        "status": "queued",
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "expires_at": row.try_get::<DateTime<Utc>,_>("expires_at")?
    }))
}

pub async fn list_exports(state: &AppState, auth: &AuthContext) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT id,status,content_hash,size_bytes,table_count,object_count,
               failure_code,created_at,started_at,completed_at,expires_at
        FROM straylight.account_exports
        WHERE user_id=$1
        ORDER BY created_at DESC,id DESC
        LIMIT 100
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let items = rows
        .iter()
        .map(export_value)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(json!({"items": items, "total": items.len()}))
}

pub async fn get_export(
    state: &AppState,
    auth: &AuthContext,
    export_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let export_id = parse_ref(export_ref, "export")?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT id,status,content_hash,size_bytes,table_count,object_count,
               failure_code,created_at,started_at,completed_at,expires_at
        FROM straylight.account_exports
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(export_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("account_export_not_found", export_ref))?;
    tx.commit().await?;
    export_value(&row)
}

pub async fn download_export(
    state: &AppState,
    auth: &AuthContext,
    export_ref: &str,
) -> ApiResult<Response<Body>> {
    auth.require(Capability::CredentialManage)?;
    let export_id = parse_ref(export_ref, "export")?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT status,object_key,object_version_id,content_hash,size_bytes,expires_at
        FROM straylight.account_exports
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(export_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("account_export_not_found", export_ref))?;
    let status: String = row.try_get("status")?;
    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    let object_key: Option<String> = row.try_get("object_key")?;
    let object_version_id: Option<String> = row.try_get("object_version_id")?;
    let content_hash: Option<String> = row.try_get("content_hash")?;
    let size_bytes: Option<i64> = row.try_get("size_bytes")?;
    tx.commit().await?;
    if status != "ready" || expires_at <= Utc::now() {
        return Err(ApiError::conflict(
            "account_export_not_ready",
            "the account export is not ready for download",
            json!({"status": status, "expires_at": expires_at}),
        ));
    }
    let object_key = object_key.ok_or_else(|| {
        ApiError::Internal("ready account export has no object storage key".to_owned())
    })?;
    let object_version_id = object_version_id
        .filter(|version_id| !version_id.is_empty() && version_id != "null")
        .ok_or_else(|| {
            ApiError::Internal("ready account export has no exact storage version ID".to_owned())
        })?;
    let content_hash = content_hash.ok_or_else(|| {
        ApiError::Internal("ready account export has no SHA-256 identity".to_owned())
    })?;
    if content_hash.len() != 64 || !content_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::Internal(
            "ready account export has an invalid SHA-256 identity".to_owned(),
        ));
    }
    let content_hash = content_hash.to_ascii_lowercase();
    let size_bytes = size_bytes
        .and_then(|size| u64::try_from(size).ok())
        .ok_or_else(|| {
            ApiError::Internal("ready account export has an invalid stored size".to_owned())
        })?;
    let object = state
        .object_store
        .get_stream_version(&object_key, Some(&object_version_id))
        .await?;
    if object.content_length != Some(i64::try_from(size_bytes).unwrap_or(i64::MAX)) {
        metrics::counter!(
            "object_store.integrity_failures",
            "failure" => "account_export_head_size_mismatch"
        )
        .increment(1);
        return Err(ApiError::conflict(
            "account_export_integrity_mismatch",
            "the stored account export does not match its recorded size",
            json!({"export_ref": format!("export:{export_id}")}),
        ));
    }
    let stream = ReaderStream::new(AccountExportIntegrityReader::new(
        object.body.into_async_read(),
        content_hash.clone(),
        size_bytes,
    ));
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/gzip")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"straylight-export-{export_id}.tar.gz\""),
        )
        .header("cache-control", "private, no-store")
        .header("x-content-type-options", "nosniff")
        .header("x-carrystate-sha256", &content_hash)
        .header(
            "x-carrystate-integrity",
            "expected-sha256-verified-on-complete",
        )
        .header(CONTENT_LENGTH, size_bytes.to_string());
    response
        .body(Body::from_stream(stream))
        .map_err(|error| ApiError::Internal(format!("could not build export response: {error}")))
}

pub async fn delete_export(
    state: &AppState,
    auth: &AuthContext,
    export_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::CredentialManage)?;
    let export_id = parse_ref(export_ref, "export")?;
    let object_key = {
        let mut tx = state.begin_write(auth).await?;
        sqlx::query("SELECT straylight.assert_storage_write_allowed($1)")
            .bind(auth.user_id.0)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(
            "SELECT status,object_key
             FROM straylight_auth.begin_account_export_delete($1,$2)",
        )
        .bind(auth.user_id.0)
        .bind(export_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_export_database_error)?;
        let object_key: Option<String> = row.try_get("object_key")?;
        tx.commit().await?;
        object_key
    };

    let purge = if let Some(key) = object_key {
        Some(state.object_store.purge_all_versions(&key).await?)
    } else {
        None
    };

    let mut tx = state.begin_write(auth).await?;
    sqlx::query("SELECT straylight.assert_storage_write_allowed($1)")
        .bind(auth.user_id.0)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT straylight_auth.finish_account_export_delete($1,$2)")
        .bind(auth.user_id.0)
        .bind(export_id)
        .execute(&mut *tx)
        .await
        .map_err(map_export_database_error)?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("export:{export_id}"),
        "status": "deleted",
        "object_purge": purge
    }))
}

pub async fn request_deletion(
    state: &AppState,
    auth: &AuthContext,
    request: DeleteAccountRequest,
) -> ApiResult<Value> {
    auth.require(Capability::CredentialManage)?;
    let mut tx = state.begin_write(auth).await?;
    let request_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT straylight_auth.request_account_deletion($1,$2,$3,$4)",
    )
    .bind(auth.user_id.0)
    .bind(&request.confirmation)
    .bind(request.reason.trim())
    .bind(state.config.account_deletion_backup_retention_days)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_deletion_database_error)?;
    let row = sqlx::query(
        r#"
        SELECT created_at,backup_expiry_due_at
        FROM straylight.account_deletion_requests
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(request_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("account_deletion:{request_id}"),
        "status": "queued",
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "backup_expiry_due_at": row.try_get::<DateTime<Utc>,_>("backup_expiry_due_at")?
    }))
}

pub async fn get_deletion(
    state: &AppState,
    auth: &AuthContext,
    request_ref: Option<&str>,
) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let request_id = request_ref
        .map(|value| parse_ref(value, "account_deletion"))
        .transpose()?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT id,status,records_total,records_completed,backup_expiry_due_at,
               backup_erasure_verified_at,backup_erasure_watermark_at,
               backup_erasure_receipt_sha256,backup_erasure_source,
               failure_code,terminal_result,created_at,started_at,completed_at
        FROM straylight.account_deletion_requests
        WHERE user_id=$1 AND ($2::uuid IS NULL OR id=$2)
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(auth.user_id.0)
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            "account_deletion_not_found",
            request_ref.unwrap_or("latest"),
        )
    })?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("account_deletion:{}", row.try_get::<Uuid,_>("id")?),
        "status": row.try_get::<String,_>("status")?,
        "records_total": row.try_get::<i64,_>("records_total")?,
        "records_completed": row.try_get::<i64,_>("records_completed")?,
        "backup_expiry_due_at": row.try_get::<DateTime<Utc>,_>("backup_expiry_due_at")?,
        "backup_erasure_verified_at":
            row.try_get::<Option<DateTime<Utc>>,_>("backup_erasure_verified_at")?,
        "backup_erasure_watermark_at":
            row.try_get::<Option<DateTime<Utc>>,_>("backup_erasure_watermark_at")?,
        "backup_erasure_receipt_sha256":
            row.try_get::<Option<String>,_>("backup_erasure_receipt_sha256")?
                .map(|hash| format!("sha256:{hash}")),
        "backup_erasure_source":
            row.try_get::<Option<String>,_>("backup_erasure_source")?,
        "failure_code": row.try_get::<Option<String>,_>("failure_code")?,
        "result": row.try_get::<Option<Value>,_>("terminal_result")?,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "started_at": row.try_get::<Option<DateTime<Utc>>,_>("started_at")?,
        "completed_at": row.try_get::<Option<DateTime<Utc>>,_>("completed_at")?
    }))
}

fn export_value(row: &sqlx::postgres::PgRow) -> ApiResult<Value> {
    let id: Uuid = row.try_get("id")?;
    let status: String = row.try_get("status")?;
    Ok(json!({
        "id": format!("export:{id}"),
        "status": status,
        "content_hash": row.try_get::<Option<String>,_>("content_hash")?
            .map(|hash| format!("sha256:{hash}")),
        "size_bytes": row.try_get::<Option<i64>,_>("size_bytes")?,
        "table_count": row.try_get::<Option<i32>,_>("table_count")?,
        "object_count": row.try_get::<Option<i32>,_>("object_count")?,
        "failure_code": row.try_get::<Option<String>,_>("failure_code")?,
        "download_path": (status == "ready").then(|| format!("/v1/account/exports/export:{id}/content")),
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "started_at": row.try_get::<Option<DateTime<Utc>>,_>("started_at")?,
        "completed_at": row.try_get::<Option<DateTime<Utc>>,_>("completed_at")?,
        "expires_at": row.try_get::<DateTime<Utc>,_>("expires_at")?
    }))
}

fn parse_ref(value: &str, prefix: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.strip_prefix(&format!("{prefix}:")).unwrap_or(value))
        .map_err(|_| ApiError::invalid(format!("{prefix} reference is invalid")))
}

fn map_export_database_error(error: sqlx::Error) -> ApiError {
    if let Some(database) = error.as_database_error() {
        return match database.code().as_deref() {
            Some("23505") => ApiError::conflict(
                "account_export_active",
                "an active account export already exists",
                json!({}),
            ),
            Some("P0002") => ApiError::not_found("account_export_not_found", "export"),
            Some("55000") => ApiError::conflict(
                "account_export_running",
                "a running account export cannot be deleted",
                json!({}),
            ),
            _ => ApiError::Database(error),
        };
    }
    ApiError::Database(error)
}

fn map_deletion_database_error(error: sqlx::Error) -> ApiError {
    if let Some(database) = error.as_database_error() {
        return match database.code().as_deref() {
            Some("22023") => ApiError::invalid(
                "confirmation must exactly match DELETE <external_ref>, and reason must be valid",
            ),
            Some("23505") => ApiError::conflict(
                "account_deletion_active",
                "an account deletion is already active",
                json!({}),
            ),
            Some("55000") => ApiError::conflict(
                "account_export_running",
                "a running account export must finish before account deletion",
                json!({}),
            ),
            Some("P0002") => ApiError::not_found("user_not_found", "user"),
            _ => ApiError::Database(error),
        };
    }
    ApiError::Database(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn account_refs_are_typed_and_strict() {
        let id = Uuid::now_v7();
        assert_eq!(parse_ref(&format!("export:{id}"), "export").unwrap(), id);
        assert!(parse_ref("export:not-a-uuid", "export").is_err());
    }

    #[test]
    fn export_errors_do_not_include_database_details() {
        let error = ApiError::conflict("account_export_active", "active", json!({}));
        assert!(matches!(
            error,
            ApiError::Public {
                status: StatusCode::CONFLICT,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn export_stream_requires_exact_size_and_sha256() {
        let expected = b"exact account export bytes";
        let digest = hex::encode(Sha256::digest(expected));
        let mut valid = AccountExportIntegrityReader::new(
            std::io::Cursor::new(expected),
            digest.clone(),
            expected.len() as u64,
        );
        let mut output = Vec::new();
        valid.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, expected);

        let corrupt = b"exact account export bytez";
        assert_eq!(corrupt.len(), expected.len());
        let mut invalid = AccountExportIntegrityReader::new(
            std::io::Cursor::new(corrupt),
            digest.clone(),
            expected.len() as u64,
        );
        assert!(invalid.read_to_end(&mut Vec::new()).await.is_err());

        let mut short = AccountExportIntegrityReader::new(
            std::io::Cursor::new(&expected[..expected.len() - 1]),
            digest.clone(),
            expected.len() as u64,
        );
        assert!(short.read_to_end(&mut Vec::new()).await.is_err());

        let mut long = AccountExportIntegrityReader::new(
            std::io::Cursor::new([expected.as_slice(), b"!"].concat()),
            digest,
            expected.len() as u64,
        );
        assert!(long.read_to_end(&mut Vec::new()).await.is_err());
    }
}

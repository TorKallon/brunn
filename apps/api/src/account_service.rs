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
use sqlx::Row;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
};

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
        SELECT status,object_key,size_bytes,expires_at
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
    let object = state.object_store.get_stream(&object_key).await?;
    let stream = ReaderStream::new(object.body.into_async_read());
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/gzip")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"straylight-export-{export_id}.tar.gz\""),
        );
    if let Some(size) = size_bytes.or(object.content_length) {
        response = response.header(CONTENT_LENGTH, size);
    }
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
    let mut read_tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        "SELECT status,object_key FROM straylight.account_exports WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(export_id)
    .fetch_optional(&mut *read_tx)
    .await?
    .ok_or_else(|| ApiError::not_found("account_export_not_found", export_ref))?;
    let status: String = row.try_get("status")?;
    let object_key: Option<String> = row.try_get("object_key")?;
    read_tx.commit().await?;
    if status == "running" {
        return Err(ApiError::conflict(
            "account_export_running",
            "a running account export cannot be deleted",
            json!({"id": export_ref}),
        ));
    }
    let purge = if let Some(key) = object_key {
        Some(state.object_store.purge_all_versions(&key).await?)
    } else {
        None
    };
    let mut tx = state.begin_write(auth).await?;
    sqlx::query("SELECT straylight_auth.mark_account_export_deleted($1,$2)")
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
}

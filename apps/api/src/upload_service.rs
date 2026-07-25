use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    Client,
    config::{Credentials, SharedCredentialsProvider},
};
use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    config::Config,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
    quota,
    write_service::{self, StageUpload, StageUploadContent},
};

const PART_SIZE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_UPLOAD_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const UPLOAD_TTL_HOURS: i32 = 24;
const COMPLETION_LEASE_SECONDS: i32 = 60 * 60;
const MULTIPART_INTENT_TTL_SECONDS: i32 = 15 * 60;

static MULTIPART_INVENTORY_CLIENT: OnceCell<Client> = OnceCell::const_new();

#[derive(Clone, Debug, Deserialize)]
pub struct CreateUploadRequest {
    pub scope: String,
    pub path: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub media_type: Option<String>,
    pub stable_import_id: Option<String>,
    pub modified_unix_ns: Option<u64>,
    pub mode: Option<u32>,
    pub attachment_context: Option<Value>,
    #[serde(default = "default_true")]
    pub describe_binary: bool,
}

#[derive(Clone, Debug)]
struct UploadRow {
    id: Uuid,
    scope_id: Uuid,
    credential_id: Uuid,
    stable_import_id: Option<String>,
    path: String,
    media_type: String,
    import_metadata: Value,
    describe_binary: bool,
    expected_content_hash: String,
    expected_size_bytes: i64,
    part_size_bytes: i32,
    temporary_object_key: String,
    temporary_object_version_id: Option<String>,
    canonical_object_key: Option<String>,
    canonical_object_version_id: Option<String>,
    multipart_upload_id: String,
    stage_id: Option<Uuid>,
    status: String,
    completion_token: Option<Uuid>,
    completion_lease_expires_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
}

pub async fn create(
    state: &AppState,
    auth: &AuthContext,
    mut request: CreateUploadRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Stage)?;
    request.scope = request.scope.trim().to_owned();
    request.path = request.path.trim().to_owned();
    write_service::validate_stage_path(&request.path)?;
    if request.size_bytes == 0 || request.size_bytes > MAX_UPLOAD_BYTES {
        return Err(ApiError::public(
            StatusCode::PAYLOAD_TOO_LARGE,
            "upload_size_invalid",
            "resumable uploads must contain between 1 byte and 5 GiB",
        ));
    }
    let content_hash = normalized_hash(&request.content_hash)?;
    let media_type = normalize_media_type(request.media_type.as_deref())?;
    let upload_id = Uuid::now_v7();
    let stable_import_id = request
        .stable_import_id
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| Some(format!("upload:{upload_id}")));
    if stable_import_id
        .as_ref()
        .is_some_and(|value| value.len() > 240)
    {
        return Err(ApiError::invalid(
            "stable_import_id is limited to 240 characters",
        ));
    }
    if request.mode.is_some_and(|mode| mode > 0o777) {
        return Err(ApiError::invalid(
            "mode must contain only ordinary portable permission bits",
        ));
    }
    validate_attachment_context(request.attachment_context.as_ref())?;
    let import_metadata = json!({
        "modified_unix_ns": request.modified_unix_ns,
        "mode": request.mode,
        "attachment_context": request.attachment_context
    });
    let intended_object_key = format!("{}/uploads/{upload_id}", auth.user_id.0);
    let mut intent_tx = state.begin_write(auth).await?;
    lock_storage_write(&mut intent_tx, auth.user_id.0).await?;
    quota::ensure_storage_capacity(
        &mut intent_tx,
        auth.user_id.0,
        &[(
            format!("pending-upload-{upload_id}"),
            usize::try_from(request.size_bytes).unwrap_or(usize::MAX),
        )],
    )
    .await?;
    let scope_id = resolve_scope(&mut intent_tx, auth, &request.scope).await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.asset_multipart_intents (
          id,user_id,scope_id,credential_id,object_key,expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,
          clock_timestamp()+make_interval(secs => $6::integer)
        )
        "#,
    )
    .bind(upload_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(&intended_object_key)
    .bind(MULTIPART_INTENT_TTL_SECONDS)
    .execute(&mut *intent_tx)
    .await?;
    intent_tx.commit().await?;

    let mut tx = match state.begin_write(auth).await {
        Ok(tx) => tx,
        Err(error) => {
            clear_multipart_intent(state, auth, upload_id).await;
            return Err(error);
        }
    };
    if let Err(error) = lock_storage_write(&mut tx, auth.user_id.0).await {
        let _ = tx.rollback().await;
        clear_multipart_intent(state, auth, upload_id).await;
        return Err(error);
    }
    let multipart = match state
        .object_store
        .create_multipart_upload(auth.user_id, upload_id, &media_type, &content_hash)
        .await
    {
        Ok(multipart) => multipart,
        Err(error) => {
            let _ = tx.rollback().await;
            clear_multipart_intent(state, auth, upload_id).await;
            return Err(error);
        }
    };
    if multipart.object_key != intended_object_key {
        let _ = tx.rollback().await;
        let _ = state
            .object_store
            .abort_multipart_upload(&multipart.object_key, &multipart.upload_id)
            .await;
        clear_multipart_intent(state, auth, upload_id).await;
        return Err(ApiError::Internal(
            "object storage created a multipart upload under an unexpected key".to_owned(),
        ));
    }
    let finalize = async {
        let intent_updated = sqlx::query(
            r#"
            UPDATE straylight.asset_multipart_intents
            SET multipart_upload_id=$1,updated_at=clock_timestamp()
            WHERE user_id=$2 AND id=$3
            "#,
        )
        .bind(&multipart.upload_id)
        .bind(auth.user_id.0)
        .bind(upload_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if intent_updated != 1 {
            return Err(ApiError::Internal(
                "durable multipart upload intent disappeared before publication".to_owned(),
            ));
        }
        sqlx::query(
            r#"
        INSERT INTO straylight.asset_uploads (
          id,user_id,scope_id,credential_id,stable_import_id,path,media_type,
          import_metadata,describe_binary,expected_content_hash,expected_size_bytes,
          part_size_bytes,temporary_object_key,multipart_upload_id,status,
          expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'uploading',
          clock_timestamp()+make_interval(hours => $15::integer)
        )
        "#,
        )
        .bind(upload_id)
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(auth.credential_id.0)
        .bind(&stable_import_id)
        .bind(&request.path)
        .bind(&media_type)
        .bind(&import_metadata)
        .bind(request.describe_binary)
        .bind(&content_hash)
        .bind(i64::try_from(request.size_bytes).unwrap_or(i64::MAX))
        .bind(i32::try_from(PART_SIZE_BYTES).unwrap_or(i32::MAX))
        .bind(&multipart.object_key)
        .bind(&multipart.upload_id)
        .bind(UPLOAD_TTL_HOURS)
        .execute(&mut *tx)
        .await?;
        insert_audit(
            &mut tx,
            auth,
            scope_id,
            "asset.upload.create",
            json!({
                "upload_ref": format!("upload:{upload_id}"),
                "size_bytes": request.size_bytes,
                "part_size_bytes": PART_SIZE_BYTES,
                "part_count": part_count(request.size_bytes, PART_SIZE_BYTES)
            }),
        )
        .await?;
        sqlx::query("DELETE FROM straylight.asset_multipart_intents WHERE user_id=$1 AND id=$2")
            .bind(auth.user_id.0)
            .bind(upload_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = finalize {
        let aborted = state
            .object_store
            .abort_multipart_upload(&multipart.object_key, &multipart.upload_id)
            .await;
        if let Err(abort_error) = &aborted {
            tracing::warn!(
                ?abort_error,
                upload_id = %upload_id,
                "multipart upload compensation failed; reconciliation intent retained"
            );
        } else {
            clear_multipart_intent(state, auth, upload_id).await;
        }
        return Err(error);
    }
    metrics::counter!("asset.upload.sessions", "operation" => "create", "result" => "success")
        .increment(1);
    Ok(json!({
        "upload_ref": format!("upload:{upload_id}"),
        "status": "uploading",
        "part_size_bytes": PART_SIZE_BYTES,
        "part_count": part_count(request.size_bytes, PART_SIZE_BYTES),
        "size_bytes": request.size_bytes,
        "content_hash": format!("sha256:{content_hash}"),
        "expires_in_seconds": UPLOAD_TTL_HOURS * 60 * 60
    }))
}

pub async fn status(state: &AppState, auth: &AuthContext, upload_ref: &str) -> ApiResult<Value> {
    auth.require(Capability::Stage)?;
    let upload_id = parse_ref(upload_ref)?;
    let mut tx = state.begin_read(auth).await?;
    let upload = load_upload(&mut tx, auth, upload_id, false).await?;
    ensure_upload_owner(&upload, auth)?;
    let parts = sqlx::query(
        r#"
        SELECT part_number,size_bytes,content_hash
        FROM straylight.asset_upload_parts
        WHERE user_id=$1 AND upload_id=$2
        ORDER BY part_number
        "#,
    )
    .bind(auth.user_id.0)
    .bind(upload_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(json!({
        "upload_ref": format!("upload:{}", upload.id),
        "status": upload.status,
        "path": upload.path,
        "size_bytes": upload.expected_size_bytes,
        "content_hash": format!("sha256:{}", upload.expected_content_hash),
        "part_size_bytes": upload.part_size_bytes,
        "part_count": part_count(
            u64::try_from(upload.expected_size_bytes).unwrap_or_default(),
            u64::try_from(upload.part_size_bytes).unwrap_or_default()
        ),
        "uploaded_parts": parts.into_iter().map(|row| {
            Ok(json!({
                "part_number": row.try_get::<i32,_>("part_number")?,
                "size_bytes": row.try_get::<i32,_>("size_bytes")?,
                "content_hash": format!(
                    "sha256:{}",
                    row.try_get::<String,_>("content_hash")?
                )
            }))
        }).collect::<Result<Vec<_>,sqlx::Error>>()?,
        "stage_ref": upload.stage_id.map(|id| format!("stage:{id}")),
        "expires_at": upload.expires_at
    }))
}

pub async fn put_part(
    state: &AppState,
    auth: &AuthContext,
    upload_ref: &str,
    part_number: i32,
    headers: &HeaderMap,
    bytes: Bytes,
) -> ApiResult<Value> {
    auth.require(Capability::Stage)?;
    let upload_id = parse_ref(upload_ref)?;
    let declared_hash = headers
        .get("x-carrystate-part-sha256")
        .ok_or_else(|| ApiError::invalid("x-carrystate-part-sha256 is required"))?
        .to_str()
        .map_err(|_| ApiError::invalid("part SHA-256 header must contain valid ASCII"))
        .and_then(normalized_hash)?;
    let actual_hash = hex::encode(Sha256::digest(&bytes));
    if actual_hash != declared_hash {
        metrics::counter!(
            "asset.upload.integrity_failures",
            "failure" => "part_hash_mismatch"
        )
        .increment(1);
        return Err(ApiError::conflict(
            "part_hash_mismatch",
            "upload part bytes do not match their declared SHA-256",
            json!({"part_number": part_number}),
        ));
    }
    let mut tx = state.begin_write(auth).await?;
    lock_storage_write(&mut tx, auth.user_id.0).await?;
    let upload = load_upload(&mut tx, auth, upload_id, true).await?;
    ensure_upload_owner(&upload, auth)?;
    ensure_uploading(&upload)?;
    let expected_size = expected_part_size(&upload, part_number)?;
    if bytes.len() != expected_size {
        return Err(ApiError::conflict(
            "part_size_mismatch",
            "upload part size does not match the session manifest",
            json!({
                "part_number": part_number,
                "expected_size_bytes": expected_size,
                "received_size_bytes": bytes.len()
            }),
        ));
    }
    if let Some(row) = sqlx::query(
        r#"
        SELECT size_bytes,content_hash,etag
        FROM straylight.asset_upload_parts
        WHERE user_id=$1 AND upload_id=$2 AND part_number=$3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(upload_id)
    .bind(part_number)
    .fetch_optional(&mut *tx)
    .await?
    {
        let prior_size: i32 = row.try_get("size_bytes")?;
        let prior_hash: String = row.try_get("content_hash")?;
        if prior_size != i32::try_from(bytes.len()).unwrap_or(i32::MAX) || prior_hash != actual_hash
        {
            return Err(ApiError::conflict(
                "part_retry_mismatch",
                "a retried upload part must contain the same bytes",
                json!({"part_number": part_number}),
            ));
        }
        tx.commit().await?;
        return Ok(json!({
            "upload_ref": format!("upload:{upload_id}"),
            "part_number": part_number,
            "size_bytes": bytes.len(),
            "content_hash": format!("sha256:{actual_hash}"),
            "replayed": true
        }));
    }
    let uploaded = state
        .object_store
        .upload_multipart_part(
            &upload.temporary_object_key,
            &upload.multipart_upload_id,
            part_number,
            bytes,
        )
        .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.asset_upload_parts (
          user_id,scope_id,upload_id,part_number,size_bytes,content_hash,etag
        ) VALUES ($1,$2,$3,$4,$5,$6,$7)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(upload.scope_id)
    .bind(upload_id)
    .bind(part_number)
    .bind(i32::try_from(expected_size).unwrap_or(i32::MAX))
    .bind(&actual_hash)
    .bind(&uploaded.etag)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE straylight.asset_uploads SET updated_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(upload_id)
    .execute(&mut *tx)
    .await?;
    insert_audit(
        &mut tx,
        auth,
        upload.scope_id,
        "asset.upload.part",
        json!({
            "upload_ref": format!("upload:{upload_id}"),
            "part_number": part_number,
            "size_bytes": expected_size
        }),
    )
    .await?;
    tx.commit().await?;
    metrics::counter!("asset.upload.parts", "result" => "success").increment(1);
    metrics::histogram!("asset.upload.part_bytes").record(expected_size as f64);
    Ok(json!({
        "upload_ref": format!("upload:{upload_id}"),
        "part_number": part_number,
        "size_bytes": expected_size,
        "content_hash": format!("sha256:{actual_hash}"),
        "replayed": false
    }))
}

pub async fn complete(state: &AppState, auth: &AuthContext, upload_ref: &str) -> ApiResult<Value> {
    auth.require(Capability::Stage)?;
    let upload_id = parse_ref(upload_ref)?;
    let completion_token = Uuid::now_v7();
    let upload = {
        let mut tx = state.begin_write(auth).await?;
        lock_storage_write(&mut tx, auth.user_id.0).await?;
        let upload = load_upload(&mut tx, auth, upload_id, true).await?;
        ensure_upload_owner(&upload, auth)?;
        if upload.status == "consumed" {
            tx.commit().await?;
            return write_service::stage_status(
                state,
                auth,
                &format!(
                    "stage:{}",
                    upload.stage_id.ok_or_else(|| {
                        ApiError::Internal("consumed upload has no stage".to_owned())
                    })?
                ),
            )
            .await;
        }
        if upload.status == "completed" {
            if upload
                .completion_lease_expires_at
                .is_some_and(|expires_at| expires_at > Utc::now())
            {
                return Err(ApiError::conflict(
                    "upload_completion_in_progress",
                    "another request is already staging this upload",
                    json!({
                        "upload_ref": format!("upload:{upload_id}"),
                        "retry_after": upload.completion_lease_expires_at
                    }),
                ));
            }
            let claimed = sqlx::query(
                r#"
                UPDATE straylight.asset_uploads
                SET completion_token=$3,
                    completion_lease_expires_at=clock_timestamp()
                      +make_interval(secs => $4::integer),
                    updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2 AND status='completed'
                  AND (
                    completion_lease_expires_at IS NULL
                    OR completion_lease_expires_at <= clock_timestamp()
                  )
                "#,
            )
            .bind(auth.user_id.0)
            .bind(upload_id)
            .bind(completion_token)
            .bind(COMPLETION_LEASE_SECONDS)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if claimed != 1 {
                return Err(ApiError::conflict(
                    "upload_completion_in_progress",
                    "another request claimed this upload for staging",
                    json!({"upload_ref": format!("upload:{upload_id}")}),
                ));
            }
            tx.commit().await?;
            UploadRow {
                completion_token: Some(completion_token),
                ..upload
            }
        } else {
            if upload.status == "verifying"
                && upload
                    .completion_lease_expires_at
                    .is_some_and(|expires_at| expires_at > Utc::now())
            {
                return Err(ApiError::conflict(
                    "upload_completion_in_progress",
                    "another request is already verifying this upload",
                    json!({
                        "upload_ref": format!("upload:{upload_id}"),
                        "retry_after": upload.completion_lease_expires_at
                    }),
                ));
            }
            if upload.status != "verifying" {
                ensure_uploading(&upload)?;
            } else if upload.expires_at <= Utc::now() {
                return Err(ApiError::conflict(
                    "upload_expired",
                    "the resumable upload session has expired",
                    json!({"upload_ref": format!("upload:{upload_id}")}),
                ));
            }
            let part_rows = sqlx::query(
                r#"
                SELECT part_number,size_bytes,etag
                FROM straylight.asset_upload_parts
                WHERE user_id=$1 AND upload_id=$2
                ORDER BY part_number
                "#,
            )
            .bind(auth.user_id.0)
            .bind(upload_id)
            .fetch_all(&mut *tx)
            .await?;
            validate_complete_parts(&upload, &part_rows)?;
            let claimed = sqlx::query(
                r#"
                UPDATE straylight.asset_uploads
                SET status='verifying',completion_token=$3,
                    completion_lease_expires_at=clock_timestamp()
                      +make_interval(secs => $4::integer),
                    updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                  AND (
                    status='uploading'
                    OR (
                      status='verifying'
                      AND completion_lease_expires_at <= clock_timestamp()
                    )
                  )
                "#,
            )
            .bind(auth.user_id.0)
            .bind(upload_id)
            .bind(completion_token)
            .bind(COMPLETION_LEASE_SECONDS)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if claimed != 1 {
                return Err(ApiError::conflict(
                    "upload_completion_in_progress",
                    "another request claimed this upload for verification",
                    json!({"upload_ref": format!("upload:{upload_id}")}),
                ));
            }
            tx.commit().await?;
            let parts = part_rows
                .iter()
                .map(|row| {
                    Ok((
                        row.try_get::<i32, _>("part_number")?,
                        row.try_get::<String, _>("etag")?,
                    ))
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?;
            let mut promote_tx = state.begin_write(auth).await?;
            lock_storage_write(&mut promote_tx, auth.user_id.0).await?;
            let completion = state
                .object_store
                .complete_multipart_upload(
                    &upload.temporary_object_key,
                    &upload.multipart_upload_id,
                    &parts,
                )
                .await;
            let mut completed_temporary_version_id = completion
                .as_ref()
                .ok()
                .and_then(|version_id| version_id.clone())
                .or_else(|| upload.temporary_object_version_id.clone());
            if completed_temporary_version_id.is_none()
                && let Ok(version_id) = state
                    .object_store
                    .resolve_object_version_id(&upload.temporary_object_key, None)
                    .await
            {
                completed_temporary_version_id = version_id;
            }
            let locked_upload = load_upload(&mut promote_tx, auth, upload_id, true).await?;
            ensure_upload_owner(&locked_upload, auth)?;
            if !completion_lease_owned(&locked_upload, completion_token) {
                promote_tx.rollback().await?;
                return Err(ApiError::conflict(
                    "upload_completion_lost",
                    "the upload completion lease changed before canonical promotion",
                    json!({"upload_ref": format!("upload:{upload_id}")}),
                ));
            }
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!(
                    "asset-upload:{}:{}",
                    auth.user_id.0, locked_upload.expected_content_hash
                ))
                .execute(&mut *promote_tx)
                .await?;
            let promoted = state
                .object_store
                .promote_verified_upload(
                    auth.user_id,
                    &locked_upload.temporary_object_key,
                    completed_temporary_version_id.as_deref(),
                    &locked_upload.expected_content_hash,
                    u64::try_from(locked_upload.expected_size_bytes).unwrap_or_default(),
                )
                .await;
            let promoted = match (completion, promoted) {
                (_, Ok(value)) => value,
                (_, Err(integrity_error)) if is_content_hash_mismatch(&integrity_error) => {
                    sqlx::query(
                        r#"
                        UPDATE straylight.asset_uploads
                        SET status='failed',updated_at=clock_timestamp(),
                            failure_code='content_hash_mismatch',
                            completion_token=NULL,completion_lease_expires_at=NULL
                        WHERE user_id=$1 AND id=$2 AND completion_token=$3
                        "#,
                    )
                    .bind(auth.user_id.0)
                    .bind(upload_id)
                    .bind(completion_token)
                    .execute(&mut *promote_tx)
                    .await?;
                    enqueue_temporary_cleanup(
                        &mut promote_tx,
                        auth.user_id.0,
                        locked_upload.scope_id,
                        &locked_upload,
                        "content_hash_mismatch",
                    )
                    .await?;
                    promote_tx.commit().await?;
                    compensate_promoted_object(
                        state,
                        auth,
                        &canonical_key(auth.user_id.0, &locked_upload.expected_content_hash),
                        None,
                        upload_id,
                    )
                    .await;
                    return Err(integrity_error);
                }
                (Err(_), Err(storage_error)) if completed_temporary_version_id.is_some() => {
                    sqlx::query(
                        r#"
                        UPDATE straylight.asset_uploads
                        SET status='verifying',updated_at=clock_timestamp(),
                            failure_code='promotion_retryable',
                            completion_lease_expires_at=clock_timestamp()+interval '5 seconds',
                            temporary_object_version_id=$4
                        WHERE user_id=$1 AND id=$2 AND completion_token=$3
                        "#,
                    )
                    .bind(auth.user_id.0)
                    .bind(upload_id)
                    .bind(completion_token)
                    .bind(&completed_temporary_version_id)
                    .execute(&mut *promote_tx)
                    .await?;
                    promote_tx.commit().await?;
                    compensate_promoted_object(
                        state,
                        auth,
                        &canonical_key(auth.user_id.0, &locked_upload.expected_content_hash),
                        None,
                        upload_id,
                    )
                    .await;
                    return Err(storage_error);
                }
                (Err(completion_error), Err(_)) => {
                    sqlx::query(
                        r#"
                        UPDATE straylight.asset_uploads
                        SET status='uploading',updated_at=clock_timestamp(),
                            failure_code='multipart_complete_failed',
                            completion_token=NULL,completion_lease_expires_at=NULL
                        WHERE user_id=$1 AND id=$2 AND status='verifying'
                          AND completion_token=$3
                        "#,
                    )
                    .bind(auth.user_id.0)
                    .bind(upload_id)
                    .bind(completion_token)
                    .execute(&mut *promote_tx)
                    .await?;
                    promote_tx.commit().await?;
                    return Err(completion_error);
                }
                (Ok(_), Err(storage_error)) => {
                    sqlx::query(
                        r#"
                        UPDATE straylight.asset_uploads
                        SET status='verifying',updated_at=clock_timestamp(),
                            failure_code='promotion_retryable',
                            completion_lease_expires_at=clock_timestamp()+interval '5 seconds',
                            temporary_object_version_id=coalesce($4,temporary_object_version_id)
                        WHERE user_id=$1 AND id=$2 AND completion_token=$3
                        "#,
                    )
                    .bind(auth.user_id.0)
                    .bind(upload_id)
                    .bind(completion_token)
                    .bind(&completed_temporary_version_id)
                    .execute(&mut *promote_tx)
                    .await?;
                    promote_tx.commit().await?;
                    compensate_promoted_object(
                        state,
                        auth,
                        &canonical_key(auth.user_id.0, &locked_upload.expected_content_hash),
                        None,
                        upload_id,
                    )
                    .await;
                    return Err(storage_error);
                }
            };
            let promoted_update = match sqlx::query(
                r#"
                UPDATE straylight.asset_uploads
                SET status='completed',canonical_object_key=$1,
                    canonical_object_version_id=$2,
                    temporary_object_version_id=$3,
                    completed_at=clock_timestamp(),updated_at=clock_timestamp(),
                    failure_code=NULL,completion_token=$6,
                    completion_lease_expires_at=clock_timestamp()
                      +make_interval(secs => $7::integer)
                WHERE user_id=$4 AND id=$5 AND status='verifying'
                  AND completion_token=$6
                "#,
            )
            .bind(&promoted.file.object_key)
            .bind(&promoted.file.object_version_id)
            .bind(&promoted.temporary_object_version_id)
            .bind(auth.user_id.0)
            .bind(upload_id)
            .bind(completion_token)
            .bind(COMPLETION_LEASE_SECONDS)
            .execute(&mut *promote_tx)
            .await
            {
                Ok(result) => result.rows_affected(),
                Err(error) => {
                    let _ = promote_tx.rollback().await;
                    compensate_promoted_object(
                        state,
                        auth,
                        &promoted.file.object_key,
                        promoted.file.object_version_id.as_deref(),
                        upload_id,
                    )
                    .await;
                    return Err(ApiError::Database(error));
                }
            };
            if promoted_update != 1 {
                promote_tx.rollback().await?;
                compensate_promoted_object(
                    state,
                    auth,
                    &promoted.file.object_key,
                    promoted.file.object_version_id.as_deref(),
                    upload_id,
                )
                .await;
                return Err(ApiError::conflict(
                    "upload_completion_lost",
                    "the upload completion lease changed before it could commit",
                    json!({"upload_ref": format!("upload:{upload_id}")}),
                ));
            }
            if let Err(error) = enqueue_temporary_cleanup(
                &mut promote_tx,
                auth.user_id.0,
                locked_upload.scope_id,
                &locked_upload,
                "upload_completed",
            )
            .await
            {
                let _ = promote_tx.rollback().await;
                compensate_promoted_object(
                    state,
                    auth,
                    &promoted.file.object_key,
                    promoted.file.object_version_id.as_deref(),
                    upload_id,
                )
                .await;
                return Err(error);
            }
            if let Err(error) = promote_tx.commit().await {
                compensate_promoted_object(
                    state,
                    auth,
                    &promoted.file.object_key,
                    promoted.file.object_version_id.as_deref(),
                    upload_id,
                )
                .await;
                return Err(ApiError::Database(error));
            }
            if let Err(error) = state
                .object_store
                .purge_all_versions(&locked_upload.temporary_object_key)
                .await
            {
                tracing::warn!(
                    ?error,
                    upload_id = %locked_upload.id,
                    "completed resumable upload temporary object cleanup failed"
                );
            } else if let Err(error) =
                mark_temporary_upload_cleaned(state, auth, locked_upload.id).await
            {
                tracing::warn!(
                    ?error,
                    upload_id = %locked_upload.id,
                    "temporary upload cleanup marker could not be persisted"
                );
            }
            UploadRow {
                temporary_object_version_id: promoted.temporary_object_version_id,
                canonical_object_key: Some(promoted.file.object_key),
                canonical_object_version_id: promoted.file.object_version_id,
                status: "completed".to_owned(),
                completion_token: Some(completion_token),
                completion_lease_expires_at: None,
                ..locked_upload
            }
        }
    };
    let canonical_object_key = upload
        .canonical_object_key
        .clone()
        .ok_or_else(|| ApiError::Internal("completed upload has no object key".to_owned()))?;
    let stage = match write_service::stage_uploads(
        state,
        auth,
        &scope_ref(state, auth, upload.scope_id).await?,
        upload.stable_import_id.clone(),
        upload.describe_binary,
        vec![StageUpload {
            path: upload.path.clone(),
            media_type: Some(upload.media_type.clone()),
            import_metadata: upload.import_metadata.clone(),
            content: StageUploadContent::Stored {
                object_key: canonical_object_key,
                object_version_id: upload.canonical_object_version_id.clone(),
                content_hash: upload.expected_content_hash.clone(),
                size_bytes: u64::try_from(upload.expected_size_bytes).unwrap_or_default(),
            },
        }],
    )
    .await
    {
        Ok(stage) => stage,
        Err(error) => {
            release_completed_finalization_lease(state, auth, upload_id, completion_token).await;
            return Err(error);
        }
    };
    let stage_id = stage
        .get("stage_ref")
        .and_then(Value::as_str)
        .map(parse_stage_ref)
        .transpose()?
        .ok_or_else(|| ApiError::Internal("upload stage returned no stage reference".to_owned()))?;
    let mut tx = state.begin_write(auth).await?;
    lock_storage_write(&mut tx, auth.user_id.0).await?;
    let completion_update = sqlx::query(
        r#"
        UPDATE straylight.asset_uploads
        SET status='consumed',stage_id=$1,consumed_at=clock_timestamp(),
            updated_at=clock_timestamp(),failure_code=NULL,
            completion_token=NULL,completion_lease_expires_at=NULL
        WHERE user_id=$2 AND id=$3 AND status='completed'
          AND completion_token=$4
        "#,
    )
    .bind(stage_id)
    .bind(auth.user_id.0)
    .bind(upload_id)
    .bind(completion_token)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if completion_update != 1 {
        tx.rollback().await?;
        return Err(ApiError::conflict(
            "upload_completion_lost",
            "the upload completion lease changed before staging could be finalized",
            json!({
                "upload_ref": format!("upload:{upload_id}"),
                "stage_ref": format!("stage:{stage_id}")
            }),
        ));
    }
    insert_audit(
        &mut tx,
        auth,
        upload.scope_id,
        "asset.upload.complete",
        json!({
            "upload_ref": format!("upload:{upload_id}"),
            "stage_ref": format!("stage:{stage_id}"),
            "size_bytes": upload.expected_size_bytes
        }),
    )
    .await?;
    tx.commit().await?;
    metrics::counter!("asset.upload.sessions", "operation" => "complete", "result" => "success")
        .increment(1);
    Ok(stage)
}

async fn enqueue_temporary_cleanup(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    upload: &UploadRow,
    reason: &str,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.background_jobs (
          user_id,scope_id,job_kind,status,payload,max_attempts
        ) VALUES ($1,$2,'asset_object_cleanup','queued',$3,10)
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(json!({
        "object_key": upload.temporary_object_key,
        "upload_id": upload.id,
        "multipart_upload_id": upload.multipart_upload_id,
        "temporary_upload": true,
        "reason": reason
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_temporary_upload_cleaned(
    state: &AppState,
    auth: &AuthContext,
    upload_id: Uuid,
) -> ApiResult<()> {
    let mut tx = state.begin_write(auth).await?;
    sqlx::query(
        r#"
        UPDATE straylight.asset_uploads
        SET temporary_cleaned_at=coalesce(temporary_cleaned_at,clock_timestamp()),
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(upload_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn release_completed_finalization_lease(
    state: &AppState,
    auth: &AuthContext,
    upload_id: Uuid,
    completion_token: Uuid,
) {
    let release = async {
        let mut tx = state.begin_write(auth).await?;
        lock_storage_write(&mut tx, auth.user_id.0).await?;
        sqlx::query(
            r#"
            UPDATE straylight.asset_uploads
            SET completion_token=NULL,completion_lease_expires_at=NULL,
                updated_at=clock_timestamp()
            WHERE user_id=$1 AND id=$2 AND status='completed'
              AND completion_token=$3
            "#,
        )
        .bind(auth.user_id.0)
        .bind(upload_id)
        .bind(completion_token)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = release {
        tracing::warn!(
            ?error,
            %upload_id,
            "could not release failed upload staging lease"
        );
    }
}

pub async fn abort(state: &AppState, auth: &AuthContext, upload_ref: &str) -> ApiResult<Value> {
    auth.require(Capability::Stage)?;
    let upload_id = parse_ref(upload_ref)?;
    let mut tx = state.begin_write(auth).await?;
    lock_storage_write(&mut tx, auth.user_id.0).await?;
    let upload = load_upload(&mut tx, auth, upload_id, true).await?;
    ensure_upload_owner(&upload, auth)?;
    if matches!(upload.status.as_str(), "consumed" | "completed") {
        return Err(ApiError::conflict(
            "upload_already_completed",
            "completed uploads cannot be aborted",
            json!({"upload_ref": format!("upload:{upload_id}")}),
        ));
    }
    if !matches!(upload.status.as_str(), "aborted" | "expired") {
        sqlx::query(
            r#"
            UPDATE straylight.asset_uploads
            SET status='aborted',updated_at=clock_timestamp(),
                completion_token=NULL,completion_lease_expires_at=NULL
            WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(upload_id)
        .execute(&mut *tx)
        .await?;
    }
    enqueue_temporary_cleanup(
        &mut tx,
        auth.user_id.0,
        upload.scope_id,
        &upload,
        "upload_aborted",
    )
    .await?;
    insert_audit(
        &mut tx,
        auth,
        upload.scope_id,
        "asset.upload.abort",
        json!({"upload_ref": format!("upload:{upload_id}")}),
    )
    .await?;
    tx.commit().await?;
    metrics::counter!("asset.upload.sessions", "operation" => "abort", "result" => "success")
        .increment(1);
    Ok(json!({
        "upload_ref": format!("upload:{upload_id}"),
        "status": "aborted"
    }))
}

pub async fn expire_one(state: &AppState) -> ApiResult<bool> {
    if reconcile_one_orphan_multipart(state).await? {
        return Ok(true);
    }
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut tx = pool.begin().await?;
    let candidate = sqlx::query(
        r#"
        SELECT id,user_id
        FROM straylight.asset_uploads
        WHERE status IN ('uploading','verifying','completed','failed')
          AND expires_at <= clock_timestamp()
          AND (
            status <> 'completed'
            OR completion_lease_expires_at IS NULL
            OR completion_lease_expires_at <= clock_timestamp()
          )
        ORDER BY expires_at,id
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(candidate) = candidate else {
        tx.rollback().await?;
        return Ok(false);
    };
    let upload_id: Uuid = candidate.try_get("id")?;
    let user_id: Uuid = candidate.try_get("user_id")?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query(
        r#"
        SELECT id,user_id,scope_id,status,temporary_object_key,
               temporary_object_version_id,canonical_object_key,
               canonical_object_version_id,
               multipart_upload_id,expected_content_hash,credential_id,
               stable_import_id,path,media_type,import_metadata,describe_binary,
               expected_size_bytes,part_size_bytes,stage_id,completion_token,
               completion_lease_expires_at,expires_at
        FROM straylight.asset_uploads
        WHERE user_id=$1 AND id=$2
          AND status IN ('uploading','verifying','completed','failed')
          AND expires_at <= clock_timestamp()
          AND (
            status <> 'completed'
            OR completion_lease_expires_at IS NULL
            OR completion_lease_expires_at <= clock_timestamp()
          )
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(upload_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(true);
    };
    let scope_id: Uuid = row.try_get("scope_id")?;
    let status: String = row.try_get("status")?;
    let object_key: String = row.try_get("temporary_object_key")?;
    let canonical_object_key: Option<String> = row.try_get("canonical_object_key")?;
    let multipart_upload_id: String = row.try_get("multipart_upload_id")?;
    let expected_content_hash: String = row.try_get("expected_content_hash")?;
    if status == "completed" {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("asset-upload:{user_id}:{expected_content_hash}"))
            .execute(&mut *tx)
            .await?;
        let canonical_object_key = canonical_object_key.clone().ok_or_else(|| {
            ApiError::Internal("completed upload has no canonical object key".to_owned())
        })?;
        let referenced = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM straylight.asset_versions
              WHERE user_id=$1 AND object_key=$2
              UNION ALL
              SELECT 1
              FROM straylight.asset_uploads
              WHERE user_id=$1 AND id<>$3 AND canonical_object_key=$2
                AND status IN ('completed','consumed')
            )
            "#,
        )
        .bind(user_id)
        .bind(&canonical_object_key)
        .bind(upload_id)
        .fetch_one(&mut *tx)
        .await?;
        if !referenced {
            sqlx::query(
                r#"
                INSERT INTO straylight.background_jobs (
                  user_id,scope_id,job_kind,status,payload,max_attempts
                ) VALUES ($1,$2,'asset_object_cleanup','queued',$3,10)
                "#,
            )
            .bind(user_id)
            .bind(scope_id)
            .bind(json!({
                "object_key": canonical_object_key,
                "reason": "completed_upload_expired"
            }))
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        r#"
        UPDATE straylight.asset_uploads
        SET status='expired',updated_at=clock_timestamp(),
            failure_code='upload_expired',
            completion_token=NULL,completion_lease_expires_at=NULL
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(user_id)
    .bind(upload_id)
    .execute(&mut *tx)
    .await?;
    let cleanup_upload = UploadRow {
        id: upload_id,
        scope_id,
        credential_id: row.try_get("credential_id")?,
        stable_import_id: row.try_get("stable_import_id")?,
        path: row.try_get("path")?,
        media_type: row.try_get("media_type")?,
        import_metadata: row.try_get("import_metadata")?,
        describe_binary: row.try_get("describe_binary")?,
        expected_content_hash: expected_content_hash.clone(),
        expected_size_bytes: row.try_get("expected_size_bytes")?,
        part_size_bytes: row.try_get("part_size_bytes")?,
        temporary_object_key: object_key,
        temporary_object_version_id: row.try_get("temporary_object_version_id")?,
        canonical_object_key,
        canonical_object_version_id: row.try_get("canonical_object_version_id")?,
        multipart_upload_id,
        stage_id: row.try_get("stage_id")?,
        status: "expired".to_owned(),
        completion_token: None,
        completion_lease_expires_at: None,
        expires_at: row.try_get("expires_at")?,
    };
    enqueue_temporary_cleanup(
        &mut tx,
        user_id,
        scope_id,
        &cleanup_upload,
        "upload_expired",
    )
    .await?;
    tx.commit().await?;
    metrics::counter!("asset.upload.sessions", "operation" => "expire", "result" => "success")
        .increment(1);
    Ok(true)
}

pub async fn expire_one_stage(state: &AppState) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut tx = pool.begin().await?;
    let candidate = sqlx::query(
        r#"
        SELECT id,user_id,scope_id
        FROM straylight.stages
        WHERE status IN ('uploading','inspecting','ready','quarantined','failed')
          AND (
            expires_at <= clock_timestamp()
            OR status IN ('quarantined','failed')
          )
        ORDER BY
          CASE WHEN status IN ('quarantined','failed') THEN 0 ELSE 1 END,
          expires_at,id
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(candidate) = candidate else {
        tx.rollback().await?;
        return Ok(false);
    };
    let stage_id: Uuid = candidate.try_get("id")?;
    let user_id: Uuid = candidate.try_get("user_id")?;
    let scope_id: Uuid = candidate.try_get("scope_id")?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let locked = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM straylight.stages
        WHERE user_id=$1 AND id=$2
          AND status IN ('uploading','inspecting','ready','quarantined','failed')
          AND (
            expires_at <= clock_timestamp()
            OR status IN ('quarantined','failed')
          )
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(stage_id)
    .fetch_optional(&mut *tx)
    .await?;
    if locked.is_none() {
        tx.rollback().await?;
        return Ok(true);
    }
    let object_keys = sqlx::query_scalar::<_, String>(
        "SELECT reclaimed_object_key FROM straylight.expire_unpromoted_stage($1)",
    )
    .bind(stage_id)
    .fetch_all(&mut *tx)
    .await?;
    for object_key in &object_keys {
        sqlx::query(
            r#"
            INSERT INTO straylight.background_jobs (
              user_id,scope_id,job_kind,status,payload,max_attempts
            ) VALUES ($1,$2,'asset_object_cleanup','queued',$3,10)
            "#,
        )
        .bind(user_id)
        .bind(scope_id)
        .bind(json!({
            "object_key": object_key,
            "reason": "stage_expired",
            "stage_id": stage_id
        }))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    metrics::counter!("asset.stage.expiry", "result" => "success").increment(1);
    metrics::histogram!("asset.stage.expiry.objects", "result" => "queued")
        .record(object_keys.len() as f64);
    Ok(true)
}

pub async fn cleanup_object_job(
    state: &AppState,
    user_id: Uuid,
    payload: &Value,
) -> ApiResult<Value> {
    let object_key = payload
        .get("object_key")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("asset cleanup job omitted object_key"))?;
    let temporary_upload = payload
        .get("temporary_upload")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let upload_id = payload
        .get("upload_id")
        .and_then(Value::as_str)
        .map(|value| {
            Uuid::parse_str(value.trim_start_matches("upload:"))
                .map_err(|_| ApiError::invalid("asset cleanup job has an invalid upload_id"))
        })
        .transpose()?;
    validate_cleanup_object_key(user_id, object_key, temporary_upload, upload_id)?;
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    if temporary_upload {
        let upload_id =
            upload_id.ok_or_else(|| ApiError::invalid("temporary cleanup omitted upload_id"))?;
        let row = sqlx::query(
            r#"
            SELECT status,temporary_object_key,multipart_upload_id,temporary_cleaned_at
            FROM straylight.asset_uploads
            WHERE user_id=$1 AND id=$2
            FOR UPDATE
            "#,
        )
        .bind(user_id)
        .bind(upload_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("upload_not_found", &format!("upload:{upload_id}")))?;
        let stored_key: String = row.try_get("temporary_object_key")?;
        if stored_key != object_key {
            return Err(ApiError::invalid(
                "temporary cleanup key does not match its upload",
            ));
        }
        let status: String = row.try_get("status")?;
        if !matches!(
            status.as_str(),
            "completed" | "consumed" | "failed" | "aborted" | "expired"
        ) {
            tx.commit().await?;
            return Ok(json!({
                "object_key": object_key,
                "status": "retained",
                "upload_status": status
            }));
        }
        if row
            .try_get::<Option<DateTime<Utc>>, _>("temporary_cleaned_at")?
            .is_some()
        {
            tx.commit().await?;
            return Ok(json!({"object_key": object_key, "status": "already_purged"}));
        }
        let multipart_upload_id: String = row.try_get("multipart_upload_id")?;
        state
            .object_store
            .abort_multipart_upload(object_key, &multipart_upload_id)
            .await?;
        let purge = state.object_store.purge_all_versions(object_key).await?;
        sqlx::query(
            r#"
            UPDATE straylight.asset_uploads
            SET temporary_cleaned_at=clock_timestamp(),updated_at=clock_timestamp()
            WHERE user_id=$1 AND id=$2 AND temporary_cleaned_at IS NULL
            "#,
        )
        .bind(user_id)
        .bind(upload_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        metrics::counter!("asset.object.cleanup", "result" => "purged_temporary").increment(1);
        return Ok(json!({
            "object_key": object_key,
            "status": "purged",
            "versions_deleted": purge.versions_deleted,
            "delete_markers_deleted": purge.delete_markers_deleted
        }));
    }
    let referenced = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM straylight.asset_versions
          WHERE user_id=$1 AND object_key=$2
          UNION ALL
          SELECT 1 FROM straylight.asset_uploads
          WHERE user_id=$1 AND canonical_object_key=$2
            AND status IN ('completed','consumed')
        )
        "#,
    )
    .bind(user_id)
    .bind(object_key)
    .fetch_one(&mut *tx)
    .await?;
    if referenced {
        tx.commit().await?;
        metrics::counter!("asset.object.cleanup", "result" => "retained").increment(1);
        return Ok(json!({"object_key": object_key, "status": "retained"}));
    }
    let purge = state.object_store.purge_all_versions(object_key).await?;
    tx.commit().await?;
    metrics::counter!("asset.object.cleanup", "result" => "purged").increment(1);
    Ok(json!({
        "object_key": object_key,
        "status": "purged",
        "versions_deleted": purge.versions_deleted,
        "delete_markers_deleted": purge.delete_markers_deleted
    }))
}

fn validate_cleanup_object_key(
    user_id: Uuid,
    object_key: &str,
    temporary_upload: bool,
    upload_id: Option<Uuid>,
) -> ApiResult<()> {
    if temporary_upload {
        let expected = upload_id
            .map(|upload_id| format!("{user_id}/uploads/{upload_id}"))
            .ok_or_else(|| ApiError::invalid("temporary cleanup omitted upload_id"))?;
        if object_key != expected {
            return Err(ApiError::invalid(
                "temporary cleanup key is outside its exact upload path",
            ));
        }
        return Ok(());
    }
    let digest = object_key
        .strip_prefix(&format!("{user_id}/blobs/"))
        .ok_or_else(|| ApiError::invalid("asset cleanup key is outside the user blob prefix"))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ApiError::invalid(
            "asset cleanup key does not contain a lowercase SHA-256 digest",
        ));
    }
    Ok(())
}

pub async fn abort_active_for_user(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<u64> {
    let rows = sqlx::query(
        r#"
        SELECT id,temporary_object_key,multipart_upload_id
        FROM straylight.asset_uploads
        WHERE user_id=$1 AND status IN ('uploading','verifying')
        ORDER BY created_at,id
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut aborted = 0_u64;
    for row in rows {
        let upload_id: Uuid = row.try_get("id")?;
        let object_key: String = row.try_get("temporary_object_key")?;
        let multipart_upload_id: String = row.try_get("multipart_upload_id")?;
        state
            .object_store
            .abort_multipart_upload(&object_key, &multipart_upload_id)
            .await?;
        let updated = sqlx::query(
            r#"
            UPDATE straylight.asset_uploads
            SET status='aborted',updated_at=clock_timestamp(),
                failure_code='account_deletion',
                completion_token=NULL,completion_lease_expires_at=NULL
            WHERE user_id=$1 AND id=$2
              AND status IN ('uploading','verifying')
            "#,
        )
        .bind(user_id)
        .bind(upload_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        aborted = aborted.saturating_add(updated);
    }
    Ok(aborted)
}

pub async fn abort_multipart_prefix(state: &AppState, prefix: &str) -> ApiResult<usize> {
    validate_user_prefix(prefix)?;
    let uploads = list_multipart_uploads(state, Some(prefix)).await?;
    for upload in &uploads {
        state
            .object_store
            .abort_multipart_upload(&upload.object_key, &upload.upload_id)
            .await?;
    }
    let remaining = list_multipart_uploads(state, Some(prefix)).await?;
    if !remaining.is_empty() {
        return Err(ApiError::Internal(format!(
            "{} multipart uploads remain under the purged account prefix",
            remaining.len()
        )));
    }
    Ok(uploads.len())
}

async fn reconcile_one_orphan_multipart(state: &AppState) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    for candidate in list_multipart_uploads(state, None).await? {
        let Some((user_id, object_id)) = parse_managed_multipart_key(&candidate.object_key) else {
            continue;
        };
        let mut tx = pool.begin().await?;
        lock_storage(&mut tx, user_id).await?;
        let known = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT
              EXISTS(
                SELECT 1
                FROM straylight.asset_uploads
                WHERE user_id=$1
                  AND temporary_object_key=$2
                  AND multipart_upload_id=$3
                  AND status IN ('uploading','verifying')
              )
              OR EXISTS(
                SELECT 1
                FROM straylight.asset_multipart_intents
                WHERE user_id=$1 AND object_key=$2
                  AND (
                    multipart_upload_id IS NULL
                    OR multipart_upload_id=$3
                  )
                  AND expires_at > clock_timestamp()
              )
              OR EXISTS(
                SELECT 1
                FROM straylight.account_exports
                WHERE user_id=$1 AND id=$4 AND status='running'
              )
            "#,
        )
        .bind(user_id)
        .bind(&candidate.object_key)
        .bind(&candidate.upload_id)
        .bind(object_id)
        .fetch_one(&mut *tx)
        .await?;
        if known {
            tx.commit().await?;
            continue;
        }
        state
            .object_store
            .abort_multipart_upload(&candidate.object_key, &candidate.upload_id)
            .await?;
        sqlx::query(
            "DELETE FROM straylight.asset_multipart_intents WHERE user_id=$1 AND object_key=$2",
        )
        .bind(user_id)
        .bind(&candidate.object_key)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        metrics::counter!("asset.upload.multipart_reconciliation", "result" => "aborted")
            .increment(1);
        return Ok(true);
    }

    let removed = sqlx::query_scalar::<_, Uuid>(
        r#"
        DELETE FROM straylight.asset_multipart_intents
        WHERE id=(
          SELECT intent.id
          FROM straylight.asset_multipart_intents AS intent
          WHERE intent.expires_at <= clock_timestamp()
            AND NOT EXISTS (
              SELECT 1
              FROM straylight.asset_uploads AS upload
              WHERE upload.user_id=intent.user_id
                AND upload.id=intent.id
            )
          ORDER BY intent.expires_at,intent.id
          FOR UPDATE SKIP LOCKED
          LIMIT 1
        )
        RETURNING id
        "#,
    )
    .fetch_optional(pool)
    .await?;
    if removed.is_some() {
        metrics::counter!("asset.upload.multipart_reconciliation", "result" => "intent_cleaned")
            .increment(1);
        return Ok(true);
    }
    Ok(false)
}

#[derive(Clone, Debug)]
struct MultipartInventoryEntry {
    object_key: String,
    upload_id: String,
}

async fn list_multipart_uploads(
    state: &AppState,
    prefix: Option<&str>,
) -> ApiResult<Vec<MultipartInventoryEntry>> {
    let client = multipart_inventory_client(&state.config).await;
    let mut uploads = Vec::new();
    let mut key_marker = None;
    let mut upload_id_marker = None;
    loop {
        let mut request = client
            .list_multipart_uploads()
            .bucket(&state.config.s3_bucket);
        if let Some(prefix) = prefix {
            request = request.prefix(prefix);
        }
        if let Some(marker) = key_marker.as_deref() {
            request = request.key_marker(marker);
        }
        if let Some(marker) = upload_id_marker.as_deref() {
            request = request.upload_id_marker(marker);
        }
        let output = request.send().await.map_err(|error| {
            ApiError::Internal(format!("multipart upload inventory failed: {error}"))
        })?;
        for upload in output.uploads() {
            let object_key = upload.key().ok_or_else(|| {
                ApiError::Internal("multipart upload inventory omitted an object key".to_owned())
            })?;
            let upload_id = upload.upload_id().ok_or_else(|| {
                ApiError::Internal("multipart upload inventory omitted an upload ID".to_owned())
            })?;
            uploads.push(MultipartInventoryEntry {
                object_key: object_key.to_owned(),
                upload_id: upload_id.to_owned(),
            });
        }
        if !output.is_truncated().unwrap_or(false) {
            break;
        }
        key_marker = output.next_key_marker().map(ToOwned::to_owned);
        upload_id_marker = output.next_upload_id_marker().map(ToOwned::to_owned);
        if key_marker.is_none() {
            return Err(ApiError::Internal(
                "truncated multipart upload inventory omitted its next key marker".to_owned(),
            ));
        }
    }
    Ok(uploads)
}

async fn multipart_inventory_client(config: &Config) -> &'static Client {
    MULTIPART_INVENTORY_CLIENT
        .get_or_init(|| async {
            let mut loader = aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(config.s3_region.clone()));
            if let (Some(access_key), Some(secret_key)) =
                (&config.s3_access_key, &config.s3_secret_key)
            {
                let credentials = Credentials::new(
                    access_key.clone(),
                    secret_key.clone(),
                    None,
                    None,
                    "straylight-multipart-reconciliation",
                );
                loader = loader.credentials_provider(SharedCredentialsProvider::new(credentials));
            }
            let shared = loader.load().await;
            let mut s3_config = aws_sdk_s3::config::Builder::from(&shared)
                .force_path_style(config.s3_force_path_style);
            if let Some(endpoint) = &config.s3_endpoint {
                s3_config = s3_config.endpoint_url(endpoint);
            }
            Client::from_conf(s3_config.build())
        })
        .await
}

fn parse_managed_multipart_key(object_key: &str) -> Option<(Uuid, Uuid)> {
    let mut parts = object_key.split('/');
    let user_id = Uuid::parse_str(parts.next()?).ok()?;
    if parts.next()? != "uploads" {
        return None;
    }
    let object_id = Uuid::parse_str(parts.next()?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((user_id, object_id))
}

fn validate_user_prefix(prefix: &str) -> ApiResult<Uuid> {
    let user = prefix
        .strip_suffix('/')
        .ok_or_else(|| ApiError::invalid("account object prefix must end in a slash"))?;
    Uuid::parse_str(user)
        .map_err(|_| ApiError::invalid("account object prefix must contain one user UUID"))
}

async fn lock_storage(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_storage_write(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> ApiResult<()> {
    sqlx::query("SELECT straylight.assert_storage_write_allowed($1)")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn completion_lease_owned(upload: &UploadRow, completion_token: Uuid) -> bool {
    upload.status == "verifying"
        && upload.completion_token == Some(completion_token)
        && upload
            .completion_lease_expires_at
            .is_some_and(|expires_at| expires_at > Utc::now())
}

fn canonical_key(user_id: Uuid, content_hash: &str) -> String {
    format!(
        "{user_id}/blobs/{}",
        content_hash.trim_start_matches("sha256:")
    )
}

async fn clear_multipart_intent(state: &AppState, auth: &AuthContext, upload_id: Uuid) {
    let result = async {
        let mut tx = if let Some(pool) = state.admin_pool.as_ref() {
            pool.begin().await?
        } else {
            state.begin_write(auth).await?
        };
        lock_storage(&mut tx, auth.user_id.0).await?;
        sqlx::query("DELETE FROM straylight.asset_multipart_intents WHERE user_id=$1 AND id=$2")
            .bind(auth.user_id.0)
            .bind(upload_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(
            ?error,
            upload_id = %upload_id,
            "multipart reconciliation intent cleanup failed"
        );
    }
}

async fn compensate_promoted_object(
    state: &AppState,
    auth: &AuthContext,
    object_key: &str,
    object_version_id: Option<&str>,
    upload_id: Uuid,
) {
    let result = async {
        let mut tx = if let Some(pool) = state.admin_pool.as_ref() {
            pool.begin().await?
        } else {
            state.begin_write(auth).await?
        };
        lock_storage(&mut tx, auth.user_id.0).await?;
        let referenced = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM straylight.asset_versions
              WHERE user_id=$1 AND object_key=$2
              UNION ALL
              SELECT 1
              FROM straylight.asset_uploads
              WHERE user_id=$1 AND canonical_object_key=$2
                AND status IN ('completed','consumed')
            )
            "#,
        )
        .bind(auth.user_id.0)
        .bind(object_key)
        .fetch_one(&mut *tx)
        .await?;
        let exact_version_id =
            object_version_id.filter(|version_id| !version_id.is_empty() && *version_id != "null");
        if !referenced {
            if let Some(version_id) = exact_version_id {
                purge_exact_object_version(state, object_key, version_id).await?;
            } else {
                state.object_store.purge_all_versions(object_key).await?;
            }
        }
        tx.commit().await?;
        Ok::<(bool, bool), ApiError>((referenced, exact_version_id.is_some()))
    }
    .await;
    match result {
        Ok((true, _)) => tracing::warn!(
            upload_id = %upload_id,
            object_key,
            "failed upload publication referenced an existing canonical object; retained it"
        ),
        Ok((false, true)) => tracing::warn!(
            upload_id = %upload_id,
            object_key,
            "purged the exact unreferenced canonical object version after upload publication failure"
        ),
        Ok((false, false)) => tracing::warn!(
            upload_id = %upload_id,
            object_key,
            "purged unreferenced canonical object after upload publication failure"
        ),
        Err(error) => tracing::error!(
            ?error,
            upload_id = %upload_id,
            object_key,
            "could not compensate failed canonical upload publication"
        ),
    }
}

async fn purge_exact_object_version(
    state: &AppState,
    object_key: &str,
    object_version_id: &str,
) -> ApiResult<()> {
    let client = multipart_inventory_client(&state.config).await;
    client
        .delete_object()
        .bucket(&state.config.s3_bucket)
        .key(object_key)
        .version_id(object_version_id)
        .send()
        .await
        .map_err(|error| {
            ApiError::Internal(format!(
                "could not purge the failed canonical object version: {error}"
            ))
        })?;
    let probe = client
        .head_object()
        .bucket(&state.config.s3_bucket)
        .key(object_key)
        .version_id(object_version_id)
        .send()
        .await;
    match probe {
        Err(error)
            if error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 404) =>
        {
            Ok(())
        }
        Ok(_) => Err(ApiError::Internal(
            "failed canonical object version remained after compensation".to_owned(),
        )),
        Err(error) => Err(ApiError::Internal(format!(
            "could not verify failed canonical object version removal: {error}"
        ))),
    }
}

async fn load_upload(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auth: &AuthContext,
    upload_id: Uuid,
    for_update: bool,
) -> ApiResult<UploadRow> {
    let row = if for_update {
        sqlx::query(
            r#"
        SELECT id,scope_id,credential_id,stable_import_id,path,media_type,
               import_metadata,describe_binary,expected_content_hash,expected_size_bytes,
               part_size_bytes,temporary_object_key,temporary_object_version_id,
               canonical_object_key,canonical_object_version_id,
               multipart_upload_id,stage_id,status,completion_token,
               completion_lease_expires_at,expires_at
        FROM straylight.asset_uploads
        WHERE user_id=$1 AND id=$2
        FOR UPDATE
        "#,
        )
        .bind(auth.user_id.0)
        .bind(upload_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query(
            r#"
        SELECT id,scope_id,credential_id,stable_import_id,path,media_type,
               import_metadata,describe_binary,expected_content_hash,expected_size_bytes,
               part_size_bytes,temporary_object_key,temporary_object_version_id,
               canonical_object_key,canonical_object_version_id,
               multipart_upload_id,stage_id,status,completion_token,
               completion_lease_expires_at,expires_at
        FROM straylight.asset_uploads
        WHERE user_id=$1 AND id=$2
        "#,
        )
        .bind(auth.user_id.0)
        .bind(upload_id)
        .fetch_optional(&mut **tx)
        .await?
    }
    .ok_or_else(|| ApiError::not_found("upload_not_found", &format!("upload:{upload_id}")))?;
    Ok(UploadRow {
        id: row.try_get("id")?,
        scope_id: row.try_get("scope_id")?,
        credential_id: row.try_get("credential_id")?,
        stable_import_id: row.try_get("stable_import_id")?,
        path: row.try_get("path")?,
        media_type: row.try_get("media_type")?,
        import_metadata: row.try_get("import_metadata")?,
        describe_binary: row.try_get("describe_binary")?,
        expected_content_hash: row.try_get("expected_content_hash")?,
        expected_size_bytes: row.try_get("expected_size_bytes")?,
        part_size_bytes: row.try_get("part_size_bytes")?,
        temporary_object_key: row.try_get("temporary_object_key")?,
        temporary_object_version_id: row.try_get("temporary_object_version_id")?,
        canonical_object_key: row.try_get("canonical_object_key")?,
        canonical_object_version_id: row.try_get("canonical_object_version_id")?,
        multipart_upload_id: row.try_get("multipart_upload_id")?,
        stage_id: row.try_get("stage_id")?,
        status: row.try_get("status")?,
        completion_token: row.try_get("completion_token")?,
        completion_lease_expires_at: row.try_get("completion_lease_expires_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}

fn ensure_upload_owner(upload: &UploadRow, auth: &AuthContext) -> ApiResult<()> {
    if upload.credential_id != auth.credential_id.0 {
        return Err(ApiError::not_found(
            "upload_not_found",
            &format!("upload:{}", upload.id),
        ));
    }
    Ok(())
}

fn ensure_uploading(upload: &UploadRow) -> ApiResult<()> {
    if upload.expires_at <= Utc::now() {
        return Err(ApiError::conflict(
            "upload_expired",
            "the resumable upload session has expired",
            json!({"upload_ref": format!("upload:{}", upload.id)}),
        ));
    }
    if upload.status != "uploading" {
        return Err(ApiError::conflict(
            "upload_not_writable",
            "the resumable upload is no longer accepting parts",
            json!({
                "upload_ref": format!("upload:{}", upload.id),
                "status": upload.status
            }),
        ));
    }
    Ok(())
}

fn is_content_hash_mismatch(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::Public {
            code: "content_hash_mismatch",
            ..
        }
    )
}

fn validate_attachment_context(value: Option<&Value>) -> ApiResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let contexts = value
        .as_array()
        .ok_or_else(|| ApiError::invalid("attachment_context must be an array"))?;
    if contexts.len() > 8 || serde_json::to_vec(value)?.len() > 32 * 1024 {
        return Err(ApiError::invalid(
            "attachment_context exceeds its bounded size",
        ));
    }
    for context in contexts {
        let object = context
            .as_object()
            .ok_or_else(|| ApiError::invalid("attachment_context entries must be objects"))?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "source_markdown_path" | "excerpt"))
        {
            return Err(ApiError::invalid(
                "attachment_context entries contain an unknown field",
            ));
        }
        let source_path = object
            .get("source_markdown_path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ApiError::invalid("attachment_context source_markdown_path is required")
            })?;
        write_service::validate_stage_path(source_path)?;
        let excerpt = object
            .get("excerpt")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("attachment_context excerpt is required"))?;
        if excerpt.trim().is_empty()
            || excerpt.chars().count() > 2_000
            || excerpt.chars().any(|character| character == '\0')
        {
            return Err(ApiError::invalid(
                "attachment_context excerpt must contain 1 to 2,000 characters",
            ));
        }
    }
    Ok(())
}

fn expected_part_size(upload: &UploadRow, part_number: i32) -> ApiResult<usize> {
    let total = u64::try_from(upload.expected_size_bytes)
        .map_err(|_| ApiError::Internal("upload size is negative".to_owned()))?;
    let part_size = u64::try_from(upload.part_size_bytes)
        .map_err(|_| ApiError::Internal("upload part size is negative".to_owned()))?;
    let count = part_count(total, part_size);
    let number = u64::try_from(part_number)
        .map_err(|_| ApiError::invalid("part number must be positive"))?;
    if number == 0 || number > count {
        return Err(ApiError::invalid(format!(
            "part number must be between 1 and {count}"
        )));
    }
    let size = if number == count {
        total - part_size.saturating_mul(count.saturating_sub(1))
    } else {
        part_size
    };
    usize::try_from(size).map_err(|_| ApiError::Internal("part size overflow".to_owned()))
}

fn validate_complete_parts(upload: &UploadRow, rows: &[sqlx::postgres::PgRow]) -> ApiResult<()> {
    let total = u64::try_from(upload.expected_size_bytes)
        .map_err(|_| ApiError::Internal("upload size is negative".to_owned()))?;
    let part_size = u64::try_from(upload.part_size_bytes)
        .map_err(|_| ApiError::Internal("upload part size is negative".to_owned()))?;
    let expected_count = part_count(total, part_size);
    if rows.len() != usize::try_from(expected_count).unwrap_or(usize::MAX) {
        return Err(ApiError::conflict(
            "upload_incomplete",
            "not all resumable upload parts have been received",
            json!({
                "expected_parts": expected_count,
                "received_parts": rows.len()
            }),
        ));
    }
    let mut received_bytes = 0_u64;
    for (index, row) in rows.iter().enumerate() {
        let expected_number = i32::try_from(index + 1).unwrap_or(i32::MAX);
        let part_number: i32 = row.try_get("part_number")?;
        let size_bytes: i32 = row.try_get("size_bytes")?;
        if part_number != expected_number
            || usize::try_from(size_bytes).ok() != Some(expected_part_size(upload, part_number)?)
        {
            return Err(ApiError::conflict(
                "upload_part_manifest_mismatch",
                "resumable upload parts do not match the declared manifest",
                json!({"part_number": part_number}),
            ));
        }
        received_bytes = received_bytes
            .checked_add(u64::try_from(size_bytes).unwrap_or(u64::MAX))
            .ok_or_else(|| ApiError::Internal("upload size overflow".to_owned()))?;
    }
    if received_bytes != total {
        return Err(ApiError::conflict(
            "upload_size_mismatch",
            "resumable upload byte count does not match the declared size",
            json!({
                "expected_size_bytes": total,
                "received_size_bytes": received_bytes
            }),
        ));
    }
    Ok(())
}

async fn resolve_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auth: &AuthContext,
    scope_ref: &str,
) -> ApiResult<Uuid> {
    if !auth.scope_refs.iter().any(|value| value == scope_ref) {
        return Err(ApiError::not_found("scope_not_found", scope_ref));
    }
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.scopes WHERE user_id=$1 AND scope_ref=$2",
    )
    .bind(auth.user_id.0)
    .bind(scope_ref)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", scope_ref))
}

async fn scope_ref(state: &AppState, auth: &AuthContext, scope_id: Uuid) -> ApiResult<String> {
    let mut tx = state.begin_read(auth).await?;
    let value = sqlx::query_scalar::<_, String>(
        "SELECT scope_ref::text FROM straylight.scopes WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", &scope_id.to_string()))?;
    tx.commit().await?;
    Ok(value)
}

async fn insert_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auth: &AuthContext,
    scope_id: Uuid,
    action: &str,
    details: Value,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,action,details,content_free
        ) VALUES ($1,$2,$3,$4,$5,$6,true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(action)
    .bind(details)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn normalized_hash(value: &str) -> ApiResult<String> {
    let digest = value
        .trim()
        .trim_start_matches("sha256:")
        .to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::invalid(
            "content_hash must be a SHA-256 hex digest",
        ));
    }
    Ok(digest)
}

fn normalize_media_type(value: Option<&str>) -> ApiResult<String> {
    let value = value.unwrap_or("application/octet-stream").trim();
    value
        .parse::<mime::Mime>()
        .map(|parsed| parsed.essence_str().to_owned())
        .map_err(|_| ApiError::invalid("media_type is invalid"))
}

fn parse_ref(value: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.trim_start_matches("upload:"))
        .map_err(|_| ApiError::invalid("upload reference is invalid"))
}

fn parse_stage_ref(value: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.trim_start_matches("stage:"))
        .map_err(|_| ApiError::invalid("stage reference is invalid"))
}

fn part_count(size_bytes: u64, part_size_bytes: u64) -> u64 {
    size_bytes.div_ceil(part_size_bytes)
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(size_bytes: i64, part_size_bytes: i32) -> UploadRow {
        UploadRow {
            id: Uuid::nil(),
            scope_id: Uuid::nil(),
            credential_id: Uuid::nil(),
            stable_import_id: None,
            path: "data/archive.sqlite".to_owned(),
            media_type: "application/vnd.sqlite3".to_owned(),
            import_metadata: json!({}),
            describe_binary: true,
            expected_content_hash: "0".repeat(64),
            expected_size_bytes: size_bytes,
            part_size_bytes,
            temporary_object_key: "temporary".to_owned(),
            temporary_object_version_id: None,
            canonical_object_key: None,
            canonical_object_version_id: None,
            multipart_upload_id: "multipart".to_owned(),
            stage_id: None,
            status: "uploading".to_owned(),
            completion_token: None,
            completion_lease_expires_at: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
        }
    }

    #[test]
    fn part_sizes_are_exact_and_bounded() {
        let value = upload(17, 8);
        assert_eq!(expected_part_size(&value, 1).unwrap(), 8);
        assert_eq!(expected_part_size(&value, 2).unwrap(), 8);
        assert_eq!(expected_part_size(&value, 3).unwrap(), 1);
        assert!(expected_part_size(&value, 4).is_err());
    }

    #[test]
    fn hashes_are_normalized_without_accepting_other_shapes() {
        assert_eq!(
            normalized_hash(&format!("sha256:{}", "A".repeat(64))).unwrap(),
            "a".repeat(64)
        );
        assert!(normalized_hash("not-a-hash").is_err());
    }

    #[test]
    fn cleanup_jobs_are_confined_to_content_addressed_user_blobs() {
        let user_id = Uuid::now_v7();
        let valid = format!("{user_id}/blobs/{}", "a".repeat(64));
        assert!(validate_cleanup_object_key(user_id, &valid, false, None).is_ok());
        let upload_id = Uuid::now_v7();
        let temporary = format!("{user_id}/uploads/{upload_id}");
        assert!(validate_cleanup_object_key(user_id, &temporary, true, Some(upload_id)).is_ok());
        assert!(
            validate_cleanup_object_key(
                user_id,
                &format!("{user_id}/uploads/{}", Uuid::now_v7()),
                true,
                Some(upload_id)
            )
            .is_err()
        );
        assert!(validate_cleanup_object_key(Uuid::now_v7(), &valid, false, None).is_err());
        assert!(
            validate_cleanup_object_key(
                user_id,
                &format!("{user_id}/blobs/{}", "A".repeat(64)),
                false,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn completion_requires_the_exact_live_lease() {
        let token = Uuid::now_v7();
        let mut value = upload(17, 8);
        value.status = "verifying".to_owned();
        value.completion_token = Some(token);
        value.completion_lease_expires_at = Some(Utc::now() + chrono::Duration::minutes(5));
        assert!(completion_lease_owned(&value, token));
        assert!(!completion_lease_owned(&value, Uuid::now_v7()));

        value.completion_lease_expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(!completion_lease_owned(&value, token));
        value.completion_lease_expires_at = Some(Utc::now() + chrono::Duration::minutes(5));
        value.status = "aborted".to_owned();
        assert!(!completion_lease_owned(&value, token));
    }

    #[test]
    fn multipart_reconciliation_only_accepts_owned_upload_paths() {
        let user_id = Uuid::now_v7();
        let upload_id = Uuid::now_v7();
        assert_eq!(
            parse_managed_multipart_key(&format!("{user_id}/uploads/{upload_id}")),
            Some((user_id, upload_id))
        );
        assert!(parse_managed_multipart_key(&format!("{user_id}/exports/{upload_id}")).is_none());
        assert!(
            parse_managed_multipart_key(&format!("{user_id}/uploads/{upload_id}/extra")).is_none()
        );
        assert!(parse_managed_multipart_key("not-a-user/uploads/not-an-upload").is_none());
        assert_eq!(
            validate_user_prefix(&format!("{user_id}/")).unwrap(),
            user_id
        );
        assert!(validate_user_prefix("/").is_err());
        assert!(validate_user_prefix(&user_id.to_string()).is_err());
    }
}

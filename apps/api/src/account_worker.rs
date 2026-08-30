use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use flate2::{Compression, write::GzEncoder};
use futures::TryStreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, PgPool, Postgres, Row, Transaction};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    db::AppState,
    error::{ApiError, ApiResult},
    models::UserId,
    quota, upload_service,
};

struct ExportJob {
    id: Uuid,
    user_id: Uuid,
}

struct ExportResult {
    object_key: String,
    object_version_id: Option<String>,
    content_hash: String,
    size_bytes: u64,
    table_count: usize,
    object_count: usize,
}

struct AccountDeletionJob {
    id: Uuid,
    user_id: Uuid,
    requested_by_credential_id: Uuid,
    status: String,
    backup_expiry_due_at: DateTime<Utc>,
    backup_erasure_verified: bool,
}

const ACCOUNT_DELETION_RECORD_KINDS: &[&str] = &[
    "object",
    "claim",
    "relation",
    "source_episode",
    "evidence",
    "document",
    "chunk",
    "dream_job",
    "asset",
    "checkpoint",
    "stage",
];
const ACCOUNT_EXPORT_PART_SIZE_BYTES: usize = 16 * 1024 * 1024;

pub async fn process_account_export(state: &AppState, worker_id: &str) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    recover_stale_exports(pool, state.config.background_job_lease).await?;
    if expire_one_export(state).await? {
        return Ok(true);
    }
    let Some(job) = claim_export(pool, worker_id).await? else {
        return Ok(false);
    };
    let started = Instant::now();
    let result = build_export(state, &job).await;
    match result {
        Ok(_) => {
            record_export_metric("ready", started);
        }
        Err(error) => {
            tracing::warn!(export_id=%job.id, user_id=%job.user_id, ?error, "account export failed");
            let row = sqlx::query(
                r#"
                UPDATE straylight.account_exports
                SET status=CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
                    failure_code=$1,
                    available_at=clock_timestamp()+interval '5 seconds',
                    completed_at=CASE WHEN attempts >= max_attempts
                                      THEN clock_timestamp() ELSE NULL END,
                    locked_at=NULL,locked_by=NULL
                WHERE user_id=$2 AND id=$3 AND status='running'
                RETURNING status
                "#,
            )
            .bind(export_failure_code(&error))
            .bind(job.user_id)
            .bind(job.id)
            .fetch_optional(pool)
            .await?;
            if let Some(row) = row {
                let status: String = row.try_get("status")?;
                record_export_metric(
                    if status == "failed" {
                        "failed"
                    } else {
                        "retry"
                    },
                    started,
                );
            } else {
                record_export_metric("canceled", started);
            }
        }
    }
    Ok(true)
}

pub async fn process_account_deletion(state: &AppState, worker_id: &str) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let Some(job) =
        claim_account_deletion(pool, worker_id, state.config.background_job_lease).await?
    else {
        return Ok(false);
    };
    let started = Instant::now();
    let result = match job.status.as_str() {
        "queued" => prepare_account_deletion(pool, &job).await,
        "running" => advance_account_deletion(state, &job).await,
        "awaiting_backup_expiry"
            if job.backup_expiry_due_at <= Utc::now() && job.backup_erasure_verified =>
        {
            finalize_account_deletion(pool, &job).await
        }
        "awaiting_backup_expiry" => clear_account_deletion_lock(pool, &job).await,
        status => Err(ApiError::Internal(format!(
            "claimed unsupported account deletion status {status}"
        ))),
    };
    match result {
        Ok(()) => record_account_deletion_metric(job.status.as_str(), "success", started),
        Err(error) => {
            tracing::warn!(
                request_id=%job.id,
                user_id=%job.user_id,
                ?error,
                "account deletion step failed"
            );
            let row = sqlx::query(
                r#"
                UPDATE straylight.account_deletion_requests
                SET failure_attempts=failure_attempts+1,
                    status=CASE WHEN failure_attempts+1 >= max_failure_attempts
                                THEN 'failed' ELSE status END,
                    failure_code=$1,
                    completed_at=CASE WHEN failure_attempts+1 >= max_failure_attempts
                                      THEN clock_timestamp() ELSE completed_at END,
                    locked_at=NULL,locked_by=NULL
                WHERE user_id=$2 AND id=$3
                RETURNING status
                "#,
            )
            .bind(account_deletion_failure_code(&error))
            .bind(job.user_id)
            .bind(job.id)
            .fetch_one(pool)
            .await?;
            let status: String = row.try_get("status")?;
            record_account_deletion_metric(
                job.status.as_str(),
                if status == "failed" {
                    "failed"
                } else {
                    "retry"
                },
                started,
            );
        }
    }
    Ok(true)
}

async fn claim_account_deletion(
    pool: &PgPool,
    worker_id: &str,
    lease: Duration,
) -> ApiResult<Option<AccountDeletionJob>> {
    let lease_seconds = i64::try_from(lease.as_secs()).unwrap_or(i64::MAX);
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT id,user_id,requested_by_credential_id,status,backup_expiry_due_at,
               (
                 backup_erasure_verified_at IS NOT NULL
                 AND backup_erasure_watermark_at IS NOT NULL
                 AND backup_erasure_receipt_sha256 IS NOT NULL
                 AND backup_erasure_source IS NOT NULL
                 AND terminal_result ? 'canonical_purged_at'
                 AND (terminal_result->>'canonical_purged_at')::timestamptz
                       < backup_erasure_watermark_at
               ) AS backup_erasure_verified
        FROM straylight.account_deletion_requests
        WHERE (
            status IN ('queued','running')
            OR (
              status='awaiting_backup_expiry'
              AND backup_expiry_due_at <= clock_timestamp()
              AND backup_erasure_verified_at IS NOT NULL
              AND backup_erasure_watermark_at IS NOT NULL
              AND backup_erasure_receipt_sha256 IS NOT NULL
              AND backup_erasure_source IS NOT NULL
              AND terminal_result ? 'canonical_purged_at'
              AND (terminal_result->>'canonical_purged_at')::timestamptz
                    < backup_erasure_watermark_at
            )
          )
          AND (
            locked_at IS NULL
            OR locked_at < clock_timestamp()-make_interval(secs => $1)
          )
        ORDER BY
          CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END,
          created_at,id
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(lease_seconds)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let job = AccountDeletionJob {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        requested_by_credential_id: row.try_get("requested_by_credential_id")?,
        status: row.try_get("status")?,
        backup_expiry_due_at: row.try_get("backup_expiry_due_at")?,
        backup_erasure_verified: row.try_get("backup_erasure_verified")?,
    };
    sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET locked_at=clock_timestamp(),locked_by=$1
        WHERE user_id=$2 AND id=$3
        "#,
    )
    .bind(worker_id)
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

async fn prepare_account_deletion(pool: &PgPool, job: &AccountDeletionJob) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    let defaults = sqlx::query(
        r#"
        SELECT scope.id AS scope_id,policy.id AS policy_id,policy.current_version
        FROM straylight.scopes AS scope
        JOIN straylight.policies AS policy
          ON policy.user_id=scope.user_id AND policy.is_default
        WHERE scope.user_id=$1 AND scope.scope_ref='scope:root'
        "#,
    )
    .bind(job.user_id)
    .fetch_one(&mut *tx)
    .await?;
    let root_scope_id: Uuid = defaults.try_get("scope_id")?;
    let policy_id: Uuid = defaults.try_get("policy_id")?;
    let policy_version: i32 = defaults.try_get("current_version")?;
    let authorization_source_id = Uuid::now_v7();
    let authorization_evidence_id = Uuid::now_v7();
    let source_ref = format!("account-deletion:{}", job.id);
    let evidence_text = "Account deletion authorized by the account owner.";

    sqlx::query(
        r#"
        INSERT INTO straylight.source_episodes (
          id,user_id,scope_id,source_ref,source_kind,source_version,title,
          captured_at,content_hash,native_locator,metadata,policy_id,policy_version
        ) VALUES (
          $1,$2,$3,$4,'account_deletion_request','1','Account deletion request',
          clock_timestamp(),encode(public.digest($4,'sha256'),'hex'),
          '{}'::jsonb,jsonb_build_object('request_id',$5::text),$6,$7
        )
        "#,
    )
    .bind(authorization_source_id)
    .bind(job.user_id)
    .bind(root_scope_id)
    .bind(&source_ref)
    .bind(job.id)
    .bind(policy_id)
    .bind(policy_version)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.evidence_items (
          id,user_id,scope_id,source_episode_id,evidence_kind,locator,
          text_content,content_hash,observed_at,policy_id,policy_version
        ) VALUES (
          $1,$2,$3,$4,'authorization',
          jsonb_build_object('request_id',$5::text),
          $6,encode(public.digest($6,'sha256'),'hex'),clock_timestamp(),$7,$8
        )
        "#,
    )
    .bind(authorization_evidence_id)
    .bind(job.user_id)
    .bind(root_scope_id)
    .bind(authorization_source_id)
    .bind(job.id)
    .bind(evidence_text)
    .bind(policy_id)
    .bind(policy_version)
    .execute(&mut *tx)
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT record_id,scope_id
        FROM straylight.record_keys
        WHERE user_id=$1
          AND record_kind=ANY($2::text[])
          AND record_id <> ALL($3::uuid[])
        ORDER BY created_at,record_id
        "#,
    )
    .bind(job.user_id)
    .bind(ACCOUNT_DELETION_RECORD_KINDS)
    .bind(vec![authorization_source_id, authorization_evidence_id])
    .fetch_all(&mut *tx)
    .await?;
    let record_ids = rows
        .iter()
        .map(|row| row.try_get("record_id"))
        .collect::<Result<Vec<Uuid>, _>>()?;
    let scope_ids = rows
        .iter()
        .map(|row| row.try_get("scope_id"))
        .collect::<Result<Vec<Uuid>, _>>()?;

    if !record_ids.is_empty() {
        sqlx::query(
            r#"
            INSERT INTO straylight.tombstones (
              id,user_id,scope_id,target_record_id,source_episode_id,evidence_id,
              authorized_credential_id,reason
            )
            SELECT gen_random_uuid(),$1,target.scope_id,target.record_id,$2,$3,$4,
                   'account deletion request'
            FROM unnest($5::uuid[],$6::uuid[]) AS target(record_id,scope_id)
            ON CONFLICT (user_id,target_record_id) DO NOTHING
            "#,
        )
        .bind(job.user_id)
        .bind(authorization_source_id)
        .bind(authorization_evidence_id)
        .bind(job.requested_by_credential_id)
        .bind(&record_ids)
        .bind(&scope_ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.deletion_jobs (
              id,user_id,scope_id,tombstone_id,status
            )
            SELECT gen_random_uuid(),tombstone.user_id,tombstone.scope_id,
                   tombstone.id,'queued'
            FROM straylight.tombstones AS tombstone
            WHERE tombstone.user_id=$1
              AND tombstone.target_record_id=ANY($2::uuid[])
            ON CONFLICT (user_id,tombstone_id) DO NOTHING
            "#,
        )
        .bind(job.user_id)
        .bind(&record_ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.account_deletion_targets (
              user_id,request_id,record_id,deletion_job_id
            )
            SELECT $1,$2,tombstone.target_record_id,deletion_job.id
            FROM straylight.tombstones AS tombstone
            JOIN straylight.deletion_jobs AS deletion_job
              ON deletion_job.user_id=tombstone.user_id
             AND deletion_job.tombstone_id=tombstone.id
            WHERE tombstone.user_id=$1
              AND tombstone.target_record_id=ANY($3::uuid[])
            ON CONFLICT (user_id,request_id,record_id) DO NOTHING
            "#,
        )
        .bind(job.user_id)
        .bind(job.id)
        .bind(&record_ids)
        .execute(&mut *tx)
        .await?;
    }
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.account_deletion_targets WHERE user_id=$1 AND request_id=$2",
    )
    .bind(job.user_id)
    .bind(job.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET status='running',records_total=$1,records_completed=0,
            started_at=coalesce(started_at,clock_timestamp()),
            failure_code=NULL,locked_at=NULL,locked_by=NULL
        WHERE user_id=$2 AND id=$3 AND status='queued'
        "#,
    )
    .bind(total)
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn advance_account_deletion(state: &AppState, job: &AccountDeletionJob) -> ApiResult<()> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let progress = sqlx::query(
        r#"
        SELECT count(*)::bigint AS total,
               count(*) FILTER (WHERE deletion_job.status='completed')::bigint AS completed,
               count(*) FILTER (WHERE deletion_job.status='failed')::bigint AS failed,
               count(*) FILTER (WHERE deletion_job.status='blocked')::bigint AS blocked
        FROM straylight.account_deletion_targets AS target
        JOIN straylight.deletion_jobs AS deletion_job
          ON deletion_job.user_id=target.user_id
         AND deletion_job.id=target.deletion_job_id
        WHERE target.user_id=$1 AND target.request_id=$2
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .fetch_one(pool)
    .await?;
    let total: i64 = progress.try_get("total")?;
    let completed: i64 = progress.try_get("completed")?;
    let failed: i64 = progress.try_get("failed")?;
    let blocked: i64 = progress.try_get("blocked")?;
    if blocked > 0 {
        sqlx::query(
            r#"
            UPDATE straylight.account_deletion_requests
            SET status='failed',records_total=$1,records_completed=$2,
                failure_code='record_deletion_blocked',
                terminal_result=jsonb_build_object(
                  'reason','record_deletion_blocked',
                  'blocked_records',$3
                ),
                completed_at=clock_timestamp(),locked_at=NULL,locked_by=NULL
            WHERE user_id=$4 AND id=$5 AND status='running'
            "#,
        )
        .bind(total)
        .bind(completed)
        .bind(blocked)
        .bind(job.user_id)
        .bind(job.id)
        .execute(pool)
        .await?;
        return Ok(());
    }
    if completed < total || failed > 0 {
        sqlx::query(
            r#"
            UPDATE straylight.account_deletion_requests
            SET records_total=$1,records_completed=$2,
                failure_code=CASE WHEN $3 > 0 THEN 'record_deletion_retrying' ELSE NULL END,
                locked_at=NULL,locked_by=NULL
            WHERE user_id=$4 AND id=$5 AND status='running'
            "#,
        )
        .bind(total)
        .bind(completed)
        .bind(failed)
        .bind(job.user_id)
        .bind(job.id)
        .execute(pool)
        .await?;
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(job.user_id)
        .execute(&mut *tx)
        .await?;
    let fenced = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
          SELECT 1
          FROM straylight.account_deletion_fences AS fence
          JOIN straylight.users AS user_row ON user_row.id=fence.user_id
          WHERE fence.user_id=$1 AND fence.request_id=$2
            AND user_row.account_status='deleting'
        )
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .fetch_one(&mut *tx)
    .await?;
    if !fenced {
        return Err(ApiError::Internal(
            "account deletion cannot purge storage without its durable fence".to_owned(),
        ));
    }
    // Asset uploads no longer exist; any in-flight multipart under the user
    // prefix (account exports included) is aborted below before the purge.
    let abandoned_uploads = 0_u64;
    let prefix = format!("{}/", job.user_id);
    let first_multipart_abort = upload_service::abort_multipart_prefix(state, &prefix).await?;
    let first_purge = state.object_store.purge_prefix(&prefix).await?;
    let second_multipart_abort = upload_service::abort_multipart_prefix(state, &prefix).await?;
    let second_purge = state.object_store.purge_prefix(&prefix).await?;
    let object_versions_deleted = first_purge
        .versions_deleted
        .saturating_add(second_purge.versions_deleted);
    let delete_markers_deleted = first_purge
        .delete_markers_deleted
        .saturating_add(second_purge.delete_markers_deleted);
    let multipart_uploads_aborted = first_multipart_abort.saturating_add(second_multipart_abort);
    redact_account_for_retention(&mut tx, job).await?;
    sqlx::query(
        r#"
        UPDATE straylight.account_exports
        SET status='deleted',object_key=NULL,object_version_id=NULL,
            completed_at=coalesce(completed_at,clock_timestamp())
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET status='awaiting_backup_expiry',records_total=$1,records_completed=$2,
            failure_code=NULL,
            terminal_result=jsonb_build_object(
              'canonical_redaction','complete',
              'object_versions_deleted',$3,
              'delete_markers_deleted',$4,
              'multipart_uploads_aborted',$7,
              'asset_upload_sessions_aborted',$8,
              'storage_reconciliation_passes',2,
              'canonical_purged_at',clock_timestamp(),
              'backup_expiry_due_at',backup_expiry_due_at,
              'backup_status','retained_until_deadline'
            ),
            locked_at=NULL,locked_by=NULL
        WHERE user_id=$5 AND id=$6 AND status='running'
        "#,
    )
    .bind(total)
    .bind(completed)
    .bind(i64::try_from(object_versions_deleted).unwrap_or(i64::MAX))
    .bind(i64::try_from(delete_markers_deleted).unwrap_or(i64::MAX))
    .bind(job.user_id)
    .bind(job.id)
    .bind(i64::try_from(multipart_uploads_aborted).unwrap_or(i64::MAX))
    .bind(i64::try_from(abandoned_uploads).unwrap_or(i64::MAX))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn redact_account_for_retention(
    tx: &mut Transaction<'_, Postgres>,
    job: &AccountDeletionJob,
) -> ApiResult<()> {
    sqlx::query("SELECT set_config('straylight.account_deletion_request_id',$1,true)")
        .bind(job.id.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar::<_, Value>("SELECT straylight.purge_account_user_rows($1)")
        .bind(job.user_id)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        r#"
        UPDATE straylight.api_credentials
        SET label=CASE WHEN id=$2
                       THEN 'Deletion status credential'
                       ELSE 'Deleted credential' END,
            token_hash=CASE WHEN id=$2 THEN token_hash ELSE encode(public.digest(
              'deleted:' || id::text || ':' || $3::text,'sha256'
            ),'hex') END,
            disabled_at=CASE WHEN id=$2 THEN disabled_at
                             ELSE coalesce(disabled_at,clock_timestamp()) END
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .bind(job.requested_by_credential_id)
    .bind(job.id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET reason='deleted',
            confirmation_hash=encode(public.digest('','sha256'),'hex')
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.users
        SET external_ref='deleting:' || id::text,
            display_name='Deleting account'
        WHERE id=$1 AND account_status='deleting'
        "#,
    )
    .bind(job.user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn finalize_account_deletion(pool: &PgPool, job: &AccountDeletionJob) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('straylight.account_deletion_request_id',$1,true)")
        .bind(job.id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query_scalar::<_, Value>("SELECT straylight.purge_account_user_rows($1)")
        .bind(job.user_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        UPDATE straylight.audit_events
        SET actor_ref=NULL,request_id=NULL,
            details=jsonb_build_object('redacted_by_account_deletion',$2::text),
            content_free=true
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.api_credentials
        SET label='Deleted credential',
            token_hash=encode(public.digest(
              'deleted:' || id::text || ':' || $2::text,'sha256'
            ),'hex'),
            disabled_at=coalesce(disabled_at,clock_timestamp())
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.account_exports
        SET status='deleted',object_key=NULL,object_version_id=NULL,
            failure_code=NULL
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET reason='deleted',
            confirmation_hash=encode(public.digest('','sha256'),'hex')
        WHERE user_id=$1
        "#,
    )
    .bind(job.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.users
        SET external_ref='deleted:' || id::text,
            display_name='Deleted account',
            account_status='deleted',deleted_at=clock_timestamp()
        WHERE id=$1 AND account_status='deleting'
        "#,
    )
    .bind(job.user_id)
    .execute(&mut *tx)
    .await?;
    let completion = sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET status='completed',completed_at=clock_timestamp(),
            failure_code=NULL,locked_at=NULL,locked_by=NULL,
            terminal_result=coalesce(terminal_result,'{}'::jsonb) || jsonb_build_object(
              'backup_status','prune_verified',
              'identity_redaction','complete',
              'completed_at',clock_timestamp()
            )
        WHERE user_id=$1 AND id=$2 AND status='awaiting_backup_expiry'
          AND backup_expiry_due_at <= clock_timestamp()
          AND backup_erasure_verified_at IS NOT NULL
          AND backup_erasure_watermark_at IS NOT NULL
          AND backup_erasure_receipt_sha256 IS NOT NULL
          AND backup_erasure_source IS NOT NULL
          AND terminal_result ? 'canonical_purged_at'
          AND (terminal_result->>'canonical_purged_at')::timestamptz
                < backup_erasure_watermark_at
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await?;
    if completion.rows_affected() != 1 {
        return Err(ApiError::Internal(
            "account deletion cannot complete without verified backup erasure".to_owned(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

async fn clear_account_deletion_lock(pool: &PgPool, job: &AccountDeletionJob) -> ApiResult<()> {
    sqlx::query(
        "UPDATE straylight.account_deletion_requests SET locked_at=NULL,locked_by=NULL WHERE user_id=$1 AND id=$2",
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(pool)
    .await?;
    Ok(())
}

fn account_deletion_failure_code(error: &ApiError) -> &'static str {
    match error {
        ApiError::Database(_) => "database_error",
        ApiError::Json(_) => "serialization_error",
        ApiError::Public { code, .. } => code,
        ApiError::Migration(_) => "migration_error",
        ApiError::Internal(_) => "deletion_error",
    }
}

fn record_account_deletion_metric(stage: &str, result: &'static str, started: Instant) {
    let stage = match stage {
        "queued" => "prepare",
        "running" => "redact",
        "awaiting_backup_expiry" => "retention",
        _ => "unknown",
    };
    metrics::counter!("account.deletions", "stage" => stage, "result" => result).increment(1);
    metrics::histogram!("account.deletion.duration_ms", "stage" => stage, "result" => result)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
}

async fn recover_stale_exports(pool: &PgPool, lease: Duration) -> ApiResult<()> {
    let lease_seconds = i64::try_from(lease.as_secs()).unwrap_or(i64::MAX);
    sqlx::query(
        r#"
        UPDATE straylight.account_exports
        SET status=CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
            failure_code='worker_lease_expired',
            available_at=clock_timestamp(),
            completed_at=CASE WHEN attempts >= max_attempts
                              THEN clock_timestamp() ELSE NULL END,
            locked_at=NULL,locked_by=NULL
        WHERE status='running'
          AND coalesce(locked_at,started_at,created_at)
              < clock_timestamp()-make_interval(secs => $1)
        "#,
    )
    .bind(lease_seconds)
    .execute(pool)
    .await?;
    Ok(())
}

async fn claim_export(pool: &PgPool, worker_id: &str) -> ApiResult<Option<ExportJob>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT id,user_id
        FROM straylight.account_exports
        WHERE status='queued' AND available_at <= clock_timestamp()
        ORDER BY created_at,id
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let job = ExportJob {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
    };
    sqlx::query(
        r#"
        UPDATE straylight.account_exports
        SET status='running',attempts=attempts+1,
            started_at=coalesce(started_at,clock_timestamp()),
            completed_at=NULL,locked_at=clock_timestamp(),locked_by=$1
        WHERE user_id=$2 AND id=$3
        "#,
    )
    .bind(worker_id)
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

async fn build_export(state: &AppState, job: &ExportJob) -> ApiResult<ExportResult> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let root = state
        .config
        .account_export_temp_dir
        .join(job.id.to_string());
    let archive_path = state
        .config
        .account_export_temp_dir
        .join(format!("{}.tar.gz", job.id));
    let result = build_export_inner(state, pool, job, &root, &archive_path).await;
    let _ = tokio::fs::remove_dir_all(&root).await;
    let _ = tokio::fs::remove_file(&archive_path).await;
    result
}

async fn build_export_inner(
    state: &AppState,
    pool: &PgPool,
    job: &ExportJob,
    root: &Path,
    archive_path: &Path,
) -> ApiResult<ExportResult> {
    tokio::fs::create_dir_all(root.join("tables"))
        .await
        .map_err(|error| {
            ApiError::Internal(format!("could not create export directory: {error}"))
        })?;
    tokio::fs::create_dir_all(root.join("objects"))
        .await
        .map_err(|error| {
            ApiError::Internal(format!("could not create export directory: {error}"))
        })?;

    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let user = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(user_row) FROM straylight.users AS user_row WHERE id=$1",
    )
    .bind(job.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("user_not_found", &job.user_id.to_string()))?;
    write_json(root.join("user.json"), &user).await?;

    let tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT column_row.table_name
        FROM information_schema.columns AS column_row
        JOIN information_schema.tables AS table_row
          ON table_row.table_schema=column_row.table_schema
         AND table_row.table_name=column_row.table_name
        WHERE column_row.table_schema='straylight'
          AND column_row.column_name='user_id'
          AND table_row.table_type='BASE TABLE'
        ORDER BY column_row.table_name
        "#,
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut table_rows = BTreeMap::new();
    for table in &tables {
        if !safe_identifier(table) {
            return Err(ApiError::Internal(
                "database returned an unsafe export table identifier".to_owned(),
            ));
        }
        if matches!(
            table.as_str(),
            "web_sessions" | "password_reset_tokens" | "web_auth_rate_limits"
        ) {
            continue;
        }
        let projection = account_export_projection(table);
        let query = format!(
            "SELECT {projection} FROM straylight.\"{table}\" AS table_row \
             WHERE user_id=$1 ORDER BY ctid"
        );
        let path = root.join("tables").join(format!("{table}.jsonl"));
        let mut file = tokio::fs::File::create(path).await.map_err(|error| {
            ApiError::Internal(format!("could not create export table file: {error}"))
        })?;
        // The identifier is accepted only after `safe_identifier` rejects
        // every character outside the database's generated table vocabulary.
        let mut rows = sqlx::query_scalar::<_, Value>(AssertSqlSafe(query.as_str()))
            .bind(job.user_id)
            .fetch(&mut *tx);
        let mut count = 0_u64;
        while let Some(value) = rows.try_next().await? {
            let mut encoded = serde_json::to_vec(&value)?;
            encoded.push(b'\n');
            file.write_all(&encoded).await.map_err(|error| {
                ApiError::Internal(format!("could not write export table file: {error}"))
            })?;
            count = count.saturating_add(1);
        }
        file.sync_all().await.map_err(|error| {
            ApiError::Internal(format!("could not sync export table file: {error}"))
        })?;
        table_rows.insert(table.clone(), count);
    }

    let object_rows = sqlx::query(
        r#"
        WITH object_references AS (
          SELECT bucket::text AS bucket,
                 object_key,
                 object_version_id,
                 content_hash::text AS content_hash,
                 size_bytes,
                 'asset_version'::text AS reference_kind,
                 asset_id::text || ':' || version::text AS reference_ref
	          FROM straylight.asset_versions
	          WHERE user_id=$1
	            AND NOT (
	              object_key::text = user_id::text || '/deleted-assets/'
	                || asset_id::text || '/' || version::text
	              AND content_hash::text =
	                'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
	              AND size_bytes=0
	              AND media_type::text='application/x-straylight-deleted'
	              AND metadata ? 'redacted_by_deletion_job'
	            )
	          UNION ALL
          SELECT $2::text AS bucket,
                 canonical_object_key AS object_key,
                 canonical_object_version_id AS object_version_id,
                 expected_content_hash::text AS content_hash,
                 expected_size_bytes AS size_bytes,
                 'completed_upload'::text AS reference_kind,
                 id::text AS reference_ref
          FROM straylight.asset_uploads
          WHERE user_id=$1
            AND status IN ('completed','consumed')
            AND canonical_object_key IS NOT NULL
            AND canonical_object_version_id IS NOT NULL
        )
        SELECT bucket,object_key,object_version_id,
               min(content_hash) AS content_hash,
               min(size_bytes) AS size_bytes,
               count(DISTINCT content_hash)::bigint AS content_hash_count,
               count(DISTINCT size_bytes)::bigint AS size_count,
               jsonb_agg(
                 jsonb_build_object(
                   'kind',reference_kind,
                   'ref',reference_ref
                 )
                 ORDER BY reference_kind,reference_ref
               ) AS object_references
        FROM object_references
        GROUP BY bucket,object_key,object_version_id
        ORDER BY bucket,object_key,object_version_id NULLS FIRST
        "#,
    )
    .bind(job.user_id)
    .bind(&state.config.s3_bucket)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut objects = Vec::with_capacity(object_rows.len());
    for (index, row) in object_rows.iter().enumerate() {
        let bucket: String = row.try_get("bucket")?;
        let object_key: String = row.try_get("object_key")?;
        let content_hash: String = row.try_get("content_hash")?;
        let expected_size: i64 = row.try_get("size_bytes")?;
        let object_version_id: Option<String> = row.try_get("object_version_id")?;
        let content_hash_count: i64 = row.try_get("content_hash_count")?;
        let size_count: i64 = row.try_get("size_count")?;
        let object_references: Value = row.try_get("object_references")?;
        if bucket != state.config.s3_bucket {
            return Err(ApiError::Internal(format!(
                "export object {index} references a bucket outside the active object store"
            )));
        }
        if content_hash_count != 1 || size_count != 1 {
            return Err(ApiError::Internal(format!(
                "export object {index} has inconsistent integrity metadata"
            )));
        }
        let path = root
            .join("objects")
            .join(format!("{index:08}-{content_hash}"));
        let stored = state
            .object_store
            .download_version_to_path(&object_key, object_version_id.as_deref(), &path)
            .await?;
        if stored.sha256.strip_prefix("sha256:") != Some(content_hash.as_str())
            || stored.size_bytes != u64::try_from(expected_size).unwrap_or(u64::MAX)
        {
            return Err(ApiError::Internal(format!(
                "export object integrity check failed for object {index}"
            )));
        }
        objects.push(json!({
            "path": format!("objects/{index:08}-{content_hash}"),
            "bucket": bucket,
            "object_key": object_key,
            "object_version_id": object_version_id,
            "content_hash": format!("sha256:{content_hash}"),
            "size_bytes": stored.size_bytes,
            "references": object_references
        }));
    }

    let manifest = json!({
        "format": "straylight-account-export",
        "version": 1,
        "export_id": format!("export:{}", job.id),
        "user_id": format!("user:{}", job.user_id),
        "created_at": Utc::now(),
        "tables": table_rows,
        "objects": objects,
        "credential_secrets": "excluded"
    });
    write_json(root.join("manifest.json"), &manifest).await?;
    create_archive(root.to_owned(), archive_path.to_owned()).await?;
    publish_account_export(
        state,
        pool,
        job,
        archive_path,
        tables.len(),
        object_rows.len(),
    )
    .await
}

async fn publish_account_export(
    state: &AppState,
    pool: &PgPool,
    job: &ExportJob,
    archive_path: &Path,
    table_count: usize,
    object_count: usize,
) -> ApiResult<ExportResult> {
    let (content_hash, size_bytes) = hash_file(archive_path).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT straylight.assert_storage_write_allowed($1)")
        .bind(job.user_id)
        .execute(&mut *tx)
        .await?;
    let expected_key = format!("{}/uploads/{}", job.user_id, job.id);
    quota::ensure_temporary_export_capacity(&mut tx, job.user_id, size_bytes).await?;
    let multipart = state
        .object_store
        .create_multipart_upload(
            UserId(job.user_id),
            job.id,
            "application/gzip",
            &content_hash,
        )
        .await?;
    if multipart.object_key != expected_key {
        let _ = tx.rollback().await;
        let _ = state
            .object_store
            .abort_multipart_upload(&multipart.object_key, &multipart.upload_id)
            .await;
        return Err(ApiError::Internal(
            "object storage created an account export under an unexpected key".to_owned(),
        ));
    }
    let parts = match upload_export_parts(
        state,
        archive_path,
        &multipart.object_key,
        &multipart.upload_id,
    )
    .await
    {
        Ok(parts) => parts,
        Err(error) => {
            let _ = tx.rollback().await;
            let _ = state
                .object_store
                .abort_multipart_upload(&multipart.object_key, &multipart.upload_id)
                .await;
            compensate_export_object(state, job, &multipart.object_key).await;
            return Err(error);
        }
    };
    let object_version_id = match state
        .object_store
        .complete_multipart_upload(&multipart.object_key, &multipart.upload_id, &parts)
        .await
    {
        Ok(Some(version_id)) if !version_id.is_empty() && version_id != "null" => version_id,
        Ok(_) => {
            let _ = tx.rollback().await;
            compensate_export_object(state, job, &multipart.object_key).await;
            return Err(ApiError::Internal(
                "object storage completed an account export without an exact version ID".to_owned(),
            ));
        }
        Err(error) => {
            let _ = tx.rollback().await;
            let _ = state
                .object_store
                .abort_multipart_upload(&multipart.object_key, &multipart.upload_id)
                .await;
            compensate_export_object(state, job, &multipart.object_key).await;
            return Err(error);
        }
    };
    let result = ExportResult {
        object_key: multipart.object_key,
        object_version_id: Some(object_version_id),
        content_hash,
        size_bytes,
        table_count,
        object_count,
    };
    let ttl_seconds = i64::try_from(state.config.account_export_ttl.as_secs()).unwrap_or(i64::MAX);
    let published = sqlx::query(
        r#"
        UPDATE straylight.account_exports
        SET status='ready',object_key=$1,object_version_id=$2,
            content_hash=$3,size_bytes=$4,
            table_count=$5,object_count=$6,completed_at=clock_timestamp(),
            expires_at=clock_timestamp()+make_interval(secs => $7),
            locked_at=NULL,locked_by=NULL,failure_code=NULL
        WHERE user_id=$8 AND id=$9 AND status='running'
        "#,
    )
    .bind(&result.object_key)
    .bind(&result.object_version_id)
    .bind(&result.content_hash)
    .bind(i64::try_from(result.size_bytes).unwrap_or(i64::MAX))
    .bind(i32::try_from(result.table_count).unwrap_or(i32::MAX))
    .bind(i32::try_from(result.object_count).unwrap_or(i32::MAX))
    .bind(ttl_seconds)
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *tx)
    .await;
    let published = match published {
        Ok(result) => result.rows_affected(),
        Err(error) => {
            let _ = tx.rollback().await;
            compensate_export_object(state, job, &result.object_key).await;
            return Err(ApiError::Database(error));
        }
    };
    if published != 1 {
        tx.rollback().await?;
        compensate_export_object(state, job, &result.object_key).await;
        return Err(ApiError::conflict(
            "account_export_publication_lost",
            "the account export was canceled before object publication committed",
            json!({"export_ref": format!("export:{}", job.id)}),
        ));
    }
    if let Err(error) = tx.commit().await {
        compensate_export_object(state, job, &result.object_key).await;
        return Err(ApiError::Database(error));
    }
    Ok(result)
}

async fn upload_export_parts(
    state: &AppState,
    archive_path: &Path,
    object_key: &str,
    upload_id: &str,
) -> ApiResult<Vec<(i32, String)>> {
    let size_bytes = tokio::fs::metadata(archive_path)
        .await
        .map_err(|error| {
            ApiError::Internal(format!("could not inspect account export size: {error}"))
        })?
        .len();
    let expected_parts = account_export_part_count(size_bytes)?;
    let mut file = tokio::fs::File::open(archive_path).await.map_err(|error| {
        ApiError::Internal(format!("could not open account export for upload: {error}"))
    })?;
    let mut buffer = vec![0_u8; ACCOUNT_EXPORT_PART_SIZE_BYTES];
    let mut parts = Vec::with_capacity(usize::try_from(expected_parts).unwrap_or(10_000));
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = file.read(&mut buffer[filled..]).await.map_err(|error| {
                ApiError::Internal(format!("could not read account export for upload: {error}"))
            })?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        let part_number = i32::try_from(parts.len() + 1)
            .map_err(|_| ApiError::Internal("account export has too many parts".to_owned()))?;
        if part_number > 10_000 {
            return Err(ApiError::Internal(
                "account export exceeds the multipart upload part limit".to_owned(),
            ));
        }
        let uploaded = state
            .object_store
            .upload_multipart_part(
                object_key,
                upload_id,
                part_number,
                Bytes::copy_from_slice(&buffer[..filled]),
            )
            .await?;
        parts.push((part_number, uploaded.etag));
        if filled < buffer.len() {
            break;
        }
    }
    if parts.is_empty() {
        return Err(ApiError::Internal(
            "account export archive is unexpectedly empty".to_owned(),
        ));
    }
    if u64::try_from(parts.len()).unwrap_or(u64::MAX) != expected_parts {
        return Err(ApiError::Internal(
            "account export changed while its multipart upload was streaming".to_owned(),
        ));
    }
    Ok(parts)
}

fn account_export_part_count(size_bytes: u64) -> ApiResult<u64> {
    let count = size_bytes.div_ceil(ACCOUNT_EXPORT_PART_SIZE_BYTES as u64);
    if count == 0 || count > 10_000 {
        return Err(ApiError::Internal(
            "account export exceeds the multipart upload part limit".to_owned(),
        ));
    }
    Ok(count)
}

async fn hash_file(path: &Path) -> ApiResult<(String, u64)> {
    let mut file = tokio::fs::File::open(path).await.map_err(|error| {
        ApiError::Internal(format!(
            "could not open account export for hashing: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| {
            ApiError::Internal(format!("could not hash account export: {error}"))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size_bytes = size_bytes.saturating_add(read as u64);
    }
    Ok((hex::encode(digest.finalize()), size_bytes))
}

async fn compensate_export_object(state: &AppState, job: &ExportJob, object_key: &str) {
    let result = async {
        let pool = state.admin_pool.as_ref().expect("checked at worker start");
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
            .bind(job.user_id)
            .execute(&mut *tx)
            .await?;
        let referenced = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM straylight.account_exports
              WHERE user_id=$1 AND object_key=$2
                AND status='ready'
              UNION ALL
              SELECT 1
              FROM straylight.asset_uploads
              WHERE user_id=$1
                AND (
                  temporary_object_key=$2
                  OR canonical_object_key=$2
                )
            )
            "#,
        )
        .bind(job.user_id)
        .bind(object_key)
        .fetch_one(&mut *tx)
        .await?;
        if !referenced {
            state.object_store.purge_all_versions(object_key).await?;
        }
        tx.commit().await?;
        Ok::<bool, ApiError>(referenced)
    }
    .await;
    match result {
        Ok(true) => tracing::warn!(
            export_id = %job.id,
            object_key,
            "failed export publication was already referenced; retained its object"
        ),
        Ok(false) => tracing::warn!(
            export_id = %job.id,
            object_key,
            "purged unreferenced account export after publication failure"
        ),
        Err(error) => tracing::error!(
            ?error,
            export_id = %job.id,
            object_key,
            "could not compensate failed account export publication"
        ),
    }
}

async fn write_json(path: PathBuf, value: &Value) -> ApiResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| ApiError::Internal(format!("could not write export manifest: {error}")))
}

async fn create_archive(root: PathBuf, archive_path: PathBuf) -> ApiResult<()> {
    tokio::task::spawn_blocking(move || -> ApiResult<()> {
        let output = File::create(&archive_path).map_err(|error| {
            ApiError::Internal(format!("could not create export archive: {error}"))
        })?;
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive
            .append_dir_all("straylight-export", &root)
            .map_err(|error| {
                ApiError::Internal(format!("could not build export archive: {error}"))
            })?;
        let encoder = archive.into_inner().map_err(|error| {
            ApiError::Internal(format!("could not finalize export archive: {error}"))
        })?;
        let mut output = encoder.finish().map_err(|error| {
            ApiError::Internal(format!("could not finish export compression: {error}"))
        })?;
        output.flush().map_err(|error| {
            ApiError::Internal(format!("could not flush export archive: {error}"))
        })?;
        output.sync_all().map_err(|error| {
            ApiError::Internal(format!("could not sync export archive: {error}"))
        })?;
        Ok(())
    })
    .await
    .map_err(|error| ApiError::Internal(format!("export archive task failed: {error}")))?
}

async fn expire_one_export(state: &AppState) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT id,user_id,object_key
        FROM straylight.account_exports
        WHERE status='ready' AND expires_at <= clock_timestamp()
        ORDER BY expires_at,id
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(false);
    };
    let id: Uuid = row.try_get("id")?;
    let user_id: Uuid = row.try_get("user_id")?;
    let object_key: Option<String> = row.try_get("object_key")?;
    tx.commit().await?;
    if let Some(key) = object_key {
        state.object_store.purge_all_versions(&key).await?;
    }
    sqlx::query(
        r#"
        UPDATE straylight.account_exports
        SET status='expired',object_key=NULL,object_version_id=NULL,
            completed_at=coalesce(completed_at,clock_timestamp())
        WHERE user_id=$1 AND id=$2 AND status='ready'
        "#,
    )
    .bind(user_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(true)
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn account_export_projection(table: &str) -> &'static str {
    match table {
        "api_credentials" => "to_jsonb(table_row) - 'token_hash'",
        "notification_installations" => {
            "to_jsonb(table_row) - 'token_ciphertext' - 'token_nonce' - 'token_hash'"
        }
        "secrets" => "to_jsonb(table_row) - 'value_ciphertext' - 'value_nonce'",
        "task_sync_state" => "to_jsonb(table_row) - 'cursor' - 'lease_owner' - 'lease_expires_at'",
        "web_identities" => "to_jsonb(table_row) - 'password_hash'",
        _ => "to_jsonb(table_row)",
    }
}

fn export_failure_code(error: &ApiError) -> &'static str {
    match error {
        ApiError::Database(_) => "database_error",
        ApiError::Json(_) => "serialization_error",
        ApiError::Public { code, .. } => code,
        ApiError::Migration(_) => "migration_error",
        ApiError::Internal(_) => "export_error",
    }
}

fn record_export_metric(result: &'static str, started: Instant) {
    metrics::counter!("account.exports", "result" => result).increment(1);
    metrics::histogram!("account.export.duration_ms", "result" => result)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_export_identifiers_are_strict() {
        assert!(safe_identifier("asset_versions"));
        assert!(!safe_identifier("asset_versions;DROP TABLE"));
        assert!(!safe_identifier(""));
    }

    #[test]
    fn notification_installation_export_excludes_push_token_material() {
        assert_eq!(
            account_export_projection("notification_installations"),
            "to_jsonb(table_row) - 'token_ciphertext' - 'token_nonce' - 'token_hash'"
        );
        assert_eq!(
            account_export_projection("notification_deliveries"),
            "to_jsonb(table_row)"
        );
    }

    #[test]
    fn secret_export_excludes_encrypted_value_material() {
        assert_eq!(
            account_export_projection("secrets"),
            "to_jsonb(table_row) - 'value_ciphertext' - 'value_nonce'"
        );
        assert_eq!(
            account_export_projection("secret_access_log"),
            "to_jsonb(table_row)"
        );
    }

    #[test]
    fn todoist_sync_cursor_is_excluded_from_account_exports() {
        assert_eq!(
            account_export_projection("task_sync_state"),
            "to_jsonb(table_row) - 'cursor' - 'lease_owner' - 'lease_expires_at'"
        );
    }

    #[test]
    fn export_failure_codes_do_not_expose_error_details() {
        let error = ApiError::Internal("secret path".to_owned());
        assert_eq!(export_failure_code(&error), "export_error");
    }

    #[test]
    fn five_gib_account_export_uses_bounded_multipart_upload() {
        let five_gib = 5_u64 * 1024 * 1024 * 1024;
        assert_eq!(account_export_part_count(five_gib).unwrap(), 320);
        assert_eq!(
            account_export_part_count(ACCOUNT_EXPORT_PART_SIZE_BYTES as u64).unwrap(),
            1
        );
        assert!(
            account_export_part_count(ACCOUNT_EXPORT_PART_SIZE_BYTES as u64 * 10_000 + 1).is_err()
        );
    }
}

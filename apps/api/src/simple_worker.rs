use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pgvector::Vector;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::{
    asset_description::{self, WORKSPACE_MODEL_FILE_BYTES},
    asset_profiles,
    db::AppState,
    embeddings::embed_indexing_inputs,
    error::{ApiError, ApiResult},
    foreground_latency::{
        ForegroundGuardDecision, ForegroundLatencySnapshot, SNAPSHOT_SCHEMA, evaluate_guard,
    },
    simple_core,
};

const MAX_ATTEMPTS: i32 = 5;
const LEASE_MINUTES: i32 = 5;
const MAX_EMBED_BATCH_JOBS: i64 = 8;
const MAX_EMBED_CHUNKS_PER_PUBLICATION: usize = 64;
const MAX_FOREGROUND_SNAPSHOT_AGE: Duration = Duration::from_secs(10);
const MAX_FOREGROUND_SNAPSHOT_FUTURE_SKEW: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Job {
    id: Uuid,
    user_id: Uuid,
    kind: String,
    payload: Value,
    attempts: i32,
}

pub async fn process_next(state: &AppState) -> ApiResult<bool> {
    let pool = state
        .admin_pool
        .as_ref()
        .ok_or_else(|| ApiError::configuration("the worker requires DATABASE_URL_ADMIN"))?;
    let job = if let Some(job) = claim_priority(pool).await? {
        job
    } else if let Some(job) = claim_stale_priority(pool).await? {
        job
    } else if !state.config.embedding_backfill_guard {
        mark_exhausted(pool).await?;
        return Ok(false);
    } else if !embedding_backfill_allowed(state).await {
        return Ok(false);
    } else if let Some(job) = claim_embedding(pool).await? {
        job
    } else if let Some(job) = claim_stale_embedding(pool).await? {
        job
    } else {
        mark_exhausted(pool).await?;
        return Ok(false);
    };
    if job.kind == "embed_entry" {
        let mut jobs = vec![job];
        jobs.extend(claim_more_embeddings(pool, MAX_EMBED_BATCH_JOBS - 1).await?);
        return process_embedding_batch(state, pool, jobs).await;
    }
    let started = Instant::now();
    let result = match job.kind.as_str() {
        "describe_binary" => describe_binary(state, &job).await,
        _ => Err(ApiError::Internal(format!(
            "unsupported simple workspace job kind {}",
            job.kind
        ))),
    };
    let outcome = if result.is_ok() { "complete" } else { "retry" };
    metrics::counter!(
        "simple.jobs.processed",
        "kind" => job.kind.clone(),
        "result" => outcome
    )
    .increment(1);
    metrics::histogram!(
        "simple.jobs.duration_ms",
        "kind" => job.kind.clone(),
        "result" => outcome
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    match result {
        Ok(()) => complete(pool, &job).await?,
        Err(error) => {
            tracing::warn!(
                job_id = %job.id,
                kind = job.kind,
                attempt = job.attempts,
                error = %error,
                "simple workspace job failed"
            );
            retry_or_fail(pool, &job, &error).await?;
        }
    }
    Ok(true)
}

async fn embedding_backfill_allowed(state: &AppState) -> bool {
    let Some(url) = state
        .config
        .embedding_backfill_foreground_status_url
        .as_deref()
    else {
        metrics::counter!(
            "simple.embedding_backfill.foreground_guard",
            "result" => "unconfigured"
        )
        .increment(1);
        return true;
    };
    match fetch_foreground_guard(
        &state.foreground_latency_client,
        url,
        state.config.embedding_backfill_open_p95_limit_ms,
        state.config.embedding_backfill_search_p95_limit_ms,
    )
    .await
    {
        Ok(decision) => {
            let result = if decision.allow_backfill {
                "allow"
            } else {
                "pause"
            };
            metrics::counter!(
                "simple.embedding_backfill.foreground_guard",
                "result" => result,
                "reason" => decision.reason.clone()
            )
            .increment(1);
            if !decision.allow_backfill {
                tracing::warn!(
                    reason = decision.reason,
                    open_p95_ms = ?decision.open_p95_ms,
                    search_p95_ms = ?decision.search_p95_ms,
                    "embedding backfill paused by foreground latency guard"
                );
            }
            decision.allow_backfill
        }
        Err(error) => {
            metrics::counter!(
                "simple.embedding_backfill.foreground_guard",
                "result" => "pause",
                "reason" => "snapshot_error"
            )
            .increment(1);
            tracing::warn!(
                %error,
                "embedding backfill paused because the foreground latency snapshot is unavailable"
            );
            false
        }
    }
}

async fn fetch_foreground_guard(
    client: &reqwest::Client,
    url: &str,
    open_limit_ms: f64,
    search_limit_ms: f64,
) -> Result<ForegroundGuardDecision, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("foreground latency request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "foreground latency endpoint returned {}",
            response.status()
        ));
    }
    let snapshot = response
        .json::<ForegroundLatencySnapshot>()
        .await
        .map_err(|error| format!("foreground latency response was invalid: {error}"))?;
    validate_foreground_snapshot(&snapshot)?;
    Ok(evaluate_guard(&snapshot, open_limit_ms, search_limit_ms))
}

fn validate_foreground_snapshot(snapshot: &ForegroundLatencySnapshot) -> Result<(), String> {
    if snapshot.schema != SNAPSHOT_SCHEMA || snapshot.window_seconds != 60 {
        return Err("foreground latency snapshot schema or window is invalid".to_owned());
    }
    validate_operation_snapshot("open", &snapshot.open)?;
    validate_operation_snapshot("search", &snapshot.search)?;
    let now_ms: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let max_age_ms: u64 = MAX_FOREGROUND_SNAPSHOT_AGE
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let max_future_ms: u64 = MAX_FOREGROUND_SNAPSHOT_FUTURE_SKEW
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    if now_ms.saturating_sub(snapshot.generated_at_unix_ms) > max_age_ms {
        return Err("foreground latency snapshot is stale".to_owned());
    }
    if snapshot.generated_at_unix_ms.saturating_sub(now_ms) > max_future_ms {
        return Err("foreground latency snapshot is from the future".to_owned());
    }
    Ok(())
}

fn validate_operation_snapshot(
    operation: &str,
    snapshot: &crate::foreground_latency::OperationLatencySnapshot,
) -> Result<(), String> {
    if snapshot.samples == 0 {
        if snapshot.p95_ms.is_some() || snapshot.newest_sample_age_ms.is_some() {
            return Err(format!(
                "{operation} foreground latency snapshot has values without samples"
            ));
        }
        return Ok(());
    }
    let p95_ms = snapshot
        .p95_ms
        .ok_or_else(|| format!("{operation} foreground latency snapshot omitted p95"))?;
    if !p95_ms.is_finite() || p95_ms < 0.0 {
        return Err(format!(
            "{operation} foreground latency snapshot has invalid p95"
        ));
    }
    let newest_sample_age_ms = snapshot
        .newest_sample_age_ms
        .ok_or_else(|| format!("{operation} foreground latency snapshot omitted sample age"))?;
    if newest_sample_age_ms > 60_000 {
        return Err(format!(
            "{operation} foreground latency snapshot has an expired sample"
        ));
    }
    Ok(())
}

async fn process_embedding_batch(
    state: &AppState,
    pool: &PgPool,
    jobs: Vec<Job>,
) -> ApiResult<bool> {
    let mut valid_jobs = Vec::with_capacity(jobs.len());
    for job in jobs {
        let valid = payload_uuid(&job.payload, "entry_id").is_ok()
            && payload_i64(&job.payload, "version").is_ok_and(|version| version > 0);
        if valid {
            valid_jobs.push(job);
        } else {
            fail_permanently(pool, &job, "invalid embedding job payload").await?;
        }
    }
    if valid_jobs.is_empty() {
        return Ok(true);
    }
    let started = Instant::now();
    let result = embed_entries(state, &valid_jobs).await;
    let outcome = if result.is_ok() { "complete" } else { "retry" };
    metrics::counter!(
        "simple.jobs.processed",
        "kind" => "embed_entry",
        "result" => outcome
    )
    .increment(valid_jobs.len() as u64);
    metrics::histogram!(
        "simple.jobs.duration_ms",
        "kind" => "embed_entry",
        "result" => outcome
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!(
        "simple.jobs.batch_size",
        "kind" => "embed_entry",
        "result" => outcome
    )
    .record(valid_jobs.len() as f64);
    match result {
        Ok(()) => complete_many(pool, &valid_jobs).await?,
        Err(error) => {
            tracing::warn!(
                jobs = valid_jobs.len(),
                error = %error,
                "simple workspace embedding batch failed"
            );
            for job in &valid_jobs {
                retry_or_fail(pool, job, &error).await?;
            }
        }
    }
    Ok(true)
}

async fn mark_exhausted(pool: &PgPool) -> ApiResult<()> {
    sqlx::query(
        r#"
        UPDATE straylight.jobs
        SET status='queued',attempts=0,started_at=NULL,finished_at=NULL,
            available_at=clock_timestamp() + interval '5 minutes'
        WHERE kind IN ('embed_entry','describe_binary')
          AND status IN ('queued','running')
          AND attempts >= $1
          AND (
            status='queued'
            OR started_at < clock_timestamp() - make_interval(mins => $2)
          )
        "#,
    )
    .bind(MAX_ATTEMPTS)
    .bind(LEASE_MINUTES)
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        WITH candidates AS (
          SELECT id
          FROM straylight.jobs
          WHERE kind IN ('embed_entry','describe_binary')
            AND status='failed'
            AND coalesce(last_error,'') NOT LIKE 'permanent:%'
            AND finished_at < clock_timestamp() - interval '5 minutes'
          ORDER BY finished_at,id
          LIMIT 16
          FOR UPDATE SKIP LOCKED
        )
        UPDATE straylight.jobs AS job
        SET status='queued',attempts=0,available_at=clock_timestamp(),
            started_at=NULL,finished_at=NULL
        FROM candidates
        WHERE job.id=candidates.id
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn claim_priority(pool: &PgPool) -> ApiResult<Option<Job>> {
    let row = sqlx::query(
        r#"
        WITH candidate AS (
          SELECT id
          FROM straylight.jobs
          WHERE kind='describe_binary'
            AND attempts < $1
            AND status='queued'
            AND available_at <= CURRENT_TIMESTAMP
          ORDER BY available_at,created_at,id
          FOR UPDATE SKIP LOCKED
          LIMIT 1
        )
        UPDATE straylight.jobs AS job
        SET status='running',attempts=job.attempts + 1,
            started_at=clock_timestamp(),finished_at=NULL,last_error=NULL
        FROM candidate
        WHERE job.id=candidate.id
        RETURNING job.id,job.user_id,job.kind,job.payload,job.attempts
        "#,
    )
    .bind(MAX_ATTEMPTS)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| Job {
        id: row.get("id"),
        user_id: row.get("user_id"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
    }))
}

async fn claim_stale_priority(pool: &PgPool) -> ApiResult<Option<Job>> {
    let row = sqlx::query(
        r#"
        WITH candidate AS (
          SELECT id
          FROM straylight.jobs
          WHERE kind='describe_binary'
            AND status='running'
            AND attempts < $1
            AND started_at < CURRENT_TIMESTAMP - make_interval(mins => $2)
          ORDER BY started_at,created_at,id
          FOR UPDATE SKIP LOCKED
          LIMIT 1
        )
        UPDATE straylight.jobs AS job
        SET status='running',attempts=job.attempts + 1,
            started_at=clock_timestamp(),finished_at=NULL,last_error=NULL
        FROM candidate
        WHERE job.id=candidate.id
        RETURNING job.id,job.user_id,job.kind,job.payload,job.attempts
        "#,
    )
    .bind(MAX_ATTEMPTS)
    .bind(LEASE_MINUTES)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| Job {
        id: row.get("id"),
        user_id: row.get("user_id"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
    }))
}

async fn claim_embedding(pool: &PgPool) -> ApiResult<Option<Job>> {
    let row = sqlx::query(
        r#"
        WITH candidate AS (
          SELECT id
          FROM straylight.jobs
          WHERE kind='embed_entry'
            AND attempts < $1
            AND status='queued'
            AND available_at <= CURRENT_TIMESTAMP
          ORDER BY available_at DESC,created_at DESC,id DESC
          FOR UPDATE SKIP LOCKED
          LIMIT 1
        )
        UPDATE straylight.jobs AS job
        SET status='running',attempts=job.attempts + 1,
            started_at=clock_timestamp(),finished_at=NULL,last_error=NULL
        FROM candidate
        WHERE job.id=candidate.id
        RETURNING job.id,job.user_id,job.kind,job.payload,job.attempts
        "#,
    )
    .bind(MAX_ATTEMPTS)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| Job {
        id: row.get("id"),
        user_id: row.get("user_id"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
    }))
}

async fn claim_stale_embedding(pool: &PgPool) -> ApiResult<Option<Job>> {
    let row = sqlx::query(
        r#"
        WITH candidate AS (
          SELECT id
          FROM straylight.jobs
          WHERE kind='embed_entry'
            AND status='running'
            AND attempts < $1
            AND started_at < CURRENT_TIMESTAMP - make_interval(mins => $2)
          ORDER BY started_at,created_at,id
          FOR UPDATE SKIP LOCKED
          LIMIT 1
        )
        UPDATE straylight.jobs AS job
        SET status='running',attempts=job.attempts + 1,
            started_at=clock_timestamp(),finished_at=NULL,last_error=NULL
        FROM candidate
        WHERE job.id=candidate.id
        RETURNING job.id,job.user_id,job.kind,job.payload,job.attempts
        "#,
    )
    .bind(MAX_ATTEMPTS)
    .bind(LEASE_MINUTES)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| Job {
        id: row.get("id"),
        user_id: row.get("user_id"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
    }))
}

async fn claim_more_embeddings(pool: &PgPool, limit: i64) -> ApiResult<Vec<Job>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
          SELECT id
          FROM straylight.jobs
          WHERE kind='embed_entry'
            AND attempts < $1
            AND status='queued'
            AND available_at <= CURRENT_TIMESTAMP
          ORDER BY available_at DESC,created_at DESC,id DESC
          FOR UPDATE SKIP LOCKED
          LIMIT $2
        )
        UPDATE straylight.jobs AS job
        SET status='running',attempts=job.attempts + 1,
            started_at=clock_timestamp(),finished_at=NULL,last_error=NULL
        FROM candidates
        WHERE job.id=candidates.id
        RETURNING job.id,job.user_id,job.kind,job.payload,job.attempts
        "#,
    )
    .bind(MAX_ATTEMPTS)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Job {
            id: row.get("id"),
            user_id: row.get("user_id"),
            kind: row.get("kind"),
            payload: row.get("payload"),
            attempts: row.get("attempts"),
        })
        .collect())
}

async fn complete(pool: &PgPool, job: &Job) -> ApiResult<()> {
    if matches!(job.kind.as_str(), "embed_entry" | "describe_binary") {
        sqlx::query(
            "DELETE FROM straylight.jobs \
             WHERE id=$1 AND user_id=$2 AND status='running'",
        )
        .bind(job.id)
        .bind(job.user_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            r#"
            UPDATE straylight.jobs
            SET status='complete',finished_at=clock_timestamp(),last_error=NULL
            WHERE id=$1 AND user_id=$2 AND status='running'
            "#,
        )
        .bind(job.id)
        .bind(job.user_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn complete_many(pool: &PgPool, jobs: &[Job]) -> ApiResult<()> {
    let ids = jobs.iter().map(|job| job.id).collect::<Vec<_>>();
    sqlx::query(
        "DELETE FROM straylight.jobs \
         WHERE id=ANY($1) AND kind='embed_entry' AND status='running'",
    )
    .bind(&ids)
    .execute(pool)
    .await?;
    Ok(())
}

async fn retry_or_fail(pool: &PgPool, job: &Job, error: &ApiError) -> ApiResult<()> {
    let repairable_enrichment = matches!(job.kind.as_str(), "embed_entry" | "describe_binary");
    let permanent_input_error = matches!(
        error,
        ApiError::Public {
            status: axum::http::StatusCode::BAD_REQUEST,
            ..
        }
    );
    if permanent_input_error {
        return fail_permanently(pool, job, &sanitize_error(error)).await;
    }
    let failed = !repairable_enrichment && job.attempts >= MAX_ATTEMPTS;
    let reset_attempts = repairable_enrichment && job.attempts >= MAX_ATTEMPTS;
    let delay_seconds = retry_delay_seconds(job.attempts);
    let message = sanitize_error(error);
    sqlx::query(
        r#"
        UPDATE straylight.jobs
        SET status=CASE WHEN $3 THEN 'failed' ELSE 'queued' END,
            attempts=CASE WHEN $6 THEN 0 ELSE attempts END,
            available_at=CASE
              WHEN $3 THEN available_at
              ELSE clock_timestamp() + make_interval(secs => $4)
            END,
            started_at=NULL,
            finished_at=CASE WHEN $3 THEN clock_timestamp() ELSE NULL END,
            last_error=$5
        WHERE id=$1 AND user_id=$2 AND status='running'
        "#,
    )
    .bind(job.id)
    .bind(job.user_id)
    .bind(failed)
    .bind(delay_seconds)
    .bind(message)
    .bind(reset_attempts)
    .execute(pool)
    .await?;
    metrics::counter!(
        "simple.jobs.transitions",
        "kind" => job.kind.clone(),
        "result" => if failed {
            "failed"
        } else if reset_attempts {
            "repair_queued"
        } else {
            "retry"
        }
    )
    .increment(1);
    Ok(())
}

async fn fail_permanently(pool: &PgPool, job: &Job, message: &str) -> ApiResult<()> {
    let message = format!(
        "permanent: {}",
        message.chars().take(980).collect::<String>()
    );
    sqlx::query(
        r#"
        UPDATE straylight.jobs
        SET status='failed',started_at=NULL,finished_at=clock_timestamp(),
            last_error=$3
        WHERE id=$1 AND user_id=$2 AND status='running'
        "#,
    )
    .bind(job.id)
    .bind(job.user_id)
    .bind(message)
    .execute(pool)
    .await?;
    metrics::counter!(
        "simple.jobs.transitions",
        "kind" => job.kind.clone(),
        "result" => "permanent_failure"
    )
    .increment(1);
    Ok(())
}

fn retry_delay_seconds(attempts: i32) -> f64 {
    let exponent = attempts.clamp(1, 8) as u32;
    f64::from(2_u32.pow(exponent).min(300))
}

fn sanitize_error(error: &ApiError) -> String {
    error
        .to_string()
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(1_000)
        .collect()
}

async fn embed_entries(state: &AppState, jobs: &[Job]) -> ApiResult<()> {
    let pool = state
        .admin_pool
        .as_ref()
        .expect("checked before claiming a job");
    let mut job_ids = Vec::with_capacity(jobs.len());
    let mut user_ids = Vec::with_capacity(jobs.len());
    let mut entry_ids = Vec::with_capacity(jobs.len());
    let mut versions = Vec::with_capacity(jobs.len());
    for job in jobs {
        job_ids.push(job.id);
        user_ids.push(job.user_id);
        entry_ids.push(payload_uuid(&job.payload, "entry_id")?);
        versions.push(payload_i64(&job.payload, "version")?);
    }
    let batch_chunks = state
        .config
        .embedding_backfill_batch_chunks
        .min(MAX_EMBED_CHUNKS_PER_PUBLICATION);
    loop {
        let rows = sqlx::query(
            r#"
            WITH requested AS (
              SELECT *
              FROM unnest($1::uuid[],$2::uuid[],$3::uuid[],$4::bigint[])
                AS item(job_id,user_id,entry_id,version)
            )
            SELECT requested.user_id,requested.entry_id,requested.version,
                   chunk.id AS chunk_id,chunk.content
            FROM requested
            JOIN straylight.entries AS entry
              ON requested.user_id=entry.user_id
             AND requested.entry_id=entry.id
             AND requested.version=entry.current_version
            JOIN straylight.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            JOIN straylight.search_chunks AS chunk
              ON chunk.user_id=entry.user_id
             AND chunk.entry_id=entry.id
             AND chunk.entry_version_id=version.id
            WHERE entry.deleted_at IS NULL
              AND chunk.embedding IS NULL
            ORDER BY requested.job_id,chunk.chunk_index
            LIMIT $5
            "#,
        )
        .bind(&job_ids)
        .bind(&user_ids)
        .bind(&entry_ids)
        .bind(&versions)
        .bind(i64::try_from(batch_chunks).map_err(|_| {
            ApiError::configuration("embedding backfill batch size is out of range")
        })?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        let chunk_ids = rows
            .iter()
            .map(|row| row.get::<Uuid, _>("chunk_id"))
            .collect::<Vec<_>>();
        let inputs = rows
            .iter()
            .map(|row| row.get::<String, _>("content"))
            .collect::<Vec<_>>();
        let vectors = embed_indexing_inputs(state.embedder.as_ref(), &inputs).await?;
        if vectors.len() != chunk_ids.len() {
            return Err(ApiError::Internal(
                "embedding provider returned an unexpected result shape".to_owned(),
            ));
        }
        let updates = rows
            .iter()
            .zip(chunk_ids)
            .zip(vectors)
            .map(|((row, chunk_id), vector)| {
                (
                    row.get::<Uuid, _>("user_id"),
                    row.get::<Uuid, _>("entry_id"),
                    chunk_id,
                    row.get::<i64, _>("version"),
                    Vector::from(vector),
                )
            })
            .collect::<Vec<_>>();
        // Each semantic batch is independently visible and independently
        // retryable. A crash resumes from chunks that still lack vectors.
        let mut statement = QueryBuilder::<Postgres>::new(
            "WITH updates(user_id,entry_id,chunk_id,version,embedding) AS (",
        );
        statement.push_values(
            updates,
            |mut row, (user_id, entry_id, chunk_id, version, embedding)| {
                row.push_bind(user_id)
                    .push_bind(entry_id)
                    .push_bind(chunk_id)
                    .push_bind(version)
                    .push_bind(embedding);
            },
        );
        statement.push(
            r#"
            )
            UPDATE straylight.search_chunks AS chunk
            SET embedding=updates.embedding
            FROM updates
            JOIN straylight.entries AS entry
              ON entry.user_id=updates.user_id
             AND entry.id=updates.entry_id
             AND entry.current_version=updates.version
             AND entry.deleted_at IS NULL
            WHERE chunk.user_id=updates.user_id
              AND chunk.entry_id=updates.entry_id
              AND chunk.id=updates.chunk_id
              AND chunk.embedding IS NULL
            "#,
        );
        statement.build().execute(pool).await?;
        if rows.len() == batch_chunks {
            tokio::time::sleep(state.config.embedding_backfill_inter_batch_delay).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
    Ok(())
}

async fn describe_binary(state: &AppState, job: &Job) -> ApiResult<()> {
    let entry_id = payload_uuid(&job.payload, "entry_id")?;
    let version = payload_i64(&job.payload, "version")?;
    let companion_path = payload_string(&job.payload, "companion_path")?;
    let pool = state
        .admin_pool
        .as_ref()
        .expect("checked before claiming a job");
    let row = sqlx::query(
        r#"
        SELECT entry.path,entry.media_type,entry.current_version,
               version.content_sha256,version.size_bytes,version.object_key,
               version.object_version_id,version.metadata,
               companion.current_version AS companion_version,
               companion_current.metadata AS companion_metadata
        FROM straylight.entries AS entry
        JOIN straylight.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        LEFT JOIN straylight.entries AS companion
          ON companion.user_id=entry.user_id
         AND lower(companion.path)=lower($4)
         AND companion.kind='markdown'
         AND companion.deleted_at IS NULL
        LEFT JOIN straylight.entry_versions AS companion_current
          ON companion_current.user_id=companion.user_id
         AND companion_current.entry_id=companion.id
         AND companion_current.version=companion.current_version
        WHERE entry.user_id=$1 AND entry.id=$2
          AND entry.current_version=$3
          AND entry.kind='binary'
          AND entry.deleted_at IS NULL
        "#,
    )
    .bind(job.user_id)
    .bind(entry_id)
    .bind(version)
    .bind(&companion_path)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let content_sha256: String = row.get("content_sha256");
    let companion_metadata: Option<Value> = row.get("companion_metadata");
    let expected_content_hash = format!("sha256:{content_sha256}");
    if companion_metadata.as_ref().is_some_and(|metadata| {
        metadata
            .get("description_status")
            .and_then(Value::as_str)
            .is_some_and(|status| status != "pending")
            && metadata.get("content_hash").and_then(Value::as_str)
                == Some(expected_content_hash.as_str())
    }) {
        return Ok(());
    }
    let metadata: Value = row.get("metadata");
    if metadata
        .get("description_status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "pending")
    {
        return Ok(());
    }
    let path: String = row.get("path");
    let media_type: String = row.get("media_type");
    let size_bytes: i64 = row.get("size_bytes");
    let object_key: String = row.get("object_key");
    let object_version_id: Option<String> = row.get("object_version_id");
    let content_complete = usize::try_from(size_bytes)
        .ok()
        .is_some_and(|size| size <= WORKSPACE_MODEL_FILE_BYTES);
    let bytes = if content_complete {
        let bytes = state
            .object_store
            .get_version(&object_key, object_version_id.as_deref())
            .await?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != content_sha256 {
            return Err(ApiError::conflict(
                "content_hash_mismatch",
                "stored binary bytes no longer match their immutable hash",
                serde_json::json!({"entry_ref": format!("entry:{entry_id}")}),
            ));
        }
        bytes
    } else {
        state
            .object_store
            .get_prefix_version(
                &object_key,
                object_version_id.as_deref(),
                asset_profiles::MAX_INSPECT_BYTES,
            )
            .await?
    };
    let description = asset_description::describe_workspace_binary(
        state,
        job.user_id,
        entry_id,
        version,
        path,
        media_type,
        size_bytes,
        content_sha256,
        object_key,
        object_version_id,
        bytes,
        content_complete,
    )
    .await;
    let expected_companion_version = row.get::<Option<i64>, _>("companion_version");
    match simple_core::write_markdown_as_worker(
        state,
        job.user_id,
        companion_path,
        description.content,
        description.metadata,
        expected_companion_version,
        Some((entry_id, version)),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(ApiError::Public {
            code: "entry_version_conflict",
            ..
        }) => Ok(()),
        Err(error) => Err(error),
    }
}

fn payload_uuid(payload: &Value, field: &'static str) -> ApiResult<Uuid> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ApiError::invalid(format!("simple job payload requires {field}")))
}

fn payload_i64(payload: &Value, field: &'static str) -> ApiResult<i64> {
    payload
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::invalid(format!("simple job payload requires {field}")))
}

fn payload_string(payload: &Value, field: &'static str) -> ApiResult<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| ApiError::invalid(format!("simple job payload requires {field}")))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use axum::{Json, Router, routing::get};
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::{
        MAX_EMBED_BATCH_JOBS, MAX_EMBED_CHUNKS_PER_PUBLICATION, claim_embedding,
        claim_more_embeddings, claim_priority, fetch_foreground_guard, retry_delay_seconds,
        sanitize_error, validate_foreground_snapshot,
    };
    use crate::{
        error::ApiError,
        foreground_latency::{
            ForegroundLatencySnapshot, OperationLatencySnapshot, SNAPSHOT_SCHEMA,
        },
    };

    #[test]
    fn retry_delay_is_bounded() {
        assert_eq!(retry_delay_seconds(1), 2.0);
        assert_eq!(retry_delay_seconds(5), 32.0);
        assert_eq!(retry_delay_seconds(100), 256.0);
    }

    #[test]
    fn stored_errors_are_bounded() {
        let error = ApiError::Internal("x".repeat(2_000));
        assert_eq!(sanitize_error(&error).chars().count(), 1_000);
    }

    #[test]
    fn embedding_batches_remain_bounded() {
        assert_eq!(MAX_EMBED_BATCH_JOBS, 8);
        assert_eq!(MAX_EMBED_CHUNKS_PER_PUBLICATION, 64);
    }

    fn foreground_snapshot(
        open_p95_ms: Option<f64>,
        search_p95_ms: Option<f64>,
    ) -> ForegroundLatencySnapshot {
        ForegroundLatencySnapshot {
            schema: SNAPSHOT_SCHEMA.to_owned(),
            generated_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .try_into()
                .unwrap(),
            window_seconds: 60,
            open: OperationLatencySnapshot {
                samples: usize::from(open_p95_ms.is_some()),
                p95_ms: open_p95_ms,
                newest_sample_age_ms: Some(0),
            },
            search: OperationLatencySnapshot {
                samples: usize::from(search_p95_ms.is_some()),
                p95_ms: search_p95_ms,
                newest_sample_age_ms: Some(0),
            },
        }
    }

    #[tokio::test]
    async fn foreground_guard_reads_another_process_over_http_and_pauses() {
        let snapshot = foreground_snapshot(Some(80.0), Some(180.0));
        let app = Router::new().route(
            "/health/foreground-latency",
            get(move || {
                let snapshot = snapshot.clone();
                async move { Json(snapshot) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let decision = fetch_foreground_guard(
            &client,
            &format!("http://{address}/health/foreground-latency"),
            120.0,
            107.0,
        )
        .await
        .unwrap();
        assert!(!decision.allow_backfill);
        assert_eq!(decision.reason, "search_p95_exceeded");
        server.abort();
    }

    #[test]
    fn stale_cross_process_snapshot_fails_closed() {
        let mut snapshot = foreground_snapshot(Some(10.0), Some(10.0));
        snapshot.generated_at_unix_ms = snapshot.generated_at_unix_ms.saturating_sub(11_000);
        assert_eq!(
            validate_foreground_snapshot(&snapshot).unwrap_err(),
            "foreground latency snapshot is stale"
        );
    }

    #[test]
    fn malformed_cross_process_operation_snapshot_fails_closed() {
        let mut snapshot = foreground_snapshot(Some(10.0), Some(10.0));
        snapshot.open.samples = 0;
        assert_eq!(
            validate_foreground_snapshot(&snapshot).unwrap_err(),
            "open foreground latency snapshot has values without samples"
        );
    }

    #[tokio::test]
    async fn priority_jobs_precede_backlog_and_embeddings_prefer_recent_writes() {
        let Ok(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL") else {
            eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping worker priority test");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .unwrap();
        let user_id = Uuid::now_v7();
        let dream_id = Uuid::now_v7();
        let description_id = Uuid::now_v7();
        let embedding_ids = (0..130).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL session_replication_role='replica'")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)",
        )
        .bind(user_id)
        .bind(format!("worker-priority-test:{user_id}"))
        .bind("Worker priority test")
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO straylight.jobs (
              id,user_id,kind,status,payload,available_at,created_at
            )
            SELECT id,$2,'embed_entry','queued','{}'::jsonb,
                   clock_timestamp() + interval '100 milliseconds',
                   '2000-01-01 00:00:00+00'::timestamptz
                     + (ordinality * interval '1 second')
            FROM unnest($1::uuid[]) WITH ORDINALITY AS queued(id,ordinality)
            "#,
        )
        .bind(&embedding_ids)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO straylight.jobs (
              id,user_id,kind,status,payload,available_at,created_at
            ) VALUES
              (
                $1,$3,'describe_binary','queued','{}'::jsonb,
                '2020-01-01 00:00:00+00'::timestamptz,
                '2020-01-01 00:00:00+00'::timestamptz
              ),
              (
                $2,$3,'describe_binary','queued','{}'::jsonb,
                '2020-01-02 00:00:00+00'::timestamptz,
                '2020-01-02 00:00:00+00'::timestamptz
              )
            "#,
        )
        .bind(dream_id)
        .bind(description_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let first = claim_priority(&pool).await.unwrap().unwrap();
        let second = claim_priority(&pool).await.unwrap().unwrap();
        assert_eq!(first.id, dream_id); // earliest-created priority job
        assert_eq!(second.id, description_id);

        tokio::time::sleep(std::time::Duration::from_millis(125)).await;
        let first_embedding = claim_embedding(&pool).await.unwrap().unwrap();
        assert_eq!(first_embedding.id, *embedding_ids.last().unwrap());
        let mut embedding_batch = vec![first_embedding];
        embedding_batch.extend(
            claim_more_embeddings(&pool, MAX_EMBED_BATCH_JOBS - 1)
                .await
                .unwrap(),
        );
        assert_eq!(embedding_batch.len(), MAX_EMBED_BATCH_JOBS as usize);
        assert!(embedding_batch.iter().all(|job| job.kind == "embed_entry"));
        let remaining = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.jobs \
             WHERE user_id=$1 AND kind='embed_entry' AND status='queued'",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            remaining,
            i64::try_from(embedding_ids.len()).unwrap() - MAX_EMBED_BATCH_JOBS
        );

        let mut cleanup = pool.begin().await.unwrap();
        sqlx::query("DELETE FROM straylight.jobs WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *cleanup)
            .await
            .unwrap();
        sqlx::query("DELETE FROM straylight.users WHERE id=$1")
            .bind(user_id)
            .execute(&mut *cleanup)
            .await
            .unwrap();
        cleanup.commit().await.unwrap();
    }
}

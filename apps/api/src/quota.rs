use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
};

const TEMPORARY_EXPORT_MIN_OVERHEAD_BYTES: i64 = 64 * 1024 * 1024;
const TEMPORARY_EXPORT_MAX_OVERHEAD_BYTES: i64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub struct UsageReservation {
    pub embedding_input_chars: u64,
    pub model_input_chars: u64,
    pub model_output_tokens: u64,
    pub compute_rows: u64,
}

impl UsageReservation {
    fn is_empty(self) -> bool {
        self.embedding_input_chars == 0
            && self.model_input_chars == 0
            && self.model_output_tokens == 0
            && self.compute_rows == 0
    }
}

pub async fn reserve_for_auth(
    state: &AppState,
    auth: &AuthContext,
    reservation: UsageReservation,
) -> ApiResult<()> {
    if reservation.is_empty() {
        return Ok(());
    }
    let mut tx = state.begin_write(auth).await?;
    reserve_in_tx(&mut tx, auth.user_id.0, reservation).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn reserve_for_user(
    pool: &PgPool,
    user_id: Uuid,
    reservation: UsageReservation,
) -> ApiResult<()> {
    if reservation.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    reserve_in_tx(&mut tx, user_id, reservation).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn reserve_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    reservation: UsageReservation,
) -> ApiResult<()> {
    let account = sqlx::query(
        r#"
        SELECT account_status, daily_embedding_input_chars,
               daily_model_input_chars, daily_model_output_tokens,
               daily_compute_rows
        FROM brunn.users
        WHERE id=$1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("user_not_found", &format!("user:{user_id}")))?;
    let account_status: String = account.try_get("account_status")?;
    if account_status != "active" {
        return Err(account_locked(&account_status));
    }

    sqlx::query(
        r#"
        INSERT INTO brunn.user_usage_daily (user_id,usage_date)
        VALUES ($1,(clock_timestamp() AT TIME ZONE 'UTC')::date)
        ON CONFLICT (user_id,usage_date) DO NOTHING
        "#,
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await?;

    let updated = sqlx::query(
        r#"
        UPDATE brunn.user_usage_daily
        SET embedding_input_chars=embedding_input_chars+$2,
            model_input_chars=model_input_chars+$3,
            model_output_tokens=model_output_tokens+$4,
            compute_rows=compute_rows+$5,
            updated_at=clock_timestamp()
        WHERE user_id=$1
          AND usage_date=(clock_timestamp() AT TIME ZONE 'UTC')::date
          AND embedding_input_chars+$2 <= $6
          AND model_input_chars+$3 <= $7
          AND model_output_tokens+$4 <= $8
          AND compute_rows+$5 <= $9
        RETURNING user_id
        "#,
    )
    .bind(user_id)
    .bind(as_i64(reservation.embedding_input_chars))
    .bind(as_i64(reservation.model_input_chars))
    .bind(as_i64(reservation.model_output_tokens))
    .bind(as_i64(reservation.compute_rows))
    .bind(account.try_get::<i64, _>("daily_embedding_input_chars")?)
    .bind(account.try_get::<i64, _>("daily_model_input_chars")?)
    .bind(account.try_get::<i64, _>("daily_model_output_tokens")?)
    .bind(account.try_get::<i64, _>("daily_compute_rows")?)
    .fetch_optional(&mut **tx)
    .await?;
    if updated.is_none() {
        metrics::counter!("quota.denials", "quota" => reservation.primary_kind()).increment(1);
        return Err(ApiError::with_details(
            http::StatusCode::TOO_MANY_REQUESTS,
            "quota_exceeded",
            "the user daily usage budget is exhausted",
            json!({
                "resets_at": "next UTC day",
                "reservation": {
                    "embedding_input_chars": reservation.embedding_input_chars,
                    "model_input_chars": reservation.model_input_chars,
                    "model_output_tokens": reservation.model_output_tokens,
                    "compute_rows": reservation.compute_rows
                }
            }),
        ));
    }
    metrics::counter!("quota.reservations", "quota" => reservation.primary_kind()).increment(1);
    Ok(())
}

pub async fn ensure_storage_capacity(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    pending_blobs: &[(String, usize)],
) -> ApiResult<()> {
    let pending_objects = pending_blobs
        .iter()
        .map(|(hash, size)| (format!("{user_id}/blobs/{hash}"), *size))
        .collect::<Vec<_>>();
    ensure_storage_capacity_for_objects(tx, user_id, &pending_objects).await
}

pub async fn ensure_storage_capacity_for_objects(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    pending_objects: &[(String, usize)],
) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query("SELECT account_status,storage_limit_bytes FROM brunn.users WHERE id=$1")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::not_found("user_not_found", &format!("user:{user_id}")))?;
    let account_status: String = row.try_get("account_status")?;
    if account_status != "active" {
        return Err(account_locked(&account_status));
    }
    let storage_limit: i64 = row.try_get("storage_limit_bytes")?;
    let current_usage = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT coalesce(sum(stored.size_bytes),0)::bigint
        FROM (
          SELECT object_key,max(size_bytes)::bigint AS size_bytes
          FROM (
            SELECT object_key,size_bytes
            FROM brunn.asset_versions
            WHERE user_id=$1 AND size_bytes > 0
          ) AS physical_objects
          GROUP BY object_key
        ) AS stored
        "#,
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    let keys = pending_objects
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let existing: Vec<String> = if keys.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            r#"
            SELECT DISTINCT existing.object_key
            FROM (
              SELECT object_key
              FROM brunn.asset_versions
              WHERE user_id=$1
            ) AS existing
            WHERE existing.object_key=ANY($2::text[])
            "#,
        )
        .bind(user_id)
        .bind(&keys)
        .fetch_all(&mut **tx)
        .await?
    };
    let existing: std::collections::HashSet<_> = existing.into_iter().collect();
    let pending_sizes = pending_objects.iter().fold(
        std::collections::HashMap::<&str, usize>::new(),
        |mut sizes, (key, size)| {
            sizes
                .entry(key.as_str())
                .and_modify(|current| *current = (*current).max(*size))
                .or_insert(*size);
            sizes
        },
    );
    let additional = pending_sizes
        .into_iter()
        .filter(|(key, _)| !existing.contains(*key))
        .map(|(_, size)| i64::try_from(size).unwrap_or(i64::MAX))
        .fold(0_i64, i64::saturating_add);
    if current_usage.saturating_add(additional) > storage_limit {
        metrics::counter!("quota.denials", "quota" => "storage").increment(1);
        return Err(ApiError::with_details(
            http::StatusCode::PAYLOAD_TOO_LARGE,
            "storage_quota_exceeded",
            "the upload would exceed the user storage budget",
            json!({
                "current_bytes": current_usage,
                "additional_bytes": additional,
                "limit_bytes": storage_limit
            }),
        ));
    }
    Ok(())
}

// One active export plus its TTL bounds object count and lifetime; this cap
// bounds temporary bytes without charging the archive against durable content.
pub async fn ensure_temporary_export_capacity(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    export_size_bytes: u64,
) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query("SELECT account_status,storage_limit_bytes FROM brunn.users WHERE id=$1")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::not_found("user_not_found", &format!("user:{user_id}")))?;
    let account_status: String = row.try_get("account_status")?;
    if account_status != "active" {
        return Err(account_locked(&account_status));
    }
    let storage_limit: i64 = row.try_get("storage_limit_bytes")?;
    let temporary_limit = temporary_export_limit(storage_limit);
    let export_size = i64::try_from(export_size_bytes).unwrap_or(i64::MAX);
    if export_size > temporary_limit {
        metrics::counter!("quota.denials", "quota" => "temporary_export").increment(1);
        return Err(ApiError::with_details(
            http::StatusCode::PAYLOAD_TOO_LARGE,
            "account_export_temporary_limit_exceeded",
            "the account export exceeds the bounded temporary export allowance",
            json!({
                "export_bytes": export_size,
                "temporary_limit_bytes": temporary_limit,
                "storage_limit_bytes": storage_limit
            }),
        ));
    }
    Ok(())
}

fn temporary_export_limit(storage_limit: i64) -> i64 {
    let proportional_overhead = storage_limit.saturating_div(10).clamp(
        TEMPORARY_EXPORT_MIN_OVERHEAD_BYTES,
        TEMPORARY_EXPORT_MAX_OVERHEAD_BYTES,
    );
    storage_limit.saturating_add(proportional_overhead)
}

impl UsageReservation {
    fn primary_kind(self) -> &'static str {
        if self.model_input_chars > 0 || self.model_output_tokens > 0 {
            "model"
        } else if self.embedding_input_chars > 0 {
            "embedding"
        } else {
            "compute"
        }
    }
}

fn as_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn account_locked(status: &str) -> ApiError {
    ApiError::with_details(
        http::StatusCode::LOCKED,
        "account_locked",
        "the account is not accepting new mutations",
        json!({"account_status": status}),
    )
}

#[derive(Clone)]
pub struct RequestRateLimiter {
    inner: Arc<Mutex<HashMap<Uuid, RequestWindow>>>,
    limit: u32,
}

#[derive(Clone)]
pub struct PreauthRateLimiter {
    inner: Arc<Mutex<RequestWindow>>,
    limit: u32,
}

#[derive(Clone, Copy)]
struct RequestWindow {
    started_at: Instant,
    count: u32,
}

impl RequestRateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            limit: limit.max(1),
        }
    }

    pub fn check(&self, credential_id: Uuid) -> ApiResult<()> {
        let now = Instant::now();
        let mut windows = self.inner.lock().expect("request limiter lock poisoned");
        if windows.len() > 10_000 {
            windows.retain(|_, window| {
                now.duration_since(window.started_at) < Duration::from_secs(60)
            });
        }
        let window = windows.entry(credential_id).or_insert(RequestWindow {
            started_at: now,
            count: 0,
        });
        if now.duration_since(window.started_at) >= Duration::from_secs(60) {
            *window = RequestWindow {
                started_at: now,
                count: 0,
            };
        }
        if window.count >= self.limit {
            metrics::counter!("quota.denials", "quota" => "request_rate").increment(1);
            return Err(ApiError::with_details(
                http::StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_exceeded",
                "the credential request-rate budget is exhausted",
                json!({"retry_after_seconds": 60}),
            ));
        }
        window.count += 1;
        Ok(())
    }
}

impl PreauthRateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RequestWindow {
                started_at: Instant::now(),
                count: 0,
            })),
            limit: limit.max(1),
        }
    }

    pub fn check(&self) -> ApiResult<()> {
        let now = Instant::now();
        let mut window = self.inner.lock().expect("preauth limiter lock poisoned");
        if now.duration_since(window.started_at) >= Duration::from_secs(60) {
            *window = RequestWindow {
                started_at: now,
                count: 0,
            };
        }
        if window.count >= self.limit {
            metrics::counter!("quota.denials", "quota" => "preauth_rate").increment(1);
            return Err(ApiError::with_details(
                http::StatusCode::TOO_MANY_REQUESTS,
                "preauth_rate_limit_exceeded",
                "the service authentication-rate budget is exhausted",
                json!({"retry_after_seconds": 60}),
            ));
        }
        window.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_rate_limits_are_credential_scoped_and_resettable() {
        let limiter = RequestRateLimiter::new(2);
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        assert!(limiter.check(first).is_ok());
        assert!(limiter.check(first).is_ok());
        assert!(limiter.check(first).is_err());
        assert!(limiter.check(second).is_ok());
    }

    #[test]
    fn preauth_rate_limit_bounds_database_amplification() {
        let limiter = PreauthRateLimiter::new(2);
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_err());
    }

    #[test]
    fn temporary_export_allowance_is_bounded_above_durable_storage() {
        let ten_gib = 10 * 1024 * 1024 * 1024;
        assert_eq!(
            temporary_export_limit(ten_gib),
            ten_gib + TEMPORARY_EXPORT_MAX_OVERHEAD_BYTES
        );

        let small_limit = 128 * 1024 * 1024;
        assert_eq!(
            temporary_export_limit(small_limit),
            small_limit + TEMPORARY_EXPORT_MIN_OVERHEAD_BYTES
        );

        assert_eq!(
            temporary_export_limit(i64::MAX),
            i64::MAX,
            "temporary allowance must saturate instead of wrapping"
        );
    }
}

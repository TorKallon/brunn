//! One-way Todoist Unified API v1 ingestion.
//!
//! The HTTP surface in this module is deliberately read-only. Todoist's
//! read-only Sync operation itself uses POST, but this client never accepts or
//! emits a `commands` form field and has no task mutation methods.

use std::{collections::HashSet, fmt, time::Duration};

use chrono::{
    DateTime, Duration as ChronoDuration, LocalResult, NaiveDate, NaiveDateTime, SecondsFormat,
    TimeZone, Utc,
};
use chrono_tz::Tz;
use futures::StreamExt;
use regex::Regex;
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{PgPool, Postgres, Row, Transaction};
use url::form_urlencoded;
use uuid::Uuid;

use crate::{
    db::AppState,
    error::{ApiError, ApiResult},
    secret_service, task_service,
};

const TODOIST_SYNC_URL: &str = "https://api.todoist.com/api/v1/sync";
const TODOIST_COMPLETED_URL: &str =
    "https://api.todoist.com/api/v1/tasks/completed/by_completion_date";
const TODOIST_RESOURCE_TYPES: &str = r#"["projects","items"]"#;
const MAX_SYNC_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMPLETED_PAGES: usize = 50;
const MAX_COMPLETION_WINDOWS: usize = 512;
const TODOIST_COMPLETED_PAGE_SIZE: &str = "200";
const TODOIST_COMPLETION_OVERLAP: ChronoDuration = ChronoDuration::minutes(10);
const TODOIST_COMPLETION_MAX_RANGE: ChronoDuration = ChronoDuration::days(89);
pub(crate) const TODOIST_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const TODOIST_LEASE_DURATION: Duration = Duration::from_secs(2 * 60);

/// Secret-bearing token wrapper. It intentionally implements neither Display,
/// Serialize, nor Clone, and its Debug representation is always redacted.
pub(crate) struct TodoistToken(String);

impl TodoistToken {
    pub(crate) fn from_secret(value: String) -> ApiResult<Self> {
        let value = value.trim().to_owned();
        if value.is_empty()
            || value.len() > 4_096
            || value.chars().any(char::is_control)
            || value.chars().any(char::is_whitespace)
        {
            return Err(ApiError::configuration(
                "the configured Todoist token is invalid",
            ));
        }
        Ok(Self(value))
    }

    fn bearer_header(&self) -> Result<HeaderValue, TodoistClientError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.0))
            .map_err(|_| TodoistClientError::new("todoist_token_invalid"))?;
        value.set_sensitive(true);
        Ok(value)
    }
}

impl fmt::Debug for TodoistToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[REDACTED]")
    }
}

fn todoist_endpoints() -> ApiResult<(reqwest::Url, reqwest::Url, bool)> {
    #[cfg(feature = "todoist-fixture")]
    if let Ok(raw_origin) = std::env::var("BRUNN_TODOIST_FIXTURE_ORIGIN") {
        if std::env::var("BRUNN_ENV").as_deref() == Ok("production") {
            return Err(ApiError::configuration(
                "Todoist fixture transport is forbidden in production",
            ));
        }
        let mut origin = reqwest::Url::parse(raw_origin.trim())
            .map_err(|_| ApiError::configuration("Todoist fixture origin is invalid"))?;
        let loopback = origin
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
        if origin.scheme() != "http"
            || !loopback
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !matches!(origin.path(), "" | "/")
        {
            return Err(ApiError::configuration(
                "Todoist fixture origin must be an uncredentialed loopback HTTP origin",
            ));
        }
        origin.set_path("/");
        let sync = origin
            .join("api/v1/sync")
            .map_err(|_| ApiError::configuration("Todoist fixture origin is invalid"))?;
        let completed = origin
            .join("api/v1/tasks/completed/by_completion_date")
            .map_err(|_| ApiError::configuration("Todoist fixture origin is invalid"))?;
        return Ok((sync, completed, true));
    }

    let sync = reqwest::Url::parse(TODOIST_SYNC_URL)
        .map_err(|_| ApiError::configuration("Todoist sync endpoint is invalid"))?;
    let completed = reqwest::Url::parse(TODOIST_COMPLETED_URL)
        .map_err(|_| ApiError::configuration("Todoist completion endpoint is invalid"))?;
    Ok((sync, completed, false))
}

#[derive(Clone)]
pub(crate) struct TodoistClient {
    http: Client,
    sync_url: reqwest::Url,
    completed_url: reqwest::Url,
}

impl TodoistClient {
    pub(crate) fn new() -> ApiResult<Self> {
        let (sync_url, completed_url, fixture_transport) = todoist_endpoints()?;
        let http = Client::builder()
            .https_only(!fixture_transport)
            .timeout(Duration::from_secs(30))
            .user_agent("Brunn-Todoist-Sync/1")
            .build()
            .map_err(|_| ApiError::configuration("could not initialize Todoist sync client"))?;
        Ok(Self {
            http,
            sync_url,
            completed_url,
        })
    }

    /// Fetches only projects and items through Todoist's read-only Sync
    /// operation. The opaque cursor is never included in Debug output.
    pub(crate) async fn sync(
        &self,
        token: &TodoistToken,
        sync_token: Option<&str>,
    ) -> Result<TodoistSyncResponse, TodoistClientError> {
        let request = self.sync_request(token, sync_token)?;
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|_| TodoistClientError::new("todoist_network_error"))?;
        if !response.status().is_success() {
            let retry_after = parse_retry_after(response.headers().get(RETRY_AFTER));
            return Err(TodoistClientError {
                code: status_error_code(response.status()),
                retry_after,
            });
        }
        decode_json_response(response).await
    }

    /// Reads completed tasks over Todoist's bounded completion-time endpoint.
    /// This is the polling-side evidence that distinguishes a recurring
    /// completion from an ordinary due-date edit. It follows opaque pagination
    /// cursors but exposes no mutation surface.
    pub(crate) async fn completed_by_completion_date(
        &self,
        token: &TodoistToken,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<TodoistCompletedOccurrence>, TodoistClientError> {
        if since >= until || until.signed_duration_since(since) > TODOIST_COMPLETION_MAX_RANGE {
            return Err(TodoistClientError::new("todoist_completion_window_invalid"));
        }
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        let mut seen_occurrences = HashSet::new();
        let mut completed = Vec::new();
        for _ in 0..MAX_COMPLETED_PAGES {
            let request = self.completed_request(token, since, until, cursor.as_deref())?;
            let response = self
                .http
                .execute(request)
                .await
                .map_err(|_| TodoistClientError::new("todoist_network_error"))?;
            if !response.status().is_success() {
                let retry_after = parse_retry_after(response.headers().get(RETRY_AFTER));
                return Err(TodoistClientError {
                    code: status_error_code(response.status()),
                    retry_after,
                });
            }
            let page: TodoistCompletedTasksResponse = decode_json_response(response).await?;
            for item in page.items {
                validate_required_remote_text("completed task id", &item.id, 512)
                    .map_err(|_| TodoistClientError::new("todoist_response_invalid"))?;
                let completed_at = item
                    .completed_at
                    .ok_or_else(|| TodoistClientError::new("todoist_response_invalid"))?;
                let occurrence_key = item
                    .due
                    .as_ref()
                    .filter(|due| due.is_recurring)
                    .map(|due| {
                        validate_required_remote_text("completed occurrence key", &due.date, 128)
                            .map_err(|_| TodoistClientError::new("todoist_response_invalid"))?;
                        canonical_todoist_occurrence_key(&due.date)
                            .map_err(|_| TodoistClientError::new("todoist_response_invalid"))
                    })
                    .transpose()?;
                let identity = (item.id.clone(), completed_at, occurrence_key.clone());
                if seen_occurrences.insert(identity) {
                    completed.push(TodoistCompletedOccurrence {
                        external_id: item.id,
                        completed_at,
                        occurrence_key,
                    });
                }
            }
            let Some(next_cursor) = page.next_cursor else {
                return Ok(completed);
            };
            validate_cursor(&next_cursor)?;
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(TodoistClientError::new("todoist_cursor_invalid"));
            }
            cursor = Some(next_cursor);
        }
        Err(TodoistClientError::new("todoist_response_too_large"))
    }

    fn sync_request(
        &self,
        token: &TodoistToken,
        sync_token: Option<&str>,
    ) -> Result<reqwest::Request, TodoistClientError> {
        let cursor = sync_token.unwrap_or("*");
        validate_cursor(cursor)?;
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("sync_token", cursor)
            .append_pair("resource_types", TODOIST_RESOURCE_TYPES)
            .finish();
        self.http
            .post(self.sync_url.clone())
            .header(AUTHORIZATION, token.bearer_header()?)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .build()
            .map_err(|_| TodoistClientError::new("todoist_request_invalid"))
    }

    fn completed_request(
        &self,
        token: &TodoistToken,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
        cursor: Option<&str>,
    ) -> Result<reqwest::Request, TodoistClientError> {
        if let Some(cursor) = cursor {
            validate_cursor(cursor)?;
        }
        let mut url = self.completed_url.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("since", &since.to_rfc3339_opts(SecondsFormat::Secs, true))
                .append_pair("until", &until.to_rfc3339_opts(SecondsFormat::Secs, true))
                .append_pair("limit", TODOIST_COMPLETED_PAGE_SIZE);
            if let Some(cursor) = cursor {
                query.append_pair("cursor", cursor);
            }
        }
        self.http
            .get(url)
            .header(AUTHORIZATION, token.bearer_header()?)
            .header(ACCEPT, "application/json")
            .build()
            .map_err(|_| TodoistClientError::new("todoist_request_invalid"))
    }
}

async fn decode_json_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, TodoistClientError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| TodoistClientError::new("todoist_network_error"))?;
        if body.len().saturating_add(chunk.len()) > MAX_SYNC_RESPONSE_BYTES {
            return Err(TodoistClientError::new("todoist_response_too_large"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| TodoistClientError::new("todoist_response_invalid"))
}

fn validate_cursor(cursor: &str) -> Result<(), TodoistClientError> {
    if cursor.is_empty() || cursor.len() > 16_384 || cursor.chars().any(char::is_control) {
        return Err(TodoistClientError::new("todoist_cursor_invalid"));
    }
    Ok(())
}

pub(crate) async fn fetch_logical_pull(
    client: &TodoistClient,
    token: &TodoistToken,
    cursor: Option<&str>,
    last_run_at: Option<DateTime<Utc>>,
) -> Result<FetchedTodoistSync, TodoistClientError> {
    let first = client.sync(token, cursor).await?;
    if first.sync_token.is_empty() || first.sync_token.len() > 16_384 {
        return Err(TodoistClientError::new("todoist_cursor_invalid"));
    }
    if cursor.is_none() && !first.full_sync {
        return Err(TodoistClientError::new("todoist_full_sync_missing"));
    }
    let (responses, final_cursor) = if cursor.is_some() {
        let final_cursor = first.sync_token.clone();
        (vec![first], final_cursor)
    } else {
        // Todoist documents that a full-sync snapshot may lag writes. An
        // immediate incremental fetch closes that window before any local
        // state is committed.
        let second = client.sync(token, Some(&first.sync_token)).await?;
        if second.full_sync || second.sync_token.is_empty() || second.sync_token.len() > 16_384 {
            return Err(TodoistClientError::new("todoist_cursor_invalid"));
        }
        let final_cursor = second.sync_token.clone();
        (vec![first, second], final_cursor)
    };
    let completion_until = Utc::now();
    let default_since = completion_until - TODOIST_COMPLETION_OVERLAP;
    let requested_since = last_run_at
        .map(|value| value - TODOIST_COMPLETION_OVERLAP)
        .unwrap_or(default_since);
    let completion_since = requested_since.min(default_since);
    let completion_windows = todoist_completion_windows(completion_since, completion_until)?;
    let mut completed_occurrences = Vec::new();
    let mut seen_occurrences = HashSet::new();
    for (since, until) in completion_windows {
        for completed in client
            .completed_by_completion_date(token, since, until)
            .await?
        {
            if seen_occurrences.insert((
                completed.external_id.clone(),
                completed.completed_at,
                completed.occurrence_key.clone(),
            )) {
                completed_occurrences.push(completed);
            }
        }
    }
    Ok(FetchedTodoistSync {
        responses,
        final_cursor,
        completed_occurrences,
        completion_watermark: completion_until,
    })
}

fn todoist_completion_windows(
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>, TodoistClientError> {
    if since >= until {
        return Err(TodoistClientError::new("todoist_completion_window_invalid"));
    }
    let mut windows = Vec::new();
    let mut start = since;
    while start < until {
        if windows.len() == MAX_COMPLETION_WINDOWS {
            // Fail the logical pull without advancing the Sync cursor instead
            // of silently discarding completion history.
            return Err(TodoistClientError::new(
                "todoist_completion_history_too_large",
            ));
        }
        let end = (start + TODOIST_COMPLETION_MAX_RANGE).min(until);
        windows.push((start, end));
        start = end;
    }
    Ok(windows)
}

pub(crate) struct FetchedTodoistSync {
    pub(crate) responses: Vec<TodoistSyncResponse>,
    pub(crate) final_cursor: String,
    pub(crate) completed_occurrences: Vec<TodoistCompletedOccurrence>,
    pub(crate) completion_watermark: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub struct TodoistClientError {
    pub(crate) code: &'static str,
    pub(crate) retry_after: Option<Duration>,
}

impl TodoistClientError {
    const fn new(code: &'static str) -> Self {
        Self {
            code,
            retry_after: None,
        }
    }

    #[doc(hidden)]
    pub const fn bounded_for_contract_test(code: &'static str) -> Self {
        Self::new(code)
    }
}

impl fmt::Display for TodoistClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for TodoistClientError {}

#[doc(hidden)]
pub struct TodoistSyncClaim {
    pub(crate) user_id: Uuid,
    pub(crate) configuration_generation: i64,
    pub(crate) cursor: Option<String>,
    pub(crate) last_run_at: Option<DateTime<Utc>>,
    pub(crate) timezone: Tz,
    pub(crate) lease_owner: String,
}

/// Clears all queued or leased work that is not currently eligible. This runs
/// before every claim so disabling either gate never accumulates a backlog.
pub(crate) async fn clear_ineligible_sync_work(
    pool: &PgPool,
    environment_enabled: bool,
) -> ApiResult<u64> {
    let result = if environment_enabled {
        sqlx::query(
            r#"
            UPDATE brunn.task_sync_state AS state
            SET next_run_at=NULL,manual_requested_at=NULL,
                lease_owner=NULL,lease_expires_at=NULL,
                updated_at=clock_timestamp()
            FROM brunn.task_integration_config AS config,
                 brunn.users AS account
            WHERE state.user_id=config.user_id
              AND state.system='todoist' AND config.system='todoist'
              AND account.id=state.user_id
              AND (
                config.mode='off'
                OR account.account_status<>'active'
                OR NOT EXISTS (
                  SELECT 1 FROM brunn.secrets AS secret
                  WHERE secret.user_id=state.user_id
                    AND secret.name='todoist-api-token'
                )
              )
              AND (
                state.next_run_at IS NOT NULL
                OR state.manual_requested_at IS NOT NULL
                OR state.lease_owner IS NOT NULL
              )
            "#,
        )
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE brunn.task_sync_state
            SET next_run_at=NULL,manual_requested_at=NULL,
                lease_owner=NULL,lease_expires_at=NULL,
                updated_at=clock_timestamp()
            WHERE system='todoist'
              AND (
                next_run_at IS NOT NULL
                OR manual_requested_at IS NOT NULL
                OR lease_owner IS NOT NULL
              )
            "#,
        )
        .execute(pool)
        .await?
    };
    Ok(result.rows_affected())
}

#[doc(hidden)]
pub async fn claim_next_sync(
    pool: &PgPool,
    environment_enabled: bool,
    worker_id: &str,
) -> ApiResult<Option<TodoistSyncClaim>> {
    if !environment_enabled {
        clear_ineligible_sync_work(pool, false).await?;
        return Ok(None);
    }
    if worker_id.is_empty() || worker_id.len() > 200 || worker_id.chars().any(char::is_control) {
        return Err(ApiError::configuration(
            "Todoist worker identity is invalid",
        ));
    }
    clear_ineligible_sync_work(pool, true).await?;
    let lease_seconds = i64::try_from(TODOIST_LEASE_DURATION.as_secs()).unwrap_or(120);
    let row = sqlx::query(
        r#"
        WITH candidate AS (
          SELECT state.user_id
          FROM brunn.task_sync_state AS state
          JOIN brunn.task_integration_config AS config
            ON config.user_id=state.user_id AND config.system=state.system
          JOIN brunn.users AS account ON account.id=state.user_id
          WHERE state.system='todoist'
            AND account.account_status='active'
            AND config.mode IN ('import_once','pull')
            AND EXISTS (
              SELECT 1 FROM brunn.secrets AS secret
              WHERE secret.user_id=state.user_id
                AND secret.name='todoist-api-token'
            )
            AND (
              state.lease_owner IS NULL
              OR state.lease_expires_at <= clock_timestamp()
            )
            AND (
              state.configuration_generation<>config.configuration_generation
              OR state.manual_requested_at IS NOT NULL
              OR (
                config.mode='pull'
                AND (state.next_run_at IS NULL
                     OR state.next_run_at <= clock_timestamp())
              )
              OR (
                config.mode='import_once'
                AND state.last_outcome IS DISTINCT FROM 'success'
                AND (state.next_run_at IS NULL
                     OR state.next_run_at <= clock_timestamp())
              )
            )
          ORDER BY
            (state.manual_requested_at IS NULL),
            COALESCE(state.manual_requested_at,state.next_run_at,'epoch'),
            state.user_id
          FOR UPDATE OF state SKIP LOCKED
          LIMIT 1
        )
        UPDATE brunn.task_sync_state AS state
        SET configuration_generation=config.configuration_generation,
            -- A mode-generation change is not an upstream identity reset.
            -- Keeping the opaque cursor is required to receive completion and
            -- deletion tombstones that occurred while saved mode was off.
            cursor=state.cursor,
            last_run_at=CASE
              WHEN state.configuration_generation<>config.configuration_generation
                THEN NULL ELSE state.last_run_at END,
            last_outcome=CASE
              WHEN state.configuration_generation<>config.configuration_generation
                THEN NULL ELSE state.last_outcome END,
            last_error_code=CASE
              WHEN state.configuration_generation<>config.configuration_generation
                THEN NULL ELSE state.last_error_code END,
            lease_owner=$1,
            lease_expires_at=clock_timestamp()+make_interval(secs=>$2),
            updated_at=clock_timestamp()
        FROM candidate,
             brunn.task_integration_config AS config,
             brunn.task_settings AS settings
        WHERE state.user_id=candidate.user_id
          AND state.system='todoist'
          AND config.user_id=state.user_id AND config.system=state.system
          AND settings.user_id=state.user_id
        RETURNING state.user_id,state.configuration_generation,
                  state.cursor,state.last_run_at,settings.timezone,state.lease_owner
        "#,
    )
    .bind(worker_id)
    .bind(lease_seconds)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let timezone_name: String = row.try_get("timezone")?;
    let timezone = timezone_name
        .parse::<Tz>()
        .map_err(|_| ApiError::Internal("stored task timezone is invalid".to_owned()))?;
    Ok(Some(TodoistSyncClaim {
        user_id: row.try_get("user_id")?,
        configuration_generation: row.try_get("configuration_generation")?,
        cursor: row.try_get("cursor")?,
        last_run_at: row.try_get("last_run_at")?,
        timezone,
        lease_owner: row.try_get("lease_owner")?,
    }))
}

#[doc(hidden)]
pub async fn finish_sync_failure(
    pool: &PgPool,
    claim: &TodoistSyncClaim,
    error: TodoistClientError,
) -> ApiResult<()> {
    let retry_after = error.retry_after.unwrap_or(TODOIST_POLL_INTERVAL);
    let retry_seconds = i64::try_from(
        retry_after
            .max(Duration::from_secs(30))
            .min(Duration::from_secs(15 * 60))
            .as_secs(),
    )
    .unwrap_or(5 * 60);
    sqlx::query(
        r#"
        UPDATE brunn.task_sync_state AS state
        SET last_outcome='error',last_error_code=$4,
            next_run_at=clock_timestamp()+make_interval(secs=>$5),
            manual_requested_at=NULL,
            lease_owner=NULL,lease_expires_at=NULL,
            updated_at=clock_timestamp()
        FROM brunn.task_integration_config AS config
        WHERE state.user_id=$1 AND state.system='todoist'
          AND state.configuration_generation=$2
          AND state.lease_owner=$3
          AND config.user_id=state.user_id AND config.system=state.system
          AND config.mode IN ('import_once','pull')
          AND config.configuration_generation=state.configuration_generation
        "#,
    )
    .bind(claim.user_id)
    .bind(claim.configuration_generation)
    .bind(&claim.lease_owner)
    .bind(error.code)
    .bind(retry_seconds)
    .execute(pool)
    .await?;
    Ok(())
}

#[doc(hidden)]
pub async fn finish_sync_success_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    claim: &TodoistSyncClaim,
    final_cursor: &str,
    completion_watermark: DateTime<Utc>,
) -> ApiResult<()> {
    if final_cursor.is_empty()
        || final_cursor.len() > 16_384
        || final_cursor.chars().any(char::is_control)
    {
        return Err(ApiError::invalid("Todoist returned an invalid sync cursor"));
    }
    let row = sqlx::query(
        r#"
        UPDATE brunn.task_sync_state AS state
        SET cursor=$4,last_run_at=$6,last_outcome='success',
            last_error_code=NULL,
            next_run_at=CASE WHEN config.mode='pull'
              THEN clock_timestamp()+make_interval(secs=>$5)
              ELSE NULL END,
            manual_requested_at=NULL,
            lease_owner=NULL,lease_expires_at=NULL,
            updated_at=clock_timestamp()
        FROM brunn.task_integration_config AS config
        WHERE state.user_id=$1 AND state.system='todoist'
          AND state.configuration_generation=$2
          AND state.lease_owner=$3
          AND config.user_id=state.user_id AND config.system=state.system
          AND config.mode IN ('import_once','pull')
          AND config.configuration_generation=state.configuration_generation
        RETURNING state.user_id
        "#,
    )
    .bind(claim.user_id)
    .bind(claim.configuration_generation)
    .bind(&claim.lease_owner)
    .bind(final_cursor)
    .bind(i64::try_from(TODOIST_POLL_INTERVAL.as_secs()).unwrap_or(300))
    .bind(completion_watermark)
    .fetch_optional(&mut **tx)
    .await?;
    if row.is_none() {
        return Err(ApiError::conflict(
            "todoist_sync_superseded",
            "Todoist configuration changed while the pull was running",
            serde_json::json!({"configuration_generation": claim.configuration_generation}),
        ));
    }
    Ok(())
}

/// Processes one eligible logical pull. Upstream failures are recorded as
/// bounded operational codes and count as handled work; database/configuration
/// failures outside an upstream apply are returned to the worker loop.
/// Canonical task writes and cursor advancement commit in one administrative
/// transaction. Content-bearing apply errors never cross the worker/logging
/// boundary after their bounded durable failure has been recorded.
pub(crate) async fn process_next(state: &AppState, worker_id: &str) -> ApiResult<bool> {
    let pool = state
        .admin_pool
        .as_ref()
        .ok_or_else(|| ApiError::configuration("DATABASE_URL_ADMIN is required by Todoist sync"))?;
    let Some(claim) = claim_next_sync(pool, state.config.todoist_sync_enabled, worker_id).await?
    else {
        return Ok(false);
    };
    let secret = match secret_service::todoist_token_for_worker(state, claim.user_id).await {
        Ok(Some(secret)) => secret,
        Ok(None) => {
            // The token was removed after the lease was claimed. Re-evaluate
            // eligibility immediately so no lease or manual backlog remains.
            clear_ineligible_sync_work(pool, true).await?;
            return Ok(true);
        }
        Err(error) => {
            finish_sync_failure(
                pool,
                &claim,
                TodoistClientError::new("todoist_secret_unavailable"),
            )
            .await?;
            return Err(error);
        }
    };
    let client = TodoistClient::new()?;
    let fetched = match fetch_logical_pull(
        &client,
        &secret.token,
        claim.cursor.as_deref(),
        claim.last_run_at,
    )
    .await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            let code = error.code;
            finish_sync_failure(pool, &claim, error).await?;
            metrics::counter!("todoist.sync.runs", "result" => "upstream_error", "code" => code)
                .increment(1);
            return Ok(true);
        }
    };

    let mut tx = pool.begin().await?;
    if task_service::apply_todoist_sync_in_tx(
        &mut tx,
        claim.user_id,
        secret.producer_credential_id,
        claim.timezone,
        &fetched.responses,
        &fetched.completed_occurrences,
    )
    .await
    .is_err()
    {
        tx.rollback().await?;
        finish_sync_failure(
            pool,
            &claim,
            TodoistClientError::new("todoist_apply_rejected"),
        )
        .await?;
        metrics::counter!(
            "todoist.sync.runs",
            "result" => "apply_error",
            "code" => "todoist_apply_rejected"
        )
        .increment(1);
        return Ok(true);
    }
    if let Err(error) = finish_sync_success_in_tx(
        &mut tx,
        &claim,
        &fetched.final_cursor,
        fetched.completion_watermark,
    )
    .await
    {
        tx.rollback().await?;
        if matches!(
            &error,
            ApiError::Public {
                code: "todoist_sync_superseded",
                ..
            }
        ) {
            return Ok(true);
        }
        return Err(error);
    }
    tx.commit().await?;
    metrics::counter!("todoist.sync.runs", "result" => "success").increment(1);
    Ok(true)
}

fn status_error_code(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "todoist_auth_rejected",
        StatusCode::TOO_MANY_REQUESTS => "todoist_rate_limited",
        status if status.is_server_error() => "todoist_unavailable",
        _ => "todoist_request_rejected",
    }
}

fn parse_retry_after(value: Option<&HeaderValue>) -> Option<Duration> {
    let seconds = value?.to_str().ok()?.parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds.min(15 * 60)))
}

#[derive(Deserialize)]
#[doc(hidden)]
pub struct TodoistSyncResponse {
    pub(crate) sync_token: String,
    #[serde(default)]
    pub(crate) full_sync: bool,
    #[serde(default)]
    pub(crate) projects: Vec<TodoistProject>,
    #[serde(default)]
    pub(crate) items: Vec<TodoistItem>,
}

#[derive(Deserialize)]
struct TodoistCompletedTasksResponse {
    #[serde(default)]
    items: Vec<TodoistItem>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[doc(hidden)]
pub struct TodoistCompletedOccurrence {
    pub external_id: String,
    pub completed_at: DateTime<Utc>,
    pub occurrence_key: Option<String>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct TodoistProject {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) is_deleted: bool,
}

#[derive(Clone, Deserialize)]
pub(crate) struct TodoistItem {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) project_id: String,
    #[serde(default)]
    pub(crate) labels: Vec<String>,
    #[serde(default = "default_todoist_priority")]
    pub(crate) priority: u8,
    #[serde(default)]
    pub(crate) checked: bool,
    #[serde(default)]
    pub(crate) is_deleted: bool,
    #[serde(default)]
    pub(crate) completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) due: Option<TodoistDue>,
    #[serde(default)]
    pub(crate) deadline: Option<TodoistDeadline>,
}

const fn default_todoist_priority() -> u8 {
    1
}

#[derive(Clone, Deserialize)]
pub(crate) struct TodoistDue {
    pub(crate) date: String,
    #[serde(default)]
    pub(crate) string: String,
    #[serde(default)]
    pub(crate) lang: String,
    #[serde(default)]
    pub(crate) is_recurring: bool,
    #[serde(default)]
    pub(crate) timezone: Option<String>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct TodoistDeadline {
    pub(crate) date: String,
    #[serde(default)]
    pub(crate) lang: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct MappedRecurrence {
    pub(crate) recurrence_source: &'static str,
    pub(crate) original: String,
    pub(crate) lang: String,
    pub(crate) series_id: String,
    pub(crate) due: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rrule: Option<String>,
    pub(crate) needs_review: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TodoistTerminal {
    Open,
    Completed,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MappedTodoistItem {
    pub(crate) external_id: String,
    pub(crate) title: String,
    pub(crate) notes: String,
    pub(crate) project_id: String,
    pub(crate) labels: Vec<String>,
    pub(crate) soft_due: Option<NaiveDate>,
    pub(crate) hard_due: Option<DateTime<Utc>>,
    pub(crate) hard_due_note: Option<&'static str>,
    pub(crate) recurrence: Option<MappedRecurrence>,
    pub(crate) occurrence_key: Option<String>,
    pub(crate) completed_at: Option<DateTime<Utc>>,
    pub(crate) terminal: TodoistTerminal,
    pub(crate) needs_triage: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NextTodoistOccurrence {
    pub(crate) occurrence_key: String,
    pub(crate) soft_due: NaiveDate,
    pub(crate) due_instant: DateTime<Utc>,
}

pub(crate) fn map_item(item: &TodoistItem, owner_timezone: Tz) -> ApiResult<MappedTodoistItem> {
    validate_required_remote_text("task id", &item.id, 512)?;
    let terminal = if item.checked || item.completed_at.is_some() {
        TodoistTerminal::Completed
    } else if item.is_deleted {
        TodoistTerminal::Deleted
    } else {
        TodoistTerminal::Open
    };
    if terminal == TodoistTerminal::Deleted {
        validate_remote_text("task title", &item.content, 10_000, false)?;
        validate_remote_text("project id", &item.project_id, 512, false)?;
    } else {
        validate_required_remote_text("task title", &item.content, 10_000)?;
        validate_required_remote_text("project id", &item.project_id, 512)?;
    }
    validate_remote_text("task description", &item.description, 100_000, true)?;
    if item.priority > 4 {
        return Err(ApiError::invalid("Todoist priority is outside 1..=4"));
    }
    let mut labels = item
        .labels
        .iter()
        .map(|label| {
            validate_required_remote_text("Todoist label", label, 255)?;
            Ok(label.trim().to_owned())
        })
        .collect::<ApiResult<Vec<_>>>()?;
    labels.sort_by_key(|label| label.to_lowercase());
    labels.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    if let Some(due) = &item.due {
        validate_required_remote_text("Todoist due date", &due.date, 128)?;
        validate_remote_text("Todoist due expression", &due.string, 512, false)?;
        validate_remote_text("Todoist due language", &due.lang, 32, false)?;
        if let Some(timezone) = &due.timezone {
            validate_required_remote_text("Todoist due timezone", timezone, 80)?;
        }
    }
    if let Some(deadline) = &item.deadline {
        validate_required_remote_text("Todoist deadline", &deadline.date, 10)?;
        validate_remote_text("Todoist deadline language", &deadline.lang, 32, false)?;
    }
    let soft_due = item.due.as_ref().map(due_date).transpose()?;
    let hard_label = labels
        .iter()
        .any(|label| label.eq_ignore_ascii_case("hard"));
    let (hard_due, hard_due_note) = if let Some(deadline) = &item.deadline {
        let date = NaiveDate::parse_from_str(&deadline.date, "%Y-%m-%d")
            .map_err(|_| ApiError::invalid("Todoist deadline must be YYYY-MM-DD"))?;
        (
            Some(local_end_of_day(date, owner_timezone)?),
            Some("todoist_deadline"),
        )
    } else if item.priority == 4 && item.due.is_some() {
        (
            Some(due_instant(
                item.due.as_ref().expect("checked due"),
                owner_timezone,
            )?),
            Some("todoist_priority_p1"),
        )
    } else if hard_label && item.due.is_some() {
        (
            Some(due_instant(
                item.due.as_ref().expect("checked due"),
                owner_timezone,
            )?),
            Some("todoist_hard_label"),
        )
    } else {
        (None, None)
    };
    let recurrence = item
        .due
        .as_ref()
        .filter(|due| due.is_recurring)
        .map(|due| MappedRecurrence {
            recurrence_source: "todoist",
            original: due.string.clone(),
            lang: due.lang.clone(),
            series_id: item.id.clone(),
            due: due.date.clone(),
            timezone: due.timezone.clone(),
            rrule: todoist_recurrence_to_rrule(&due.string, &due.lang),
            needs_review: todoist_recurrence_to_rrule(&due.string, &due.lang).is_none(),
        });
    let occurrence_key = recurrence
        .as_ref()
        .map(|_| {
            canonical_todoist_occurrence_key(
                &item.due.as_ref().expect("recurrence requires due").date,
            )
        })
        .transpose()?;
    let needs_triage = (hard_label && item.due.is_none())
        || recurrence
            .as_ref()
            .is_some_and(|recurrence| recurrence.needs_review);

    Ok(MappedTodoistItem {
        external_id: item.id.clone(),
        title: item.content.trim().to_owned(),
        notes: item.description.clone(),
        project_id: item.project_id.clone(),
        labels,
        soft_due,
        hard_due,
        hard_due_note,
        recurrence,
        occurrence_key,
        completed_at: item.completed_at,
        terminal,
        needs_triage,
    })
}

pub(crate) fn next_todoist_occurrence(
    recurrence: &MappedRecurrence,
    owner_timezone: Tz,
) -> ApiResult<Option<NextTodoistOccurrence>> {
    let Some(rule) = recurrence.rrule.as_deref() else {
        return Ok(None);
    };
    let timezone = recurrence
        .timezone
        .as_deref()
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(owner_timezone);
    enum Representation {
        AllDay,
        Floating,
        Fixed,
    }
    let (local_start, representation) = if let Ok(value) =
        DateTime::parse_from_rfc3339(&recurrence.due)
    {
        (
            value.with_timezone(&timezone).naive_local(),
            Representation::Fixed,
        )
    } else if let Ok(value) = NaiveDateTime::parse_from_str(&recurrence.due, "%Y-%m-%dT%H:%M:%S%.f")
    {
        (value, Representation::Floating)
    } else {
        let date = NaiveDate::parse_from_str(&recurrence.due, "%Y-%m-%d")
            .map_err(|_| ApiError::invalid("Todoist recurrence due date is invalid"))?;
        (
            date.and_hms_opt(0, 0, 0)
                .ok_or_else(|| ApiError::invalid("Todoist recurrence due date is invalid"))?,
            Representation::AllDay,
        )
    };
    let recurrence_timezone = rrule::Tz::from(timezone);
    let start = match recurrence_timezone.from_local_datetime(&local_start) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.max(second),
        LocalResult::None => return Ok(None),
    };
    let expanded = crate::recurrence::expand(
        start,
        &[rule.to_owned()],
        vec![],
        Some(start.with_timezone(&Utc) + chrono::Duration::seconds(1)),
        None,
        1,
    )?;
    let Some(next) = expanded.dates.into_iter().next() else {
        return Ok(None);
    };
    let local = next.with_timezone(&timezone);
    let soft_due = local.date_naive();
    let (occurrence_key, due_instant) = match representation {
        Representation::AllDay => (
            soft_due.format("%Y-%m-%d").to_string(),
            local_end_of_day(soft_due, timezone)?,
        ),
        Representation::Floating => (
            local.format("%Y-%m-%dT%H:%M:%S").to_string(),
            local.with_timezone(&Utc),
        ),
        Representation::Fixed => (
            local
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            local.with_timezone(&Utc),
        ),
    };
    Ok(Some(NextTodoistOccurrence {
        occurrence_key,
        soft_due,
        due_instant,
    }))
}

fn canonical_todoist_occurrence_key(value: &str) -> ApiResult<String> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date.format("%Y-%m-%d").to_string());
    }
    if let Ok(fixed) = DateTime::parse_from_rfc3339(value) {
        return Ok(fixed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true));
    }
    if let Ok(floating) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f") {
        return Ok(floating.format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    Err(ApiError::invalid(
        "Todoist recurrence occurrence identity is invalid",
    ))
}

fn validate_remote_text(
    name: &str,
    value: &str,
    max_chars: usize,
    allow_markdown_whitespace: bool,
) -> ApiResult<()> {
    if value.chars().count() > max_chars
        || value.chars().any(|character| {
            character.is_control()
                && !(allow_markdown_whitespace && matches!(character, '\n' | '\r' | '\t'))
        })
    {
        return Err(ApiError::invalid(format!("{name} is invalid")));
    }
    Ok(())
}

fn validate_required_remote_text(name: &str, value: &str, max_chars: usize) -> ApiResult<()> {
    validate_remote_text(name, value, max_chars, false)?;
    if value.trim().is_empty() {
        return Err(ApiError::invalid(format!("{name} must not be empty")));
    }
    Ok(())
}

fn due_date(due: &TodoistDue) -> ApiResult<NaiveDate> {
    due.date
        .get(..10)
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .ok_or_else(|| ApiError::invalid("Todoist due date is invalid"))
}

fn due_instant(due: &TodoistDue, owner_timezone: Tz) -> ApiResult<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&due.date) {
        return Ok(value.with_timezone(&Utc));
    }
    if let Ok(value) = NaiveDateTime::parse_from_str(&due.date, "%Y-%m-%dT%H:%M:%S") {
        let timezone = due
            .timezone
            .as_deref()
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(owner_timezone);
        return resolve_local(value, timezone, "Todoist due time");
    }
    local_end_of_day(due_date(due)?, owner_timezone)
}

pub(crate) fn local_end_of_day(date: NaiveDate, timezone: Tz) -> ApiResult<DateTime<Utc>> {
    let local = date
        .and_hms_opt(23, 59, 59)
        .ok_or_else(|| ApiError::invalid("Todoist deadline date is invalid"))?;
    resolve_local(local, timezone, "Todoist deadline")
}

fn resolve_local(local: NaiveDateTime, timezone: Tz, field: &str) -> ApiResult<DateTime<Utc>> {
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        // Prefer the later physical instant so an ambiguous civil deadline is
        // never silently moved earlier.
        LocalResult::Ambiguous(first, second) => Ok(first.max(second).with_timezone(&Utc)),
        LocalResult::None => Err(ApiError::invalid(format!(
            "{field} does not exist in the configured time zone",
        ))),
    }
}

pub(crate) fn todoist_recurrence_to_rrule(value: &str, lang: &str) -> Option<String> {
    if !lang.is_empty() && !lang.eq_ignore_ascii_case("en") {
        return None;
    }
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let without_time = normalized
        .split_once(" at ")
        .map_or(normalized.as_str(), |(prefix, _)| prefix);
    match without_time {
        "every day" | "daily" => return Some("FREQ=DAILY".to_owned()),
        "every weekday" => {
            return Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".to_owned());
        }
        "every week" | "weekly" => return Some("FREQ=WEEKLY".to_owned()),
        "every month" | "monthly" => return Some("FREQ=MONTHLY".to_owned()),
        "every year" | "yearly" | "annually" => return Some("FREQ=YEARLY".to_owned()),
        _ => {}
    }
    const WEEKDAYS: [(&str, &str); 7] = [
        ("monday", "MO"),
        ("tuesday", "TU"),
        ("wednesday", "WE"),
        ("thursday", "TH"),
        ("friday", "FR"),
        ("saturday", "SA"),
        ("sunday", "SU"),
    ];
    if let Some((_, day)) = WEEKDAYS
        .iter()
        .find(|(name, _)| without_time == format!("every {name}"))
    {
        return Some(format!("FREQ=WEEKLY;BYDAY={day}"));
    }
    let interval = Regex::new(r"^every ([2-9]|[1-9][0-9]{1,2}) (days?|weeks?|months?|years?)$")
        .expect("constant Todoist recurrence regex");
    let captures = interval.captures(without_time)?;
    let count = captures.get(1)?.as_str();
    let frequency = match captures.get(2)?.as_str().trim_end_matches('s') {
        "day" => "DAILY",
        "week" => "WEEKLY",
        "month" => "MONTHLY",
        "year" => "YEARLY",
        _ => return None,
    };
    Some(format!("FREQ={frequency};INTERVAL={count}"))
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, Timelike};

    use super::*;

    fn fixture() -> TodoistSyncResponse {
        serde_json::from_str(include_str!("../tests/fixtures/todoist/v1/full_sync.json"))
            .expect("recorded Todoist fixture")
    }

    #[test]
    fn sync_request_has_only_read_resource_fields() {
        let client = TodoistClient::new().unwrap();
        let token = TodoistToken::from_secret("canary-token".to_owned()).unwrap();
        let request = client.sync_request(&token, Some("cursor-value")).unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), TODOIST_SYNC_URL);
        assert!(request.headers()[AUTHORIZATION].is_sensitive());
        let body = request.body().and_then(reqwest::Body::as_bytes).unwrap();
        let pairs = form_urlencoded::parse(body).collect::<Vec<_>>();
        assert_eq!(
            pairs,
            vec![
                ("sync_token".into(), "cursor-value".into()),
                ("resource_types".into(), TODOIST_RESOURCE_TYPES.into())
            ]
        );
    }

    #[test]
    fn completed_request_is_bounded_get_only_with_opaque_pagination() {
        let client = TodoistClient::new().unwrap();
        let token = TodoistToken::from_secret("canary-token".to_owned()).unwrap();
        let since = "2026-08-27T10:00:00Z".parse().unwrap();
        let until = "2026-08-27T11:00:00Z".parse().unwrap();
        let request = client
            .completed_request(&token, since, until, Some("opaque.cursor/value"))
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().path(),
            "/api/v1/tasks/completed/by_completion_date"
        );
        let query = request.url().query_pairs().collect::<Vec<_>>();
        assert_eq!(
            query,
            vec![
                ("since".into(), "2026-08-27T10:00:00Z".into()),
                ("until".into(), "2026-08-27T11:00:00Z".into()),
                ("limit".into(), "200".into()),
                ("cursor".into(), "opaque.cursor/value".into()),
            ]
        );
        assert!(request.headers()[AUTHORIZATION].is_sensitive());
        assert!(request.body().is_none());
    }

    #[test]
    fn long_completion_history_is_partitioned_without_clipping() {
        let since = "2025-08-01T00:00:00Z".parse().unwrap();
        let until = "2026-08-27T00:00:00Z".parse().unwrap();
        let windows = todoist_completion_windows(since, until).unwrap();
        assert!(windows.len() > 4);
        assert_eq!(windows.first().unwrap().0, since);
        assert_eq!(windows.last().unwrap().1, until);
        for pair in windows.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
        assert!(
            windows
                .iter()
                .all(|(start, end)| *end - *start <= TODOIST_COMPLETION_MAX_RANGE)
        );
    }

    #[test]
    fn token_debug_is_redacted() {
        let token = TodoistToken::from_secret("secret-canary".to_owned()).unwrap();
        assert_eq!(format!("{token:?}"), "[REDACTED]");
    }

    #[test]
    fn recorded_fixture_maps_deadline_due_priority_labels_and_recurrence() {
        let fixture = fixture();
        assert!(fixture.full_sync);
        assert_eq!(fixture.projects.len(), 2);
        let deadline = map_item(&fixture.items[0], Tz::America__Los_Angeles).unwrap();
        assert_eq!(deadline.soft_due.unwrap().day(), 30);
        assert_eq!(deadline.hard_due_note, Some("todoist_deadline"));
        assert_eq!(deadline.hard_due.unwrap().hour(), 6); // 23:59 PDT in UTC.

        let recurring = map_item(&fixture.items[1], Tz::America__Los_Angeles).unwrap();
        assert_eq!(recurring.hard_due_note, Some("todoist_priority_p1"));
        assert_eq!(
            recurring.recurrence.unwrap().rrule.as_deref(),
            Some("FREQ=WEEKLY;BYDAY=MO")
        );

        let no_due = map_item(&fixture.items[2], Tz::America__Los_Angeles).unwrap();
        assert!(no_due.hard_due.is_none());
        assert!(no_due.needs_triage);
    }

    #[test]
    fn date_only_deadline_uses_owner_local_end_of_day_across_dst() {
        let before = local_end_of_day(
            NaiveDate::from_ymd_opt(2026, 3, 7).unwrap(),
            Tz::America__Los_Angeles,
        )
        .unwrap();
        let after = local_end_of_day(
            NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
            Tz::America__Los_Angeles,
        )
        .unwrap();
        assert_eq!(before.to_rfc3339(), "2026-03-08T07:59:59+00:00");
        assert_eq!(after.to_rfc3339(), "2026-03-09T06:59:59+00:00");
    }

    #[test]
    fn completion_wins_when_deleted_flag_is_also_present() {
        let mut fixture = fixture();
        fixture.items[0].checked = true;
        fixture.items[0].is_deleted = true;
        assert_eq!(
            map_item(&fixture.items[0], Tz::UTC).unwrap().terminal,
            TodoistTerminal::Completed
        );
    }

    #[test]
    fn incremental_tombstones_may_omit_mutable_item_fields() {
        let item: TodoistItem = serde_json::from_value(serde_json::json!({
            "id": "deleted-item-1",
            "is_deleted": true
        }))
        .unwrap();
        let mapped = map_item(&item, Tz::UTC).unwrap();
        assert_eq!(mapped.terminal, TodoistTerminal::Deleted);
        assert!(mapped.title.is_empty());
        assert!(mapped.project_id.is_empty());

        let project: TodoistProject = serde_json::from_value(serde_json::json!({
            "id": "deleted-project-1",
            "is_deleted": true
        }))
        .unwrap();
        assert!(project.name.is_empty());
        assert!(project.is_deleted);
    }

    #[test]
    fn recurrence_parser_is_bounded_and_fail_closed() {
        assert_eq!(
            todoist_recurrence_to_rrule("every 2 weeks at 9am", "en").as_deref(),
            Some("FREQ=WEEKLY;INTERVAL=2")
        );
        assert!(todoist_recurrence_to_rrule("cada semana", "es").is_none());
        assert!(todoist_recurrence_to_rrule("every third business day", "en").is_none());
    }

    #[test]
    fn next_occurrence_preserves_floating_and_fixed_identity_across_dst() {
        let floating = MappedRecurrence {
            recurrence_source: "todoist",
            original: "every Monday at 9am".to_owned(),
            lang: "en".to_owned(),
            series_id: "series-1".to_owned(),
            due: "2026-03-02T09:00:00".to_owned(),
            timezone: None,
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".to_owned()),
            needs_review: false,
        };
        let next = next_todoist_occurrence(&floating, Tz::America__Los_Angeles)
            .unwrap()
            .unwrap();
        assert_eq!(next.soft_due.to_string(), "2026-03-09");
        assert_eq!(next.due_instant.to_rfc3339(), "2026-03-09T16:00:00+00:00");
        assert_eq!(next.occurrence_key, "2026-03-09T09:00:00");

        let fixed = MappedRecurrence {
            due: "2026-03-02T17:00:00Z".to_owned(),
            timezone: Some("America/Los_Angeles".to_owned()),
            ..floating
        };
        let next = next_todoist_occurrence(&fixed, Tz::UTC).unwrap().unwrap();
        assert_eq!(next.due_instant.to_rfc3339(), "2026-03-09T16:00:00+00:00");
        assert_eq!(next.occurrence_key, "2026-03-09T16:00:00Z");
    }

    #[test]
    fn unparseable_recurrence_never_materializes_a_guess() {
        let recurrence = MappedRecurrence {
            recurrence_source: "todoist",
            original: "every third business day".to_owned(),
            lang: "en".to_owned(),
            series_id: "series-2".to_owned(),
            due: "2026-03-02".to_owned(),
            timezone: None,
            rrule: None,
            needs_review: true,
        };
        assert!(
            next_todoist_occurrence(&recurrence, Tz::America__Los_Angeles)
                .unwrap()
                .is_none()
        );
    }
}

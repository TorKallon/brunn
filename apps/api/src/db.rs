use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use sqlx::{
    PgPool, Postgres, Row, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    auth::{AuthContext, hash_token},
    config::Config,
    embeddings::{SharedEmbedder, from_config as embedder_from_config},
    error::{ApiError, ApiResult},
    foreground_latency::ForegroundLatencyTracker,
    object_store::ObjectStore,
    quota::{PreauthRateLimiter, RequestRateLimiter},
    semantic_policy::SemanticRuntime,
    usage::UsageTracker,
    workspace_features::WorkspaceFeatureCache,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub auth_pool: PgPool,
    pub rw_pool: PgPool,
    pub ro_pool: PgPool,
    pub admin_pool: Option<PgPool>,
    pub embedder: SharedEmbedder,
    pub object_store: ObjectStore,
    pub transfer_limiter: Arc<Semaphore>,
    pub preauth_rate_limiter: PreauthRateLimiter,
    pub request_rate_limiter: RequestRateLimiter,
    pub usage_tracker: UsageTracker,
    pub workspace_features: WorkspaceFeatureCache,
    pub semantic_runtime: SemanticRuntime,
    pub foreground_latency: ForegroundLatencyTracker,
    pub foreground_latency_client: reqwest::Client,
    pub web_auth_email_client: reqwest::Client,
    pub web_auth_password_limiter: Arc<Semaphore>,
}

impl AppState {
    pub async fn connect(config: Config) -> ApiResult<Self> {
        let auth_pool = pool(
            &config.database_url_rw,
            config.database_max_connections,
            "straylight-auth",
        )
        .await?;
        let rw_pool = pool(
            &config.database_url_rw,
            config.database_max_connections,
            "straylight-rw",
        )
        .await?;
        let ro_pool = pool(
            &config.database_url_ro,
            config.database_max_connections,
            "straylight-ro",
        )
        .await?;
        let admin_pool = match &config.database_url_admin {
            Some(url) => Some(pool(url, 4, "straylight-worker-admin").await?),
            None => None,
        };
        let embedder = embedder_from_config(&config)?;
        let object_store = ObjectStore::new(&config).await?;
        object_store.ensure_versioned_bucket().await?;
        let preauth_rate_limiter =
            PreauthRateLimiter::new(config.requests_per_minute.saturating_mul(10));
        let request_rate_limiter = RequestRateLimiter::new(config.requests_per_minute);
        let transfer_limiter = Arc::new(Semaphore::new(config.max_concurrent_transfers));
        let usage_tracker = UsageTracker::start(rw_pool.clone());
        let workspace_features = WorkspaceFeatureCache::default();
        let semantic_runtime = SemanticRuntime::new(config.semantic_query_concurrency);
        let foreground_latency = ForegroundLatencyTracker::default();
        let foreground_latency_client = reqwest::Client::builder()
            .timeout(config.embedding_backfill_foreground_status_timeout)
            .build()
            .map_err(|error| {
                ApiError::configuration(format!(
                    "could not build foreground-latency client: {error}"
                ))
            })?;
        let web_auth_email_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| {
                ApiError::configuration(format!("could not build web-auth email client: {error}"))
            })?;
        let web_auth_password_limiter = Arc::new(Semaphore::new(4));
        Ok(Self {
            config,
            auth_pool,
            rw_pool,
            ro_pool,
            admin_pool,
            embedder,
            object_store,
            transfer_limiter,
            preauth_rate_limiter,
            request_rate_limiter,
            usage_tracker,
            workspace_features,
            semantic_runtime,
            foreground_latency,
            foreground_latency_client,
            web_auth_email_client,
            web_auth_password_limiter,
        })
    }

    pub async fn begin_read(&self, auth: &AuthContext) -> ApiResult<Transaction<'_, Postgres>> {
        let started = Instant::now();
        let result = self.ro_pool.begin().await;
        metrics::histogram!(
            "db.transaction.begin.duration_ms",
            "access" => "read",
            "result" => if result.is_ok() { "success" } else { "error" }
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        let mut transaction = result?;
        set_context(&mut transaction, auth).await?;
        set_statement_timeout(&mut transaction, self.config.request_timeout).await?;
        Ok(transaction)
    }

    pub async fn begin_write(&self, auth: &AuthContext) -> ApiResult<Transaction<'_, Postgres>> {
        self.begin_write_with_timeout(auth, self.config.request_timeout)
            .await
    }

    pub async fn begin_transfer_write(
        &self,
        auth: &AuthContext,
    ) -> ApiResult<Transaction<'_, Postgres>> {
        self.begin_write_with_timeout(auth, self.config.transfer_timeout)
            .await
    }

    async fn begin_write_with_timeout(
        &self,
        auth: &AuthContext,
        timeout: Duration,
    ) -> ApiResult<Transaction<'_, Postgres>> {
        let started = Instant::now();
        let result = self.rw_pool.begin().await;
        metrics::histogram!(
            "db.transaction.begin.duration_ms",
            "access" => "write",
            "result" => if result.is_ok() { "success" } else { "error" }
        )
        .record(started.elapsed().as_secs_f64() * 1_000.0);
        let mut transaction = result?;
        set_context(&mut transaction, auth).await?;
        set_statement_timeout(&mut transaction, timeout).await?;
        let status = sqlx::query_scalar::<_, String>(
            "SELECT account_status FROM straylight.users WHERE id=$1",
        )
        .bind(auth.user_id.0)
        .fetch_one(&mut *transaction)
        .await?;
        if status != "active" {
            return Err(ApiError::with_details(
                http::StatusCode::LOCKED,
                "account_locked",
                "the account is not accepting new mutations",
                serde_json::json!({"account_status": status}),
            ));
        }
        Ok(transaction)
    }
}

async fn set_statement_timeout(
    transaction: &mut Transaction<'_, Postgres>,
    request_timeout: Duration,
) -> ApiResult<()> {
    let timeout = statement_timeout(request_timeout);
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{}ms", timeout.as_millis()))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn statement_timeout(request_timeout: Duration) -> Duration {
    request_timeout
        .saturating_sub(Duration::from_secs(5))
        .max(Duration::from_secs(1))
}

async fn pool(url: &str, max: u32, application_name: &str) -> ApiResult<PgPool> {
    let started = Instant::now();
    let options = PgConnectOptions::from_str(url)
        .map_err(|error| ApiError::configuration(format!("invalid database URL: {error}")))?
        .application_name(application_name);
    let result = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(std::time::Duration::from_secs(15))
        .connect_with(options)
        .await;
    metrics::histogram!(
        "db.pool.connect.duration_ms",
        "pool" => application_name.to_owned(),
        "result" => if result.is_ok() { "success" } else { "error" }
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    Ok(result?)
}

pub async fn operator_pool(url: &str) -> ApiResult<PgPool> {
    pool(url, 1, "straylight-operator").await
}

pub async fn set_context(
    transaction: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
) -> ApiResult<()> {
    let started = Instant::now();
    let mut capabilities: Vec<_> = auth.capabilities.iter().cloned().collect();
    capabilities.sort();
    let row = sqlx::query(
        r#"
        SELECT valid, scope_ids
        FROM straylight_auth.validate_transaction_context($1, $2, $3, $4)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(&capabilities)
    .bind(&auth.scope_refs)
    .fetch_one(&mut **transaction)
    .await?;
    if !row.try_get::<bool, _>("valid")? {
        return Err(ApiError::unauthenticated());
    }
    let scope_ids: Vec<Uuid> = row.try_get("scope_ids")?;
    sqlx::query(
        r#"
        SELECT
          set_config('app.current_user_id', $1, true),
          set_config('app.current_credential_id', $2, true),
          set_config('app.capabilities', $3, true),
          set_config('app.scope_refs', $4, true),
          set_config('app.scope_ids', $5, true),
          set_config('app.context_valid', 'true', true)
        "#,
    )
    .bind(auth.user_id.0.to_string())
    .bind(auth.credential_id.0.to_string())
    .bind(auth.capability_guc())
    .bind(auth.scope_guc())
    .bind(
        scope_ids
            .iter()
            .map(Uuid::to_string)
            .collect::<Vec<_>>()
            .join(","),
    )
    .execute(&mut **transaction)
    .await?;
    metrics::histogram!("db.context.duration_ms").record(started.elapsed().as_secs_f64() * 1_000.0);
    Ok(())
}

pub async fn migrate_and_bootstrap(config: &Config) -> ApiResult<()> {
    let admin_url = config
        .database_url_admin
        .as_deref()
        .ok_or_else(|| ApiError::configuration("DATABASE_URL_ADMIN is required for migrations"))?;
    let admin = pool(admin_url, 2, "straylight-migrate").await?;
    sqlx::migrate!("./migrations").run(&admin).await?;
    bootstrap_dev_identity(&admin, config).await?;
    Ok(())
}

async fn bootstrap_dev_identity(pool: &PgPool, config: &Config) -> ApiResult<()> {
    let Some(read_write_token) = &config.dev_read_write_token else {
        tracing::warn!(
            "STRAYLIGHT_DEV_READ_WRITE_TOKEN is unset; no local credential was bootstrapped"
        );
        return Ok(());
    };

    let write_capabilities = dev_write_capabilities(config.messaging_enabled);
    let (_user_id, _, _scope_id, _): (Uuid, Uuid, Uuid, Uuid) =
        sqlx::query_as("SELECT * FROM straylight_auth.bootstrap_user($1, $2, $3, $4, $5)")
            .bind(&config.dev_user_ref)
            .bind(&config.dev_user_name)
            .bind("Local read/write")
            .bind(hash_token(read_write_token))
            .bind(&write_capabilities)
            .fetch_one(pool)
            .await?;

    if let Some(read_only_token) = &config.dev_read_only_token {
        let read_capabilities = dev_read_capabilities(config.messaging_enabled);
        let _: (Uuid, Uuid, Uuid, Uuid) =
            sqlx::query_as("SELECT * FROM straylight_auth.bootstrap_user($1, $2, $3, $4, $5)")
                .bind(&config.dev_user_ref)
                .bind(&config.dev_user_name)
                .bind("Local read-only")
                .bind(hash_token(read_only_token))
                .bind(&read_capabilities)
                .fetch_one(pool)
                .await?;
    }

    Ok(())
}

fn dev_write_capabilities(messaging_enabled: bool) -> Vec<&'static str> {
    let mut capabilities = vec![
        "open",
        "query",
        "read",
        "compute",
        "verify",
        "status",
        "checkpoint",
        "save",
        "stage",
        "correct",
        "delete",
        "dream",
        "credential:manage",
        "notification:publish",
        "notification:manage",
        "secret:read",
        "secret:write",
        "task.read",
        "task.write",
        "integration.manage",
        "admin",
    ];
    if messaging_enabled {
        capabilities.extend(["message.read", "message.write"]);
    }
    capabilities
}

fn dev_read_capabilities(messaging_enabled: bool) -> Vec<&'static str> {
    let mut capabilities = vec![
        "open",
        "query",
        "read",
        "compute",
        "verify",
        "status",
        "task.read",
    ];
    if messaging_enabled {
        capabilities.push("message.read");
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_timeout_precedes_the_request_deadline() {
        assert_eq!(
            statement_timeout(Duration::from_secs(30)),
            Duration::from_secs(25)
        );
        assert_eq!(
            statement_timeout(Duration::from_secs(3)),
            Duration::from_secs(1)
        );
        assert_eq!(
            statement_timeout(Duration::from_secs(3_600)),
            Duration::from_secs(3_595)
        );
    }

    #[test]
    fn dev_credentials_add_messaging_capabilities_only_when_enabled() {
        let read_write_off = dev_write_capabilities(false);
        assert!(!read_write_off.contains(&"message.read"));
        assert!(!read_write_off.contains(&"message.write"));
        let read_only_off = dev_read_capabilities(false);
        assert!(!read_only_off.contains(&"message.read"));
        assert!(!read_only_off.contains(&"message.write"));

        let read_write_on = dev_write_capabilities(true);
        assert!(read_write_on.contains(&"message.read"));
        assert!(read_write_on.contains(&"message.write"));
        let read_only_on = dev_read_capabilities(true);
        assert!(read_only_on.contains(&"message.read"));
        assert!(!read_only_on.contains(&"message.write"));
    }
}

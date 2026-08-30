use std::{env, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};

use crate::error::{ApiError, ApiResult};

#[derive(Clone)]
pub struct Config {
    pub deployment_environment: String,
    pub evaluation_api_enabled: bool,
    pub supersession_demotion: bool,
    pub supersession_demotion_weight: f64,
    pub intention_ledger: bool,
    pub read_path_roundtrip_v1: bool,
    pub lexical_single_scan: bool,
    pub resume_deltas: bool,
    pub bind: SocketAddr,
    pub database_url_admin: Option<String>,
    pub database_url_rw: String,
    pub database_url_ro: String,
    pub database_max_connections: u32,
    pub s3_endpoint: Option<String>,
    pub s3_region: String,
    pub s3_bucket: String,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub s3_force_path_style: bool,
    pub s3_create_bucket: bool,
    pub openai_api_key: Option<String>,
    pub openai_base_url: String,
    pub capture_model: String,
    pub capture_max_output_tokens: u64,
    pub asset_description_model: String,
    pub asset_description_max_output_tokens: u64,
    pub asset_description_image_detail: String,
    pub background_job_lease: Duration,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub embedding_provider: String,
    pub allow_degraded_embeddings: bool,
    pub observability_timings_ms: bool,
    pub verbatim_spans: bool,
    pub semantic_lane: bool,
    pub embed_cache: bool,
    pub semantic_deadline: Option<Duration>,
    pub semantic_query_provider_timeout: Duration,
    pub semantic_query_concurrency: usize,
    pub embedding_backfill_guard: bool,
    pub embedding_backfill_batch_chunks: usize,
    pub embedding_backfill_inter_batch_delay: Duration,
    pub embedding_backfill_open_p95_limit_ms: f64,
    pub embedding_backfill_search_p95_limit_ms: f64,
    pub embedding_backfill_foreground_status_url: Option<String>,
    pub embedding_backfill_foreground_status_timeout: Duration,
    pub continuation_secret: String,
    pub materialize_token_budget: usize,
    pub search_fair_share: bool,
    pub search_top1_hydration: bool,
    pub search_char_cap: bool,
    pub search_section_demotion_top_n: Option<usize>,
    pub request_timeout: Duration,
    pub transfer_timeout: Duration,
    pub max_concurrent_transfers: usize,
    pub readiness_timeout: Duration,
    pub requests_per_minute: u32,
    pub allowed_origins: Vec<String>,
    pub resend_api_key: Option<String>,
    pub auth_email_from: Option<String>,
    pub auth_email_reply_to: Option<String>,
    pub apns_team_id: Option<String>,
    pub apns_key_id: Option<String>,
    pub apns_private_key: Option<String>,
    pub apns_app_id: Option<String>,
    pub notification_token_encryption_key: Option<String>,
    pub secret_encryption_key: Option<String>,
    pub apns_delivery_enabled: bool,
    pub messaging_enabled: bool,
    pub todoist_sync_enabled: bool,
    pub public_url: String,
    pub account_export_ttl: Duration,
    pub account_export_temp_dir: PathBuf,
    pub account_deletion_backup_retention_days: i32,
    pub dev_user_ref: String,
    pub dev_user_name: String,
    pub dev_read_write_token: Option<String>,
    pub dev_read_only_token: Option<String>,
    pub dreamer_internal_url: Option<String>,
    pub dreamer_internal_token: Option<String>,
}

impl Config {
    pub fn admin_database_url_from_env() -> ApiResult<String> {
        required_any_or_file(&["DATABASE_URL_ADMIN", "STRAYLIGHT_ADMIN_DATABASE_URL"])
    }

    pub fn from_env() -> ApiResult<Self> {
        let deployment_environment = env_default("STRAYLIGHT_ENV", "development");
        if !matches!(
            deployment_environment.as_str(),
            "development" | "production"
        ) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_ENV must be development or production",
            ));
        }
        let non_production_default = if deployment_environment == "production" {
            "false"
        } else {
            "true"
        };
        let bind = env_parse_value(
            "STRAYLIGHT_BIND",
            first_env(&["STRAYLIGHT_BIND", "STRAYLIGHT_BIND_ADDR"])
                .unwrap_or_else(|| "0.0.0.0:8080".to_owned()),
        )?;
        let database_url_rw =
            required_any_or_file(&["DATABASE_URL_RW", "STRAYLIGHT_DATABASE_URL"])?;
        let database_url_ro =
            first_env_or_file(&["DATABASE_URL_RO", "STRAYLIGHT_READ_ONLY_DATABASE_URL"])?
                .unwrap_or_else(|| database_url_rw.clone());
        let continuation_secret = required_any_or_file(&[
            "STRAYLIGHT_CONTINUATION_SECRET",
            "STRAYLIGHT_CONTINUATION_SIGNING_KEY",
        ])?;
        if continuation_secret.len() < 32 {
            return Err(ApiError::configuration(
                "STRAYLIGHT_CONTINUATION_SECRET must contain at least 32 characters",
            ));
        }
        let s3_endpoint = first_env(&["STRAYLIGHT_S3_ENDPOINT", "STRAYLIGHT_MINIO_ENDPOINT"]);
        let s3_access_key =
            first_env_or_file(&["STRAYLIGHT_S3_ACCESS_KEY", "STRAYLIGHT_MINIO_ACCESS_KEY"])?;
        let s3_secret_key =
            first_env_or_file(&["STRAYLIGHT_S3_SECRET_KEY", "STRAYLIGHT_MINIO_SECRET_KEY"])?;
        validate_explicit_s3_credentials(&s3_access_key, &s3_secret_key)?;
        let s3_force_path_style = first_env_parse(&[
            "STRAYLIGHT_S3_FORCE_PATH_STYLE",
            "STRAYLIGHT_MINIO_FORCE_PATH_STYLE",
        ])?
        .unwrap_or(s3_endpoint.is_some());
        let s3_create_bucket = first_env_parse(&[
            "STRAYLIGHT_S3_CREATE_BUCKET",
            "STRAYLIGHT_MINIO_CREATE_BUCKET",
        ])?
        .unwrap_or(deployment_environment != "production");

        let config = Self {
            deployment_environment,
            evaluation_api_enabled: env_parse(
                "STRAYLIGHT_EVALUATION_API_ENABLED",
                non_production_default,
            )?,
            supersession_demotion: env_parse("STRAYLIGHT_SUPERSESSION_DEMOTION", "false")?,
            supersession_demotion_weight: env_parse(
                "STRAYLIGHT_SUPERSESSION_DEMOTION_WEIGHT",
                "1.5",
            )?,
            intention_ledger: env_parse("STRAYLIGHT_INTENTION_LEDGER", "false")?,
            read_path_roundtrip_v1: env_parse("STRAYLIGHT_READ_PATH_ROUNDTRIP_V1", "false")?,
            lexical_single_scan: env_parse("STRAYLIGHT_LEXICAL_SINGLE_SCAN", "false")?,
            resume_deltas: env_parse("STRAYLIGHT_RESUME_DELTAS", "false")?,
            bind,
            database_url_admin: first_env_or_file(&[
                "DATABASE_URL_ADMIN",
                "STRAYLIGHT_ADMIN_DATABASE_URL",
            ])?,
            database_url_rw,
            database_url_ro,
            database_max_connections: env_parse("STRAYLIGHT_DATABASE_MAX_CONNECTIONS", "20")?,
            s3_endpoint,
            s3_region: first_env(&["STRAYLIGHT_S3_REGION", "STRAYLIGHT_MINIO_REGION"])
                .unwrap_or_else(|| "us-east-1".to_owned()),
            s3_bucket: first_env(&["STRAYLIGHT_S3_BUCKET", "STRAYLIGHT_MINIO_BUCKET"])
                .unwrap_or_else(|| "straylight".to_owned()),
            s3_access_key,
            s3_secret_key,
            s3_force_path_style,
            s3_create_bucket,
            openai_api_key: first_env_or_file(&["OPENAI_API_KEY"])?,
            openai_base_url: env_default("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            capture_model: env_default("STRAYLIGHT_CAPTURE_MODEL", "gpt-5.6"),
            capture_max_output_tokens: env_parse("STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS", "8192")?,
            asset_description_model: first_env(&[
                "CARRYSTATE_ASSET_DESCRIPTION_MODEL",
                "STRAYLIGHT_ASSET_DESCRIPTION_MODEL",
            ])
            .unwrap_or_else(|| "gpt-5.6".to_owned()),
            asset_description_max_output_tokens: env_parse_value(
                "CARRYSTATE_ASSET_DESCRIPTION_MAX_OUTPUT_TOKENS",
                first_env(&[
                    "CARRYSTATE_ASSET_DESCRIPTION_MAX_OUTPUT_TOKENS",
                    "STRAYLIGHT_ASSET_DESCRIPTION_MAX_OUTPUT_TOKENS",
                ])
                .unwrap_or_else(|| "4096".to_owned()),
            )?,
            asset_description_image_detail: first_env(&[
                "CARRYSTATE_ASSET_DESCRIPTION_IMAGE_DETAIL",
                "STRAYLIGHT_ASSET_DESCRIPTION_IMAGE_DETAIL",
            ])
            .unwrap_or_else(|| "high".to_owned()),
            background_job_lease: Duration::from_secs(env_parse(
                "STRAYLIGHT_BACKGROUND_JOB_LEASE_SECONDS",
                "300",
            )?),
            embedding_model: env_default("STRAYLIGHT_EMBEDDING_MODEL", "text-embedding-3-small"),
            embedding_dimensions: env_parse("STRAYLIGHT_EMBEDDING_DIMENSIONS", "1536")?,
            embedding_provider: env_default("STRAYLIGHT_EMBEDDING_PROVIDER", "openai"),
            allow_degraded_embeddings: env_parse("STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS", "false")?,
            observability_timings_ms: env_parse("STRAYLIGHT_OBSERVABILITY_TIMINGS_MS", "true")?,
            verbatim_spans: env_parse("STRAYLIGHT_VERBATIM_SPANS", "false")?,
            semantic_lane: env_parse("STRAYLIGHT_SEMANTIC_LANE", "false")?,
            embed_cache: env_parse("STRAYLIGHT_EMBED_CACHE", "true")?,
            semantic_deadline: match env_parse::<u64>("STRAYLIGHT_SEMANTIC_DEADLINE_MS", "2500")? {
                0 => None,
                milliseconds => Some(Duration::from_millis(milliseconds)),
            },
            semantic_query_provider_timeout: Duration::from_millis(env_parse(
                "STRAYLIGHT_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS",
                "5000",
            )?),
            semantic_query_concurrency: env_parse("STRAYLIGHT_SEMANTIC_QUERY_CONCURRENCY", "8")?,
            embedding_backfill_guard: env_parse("STRAYLIGHT_EMBEDDING_BACKFILL_GUARD", "true")?,
            embedding_backfill_batch_chunks: env_parse(
                "STRAYLIGHT_EMBEDDING_BACKFILL_BATCH_CHUNKS",
                "64",
            )?,
            embedding_backfill_inter_batch_delay: Duration::from_millis(env_parse(
                "STRAYLIGHT_EMBEDDING_BACKFILL_INTER_BATCH_MS",
                "250",
            )?),
            embedding_backfill_open_p95_limit_ms: env_parse(
                "STRAYLIGHT_EMBEDDING_BACKFILL_OPEN_P95_LIMIT_MS",
                "120",
            )?,
            embedding_backfill_search_p95_limit_ms: env_parse(
                "STRAYLIGHT_EMBEDDING_BACKFILL_SEARCH_P95_LIMIT_MS",
                "107",
            )?,
            embedding_backfill_foreground_status_url: first_env(&[
                "STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL",
            ]),
            embedding_backfill_foreground_status_timeout: Duration::from_millis(env_parse(
                "STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_TIMEOUT_MS",
                "1000",
            )?),
            continuation_secret,
            materialize_token_budget: env_parse("STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET", "24000")?,
            search_fair_share: env_parse("STRAYLIGHT_SEARCH_FAIR_SHARE", "false")?,
            search_top1_hydration: env_parse("STRAYLIGHT_SEARCH_TOP1_HYDRATION", "false")?,
            search_char_cap: env_parse("STRAYLIGHT_SEARCH_CHAR_CAP", "false")?,
            search_section_demotion_top_n: first_env_parse(&[
                "STRAYLIGHT_SEARCH_SECTION_DEMOTION_TOP_N",
            ])?,
            request_timeout: Duration::from_secs(env_parse(
                "STRAYLIGHT_REQUEST_TIMEOUT_SECONDS",
                "30",
            )?),
            transfer_timeout: Duration::from_secs(env_parse(
                "STRAYLIGHT_TRANSFER_TIMEOUT_SECONDS",
                "3600",
            )?),
            max_concurrent_transfers: env_parse("STRAYLIGHT_MAX_CONCURRENT_TRANSFERS", "8")?,
            readiness_timeout: Duration::from_secs(env_parse(
                "STRAYLIGHT_READINESS_TIMEOUT_SECONDS",
                "3",
            )?),
            requests_per_minute: env_parse("STRAYLIGHT_REQUESTS_PER_MINUTE", "600")?,
            allowed_origins: parse_allowed_origins(&env_default("STRAYLIGHT_ALLOWED_ORIGINS", ""))?,
            resend_api_key: first_env_or_file(&["RESEND_API_KEY"])?,
            auth_email_from: first_env(&["AUTH_EMAIL_FROM"]).map(|value| value.trim().to_owned()),
            auth_email_reply_to: first_env(&["AUTH_EMAIL_REPLY_TO"])
                .map(|value| value.trim().to_owned()),
            apns_team_id: first_env(&["STRAYLIGHT_APNS_TEAM_ID"])
                .map(|value| value.trim().to_owned()),
            apns_key_id: first_env(&["STRAYLIGHT_APNS_KEY_ID"])
                .map(|value| value.trim().to_owned()),
            apns_private_key: first_env_or_file(&["STRAYLIGHT_APNS_PRIVATE_KEY"])?,
            apns_app_id: first_env(&["STRAYLIGHT_APNS_APP_ID"])
                .map(|value| value.trim().to_owned()),
            notification_token_encryption_key: first_env_or_file(&[
                "STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY",
            ])?,
            secret_encryption_key: first_env_or_file(&["STRAYLIGHT_SECRET_ENCRYPTION_KEY"])?,
            apns_delivery_enabled: env_parse("STRAYLIGHT_APNS_DELIVERY_ENABLED", "false")?,
            messaging_enabled: env_parse("STRAYLIGHT_MESSAGING_ENABLED", "false")?,
            todoist_sync_enabled: env_parse("STRAYLIGHT_TODOIST_SYNC_ENABLED", "false")?,
            public_url: env_default("STRAYLIGHT_PUBLIC_URL", "https://straylight.rourkem.com")
                .trim()
                .trim_end_matches('/')
                .to_owned(),
            account_export_ttl: Duration::from_secs(
                env_parse::<u64>("STRAYLIGHT_ACCOUNT_EXPORT_TTL_HOURS", "24")?
                    .saturating_mul(60 * 60),
            ),
            account_export_temp_dir: PathBuf::from(env_default(
                "STRAYLIGHT_ACCOUNT_EXPORT_TEMP_DIR",
                "/tmp/straylight-exports",
            )),
            account_deletion_backup_retention_days: env_parse(
                "STRAYLIGHT_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS",
                "30",
            )?,
            dev_user_ref: env_default("STRAYLIGHT_DEV_USER_REF", "user:local"),
            dev_user_name: env_default("STRAYLIGHT_DEV_USER_NAME", "Local user"),
            dev_read_write_token: first_env_or_file(&["STRAYLIGHT_DEV_READ_WRITE_TOKEN"])?,
            dev_read_only_token: first_env_or_file(&["STRAYLIGHT_DEV_READ_ONLY_TOKEN"])?,
            dreamer_internal_url: first_env(&["DREAMER_INTERNAL_URL"]),
            dreamer_internal_token: first_env_or_file(&["DREAMER_INTERNAL_TOKEN"])?,
        };
        if !matches!(
            config.asset_description_image_detail.as_str(),
            "low" | "high" | "auto" | "original"
        ) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_ASSET_DESCRIPTION_IMAGE_DETAIL must be low, high, auto, or original",
            ));
        }
        if config.request_timeout.is_zero() || config.transfer_timeout.is_zero() {
            return Err(ApiError::configuration(
                "request and transfer timeouts must be greater than zero",
            ));
        }
        if config.max_concurrent_transfers == 0 {
            return Err(ApiError::configuration(
                "STRAYLIGHT_MAX_CONCURRENT_TRANSFERS must be greater than zero",
            ));
        }
        let apns_provider_parts = [
            config.apns_team_id.is_some(),
            config.apns_key_id.is_some(),
            config.apns_private_key.is_some(),
        ];
        if apns_provider_parts.iter().any(|configured| *configured)
            && (!apns_provider_parts.iter().all(|configured| *configured)
                || config.apns_app_id.is_none())
        {
            return Err(ApiError::configuration(
                "APNs provider credentials require team ID, key ID, private key, and app ID",
            ));
        }
        for (name, value) in [
            ("STRAYLIGHT_APNS_TEAM_ID", config.apns_team_id.as_deref()),
            ("STRAYLIGHT_APNS_KEY_ID", config.apns_key_id.as_deref()),
        ] {
            if value.is_some_and(|value| {
                value.len() != 10 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
            }) {
                return Err(ApiError::configuration(format!(
                    "{name} must be a 10-character ASCII identifier"
                )));
            }
        }
        if config.apns_app_id.as_deref().is_some_and(|value| {
            value.len() < 3
                || value.len() > 255
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        }) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_APNS_APP_ID is not a valid app topic",
            ));
        }
        if let Some(key) = config.notification_token_encryption_key.as_deref() {
            decode_notification_token_key(key)?;
        }
        if let Some(key) = config.secret_encryption_key.as_deref() {
            decode_secret_encryption_key(key)?;
        }
        if config.todoist_sync_enabled && config.secret_encryption_key.is_none() {
            return Err(ApiError::configuration(
                "enabled Todoist sync requires STRAYLIGHT_SECRET_ENCRYPTION_KEY",
            ));
        }
        if config.apns_app_id.is_some() != config.notification_token_encryption_key.is_some() {
            return Err(ApiError::configuration(
                "STRAYLIGHT_APNS_APP_ID and STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY must be configured together",
            ));
        }
        if config.apns_configured() && config.notification_token_encryption_key.is_none() {
            return Err(ApiError::configuration(
                "APNs delivery requires STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY",
            ));
        }
        if config.apns_delivery_enabled
            && (config.apns_app_id.is_none() || config.notification_token_encryption_key.is_none())
        {
            return Err(ApiError::configuration(
                "enabled APNs delivery requires app ID and notification token encryption key",
            ));
        }
        if config.search_section_demotion_top_n == Some(0) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_SEARCH_SECTION_DEMOTION_TOP_N must be greater than zero when set",
            ));
        }
        if !config.supersession_demotion_weight.is_finite()
            || config.supersession_demotion_weight < 0.0
        {
            return Err(ApiError::configuration(
                "STRAYLIGHT_SUPERSESSION_DEMOTION_WEIGHT must be a finite nonnegative number",
            ));
        }
        if !(1..=64).contains(&config.embedding_backfill_batch_chunks) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_EMBEDDING_BACKFILL_BATCH_CHUNKS must be between 1 and 64",
            ));
        }
        if config.embedding_backfill_inter_batch_delay < Duration::from_millis(250) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_EMBEDDING_BACKFILL_INTER_BATCH_MS must be at least 250",
            ));
        }
        if config.semantic_query_provider_timeout.is_zero()
            || config.semantic_query_provider_timeout > Duration::from_secs(60)
        {
            return Err(ApiError::configuration(
                "STRAYLIGHT_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS must be between 1 and 60000",
            ));
        }
        if !(1..=64).contains(&config.semantic_query_concurrency) {
            return Err(ApiError::configuration(
                "STRAYLIGHT_SEMANTIC_QUERY_CONCURRENCY must be between 1 and 64",
            ));
        }
        if !config.embedding_backfill_open_p95_limit_ms.is_finite()
            || !config.embedding_backfill_search_p95_limit_ms.is_finite()
            || config.embedding_backfill_open_p95_limit_ms <= 0.0
            || config.embedding_backfill_search_p95_limit_ms <= 0.0
        {
            return Err(ApiError::configuration(
                "embedding backfill foreground p95 limits must be greater than zero",
            ));
        }
        if config
            .embedding_backfill_foreground_status_timeout
            .is_zero()
        {
            return Err(ApiError::configuration(
                "STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_TIMEOUT_MS must be greater than zero",
            ));
        }
        if config
            .embedding_backfill_foreground_status_url
            .as_ref()
            .is_some_and(|value| !value.starts_with("http://") && !value.starts_with("https://"))
        {
            return Err(ApiError::configuration(
                "STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL must use http or https",
            ));
        }
        config.validate_production()?;
        Ok(config)
    }

    fn validate_production(&self) -> ApiResult<()> {
        if self.deployment_environment != "production" {
            return Ok(());
        }
        if self.dev_read_write_token.is_some() || self.dev_read_only_token.is_some() {
            return Err(ApiError::configuration(
                "development bootstrap credentials are forbidden in production",
            ));
        }
        if self.embedding_provider != "openai"
            || self.openai_api_key.is_none()
            || self.allow_degraded_embeddings
        {
            return Err(ApiError::configuration(
                "production requires non-degraded OpenAI embeddings",
            ));
        }
        if self
            .continuation_secret
            .to_ascii_lowercase()
            .contains("replace")
            || self
                .s3_access_key
                .as_deref()
                .is_some_and(contains_placeholder)
            || self
                .s3_secret_key
                .as_deref()
                .is_some_and(contains_placeholder)
            || self
                .database_url_rw
                .to_ascii_lowercase()
                .contains("replace")
            || self
                .database_url_ro
                .to_ascii_lowercase()
                .contains("replace")
        {
            return Err(ApiError::configuration(
                "placeholder credentials are forbidden in production",
            ));
        }
        if self
            .s3_secret_key
            .as_ref()
            .is_some_and(|secret| secret.len() < 16)
        {
            return Err(ApiError::configuration(
                "the production object-store secret must contain at least 16 characters",
            ));
        }
        if self
            .allowed_origins
            .iter()
            .any(|origin| !origin.to_ascii_lowercase().starts_with("https://"))
        {
            return Err(ApiError::configuration(
                "production CORS origins must use HTTPS",
            ));
        }
        if !self.public_url.to_ascii_lowercase().starts_with("https://") {
            return Err(ApiError::configuration(
                "STRAYLIGHT_PUBLIC_URL must use HTTPS in production",
            ));
        }
        Ok(())
    }

    pub fn web_email_configured(&self) -> bool {
        self.resend_api_key.is_some() && self.auth_email_from.is_some()
    }

    pub fn validate_web_auth(&self) -> ApiResult<()> {
        validate_web_auth_config(self)
    }

    pub fn secure_web_cookies(&self) -> bool {
        self.deployment_environment == "production"
    }

    pub fn apns_configured(&self) -> bool {
        self.apns_team_id.is_some()
            && self.apns_key_id.is_some()
            && self.apns_private_key.is_some()
            && self.apns_app_id.is_some()
    }
}

pub fn decode_notification_token_key(value: &str) -> ApiResult<[u8; 32]> {
    let value = value.trim();
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| STANDARD.decode(value))
        .map_err(|_| {
            ApiError::configuration(
                "STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY must be base64 encoded",
            )
        })?;
    bytes.try_into().map_err(|_| {
        ApiError::configuration(
            "STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY must decode to exactly 32 bytes",
        )
    })
}

pub fn decode_secret_encryption_key(value: &str) -> ApiResult<[u8; 32]> {
    let value = value.trim();
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| STANDARD.decode(value))
        .map_err(|_| {
            ApiError::configuration("STRAYLIGHT_SECRET_ENCRYPTION_KEY must be base64 encoded")
        })?;
    bytes.try_into().map_err(|_| {
        ApiError::configuration("STRAYLIGHT_SECRET_ENCRYPTION_KEY must decode to exactly 32 bytes")
    })
}

fn env_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn first_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

fn first_env_or_file(names: &[&str]) -> ApiResult<Option<String>> {
    for name in names {
        let direct = env::var(name).ok().filter(|value| !value.trim().is_empty());
        let file_name = format!("{name}_FILE");
        let file = env::var(&file_name)
            .ok()
            .filter(|value| !value.trim().is_empty());
        if direct.is_some() && file.is_some() {
            return Err(ApiError::configuration(format!(
                "{name} and {file_name} cannot both be set"
            )));
        }
        if let Some(value) = direct {
            return Ok(Some(value));
        }
        if let Some(path) = file {
            return read_secret_file(name, &path).map(Some);
        }
    }
    Ok(None)
}

fn read_secret_file(name: &str, path: &str) -> ApiResult<String> {
    let bytes = std::fs::read(path).map_err(|error| {
        ApiError::configuration(format!(
            "could not read {name} from its configured file: {error}"
        ))
    })?;
    if bytes.len() > 64 * 1024 {
        return Err(ApiError::configuration(format!(
            "{name} file exceeds 64 KiB"
        )));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| ApiError::configuration(format!("{name} file is not valid UTF-8")))?;
    let value = value.trim_end_matches(['\r', '\n']).to_owned();
    if value.trim().is_empty() || value.contains('\0') {
        return Err(ApiError::configuration(format!(
            "{name} file is empty or invalid"
        )));
    }
    Ok(value)
}

fn required_any_or_file(names: &[&str]) -> ApiResult<String> {
    first_env_or_file(names)?
        .ok_or_else(|| ApiError::configuration(format!("one of {} is required", names.join(", "))))
}

fn env_parse<T>(name: &str, default: &str) -> ApiResult<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    env_default(name, default)
        .parse::<T>()
        .map_err(|error| ApiError::configuration(format!("invalid {name}: {error}")))
}

fn env_parse_value<T>(name: &str, value: String) -> ApiResult<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| ApiError::configuration(format!("invalid {name}: {error}")))
}

fn first_env_parse<T>(names: &[&str]) -> ApiResult<Option<T>>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    for name in names {
        if let Some(value) = env::var(name).ok().filter(|value| !value.trim().is_empty()) {
            return env_parse_value(name, value).map(Some);
        }
    }
    Ok(None)
}

fn validate_explicit_s3_credentials(
    access_key: &Option<String>,
    secret_key: &Option<String>,
) -> ApiResult<()> {
    if access_key.is_some() == secret_key.is_some() {
        return Ok(());
    }
    Err(ApiError::configuration(
        "object-store access and secret keys must be configured together; omit both to use the AWS default credential chain",
    ))
}

fn validate_web_auth_config(config: &Config) -> ApiResult<()> {
    if config.resend_api_key.is_some() != config.auth_email_from.is_some() {
        return Err(ApiError::configuration(
            "RESEND_API_KEY and AUTH_EMAIL_FROM must be configured together",
        ));
    }
    if config.auth_email_reply_to.is_some() && !config.web_email_configured() {
        return Err(ApiError::configuration(
            "AUTH_EMAIL_REPLY_TO requires RESEND_API_KEY and AUTH_EMAIL_FROM",
        ));
    }
    if config.deployment_environment == "production" && !config.web_email_configured() {
        return Err(ApiError::configuration(
            "production Web UI authentication requires RESEND_API_KEY and AUTH_EMAIL_FROM",
        ));
    }
    if let Some(api_key) = config.resend_api_key.as_deref()
        && (api_key.len() < 20
            || api_key.chars().any(char::is_whitespace)
            || api_key.chars().any(char::is_control)
            || contains_placeholder(api_key))
    {
        return Err(ApiError::configuration(
            "RESEND_API_KEY is too short, contains whitespace, or is a placeholder",
        ));
    }
    for (name, value) in [
        ("AUTH_EMAIL_FROM", config.auth_email_from.as_deref()),
        ("AUTH_EMAIL_REPLY_TO", config.auth_email_reply_to.as_deref()),
    ] {
        if let Some(value) = value
            && (!value.contains('@') || value.chars().any(char::is_control))
        {
            return Err(ApiError::configuration(format!(
                "{name} must be a printable email sender value"
            )));
        }
    }
    let public_url = reqwest::Url::parse(&config.public_url)
        .map_err(|_| ApiError::configuration("STRAYLIGHT_PUBLIC_URL must be an absolute URL"))?;
    if !matches!(public_url.scheme(), "http" | "https")
        || public_url.host_str().is_none()
        || public_url.query().is_some()
        || public_url.fragment().is_some()
        || !public_url.username().is_empty()
        || public_url.password().is_some()
        || public_url.path() != "/"
    {
        return Err(ApiError::configuration(
            "STRAYLIGHT_PUBLIC_URL must be an HTTP(S) origin without credentials, path, query, or fragment",
        ));
    }
    Ok(())
}

fn contains_placeholder(value: &str) -> bool {
    value.to_ascii_lowercase().contains("replace")
}

fn parse_allowed_origins(value: &str) -> ApiResult<Vec<String>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(|origin| {
            let parsed = http::HeaderValue::from_str(origin).map_err(|error| {
                ApiError::configuration(format!(
                    "invalid STRAYLIGHT_ALLOWED_ORIGINS entry {origin:?}: {error}"
                ))
            })?;
            let lower = origin.to_ascii_lowercase();
            if !(lower.starts_with("https://")
                || lower.starts_with("http://localhost")
                || lower.starts_with("http://127.0.0.1")
                || lower.starts_with("http://nyx"))
            {
                return Err(ApiError::configuration(format!(
                    "STRAYLIGHT_ALLOWED_ORIGINS entry must be HTTPS or an explicit local development origin: {origin}"
                )));
            }
            drop(parsed);
            Ok(origin.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, process::Command};

    #[test]
    fn cors_origins_are_deny_by_default_and_explicit() {
        assert!(parse_allowed_origins("").unwrap().is_empty());
        assert_eq!(
            parse_allowed_origins("https://memory.example, http://localhost:13110").unwrap(),
            vec![
                "https://memory.example".to_owned(),
                "http://localhost:13110".to_owned()
            ]
        );
        assert!(parse_allowed_origins("*").is_err());
        assert!(parse_allowed_origins("http://public.example").is_err());
    }

    #[test]
    fn secret_files_trim_only_line_endings_and_reject_empty_values() {
        let mut secret = tempfile::NamedTempFile::new().unwrap();
        writeln!(secret, "  keep-spaces  ").unwrap();
        assert_eq!(
            read_secret_file("TEST_SECRET", secret.path().to_str().unwrap()).unwrap(),
            "  keep-spaces  "
        );

        let empty = tempfile::NamedTempFile::new().unwrap();
        assert!(read_secret_file("TEST_SECRET", empty.path().to_str().unwrap()).is_err());
    }

    #[test]
    fn production_can_use_the_aws_default_credential_chain() {
        run_config_env_probe("aws_default_chain");
    }

    #[test]
    fn development_minio_aliases_keep_their_existing_defaults() {
        run_config_env_probe("minio_aliases");
    }

    #[test]
    fn object_store_addressing_and_bucket_creation_are_configurable() {
        run_config_env_probe("explicit_overrides");
    }

    #[test]
    fn semantic_policy_flags_are_explicit_and_zero_deadline_is_unbounded() {
        run_config_env_probe("semantic_policy_flags");
    }

    #[test]
    fn partial_static_object_store_credentials_are_rejected() {
        run_config_env_probe("partial_credentials");
    }

    #[test]
    fn search_contract_flags_are_off_by_default() {
        run_config_env_probe("search_contract_defaults");
    }

    #[test]
    fn search_contract_flags_and_knob_are_configurable() {
        run_config_env_probe("search_contract_flags");
    }

    #[test]
    fn resume_delta_flag_is_default_off_and_explicit() {
        run_config_env_probe("resume_delta_flag");
    }

    #[test]
    fn resend_secret_file_is_supported_for_the_api() {
        run_config_env_probe("resend_secret_file");
    }

    #[test]
    fn production_web_mail_configuration_fails_closed() {
        run_config_env_probe("production_missing_web_mail");
        run_config_env_probe("production_placeholder_web_mail");
    }

    #[test]
    fn deployment_environment_typos_fail_closed() {
        run_config_env_probe("invalid_environment");
    }

    #[test]
    fn apns_api_and_worker_configuration_are_separated_and_fail_closed() {
        run_config_env_probe("notification_api_config");
        run_config_env_probe("notification_app_without_key");
        run_config_env_probe("notification_full_worker");
        run_config_env_probe("notification_partial_provider");
        run_config_env_probe("notification_provider_missing_token_key");
    }

    #[test]
    fn notification_token_key_requires_exactly_32_base64_bytes() {
        let encoded = STANDARD.encode([7_u8; 32]);
        assert_eq!(decode_notification_token_key(&encoded).unwrap(), [7_u8; 32]);
        assert!(decode_notification_token_key("not-base64").is_err());
        assert!(decode_notification_token_key(&STANDARD.encode([7_u8; 31])).is_err());
    }

    #[test]
    fn enabled_todoist_sync_requires_the_secret_vault_key() {
        run_config_env_probe("todoist_without_secret_key");
        run_config_env_probe("todoist_with_secret_key");
    }

    fn run_config_env_probe(scenario: &str) {
        let mut command = Command::new(std::env::current_exe().unwrap());
        let mut resend_secret_file = None;
        let mut notification_key_file = None;
        command
            .arg("--exact")
            .arg("config::tests::config_env_probe")
            .arg("--ignored")
            .arg("--nocapture")
            .env_clear()
            .env("STRAYLIGHT_CONFIG_ENV_PROBE", scenario)
            .env(
                "DATABASE_URL_RW",
                "postgres://app_rw:unit-secret@db:5432/straylight",
            )
            .env(
                "DATABASE_URL_RO",
                "postgres://app_ro:unit-secret@db:5432/straylight",
            )
            .env("STRAYLIGHT_CONTINUATION_SECRET", "c".repeat(32));
        match scenario {
            "aws_default_chain" => {
                command
                    .env("STRAYLIGHT_ENV", "production")
                    .env("OPENAI_API_KEY", "sk-unit-openai");
            }
            "minio_aliases" => {
                command
                    .env("STRAYLIGHT_MINIO_ENDPOINT", "http://minio.test:9000")
                    .env("STRAYLIGHT_MINIO_REGION", "minio-region")
                    .env("STRAYLIGHT_MINIO_BUCKET", "minio-bucket")
                    .env("STRAYLIGHT_MINIO_ACCESS_KEY", "minio-access")
                    .env("STRAYLIGHT_MINIO_SECRET_KEY", "minio-secret");
            }
            "explicit_overrides" => {
                command
                    .env("STRAYLIGHT_S3_ENDPOINT", "https://objects.example")
                    .env("STRAYLIGHT_S3_FORCE_PATH_STYLE", "false")
                    .env("STRAYLIGHT_S3_CREATE_BUCKET", "false")
                    .env("STRAYLIGHT_S3_ACCESS_KEY", "s3-access")
                    .env("STRAYLIGHT_S3_SECRET_KEY", "s3-secret")
                    .env("STRAYLIGHT_VERBATIM_SPANS", "true");
            }
            "partial_credentials" => {
                command.env("STRAYLIGHT_S3_ACCESS_KEY", "access-without-secret");
            }
            "search_contract_defaults" => {}
            "search_contract_flags" => {
                command
                    .env("STRAYLIGHT_SEARCH_FAIR_SHARE", "true")
                    .env("STRAYLIGHT_SEARCH_TOP1_HYDRATION", "true")
                    .env("STRAYLIGHT_SEARCH_CHAR_CAP", "true")
                    .env("STRAYLIGHT_SEARCH_SECTION_DEMOTION_TOP_N", "8");
            }
            "resume_delta_flag" => {
                command.env("STRAYLIGHT_RESUME_DELTAS", "true");
            }
            "semantic_policy_flags" => {
                command
                    .env("STRAYLIGHT_SEMANTIC_LANE", "true")
                    .env("STRAYLIGHT_EMBED_CACHE", "false")
                    .env("STRAYLIGHT_SEMANTIC_DEADLINE_MS", "0")
                    .env("STRAYLIGHT_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS", "1500")
                    .env("STRAYLIGHT_SEMANTIC_QUERY_CONCURRENCY", "3")
                    .env("STRAYLIGHT_EMBEDDING_BACKFILL_GUARD", "false")
                    .env("STRAYLIGHT_EMBEDDING_BACKFILL_BATCH_CHUNKS", "32")
                    .env("STRAYLIGHT_EMBEDDING_BACKFILL_INTER_BATCH_MS", "500")
                    .env(
                        "STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL",
                        "http://api:8080/health/foreground-latency",
                    )
                    .env(
                        "STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_TIMEOUT_MS",
                        "750",
                    );
            }
            "resend_secret_file" => {
                let mut secret = tempfile::NamedTempFile::new().unwrap();
                writeln!(secret, "re_unit_1234567890abcdef").unwrap();
                command
                    .env("RESEND_API_KEY_FILE", secret.path())
                    .env("AUTH_EMAIL_FROM", "Straylight <login@example.com>")
                    .env("STRAYLIGHT_PUBLIC_URL", "http://localhost:13110");
                resend_secret_file = Some(secret);
            }
            "production_missing_web_mail" => {
                command
                    .env("STRAYLIGHT_ENV", "production")
                    .env("OPENAI_API_KEY", "sk-unit-openai");
            }
            "production_placeholder_web_mail" => {
                command
                    .env("STRAYLIGHT_ENV", "production")
                    .env("OPENAI_API_KEY", "sk-unit-openai")
                    .env("RESEND_API_KEY", "replace-with-resend-key")
                    .env("AUTH_EMAIL_FROM", "Straylight <login@example.com>");
            }
            "invalid_environment" => {
                command.env("STRAYLIGHT_ENV", "Production");
            }
            "notification_api_config" => {
                command
                    .env("STRAYLIGHT_APNS_APP_ID", "com.example.Straylight")
                    .env(
                        "STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY",
                        STANDARD.encode([8_u8; 32]),
                    );
            }
            "notification_app_without_key" => {
                command.env("STRAYLIGHT_APNS_APP_ID", "com.example.Straylight");
            }
            "notification_full_worker" => {
                let mut secret = tempfile::NamedTempFile::new().unwrap();
                writeln!(secret, "{}", STANDARD.encode([9_u8; 32])).unwrap();
                command
                    .env("STRAYLIGHT_APNS_TEAM_ID", "TEAMID1234")
                    .env("STRAYLIGHT_APNS_KEY_ID", "KEYID12345")
                    .env("STRAYLIGHT_APNS_PRIVATE_KEY", "unit-private-key")
                    .env("STRAYLIGHT_APNS_APP_ID", "com.example.Straylight")
                    .env("STRAYLIGHT_APNS_DELIVERY_ENABLED", "true")
                    .env(
                        "STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY_FILE",
                        secret.path(),
                    );
                notification_key_file = Some(secret);
            }
            "notification_partial_provider" => {
                command.env("STRAYLIGHT_APNS_TEAM_ID", "TEAMID1234");
            }
            "notification_provider_missing_token_key" => {
                command
                    .env("STRAYLIGHT_APNS_TEAM_ID", "TEAMID1234")
                    .env("STRAYLIGHT_APNS_KEY_ID", "KEYID12345")
                    .env("STRAYLIGHT_APNS_PRIVATE_KEY", "unit-private-key")
                    .env("STRAYLIGHT_APNS_APP_ID", "com.example.Straylight");
            }
            "todoist_without_secret_key" => {
                command.env("STRAYLIGHT_TODOIST_SYNC_ENABLED", "true");
            }
            "todoist_with_secret_key" => {
                command.env("STRAYLIGHT_TODOIST_SYNC_ENABLED", "true").env(
                    "STRAYLIGHT_SECRET_ENCRYPTION_KEY",
                    STANDARD.encode([11_u8; 32]),
                );
            }
            scenario => panic!("unknown config probe scenario {scenario}"),
        }
        let output = command.output().unwrap();
        drop(resend_secret_file);
        drop(notification_key_file);
        assert!(
            output.status.success(),
            "config probe {scenario} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "executed in an isolated child process by the config environment tests"]
    fn config_env_probe() {
        match std::env::var("STRAYLIGHT_CONFIG_ENV_PROBE")
            .expect("STRAYLIGHT_CONFIG_ENV_PROBE must select a scenario")
            .as_str()
        {
            "aws_default_chain" => {
                assert_eq!(std::env::var("STRAYLIGHT_ENV").as_deref(), Ok("production"));
                let config = Config::from_env().unwrap();
                assert_eq!(config.deployment_environment, "production");
                assert_eq!(config.s3_endpoint, None);
                assert_eq!(config.s3_access_key, None);
                assert_eq!(config.s3_secret_key, None);
                assert!(!config.s3_force_path_style);
                assert!(!config.s3_create_bucket);
                assert!(!config.evaluation_api_enabled);
                assert!(!config.verbatim_spans);
                assert!(!config.supersession_demotion);
                assert!(!config.intention_ledger);
                assert!(!config.messaging_enabled);
                assert_eq!(config.supersession_demotion_weight, 1.5);
                assert!(!config.resume_deltas);
                assert_eq!(config.semantic_deadline, Some(Duration::from_millis(2500)));
                assert_eq!(
                    config.semantic_query_provider_timeout,
                    Duration::from_millis(5000)
                );
                assert_eq!(config.semantic_query_concurrency, 8);
            }
            "minio_aliases" => {
                let config = Config::from_env().unwrap();
                assert_eq!(
                    config.s3_endpoint.as_deref(),
                    Some("http://minio.test:9000")
                );
                assert_eq!(config.s3_region, "minio-region");
                assert_eq!(config.s3_bucket, "minio-bucket");
                assert_eq!(config.s3_access_key.as_deref(), Some("minio-access"));
                assert_eq!(config.s3_secret_key.as_deref(), Some("minio-secret"));
                assert!(config.s3_force_path_style);
                assert!(config.s3_create_bucket);
                assert!(config.evaluation_api_enabled);
                assert!(!config.verbatim_spans);
                assert!(!config.supersession_demotion);
                assert!(!config.intention_ledger);
                assert!(!config.resume_deltas);
            }
            "explicit_overrides" => {
                let config = Config::from_env().unwrap();
                assert_eq!(
                    config.s3_endpoint.as_deref(),
                    Some("https://objects.example")
                );
                assert_eq!(config.s3_access_key.as_deref(), Some("s3-access"));
                assert_eq!(config.s3_secret_key.as_deref(), Some("s3-secret"));
                assert!(!config.s3_force_path_style);
                assert!(!config.s3_create_bucket);
                assert!(config.verbatim_spans);
            }
            "partial_credentials" => {
                let error = Config::from_env().err().expect("partial keys must fail");
                assert!(
                    error
                        .to_string()
                        .contains("access and secret keys must be configured together")
                );
            }
            "search_contract_defaults" => {
                let config = Config::from_env().unwrap();
                assert!(!config.search_fair_share);
                assert!(!config.search_top1_hydration);
                assert!(!config.search_char_cap);
                assert_eq!(config.search_section_demotion_top_n, None);
            }
            "search_contract_flags" => {
                let config = Config::from_env().unwrap();
                assert!(config.search_fair_share);
                assert!(config.search_top1_hydration);
                assert!(config.search_char_cap);
                assert_eq!(config.search_section_demotion_top_n, Some(8));
            }
            "resume_delta_flag" => {
                let config = Config::from_env().unwrap();
                assert!(config.resume_deltas);
            }
            "semantic_policy_flags" => {
                let config = Config::from_env().unwrap();
                assert!(config.semantic_lane);
                assert!(!config.embed_cache);
                assert_eq!(config.semantic_deadline, None);
                assert_eq!(
                    config.semantic_query_provider_timeout,
                    Duration::from_millis(1500)
                );
                assert_eq!(config.semantic_query_concurrency, 3);
                assert!(!config.embedding_backfill_guard);
                assert_eq!(config.embedding_backfill_batch_chunks, 32);
                assert_eq!(
                    config.embedding_backfill_inter_batch_delay,
                    Duration::from_millis(500)
                );
                assert_eq!(
                    config.embedding_backfill_foreground_status_url.as_deref(),
                    Some("http://api:8080/health/foreground-latency")
                );
                assert_eq!(
                    config.embedding_backfill_foreground_status_timeout,
                    Duration::from_millis(750)
                );
            }
            "resend_secret_file" => {
                let config = Config::from_env().unwrap();
                config.validate_web_auth().unwrap();
                assert_eq!(
                    config.resend_api_key.as_deref(),
                    Some("re_unit_1234567890abcdef")
                );
                assert_eq!(
                    config.auth_email_from.as_deref(),
                    Some("Straylight <login@example.com>")
                );
            }
            "production_missing_web_mail" => {
                let config = Config::from_env().unwrap();
                assert!(config.validate_web_auth().is_err());
            }
            "production_placeholder_web_mail" => {
                let config = Config::from_env().unwrap();
                assert!(config.validate_web_auth().is_err());
            }
            "invalid_environment" => {
                let error = Config::from_env()
                    .err()
                    .expect("invalid environment must fail");
                assert!(error.to_string().contains("STRAYLIGHT_ENV"));
            }
            "notification_api_config" => {
                let config = Config::from_env().unwrap();
                assert_eq!(
                    config.apns_app_id.as_deref(),
                    Some("com.example.Straylight")
                );
                assert!(!config.apns_configured());
                assert!(!config.apns_delivery_enabled);
            }
            "notification_app_without_key" => {
                let error = Config::from_env()
                    .err()
                    .expect("app ID without encryption key must fail");
                assert!(error.to_string().contains("must be configured together"));
            }
            "notification_full_worker" => {
                let config = Config::from_env().unwrap();
                assert!(config.apns_configured());
                assert!(config.apns_delivery_enabled);
                assert_eq!(
                    decode_notification_token_key(
                        config.notification_token_encryption_key.as_deref().unwrap()
                    )
                    .unwrap(),
                    [9_u8; 32]
                );
            }
            "notification_partial_provider" => {
                let error = Config::from_env()
                    .err()
                    .expect("partial APNs config must fail");
                assert!(error.to_string().contains("APNs provider credentials"));
            }
            "notification_provider_missing_token_key" => {
                let error = Config::from_env()
                    .err()
                    .expect("APNs provider without token encryption must fail");
                assert!(
                    error
                        .to_string()
                        .contains("STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY")
                );
            }
            "todoist_without_secret_key" => {
                let error = Config::from_env()
                    .err()
                    .expect("enabled Todoist sync without a vault key must fail");
                assert!(
                    error
                        .to_string()
                        .contains("STRAYLIGHT_SECRET_ENCRYPTION_KEY")
                );
            }
            "todoist_with_secret_key" => {
                let config = Config::from_env().unwrap();
                assert!(config.todoist_sync_enabled);
                assert_eq!(
                    decode_secret_encryption_key(config.secret_encryption_key.as_deref().unwrap())
                        .unwrap(),
                    [11_u8; 32]
                );
            }
            scenario => panic!("unknown config probe scenario {scenario}"),
        }
    }
}

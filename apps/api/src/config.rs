use std::{env, net::SocketAddr, str::FromStr, time::Duration};

use crate::error::{ApiError, ApiResult};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url_admin: Option<String>,
    pub database_url_rw: String,
    pub database_url_ro: String,
    pub database_max_connections: u32,
    pub s3_endpoint: String,
    pub s3_region: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub openai_api_key: Option<String>,
    pub openai_base_url: String,
    pub capture_model: String,
    pub capture_max_output_tokens: u64,
    pub dream_model: String,
    pub dream_scheduler_enabled: bool,
    pub dream_scheduler_poll_interval: Duration,
    pub dream_inactivity_window: Duration,
    pub dream_cooldown: Duration,
    pub dream_dirty_threshold: i32,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub embedding_provider: String,
    pub continuation_secret: String,
    pub materialize_token_budget: usize,
    pub request_timeout: Duration,
    pub dev_user_ref: String,
    pub dev_user_name: String,
    pub dev_read_write_token: Option<String>,
    pub dev_read_only_token: Option<String>,
}

impl Config {
    pub fn from_env() -> ApiResult<Self> {
        let bind = env_parse_value(
            "STRAYLIGHT_BIND",
            first_env(&["STRAYLIGHT_BIND", "STRAYLIGHT_BIND_ADDR"])
                .unwrap_or_else(|| "0.0.0.0:8080".to_owned()),
        )?;
        let database_url_rw = required_any(&["DATABASE_URL_RW", "STRAYLIGHT_DATABASE_URL"])?;
        let database_url_ro = first_env(&["DATABASE_URL_RO", "STRAYLIGHT_READ_ONLY_DATABASE_URL"])
            .unwrap_or_else(|| database_url_rw.clone());
        let continuation_secret = required_any(&[
            "STRAYLIGHT_CONTINUATION_SECRET",
            "STRAYLIGHT_CONTINUATION_SIGNING_KEY",
        ])?;
        if continuation_secret.len() < 32 {
            return Err(ApiError::configuration(
                "STRAYLIGHT_CONTINUATION_SECRET must contain at least 32 characters",
            ));
        }

        Ok(Self {
            bind,
            database_url_admin: first_env(&["DATABASE_URL_ADMIN", "STRAYLIGHT_ADMIN_DATABASE_URL"]),
            database_url_rw,
            database_url_ro,
            database_max_connections: env_parse("STRAYLIGHT_DATABASE_MAX_CONNECTIONS", "20")?,
            s3_endpoint: first_env(&["STRAYLIGHT_S3_ENDPOINT", "STRAYLIGHT_MINIO_ENDPOINT"])
                .unwrap_or_else(|| "http://minio:9000".to_owned()),
            s3_region: first_env(&["STRAYLIGHT_S3_REGION", "STRAYLIGHT_MINIO_REGION"])
                .unwrap_or_else(|| "us-east-1".to_owned()),
            s3_bucket: first_env(&["STRAYLIGHT_S3_BUCKET", "STRAYLIGHT_MINIO_BUCKET"])
                .unwrap_or_else(|| "straylight".to_owned()),
            s3_access_key: required_any(&[
                "STRAYLIGHT_S3_ACCESS_KEY",
                "STRAYLIGHT_MINIO_ACCESS_KEY",
            ])?,
            s3_secret_key: required_any(&[
                "STRAYLIGHT_S3_SECRET_KEY",
                "STRAYLIGHT_MINIO_SECRET_KEY",
            ])?,
            openai_api_key: env::var("OPENAI_API_KEY")
                .ok()
                .filter(|value| !value.is_empty()),
            openai_base_url: env_default("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            capture_model: env_default("STRAYLIGHT_CAPTURE_MODEL", "gpt-5.6"),
            capture_max_output_tokens: env_parse(
                "STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS",
                "8192",
            )?,
            dream_model: env_default("STRAYLIGHT_DREAM_MODEL", "gpt-5.6"),
            dream_scheduler_enabled: env_parse("STRAYLIGHT_DREAM_SCHEDULER_ENABLED", "true")?,
            dream_scheduler_poll_interval: Duration::from_secs(env_parse(
                "STRAYLIGHT_DREAM_SCHEDULER_POLL_SECONDS",
                "15",
            )?),
            dream_inactivity_window: Duration::from_secs(env_parse(
                "STRAYLIGHT_DREAM_INACTIVITY_SECONDS",
                "60",
            )?),
            dream_cooldown: Duration::from_secs(env_parse(
                "STRAYLIGHT_DREAM_COOLDOWN_SECONDS",
                "900",
            )?),
            dream_dirty_threshold: env_parse("STRAYLIGHT_DREAM_DIRTY_THRESHOLD", "10")?,
            embedding_model: env_default("STRAYLIGHT_EMBEDDING_MODEL", "text-embedding-3-small"),
            embedding_dimensions: env_parse("STRAYLIGHT_EMBEDDING_DIMENSIONS", "1536")?,
            embedding_provider: env_default("STRAYLIGHT_EMBEDDING_PROVIDER", "openai"),
            continuation_secret,
            materialize_token_budget: env_parse("STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET", "24000")?,
            request_timeout: Duration::from_secs(env_parse(
                "STRAYLIGHT_REQUEST_TIMEOUT_SECONDS",
                "30",
            )?),
            dev_user_ref: env_default("STRAYLIGHT_DEV_USER_REF", "user:local"),
            dev_user_name: env_default("STRAYLIGHT_DEV_USER_NAME", "Local user"),
            dev_read_write_token: env::var("STRAYLIGHT_DEV_READ_WRITE_TOKEN")
                .ok()
                .filter(|value| !value.is_empty()),
            dev_read_only_token: env::var("STRAYLIGHT_DEV_READ_ONLY_TOKEN")
                .ok()
                .filter(|value| !value.is_empty()),
        })
    }
}

fn env_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn first_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

fn required_any(names: &[&str]) -> ApiResult<String> {
    first_env(names)
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

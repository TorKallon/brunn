use std::{env, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use crate::error::{ApiError, ApiResult};

#[derive(Clone)]
pub struct Config {
    pub deployment_environment: String,
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
    pub background_job_lease: Duration,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub embedding_provider: String,
    pub allow_degraded_embeddings: bool,
    pub continuation_secret: String,
    pub materialize_token_budget: usize,
    pub request_timeout: Duration,
    pub readiness_timeout: Duration,
    pub requests_per_minute: u32,
    pub allowed_origins: Vec<String>,
    pub account_export_ttl: Duration,
    pub account_export_temp_dir: PathBuf,
    pub account_deletion_backup_retention_days: i32,
    pub dev_user_ref: String,
    pub dev_user_name: String,
    pub dev_read_write_token: Option<String>,
    pub dev_read_only_token: Option<String>,
}

impl Config {
    pub fn admin_database_url_from_env() -> ApiResult<String> {
        required_any_or_file(&["DATABASE_URL_ADMIN", "STRAYLIGHT_ADMIN_DATABASE_URL"])
    }

    pub fn from_env() -> ApiResult<Self> {
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

        let config = Self {
            deployment_environment: env_default("STRAYLIGHT_ENV", "development"),
            bind,
            database_url_admin: first_env_or_file(&[
                "DATABASE_URL_ADMIN",
                "STRAYLIGHT_ADMIN_DATABASE_URL",
            ])?,
            database_url_rw,
            database_url_ro,
            database_max_connections: env_parse("STRAYLIGHT_DATABASE_MAX_CONNECTIONS", "20")?,
            s3_endpoint: first_env(&["STRAYLIGHT_S3_ENDPOINT", "STRAYLIGHT_MINIO_ENDPOINT"])
                .unwrap_or_else(|| "http://minio:9000".to_owned()),
            s3_region: first_env(&["STRAYLIGHT_S3_REGION", "STRAYLIGHT_MINIO_REGION"])
                .unwrap_or_else(|| "us-east-1".to_owned()),
            s3_bucket: first_env(&["STRAYLIGHT_S3_BUCKET", "STRAYLIGHT_MINIO_BUCKET"])
                .unwrap_or_else(|| "straylight".to_owned()),
            s3_access_key: required_any_or_file(&[
                "STRAYLIGHT_S3_ACCESS_KEY",
                "STRAYLIGHT_MINIO_ACCESS_KEY",
            ])?,
            s3_secret_key: required_any_or_file(&[
                "STRAYLIGHT_S3_SECRET_KEY",
                "STRAYLIGHT_MINIO_SECRET_KEY",
            ])?,
            openai_api_key: first_env_or_file(&["OPENAI_API_KEY"])?,
            openai_base_url: env_default("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            capture_model: env_default("STRAYLIGHT_CAPTURE_MODEL", "gpt-5.6"),
            capture_max_output_tokens: env_parse("STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS", "8192")?,
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
            background_job_lease: Duration::from_secs(env_parse(
                "STRAYLIGHT_BACKGROUND_JOB_LEASE_SECONDS",
                "300",
            )?),
            embedding_model: env_default("STRAYLIGHT_EMBEDDING_MODEL", "text-embedding-3-small"),
            embedding_dimensions: env_parse("STRAYLIGHT_EMBEDDING_DIMENSIONS", "1536")?,
            embedding_provider: env_default("STRAYLIGHT_EMBEDDING_PROVIDER", "openai"),
            allow_degraded_embeddings: env_parse("STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS", "false")?,
            continuation_secret,
            materialize_token_budget: env_parse("STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET", "24000")?,
            request_timeout: Duration::from_secs(env_parse(
                "STRAYLIGHT_REQUEST_TIMEOUT_SECONDS",
                "30",
            )?),
            readiness_timeout: Duration::from_secs(env_parse(
                "STRAYLIGHT_READINESS_TIMEOUT_SECONDS",
                "3",
            )?),
            requests_per_minute: env_parse("STRAYLIGHT_REQUESTS_PER_MINUTE", "600")?,
            allowed_origins: parse_allowed_origins(&env_default("STRAYLIGHT_ALLOWED_ORIGINS", ""))?,
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
        };
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
            || self.s3_secret_key.to_ascii_lowercase().contains("replace")
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
        if self.s3_secret_key.len() < 16 {
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
        Ok(())
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
    use std::io::Write;

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
}

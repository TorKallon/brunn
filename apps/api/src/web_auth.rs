use std::{collections::HashSet, time::Duration};

use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::SaltString,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zxcvbn::{Score, zxcvbn};

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{ApiEnvelope, Capability, CredentialId, UserId},
};

const SESSION_TTL_DAYS: i64 = 30;
const SESSION_TTL_SECONDS: i64 = SESSION_TTL_DAYS * 24 * 60 * 60;
const RESET_TTL_MINUTES: i64 = 30;
const SESSION_COOKIE_PRODUCTION: &str = "__Host-straylight_session";
const CSRF_COOKIE_PRODUCTION: &str = "__Host-straylight_csrf";
const SESSION_COOKIE_DEVELOPMENT: &str = "straylight_session";
const CSRF_COOKIE_DEVELOPMENT: &str = "straylight_csrf";
const CSRF_HEADER: HeaderName = HeaderName::from_static("x-csrf-token");
const MIN_PASSWORD_CHARS: usize = 15;
const MAX_PASSWORD_CHARS: usize = 1024;
const MAX_PASSWORD_BYTES: usize = MAX_PASSWORD_CHARS * 4;
const ARGON2_MEMORY_KIB: u32 = 19_456;
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;
const ARGON2_OUTPUT_BYTES: usize = 32;
const FORGOT_RESPONSE_FLOOR: Duration = Duration::from_millis(250);
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c3RyYXlsaWdodC1kdW1teSE$G4cTCRccjpoNV+1tywKuUd5bq/LsW4NT7Sq0Wt9H1Hw";

const BLOCKED_PASSWORDS: &[&str] = &[
    "123456789012345",
    "1234567890123456",
    "abc123abc123abc",
    "adminadminadmin",
    "changemechangeme",
    "correcthorsebatterystaple",
    "correct horse battery staple",
    "dragon123456789",
    "footballfootball",
    "iloveyouiloveyou",
    "letmeinletmein",
    "monkey123456789",
    "passwordpassword",
    "princessprincess",
    "qwertyuiopasdfgh",
    "straylight2026",
    "straylightstraylight",
    "sunshinesunshine",
    "thisisapassword",
    "trustno1trustno1",
    "welcome123456789",
];

type HmacSha256 = Hmac<Sha256>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[serde(alias = "username")]
    email: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgotPasswordRequest {
    identifier: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetPasswordRequest {
    token: String,
    password: String,
}

struct WebIdentity {
    user_id: Uuid,
    email: String,
    password_hash: Option<String>,
}

pub struct AuthenticatedWebSession {
    pub auth: AuthContext,
    pub session_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub email: String,
    pub display_name: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Response> {
    state.preauth_rate_limiter.check()?;
    let identifier = normalize_identifier(&request.email)
        .ok_or_else(|| ApiError::invalid("enter a valid email address"))?;
    let identity = lookup_identity(&state, &identifier).await?;
    let rate_key = login_rate_key(
        &state.config.continuation_secret,
        &identifier,
        identity.as_ref().map(|value| value.user_id),
    )?;
    let rate_allowed = consume_rate_limit(
        &state,
        "login",
        &rate_key,
        identity.as_ref().map(|value| value.user_id),
    )
    .await?;

    let candidate_hash = match identity
        .as_ref()
        .and_then(|value| value.password_hash.as_deref())
    {
        Some(value) => value.to_owned(),
        None => DUMMY_PASSWORD_HASH.to_owned(),
    };
    let password = normalize_password_input(&request.password);
    let password_input_valid = password.is_some();
    let password = password.unwrap_or_else(|| "invalid-password-input".to_owned());
    let password_valid = verify_password(&state, password, candidate_hash.clone()).await?;
    let Some(identity) = identity.filter(|value| {
        value.password_hash.is_some() && password_input_valid && password_valid && rate_allowed
    }) else {
        let result = if rate_allowed {
            "failed"
        } else {
            "rate_limited"
        };
        metrics::counter!("auth.password_attempts", "result" => result).increment(1);
        return Err(invalid_credentials());
    };

    clear_rate_limit(&state, "login", &rate_key).await?;
    let token = generate_token("sws_");
    let expires_at = Utc::now() + ChronoDuration::days(SESSION_TTL_DAYS);
    let created =
        sqlx::query_scalar::<_, Uuid>("SELECT straylight_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(identity.user_id)
            .bind(hash_secret(&token))
            .bind(expires_at)
            .bind(candidate_hash)
            .bind(&identifier)
            .fetch_one(&state.auth_pool)
            .await;
    if let Err(error) = created {
        if database_error_is(&error, "P0002") {
            metrics::counter!("auth.password_attempts", "result" => "stale").increment(1);
            return Err(invalid_credentials());
        }
        return Err(error.into());
    }
    let session = authenticate_session(&state, &token)
        .await?
        .ok_or_else(ApiError::unauthenticated)?;
    metrics::counter!("auth.password_attempts", "result" => "success").increment(1);
    let response = auth_envelope(&session).into_response();
    Ok(with_session_cookies(response, &state, &token, expires_at))
}

pub async fn session(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    state.preauth_rate_limiter.check()?;
    let Some(token) = raw_session_cookie(&headers, &state) else {
        return Ok(missing_session_response());
    };
    if !valid_session_token(&token) {
        return Ok(with_cleared_cookies(
            ApiError::unauthenticated().into_response(),
            &state,
        ));
    };
    let Some(session) = authenticate_session(&state, &token).await? else {
        return Ok(with_cleared_cookies(
            ApiError::unauthenticated().into_response(),
            &state,
        ));
    };
    state
        .request_rate_limiter
        .check(session.auth.credential_id.0)?;
    Ok(with_no_store(auth_envelope(&session).into_response()))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    state.preauth_rate_limiter.check()?;
    let response = logout_success_response();
    let Some(token) = raw_session_cookie(&headers, &state) else {
        return Ok(response);
    };
    require_csrf(&headers, &state, &token)?;
    if valid_session_token(&token) {
        let _ = sqlx::query_scalar::<_, bool>("SELECT straylight_auth.revoke_web_session($1)")
            .bind(hash_secret(&token))
            .fetch_one(&state.auth_pool)
            .await?;
    }
    Ok(with_cleared_cookies(response, &state))
}

pub async fn forgot_password(
    State(state): State<AppState>,
    Json(request): Json<ForgotPasswordRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let started = tokio::time::Instant::now();
    state.preauth_rate_limiter.check()?;
    let Some(identifier) = normalize_identifier(&request.identifier) else {
        return Ok(forgot_password_response(started).await);
    };
    let identity = lookup_identity(&state, &identifier).await?;
    let rate_key = match identity.as_ref() {
        Some(identity) => resolved_user_rate_key(&state, "reset", identity.user_id)?,
        None => identifier_rate_key(&state.config.continuation_secret, "reset", &identifier)?,
    };
    if !consume_rate_limit(
        &state,
        "reset",
        &rate_key,
        identity.as_ref().map(|value| value.user_id),
    )
    .await?
    {
        metrics::counter!("auth.password_reset_requests", "result" => "rate_limited").increment(1);
        return Ok(forgot_password_response(started).await);
    }
    let Some(identity) = identity else {
        metrics::counter!("auth.password_reset_requests", "result" => "accepted").increment(1);
        return Ok(forgot_password_response(started).await);
    };
    if !state.config.web_email_configured() {
        metrics::counter!("auth.password_reset_requests", "result" => "not_configured")
            .increment(1);
        return Ok(forgot_password_response(started).await);
    }

    let token = generate_token("swr_");
    let expires_at = Utc::now() + ChronoDuration::minutes(RESET_TTL_MINUTES);
    let reset_url = reset_url(&state.config.public_url, &token);
    let reset_id = match sqlx::query_scalar::<_, Uuid>(
        "SELECT straylight_auth.issue_password_reset($1,$2,$3,$4)",
    )
    .bind(identity.user_id)
    .bind(hash_secret(&token))
    .bind(expires_at)
    .bind(&identity.email)
    .fetch_one(&state.auth_pool)
    .await
    {
        Ok(reset_id) => reset_id,
        Err(_) => {
            metrics::counter!("auth.password_reset_requests", "result" => "database_failed")
                .increment(1);
            tracing::warn!(
                provider = "resend",
                failure = "reset_persistence",
                "password reset email delivery failed"
            );
            return Ok(forgot_password_response(started).await);
        }
    };
    let delivery_state = state.clone();
    tokio::spawn(async move {
        let delivery =
            send_reset_email(&delivery_state, &identity.email, &reset_url, reset_id).await;
        let result = if delivery { "sent" } else { "delivery_failed" };
        metrics::counter!("auth.password_reset_requests", "result" => result).increment(1);
    });
    Ok(forgot_password_response(started).await)
}

pub async fn reset_password(
    State(state): State<AppState>,
    Json(request): Json<ResetPasswordRequest>,
) -> ApiResult<Response> {
    state.preauth_rate_limiter.check()?;
    validate_reset_token(&request.token)?;
    let password_hash = hash_password(&state, request.password).await?;
    let consumed =
        sqlx::query_scalar::<_, Uuid>("SELECT straylight_auth.consume_password_reset($1,$2)")
            .bind(hash_secret(&request.token))
            .bind(password_hash)
            .fetch_optional(&state.auth_pool)
            .await;
    match consumed {
        Ok(Some(_)) => {
            metrics::counter!("auth.password_resets", "result" => "success").increment(1);
            let response = Json(ApiEnvelope::complete(json!({
                "message": "Password reset complete."
            })))
            .into_response();
            Ok(with_cleared_cookies(response, &state))
        }
        Ok(None) => Err(invalid_reset()),
        Err(error) if database_error_is(&error, "P0002") => {
            metrics::counter!("auth.password_resets", "result" => "invalid").increment(1);
            Err(invalid_reset())
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn authenticate_session(
    state: &AppState,
    token: &str,
) -> ApiResult<Option<AuthenticatedWebSession>> {
    if !valid_session_token(token) {
        return Ok(None);
    }
    let row = sqlx::query("SELECT * FROM straylight_auth.authenticate_web_session($1)")
        .bind(hash_secret(token))
        .fetch_optional(&state.auth_pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let capabilities: Vec<String> = row.try_get("capabilities")?;
    let capabilities: HashSet<_> = capabilities.into_iter().collect();
    let read_only = !capabilities.contains(Capability::Save.as_str())
        && !capabilities.contains(Capability::Checkpoint.as_str());
    Ok(Some(AuthenticatedWebSession {
        auth: AuthContext {
            credential_id: CredentialId(row.try_get("credential_id")?),
            user_id: UserId(row.try_get("user_id")?),
            capabilities,
            scope_refs: row.try_get("scope_refs")?,
            read_only,
        },
        session_id: row.try_get("web_session_id")?,
        expires_at: row.try_get("expires_at")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
    }))
}

pub fn session_token(headers: &HeaderMap, state: &AppState) -> Option<String> {
    raw_session_cookie(headers, state).filter(|value| valid_session_token(value))
}

fn raw_session_cookie(headers: &HeaderMap, state: &AppState) -> Option<String> {
    cookie_value(headers, session_cookie_name(state))
}

pub fn require_csrf(headers: &HeaderMap, state: &AppState, session_token: &str) -> ApiResult<()> {
    let expected = derive_csrf(session_token);
    let header_value = headers
        .get(&CSRF_HEADER)
        .and_then(|value| value.to_str().ok());
    let cookie_value = cookie_value(headers, csrf_cookie_name(state));
    if header_value.is_some_and(|value| constant_time_eq(value, &expected))
        && cookie_value.is_some_and(|value| constant_time_eq(&value, &expected))
    {
        return Ok(());
    }
    metrics::counter!("auth.csrf", "result" => "failed").increment(1);
    Err(ApiError::public(
        StatusCode::FORBIDDEN,
        "csrf_validation_failed",
        "the session request lacks a valid CSRF token",
    ))
}

fn auth_envelope(session: &AuthenticatedWebSession) -> Json<ApiEnvelope<Value>> {
    Json(ApiEnvelope::complete(json!({
        "user": {
            "id": format!("user:{}", session.auth.user_id.0),
            "display_name": session.display_name,
            "username": session.email,
            "email": session.email
        },
        "expires_at": session.expires_at.to_rfc3339_opts(SecondsFormat::Millis, true)
    })))
}

async fn lookup_identity(state: &AppState, identifier: &str) -> ApiResult<Option<WebIdentity>> {
    let row = sqlx::query("SELECT * FROM straylight_auth.lookup_web_identity($1)")
        .bind(identifier)
        .fetch_optional(&state.auth_pool)
        .await?;
    row.map(|row| {
        Ok(WebIdentity {
            user_id: row.try_get("user_id")?,
            email: row.try_get("email")?,
            password_hash: row.try_get("password_hash")?,
        })
    })
    .transpose()
}

async fn consume_rate_limit(
    state: &AppState,
    kind: &str,
    identifier_hash: &str,
    user_id: Option<Uuid>,
) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT straylight_auth.consume_web_auth_rate_limit($1,$2,$3)")
            .bind(kind)
            .bind(identifier_hash)
            .bind(user_id)
            .fetch_one(&state.auth_pool)
            .await?,
    )
}

async fn clear_rate_limit(state: &AppState, kind: &str, identifier_hash: &str) -> ApiResult<()> {
    sqlx::query("SELECT straylight_auth.clear_web_auth_rate_limit($1,$2)")
        .bind(kind)
        .bind(identifier_hash)
        .execute(&state.auth_pool)
        .await?;
    Ok(())
}

async fn send_reset_email(
    state: &AppState,
    recipient: &str,
    reset_url: &str,
    reset_id: Uuid,
) -> bool {
    let (Some(api_key), Some(from)) = (
        state.config.resend_api_key.as_deref(),
        state.config.auth_email_from.as_deref(),
    ) else {
        tracing::warn!(
            provider = "resend",
            failure = "not_configured",
            "password reset email delivery failed"
        );
        return false;
    };
    let mut payload = json!({
        "from": from,
        "to": [recipient],
        "subject": "Reset your Straylight password",
        "text": format!(
            "Use this link within 30 minutes to reset your Straylight password:\n\n{reset_url}\n\nIf you did not request this, you can ignore this email."
        )
    });
    if let Some(reply_to) = state.config.auth_email_reply_to.as_deref() {
        payload["reply_to"] = Value::String(reply_to.to_owned());
    }
    let request = state
        .web_auth_email_client
        .post("https://api.resend.com/emails")
        .bearer_auth(api_key)
        .header(
            "Idempotency-Key",
            format!("password-reset-{}", reset_id.simple()),
        )
        .json(&payload)
        .send()
        .await;
    match request {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            tracing::warn!(
                provider = "resend",
                status = response.status().as_u16(),
                "password reset email delivery failed"
            );
            false
        }
        Err(_) => {
            tracing::warn!(
                provider = "resend",
                failure = "transport",
                "password reset email delivery failed"
            );
            false
        }
    }
}

async fn forgot_password_response(started: tokio::time::Instant) -> Json<ApiEnvelope<Value>> {
    tokio::time::sleep_until(started + FORGOT_RESPONSE_FLOOR).await;
    Json(ApiEnvelope::complete(json!({
        "message": "If the account exists, a password reset email will be sent."
    })))
}

fn normalize_identifier(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 254 || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_lowercase())
}

fn normalize_password_input(value: &str) -> Option<String> {
    if value.len() > MAX_PASSWORD_BYTES
        || value.chars().count() > MAX_PASSWORD_CHARS
        || value.contains('\0')
    {
        return None;
    }
    let normalized: String = value.nfc().collect();
    (normalized.len() <= MAX_PASSWORD_BYTES && normalized.chars().count() <= MAX_PASSWORD_CHARS)
        .then_some(normalized)
}

#[cfg(test)]
fn prepare_new_password(value: &str) -> ApiResult<String> {
    let normalized = normalize_password_input(value).ok_or_else(password_policy_violation)?;
    validate_normalized_password(&normalized)?;
    Ok(normalized)
}

fn validate_normalized_password(normalized: &str) -> ApiResult<()> {
    let normalized_lower = normalized.to_lowercase();
    let estimate = zxcvbn(normalized, &["straylight"]);
    if normalized.chars().count() < MIN_PASSWORD_CHARS
        || BLOCKED_PASSWORDS.contains(&normalized_lower.as_str())
        || is_predictable_password(&normalized_lower)
        || matches!(estimate.score(), Score::Zero | Score::One | Score::Two)
    {
        return Err(password_policy_violation());
    }
    Ok(())
}

fn is_predictable_password(value: &str) -> bool {
    let characters: Vec<char> = value.chars().collect();
    if characters.len() >= MIN_PASSWORD_CHARS
        && characters
            .iter()
            .all(|character| *character == characters[0])
    {
        return true;
    }
    for width in 1..=(characters.len() / 2).min(16) {
        if characters.len().is_multiple_of(width)
            && characters
                .chunks(width)
                .all(|chunk| chunk == &characters[..width])
        {
            return true;
        }
    }
    [
        "012345678901234567890123456789",
        "987654321098765432109876543210",
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
        "zyxwvutsrqponmlkjihgfedcbazyxwvutsrqponmlkjihgfedcba",
        "qwertyuiopasdfghjklzxcvbnmqwertyuiopasdfghjklzxcvbnm",
    ]
    .iter()
    .any(|sequence| sequence.contains(value))
}

fn validate_reset_token(value: &str) -> ApiResult<()> {
    if value.starts_with("swr_")
        && value.len() == 47
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Ok(());
    }
    Err(invalid_reset())
}

fn invalid_credentials() -> ApiError {
    ApiError::public(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "the email or password is invalid",
    )
}

fn database_error_is(error: &sqlx::Error, expected: &str) -> bool {
    error
        .as_database_error()
        .and_then(|value| value.code())
        .is_some_and(|code| code == expected)
}

fn invalid_reset() -> ApiError {
    ApiError::public(
        StatusCode::BAD_REQUEST,
        "invalid_password_reset",
        "the password reset token is invalid or expired",
    )
}

fn password_policy_violation() -> ApiError {
    ApiError::public(
        StatusCode::UNPROCESSABLE_ENTITY,
        "password_policy_violation",
        "choose at least 15 characters and avoid common or predictable passwords",
    )
}

async fn hash_password(state: &AppState, password: String) -> ApiResult<String> {
    let password = normalize_password_input(&password).ok_or_else(password_policy_violation)?;
    let permit = password_work_permit(state)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        validate_normalized_password(&password)?;
        hash_password_sync(&password)
    })
    .await
    .map_err(|_| ApiError::Internal("password hashing task failed".to_owned()))?
}

fn hash_password_sync(password: &str) -> ApiResult<String> {
    let mut salt_bytes = [0_u8; 16];
    rand::rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|_| ApiError::Internal("could not generate password salt".to_owned()))?;
    configured_argon2()
        .hash_password(password.as_bytes(), &salt)
        .map(|value| value.to_string())
        .map_err(|_| ApiError::Internal("could not hash password".to_owned()))
}

async fn verify_password(state: &AppState, password: String, encoded: String) -> ApiResult<bool> {
    let permit = password_work_permit(state)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let Ok(hash) = PasswordHash::new(&encoded) else {
            return false;
        };
        configured_argon2()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
    .await
    .map_err(|_| ApiError::Internal("password verification task failed".to_owned()))
}

fn configured_argon2() -> Argon2<'static> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(ARGON2_OUTPUT_BYTES),
    )
    .expect("fixed Argon2id parameters are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn password_work_permit(state: &AppState) -> ApiResult<tokio::sync::OwnedSemaphorePermit> {
    state
        .web_auth_password_limiter
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::public(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication_busy",
                "authentication is temporarily busy; try again",
            )
        })
}

fn generate_token(prefix: &str) -> String {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(secret))
}

fn hash_secret(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn identifier_rate_key(secret: &str, kind: &str, identifier: &str) -> ApiResult<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::Internal("could not initialize auth rate limiter".to_owned()))?;
    mac.update(b"straylight.web-auth-rate.v1\0");
    mac.update(kind.as_bytes());
    mac.update(b"\0");
    mac.update(identifier.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn login_rate_key(secret: &str, identifier: &str, user_id: Option<Uuid>) -> ApiResult<String> {
    match user_id {
        Some(user_id) => identifier_rate_key(secret, "login-user", &user_id.to_string()),
        None => identifier_rate_key(secret, "login", identifier),
    }
}

fn resolved_user_rate_key(state: &AppState, kind: &str, user_id: Uuid) -> ApiResult<String> {
    identifier_rate_key(
        &state.config.continuation_secret,
        &format!("{kind}-user"),
        &user_id.to_string(),
    )
}

fn derive_csrf(session_token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"straylight.web-csrf.v1\0");
    digest.update(session_token.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.finalize())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len() && bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn valid_session_token(value: &str) -> bool {
    value.starts_with("sws_")
        && value.len() == 47
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn reset_url(public_url: &str, token: &str) -> String {
    format!(
        "{}/reset-password#token={token}",
        public_url.trim_end_matches('/')
    )
}

fn session_cookie_name(state: &AppState) -> &'static str {
    if state.config.secure_web_cookies() {
        SESSION_COOKIE_PRODUCTION
    } else {
        SESSION_COOKIE_DEVELOPMENT
    }
}

fn csrf_cookie_name(state: &AppState) -> &'static str {
    if state.config.secure_web_cookies() {
        CSRF_COOKIE_PRODUCTION
    } else {
        CSRF_COOKIE_DEVELOPMENT
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(cookie_name, value)| (cookie_name == name).then(|| value.to_owned()))
}

fn with_session_cookies(
    mut response: Response,
    state: &AppState,
    token: &str,
    _expires_at: DateTime<Utc>,
) -> Response {
    let secure = if state.config.secure_web_cookies() {
        "; Secure"
    } else {
        ""
    };
    let csrf = derive_csrf(token);
    let session_cookie = format!(
        "{}={token}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict{secure}",
        session_cookie_name(state),
        SESSION_TTL_SECONDS
    );
    let csrf_cookie = format!(
        "{}={csrf}; Path=/; Max-Age={}; SameSite=Strict{secure}",
        csrf_cookie_name(state),
        SESSION_TTL_SECONDS
    );
    append_set_cookie(&mut response, &session_cookie);
    append_set_cookie(&mut response, &csrf_cookie);
    with_no_store(response)
}

fn with_cleared_cookies(mut response: Response, state: &AppState) -> Response {
    let secure = if state.config.secure_web_cookies() {
        "; Secure"
    } else {
        ""
    };
    for name in [session_cookie_name(state), csrf_cookie_name(state)] {
        append_set_cookie(
            &mut response,
            &format!(
                "{name}=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; SameSite=Strict{secure}"
            ),
        );
    }
    with_no_store(response)
}

fn append_set_cookie(response: &mut Response, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn with_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn missing_session_response() -> Response {
    with_no_store(ApiError::unauthenticated().into_response())
}

fn logout_success_response() -> Response {
    with_no_store(
        Json(ApiEnvelope::complete(json!({
            "message": "Signed out."
        })))
        .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_sessions_persist_for_thirty_days() {
        assert_eq!(SESSION_TTL_DAYS, 30);
        assert_eq!(SESSION_TTL_SECONDS, 2_592_000);
    }

    #[test]
    fn random_tokens_have_256_bits_and_are_not_reused() {
        let first = generate_token("sws_");
        let second = generate_token("sws_");
        assert_eq!(first.len(), 47);
        assert!(valid_session_token(&first));
        assert_ne!(first, second);
        assert_ne!(hash_secret(&first), first);
    }

    #[test]
    fn csrf_is_session_bound_and_compared_in_constant_time() {
        let first = derive_csrf("sws_first");
        let second = derive_csrf("sws_second");
        assert_ne!(first, second);
        assert!(constant_time_eq(&first, &first));
        assert!(!constant_time_eq(&first, &second));
    }

    #[test]
    fn password_hashes_are_argon2id_phc_and_verify() {
        let hash = hash_password_sync("a sufficiently long password").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(hash.contains("$m=19456,t=2,p=1$"));
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(
            configured_argon2()
                .verify_password(b"a sufficiently long password", &parsed)
                .is_ok()
        );
        assert!(
            configured_argon2()
                .verify_password(b"not the password", &parsed)
                .is_err()
        );
        let dummy = PasswordHash::new(DUMMY_PASSWORD_HASH).unwrap();
        assert!(
            configured_argon2()
                .verify_password(b"not-a-real-password", &dummy)
                .is_ok()
        );
    }

    #[test]
    fn reset_secret_is_only_in_the_fragment() {
        let token = generate_token("swr_");
        let url = reset_url("https://straylight.rourkem.com", &token);
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(parsed.path(), "/reset-password");
        assert!(parsed.query().is_none());
        let expected_fragment = format!("token={token}");
        assert_eq!(parsed.fragment(), Some(expected_fragment.as_str()));
    }

    #[test]
    fn password_policy_is_normalized_bounded_and_blocks_common_values() {
        assert!(prepare_new_password("violet-archway-7!orbits-copper").is_ok());
        for password in [
            "short",
            "passwordpassword1",
            "straylightpassword",
            "correct horse battery staple",
            "12345678901234567890",
        ] {
            assert!(
                prepare_new_password(password).is_err(),
                "{password:?} must be rejected"
            );
        }
        assert_eq!(normalize_password_input("e\u{301}"), Some("é".to_owned()));
        assert!(normalize_password_input(&"🛰".repeat(MAX_PASSWORD_CHARS)).is_some());
        assert!(normalize_password_input(&"🛰".repeat(MAX_PASSWORD_CHARS + 1)).is_none());
        assert!(normalize_password_input(&"x".repeat(MAX_PASSWORD_BYTES + 1)).is_none());
        assert!(normalize_identifier(" Owner ").is_some_and(|value| value == "owner"));
        assert!(normalize_identifier("").is_none());
        assert_eq!(
            normalize_identifier(" Owner.Name ").as_deref(),
            Some("owner.name")
        );
        assert_eq!(
            normalize_identifier(" Owner@Example.com ").as_deref(),
            Some("owner@example.com")
        );
    }

    #[test]
    fn resolved_user_rate_bucket_is_shared_across_forgot_password_aliases() {
        let secret = "c".repeat(32);
        let user_id = Uuid::now_v7();
        let username_bucket = identifier_rate_key(&secret, "reset", "owner").unwrap();
        let email_bucket = identifier_rate_key(&secret, "reset", "owner@example.com").unwrap();
        let first_user_bucket =
            identifier_rate_key(&secret, "reset-user", &user_id.to_string()).unwrap();
        let second_user_bucket =
            identifier_rate_key(&secret, "reset-user", &user_id.to_string()).unwrap();
        assert_ne!(username_bucket, email_bucket);
        assert_eq!(first_user_bucket, second_user_bucket);
        assert_ne!(first_user_bucket, username_bucket);
    }

    #[test]
    fn resolved_user_rate_bucket_is_shared_across_login_aliases() {
        let secret = "c".repeat(32);
        let user_id = Uuid::now_v7();
        let username_bucket = login_rate_key(&secret, "owner", Some(user_id)).unwrap();
        let email_bucket = login_rate_key(&secret, "owner@example.com", Some(user_id)).unwrap();
        let unknown_bucket = login_rate_key(&secret, "unknown@example.com", None).unwrap();
        assert_eq!(username_bucket, email_bucket);
        assert_ne!(username_bucket, unknown_bucket);
    }

    #[test]
    fn login_request_uses_email_and_accepts_legacy_username_payloads() {
        let current: LoginRequest = serde_json::from_value(json!({
            "email": "owner@example.com",
            "password": "not-a-real-password"
        }))
        .unwrap();
        assert_eq!(current.email, "owner@example.com");

        let legacy: LoginRequest = serde_json::from_value(json!({
            "username": "owner",
            "password": "not-a-real-password"
        }))
        .unwrap();
        assert_eq!(legacy.email, "owner");
    }

    #[test]
    fn missing_cookie_responses_never_delete_browser_cookies() {
        for response in [missing_session_response(), logout_success_response()] {
            assert!(response.headers().get(header::SET_COOKIE).is_none());
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL),
                Some(&HeaderValue::from_static("no-store"))
            );
        }
    }
}

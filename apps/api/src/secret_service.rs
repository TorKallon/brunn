//! Trusted-agent secret vault. Values are AES-256-GCM encrypted with a key
//! that never enters Postgres; the additional authenticated data binds each
//! ciphertext to the deployment environment, user, secret, and version so
//! stored ciphertext cannot be replayed across rows or environments.
//!
//! Secret values must never reach the memory corpus, embeddings, dreaming,
//! search, exports, object storage, logs, traces, or metrics. Only bounded
//! operational metadata (operation kind, result, duration) is emitted.

use std::time::Instant;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use axum::{
    Extension, Json,
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    config::decode_secret_encryption_key,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
};

const MAX_SECRET_VALUE_BYTES: usize = 64 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 1_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutRequest {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PutResponse {
    pub secret_ref: String,
    pub name: String,
    pub version: i32,
    pub status: &'static str,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct GetResponse {
    pub secret_ref: String,
    pub name: String,
    pub value: String,
    pub version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct SecretMetadata {
    pub secret_ref: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub secrets: Vec<SecretMetadata>,
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub secret_ref: String,
    pub name: String,
    pub status: &'static str,
}

pub async fn put(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<PutRequest>,
) -> ApiResult<Json<PutResponse>> {
    let started = Instant::now();
    let result = put_inner(&state, &auth, request).await;
    record_operation("put", started, &result);
    result.map(Json)
}

async fn put_inner(
    state: &AppState,
    auth: &AuthContext,
    request: PutRequest,
) -> ApiResult<PutResponse> {
    require_write(auth)?;
    let name = normalize_name(&request.name)?;
    let description = normalize_description(request.description)?;
    if request.value.is_empty() {
        return Err(ApiError::invalid("secret value must not be empty"));
    }
    if request.value.len() > MAX_SECRET_VALUE_BYTES {
        return Err(ApiError::invalid(format!(
            "secret value is limited to {MAX_SECRET_VALUE_BYTES} bytes"
        )));
    }
    let key = encryption_key(state)?;
    let mut tx = state.begin_write(auth).await?;
    let existing = sqlx::query(
        r#"
        SELECT id,version FROM straylight.secrets
        WHERE user_id=$1 AND name=$2
        FOR UPDATE
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?;
    let (secret_id, version, updated_at) = match existing {
        Some(row) => {
            let secret_id: Uuid = row.try_get("id")?;
            let version = row
                .try_get::<i32, _>("version")?
                .checked_add(1)
                .ok_or_else(|| ApiError::Internal("secret version overflow".to_owned()))?;
            let aad = secret_value_aad(
                &state.config.deployment_environment,
                auth.user_id.0,
                secret_id,
                version,
            );
            let (ciphertext, nonce) = encrypt_secret_value(&key, &aad, &request.value)?;
            let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
                r#"
                UPDATE straylight.secrets
                SET value_ciphertext=$3,value_nonce=$4,version=$5,
                    description=coalesce($6,description),
                    updated_by_credential_id=$7,updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                RETURNING updated_at
                "#,
            )
            .bind(auth.user_id.0)
            .bind(secret_id)
            .bind(&ciphertext)
            .bind(&nonce)
            .bind(version)
            .bind(&description)
            .bind(auth.credential_id.0)
            .fetch_one(&mut *tx)
            .await?;
            (secret_id, version, updated_at)
        }
        None => {
            let secret_id = Uuid::now_v7();
            let version = 1;
            let aad = secret_value_aad(
                &state.config.deployment_environment,
                auth.user_id.0,
                secret_id,
                version,
            );
            let (ciphertext, nonce) = encrypt_secret_value(&key, &aad, &request.value)?;
            let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
                r#"
                INSERT INTO straylight.secrets (
                  id,user_id,name,description,value_ciphertext,value_nonce,
                  version,created_by_credential_id,updated_by_credential_id
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8)
                RETURNING updated_at
                "#,
            )
            .bind(secret_id)
            .bind(auth.user_id.0)
            .bind(&name)
            .bind(&description)
            .bind(&ciphertext)
            .bind(&nonce)
            .bind(version)
            .bind(auth.credential_id.0)
            .fetch_one(&mut *tx)
            .await?;
            (secret_id, version, updated_at)
        }
    };
    record_access(&mut tx, auth, secret_id, "put").await?;
    tx.commit().await?;
    Ok(PutResponse {
        secret_ref: format_ref(secret_id),
        name,
        version,
        status: "stored",
        updated_at,
    })
}

pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<GetRequest>,
) -> ApiResult<Response> {
    let started = Instant::now();
    let result = get_inner(&state, &auth, request).await;
    record_operation("get", started, &result);
    // Secret values must never be cached by any intermediary or client.
    result.map(|response| ([(header::CACHE_CONTROL, "no-store")], Json(response)).into_response())
}

async fn get_inner(
    state: &AppState,
    auth: &AuthContext,
    request: GetRequest,
) -> ApiResult<GetResponse> {
    require_read(auth)?;
    let name = normalize_name(&request.name)?;
    let key = encryption_key(state)?;
    let mut tx = state.begin_write(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT id,description,value_ciphertext,value_nonce,version,updated_at
        FROM straylight.secrets
        WHERE user_id=$1 AND name=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("secret", &name))?;
    let secret_id: Uuid = row.try_get("id")?;
    let version: i32 = row.try_get("version")?;
    let ciphertext: Vec<u8> = row.try_get("value_ciphertext")?;
    let nonce: Vec<u8> = row.try_get("value_nonce")?;
    let aad = secret_value_aad(
        &state.config.deployment_environment,
        auth.user_id.0,
        secret_id,
        version,
    );
    let value = decrypt_secret_value(&key, &aad, &ciphertext, &nonce)?;
    record_access(&mut tx, auth, secret_id, "get").await?;
    tx.commit().await?;
    Ok(GetResponse {
        secret_ref: format_ref(secret_id),
        name,
        value,
        version,
        description: row.try_get("description")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<ListResponse>> {
    let started = Instant::now();
    let result = list_inner(&state, &auth).await;
    record_operation("list", started, &result);
    result.map(Json)
}

async fn list_inner(state: &AppState, auth: &AuthContext) -> ApiResult<ListResponse> {
    require_list(auth)?;
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT secret.id,secret.name,secret.description,secret.version,
               secret.created_at,secret.updated_at,
               last_used.recorded_at AS last_used_at
        FROM straylight.secrets AS secret
        LEFT JOIN LATERAL (
          SELECT log.recorded_at
          FROM straylight.secret_access_log AS log
          WHERE log.user_id=secret.user_id
            AND log.secret_id=secret.id
            AND log.operation='get'
          ORDER BY log.recorded_at DESC
          LIMIT 1
        ) AS last_used ON true
        WHERE secret.user_id=$1
        ORDER BY secret.name
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut *tx)
    .await?;
    tx.rollback().await?;
    let secrets = rows
        .iter()
        .map(|row| {
            Ok(SecretMetadata {
                secret_ref: format_ref(row.try_get("id")?),
                name: row.try_get("name")?,
                description: row.try_get("description")?,
                version: row.try_get("version")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                last_used_at: row.try_get("last_used_at")?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(ListResponse { secrets })
}

pub async fn delete_secret(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DeleteRequest>,
) -> ApiResult<Json<DeleteResponse>> {
    let started = Instant::now();
    let result = delete_inner(&state, &auth, request).await;
    record_operation("delete", started, &result);
    result.map(Json)
}

async fn delete_inner(
    state: &AppState,
    auth: &AuthContext,
    request: DeleteRequest,
) -> ApiResult<DeleteResponse> {
    require_write(auth)?;
    let name = normalize_name(&request.name)?;
    let mut tx = state.begin_write(auth).await?;
    let secret_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        DELETE FROM straylight.secrets
        WHERE user_id=$1 AND name=$2
        RETURNING id
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("secret", &name))?;
    record_access(&mut tx, auth, secret_id, "delete").await?;
    tx.commit().await?;
    Ok(DeleteResponse {
        secret_ref: format_ref(secret_id),
        name,
        status: "deleted",
    })
}

async fn record_access(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    secret_id: Uuid,
    operation: &'static str,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.secret_access_log (
          user_id,secret_id,credential_id,operation
        ) VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(secret_id)
    .bind(auth.credential_id.0)
    .bind(operation)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn require_read(auth: &AuthContext) -> ApiResult<()> {
    if auth.can(Capability::SecretRead) || auth.can(Capability::Admin) {
        Ok(())
    } else {
        Err(ApiError::capability(Capability::SecretRead.as_str()))
    }
}

fn require_write(auth: &AuthContext) -> ApiResult<()> {
    if auth.can(Capability::SecretWrite) || auth.can(Capability::Admin) {
        Ok(())
    } else {
        Err(ApiError::capability(Capability::SecretWrite.as_str()))
    }
}

fn require_list(auth: &AuthContext) -> ApiResult<()> {
    if auth.can(Capability::SecretRead)
        || auth.can(Capability::SecretWrite)
        || auth.can(Capability::Admin)
    {
        Ok(())
    } else {
        Err(ApiError::capability(Capability::SecretRead.as_str()))
    }
}

fn encryption_key(state: &AppState) -> ApiResult<[u8; 32]> {
    let encoded = state
        .config
        .secret_encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::configuration(
                "STRAYLIGHT_SECRET_ENCRYPTION_KEY is required for the secret vault",
            )
        })?;
    decode_secret_encryption_key(encoded)
}

fn normalize_name(value: &str) -> ApiResult<String> {
    let name = value.trim().to_ascii_lowercase();
    let valid_start = name
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    let valid_rest = name.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
    });
    if name.is_empty() || name.len() > 120 || !valid_start || !valid_rest {
        return Err(ApiError::invalid(
            "secret name must be 1-120 characters of lowercase letters, digits, '.', '_', or '-' \
             and start with a letter or digit",
        ));
    }
    Ok(name)
}

fn normalize_description(value: Option<String>) -> ApiResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(ApiError::invalid(format!(
            "secret description is limited to {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

pub fn secret_value_aad(
    environment: &str,
    user_id: Uuid,
    secret_id: Uuid,
    version: i32,
) -> Vec<u8> {
    format!("straylight.secret.v1|{environment}|{user_id}|{secret_id}|{version}").into_bytes()
}

pub fn encrypt_secret_value(
    key: &[u8; 32],
    aad: &[u8],
    value: &str,
) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| ApiError::Internal("could not initialize secret encryption".to_owned()))?;
    let nonce_bytes: [u8; 12] = rand::random();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: value.as_bytes(),
                aad,
            },
        )
        .map_err(|_| ApiError::Internal("could not encrypt secret value".to_owned()))?;
    Ok((ciphertext, nonce_bytes.to_vec()))
}

pub fn decrypt_secret_value(
    key: &[u8; 32],
    aad: &[u8],
    ciphertext: &[u8],
    nonce: &[u8],
) -> ApiResult<String> {
    if nonce.len() != 12 {
        return Err(ApiError::Internal(
            "stored secret nonce is invalid".to_owned(),
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| ApiError::Internal("could not initialize secret decryption".to_owned()))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| ApiError::Internal("stored secret could not be decrypted".to_owned()))?;
    String::from_utf8(plaintext)
        .map_err(|_| ApiError::Internal("stored secret is not UTF-8".to_owned()))
}

fn format_ref(id: Uuid) -> String {
    format!("secret:{}", id.simple())
}

fn record_operation<T>(operation: &'static str, started: Instant, result: &ApiResult<T>) {
    let outcome = match result {
        Ok(_) => "success",
        Err(ApiError::Public { code, .. }) if *code == "capability_denied" => "denied",
        Err(_) => "failure",
    };
    metrics::counter!(
        "secret.operations",
        "operation" => operation,
        "result" => outcome
    )
    .increment(1);
    metrics::histogram!("secret.operation.duration_ms", "operation" => operation)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_normalize_and_validate() {
        assert_eq!(normalize_name(" Datadog-Prod ").unwrap(), "datadog-prod");
        assert_eq!(normalize_name("a.b_c-1").unwrap(), "a.b_c-1");
        assert!(normalize_name("").is_err());
        assert!(normalize_name("-leading-dash").is_err());
        assert!(normalize_name("has space").is_err());
        assert!(normalize_name(&"x".repeat(121)).is_err());
    }

    #[test]
    fn descriptions_trim_and_bound() {
        assert_eq!(normalize_description(None).unwrap(), None);
        assert_eq!(normalize_description(Some("  ".to_owned())).unwrap(), None);
        assert_eq!(
            normalize_description(Some(" api key ".to_owned())).unwrap(),
            Some("api key".to_owned())
        );
        assert!(normalize_description(Some("x".repeat(1_001))).is_err());
    }

    #[test]
    fn encryption_round_trips_and_binds_aad() {
        let key = [7u8; 32];
        let aad = b"straylight.secret.v1|test";
        let value = "-----BEGIN KEY-----\nline\n-----END KEY-----\n";
        let (ciphertext, nonce) = encrypt_secret_value(&key, aad, value).unwrap();
        assert_eq!(
            decrypt_secret_value(&key, aad, &ciphertext, &nonce).unwrap(),
            value
        );
        assert!(decrypt_secret_value(&key, b"other-context", &ciphertext, &nonce).is_err());
        let other_key = [8u8; 32];
        assert!(decrypt_secret_value(&other_key, aad, &ciphertext, &nonce).is_err());
    }

    #[test]
    fn ciphertext_never_contains_plaintext() {
        let key = [9u8; 32];
        let value = "super-secret-canary-value";
        let (ciphertext, _nonce) = encrypt_secret_value(&key, b"aad", value).unwrap();
        let haystack = String::from_utf8_lossy(&ciphertext).into_owned();
        assert!(!haystack.contains("canary"));
    }
}

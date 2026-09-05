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
    todoist_sync::TodoistToken,
};

const MAX_SECRET_VALUE_BYTES: usize = 64 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 1_000;

/// The authenticated envelope identity that successfully opened a stored
/// secret. This deliberately has no `Debug` implementation because it carries
/// plaintext.
enum DecryptedSecretValue {
    Current(String),
    Legacy(String),
}

/// The token and non-bearer producer identity used by one worker pull. This
/// type deliberately has no Debug implementation so a caller cannot
/// accidentally format the secret-bearing value.
pub(crate) struct TodoistWorkerSecret {
    pub(crate) token: TodoistToken,
    pub(crate) producer_credential_id: Uuid,
}

/// Reads and audits exactly the `todoist-api-token` secret through the
/// migration-owned, admin-only primitive. No generic worker vault read exists.
pub(crate) async fn todoist_token_for_worker(
    state: &AppState,
    user_id: Uuid,
) -> ApiResult<Option<TodoistWorkerSecret>> {
    let pool = state
        .admin_pool
        .as_ref()
        .ok_or_else(|| ApiError::configuration("DATABASE_URL_ADMIN is required by Todoist sync"))?;
    let row = sqlx::query(
        r#"
        SELECT secret_id,value_ciphertext,value_nonce,version,
               producer_credential_id
        FROM brunn.task_todoist_secret_for_worker($1)
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let secret_id: Uuid = row.try_get("secret_id")?;
    let version: i32 = row.try_get("version")?;
    let ciphertext: Vec<u8> = row.try_get("value_ciphertext")?;
    let nonce: Vec<u8> = row.try_get("value_nonce")?;
    let producer_credential_id: Uuid = row.try_get("producer_credential_id")?;
    let aad = secret_value_aad(
        &state.config.deployment_environment,
        user_id,
        secret_id,
        version,
    );
    let plaintext = decrypt_secret_value(&encryption_key(state)?, &aad, &ciphertext, &nonce)?;
    let token = TodoistToken::from_secret(plaintext)?;
    Ok(Some(TodoistWorkerSecret {
        token,
        producer_credential_id,
    }))
}

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
        SELECT id,version FROM brunn.secrets
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
                UPDATE brunn.secrets
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
                INSERT INTO brunn.secrets (
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
    let response = get_in_tx(
        &mut tx,
        auth,
        name,
        &key,
        &state.config.deployment_environment,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

async fn get_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    name: String,
    key: &[u8; 32],
    environment: &str,
) -> ApiResult<GetResponse> {
    let may_rewrap = auth.can(Capability::SecretWrite) || auth.can(Capability::Admin);
    let mut query = sqlx::QueryBuilder::new(
        "SELECT id,description,value_ciphertext,value_nonce,version,updated_at \
         FROM brunn.secrets WHERE user_id=",
    );
    query
        .push_bind(auth.user_id.0)
        .push(" AND name=")
        .push_bind(&name);
    // Writers lock once before reading so a put and a legacy rewrap serialize.
    // Read-only credentials retain their ordinary RLS read path.
    if may_rewrap {
        query.push(" FOR UPDATE");
    }
    let row = query
        .build()
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::not_found("secret", &name))?;
    let secret_id = row.try_get("id")?;
    let mut version: i32 = row.try_get("version")?;
    let mut updated_at = row.try_get("updated_at")?;
    let decrypted = decrypt_stored_secret_value(
        key,
        environment,
        auth.user_id.0,
        secret_id,
        version,
        row.try_get("value_ciphertext")?,
        row.try_get("value_nonce")?,
    )?;
    let value = match decrypted {
        DecryptedSecretValue::Legacy(value) if may_rewrap => {
            version = version
                .checked_add(1)
                .ok_or_else(|| ApiError::Internal("secret version overflow".to_owned()))?;
            let aad = secret_value_aad(environment, auth.user_id.0, secret_id, version);
            let (ciphertext, nonce) = encrypt_secret_value(key, &aad, &value)?;
            updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
                r#"
                UPDATE brunn.secrets
                SET value_ciphertext=$3,value_nonce=$4,version=$5,
                    updated_by_credential_id=$6,updated_at=clock_timestamp()
                WHERE user_id=$1 AND id=$2
                RETURNING updated_at
                "#,
            )
            .bind(auth.user_id.0)
            .bind(secret_id)
            .bind(ciphertext)
            .bind(nonce)
            .bind(version)
            .bind(auth.credential_id.0)
            .fetch_one(&mut **tx)
            .await?;
            record_access(tx, auth, secret_id, "rewrap").await?;
            value
        }
        DecryptedSecretValue::Current(value) | DecryptedSecretValue::Legacy(value) => value,
    };
    record_access(tx, auth, secret_id, "get").await?;
    Ok(GetResponse {
        secret_ref: format_ref(secret_id),
        name,
        value,
        version,
        description: row.try_get("description")?,
        updated_at,
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
        FROM brunn.secrets AS secret
        LEFT JOIN LATERAL (
          SELECT log.recorded_at
          FROM brunn.secret_access_log AS log
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
        DELETE FROM brunn.secrets
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
        INSERT INTO brunn.secret_access_log (
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
            ApiError::configuration("BRUNN_SECRET_ENCRYPTION_KEY is required for the secret vault")
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
    format!("brunn.secret.v1|{environment}|{user_id}|{secret_id}|{version}").into_bytes()
}

fn legacy_secret_value_aad(
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

fn decrypt_stored_secret_value(
    key: &[u8; 32],
    environment: &str,
    user_id: Uuid,
    secret_id: Uuid,
    version: i32,
    ciphertext: &[u8],
    nonce: &[u8],
) -> ApiResult<DecryptedSecretValue> {
    let current_aad = secret_value_aad(environment, user_id, secret_id, version);
    decrypt_secret_value(key, &current_aad, ciphertext, nonce)
        .map(DecryptedSecretValue::Current)
        .or_else(|_| {
            // Only the exact pre-rename AAD is a compatibility alternative.
            let legacy_aad = legacy_secret_value_aad(environment, user_id, secret_id, version);
            decrypt_secret_value(key, &legacy_aad, ciphertext, nonce)
                .map(DecryptedSecretValue::Legacy)
        })
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
    use std::collections::HashSet;

    use sqlx::{PgPool, postgres::PgPoolOptions};

    use super::*;
    use crate::{
        db::set_context,
        models::{CredentialId, UserId},
    };

    const FIXTURE_ENVIRONMENT: &str = "production";
    const FIXTURE_VALUE: &str = "legacy-dreamer-secret-fixture-v1";
    const FIXTURE_NONCE_HEX: &str = "000102030405060708090a0b";
    const LEGACY_FIXTURE_CIPHERTEXT_HEX: &str = "20483329ff5a0e5aec566786b7cea8f04889e0eaed36df3950bbb1705254879ed09b33d86458365f976f7e9794ce47a5";
    const CURRENT_FIXTURE_CIPHERTEXT_HEX: &str = "20483329ff5a0e5aec566786b7cea8f04889e0eaed36df3950bbb1705254879e5609499c07c1c86f6935d29696e2e437";

    fn fixture_ids() -> (Uuid, Uuid) {
        (
            Uuid::parse_str("018f1d7e-b8df-7ad3-b02d-51dd5df08a11").unwrap(),
            Uuid::parse_str("018f1d7e-b8df-7ad3-b02d-51dd5df08a22").unwrap(),
        )
    }

    fn fixture_bytes(hex_value: &str) -> Vec<u8> {
        hex::decode(hex_value).unwrap()
    }

    struct DatabaseFixture {
        user_id: Uuid,
        writer_credential_id: Uuid,
        reader_credential_id: Uuid,
        writer: AuthContext,
        reader: AuthContext,
    }

    async fn connect_test_pool() -> Option<PgPool> {
        let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping secret rewrap database test");
            return None;
        };
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("connect to disposable PostgreSQL");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("apply Brunn migrations");
        Some(pool)
    }

    async fn insert_database_fixture(pool: &PgPool) -> DatabaseFixture {
        let user_id = Uuid::now_v7();
        let writer_credential_id = Uuid::now_v7();
        let reader_credential_id = Uuid::now_v7();
        let scope_id = Uuid::now_v7();
        let scope_ref = format!("scope:secret-rewrap-{scope_id}");
        sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
            .bind(user_id)
            .bind(format!("secret-rewrap-test:{user_id}"))
            .bind("Secret rewrap test")
            .execute(pool)
            .await
            .expect("insert secret rewrap test user");
        sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
            .bind(scope_id)
            .bind(user_id)
            .bind(&scope_ref)
            .bind("Secret rewrap test")
            .execute(pool)
            .await
            .expect("insert secret rewrap test scope");

        let writer_capabilities = vec!["read", "secret:read", "secret:write"];
        let reader_capabilities = vec!["read", "secret:read"];
        for (credential_id, label, capabilities) in [
            (
                writer_credential_id,
                "Secret rewrap writer",
                writer_capabilities.as_slice(),
            ),
            (
                reader_credential_id,
                "Secret rewrap reader",
                reader_capabilities.as_slice(),
            ),
        ] {
            sqlx::query(
                r#"
                INSERT INTO brunn.api_credentials(
                  id,user_id,label,token_hash,capabilities
                ) VALUES($1,$2,$3,$4,$5)
                "#,
            )
            .bind(credential_id)
            .bind(user_id)
            .bind(label)
            .bind(format!("secret-rewrap-test-token-{credential_id}"))
            .bind(capabilities)
            .execute(pool)
            .await
            .expect("insert secret rewrap test credential");
            sqlx::query(
                "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) \
                 VALUES($1,$2,$3)",
            )
            .bind(credential_id)
            .bind(user_id)
            .bind(scope_id)
            .execute(pool)
            .await
            .expect("grant secret rewrap test scope");
        }

        let auth = |credential_id, capabilities: &[&str]| AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref.clone()],
            read_only: true,
        };
        DatabaseFixture {
            user_id,
            writer_credential_id,
            reader_credential_id,
            writer: auth(writer_credential_id, &writer_capabilities),
            reader: auth(reader_credential_id, &reader_capabilities),
        }
    }

    async fn begin_as_app_rw<'a>(
        pool: &'a PgPool,
        auth: &AuthContext,
    ) -> Transaction<'a, Postgres> {
        let mut tx = pool.begin().await.expect("begin secret rewrap transaction");
        sqlx::query("SET LOCAL ROLE app_rw")
            .execute(&mut *tx)
            .await
            .expect("assume app_rw role");
        set_context(&mut tx, auth)
            .await
            .expect("set secret rewrap test context");
        tx
    }

    async fn database_get(
        pool: &PgPool,
        auth: &AuthContext,
        name: &str,
        key: &[u8; 32],
    ) -> GetResponse {
        let mut tx = begin_as_app_rw(pool, auth).await;
        let response = get_in_tx(&mut tx, auth, name.to_owned(), key, "production")
            .await
            .expect("read secret through compatibility path");
        tx.commit().await.expect("commit compatibility read");
        response
    }

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
        let aad = b"brunn.secret.v1|test";
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
    fn fixed_current_and_legacy_ciphertexts_are_distinguished() {
        let key = [42_u8; 32];
        let (user_id, secret_id) = fixture_ids();
        let nonce = fixture_bytes(FIXTURE_NONCE_HEX);

        let current = decrypt_stored_secret_value(
            &key,
            FIXTURE_ENVIRONMENT,
            user_id,
            secret_id,
            7,
            &fixture_bytes(CURRENT_FIXTURE_CIPHERTEXT_HEX),
            &nonce,
        )
        .unwrap();
        match current {
            DecryptedSecretValue::Current(value) => assert!(value == FIXTURE_VALUE),
            DecryptedSecretValue::Legacy(_) => panic!("current ciphertext used the legacy lane"),
        }

        let legacy = decrypt_stored_secret_value(
            &key,
            FIXTURE_ENVIRONMENT,
            user_id,
            secret_id,
            7,
            &fixture_bytes(LEGACY_FIXTURE_CIPHERTEXT_HEX),
            &nonce,
        )
        .unwrap();
        match legacy {
            DecryptedSecretValue::Legacy(value) => assert!(value == FIXTURE_VALUE),
            DecryptedSecretValue::Current(_) => panic!("legacy ciphertext used the current lane"),
        }
    }

    #[test]
    fn legacy_compatibility_rejects_wrong_key_context_and_corruption() {
        let key = [42_u8; 32];
        let wrong_key = [43_u8; 32];
        let (user_id, secret_id) = fixture_ids();
        let nonce = fixture_bytes(FIXTURE_NONCE_HEX);
        let ciphertext = fixture_bytes(LEGACY_FIXTURE_CIPHERTEXT_HEX);

        assert!(
            decrypt_stored_secret_value(
                &wrong_key,
                FIXTURE_ENVIRONMENT,
                user_id,
                secret_id,
                7,
                &ciphertext,
                &nonce,
            )
            .is_err()
        );
        assert!(
            decrypt_stored_secret_value(
                &key,
                "development",
                user_id,
                secret_id,
                7,
                &ciphertext,
                &nonce,
            )
            .is_err()
        );
        assert!(
            decrypt_stored_secret_value(
                &key,
                FIXTURE_ENVIRONMENT,
                user_id,
                secret_id,
                8,
                &ciphertext,
                &nonce,
            )
            .is_err()
        );
        let mut corrupted = ciphertext;
        corrupted[0] ^= 1;
        assert!(
            decrypt_stored_secret_value(
                &key,
                FIXTURE_ENVIRONMENT,
                user_id,
                secret_id,
                7,
                &corrupted,
                &nonce,
            )
            .is_err()
        );
        assert!(
            decrypt_stored_secret_value(
                &key,
                FIXTURE_ENVIRONMENT,
                user_id,
                secret_id,
                7,
                &fixture_bytes(LEGACY_FIXTURE_CIPHERTEXT_HEX),
                &[0_u8; 11],
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn database_rewrap_is_capability_safe_single_winner_and_audited() {
        let Some(pool) = connect_test_pool().await else {
            return;
        };
        let fixture = insert_database_fixture(&pool).await;
        let key = [51_u8; 32];
        let secret_id = Uuid::now_v7();
        let name = format!("legacy-dreamer-{}", secret_id.simple());
        let legacy_aad = legacy_secret_value_aad("production", fixture.user_id, secret_id, 1);
        let (legacy_ciphertext, legacy_nonce) =
            encrypt_secret_value(&key, &legacy_aad, FIXTURE_VALUE).unwrap();
        let original_updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            r#"
            INSERT INTO brunn.secrets(
              id,user_id,name,description,value_ciphertext,value_nonce,version,
              created_by_credential_id,updated_by_credential_id
            ) VALUES($1,$2,$3,'legacy fixture',$4,$5,1,$6,$6)
            RETURNING updated_at
            "#,
        )
        .bind(secret_id)
        .bind(fixture.user_id)
        .bind(&name)
        .bind(&legacy_ciphertext)
        .bind(&legacy_nonce)
        .bind(fixture.writer_credential_id)
        .fetch_one(&pool)
        .await
        .expect("insert legacy secret fixture");

        let reader_response = database_get(&pool, &fixture.reader, &name, &key).await;
        assert!(reader_response.value == FIXTURE_VALUE);
        assert_eq!(reader_response.version, 1);
        let after_reader = sqlx::query(
            r#"
            SELECT value_ciphertext,value_nonce,version,updated_at,
                   updated_by_credential_id
            FROM brunn.secrets WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(fixture.user_id)
        .bind(secret_id)
        .fetch_one(&pool)
        .await
        .expect("read unchanged legacy row");
        assert_eq!(
            after_reader
                .try_get::<Vec<u8>, _>("value_ciphertext")
                .unwrap(),
            legacy_ciphertext
        );
        assert_eq!(
            after_reader.try_get::<Vec<u8>, _>("value_nonce").unwrap(),
            legacy_nonce
        );
        assert_eq!(after_reader.try_get::<i32, _>("version").unwrap(), 1);
        assert_eq!(
            after_reader
                .try_get::<DateTime<Utc>, _>("updated_at")
                .unwrap(),
            original_updated_at
        );
        assert_eq!(
            after_reader
                .try_get::<Uuid, _>("updated_by_credential_id")
                .unwrap(),
            fixture.writer_credential_id
        );

        let (first_writer, second_writer) = tokio::join!(
            database_get(&pool, &fixture.writer, &name, &key),
            database_get(&pool, &fixture.writer, &name, &key),
        );
        let writer_responses = [first_writer, second_writer];

        let rewrapped = sqlx::query(
            r#"
            SELECT value_ciphertext,value_nonce,version,updated_at,
                   updated_by_credential_id
            FROM brunn.secrets WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(fixture.user_id)
        .bind(secret_id)
        .fetch_one(&pool)
        .await
        .expect("read rewrapped secret row");
        let rewrapped_ciphertext: Vec<u8> = rewrapped.try_get("value_ciphertext").unwrap();
        let rewrapped_nonce: Vec<u8> = rewrapped.try_get("value_nonce").unwrap();
        let rewrapped_updated_at: DateTime<Utc> = rewrapped.try_get("updated_at").unwrap();
        for response in &writer_responses {
            assert!(response.value == FIXTURE_VALUE);
            assert_eq!(response.version, 2);
            assert_eq!(response.description.as_deref(), Some("legacy fixture"));
            assert_eq!(response.secret_ref, format_ref(secret_id));
            assert_eq!(response.updated_at, rewrapped_updated_at);
        }
        assert_eq!(rewrapped.try_get::<i32, _>("version").unwrap(), 2);
        assert!(rewrapped_ciphertext != legacy_ciphertext);
        assert!(rewrapped_nonce != legacy_nonce);
        assert!(rewrapped.try_get::<DateTime<Utc>, _>("updated_at").unwrap() > original_updated_at);
        assert_eq!(
            rewrapped
                .try_get::<Uuid, _>("updated_by_credential_id")
                .unwrap(),
            fixture.writer_credential_id
        );
        let current_aad = secret_value_aad("production", fixture.user_id, secret_id, 2);
        assert!(
            decrypt_secret_value(&key, &current_aad, &rewrapped_ciphertext, &rewrapped_nonce)
                .is_ok_and(|opened| opened == FIXTURE_VALUE)
        );
        assert!(
            decrypt_secret_value(
                &key,
                &legacy_secret_value_aad("production", fixture.user_id, secret_id, 2),
                &rewrapped_ciphertext,
                &rewrapped_nonce,
            )
            .is_err()
        );

        let access_rows = sqlx::query(
            r#"
            SELECT operation,credential_id FROM brunn.secret_access_log
            WHERE user_id=$1 AND secret_id=$2
            ORDER BY recorded_at,id
            "#,
        )
        .bind(fixture.user_id)
        .bind(secret_id)
        .fetch_all(&pool)
        .await
        .expect("read content-free secret audit operations");
        let operations = access_rows
            .iter()
            .map(|row| row.try_get::<String, _>("operation").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation.as_str() == "rewrap")
                .count(),
            1
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation.as_str() == "get")
                .count(),
            3
        );
        assert_eq!(
            access_rows
                .iter()
                .filter(|row| {
                    row.try_get::<String, _>("operation").unwrap() == "get"
                        && row.try_get::<Uuid, _>("credential_id").unwrap()
                            == fixture.reader_credential_id
                })
                .count(),
            1
        );
        assert_eq!(
            access_rows
                .iter()
                .filter(|row| {
                    row.try_get::<Uuid, _>("credential_id").unwrap() == fixture.writer_credential_id
                })
                .count(),
            3
        );

        let immutable = sqlx::query(
            "UPDATE brunn.secret_access_log SET operation='get' \
             WHERE user_id=$1 AND secret_id=$2 AND operation='rewrap'",
        )
        .bind(fixture.user_id)
        .bind(secret_id)
        .execute(&pool)
        .await
        .expect_err("rewrap audit row must remain immutable");
        assert!(immutable.as_database_error().is_some());
        let arbitrary = sqlx::query(
            r#"
            INSERT INTO brunn.secret_access_log(
              user_id,secret_id,credential_id,operation
            ) VALUES($1,$2,$3,'legacy-fallback')
            "#,
        )
        .bind(fixture.user_id)
        .bind(secret_id)
        .bind(fixture.writer_credential_id)
        .execute(&pool)
        .await
        .expect_err("the audit operation set must remain bounded");
        assert_eq!(
            arbitrary
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("23514")
        );
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

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
};

const OWNER_CAPABILITIES: &[&str] = &[
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
    "message.read",
    "message.write",
    "admin",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionUserRequest {
    external_ref: String,
    display_name: String,
    #[serde(default = "default_owner_label")]
    credential_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverCredentialRequest {
    #[serde(default = "default_recovery_label")]
    credential_name: String,
}

pub async fn provision_user(
    state: &AppState,
    auth: &AuthContext,
    request: ProvisionUserRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Admin)?;
    validate_name(&request.external_ref, 200, "external_ref")?;
    validate_name(&request.display_name, 200, "display_name")?;
    validate_name(&request.credential_name, 120, "credential_name")?;

    let token = generate_token();
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    let empty_manifest_hash = hex::encode(Sha256::digest(b"straylight:empty-corpus@v1"));
    let mut tx = state.begin_write(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT *
        FROM straylight_auth.admin_provision_user($1,$2,$3,$4,$5)
        "#,
    )
    .bind(request.external_ref.trim())
    .bind(request.display_name.trim())
    .bind(request.credential_name.trim())
    .bind(token_hash)
    .bind(empty_manifest_hash)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_admin_database_error)?;
    tx.commit().await?;

    Ok(json!({
        "user": {
            "id": format!("user:{}", row.try_get::<Uuid,_>("user_id")?),
            "external_ref": request.external_ref.trim(),
            "display_name": request.display_name.trim(),
            "created_at": Utc::now()
        },
        "credential": {
            "id": format!("credential:{}", row.try_get::<Uuid,_>("credential_id")?),
            "name": request.credential_name.trim(),
            "access": "owner",
            "scope_ids": ["scope:root"],
            "capabilities": OWNER_CAPABILITIES,
            "token": token,
            "token_status": "issued_once"
        },
        "scope_id": "scope:root",
        "policy_id": format!("policy:{}", row.try_get::<Uuid,_>("policy_id")?)
    }))
}

pub async fn recover_credential(
    state: &AppState,
    auth: &AuthContext,
    user_ref: &str,
    request: RecoverCredentialRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Admin)?;
    validate_name(&request.credential_name, 120, "credential_name")?;
    let user_id = parse_user_ref(user_ref)?;
    let token = generate_token();
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    let mut tx = state.begin_write(auth).await?;
    let scope_refs = sqlx::query_scalar::<_, String>(
        "SELECT scope_ref FROM straylight.scopes WHERE user_id=$1 ORDER BY scope_ref",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    if scope_refs.is_empty() {
        return Err(ApiError::not_found("user_not_found", user_ref));
    }
    let credential_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT straylight_auth.admin_issue_credential($1,$2,$3,$4,$5)",
    )
    .bind(user_id)
    .bind(request.credential_name.trim())
    .bind(token_hash)
    .bind(OWNER_CAPABILITIES)
    .bind(&scope_refs)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_admin_database_error)?;
    tx.commit().await?;

    Ok(json!({
        "user_id": format!("user:{user_id}"),
        "credential": {
            "id": format!("credential:{credential_id}"),
            "name": request.credential_name.trim(),
            "access": "owner",
            "scope_ids": scope_refs,
            "capabilities": OWNER_CAPABILITIES,
            "token": token,
            "token_status": "issued_once",
            "created_at": Utc::now()
        }
    }))
}

fn validate_name(value: &str, max: usize, field: &str) -> ApiResult<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::invalid(format!(
            "{field} must contain 1 to {max} printable characters"
        )));
    }
    Ok(())
}

fn parse_user_ref(value: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.strip_prefix("user:").unwrap_or(value))
        .map_err(|_| ApiError::invalid("user reference must be user:<uuid> or a UUID"))
}

fn generate_token() -> String {
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    format!("sl_{}", URL_SAFE_NO_PAD.encode(secret))
}

fn map_admin_database_error(error: sqlx::Error) -> ApiError {
    if let Some(database) = error.as_database_error() {
        return match database.code().as_deref() {
            Some("23505") => ApiError::conflict(
                "user_exists",
                "a user with that external_ref already exists",
                json!({}),
            ),
            Some("P0002") => ApiError::not_found("user_not_found", "user"),
            _ => ApiError::Database(error),
        };
    }
    ApiError::Database(error)
}

fn default_owner_label() -> String {
    "Initial owner".to_owned()
}

fn default_recovery_label() -> String {
    format!("Recovered owner {}", Utc::now().format("%Y-%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_reject_controls_and_empty_values() {
        assert!(validate_name("alpha", 10, "name").is_ok());
        assert!(validate_name("", 10, "name").is_err());
        assert!(validate_name("bad\nname", 20, "name").is_err());
    }

    #[test]
    fn generated_tokens_are_distinct_and_not_plain_uuids() {
        let first = generate_token();
        let second = generate_token();
        assert!(first.starts_with("sl_"));
        assert_ne!(first, second);
        assert!(first.len() >= 40);
    }

    #[test]
    fn new_and_recovered_owner_credentials_include_messaging_authority() {
        assert!(OWNER_CAPABILITIES.contains(&"message.read"));
        assert!(OWNER_CAPABILITIES.contains(&"message.write"));
    }
}

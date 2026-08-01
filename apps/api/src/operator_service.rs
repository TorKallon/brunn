use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::RngCore;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::hash_token,
    db,
    error::{ApiError, ApiResult},
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
    "admin",
];

pub async fn provision_user(
    database_url: &str,
    external_ref: &str,
    display_name: &str,
    credential_name: &str,
) -> ApiResult<Value> {
    validate_name(external_ref, 200, "external_ref")?;
    validate_name(display_name, 200, "display_name")?;
    validate_name(credential_name, 120, "credential_name")?;

    let pool = db::operator_pool(database_url).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('straylight.operator.provision'))")
        .execute(&mut *tx)
        .await?;
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM straylight.users WHERE external_ref=$1)",
    )
    .bind(external_ref.trim())
    .fetch_one(&mut *tx)
    .await?;
    if exists {
        return Err(ApiError::conflict(
            "user_exists",
            "a user with that external_ref already exists",
            json!({}),
        ));
    }

    let token = generate_token();
    let capabilities: Vec<String> = OWNER_CAPABILITIES
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let (user_id, credential_id, scope_id, policy_id): (Uuid, Uuid, Uuid, Uuid) =
        sqlx::query_as("SELECT * FROM straylight_auth.bootstrap_user($1,$2,$3,$4,$5)")
            .bind(external_ref.trim())
            .bind(display_name.trim())
            .bind(credential_name.trim())
            .bind(hash_token(&token))
            .bind(&capabilities)
            .fetch_one(&mut *tx)
            .await?;
    let corpus_revision_id = db::ensure_initial_manifest(&mut tx, user_id, scope_id).await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          user_id, actor_ref, action, details, content_free
        ) VALUES (
          $1, 'operator:local', 'operator.user.provision', $2, true
        )
        "#,
    )
    .bind(user_id)
    .bind(json!({
        "credential_id": credential_id,
        "scope_id": scope_id,
        "policy_id": policy_id,
        "corpus_revision_id": corpus_revision_id
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(json!({
        "user": {
            "id": format!("user:{user_id}"),
            "external_ref": external_ref.trim(),
            "display_name": display_name.trim()
        },
        "credential": {
            "id": format!("credential:{credential_id}"),
            "name": credential_name.trim(),
            "access": "owner",
            "scope_ids": ["scope:root"],
            "capabilities": OWNER_CAPABILITIES,
            "token": token,
            "token_status": "issued_once"
        },
        "policy_id": format!("policy:{policy_id}"),
        "corpus_revision": format!("revision:{corpus_revision_id}")
    }))
}

pub async fn recover_user(
    database_url: &str,
    user_ref: &str,
    credential_name: &str,
    revoke_existing_owner_credentials: bool,
) -> ApiResult<Value> {
    validate_name(credential_name, 120, "credential_name")?;
    let user_id = parse_user_ref(user_ref)?;
    let pool = db::operator_pool(database_url).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('straylight.operator.recover'))")
        .execute(&mut *tx)
        .await?;
    let user = sqlx::query(
        "SELECT display_name, account_status FROM straylight.users WHERE id=$1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("user_not_found", user_ref))?;
    let account_status: String = user.try_get("account_status")?;
    if account_status != "active" {
        return Err(ApiError::conflict(
            "account_not_active",
            "owner recovery requires an active account",
            json!({"account_status": account_status}),
        ));
    }

    let scope_ids = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, scope_ref::text FROM straylight.scopes WHERE user_id=$1 ORDER BY scope_ref",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    if scope_ids.is_empty() {
        return Err(ApiError::Internal(
            "the recovery user has no authorized scopes".to_owned(),
        ));
    }

    let token = generate_token();
    let capabilities: Vec<String> = OWNER_CAPABILITIES
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let credential_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO straylight.api_credentials (
          user_id, label, token_hash, capabilities
        ) VALUES ($1,$2,$3,$4)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(credential_name.trim())
    .bind(hash_token(&token))
    .bind(&capabilities)
    .fetch_one(&mut *tx)
    .await?;
    for (scope_id, _) in &scope_ids {
        sqlx::query(
            r#"
            INSERT INTO straylight.credential_scope_grants (
              credential_id, user_id, scope_id
            ) VALUES ($1,$2,$3)
            "#,
        )
        .bind(credential_id)
        .bind(user_id)
        .bind(scope_id)
        .execute(&mut *tx)
        .await?;
    }
    let revoked_existing_owner_credentials = if revoke_existing_owner_credentials {
        sqlx::query(
            r#"
            UPDATE straylight.api_credentials AS credential
            SET disabled_at = coalesce(credential.disabled_at, clock_timestamp())
            WHERE credential.user_id=$1
              AND credential.id<>$2
              AND credential.disabled_at IS NULL
              AND credential.capabilities @> $3
              AND credential.capabilities <@ $3
            "#,
        )
        .bind(user_id)
        .bind(credential_id)
        .bind(&capabilities)
        .execute(&mut *tx)
        .await?
        .rows_affected()
    } else {
        0
    };
    let scope_refs: Vec<&str> = scope_ids
        .iter()
        .map(|(_, scope_ref)| scope_ref.as_str())
        .collect();
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          user_id, actor_ref, action, details, content_free
        ) VALUES (
          $1, 'operator:local', 'operator.credential.recover', $2, true
        )
        "#,
    )
    .bind(user_id)
    .bind(json!({
        "credential_id": credential_id,
        "scope_refs": scope_refs,
        "recovery_mode": if revoke_existing_owner_credentials {
            "compromised"
        } else {
            "lost"
        },
        "revoked_existing_owner_credentials": revoked_existing_owner_credentials
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(json!({
        "user": {
            "id": format!("user:{user_id}"),
            "display_name": user.try_get::<String, _>("display_name")?
        },
        "credential": {
            "id": format!("credential:{credential_id}"),
            "name": credential_name.trim(),
            "access": "owner",
            "scope_ids": scope_refs,
            "capabilities": OWNER_CAPABILITIES,
            "token": token,
            "token_status": "issued_once"
        },
        "recovery_mode": if revoke_existing_owner_credentials {
            "compromised"
        } else {
            "lost"
        },
        "revoked_existing_owner_credentials": revoked_existing_owner_credentials
    }))
}

pub async fn record_backup_watermark(
    database_url: &str,
    oldest_retained_created_at: &str,
    receipt_sha256: &str,
    source: &str,
) -> ApiResult<Value> {
    validate_name(source, 240, "source")?;
    let watermark = DateTime::parse_from_rfc3339(oldest_retained_created_at.trim())
        .map_err(|_| ApiError::invalid("oldest_retained_created_at must be an RFC 3339 timestamp"))?
        .with_timezone(&Utc);
    if watermark > Utc::now() + ChronoDuration::minutes(5) {
        return Err(ApiError::invalid(
            "oldest_retained_created_at cannot be in the future",
        ));
    }
    let receipt_sha256 = receipt_sha256
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or(receipt_sha256.trim())
        .to_ascii_lowercase();
    if receipt_sha256.len() != 64
        || !receipt_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ApiError::invalid(
            "receipt_sha256 must be a lowercase SHA-256 digest",
        ));
    }

    let pool = db::operator_pool(database_url).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('straylight.operator.backup_watermark'))")
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(
        r#"
        UPDATE straylight.account_deletion_requests
        SET backup_erasure_verified_at=clock_timestamp(),
            backup_erasure_watermark_at=$1,
            backup_erasure_receipt_sha256=$2,
            backup_erasure_source=$3,
            terminal_result=coalesce(terminal_result,'{}'::jsonb)
              || jsonb_build_object(
                'backup_status','prune_verified',
                'backup_erasure_watermark_at',$1,
                'backup_erasure_receipt_sha256','sha256:' || $2,
                'backup_erasure_source',$3
              )
        WHERE status='awaiting_backup_expiry'
          AND backup_expiry_due_at <= clock_timestamp()
          AND terminal_result ? 'canonical_purged_at'
          AND (terminal_result->>'canonical_purged_at')::timestamptz < $1
          AND (
            backup_erasure_verified_at IS NULL
            OR backup_erasure_watermark_at < $1
          )
        RETURNING user_id,id
        "#,
    )
    .bind(watermark)
    .bind(&receipt_sha256)
    .bind(source.trim())
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        let user_id: Uuid = row.try_get("user_id")?;
        let request_id: Uuid = row.try_get("id")?;
        sqlx::query(
            r#"
            INSERT INTO straylight.audit_events (
              user_id,actor_ref,action,details,content_free
            ) VALUES (
              $1,'operator:local','operator.backup_erasure.verify',$2,true
            )
            "#,
        )
        .bind(user_id)
        .bind(json!({
            "request_id": request_id,
            "oldest_retained_created_at": watermark,
            "receipt_sha256": format!("sha256:{receipt_sha256}"),
            "source": source.trim()
        }))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(json!({
        "status": "recorded",
        "oldest_retained_created_at": watermark,
        "receipt_sha256": format!("sha256:{receipt_sha256}"),
        "source": source.trim(),
        "account_deletions_verified": rows.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_tokens_are_random_and_prefixed() {
        let first = generate_token();
        let second = generate_token();
        assert!(first.starts_with("sl_"));
        assert_ne!(first, second);
        assert!(first.len() >= 40);
    }

    #[test]
    fn operator_names_and_user_refs_are_bounded() {
        assert!(validate_name("Alpha", 20, "name").is_ok());
        assert!(validate_name("", 20, "name").is_err());
        assert!(validate_name("bad\nname", 20, "name").is_err());
        assert!(parse_user_ref(&Uuid::nil().to_string()).is_ok());
        assert!(parse_user_ref("user:not-a-uuid").is_err());
    }
}

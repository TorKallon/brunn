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
    "notification:publish",
    "notification:manage",
    "secret:read",
    "secret:write",
    "task.read",
    "task.write",
    "location.write",
    "integration.manage",
    "message.read",
    "message.write",
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('brunn.operator.provision'))")
        .execute(&mut *tx)
        .await?;
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM brunn.users WHERE external_ref=$1)",
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
        sqlx::query_as("SELECT * FROM brunn_auth.bootstrap_user($1,$2,$3,$4,$5)")
            .bind(external_ref.trim())
            .bind(display_name.trim())
            .bind(credential_name.trim())
            .bind(hash_token(&token))
            .bind(&capabilities)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        r#"
        INSERT INTO brunn.audit_events (
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
        "policy_id": policy_id
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
        "policy_id": format!("policy:{policy_id}")
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('brunn.operator.recover'))")
        .execute(&mut *tx)
        .await?;
    let user =
        sqlx::query("SELECT display_name, account_status FROM brunn.users WHERE id=$1 FOR UPDATE")
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
        "SELECT id, scope_ref::text FROM brunn.scopes WHERE user_id=$1 ORDER BY scope_ref",
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
        INSERT INTO brunn.api_credentials (
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
            INSERT INTO brunn.credential_scope_grants (
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
            UPDATE brunn.api_credentials AS credential
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
        INSERT INTO brunn.audit_events (
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

pub async fn configure_web_identity(
    database_url: &str,
    user_ref: &str,
    username: &str,
    email: &str,
) -> ApiResult<Value> {
    let user_id = parse_user_ref(user_ref)?;
    let username = normalize_web_username(username)?;
    let email = normalize_web_email(email)?;
    let pool = db::operator_pool(database_url).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('brunn.operator.web_identity:' || $1::text,0))",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    let user =
        sqlx::query("SELECT display_name, account_status FROM brunn.users WHERE id=$1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::not_found("user_not_found", user_ref))?;
    let account_status: String = user.try_get("account_status")?;
    if account_status != "active" {
        return Err(ApiError::conflict(
            "account_not_active",
            "web identity configuration requires an active account",
            json!({"account_status": account_status}),
        ));
    }
    let identity_conflict = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1
          FROM brunn.web_identities
          WHERE user_id<>$1
            AND (username_normalized=$2 OR email_normalized=$3)
        )
        "#,
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .fetch_one(&mut *tx)
    .await?;
    if identity_conflict {
        return Err(ApiError::conflict(
            "web_identity_conflict",
            "the username or email is already assigned",
            json!({}),
        ));
    }

    let existing_credential_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT web_credential_id FROM brunn.web_identities WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let capabilities: Vec<String> = OWNER_CAPABILITIES
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let (credential_id, credential_created) = match existing_credential_id {
        Some(credential_id) => {
            let updated = sqlx::query(
                r#"
                UPDATE brunn.api_credentials
                SET label='Web UI session principal',
                    capabilities=$1,
                    disabled_at=NULL
                WHERE user_id=$2 AND id=$3
                "#,
            )
            .bind(&capabilities)
            .bind(user_id)
            .bind(credential_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(ApiError::Internal(
                    "web identity principal is missing".to_owned(),
                ));
            }
            (credential_id, false)
        }
        None => {
            // The random bearer material is immediately discarded. Only its
            // one-way digest exists so this principal can be used by web
            // sessions without creating another usable bearer credential.
            let discarded_bearer = generate_token();
            let credential_id = sqlx::query_scalar::<_, Uuid>(
                r#"
                INSERT INTO brunn.api_credentials (
                  user_id, label, token_hash, capabilities
                ) VALUES ($1,'Web UI session principal',$2,$3)
                RETURNING id
                "#,
            )
            .bind(user_id)
            .bind(hash_token(&discarded_bearer))
            .bind(&capabilities)
            .fetch_one(&mut *tx)
            .await?;
            drop(discarded_bearer);
            (credential_id, true)
        }
    };
    let scope_grants = sqlx::query(
        r#"
        INSERT INTO brunn.credential_scope_grants (
          credential_id, user_id, scope_id
        )
        SELECT $1, scope.user_id, scope.id
        FROM brunn.scopes AS scope
        WHERE scope.user_id=$2
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let password_configured = sqlx::query_scalar::<_, bool>(
        r#"
        INSERT INTO brunn.web_identities (
          user_id, username, username_normalized,
          email, email_normalized, password_hash, web_credential_id
        ) VALUES ($1,$2,$2,$3,$3,NULL,$4)
        ON CONFLICT (user_id) DO UPDATE
        SET username=EXCLUDED.username,
            username_normalized=EXCLUDED.username_normalized,
            email=EXCLUDED.email,
            email_normalized=EXCLUDED.email_normalized,
            web_credential_id=EXCLUDED.web_credential_id,
            updated_at=clock_timestamp()
        RETURNING password_hash IS NOT NULL
        "#,
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .bind(credential_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO brunn.audit_events (
          user_id, credential_id, actor_ref, action, details, content_free
        ) VALUES (
          $1, $2, 'operator:local', 'operator.web_identity.configure',
          jsonb_build_object(
            'web_credential_id',$2,
            'credential_created',$3,
            'scope_grants_added',$4,
            'password_configured',$5
          ), true
        )
        "#,
    )
    .bind(user_id)
    .bind(credential_id)
    .bind(credential_created)
    .bind(i64::try_from(scope_grants).unwrap_or(i64::MAX))
    .bind(password_configured)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(json!({
        "user": {
            "id": format!("user:{user_id}"),
            "display_name": user.try_get::<String, _>("display_name")?,
            "username": username,
            "email": email
        },
        "web_credential": {
            "id": format!("credential:{credential_id}"),
            "created": credential_created,
            "bearer_token_status": "discarded"
        },
        "password_status": if password_configured { "configured" } else { "reset_required" },
        "scope_grants_added": scope_grants
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('brunn.operator.backup_watermark'))")
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(
        r#"
        UPDATE brunn.account_deletion_requests
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
            INSERT INTO brunn.audit_events (
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

fn normalize_web_username(value: &str) -> ApiResult<String> {
    let value = value.trim().to_ascii_lowercase();
    let valid = (3..=64).contains(&value.len())
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'0'..=b'9' => true,
            b'.' | b'_' | b'-' => index > 0,
            _ => false,
        });
    if !valid {
        return Err(ApiError::invalid(
            "username must contain 3 to 64 lowercase letters, digits, dots, underscores, or hyphens and start with a letter or digit",
        ));
    }
    Ok(value)
}

fn normalize_web_email(value: &str) -> ApiResult<String> {
    let value = value.trim().to_ascii_lowercase();
    let (local, domain) = value.split_once('@').unwrap_or_default();
    if value.len() > 254
        || local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::invalid("email must be a valid address"));
    }
    Ok(value)
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
        assert_eq!(
            normalize_web_username(" Owner.Name ").unwrap(),
            "owner.name"
        );
        assert!(normalize_web_username("-owner").is_err());
        assert_eq!(
            normalize_web_email(" Owner@Example.com ").unwrap(),
            "owner@example.com"
        );
        assert!(normalize_web_email("owner@example").is_err());
    }
}

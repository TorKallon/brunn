use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
};

pub async fn me(state: &AppState, auth: &AuthContext) -> ApiResult<Value> {
    let mut tx = state.begin_read(auth).await?;
    let user = sqlx::query(
        "SELECT external_ref,display_name,created_at FROM straylight.users WHERE id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let scopes = scope_items(&mut tx, auth).await?;
    let active_scope = scopes.first().cloned();
    let corpus_revision = if let Some(scope_ref) = active_scope
        .as_ref()
        .and_then(|scope| scope.get("scope_ref"))
        .and_then(Value::as_str)
    {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT manifest.active_corpus_revision_id
            FROM straylight.active_manifests AS manifest
            JOIN straylight.scopes AS scope
              ON scope.user_id=manifest.user_id AND scope.id=manifest.scope_id
            WHERE manifest.user_id=$1 AND scope.scope_ref=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(scope_ref)
        .fetch_optional(&mut *tx)
        .await?
        .map(|id| format!("revision:{id}"))
    } else {
        None
    };
    tx.commit().await?;
    let mut capabilities: Vec<_> = auth.capabilities.iter().cloned().collect();
    capabilities.sort();
    Ok(json!({
        "user": {
            "id": format!("user:{}", auth.user_id.0),
            "external_ref": user.try_get::<String,_>("external_ref")?,
            "display_name": user.try_get::<String,_>("display_name")?,
            "created_at": user.try_get::<DateTime<Utc>,_>("created_at")?
        },
        "credential_id": format!("credential:{}", auth.credential_id.0),
        "active_scope": active_scope,
        "scopes": scopes,
        "corpus_revision": corpus_revision,
        "capabilities": capabilities,
        "read_only": auth.read_only
    }))
}

pub async fn list_scopes(state: &AppState, auth: &AuthContext) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let mut tx = state.begin_read(auth).await?;
    let items = scope_items(&mut tx, auth).await?;
    tx.commit().await?;
    let total = items.len();
    Ok(json!({"items": items, "continuation_token": null, "total": total}))
}

pub async fn list_policies(state: &AppState, auth: &AuthContext) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT policy.id,policy.policy_ref,policy.name,policy.current_version,
               policy.is_default,revision.default_effect,revision.rules,revision.recorded_at
        FROM straylight.policies AS policy
        JOIN straylight.policy_revisions AS revision
          ON revision.user_id=policy.user_id AND revision.policy_id=policy.id
         AND revision.version=policy.current_version
        WHERE policy.user_id=$1 ORDER BY policy.is_default DESC,policy.name
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(|row| -> ApiResult<Value> {
            Ok(json!({
                "id": row.try_get::<String,_>("policy_ref")?,
                "record_id": format!("policy:{}", row.try_get::<Uuid,_>("id")?),
                "name": row.try_get::<String,_>("name")?,
                "version": row.try_get::<i32,_>("current_version")?,
                "status": if row.try_get::<bool,_>("is_default")? { "default" } else { "active" },
                "default_effect": row.try_get::<String,_>("default_effect")?,
                "rules": row.try_get::<Value,_>("rules")?,
                "updated_at": row.try_get::<DateTime<Utc>,_>("recorded_at")?
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let total = items.len();
    Ok(json!({"items": items, "continuation_token": null, "total": total}))
}

pub async fn list_credentials(state: &AppState, auth: &AuthContext) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query("SELECT * FROM straylight_auth.list_credentials($1)")
        .bind(auth.user_id.0)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(credential_row_value)
        .collect::<ApiResult<Vec<_>>>()?;
    let total = items.len();
    Ok(json!({"items": items, "continuation_token": null, "total": total}))
}

pub async fn create_credential(
    state: &AppState,
    auth: &AuthContext,
    request: &Value,
) -> ApiResult<Value> {
    auth.require(Capability::CredentialManage)?;
    let requested_name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if requested_name.is_empty() || requested_name.len() > 120 {
        return Err(ApiError::invalid(
            "credential name must contain 1 to 120 characters",
        ));
    }
    let (access, capabilities) = credential_template_for_gate(
        request.get("access").and_then(Value::as_str),
        state.config.messaging_enabled,
    )?;
    if access == "ios_tasks" {
        auth.require(Capability::Admin)?;
    }
    let name = if access == "ios_tasks" {
        let device_name = requested_name
            .strip_prefix("iOS Tasks — ")
            .unwrap_or(requested_name);
        let label = format!("iOS Tasks — {device_name}");
        if label.len() > 120 {
            return Err(ApiError::invalid(
                "credential name including the iOS Tasks prefix must be at most 120 characters",
            ));
        }
        label
    } else {
        requested_name.to_owned()
    };
    let requested_scopes = request
        .get("scope_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut tx = state.begin_write(auth).await?;
    let scope_refs = if requested_scopes.is_empty() {
        auth.scope_refs.clone()
    } else {
        let mut values = Vec::with_capacity(requested_scopes.len());
        for value in requested_scopes {
            let raw = value
                .as_str()
                .ok_or_else(|| ApiError::invalid("scope_ids must contain strings"))?;
            if raw.starts_with("scope:") {
                values.push(raw.to_owned());
            } else {
                let id = Uuid::parse_str(raw)
                    .map_err(|_| ApiError::invalid("scope_ids must contain scope refs or UUIDs"))?;
                values.push(
                    sqlx::query_scalar::<_, String>(
                        "SELECT scope_ref FROM straylight.scopes WHERE user_id=$1 AND id=$2",
                    )
                    .bind(auth.user_id.0)
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| ApiError::not_found("scope_not_found", raw))?,
                );
            }
        }
        values
    };
    let mut secret = [0_u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let token = format!("sl_{}", URL_SAFE_NO_PAD.encode(secret));
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    let credential_id =
        sqlx::query_scalar::<_, Uuid>("SELECT straylight_auth.issue_credential($1,$2,$3,$4,$5)")
            .bind(auth.user_id.0)
            .bind(&name)
            .bind(token_hash)
            .bind(&capabilities)
            .bind(&scope_refs)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_credential_issue_error)?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("credential:{credential_id}"),
        "name": name,
        "access": access,
        "scope_ids": scope_refs,
        "capabilities": capabilities,
        "token": token,
        "created_at": Utc::now(),
        "revoked_at": null
    }))
}

fn credential_template(access: Option<&str>) -> ApiResult<(&'static str, Vec<&'static str>)> {
    match access.unwrap_or("read_write") {
        "read_only" => Ok((
            "read_only",
            vec![
                "open",
                "query",
                "read",
                "compute",
                "verify",
                "status",
                "task.read",
                "message.read",
            ],
        )),
        "read_write" => Ok((
            "read_write",
            vec![
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
                "task.read",
                "task.write",
                "message.read",
                "message.write",
            ],
        )),
        "ios_tasks" => Ok(("ios_tasks", vec!["task.write", "notification:manage"])),
        // The dreamer wrapper: vault custody of the codex tokens plus the
        // run's single operational notification. Codex never holds this.
        "dreamer_runner" => Ok((
            "dreamer_runner",
            vec!["secret:read", "secret:write", "notification:publish"],
        )),
        "owner" => Ok((
            "owner",
            vec![
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
            ],
        )),
        _ => Err(ApiError::invalid(
            "credential access must be read_only, read_write, ios_tasks, dreamer_runner, or owner",
        )),
    }
}

fn credential_template_for_gate(
    access: Option<&str>,
    messaging_enabled: bool,
) -> ApiResult<(&'static str, Vec<&'static str>)> {
    let (access, mut capabilities) = credential_template(access)?;
    if access == "ios_tasks" && messaging_enabled {
        capabilities.push("message.write");
    }
    if !messaging_enabled {
        capabilities.retain(|capability| !capability.starts_with("message."));
    }
    Ok((access, capabilities))
}

fn map_credential_issue_error(error: sqlx::Error) -> ApiError {
    if error
        .as_database_error()
        .and_then(|database| database.code())
        .as_deref()
        == Some("42501")
    {
        return ApiError::public(
            http::StatusCode::FORBIDDEN,
            "credential_delegation_denied",
            "a credential cannot delegate capabilities or scopes it does not hold",
        );
    }
    ApiError::Database(error)
}

pub async fn revoke_credential(
    state: &AppState,
    auth: &AuthContext,
    credential_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::CredentialManage)?;
    let credential_id = parse_uuid_ref(credential_ref, "credential")?;
    if credential_id == auth.credential_id.0 {
        return Err(ApiError::conflict(
            "active_credential",
            "the currently authenticated credential cannot revoke itself",
            json!({"credential_id": credential_ref}),
        ));
    }
    let mut tx = state.begin_write(auth).await?;
    let revoked_at =
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT straylight_auth.revoke_credential($1,$2)")
            .bind(auth.user_id.0)
            .bind(credential_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("credential:{credential_id}"),
        "revoked_at": revoked_at,
        "status": "revoked"
    }))
}

async fn scope_items(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT scope.id,scope.scope_ref,scope.name,scope.created_at,
               count(DISTINCT key.record_id) AS object_count
        FROM straylight.scopes AS scope
        LEFT JOIN straylight.record_keys AS key
          ON key.user_id=scope.user_id AND key.scope_id=scope.id
        WHERE scope.user_id=$1
        GROUP BY scope.id,scope.scope_ref,scope.name,scope.created_at
        ORDER BY scope.name,scope.scope_ref
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| -> ApiResult<Value> {
            let scope_ref: String = row.try_get("scope_ref")?;
            Ok(json!({
                "id": scope_ref,
                "record_id": row.try_get::<Uuid,_>("id")?.to_string(),
                "scope_ref": scope_ref,
                "name": row.try_get::<String,_>("name")?,
                "access": if auth.read_only { "read_only" } else { "read_write" },
                "object_count": row.try_get::<i64,_>("object_count")?,
                "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?
            }))
        })
        .collect()
}

fn credential_row_value(row: sqlx::postgres::PgRow) -> ApiResult<Value> {
    let capabilities: Vec<String> = row.try_get("capabilities")?;
    let disabled_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;
    Ok(json!({
        "id": format!("credential:{}", row.try_get::<Uuid,_>("id")?),
        "name": row.try_get::<String,_>("label")?,
        "access": credential_access_label(&capabilities),
        "scope_ids": row.try_get::<Vec<String>,_>("scope_refs")?,
        "capabilities": capabilities,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "revoked_at": disabled_at,
        "status": if disabled_at.is_some() { "revoked" } else { "active" }
    }))
}

fn credential_access_label(capabilities: &[String]) -> &'static str {
    if capabilities.len() == 3
        && capabilities.iter().any(|value| value == "secret:read")
        && capabilities.iter().any(|value| value == "secret:write")
        && capabilities
            .iter()
            .any(|value| value == "notification:publish")
    {
        "dreamer_runner"
    } else if matches!(capabilities.len(), 2 | 3)
        && capabilities.iter().any(|value| value == "task.write")
        && capabilities
            .iter()
            .any(|value| value == "notification:manage")
        && (capabilities.len() == 2 || capabilities.iter().any(|value| value == "message.write"))
    {
        "ios_tasks"
    } else if capabilities
        .iter()
        .any(|value| value == "credential:manage")
    {
        "owner"
    } else if capabilities
        .iter()
        .any(|value| value == "save" || value == "checkpoint")
    {
        "read_write"
    } else {
        "read_only"
    }
}

#[cfg(test)]
mod credential_tests {
    use super::{credential_access_label, credential_template, credential_template_for_gate};

    #[test]
    fn omitted_credential_access_defaults_to_read_write() {
        let (access, capabilities) = credential_template(None).expect("default template");
        assert_eq!(access, "read_write");
        assert!(capabilities.contains(&"save"));
        assert!(capabilities.contains(&"message.read"));
        assert!(capabilities.contains(&"message.write"));
        assert!(!capabilities.contains(&"credential:manage"));
    }

    #[test]
    fn read_only_credentials_can_receive_but_not_send_messages() {
        let (access, capabilities) =
            credential_template(Some("read_only")).expect("read-only template");
        assert_eq!(access, "read_only");
        assert!(capabilities.contains(&"message.read"));
        assert!(!capabilities.contains(&"message.write"));
    }

    #[test]
    fn owner_template_has_every_capability() {
        let (access, capabilities) = credential_template(Some("owner")).expect("owner template");
        assert_eq!(access, "owner");
        assert_eq!(capabilities.len(), 23);
        assert!(capabilities.contains(&"dream"));
        assert!(capabilities.contains(&"credential:manage"));
        assert!(capabilities.contains(&"notification:publish"));
        assert!(capabilities.contains(&"notification:manage"));
        assert!(capabilities.contains(&"secret:read"));
        assert!(capabilities.contains(&"secret:write"));
        assert!(capabilities.contains(&"task.read"));
        assert!(capabilities.contains(&"task.write"));
        assert!(capabilities.contains(&"message.read"));
        assert!(capabilities.contains(&"message.write"));
        assert!(capabilities.contains(&"integration.manage"));
        assert!(capabilities.contains(&"admin"));
    }

    #[test]
    fn ios_tasks_template_is_exactly_narrow_and_inventory_preserves_its_access() {
        let (access, capabilities) =
            credential_template(Some("ios_tasks")).expect("iOS task template");
        assert_eq!(access, "ios_tasks");
        assert_eq!(capabilities, ["task.write", "notification:manage"]);
        for forbidden in [
            "task.read",
            "save",
            "checkpoint",
            "secret:read",
            "secret:write",
            "admin",
            "integration.manage",
        ] {
            assert!(!capabilities.contains(&forbidden), "unexpected {forbidden}");
        }
        let stored = capabilities
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(credential_access_label(&stored), "ios_tasks");
    }

    #[test]
    fn ios_tasks_adds_only_message_write_when_messaging_is_enabled() {
        let (off_access, off) =
            credential_template_for_gate(Some("ios_tasks"), false).expect("gate-off template");
        assert_eq!(off_access, "ios_tasks");
        assert_eq!(off, ["task.write", "notification:manage"]);

        let (on_access, on) =
            credential_template_for_gate(Some("ios_tasks"), true).expect("gate-on template");
        assert_eq!(on_access, "ios_tasks");
        assert_eq!(on, ["task.write", "notification:manage", "message.write"]);
        assert!(!on.contains(&"message.read"));
        assert_eq!(
            credential_access_label(&on.into_iter().map(str::to_owned).collect::<Vec<_>>()),
            "ios_tasks"
        );
    }

    #[test]
    fn templates_omit_message_capabilities_when_messaging_is_disabled() {
        for access in ["read_only", "read_write", "owner"] {
            let (_, off) =
                credential_template_for_gate(Some(access), false).expect("gate-off template");
            assert!(
                off.iter()
                    .all(|capability| !capability.starts_with("message.")),
                "{access} leaked message capabilities with messaging disabled"
            );
            let (_, on) =
                credential_template_for_gate(Some(access), true).expect("gate-on template");
            assert!(
                on.iter()
                    .any(|capability| capability.starts_with("message.")),
                "{access} lost message capabilities with messaging enabled"
            );
        }
    }

    #[test]
    fn credential_manager_is_labeled_owner() {
        let capabilities = vec![
            "open".to_owned(),
            "save".to_owned(),
            "credential:manage".to_owned(),
        ];
        assert_eq!(credential_access_label(&capabilities), "owner");
    }

    #[test]
    fn dreamer_runner_template_is_vault_and_notify_only() {
        let (access, capabilities) =
            credential_template(Some("dreamer_runner")).expect("dreamer_runner template");
        assert_eq!(access, "dreamer_runner");
        assert_eq!(
            capabilities,
            ["secret:read", "secret:write", "notification:publish"]
        );
        for forbidden in [
            "open",
            "read",
            "save",
            "checkpoint",
            "delete",
            "admin",
            "credential:manage",
            "task.write",
        ] {
            assert!(!capabilities.contains(&forbidden), "unexpected {forbidden}");
        }
        let stored = capabilities
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(credential_access_label(&stored), "dreamer_runner");
    }
}

fn parse_uuid_ref(value: &str, expected_prefix: &str) -> ApiResult<Uuid> {
    let raw = value
        .strip_prefix(&format!("{expected_prefix}:"))
        .unwrap_or(value);
    Uuid::parse_str(raw)
        .map_err(|_| ApiError::invalid(format!("invalid {expected_prefix} reference")))
}

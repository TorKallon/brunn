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
    models::{Capability, ListQuery},
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

pub async fn list_sessions(
    state: &AppState,
    auth: &AuthContext,
    query: &ListQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Open)?;
    let limit = list_limit(query);
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT s.id,s.task_hash,s.mode,s.created_at,s.expires_at,s.resumed_checkpoint_id,
               sc.scope_ref,sc.name AS scope_name,cr.id AS revision_id,
               cr.revision_number,
               latest.id AS latest_checkpoint_id,latest.title AS checkpoint_title
        FROM straylight.sessions AS s
        JOIN straylight.scopes AS sc ON sc.user_id=s.user_id AND sc.id=s.scope_id
        JOIN straylight.corpus_revisions AS cr
          ON cr.user_id=s.user_id AND cr.id=s.corpus_revision_id
        LEFT JOIN LATERAL (
          SELECT cp.id,cp.title FROM straylight.checkpoints AS cp
          WHERE cp.user_id=s.user_id AND cp.session_id=s.id
          ORDER BY cp.created_at DESC LIMIT 1
        ) AS latest ON true
        WHERE s.user_id=$1
          AND ($2::text IS NULL OR sc.scope_ref=$2)
        ORDER BY s.created_at DESC,s.id DESC
        LIMIT $3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.scope.as_deref())
    .bind(limit as i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let items: Vec<_> = rows
        .into_iter()
        .map(|row| -> ApiResult<Value> {
            let id: Uuid = row.try_get("id")?;
            let checkpoint: Option<Uuid> = row.try_get("latest_checkpoint_id")?;
            let resumed: Option<Uuid> = row.try_get("resumed_checkpoint_id")?;
            Ok(json!({
                "id": format!("session:{id}"),
                "session_id": format!("session:{id}"),
                "title": row.try_get::<Option<String>,_>("checkpoint_title")?,
                "task_hash": format!("sha256:{}", row.try_get::<String,_>("task_hash")?),
                "status": "open",
                "mode": row.try_get::<String,_>("mode")?,
                "scope_id": row.try_get::<String,_>("scope_ref")?,
                "scope_name": row.try_get::<String,_>("scope_name")?,
                "corpus_revision": format!("revision:{}", row.try_get::<Uuid,_>("revision_id")?),
                "revision_sequence": row.try_get::<i64,_>("revision_number")?,
                "checkpoint_id": checkpoint.map(|value| format!("checkpoint:{value}")),
                "resumed_checkpoint_id": resumed.map(|value| format!("checkpoint:{value}")),
                "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
                "expires_at": row.try_get::<DateTime<Utc>,_>("expires_at")?
            }))
        })
        .collect::<ApiResult<_>>()?;
    Ok(list_envelope(items, limit))
}

pub async fn get_session(
    state: &AppState,
    auth: &AuthContext,
    session_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Open)?;
    let session_id = parse_uuid_ref(session_ref, "session")?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT s.id,s.task_hash,s.task_size_bytes,s.mode,s.capability_snapshot,
               s.authorization_scope_refs,s.parent_session_id,s.resumed_checkpoint_id,
               s.created_at,s.expires_at,sc.scope_ref,sc.name AS scope_name,
               cr.id AS revision_id,cr.revision_number,cr.manifest_hash
        FROM straylight.sessions AS s
        JOIN straylight.scopes AS sc ON sc.user_id=s.user_id AND sc.id=s.scope_id
        JOIN straylight.corpus_revisions AS cr
          ON cr.user_id=s.user_id AND cr.id=s.corpus_revision_id
        WHERE s.user_id=$1 AND s.id=$2 AND s.credential_id=$3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(auth.credential_id.0)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", session_ref))?;
    let checkpoint_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.checkpoints WHERE user_id=$1 AND session_id=$2 ORDER BY created_at",
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut checkpoints = Vec::with_capacity(checkpoint_ids.len());
    for checkpoint_id in checkpoint_ids {
        checkpoints.push(checkpoint_value(&mut tx, auth.user_id.0, checkpoint_id).await?);
    }
    let operations = operation_items(&mut tx, auth.user_id.0, session_id).await?;
    tx.commit().await?;
    let latest = checkpoints.last().cloned();
    let parent_session: Option<Uuid> = row.try_get("parent_session_id")?;
    let resumed: Option<Uuid> = row.try_get("resumed_checkpoint_id")?;
    Ok(json!({
        "id": format!("session:{session_id}"),
        "session_id": format!("session:{session_id}"),
        "task_hash": format!("sha256:{}", row.try_get::<String,_>("task_hash")?),
        "task_size_bytes": row.try_get::<i32,_>("task_size_bytes")?,
        "mode": row.try_get::<String,_>("mode")?,
        "capabilities": row.try_get::<Vec<String>,_>("capability_snapshot")?,
        "authorization_scope_refs": row.try_get::<Vec<String>,_>("authorization_scope_refs")?,
        "scope_id": row.try_get::<String,_>("scope_ref")?,
        "scope_name": row.try_get::<String,_>("scope_name")?,
        "corpus_revision": format!("revision:{}", row.try_get::<Uuid,_>("revision_id")?),
        "revision_sequence": row.try_get::<i64,_>("revision_number")?,
        "manifest_hash": format!("sha256:{}", row.try_get::<String,_>("manifest_hash")?),
        "parent_session_id": parent_session.map(|value| format!("session:{value}")),
        "resumed_checkpoint_id": resumed.map(|value| format!("checkpoint:{value}")),
        "checkpoint_id": latest.as_ref().and_then(|value| value.get("checkpoint_id")).cloned(),
        "checkpoint": latest,
        "checkpoints": checkpoints,
        "operations": operations,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "expires_at": row.try_get::<DateTime<Utc>,_>("expires_at")?
    }))
}

pub async fn get_checkpoint(
    state: &AppState,
    auth: &AuthContext,
    checkpoint_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let checkpoint_id = parse_uuid_ref(checkpoint_ref, "checkpoint")?;
    let mut tx = state.begin_read(auth).await?;
    let value = checkpoint_value(&mut tx, auth.user_id.0, checkpoint_id).await?;
    tx.commit().await?;
    Ok(value)
}

pub async fn list_objects(
    state: &AppState,
    auth: &AuthContext,
    query: &ListQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let limit = list_limit(query);
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT object.id,object.current_version,revision.label,revision.properties,
               revision.recorded_at,scope.scope_ref,
               coalesce(array_agg(profile.profile_ref::text ORDER BY profile.profile_ref)
                 FILTER (WHERE profile.profile_ref IS NOT NULL),'{}'::text[]) AS profiles
        FROM straylight.objects AS object
        JOIN straylight.object_revisions AS revision
          ON revision.user_id=object.user_id AND revision.object_id=object.id
         AND revision.version=object.current_version
        JOIN straylight.scopes AS scope
          ON scope.user_id=object.user_id AND scope.id=object.scope_id
        LEFT JOIN straylight.object_revision_profiles AS profile
          ON profile.user_id=revision.user_id AND profile.object_id=revision.object_id
         AND profile.object_version=revision.version
        WHERE object.user_id=$1
          AND ($2::text IS NULL OR scope.scope_ref=$2)
          AND ($3::text IS NULL OR revision.label ILIKE '%'||$3||'%'
               OR revision.properties::text ILIKE '%'||$3||'%')
          AND ($4::text IS NULL OR EXISTS (
            SELECT 1 FROM straylight.object_revision_profiles AS selected_profile
            WHERE selected_profile.user_id=object.user_id
              AND selected_profile.object_id=object.id
              AND selected_profile.object_version=object.current_version
              AND selected_profile.profile_ref=$4
          ))
        GROUP BY object.id,object.current_version,revision.label,revision.properties,
                 revision.recorded_at,scope.scope_ref
        ORDER BY revision.recorded_at DESC,object.id DESC
        LIMIT $5
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.scope.as_deref())
    .bind(query.query.as_deref())
    .bind(query.type_profile.as_deref())
    .bind(limit as i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let items: Vec<_> = rows.into_iter().map(|row| -> ApiResult<Value> {
        let id: Uuid = row.try_get("id")?;
        Ok(json!({
            "id": format!("object:{id}"),
            "display_name": row.try_get::<Option<String>,_>("label")?.unwrap_or_else(|| "Untitled object".to_owned()),
            "version": row.try_get::<i32,_>("current_version")?,
            "profiles": row.try_get::<Vec<String>,_>("profiles")?,
            "properties": row.try_get::<Value,_>("properties")?,
            "scope_id": row.try_get::<String,_>("scope_ref")?,
            "updated_at": row.try_get::<DateTime<Utc>,_>("recorded_at")?
        }))
    }).collect::<ApiResult<_>>()?;
    Ok(list_envelope(items, limit))
}

pub async fn get_object(
    state: &AppState,
    auth: &AuthContext,
    object_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let object_id = parse_uuid_ref(object_ref, "object")?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT object.current_version,revision.label,revision.properties,
               revision.recorded_at,scope.scope_ref,policy.policy_ref,
               policy.name AS policy_name,policy.current_version AS policy_version
        FROM straylight.objects AS object
        JOIN straylight.object_revisions AS revision
          ON revision.user_id=object.user_id AND revision.object_id=object.id
         AND revision.version=object.current_version
        JOIN straylight.scopes AS scope
          ON scope.user_id=object.user_id AND scope.id=object.scope_id
        JOIN straylight.policies AS policy
          ON policy.user_id=object.user_id AND policy.id=object.policy_id
        WHERE object.user_id=$1 AND object.id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(object_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("object_not_found", object_ref))?;
    let current_version: i32 = row.try_get("current_version")?;
    let profiles = sqlx::query_scalar::<_, String>(
        "SELECT profile_ref::text FROM straylight.object_revision_profiles WHERE user_id=$1 AND object_id=$2 AND object_version=$3 ORDER BY profile_ref",
    )
    .bind(auth.user_id.0)
    .bind(object_id)
    .bind(current_version)
    .fetch_all(&mut *tx)
    .await?;
    let claims = object_claims(&mut tx, auth.user_id.0, object_id).await?;
    let states = object_states(&mut tx, auth.user_id.0, object_id).await?;
    let versions = object_versions(&mut tx, auth.user_id.0, object_id).await?;
    tx.commit().await?;
    Ok(json!({
        "id": format!("object:{object_id}"),
        "display_name": row.try_get::<Option<String>,_>("label")?.unwrap_or_else(|| "Untitled object".to_owned()),
        "profiles": profiles,
        "version": current_version,
        "scope_id": row.try_get::<String,_>("scope_ref")?,
        "updated_at": row.try_get::<DateTime<Utc>,_>("recorded_at")?,
        "properties": row.try_get::<Value,_>("properties")?,
        "claims": claims,
        "relations": [],
        "states": states,
        "versions": versions,
        "policy": {
            "id": row.try_get::<String,_>("policy_ref")?,
            "name": row.try_get::<String,_>("policy_name")?,
            "version": row.try_get::<i32,_>("policy_version")?
        }
    }))
}

pub async fn list_sources(
    state: &AppState,
    auth: &AuthContext,
    query: &ListQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let limit = list_limit(query);
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT source.id,source.source_ref,source.source_kind,source.source_version,
               source.title,source.content_hash,source.captured_at,source.created_at,
               source.metadata,scope.scope_ref,
               document.id AS document_id,document.current_version,
               revision.media_type,revision.byte_size
        FROM straylight.source_episodes AS source
        JOIN straylight.scopes AS scope
          ON scope.user_id=source.user_id AND scope.id=source.scope_id
        LEFT JOIN straylight.document_revisions AS revision
          ON revision.user_id=source.user_id AND revision.source_episode_id=source.id
        LEFT JOIN straylight.documents AS document
          ON document.user_id=revision.user_id AND document.id=revision.document_id
         AND document.current_version=revision.version
        WHERE source.user_id=$1
          AND ($2::text IS NULL OR scope.scope_ref=$2)
          AND ($3::text IS NULL OR source.title ILIKE '%'||$3||'%'
               OR source.source_ref ILIKE '%'||$3||'%')
        ORDER BY source.captured_at DESC,source.id DESC
        LIMIT $4
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.scope.as_deref())
    .bind(query.query.as_deref())
    .bind(limit as i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let items: Vec<_> = rows
        .into_iter()
        .map(source_row_value)
        .collect::<ApiResult<_>>()?;
    Ok(list_envelope(items, limit))
}

pub async fn get_source(
    state: &AppState,
    auth: &AuthContext,
    source_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let (kind, record_id) = split_uuid_ref(source_ref)?;
    let mut tx = state.begin_read(auth).await?;
    let row = if kind == "document" {
        sqlx::query(
            r#"
            SELECT source.id,source.source_ref,source.source_kind,source.source_version,
                   source.title,source.content_hash,source.captured_at,source.created_at,
                   source.metadata,scope.scope_ref,document.id AS document_id,
                   document.current_version,revision.media_type,revision.byte_size
            FROM straylight.documents AS document
            JOIN straylight.document_revisions AS revision
              ON revision.user_id=document.user_id AND revision.document_id=document.id
             AND revision.version=document.current_version
            JOIN straylight.source_episodes AS source
              ON source.user_id=revision.user_id AND source.id=revision.source_episode_id
            JOIN straylight.scopes AS scope
              ON scope.user_id=source.user_id AND scope.id=source.scope_id
            WHERE document.user_id=$1 AND document.id=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(record_id)
        .fetch_optional(&mut *tx)
        .await?
    } else if kind == "source" || kind == "source_episode" {
        sqlx::query(
            r#"
            SELECT source.id,source.source_ref,source.source_kind,source.source_version,
                   source.title,source.content_hash,source.captured_at,source.created_at,
                   source.metadata,scope.scope_ref,document.id AS document_id,
                   document.current_version,revision.media_type,revision.byte_size
            FROM straylight.source_episodes AS source
            JOIN straylight.scopes AS scope
              ON scope.user_id=source.user_id AND scope.id=source.scope_id
            LEFT JOIN straylight.document_revisions AS revision
              ON revision.user_id=source.user_id AND revision.source_episode_id=source.id
            LEFT JOIN straylight.documents AS document
              ON document.user_id=revision.user_id AND document.id=revision.document_id
             AND document.current_version=revision.version
            WHERE source.user_id=$1 AND source.id=$2
            ORDER BY document.created_at LIMIT 1
            "#,
        )
        .bind(auth.user_id.0)
        .bind(record_id)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        return Err(ApiError::invalid(
            "source ref must use source: or document:",
        ));
    }
    .ok_or_else(|| ApiError::not_found("source_not_found", source_ref))?;
    let mut value = source_row_value(row)?;
    if let Some(document_id) = value.get("document_id").and_then(Value::as_str) {
        let document_id = parse_uuid_ref(document_id, "document")?;
        let outline = sqlx::query(
            "SELECT heading_path,locator,ordinal FROM straylight.chunks WHERE user_id=$1 AND document_id=$2 ORDER BY ordinal",
        )
        .bind(auth.user_id.0)
        .bind(document_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|chunk| -> ApiResult<Value> {
            let headings: Vec<String> = chunk.try_get("heading_path")?;
            Ok(json!({
                "title": headings.last().cloned().unwrap_or_else(|| "Document".to_owned()),
                "level": headings.len(),
                "locator": chunk.try_get::<Value,_>("locator")?,
                "ordinal": chunk.try_get::<i32,_>("ordinal")?
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
        value
            .as_object_mut()
            .expect("source value is an object")
            .insert("outline".to_owned(), Value::Array(outline));
    }
    tx.commit().await?;
    Ok(value)
}

pub async fn source_content(
    state: &AppState,
    auth: &AuthContext,
    source_ref: &str,
) -> ApiResult<(String, String)> {
    auth.require(Capability::Read)?;
    let (kind, record_id) = split_uuid_ref(source_ref)?;
    let mut tx = state.begin_read(auth).await?;
    let row = if kind == "document" {
        sqlx::query(
            r#"
            SELECT revision.document_id,revision.version,revision.media_type
            FROM straylight.documents AS document
            JOIN straylight.document_revisions AS revision
              ON revision.user_id=document.user_id AND revision.document_id=document.id
             AND revision.version=document.current_version
            WHERE document.user_id=$1 AND document.id=$2
            "#,
        )
        .bind(auth.user_id.0)
        .bind(record_id)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT revision.document_id,revision.version,revision.media_type
            FROM straylight.document_revisions AS revision
            JOIN straylight.documents AS document
              ON document.user_id=revision.user_id AND document.id=revision.document_id
             AND document.current_version=revision.version
            WHERE revision.user_id=$1 AND revision.source_episode_id=$2
            ORDER BY revision.recorded_at DESC LIMIT 1
            "#,
        )
        .bind(auth.user_id.0)
        .bind(record_id)
        .fetch_optional(&mut *tx)
        .await?
    }
    .ok_or_else(|| ApiError::not_found("source_content_not_found", source_ref))?;
    let document_id: Uuid = row.try_get("document_id")?;
    let version: i32 = row.try_get("version")?;
    let media_type: String = row.try_get("media_type")?;
    let content = sqlx::query_scalar::<_, String>(
        "SELECT content FROM straylight.chunks WHERE user_id=$1 AND document_id=$2 AND document_version=$3 ORDER BY ordinal",
    )
    .bind(auth.user_id.0)
    .bind(document_id)
    .bind(version)
    .fetch_all(&mut *tx)
    .await?
    .join("\n\n");
    tx.commit().await?;
    Ok((media_type, content))
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

pub async fn list_audit(
    state: &AppState,
    auth: &AuthContext,
    query: &ListQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let limit = list_limit(query);
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        SELECT event.id,event.action,event.actor_ref,event.target_record_id,
               event.request_id,event.details,event.content_free,event.recorded_at,
               event.credential_id,scope.scope_ref
        FROM straylight.audit_events AS event
        LEFT JOIN straylight.scopes AS scope
          ON scope.user_id=event.user_id AND scope.id=event.scope_id
        WHERE event.user_id=$1
          AND ($2::text IS NULL OR scope.scope_ref=$2)
        ORDER BY event.recorded_at DESC,event.id DESC LIMIT $3
        "#,
    )
    .bind(auth.user_id.0)
    .bind(query.scope.as_deref())
    .bind(limit as i64)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let items = rows
        .into_iter()
        .map(|row| -> ApiResult<Value> {
            let target: Option<Uuid> = row.try_get("target_record_id")?;
            let credential: Option<Uuid> = row.try_get("credential_id")?;
            Ok(json!({
                "id": format!("audit:{}", row.try_get::<Uuid,_>("id")?),
                "action": row.try_get::<String,_>("action")?,
                "actor": row.try_get::<Option<String>,_>("actor_ref")?
                    .or_else(|| credential.map(|id| format!("credential:{id}"))),
                "target": target.map(|id| id.to_string()),
                "scope_id": row.try_get::<Option<String>,_>("scope_ref")?,
                "status": "recorded",
                "request_id": row.try_get::<Option<String>,_>("request_id")?,
                "details": row.try_get::<Value,_>("details")?,
                "content_free": row.try_get::<bool,_>("content_free")?,
                "created_at": row.try_get::<DateTime<Utc>,_>("recorded_at")?
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(list_envelope(items, limit))
}

pub async fn get_deletion(
    state: &AppState,
    auth: &AuthContext,
    deletion_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Status)?;
    let deletion_id = parse_uuid_ref(deletion_ref, "deletion")?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT job.id,job.status,job.started_at,job.completed_at,
               job.terminal_result,job.created_at,job.tombstone_id,
               tombstone.target_record_id,record_key.record_kind,scope.scope_ref
        FROM straylight.deletion_jobs AS job
        JOIN straylight.tombstones AS tombstone
          ON tombstone.user_id=job.user_id AND tombstone.id=job.tombstone_id
        JOIN straylight.record_keys AS record_key
          ON record_key.user_id=tombstone.user_id
         AND record_key.record_id=tombstone.target_record_id
        JOIN straylight.scopes AS scope
          ON scope.user_id=job.user_id AND scope.id=job.scope_id
        WHERE job.user_id=$1 AND job.id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(deletion_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("deletion_not_found", deletion_ref))?;
    let target_rows = sqlx::query(
        r#"
        SELECT target_kind,target_ref,status,attempt_count,details,updated_at
        FROM straylight.derivative_cleanup_targets
        WHERE user_id=$1 AND deletion_job_id=$2
        ORDER BY target_kind,target_ref
        "#,
    )
    .bind(auth.user_id.0)
    .bind(deletion_id)
    .fetch_all(&mut *tx)
    .await?;
    let event_rows = sqlx::query(
        r#"
        SELECT id,event_kind,details,recorded_at
        FROM straylight.deletion_job_events
        WHERE user_id=$1 AND deletion_job_id=$2
        ORDER BY recorded_at,id
        "#,
    )
    .bind(auth.user_id.0)
    .bind(deletion_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let target_kind: String = row.try_get("record_kind")?;
    let target_id: Uuid = row.try_get("target_record_id")?;
    let targets = target_rows
        .into_iter()
        .map(|target| -> ApiResult<Value> {
            Ok(json!({
                "kind": target.try_get::<String,_>("target_kind")?,
                "ref": target.try_get::<String,_>("target_ref")?,
                "status": target.try_get::<String,_>("status")?,
                "attempt_count": target.try_get::<i32,_>("attempt_count")?,
                "details": target.try_get::<Value,_>("details")?,
                "updated_at": target.try_get::<DateTime<Utc>,_>("updated_at")?
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let events = event_rows
        .into_iter()
        .map(|event| -> ApiResult<Value> {
            Ok(json!({
                "id": format!("deletion-event:{}", event.try_get::<Uuid,_>("id")?),
                "kind": event.try_get::<String,_>("event_kind")?,
                "details": event.try_get::<Value,_>("details")?,
                "recorded_at": event.try_get::<DateTime<Utc>,_>("recorded_at")?
            }))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(json!({
        "id": format!("deletion:{deletion_id}"),
        "status": row.try_get::<String,_>("status")?,
        "scope_id": row.try_get::<String,_>("scope_ref")?,
        "tombstone_id": format!("tombstone:{}", row.try_get::<Uuid,_>("tombstone_id")?),
        "target_ref": format!("{}:{target_id}", record_prefix(&target_kind)),
        "targets": targets,
        "events": events,
        "result": row.try_get::<Option<Value>,_>("terminal_result")?,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "started_at": row.try_get::<Option<DateTime<Utc>>,_>("started_at")?,
        "completed_at": row.try_get::<Option<DateTime<Utc>>,_>("completed_at")?
    }))
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
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() || name.len() > 120 {
        return Err(ApiError::invalid(
            "credential name must contain 1 to 120 characters",
        ));
    }
    let access = request
        .get("access")
        .and_then(Value::as_str)
        .unwrap_or("read_only");
    let capabilities: Vec<&str> = match access {
        "read_only" => vec!["open", "query", "read", "compute", "verify", "status"],
        "read_write" => vec![
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
        ],
        _ => {
            return Err(ApiError::invalid(
                "credential access must be read_only or read_write",
            ));
        }
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
            .bind(name)
            .bind(token_hash)
            .bind(&capabilities)
            .bind(&scope_refs)
            .fetch_one(&mut *tx)
            .await?;
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

async fn checkpoint_value(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    checkpoint_id: Uuid,
) -> ApiResult<Value> {
    let row = sqlx::query(
        r#"
        SELECT checkpoint.id,checkpoint.title,checkpoint.summary,
               checkpoint.parent_checkpoint_id,checkpoint.session_id,
               checkpoint.created_at,revision.id AS revision_id,
               revision.revision_number,scope.scope_ref
        FROM straylight.checkpoints AS checkpoint
        JOIN straylight.corpus_revisions AS revision
          ON revision.user_id=checkpoint.user_id AND revision.id=checkpoint.corpus_revision_id
        JOIN straylight.scopes AS scope
          ON scope.user_id=checkpoint.user_id AND scope.id=checkpoint.scope_id
        WHERE checkpoint.user_id=$1 AND checkpoint.id=$2
        "#,
    )
    .bind(user_id)
    .bind(checkpoint_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            "checkpoint_not_found",
            &format!("checkpoint:{checkpoint_id}"),
        )
    })?;
    let goals = sqlx::query(
        "SELECT position,goal_text,work_item_object_id,state_claim_id FROM straylight.checkpoint_goals WHERE user_id=$1 AND checkpoint_id=$2 ORDER BY position",
    )
    .bind(user_id).bind(checkpoint_id).fetch_all(&mut **tx).await?
    .into_iter().map(|item| -> ApiResult<Value> {
        let object: Option<Uuid> = item.try_get("work_item_object_id")?;
        let state: Option<Uuid> = item.try_get("state_claim_id")?;
        Ok(json!({
            "id": format!("goal:{}", item.try_get::<i32,_>("position")?),
            "title": item.try_get::<String,_>("goal_text")?,
            "work_item_object_id": object.map(|id| format!("object:{id}")),
            "state_claim_id": state.map(|id| format!("claim:{id}"))
        }))
    }).collect::<ApiResult<Vec<_>>>()?;
    let gaps = sqlx::query(
        "SELECT position,gap_kind,description,related_record_id FROM straylight.checkpoint_gaps WHERE user_id=$1 AND checkpoint_id=$2 ORDER BY position",
    )
    .bind(user_id).bind(checkpoint_id).fetch_all(&mut **tx).await?
    .into_iter().map(|item| -> ApiResult<Value> {
        Ok(json!({
            "kind": item.try_get::<String,_>("gap_kind")?,
            "description": item.try_get::<String,_>("description")?,
            "related_record_id": item.try_get::<Option<Uuid>,_>("related_record_id")?.map(|id| id.to_string())
        }))
    }).collect::<ApiResult<Vec<_>>>()?;
    let decisions = sqlx::query(
        r#"
        SELECT decision.position,decision.decision_kind,claim.id,claim.value,claim.predicate
        FROM straylight.checkpoint_decisions AS decision
        JOIN straylight.claims AS claim ON claim.user_id=decision.user_id AND claim.id=decision.claim_id
        WHERE decision.user_id=$1 AND decision.checkpoint_id=$2 ORDER BY decision.position
        "#,
    )
    .bind(user_id).bind(checkpoint_id).fetch_all(&mut **tx).await?
    .into_iter().map(|item| -> ApiResult<Value> {
        Ok(json!({
            "kind": item.try_get::<String,_>("decision_kind")?,
            "claim_id": format!("claim:{}", item.try_get::<Uuid,_>("id")?),
            "predicate": item.try_get::<String,_>("predicate")?,
            "value": item.try_get::<Value,_>("value")?
        }))
    }).collect::<ApiResult<Vec<_>>>()?;
    let gates = sqlx::query(
        "SELECT position,description,gate_object_id,state_claim_id FROM straylight.checkpoint_gates WHERE user_id=$1 AND checkpoint_id=$2 ORDER BY position",
    )
    .bind(user_id).bind(checkpoint_id).fetch_all(&mut **tx).await?
    .into_iter().map(|item| -> ApiResult<Value> {
        Ok(json!({
            "id": format!("gate:{}", item.try_get::<i32,_>("position")?),
            "title": item.try_get::<String,_>("description")?,
            "gate_object_id": item.try_get::<Option<Uuid>,_>("gate_object_id")?.map(|id| format!("object:{id}")),
            "state_claim_id": item.try_get::<Option<Uuid>,_>("state_claim_id")?.map(|id| format!("claim:{id}"))
        }))
    }).collect::<ApiResult<Vec<_>>>()?;
    let actions = sqlx::query(
        "SELECT position,action_text,responsible_record_id,due_temporal_id,preconditions FROM straylight.checkpoint_actions WHERE user_id=$1 AND checkpoint_id=$2 ORDER BY position",
    )
    .bind(user_id).bind(checkpoint_id).fetch_all(&mut **tx).await?
    .into_iter().map(|item| -> ApiResult<Value> {
        Ok(json!({
            "id": format!("action:{}", item.try_get::<i32,_>("position")?),
            "title": item.try_get::<String,_>("action_text")?,
            "responsible_record_id": item.try_get::<Option<Uuid>,_>("responsible_record_id")?.map(|id| id.to_string()),
            "due_temporal_id": item.try_get::<Option<Uuid>,_>("due_temporal_id")?.map(|id| format!("temporal:{id}")),
            "preconditions": item.try_get::<Value,_>("preconditions")?
        }))
    }).collect::<ApiResult<Vec<_>>>()?;
    let sources = sqlx::query(
        r#"
        SELECT source.position,source.source_record_id,source.source_version,
               source.locator,key.record_kind
        FROM straylight.checkpoint_source_refs AS source
        JOIN straylight.record_keys AS key
          ON key.user_id=source.user_id AND key.record_id=source.source_record_id
        WHERE source.user_id=$1 AND source.checkpoint_id=$2 ORDER BY source.position
        "#,
    )
    .bind(user_id)
    .bind(checkpoint_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|item| -> ApiResult<Value> {
        let kind: String = item.try_get("record_kind")?;
        let prefix = record_prefix(&kind);
        let id: Uuid = item.try_get("source_record_id")?;
        Ok(json!({
            "ref": format!("{prefix}:{id}"),
            "version": item.try_get::<Option<i32>,_>("source_version")?,
            "locator": item.try_get::<Value,_>("locator")?
        }))
    })
    .collect::<ApiResult<Vec<_>>>()?;
    let source_refs: Vec<_> = sources
        .iter()
        .filter_map(|value| value.get("ref").cloned())
        .collect();
    let parent: Option<Uuid> = row.try_get("parent_checkpoint_id")?;
    Ok(json!({
        "id": format!("checkpoint:{checkpoint_id}"),
        "checkpoint_id": format!("checkpoint:{checkpoint_id}"),
        "title": row.try_get::<Option<String>,_>("title")?,
        "objective": row.try_get::<String,_>("summary")?,
        "summary": row.try_get::<String,_>("summary")?,
        "parent_checkpoint_id": parent.map(|id| format!("checkpoint:{id}")),
        "session_id": format!("session:{}", row.try_get::<Uuid,_>("session_id")?),
        "scope_id": row.try_get::<String,_>("scope_ref")?,
        "corpus_revision": format!("revision:{}", row.try_get::<Uuid,_>("revision_id")?),
        "revision_sequence": row.try_get::<i64,_>("revision_number")?,
        "goals": goals,
        "gaps": gaps,
        "decisions": decisions,
        "gates": gates,
        "next_actions": actions,
        "sources": sources,
        "source_refs": source_refs,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?
    }))
}

async fn operation_items(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT id,operation_kind AS operation,status,started_at AS created_at,
               finished_at,metrics
        FROM straylight.retrieval_operations
        WHERE user_id=$1 AND session_id=$2
        ORDER BY started_at
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| -> ApiResult<Value> {
            let started: DateTime<Utc> = row.try_get("created_at")?;
            let finished: Option<DateTime<Utc>> = row.try_get("finished_at")?;
            Ok(json!({
                "id": format!("operation:{}", row.try_get::<Uuid,_>("id")?),
                "operation": row.try_get::<String,_>("operation")?,
                "status": row.try_get::<String,_>("status")?,
                "created_at": started,
                "duration_ms": finished.map(|end| (end-started).num_milliseconds()),
                "metrics": row.try_get::<Value,_>("metrics")?
            }))
        })
        .collect()
}

async fn object_claims(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    object_id: Uuid,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT claim.id,claim.predicate,claim.value,claim.support_state,
               claim.authority,claim.canonicality,claim.confidence,claim.recorded_at
        FROM straylight.claims AS claim
        JOIN straylight.claim_about_targets AS target
          ON target.user_id=claim.user_id AND target.claim_id=claim.id
        WHERE claim.user_id=$1 AND target.target_record_id=$2
        ORDER BY claim.recorded_at DESC
        "#,
    )
    .bind(user_id)
    .bind(object_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| -> ApiResult<Value> {
            Ok(json!({
                "id": format!("claim:{}", row.try_get::<Uuid,_>("id")?),
                "predicate": row.try_get::<String,_>("predicate")?,
                "value": row.try_get::<Value,_>("value")?,
                "status": row.try_get::<String,_>("support_state")?,
                "authority": row.try_get::<String,_>("authority")?,
                "canonicality": row.try_get::<String,_>("canonicality")?,
                "confidence": row.try_get::<Option<f64>,_>("confidence")?,
                "created_at": row.try_get::<DateTime<Utc>,_>("recorded_at")?
            }))
        })
        .collect()
}

async fn object_states(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    object_id: Uuid,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT state.state_machine_ref,state_assignment.state_value,
               state.current_claim_id,state.updated_at
        FROM straylight.state_heads AS state
        JOIN straylight.state_assignments AS state_assignment
          ON state_assignment.user_id=state.user_id
         AND state_assignment.claim_id=state.current_claim_id
        WHERE state.user_id=$1 AND state.target_record_id=$2
        ORDER BY state.state_machine_ref
        "#,
    )
    .bind(user_id)
    .bind(object_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| -> ApiResult<Value> {
            Ok(json!({
                "machine": row.try_get::<String,_>("state_machine_ref")?,
                "value": row.try_get::<String,_>("state_value")?,
                "claim_id": format!("claim:{}", row.try_get::<Uuid,_>("current_claim_id")?),
                "effective_at": row.try_get::<DateTime<Utc>,_>("updated_at")?
            }))
        })
        .collect()
}

async fn object_versions(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    object_id: Uuid,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT version,previous_version,label,source_episode_id,recorded_at FROM straylight.object_revisions WHERE user_id=$1 AND object_id=$2 ORDER BY version DESC",
    )
    .bind(user_id)
    .bind(object_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| -> ApiResult<Value> {
            Ok(json!({
                "version": row.try_get::<i32,_>("version")?,
                "previous_version": row.try_get::<Option<i32>,_>("previous_version")?,
                "change_summary": row.try_get::<Option<String>,_>("label")?,
                "source_refs": [format!("source:{}", row.try_get::<Uuid,_>("source_episode_id")?)],
                "created_at": row.try_get::<DateTime<Utc>,_>("recorded_at")?
            }))
        })
        .collect()
}

fn source_row_value(row: sqlx::postgres::PgRow) -> ApiResult<Value> {
    let source_id: Uuid = row.try_get("id")?;
    let document_id: Option<Uuid> = row.try_get("document_id")?;
    let content_hash: Option<String> = row.try_get("content_hash")?;
    Ok(json!({
        "id": format!("source:{source_id}"),
        "source_ref": row.try_get::<String,_>("source_ref")?,
        "title": row.try_get::<Option<String>,_>("title")?.unwrap_or_else(|| "Untitled source".to_owned()),
        "kind": row.try_get::<String,_>("source_kind")?,
        "version": row.try_get::<Option<String>,_>("source_version")?,
        "hash": content_hash.map(|hash| format!("sha256:{hash}")),
        "scope_id": row.try_get::<String,_>("scope_ref")?,
        "document_id": document_id.map(|id| format!("document:{id}")),
        "document_version": row.try_get::<Option<i32>,_>("current_version")?,
        "content_type": row.try_get::<Option<String>,_>("media_type")?,
        "size_bytes": row.try_get::<Option<i64>,_>("byte_size")?,
        "metadata": row.try_get::<Value,_>("metadata")?,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "captured_at": row.try_get::<DateTime<Utc>,_>("captured_at")?
    }))
}

fn credential_row_value(row: sqlx::postgres::PgRow) -> ApiResult<Value> {
    let capabilities: Vec<String> = row.try_get("capabilities")?;
    let disabled_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;
    Ok(json!({
        "id": format!("credential:{}", row.try_get::<Uuid,_>("id")?),
        "name": row.try_get::<String,_>("label")?,
        "access": if capabilities.iter().any(|value| value == "save" || value == "checkpoint") { "read_write" } else { "read_only" },
        "scope_ids": row.try_get::<Vec<String>,_>("scope_refs")?,
        "capabilities": capabilities,
        "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
        "revoked_at": disabled_at,
        "status": if disabled_at.is_some() { "revoked" } else { "active" }
    }))
}

fn list_limit(query: &ListQuery) -> usize {
    query.limit.unwrap_or(50).clamp(1, 100)
}

fn list_envelope(items: Vec<Value>, limit: usize) -> Value {
    let has_more = items.len() == limit;
    json!({
        "total": items.len(),
        "items": items,
        "continuation_token": if has_more { Some("more") } else { None::<&str> }
    })
}

fn parse_uuid_ref(value: &str, expected_prefix: &str) -> ApiResult<Uuid> {
    let raw = value
        .strip_prefix(&format!("{expected_prefix}:"))
        .unwrap_or(value);
    Uuid::parse_str(raw)
        .map_err(|_| ApiError::invalid(format!("invalid {expected_prefix} reference")))
}

fn split_uuid_ref(value: &str) -> ApiResult<(&str, Uuid)> {
    let (kind, raw) = value.split_once(':').unwrap_or(("source", value));
    let id = Uuid::parse_str(raw).map_err(|_| ApiError::invalid("invalid record reference"))?;
    Ok((kind, id))
}

fn record_prefix(kind: &str) -> &str {
    match kind {
        "source_episode" => "source",
        "corpus_revision" => "revision",
        "temporal_spec" => "temporal",
        "recurrence_spec" => "recurrence",
        value => value,
    }
}

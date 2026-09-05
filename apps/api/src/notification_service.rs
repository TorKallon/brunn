use std::{collections::HashMap, sync::Arc, time::Duration};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use async_trait::async_trait;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use chrono::{DateTime, NaiveDate, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    config::decode_notification_token_key,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
    pagination,
};

const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 100;
const APNS_LEASE_SECONDS: i64 = 300;
const APNS_ALERT_PREVIEW_MAX_CHARS: usize = 500;
const DEFAULT_NOTIFICATION_TTL_HOURS: i64 = 24;
const MAX_NOTIFICATION_TTL_HOURS: i64 = 24 * 7;
const LIST_NOTIFICATIONS_SQL: &str = r#"
    SELECT notification.id,notification.kind,notification.importance,
           notification.title,notification.body,notification.source,
           notification.target,notification.occurred_at,
           notification.expires_at,notification.created_at,
           state.opened_at,state.acknowledged_at
    FROM brunn.notifications AS notification
    LEFT JOIN brunn.notification_user_state AS state
      ON state.user_id=notification.user_id
     AND state.notification_id=notification.id
    WHERE notification.user_id=$1
      AND notification.kind <> 'location_heartbeat'
      AND (
        $2::boolean IS DISTINCT FROM true
        OR (
          state.opened_at IS NULL
          AND (notification.expires_at IS NULL OR notification.expires_at > clock_timestamp())
        )
      )
      AND ($3::text IS NULL OR notification.importance=$3)
      AND (
        $4::timestamptz IS NULL
        OR (notification.created_at,notification.id) < ($4,$5)
      )
    ORDER BY notification.created_at DESC,notification.id DESC
    LIMIT $6
    "#;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotificationTarget {
    Push,
    Notification,
    Today,
    Briefing {
        date: String,
        edition: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
    },
    Entry {
        entry_ref: String,
    },
    Task {
        task_ref: String,
    },
    Conversation {
        conversation_id: String,
        seq: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRequest {
    pub event_key: String,
    pub correlation_id: String,
    pub kind: String,
    pub importance: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<NotificationSource>,
    pub target: NotificationTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// The fixed four-hour event key is the only heartbeat scheduling state.
/// Reuse the worker's existing non-bearer producer credential and outbox.
pub async fn enqueue_location_heartbeats(
    pool: &PgPool,
    now: DateTime<Utc>,
    delivery_enabled: bool,
) -> ApiResult<u64> {
    let result = sqlx::query(
        r#"
        WITH due AS (
          SELECT presence.user_id,producer.credential_id,
                 'location-heartbeat:' || presence.user_id::text || ':' || $2::text AS event_key
          FROM brunn.location_presence AS presence
          JOIN brunn.task_guard_producers AS producer USING (user_id)
          WHERE presence.reported_at < $1::timestamptz - interval '4 hours'
            AND EXISTS (
              SELECT 1 FROM brunn.notification_installations AS installation
              WHERE installation.user_id=presence.user_id
                AND installation.platform='ios'
                AND installation.enabled AND installation.revoked_at IS NULL
            )
        ), inserted AS (
          INSERT INTO brunn.notifications (
            user_id,producer_credential_id,event_key,request_hash,correlation_id,
            kind,importance,title,body,target,occurred_at,expires_at
          )
          SELECT user_id,credential_id,event_key,
                 encode(public.digest(event_key,'sha256'),'hex'),event_key,
                 'location_heartbeat','normal','Location heartbeat','Location heartbeat',
                 '{"type":"push"}'::jsonb,$1,$1::timestamptz + interval '4 hours'
          FROM due
          ON CONFLICT (user_id,event_key) DO NOTHING
          RETURNING user_id,id
        )
        INSERT INTO brunn.notification_deliveries (
          user_id,notification_id,installation_id,state,available_at,last_error_code
        )
        SELECT notification.user_id,notification.id,installation.id,
               CASE WHEN $3 THEN 'queued' ELSE 'suppressed' END,$1,
               CASE WHEN $3 THEN NULL ELSE 'transport_disabled' END
        FROM inserted AS notification
        JOIN brunn.notification_installations AS installation USING (user_id)
        WHERE installation.platform='ios'
          AND installation.enabled AND installation.revoked_at IS NULL
        "#,
    )
    .bind(now)
    .bind(now.timestamp().div_euclid(4 * 60 * 60).to_string())
    .bind(delivery_enabled)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[derive(Clone, Debug, Serialize)]
pub struct DeliveryView {
    pub delivery_ref: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NotificationView {
    pub notification_ref: String,
    pub kind: String,
    pub importance: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    pub target: Value,
    pub occurred_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub deliveries: Vec<DeliveryView>,
}

#[derive(Debug, Serialize)]
pub struct PublishResponse {
    pub notification: NotificationView,
    pub replayed: bool,
    pub delivery_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublishAccess {
    Public,
    InternalMessaging,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PublishTxResult {
    pub notification_id: Uuid,
    pub inserted: bool,
    pub delivery_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub unread: Option<bool>,
    pub importance: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub items: Vec<NotificationView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub unread_count: i64,
}

#[derive(Debug, Serialize)]
pub struct DetailResponse {
    pub notification: NotificationView,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRequest {
    pub kind: String,
    #[serde(default)]
    pub delivery_ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReceiptResponse {
    pub notification_ref: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_ref: Option<String>,
    pub recorded_at: DateTime<Utc>,
    pub replayed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationRequest {
    pub platform: String,
    pub environment: String,
    pub app_id: String,
    pub device_token: String,
    pub preview: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct InstallationResponse {
    pub installation_ref: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

pub async fn publish(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<PublishRequest>,
) -> ApiResult<Json<PublishResponse>> {
    require_publish(&auth)?;
    auth.require(Capability::Read)?;
    let request = normalize_publish(request);
    if !state.config.messaging_enabled
        && matches!(request.target, NotificationTarget::Conversation { .. })
    {
        return Err(ApiError::invalid(
            "conversation notification targets are unavailable while messaging is disabled",
        ));
    }
    validate_publish(&request)?;
    let mut tx = state.begin_write(&auth).await?;
    let result = publish_in_tx(
        &mut tx,
        &state,
        &auth,
        &request,
        PublishAccess::Public,
        None,
    )
    .await?;
    tx.commit().await?;
    let notification = load_notification(&state, &auth, result.notification_id).await?;
    metrics::counter!(
        "notifications.publish",
        "result" => if result.inserted { "created" } else { "replayed" },
        "kind" => request.kind
    )
    .increment(1);
    Ok(Json(PublishResponse {
        notification,
        replayed: !result.inserted,
        delivery_count: result.delivery_count,
    }))
}

pub(crate) async fn publish_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    auth: &AuthContext,
    request: &PublishRequest,
    access: PublishAccess,
    delivery_available_at: Option<DateTime<Utc>>,
) -> ApiResult<PublishTxResult> {
    validate_publish_for_access(request, access)?;
    let request_hash = canonical_request_hash(request)?;
    let occurred_at = request.occurred_at.unwrap_or_else(Utc::now);
    let expires_at = effective_notification_expiry(occurred_at, request.expires_at);
    let source = request
        .source
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let target = serde_json::to_value(&request.target)?;
    let notification_id = Uuid::now_v7();
    let inserted = sqlx::query(
        r#"
        INSERT INTO brunn.notifications (
          id,user_id,producer_credential_id,event_key,request_hash,
          correlation_id,kind,importance,title,body,source,target,
          occurred_at,expires_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        ON CONFLICT (user_id,event_key) DO NOTHING
        "#,
    )
    .bind(notification_id)
    .bind(auth.user_id.0)
    .bind(auth.credential_id.0)
    .bind(request.event_key.trim())
    .bind(&request_hash)
    .bind(request.correlation_id.trim())
    .bind(&request.kind)
    .bind(&request.importance)
    .bind(request.title.trim())
    .bind(request.body.trim())
    .bind(source)
    .bind(target)
    .bind(occurred_at)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;

    let existing = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,request_hash FROM brunn.notifications WHERE user_id=$1 AND event_key=$2",
    )
    .bind(auth.user_id.0)
    .bind(request.event_key.trim())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((resolved_id, existing_hash)) = existing else {
        return Err(ApiError::conflict(
            "notification_event_key_conflict",
            "the event key was already used with different notification content",
            json!({"event_key": request.event_key.trim()}),
        ));
    };
    if existing_hash != request_hash {
        return Err(ApiError::conflict(
            "notification_event_key_conflict",
            "the event key was already used with different notification content",
            json!({"event_key": request.event_key.trim()}),
        ));
    }
    if inserted {
        sqlx::query(
            r#"
            INSERT INTO brunn.notification_deliveries (
              user_id,notification_id,installation_id,state,last_error_code,available_at
            )
            SELECT $1,$2,installation.id,$3,$4,COALESCE($5,clock_timestamp())
            FROM brunn.notification_installations AS installation
            WHERE installation.user_id=$1
              AND installation.enabled
              AND installation.revoked_at IS NULL
            ON CONFLICT (user_id,notification_id,installation_id) DO NOTHING
            "#,
        )
        .bind(auth.user_id.0)
        .bind(resolved_id)
        .bind(if state.config.apns_delivery_enabled {
            "queued"
        } else {
            "suppressed"
        })
        .bind(if state.config.apns_delivery_enabled {
            None::<&str>
        } else {
            Some("transport_disabled")
        })
        .bind(delivery_available_at)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    }
    let delivery_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.notification_deliveries WHERE user_id=$1 AND notification_id=$2",
    )
    .bind(auth.user_id.0)
    .bind(resolved_id)
    .fetch_one(&mut **tx)
    .await? as usize;
    Ok(PublishTxResult {
        notification_id: resolved_id,
        inserted,
        delivery_count,
    })
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    auth.require(Capability::Read)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if !(1..=MAX_LIST_LIMIT).contains(&limit) {
        return Err(ApiError::invalid("limit must be between 1 and 100"));
    }
    if query
        .importance
        .as_deref()
        .is_some_and(|value| !matches!(value, "normal" | "important"))
    {
        return Err(ApiError::invalid("importance must be normal or important"));
    }
    let filter_hash = list_filter_hash(query.unread, query.importance.as_deref());
    let cursor = query
        .cursor
        .as_deref()
        .map(|cursor| {
            pagination::decode(
                &state.config.continuation_secret,
                cursor,
                auth.user_id.0,
                "notifications",
                &filter_hash,
            )
        })
        .transpose()?;
    let mut tx = state.begin_read(&auth).await?;
    let rows = sqlx::query(LIST_NOTIFICATIONS_SQL)
        .bind(auth.user_id.0)
        .bind(query.unread)
        .bind(query.importance.as_deref())
        .bind(cursor.as_ref().map(|value| value.sort_time))
        .bind(cursor.as_ref().map(|value| value.sort_id))
        .bind(limit + 1)
        .fetch_all(&mut *tx)
        .await?;
    let unread_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM brunn.notifications AS notification
        LEFT JOIN brunn.notification_user_state AS state
          ON state.user_id=notification.user_id
         AND state.notification_id=notification.id
        WHERE notification.user_id=$1
          AND notification.kind <> 'location_heartbeat'
          AND state.opened_at IS NULL
          AND (notification.expires_at IS NULL OR notification.expires_at > clock_timestamp())
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let truncated = rows.len() > limit as usize;
    let visible_rows = &rows[..rows.len().min(limit as usize)];
    let ids: Vec<Uuid> = visible_rows.iter().map(|row| row.get("id")).collect();
    let deliveries = load_deliveries_tx(&mut tx, auth.user_id.0, &ids).await?;
    let mut items = Vec::with_capacity(visible_rows.len());
    for row in visible_rows {
        items.push(notification_from_row(row, &deliveries)?);
    }
    tx.commit().await?;
    let next_cursor = if truncated {
        let row = visible_rows
            .last()
            .expect("a truncated notification page has a final visible row");
        Some(pagination::issue(
            &state.config.continuation_secret,
            auth.user_id.0,
            "notifications",
            &filter_hash,
            row.try_get("created_at")?,
            row.try_get("id")?,
        )?)
    } else {
        None
    };
    Ok(Json(ListResponse {
        items,
        next_cursor,
        unread_count,
    }))
}

pub async fn detail(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(notification_ref): Path<String>,
) -> ApiResult<Json<DetailResponse>> {
    auth.require(Capability::Read)?;
    let id = parse_ref(&notification_ref, "notification")?;
    Ok(Json(DetailResponse {
        notification: load_notification(&state, &auth, id).await?,
    }))
}

pub async fn receipt(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(notification_ref): Path<String>,
    Json(request): Json<ReceiptRequest>,
) -> ApiResult<Json<ReceiptResponse>> {
    require_manage(&auth)?;
    if !matches!(request.kind.as_str(), "opened" | "acknowledged") {
        return Err(ApiError::invalid(
            "receipt kind must be opened or acknowledged",
        ));
    }
    let notification_id = parse_ref(&notification_ref, "notification")?;
    let delivery_id = request
        .delivery_ref
        .as_deref()
        .map(|value| parse_ref(value, "delivery"))
        .transpose()?;
    let mut tx = state.begin_write(&auth).await?;
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM brunn.notifications WHERE user_id=$1 AND id=$2)",
    )
    .bind(auth.user_id.0)
    .bind(notification_id)
    .fetch_one(&mut *tx)
    .await?;
    if !exists {
        return Err(ApiError::not_found(
            "notification_not_found",
            &notification_ref,
        ));
    }
    if let Some(delivery_id) = delivery_id {
        let attributed = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
              SELECT 1 FROM brunn.notification_deliveries
              WHERE user_id=$1 AND id=$2 AND notification_id=$3
            )
            "#,
        )
        .bind(auth.user_id.0)
        .bind(delivery_id)
        .bind(notification_id)
        .fetch_one(&mut *tx)
        .await?;
        if !attributed {
            return Err(ApiError::invalid(
                "delivery_ref does not belong to this notification",
            ));
        }
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO brunn.notification_receipts (
          user_id,notification_id,delivery_id,kind,recorded_by_credential_id
        ) VALUES ($1,$2,$3,$4,$5)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(auth.user_id.0)
    .bind(notification_id)
    .bind(delivery_id)
    .bind(&request.kind)
    .bind(auth.credential_id.0)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let state_row = sqlx::query(
        r#"
        INSERT INTO brunn.notification_user_state (
          user_id,notification_id,opened_at,acknowledged_at
        ) VALUES (
          $1,$2,clock_timestamp(),
          CASE WHEN $3='acknowledged' THEN clock_timestamp() END
        )
        ON CONFLICT (user_id,notification_id) DO UPDATE SET
          opened_at=coalesce(brunn.notification_user_state.opened_at,clock_timestamp()),
          acknowledged_at=CASE WHEN $3='acknowledged'
            THEN coalesce(brunn.notification_user_state.acknowledged_at,clock_timestamp())
            ELSE brunn.notification_user_state.acknowledged_at END,
          updated_at=clock_timestamp()
        RETURNING opened_at,acknowledged_at
        "#,
    )
    .bind(auth.user_id.0)
    .bind(notification_id)
    .bind(&request.kind)
    .fetch_one(&mut *tx)
    .await?;
    let recorded_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        SELECT recorded_at FROM brunn.notification_receipts
        WHERE user_id=$1 AND notification_id=$2 AND kind=$3
          AND delivery_id IS NOT DISTINCT FROM $4
        "#,
    )
    .bind(auth.user_id.0)
    .bind(notification_id)
    .bind(&request.kind)
    .bind(delivery_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(ReceiptResponse {
        notification_ref: format_ref("notification", notification_id),
        kind: request.kind,
        delivery_ref: delivery_id.map(|id| format_ref("delivery", id)),
        recorded_at,
        replayed: !inserted,
        opened_at: state_row.try_get("opened_at")?,
        acknowledged_at: state_row.try_get("acknowledged_at")?,
    }))
}

pub async fn upsert_installation(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(installation_id): Path<String>,
    Json(request): Json<InstallationRequest>,
) -> ApiResult<Json<InstallationResponse>> {
    require_manage(&auth)?;
    let client_installation_id = parse_installation_id(&installation_id)?;
    validate_installation(&request)?;
    let token = request.device_token.trim().to_ascii_lowercase();
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    let expected_app_id = state.config.apns_app_id.as_deref().ok_or_else(|| {
        ApiError::configuration(
            "BRUNN_APNS_APP_ID is required for notification installation registration",
        )
    })?;
    if request.app_id.trim() != expected_app_id {
        return Err(ApiError::invalid(
            "app_id does not match the configured APNs topic",
        ));
    }
    let (ciphertext, nonce, stored_token_hash) = if request.enabled {
        let encoded_key = state
            .config
            .notification_token_encryption_key
            .as_deref()
            .ok_or_else(|| {
                ApiError::configuration(
                    "BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY is required for device registration",
                )
            })?;
        let token_key = decode_notification_token_key(encoded_key)?;
        let token_aad = device_token_aad(
            auth.user_id.0,
            client_installation_id,
            &request.environment,
            request.app_id.trim(),
        );
        let (ciphertext, nonce) = encrypt_device_token(&token_key, &token_aad, &token)?;
        (Some(ciphertext), Some(nonce), Some(token_hash))
    } else {
        (None, None, None)
    };
    let mut tx = state.begin_write(&auth).await?;
    if let Some(token_hash) = stored_token_hash.as_deref() {
        sqlx::query_scalar::<_, i64>("SELECT brunn.claim_notification_device_token($1,$2,$3,$4)")
            .bind(client_installation_id)
            .bind(&request.environment)
            .bind(request.app_id.trim())
            .bind(token_hash)
            .fetch_one(&mut *tx)
            .await?;
    }
    let row = sqlx::query(
        r#"
        INSERT INTO brunn.notification_installations (
          user_id,client_installation_id,registered_by_credential_id,
          platform,environment,app_id,
          token_ciphertext,token_nonce,token_hash,preview,enabled,revoked_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,
                  CASE WHEN $11 THEN NULL ELSE clock_timestamp() END)
        ON CONFLICT (user_id,client_installation_id) DO UPDATE SET
          registered_by_credential_id=EXCLUDED.registered_by_credential_id,
          platform=EXCLUDED.platform,environment=EXCLUDED.environment,
          app_id=EXCLUDED.app_id,token_ciphertext=EXCLUDED.token_ciphertext,
          token_nonce=EXCLUDED.token_nonce,token_hash=EXCLUDED.token_hash,
          preview=EXCLUDED.preview,enabled=EXCLUDED.enabled,
          revoked_at=EXCLUDED.revoked_at,updated_at=clock_timestamp(),
          last_seen_at=clock_timestamp()
        RETURNING id,updated_at
        "#,
    )
    .bind(auth.user_id.0)
    .bind(client_installation_id)
    .bind(auth.credential_id.0)
    .bind(&request.platform)
    .bind(&request.environment)
    .bind(request.app_id.trim())
    .bind(ciphertext)
    .bind(nonce)
    .bind(stored_token_hash)
    .bind(&request.preview)
    .bind(request.enabled)
    .fetch_one(&mut *tx)
    .await?;
    let internal_installation_id: Uuid = row.try_get("id")?;
    if !request.enabled {
        sqlx::query(
            r#"
            UPDATE brunn.notification_deliveries
            SET state='expired',failed_at=clock_timestamp(),lease_expires_at=NULL,
                last_error_code='installation_disabled',updated_at=clock_timestamp()
            WHERE user_id=$1 AND installation_id=$2 AND state IN ('queued','running')
            "#,
        )
        .bind(auth.user_id.0)
        .bind(internal_installation_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(InstallationResponse {
        installation_ref: format_ref("installation", client_installation_id),
        status: if request.enabled {
            "active"
        } else {
            "disabled"
        }
        .to_owned(),
        updated_at: row.try_get("updated_at")?,
    }))
}

pub async fn revoke_installation(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(installation_id): Path<String>,
) -> ApiResult<Json<InstallationResponse>> {
    require_manage(&auth)?;
    let client_installation_id = parse_installation_id(&installation_id)?;
    let mut tx = state.begin_write(&auth).await?;
    let row = sqlx::query(
        r#"
        UPDATE brunn.notification_installations
        SET enabled=false,revoked_at=coalesce(revoked_at,clock_timestamp()),
            token_ciphertext=NULL,token_nonce=NULL,token_hash=NULL,
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND client_installation_id=$2
        RETURNING id,updated_at
        "#,
    )
    .bind(auth.user_id.0)
    .bind(client_installation_id)
    .fetch_optional(&mut *tx)
    .await?;
    let updated_at = if let Some(row) = row {
        let internal_installation_id: Uuid = row.try_get("id")?;
        sqlx::query(
            r#"
            UPDATE brunn.notification_deliveries
            SET state='expired',failed_at=clock_timestamp(),lease_expires_at=NULL,
                last_error_code='installation_revoked',updated_at=clock_timestamp()
            WHERE user_id=$1 AND installation_id=$2 AND state IN ('queued','running')
            "#,
        )
        .bind(auth.user_id.0)
        .bind(internal_installation_id)
        .execute(&mut *tx)
        .await?;
        row.try_get("updated_at")?
    } else {
        Utc::now()
    };
    tx.commit().await?;
    Ok(Json(InstallationResponse {
        installation_ref: format_ref("installation", client_installation_id),
        status: "revoked".to_owned(),
        updated_at,
    }))
}

fn require_publish(auth: &AuthContext) -> ApiResult<()> {
    if auth.can(Capability::NotificationPublish)
        || auth.can(Capability::Save)
        || auth.can(Capability::Admin)
    {
        Ok(())
    } else {
        Err(ApiError::capability(
            Capability::NotificationPublish.as_str(),
        ))
    }
}

fn require_manage(auth: &AuthContext) -> ApiResult<()> {
    if auth.can(Capability::NotificationManage) || auth.can(Capability::Admin) {
        Ok(())
    } else {
        Err(ApiError::capability(
            Capability::NotificationManage.as_str(),
        ))
    }
}

fn validate_publish(request: &PublishRequest) -> ApiResult<()> {
    validate_publish_for_access(request, PublishAccess::Public)
}

fn validate_publish_for_access(request: &PublishRequest, access: PublishAccess) -> ApiResult<()> {
    validate_text(&request.event_key, 200, "event_key")?;
    if request.event_key.starts_with("task-deadline:")
        || request.event_key.starts_with("task-cost:")
        || request.event_key.starts_with("location-heartbeat:")
    {
        return Err(ApiError::invalid(
            "worker event-key namespaces are reserved for the internal scheduler",
        ));
    }
    if access == PublishAccess::Public
        && ["message:", "message-system:", "needs-human:", "reply-by:"]
            .iter()
            .any(|prefix| request.event_key.starts_with(prefix))
    {
        return Err(ApiError::invalid(
            "messaging event-key namespaces are reserved for the internal messaging service",
        ));
    }
    validate_text(&request.correlation_id, 200, "correlation_id")?;
    if !matches!(
        request.kind.as_str(),
        "briefing_ready" | "news_alert" | "correction" | "operational"
    ) {
        return Err(ApiError::invalid("notification kind is unsupported"));
    }
    if !matches!(request.importance.as_str(), "normal" | "important") {
        return Err(ApiError::invalid("importance must be normal or important"));
    }
    validate_text(&request.title, 240, "title")?;
    validate_text(&request.body, 20_000, "body")?;
    if let Some(source) = &request.source {
        validate_text(&source.source_type, 64, "source.type")?;
        validate_text(&source.r#ref, 500, "source.ref")?;
        validate_optional_text(source.version_ref.as_deref(), 500, "source.version_ref")?;
    }
    match &request.target {
        NotificationTarget::Briefing {
            date,
            edition,
            item_id,
        } => {
            NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| ApiError::invalid("target.date must be YYYY-MM-DD"))?;
            validate_text(edition, 64, "target.edition")?;
            validate_optional_text(item_id.as_deref(), 200, "target.item_id")?;
        }
        NotificationTarget::Entry { entry_ref } => {
            validate_text(entry_ref, 500, "target.entry_ref")?;
        }
        NotificationTarget::Task { .. } | NotificationTarget::Push => {
            return Err(ApiError::invalid(
                "task and push targets are reserved for the internal scheduler",
            ));
        }
        NotificationTarget::Conversation {
            conversation_id,
            seq,
        } => {
            parse_canonical_conversation_id(conversation_id)?;
            if *seq <= 0 {
                return Err(ApiError::invalid(
                    "target.seq must be a positive message sequence",
                ));
            }
        }
        NotificationTarget::Notification | NotificationTarget::Today => {}
    }
    let occurred_at = request.occurred_at.unwrap_or_else(Utc::now);
    if request
        .expires_at
        .is_some_and(|expires_at| expires_at <= occurred_at)
    {
        return Err(ApiError::invalid("expires_at must be after occurred_at"));
    }
    if request.expires_at.is_some_and(|expires_at| {
        expires_at > occurred_at + chrono::Duration::hours(MAX_NOTIFICATION_TTL_HOURS)
    }) {
        return Err(ApiError::invalid(
            "expires_at must be no more than 7 days after occurred_at",
        ));
    }
    Ok(())
}

fn effective_notification_expiry(
    occurred_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    expires_at
        .unwrap_or_else(|| occurred_at + chrono::Duration::hours(DEFAULT_NOTIFICATION_TTL_HOURS))
}

fn normalize_publish(mut request: PublishRequest) -> PublishRequest {
    request.event_key = request.event_key.trim().to_owned();
    request.correlation_id = request.correlation_id.trim().to_owned();
    request.kind = request.kind.trim().to_owned();
    request.importance = request.importance.trim().to_owned();
    request.title = request.title.trim().to_owned();
    request.body = request.body.trim().to_owned();
    if let Some(source) = request.source.as_mut() {
        source.source_type = source.source_type.trim().to_owned();
        source.r#ref = source.r#ref.trim().to_owned();
        source.version_ref = source
            .version_ref
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
    }
    match &mut request.target {
        NotificationTarget::Briefing {
            date,
            edition,
            item_id,
        } => {
            *date = date.trim().to_owned();
            *edition = edition.trim().to_owned();
            *item_id = item_id
                .take()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
        }
        NotificationTarget::Entry { entry_ref } => {
            *entry_ref = entry_ref.trim().to_owned();
        }
        NotificationTarget::Task { .. }
        | NotificationTarget::Push
        | NotificationTarget::Conversation { .. }
        | NotificationTarget::Notification
        | NotificationTarget::Today => {}
    }
    request
}

fn validate_installation(request: &InstallationRequest) -> ApiResult<()> {
    if request.platform != "ios" {
        return Err(ApiError::invalid("platform must be ios"));
    }
    if !matches!(request.environment.as_str(), "development" | "production") {
        return Err(ApiError::invalid(
            "environment must be development or production",
        ));
    }
    validate_text(&request.app_id, 255, "app_id")?;
    if !request
        .app_id
        .bytes()
        .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-'))
    {
        return Err(ApiError::invalid("app_id contains unsupported characters"));
    }
    let token = request.device_token.trim();
    if token.len() < 32
        || token.len() > 400
        || !token.len().is_multiple_of(2)
        || !token.bytes().all(|value| value.is_ascii_hexdigit())
    {
        return Err(ApiError::invalid(
            "device_token must be an even-length hexadecimal token",
        ));
    }
    if request.preview != "generic" {
        return Err(ApiError::invalid("preview must be generic"));
    }
    Ok(())
}

fn validate_text(value: &str, max: usize, field: &str) -> ApiResult<()> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(ApiError::invalid(format!(
            "{field} must contain between 1 and {max} characters"
        )));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, max: usize, field: &str) -> ApiResult<()> {
    if let Some(value) = value {
        validate_text(value, max, field)?;
    }
    Ok(())
}

fn canonical_request_hash(request: &PublishRequest) -> ApiResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(request)?)))
}

fn list_filter_hash(unread: Option<bool>, importance: Option<&str>) -> String {
    hex::encode(Sha256::digest(
        format!("unread={unread:?};importance={importance:?}").as_bytes(),
    ))
}

fn format_ref(prefix: &str, id: Uuid) -> String {
    format!("{prefix}:{}", id.simple())
}

fn parse_ref(value: &str, expected_prefix: &str) -> ApiResult<Uuid> {
    let (prefix, id) = value
        .split_once(':')
        .ok_or_else(|| ApiError::invalid(format!("{expected_prefix}_ref is malformed")))?;
    if prefix != expected_prefix {
        return Err(ApiError::invalid(format!(
            "expected a {expected_prefix} reference"
        )));
    }
    Uuid::parse_str(id).map_err(|_| ApiError::invalid(format!("{expected_prefix}_ref is invalid")))
}

fn parse_installation_id(value: &str) -> ApiResult<Uuid> {
    if value.starts_with("installation:") {
        parse_ref(value, "installation")
    } else {
        Uuid::parse_str(value).map_err(|_| ApiError::invalid("installation ID is invalid"))
    }
}

fn notification_from_row(
    row: &sqlx::postgres::PgRow,
    deliveries: &HashMap<Uuid, Vec<DeliveryView>>,
) -> ApiResult<NotificationView> {
    let id: Uuid = row.try_get("id")?;
    Ok(NotificationView {
        notification_ref: format_ref("notification", id),
        kind: row.try_get("kind")?,
        importance: row.try_get("importance")?,
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        source: row.try_get("source")?,
        target: row.try_get("target")?,
        occurred_at: row.try_get("occurred_at")?,
        expires_at: row.try_get("expires_at")?,
        opened_at: row.try_get("opened_at")?,
        acknowledged_at: row.try_get("acknowledged_at")?,
        deliveries: deliveries.get(&id).cloned().unwrap_or_default(),
    })
}

async fn load_notification(
    state: &AppState,
    auth: &AuthContext,
    notification_id: Uuid,
) -> ApiResult<NotificationView> {
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT notification.id,notification.kind,notification.importance,
               notification.title,notification.body,notification.source,
               notification.target,notification.occurred_at,
               notification.expires_at,notification.created_at,
               state.opened_at,state.acknowledged_at
        FROM brunn.notifications AS notification
        LEFT JOIN brunn.notification_user_state AS state
          ON state.user_id=notification.user_id
         AND state.notification_id=notification.id
        WHERE notification.user_id=$1 AND notification.id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(notification_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::not_found(
            "notification_not_found",
            &format_ref("notification", notification_id),
        )
    })?;
    let deliveries = load_deliveries_tx(&mut tx, auth.user_id.0, &[notification_id]).await?;
    let result = notification_from_row(&row, &deliveries)?;
    tx.commit().await?;
    Ok(result)
}

async fn load_deliveries_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    notification_ids: &[Uuid],
) -> ApiResult<HashMap<Uuid, Vec<DeliveryView>>> {
    if notification_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT notification_id,id,state,accepted_at,failed_at,last_error_code
        FROM brunn.notification_deliveries
        WHERE user_id=$1 AND notification_id=ANY($2)
        ORDER BY created_at,id
        "#,
    )
    .bind(user_id)
    .bind(notification_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut result: HashMap<Uuid, Vec<DeliveryView>> = HashMap::new();
    for row in rows {
        let notification_id: Uuid = row.try_get("notification_id")?;
        let delivery_id: Uuid = row.try_get("id")?;
        result
            .entry(notification_id)
            .or_default()
            .push(DeliveryView {
                delivery_ref: format_ref("delivery", delivery_id),
                state: row.try_get("state")?,
                accepted_at: row.try_get("accepted_at")?,
                failed_at: row.try_get("failed_at")?,
                last_error_code: row.try_get("last_error_code")?,
            });
    }
    Ok(result)
}

fn device_token_aad(
    user_id: Uuid,
    installation_id: Uuid,
    environment: &str,
    app_id: &str,
) -> Vec<u8> {
    format!("brunn.apns-token.v1|{user_id}|{installation_id}|{environment}|{app_id}").into_bytes()
}

fn encrypt_device_token(key: &[u8; 32], aad: &[u8], token: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| ApiError::Internal("could not initialize token encryption".to_owned()))?;
    let nonce_bytes: [u8; 12] = rand::random();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: token.as_bytes(),
                aad,
            },
        )
        .map_err(|_| ApiError::Internal("could not encrypt APNs token".to_owned()))?;
    Ok((ciphertext, nonce_bytes.to_vec()))
}

fn decrypt_device_token(
    key: &[u8; 32],
    aad: &[u8],
    ciphertext: &[u8],
    nonce: &[u8],
) -> ApiResult<String> {
    if nonce.len() != 12 {
        return Err(ApiError::Internal(
            "stored APNs nonce is invalid".to_owned(),
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| ApiError::Internal("could not initialize token decryption".to_owned()))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| ApiError::Internal("stored APNs token could not be decrypted".to_owned()))?;
    String::from_utf8(plaintext)
        .map_err(|_| ApiError::Internal("stored APNs token is not UTF-8".to_owned()))
}

#[derive(Clone, Debug)]
pub struct ApnsRequest {
    pub device_token: String,
    pub environment: String,
    pub app_id: String,
    pub payload: Value,
    pub apns_id: Uuid,
    pub collapse_id: String,
    pub expiration: Option<DateTime<Utc>>,
    pub push_type: &'static str,
    pub priority: u16,
}

#[derive(Clone, Debug)]
pub struct ApnsAccepted {
    pub provider_request_id: Option<String>,
    pub status: u16,
}

#[derive(Clone, Debug)]
pub struct ApnsFailure {
    pub code: String,
    pub status: Option<u16>,
    pub provider_request_id: Option<String>,
    pub retryable: bool,
    pub provider_blocked: bool,
    pub invalidate_token: bool,
    pub retry_after_seconds: Option<i64>,
}

#[async_trait]
pub trait ApnsProvider: Send + Sync {
    async fn send(&self, request: ApnsRequest) -> Result<ApnsAccepted, ApnsFailure>;

    async fn blocked_until(&self) -> Option<DateTime<Utc>> {
        None
    }
}

struct HttpApnsProvider {
    client: reqwest::Client,
    team_id: String,
    key_id: String,
    app_id: String,
    key: EncodingKey,
    cached_bearer: Mutex<Option<CachedBearer>>,
    blocked_until: Mutex<Option<DateTime<Utc>>>,
}

struct CachedBearer {
    token: String,
    issued_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ApnsClaims<'a> {
    iss: &'a str,
    iat: i64,
}

fn bearer_is_fresh(issued_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    let age = now.signed_duration_since(issued_at).num_seconds();
    (0..chrono::Duration::minutes(50).num_seconds()).contains(&age)
}

fn apns_id_header(apns_id: Uuid) -> String {
    apns_id.to_string()
}

fn apns_collapse_id(notification_id: Uuid) -> String {
    format!("notification-{}", notification_id.simple())
}

fn apns_expiration_header(expiration: Option<DateTime<Utc>>) -> String {
    expiration
        .map_or(0, |value| value.timestamp().max(0))
        .to_string()
}

fn retry_after_seconds(value: Option<&str>, now: DateTime<Utc>) -> Option<i64> {
    let value = value?;
    value
        .parse::<i64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .or_else(|| {
            DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|timestamp| {
                    timestamp
                        .with_timezone(&Utc)
                        .signed_duration_since(now)
                        .num_seconds()
                })
                .filter(|seconds| *seconds > 0)
        })
}

fn classify_apns_failure(
    status: u16,
    raw_code: &str,
    provider_request_id: Option<String>,
    retry_after_seconds: Option<i64>,
) -> ApnsFailure {
    let provider_blocked = matches!(
        raw_code,
        "BadCertificate"
            | "BadCertificateEnvironment"
            | "BadTopic"
            | "ExpiredProviderToken"
            | "Forbidden"
            | "InvalidProviderToken"
            | "MissingProviderToken"
            | "TooManyProviderTokenUpdates"
    );
    ApnsFailure {
        code: sanitize_provider_code(raw_code),
        status: Some(status),
        provider_request_id,
        retryable: provider_blocked
            || status == 429
            || status >= 500
            || matches!(raw_code, "ExpiredProviderToken"),
        provider_blocked,
        invalidate_token: status == 410
            || matches!(raw_code, "BadDeviceToken" | "DeviceTokenNotForTopic"),
        retry_after_seconds,
    }
}

impl HttpApnsProvider {
    fn from_state(state: &AppState) -> ApiResult<Option<Self>> {
        if !state.config.apns_configured() {
            return Ok(None);
        }
        let private_key = state
            .config
            .apns_private_key
            .as_deref()
            .expect("configured APNs private key");
        let key = EncodingKey::from_ec_pem(private_key.as_bytes())
            .map_err(|_| ApiError::configuration("BRUNN_APNS_PRIVATE_KEY is not a valid EC key"))?;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| {
                ApiError::configuration(format!("could not build APNs client: {error}"))
            })?;
        Ok(Some(Self {
            client,
            team_id: state
                .config
                .apns_team_id
                .clone()
                .expect("configured APNs team ID"),
            key_id: state
                .config
                .apns_key_id
                .clone()
                .expect("configured APNs key ID"),
            app_id: state
                .config
                .apns_app_id
                .clone()
                .expect("configured APNs app ID"),
            key,
            cached_bearer: Mutex::new(None),
            blocked_until: Mutex::new(None),
        }))
    }

    async fn note_provider_block(&self, failure: &ApnsFailure) {
        if !failure.provider_blocked {
            return;
        }
        *self.cached_bearer.lock().await = None;
        let delay_seconds = provider_block_delay_seconds(failure.retry_after_seconds);
        *self.blocked_until.lock().await =
            Some(Utc::now() + chrono::Duration::seconds(delay_seconds));
    }

    async fn bearer(&self) -> Result<String, ApnsFailure> {
        let now = Utc::now();
        let mut cached = self.cached_bearer.lock().await;
        if let Some(cached) = cached.as_ref()
            && bearer_is_fresh(cached.issued_at, now)
        {
            return Ok(cached.token.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let token = encode(
            &header,
            &ApnsClaims {
                iss: &self.team_id,
                iat: now.timestamp(),
            },
            &self.key,
        )
        .map_err(|_| ApnsFailure {
            code: "provider_token_signing_failed".to_owned(),
            status: None,
            provider_request_id: None,
            retryable: true,
            provider_blocked: true,
            invalidate_token: false,
            retry_after_seconds: None,
        })?;
        *cached = Some(CachedBearer {
            token: token.clone(),
            issued_at: now,
        });
        Ok(token)
    }
}

#[async_trait]
impl ApnsProvider for HttpApnsProvider {
    async fn send(&self, request: ApnsRequest) -> Result<ApnsAccepted, ApnsFailure> {
        if let Some(blocked_until) = *self.blocked_until.lock().await
            && blocked_until > Utc::now()
        {
            return Err(ApnsFailure {
                code: "provider_circuit_open".to_owned(),
                status: None,
                provider_request_id: None,
                retryable: true,
                provider_blocked: true,
                invalidate_token: false,
                retry_after_seconds: Some(
                    blocked_until
                        .signed_duration_since(Utc::now())
                        .num_seconds()
                        .max(1),
                ),
            });
        }
        if request.app_id != self.app_id {
            let failure = ApnsFailure {
                code: "topic_mismatch".to_owned(),
                status: None,
                provider_request_id: None,
                retryable: true,
                provider_blocked: true,
                invalidate_token: false,
                retry_after_seconds: None,
            };
            self.note_provider_block(&failure).await;
            return Err(failure);
        }
        let host = if request.environment == "development" {
            "https://api.sandbox.push.apple.com"
        } else {
            "https://api.push.apple.com"
        };
        let bearer = match self.bearer().await {
            Ok(bearer) => bearer,
            Err(failure) => {
                self.note_provider_block(&failure).await;
                return Err(failure);
            }
        };
        let response = self
            .client
            .post(format!("{host}/3/device/{}", request.device_token))
            .header("authorization", format!("bearer {bearer}"))
            .header("apns-topic", request.app_id)
            .header("apns-push-type", request.push_type)
            .header("apns-priority", request.priority)
            .header("apns-id", apns_id_header(request.apns_id))
            .header("apns-collapse-id", request.collapse_id)
            .header(
                "apns-expiration",
                apns_expiration_header(request.expiration),
            )
            .json(&request.payload)
            .send()
            .await
            .map_err(|_| ApnsFailure {
                code: "transport_error".to_owned(),
                status: None,
                provider_request_id: None,
                retryable: true,
                provider_blocked: false,
                invalidate_token: false,
                retry_after_seconds: None,
            })?;
        let status = response.status().as_u16();
        let provider_request_id = response
            .headers()
            .get("apns-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry_after_seconds = retry_after_seconds(
            response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Utc::now(),
        );
        if status == 200 {
            return Ok(ApnsAccepted {
                provider_request_id,
                status,
            });
        }
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let raw_code = body
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("apns_rejected");
        let failure =
            classify_apns_failure(status, raw_code, provider_request_id, retry_after_seconds);
        self.note_provider_block(&failure).await;
        Err(failure)
    }

    async fn blocked_until(&self) -> Option<DateTime<Utc>> {
        *self.blocked_until.lock().await
    }
}

#[derive(Debug)]
struct ClaimedDelivery {
    id: Uuid,
    user_id: Uuid,
    notification_id: Uuid,
    installation_id: Uuid,
    client_installation_id: Uuid,
    attempt_number: i32,
    budget_attempt_count: i32,
    max_attempts: i32,
    kind: String,
    body: String,
    target: Value,
    environment: String,
    app_id: String,
    token_ciphertext: Vec<u8>,
    token_nonce: Vec<u8>,
    expires_at: Option<DateTime<Utc>>,
}

pub fn configured_apns_provider(state: &AppState) -> ApiResult<Option<Arc<dyn ApnsProvider>>> {
    Ok(HttpApnsProvider::from_state(state)?
        .map(|provider| Arc::new(provider) as Arc<dyn ApnsProvider>))
}

pub async fn process_next_delivery(state: &AppState) -> ApiResult<bool> {
    let Some(provider) = configured_apns_provider(state)? else {
        return expire_queued_deliveries(state).await;
    };
    process_next_with_provider(state, provider).await
}

pub async fn process_next_with_provider(
    state: &AppState,
    provider: Arc<dyn ApnsProvider>,
) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().ok_or_else(|| {
        ApiError::configuration("DATABASE_URL_ADMIN is required by notification delivery")
    })?;
    let encoded_key = state
        .config
        .notification_token_encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::configuration(
                "BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY is required for APNs delivery",
            )
        })?;
    process_next_on_pool(pool, encoded_key, provider).await
}

pub async fn process_next_on_pool(
    pool: &PgPool,
    encoded_key: &str,
    provider: Arc<dyn ApnsProvider>,
) -> ApiResult<bool> {
    let expired = expire_queued_deliveries_on_pool(pool).await?;
    if provider
        .blocked_until()
        .await
        .is_some_and(|blocked_until| blocked_until > Utc::now())
    {
        return Ok(expired);
    }
    let Some(delivery) = claim_delivery(pool).await? else {
        return Ok(expired);
    };
    let token_key = decode_notification_token_key(encoded_key)?;
    let token_aad = device_token_aad(
        delivery.user_id,
        delivery.client_installation_id,
        &delivery.environment,
        &delivery.app_id,
    );
    let device_token = match decrypt_device_token(
        &token_key,
        &token_aad,
        &delivery.token_ciphertext,
        &delivery.token_nonce,
    ) {
        Ok(token) => token,
        Err(_) => {
            record_failure(
                pool,
                &delivery,
                ApnsFailure {
                    code: "token_decryption_failed".to_owned(),
                    status: None,
                    provider_request_id: None,
                    retryable: false,
                    provider_blocked: false,
                    invalidate_token: false,
                    retry_after_seconds: None,
                },
            )
            .await?;
            return Ok(true);
        }
    };
    let push_target = resolve_push_target(&delivery.target);
    let heartbeat = delivery.kind == "location_heartbeat";
    let request = ApnsRequest {
        device_token,
        environment: delivery.environment.clone(),
        app_id: delivery.app_id.clone(),
        payload: push_payload(&delivery, &push_target),
        apns_id: delivery.id,
        collapse_id: push_collapse_id(&delivery, &push_target),
        expiration: delivery.expires_at,
        push_type: if heartbeat { "background" } else { "alert" },
        priority: if heartbeat { 5 } else { 10 },
    };
    match provider.send(request).await {
        Ok(accepted) => record_acceptance(pool, &delivery, accepted).await?,
        Err(failure) => record_failure(pool, &delivery, failure).await?,
    }
    Ok(true)
}

pub async fn expire_queued_deliveries(state: &AppState) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().ok_or_else(|| {
        ApiError::configuration("DATABASE_URL_ADMIN is required by notification delivery")
    })?;
    expire_queued_deliveries_on_pool(pool).await
}

pub async fn suppress_queued_deliveries(state: &AppState) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().ok_or_else(|| {
        ApiError::configuration("DATABASE_URL_ADMIN is required by notification delivery")
    })?;
    suppress_queued_deliveries_on_pool(pool).await
}

pub async fn suppress_queued_deliveries_on_pool(pool: &PgPool) -> ApiResult<bool> {
    let affected = sqlx::query(
        r#"
        UPDATE brunn.notification_deliveries
        SET state='suppressed',failed_at=NULL,lease_expires_at=NULL,
            last_error_code='transport_disabled',updated_at=clock_timestamp()
        WHERE state IN ('queued','running')
        "#,
    )
    .execute(pool)
    .await?
    .rows_affected();
    if affected > 0 {
        metrics::counter!("notifications.delivery", "result" => "suppressed").increment(affected);
    }
    Ok(affected > 0)
}

async fn expire_queued_deliveries_on_pool(pool: &PgPool) -> ApiResult<bool> {
    let affected = sqlx::query(
        r#"
        UPDATE brunn.notification_deliveries AS delivery
        SET state='expired',failed_at=clock_timestamp(),
            lease_expires_at=NULL,last_error_code='notification_expired',
            updated_at=clock_timestamp()
        FROM brunn.notifications AS notification
        WHERE delivery.user_id=notification.user_id
          AND delivery.notification_id=notification.id
          AND delivery.state IN ('queued','running')
          AND notification.expires_at <= clock_timestamp()
        "#,
    )
    .execute(pool)
    .await?
    .rows_affected();
    if affected > 0 {
        metrics::counter!("notifications.delivery", "result" => "expired").increment(affected);
    }
    Ok(affected > 0)
}

async fn claim_delivery(pool: &PgPool) -> ApiResult<Option<ClaimedDelivery>> {
    let row = sqlx::query(
        r#"
        WITH candidate AS (
          SELECT delivery.id
          FROM brunn.notification_deliveries AS delivery
          JOIN brunn.notification_installations AS installation
            ON installation.user_id=delivery.user_id
           AND installation.id=delivery.installation_id
          JOIN brunn.notifications AS notification
            ON notification.user_id=delivery.user_id
           AND notification.id=delivery.notification_id
          WHERE (
            (
              delivery.state='queued'
              AND delivery.available_at <= clock_timestamp()
              AND delivery.attempt_count < delivery.max_attempts
            ) OR (
              delivery.state='running'
              AND delivery.lease_expires_at <= clock_timestamp()
            )
          )
          AND installation.enabled
          AND installation.revoked_at IS NULL
          AND (notification.expires_at IS NULL OR notification.expires_at > clock_timestamp())
          ORDER BY delivery.available_at,delivery.created_at,delivery.id
          FOR UPDATE OF delivery SKIP LOCKED
          LIMIT 1
        )
        UPDATE brunn.notification_deliveries AS delivery
        SET state='running',attempt_count=CASE
              WHEN delivery.state='queued' THEN delivery.attempt_count+1
              ELSE delivery.attempt_count
            END,
            last_attempt_at=clock_timestamp(),
            lease_expires_at=clock_timestamp()+make_interval(secs => $1),
            updated_at=clock_timestamp()
        FROM candidate,
             brunn.notifications AS notification,
             brunn.notification_installations AS installation
        WHERE delivery.id=candidate.id
          AND notification.user_id=delivery.user_id
          AND notification.id=delivery.notification_id
          AND installation.user_id=delivery.user_id
          AND installation.id=delivery.installation_id
        RETURNING delivery.id,delivery.user_id,delivery.notification_id,
                  delivery.installation_id,
                  delivery.attempt_count+delivery.provider_block_count AS attempt_number,
                  delivery.attempt_count AS budget_attempt_count,
                  delivery.max_attempts,notification.kind,notification.body,
                  notification.target,
                  notification.expires_at,
                  installation.client_installation_id,
                  installation.environment,installation.app_id,
                  installation.token_ciphertext,installation.token_nonce
        "#,
    )
    .bind(APNS_LEASE_SECONDS)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(ClaimedDelivery {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            notification_id: row.try_get("notification_id")?,
            installation_id: row.try_get("installation_id")?,
            client_installation_id: row.try_get("client_installation_id")?,
            attempt_number: row.try_get("attempt_number")?,
            budget_attempt_count: row.try_get("budget_attempt_count")?,
            max_attempts: row.try_get("max_attempts")?,
            kind: row.try_get("kind")?,
            body: row.try_get("body")?,
            target: row.try_get("target")?,
            environment: row.try_get("environment")?,
            app_id: row.try_get("app_id")?,
            token_ciphertext: row.try_get("token_ciphertext")?,
            token_nonce: row.try_get("token_nonce")?,
            expires_at: row.try_get("expires_at")?,
        })
    })
    .transpose()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ResolvedPushTarget {
    Conversation { conversation_id: Uuid, seq: i64 },
    Task { task_ref: String },
    Ordinary,
    Malformed,
}

fn resolve_push_target(target: &Value) -> ResolvedPushTarget {
    match serde_json::from_value::<NotificationTarget>(target.clone()) {
        Ok(NotificationTarget::Conversation {
            conversation_id,
            seq,
        }) => match parse_canonical_conversation_id(&conversation_id) {
            Ok(conversation_id) if seq > 0 => ResolvedPushTarget::Conversation {
                conversation_id,
                seq,
            },
            _ => ResolvedPushTarget::Malformed,
        },
        Ok(NotificationTarget::Task { task_ref }) => match parse_canonical_task_ref(&task_ref) {
            Ok(_) => ResolvedPushTarget::Task { task_ref },
            Err(_) => ResolvedPushTarget::Malformed,
        },
        Ok(
            NotificationTarget::Notification
            | NotificationTarget::Push
            | NotificationTarget::Today
            | NotificationTarget::Briefing { .. }
            | NotificationTarget::Entry { .. },
        ) => ResolvedPushTarget::Ordinary,
        Err(_) => ResolvedPushTarget::Malformed,
    }
}

fn push_payload(delivery: &ClaimedDelivery, target: &ResolvedPushTarget) -> Value {
    if delivery.kind == "location_heartbeat" {
        return json!({
            "aps": {"content-available": 1},
            "schema": "brunn-push@v1",
            "kind": "location_heartbeat"
        });
    }
    let notification_ref = format_ref("notification", delivery.notification_id);
    let delivery_ref = format_ref("delivery", delivery.id);
    let mut aps = json!({
        "alert": {
            "title": "Brunn",
            "body": push_body(delivery, target)
        }
    });
    if matches!(target, ResolvedPushTarget::Conversation { .. }) {
        aps["content-available"] = json!(1);
    }
    json!({
        "aps": aps,
        "schema": "brunn-push@v1",
        "notification_ref": notification_ref,
        "delivery_ref": delivery_ref,
        "brunn_route": push_route(delivery, target)
    })
}

fn push_route(delivery: &ClaimedDelivery, target: &ResolvedPushTarget) -> String {
    match target {
        ResolvedPushTarget::Conversation {
            conversation_id,
            seq,
        } => format!("brunn://conversation/{conversation_id}?seq={seq}"),
        ResolvedPushTarget::Task { task_ref } => format!("brunn://task/{task_ref}"),
        _ => format!(
            "brunn://notification/{}?delivery={}",
            delivery.notification_id.simple(),
            delivery.id.simple()
        ),
    }
}

fn push_collapse_id(delivery: &ClaimedDelivery, target: &ResolvedPushTarget) -> String {
    if delivery.kind == "location_heartbeat" {
        return "location-heartbeat".to_owned();
    }
    match target {
        ResolvedPushTarget::Conversation {
            conversation_id, ..
        } => conversation_id.to_string(),
        _ => apns_collapse_id(delivery.notification_id),
    }
}

fn push_body(delivery: &ClaimedDelivery, target: &ResolvedPushTarget) -> String {
    match target {
        ResolvedPushTarget::Conversation { .. } => "A new agent message is available.".to_owned(),
        ResolvedPushTarget::Task { .. } => generic_push_body(&delivery.kind).to_owned(),
        ResolvedPushTarget::Ordinary if delivery.kind == "operational" => {
            alert_text_preview(&delivery.body)
        }
        ResolvedPushTarget::Ordinary | ResolvedPushTarget::Malformed => {
            generic_push_body(&delivery.kind).to_owned()
        }
    }
}

fn generic_push_body(kind: &str) -> &'static str {
    match kind {
        "briefing_ready" => "Your briefing is ready.",
        "correction" => "A Brunn update needs your attention.",
        "operational" => "Brunn has an operational alert.",
        _ => "A new Brunn alert is available.",
    }
}

fn parse_canonical_task_ref(value: &str) -> ApiResult<Uuid> {
    let task_id = Uuid::parse_str(value)
        .map_err(|_| ApiError::invalid("target.task_ref must be a canonical UUIDv7"))?;
    if value != task_id.to_string() || task_id.get_version_num() != 7 {
        return Err(ApiError::invalid(
            "target.task_ref must be a canonical lowercase hyphenated UUIDv7",
        ));
    }
    Ok(task_id)
}

fn parse_canonical_conversation_id(value: &str) -> ApiResult<Uuid> {
    let conversation_id = Uuid::parse_str(value)
        .map_err(|_| ApiError::invalid("target.conversation_id must be a canonical UUIDv7"))?;
    if value != conversation_id.to_string() || conversation_id.get_version_num() != 7 {
        return Err(ApiError::invalid(
            "target.conversation_id must be a canonical lowercase hyphenated UUIDv7",
        ));
    }
    Ok(conversation_id)
}

fn alert_text_preview(body: &str) -> String {
    if body.chars().count() <= APNS_ALERT_PREVIEW_MAX_CHARS {
        return body.to_owned();
    }
    let mut preview = body
        .chars()
        .take(APNS_ALERT_PREVIEW_MAX_CHARS - 1)
        .collect::<String>();
    preview.push('…');
    preview
}

async fn record_acceptance(
    pool: &PgPool,
    delivery: &ClaimedDelivery,
    accepted: ApnsAccepted,
) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO brunn.notification_attempts (
          user_id,delivery_id,attempt_number,result,provider_status,
          provider_request_id
        ) VALUES ($1,$2,$3,'accepted_by_apns',$4,$5)
        ON CONFLICT (user_id,delivery_id,attempt_number) DO NOTHING
        "#,
    )
    .bind(delivery.user_id)
    .bind(delivery.id)
    .bind(delivery.attempt_number)
    .bind(i32::from(accepted.status))
    .bind(accepted.provider_request_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE brunn.notification_deliveries
        SET state='accepted_by_apns',accepted_at=clock_timestamp(),
            lease_expires_at=NULL,last_error_code=NULL,updated_at=clock_timestamp()
        WHERE user_id=$1 AND id=$2 AND state='running' AND attempt_count=$3
        "#,
    )
    .bind(delivery.user_id)
    .bind(delivery.id)
    .bind(delivery.budget_attempt_count)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    metrics::counter!("notifications.delivery", "result" => "accepted_by_apns").increment(1);
    Ok(())
}

async fn record_failure(
    pool: &PgPool,
    delivery: &ClaimedDelivery,
    failure: ApnsFailure,
) -> ApiResult<()> {
    let exhausted = delivery.budget_attempt_count >= delivery.max_attempts;
    let retry = failure.provider_blocked || (failure.retryable && !exhausted);
    let result = if retry {
        "retryable_failure"
    } else {
        "permanent_failure"
    };
    let delay_seconds = if failure.provider_blocked {
        provider_block_delay_seconds(failure.retry_after_seconds)
    } else {
        retry_delay_seconds(delivery.budget_attempt_count, failure.retry_after_seconds)
    };
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO brunn.notification_attempts (
          user_id,delivery_id,attempt_number,result,provider_status,
          provider_request_id,error_code
        ) VALUES ($1,$2,$3,$4,$5,$6,$7)
        ON CONFLICT (user_id,delivery_id,attempt_number) DO NOTHING
        "#,
    )
    .bind(delivery.user_id)
    .bind(delivery.id)
    .bind(delivery.attempt_number)
    .bind(result)
    .bind(failure.status.map(i32::from))
    .bind(&failure.provider_request_id)
    .bind(&failure.code)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE brunn.notification_deliveries
        SET state=CASE WHEN $4 THEN 'queued' ELSE 'failed' END,
            available_at=CASE WHEN $4
              THEN clock_timestamp()+make_interval(secs => $5)
              ELSE available_at END,
            failed_at=CASE WHEN $4 THEN NULL ELSE clock_timestamp() END,
            attempt_count=CASE WHEN $7
              THEN greatest(attempt_count-1,0)
              ELSE attempt_count END,
            provider_block_count=CASE WHEN $7
              THEN provider_block_count+1
              ELSE provider_block_count END,
            lease_expires_at=NULL,last_error_code=$6,updated_at=clock_timestamp()
        WHERE user_id=$1 AND id=$2 AND state='running' AND attempt_count=$3
        "#,
    )
    .bind(delivery.user_id)
    .bind(delivery.id)
    .bind(delivery.budget_attempt_count)
    .bind(retry)
    .bind(delay_seconds)
    .bind(&failure.code)
    .bind(failure.provider_blocked)
    .execute(&mut *tx)
    .await?;
    if failure.invalidate_token {
        sqlx::query(
            r#"
            UPDATE brunn.notification_installations
            SET enabled=false,revoked_at=clock_timestamp(),
                token_ciphertext=NULL,token_nonce=NULL,token_hash=NULL,
                updated_at=clock_timestamp()
            WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(delivery.user_id)
        .bind(delivery.installation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE brunn.notification_deliveries
            SET state='expired',failed_at=clock_timestamp(),
                last_error_code='installation_token_invalid',updated_at=clock_timestamp()
            WHERE user_id=$1 AND installation_id=$2 AND state='queued'
            "#,
        )
        .bind(delivery.user_id)
        .bind(delivery.installation_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    metrics::counter!("notifications.delivery", "result" => result).increment(1);
    Ok(())
}

fn retry_delay_seconds(attempt_number: i32, retry_after_seconds: Option<i64>) -> i64 {
    2_i64
        .pow(attempt_number.clamp(1, 8) as u32)
        .max(retry_after_seconds.unwrap_or(0))
        .min(3_600)
}

fn provider_block_delay_seconds(retry_after_seconds: Option<i64>) -> i64 {
    retry_after_seconds.unwrap_or(0).clamp(60, 3_600)
}

fn sanitize_provider_code(value: &str) -> String {
    let value: String = value
        .chars()
        .filter(|value| value.is_ascii_alphanumeric() || *value == '_')
        .take(120)
        .collect();
    if value.is_empty() {
        "apns_rejected".to_owned()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PublishRequest {
        PublishRequest {
            event_key: "briefing:2026-08-03:morning".to_owned(),
            correlation_id: "briefing:2026-08-03".to_owned(),
            kind: "briefing_ready".to_owned(),
            importance: "normal".to_owned(),
            title: "Morning briefing".to_owned(),
            body: "Your morning briefing is ready.".to_owned(),
            source: Some(NotificationSource {
                source_type: "entry".to_owned(),
                r#ref: "entry:019fc000000070008000000000000001".to_owned(),
                version_ref: Some("entry-version:019fc000000070008000000000000002".to_owned()),
            }),
            target: NotificationTarget::Briefing {
                date: "2026-08-03".to_owned(),
                edition: "morning".to_owned(),
                item_id: None,
            },
            occurred_at: Some(Utc::now()),
            expires_at: None,
        }
    }

    #[test]
    fn canonical_hash_is_stable_and_content_sensitive() {
        let request = fixture();
        let first = canonical_request_hash(&request).unwrap();
        assert_eq!(first, canonical_request_hash(&request).unwrap());
        let mut changed = request;
        changed.body.push_str(" Updated.");
        assert_ne!(first, canonical_request_hash(&changed).unwrap());
    }

    #[test]
    fn public_publish_cannot_preempt_task_guard_event_keys() {
        for event_key in [
            format!("task-deadline:{}:7d", Uuid::now_v7()),
            format!("task-cost:{}:set", Uuid::now_v7()),
            format!("location-heartbeat:{}:1", Uuid::now_v7()),
        ] {
            let mut request = fixture();
            request.event_key = event_key;
            assert!(validate_publish(&request).is_err());
        }
    }

    #[test]
    fn public_publish_cannot_preempt_messaging_event_keys() {
        for prefix in ["message", "message-system", "needs-human", "reply-by"] {
            let mut request = fixture();
            request.event_key = format!("{prefix}:{}:1", Uuid::now_v7());
            assert!(validate_publish(&request).is_err());
        }
    }

    #[test]
    fn public_publish_cannot_create_task_target_notifications() {
        let mut request = fixture();
        request.event_key = format!("operational:{}", Uuid::now_v7());
        request.target = NotificationTarget::Task {
            task_ref: Uuid::now_v7().to_string(),
        };
        assert!(validate_publish(&request).is_err());
        request.target = NotificationTarget::Push;
        assert!(validate_publish(&request).is_err());
    }

    #[test]
    fn notification_expiry_defaults_to_one_day_and_is_bounded() {
        let occurred_at = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            effective_notification_expiry(occurred_at, None),
            occurred_at + chrono::Duration::hours(24)
        );
        let mut request = fixture();
        request.occurred_at = Some(occurred_at);
        request.expires_at = Some(occurred_at + chrono::Duration::days(8));
        assert!(validate_publish(&request).is_err());
        request.expires_at = Some(occurred_at + chrono::Duration::days(7));
        assert!(validate_publish(&request).is_ok());
    }

    #[test]
    fn publish_normalization_makes_equivalent_retries_hash_identically() {
        let expected = normalize_publish(fixture());
        let mut padded = fixture();
        padded.occurred_at = expected.occurred_at;
        padded.event_key = format!("  {}  ", padded.event_key);
        padded.correlation_id = format!("  {}  ", padded.correlation_id);
        padded.kind = format!(" {} ", padded.kind);
        padded.importance = format!(" {} ", padded.importance);
        padded.title = format!("  {}  ", padded.title);
        padded.body = format!("\n{}\n", padded.body);
        if let NotificationTarget::Briefing { date, edition, .. } = &mut padded.target {
            *date = format!(" {date} ");
            *edition = format!(" {edition} ");
        }
        let padded = normalize_publish(padded);
        assert_eq!(
            canonical_request_hash(&expected).unwrap(),
            canonical_request_hash(&padded).unwrap()
        );
    }

    #[test]
    fn unread_list_filter_excludes_expired_notifications_like_the_count() {
        let sql = LIST_NOTIFICATIONS_SQL
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(sql.contains(
            "state.opened_at IS NULL AND (notification.expires_at IS NULL OR notification.expires_at > clock_timestamp())"
        ));
    }

    #[test]
    fn device_tokens_are_encrypted_and_round_trip() {
        let token = "ab".repeat(32);
        let key = [7_u8; 32];
        let aad = b"installation-context";
        let (ciphertext, nonce) = encrypt_device_token(&key, aad, &token).unwrap();
        assert_ne!(ciphertext, token.as_bytes());
        assert_eq!(
            decrypt_device_token(&key, aad, &ciphertext, &nonce).unwrap(),
            token
        );
        assert!(decrypt_device_token(&key, b"other-installation", &ciphertext, &nonce).is_err());
    }

    #[test]
    fn news_push_payload_contains_only_generic_copy_and_opaque_refs() {
        let mut delivery = ClaimedDelivery {
            id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            notification_id: Uuid::now_v7(),
            installation_id: Uuid::now_v7(),
            client_installation_id: Uuid::now_v7(),
            attempt_number: 1,
            budget_attempt_count: 1,
            max_attempts: 8,
            kind: "news_alert".to_owned(),
            body: "Private news detail".to_owned(),
            target: json!({"type":"notification"}),
            environment: "development".to_owned(),
            app_id: "com.rourkem.brunn".to_owned(),
            token_ciphertext: Vec::new(),
            token_nonce: Vec::new(),
            expires_at: None,
        };
        let target = resolve_push_target(&delivery.target);
        let payload = push_payload(&delivery, &target);
        assert_eq!(payload["schema"], "brunn-push@v1");
        assert_eq!(payload["aps"]["alert"]["title"], "Brunn");
        assert_eq!(
            payload["aps"]["alert"]["body"],
            "A new Brunn alert is available."
        );
        assert!(!payload.to_string().contains("Private news detail"));
        assert!(payload.get("title").is_none());
        assert!(payload.get("body").is_none());
        assert!(
            payload["brunn_route"]
                .as_str()
                .unwrap()
                .starts_with("brunn://notification/")
        );
        let keys: std::collections::BTreeSet<_> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "aps",
                "delivery_ref",
                "notification_ref",
                "schema",
                "brunn_route",
            ])
        );
        delivery.kind = "location_heartbeat".to_owned();
        delivery.target = json!({"type":"push"});
        let target = resolve_push_target(&delivery.target);
        assert_eq!(target, ResolvedPushTarget::Ordinary);
        let payload = push_payload(&delivery, &target);
        assert_eq!(payload["aps"], json!({"content-available": 1}));
        assert_eq!(payload["kind"], "location_heartbeat");
        assert!(payload.get("brunn_route").is_none());
        assert_eq!(push_collapse_id(&delivery, &target), "location-heartbeat");
    }

    #[test]
    fn operational_push_payload_previews_alert_text_with_a_bounded_size() {
        let alert_text = "Storage on Nyx is above the operational threshold.";
        let mut delivery = ClaimedDelivery {
            id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            notification_id: Uuid::now_v7(),
            installation_id: Uuid::now_v7(),
            client_installation_id: Uuid::now_v7(),
            attempt_number: 1,
            budget_attempt_count: 1,
            max_attempts: 8,
            kind: "operational".to_owned(),
            body: alert_text.to_owned(),
            target: json!({"type":"notification"}),
            environment: "development".to_owned(),
            app_id: "com.rourkem.brunn".to_owned(),
            token_ciphertext: Vec::new(),
            token_nonce: Vec::new(),
            expires_at: None,
        };
        let target = resolve_push_target(&delivery.target);
        let payload = push_payload(&delivery, &target);
        assert_eq!(payload["aps"]["alert"]["body"], alert_text);

        delivery.body = "🚨".repeat(20_000);
        let payload = push_payload(&delivery, &target);
        let preview = payload["aps"]["alert"]["body"].as_str().unwrap();
        assert_eq!(preview.chars().count(), APNS_ALERT_PREVIEW_MAX_CHARS);
        assert!(preview.ends_with('…'));
        assert!(serde_json::to_vec(&payload).unwrap().len() < 4_096);
    }

    #[test]
    fn task_push_uses_the_exact_typed_uuidv7_route() {
        let task_id = Uuid::now_v7();
        let delivery = ClaimedDelivery {
            id: Uuid::now_v7(),
            user_id: Uuid::now_v7(),
            notification_id: Uuid::now_v7(),
            installation_id: Uuid::now_v7(),
            client_installation_id: Uuid::now_v7(),
            attempt_number: 1,
            budget_attempt_count: 1,
            max_attempts: 8,
            kind: "task_guard".to_owned(),
            body: "Private deadline detail".to_owned(),
            target: json!({"type":"task","task_ref":task_id.to_string()}),
            environment: "development".to_owned(),
            app_id: "com.rourkem.brunn".to_owned(),
            token_ciphertext: Vec::new(),
            token_nonce: Vec::new(),
            expires_at: None,
        };
        let target = resolve_push_target(&delivery.target);
        let payload = push_payload(&delivery, &target);
        assert_eq!(payload["brunn_route"], format!("brunn://task/{task_id}"));
        assert_eq!(
            payload["aps"]["alert"]["body"],
            "A new Brunn alert is available."
        );
        assert!(!payload.to_string().contains("Private deadline detail"));

        let mut request = fixture();
        request.target = NotificationTarget::Task {
            task_ref: Uuid::new_v4().to_string(),
        };
        assert!(validate_publish(&request).is_err());
        request.target = NotificationTarget::Task {
            task_ref: format!("task:{task_id}"),
        };
        assert!(validate_publish(&request).is_err());
        request.target = NotificationTarget::Task {
            task_ref: task_id.to_string().to_uppercase(),
        };
        assert!(validate_publish(&request).is_err());
        request.target = NotificationTarget::Task {
            task_ref: task_id.to_string(),
        };
        assert!(validate_publish(&request).is_err());
    }

    #[test]
    fn validation_rejects_private_or_malformed_installation_contracts() {
        let request = InstallationRequest {
            platform: "ios".to_owned(),
            environment: "development".to_owned(),
            app_id: "com.rourkem.brunn".to_owned(),
            device_token: "ab".repeat(32),
            preview: "generic".to_owned(),
            enabled: true,
        };
        assert!(validate_installation(&request).is_ok());
        let mut invalid = request;
        invalid.preview = "full".to_owned();
        assert!(validate_installation(&invalid).is_err());
    }

    #[test]
    fn apns_headers_are_stable_and_expiry_aware() {
        let id = Uuid::parse_str("019fc000-0000-7000-8000-000000000001").unwrap();
        let expiration = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(apns_id_header(id), "019fc000-0000-7000-8000-000000000001");
        assert_eq!(
            apns_collapse_id(id),
            "notification-019fc000000070008000000000000001"
        );
        assert_eq!(apns_expiration_header(Some(expiration)), "1785758400");
        assert_eq!(apns_expiration_header(None), "0");
    }

    #[test]
    fn apns_failure_classification_preserves_transport_truth() {
        let gone = classify_apns_failure(410, "Unregistered", None, None);
        assert!(gone.invalidate_token);
        assert!(!gone.retryable);

        for reason in ["BadDeviceToken", "DeviceTokenNotForTopic"] {
            let failure = classify_apns_failure(400, reason, None, None);
            assert!(failure.invalidate_token, "{reason}");
            assert!(!failure.retryable, "{reason}");
        }

        let throttled = classify_apns_failure(429, "TooManyRequests", None, Some(90));
        assert!(throttled.retryable);
        assert!(!throttled.invalidate_token);
        assert_eq!(throttled.retry_after_seconds, Some(90));

        for reason in [
            "BadCertificate",
            "BadCertificateEnvironment",
            "BadTopic",
            "ExpiredProviderToken",
            "Forbidden",
            "InvalidProviderToken",
            "MissingProviderToken",
            "TooManyProviderTokenUpdates",
        ] {
            let failure = classify_apns_failure(403, reason, None, None);
            assert!(failure.retryable, "{reason}");
            assert!(failure.provider_blocked, "{reason}");
            assert!(!failure.invalidate_token, "{reason}");
        }

        let permanent = classify_apns_failure(400, "PayloadEmpty", None, None);
        assert!(!permanent.retryable);
        assert!(!permanent.provider_blocked);
        assert!(!permanent.invalidate_token);
    }

    #[test]
    fn apns_bearer_cache_expires_before_apples_hour_limit() {
        let issued_at = Utc::now();
        assert!(bearer_is_fresh(
            issued_at,
            issued_at + chrono::Duration::minutes(49)
        ));
        assert!(!bearer_is_fresh(
            issued_at,
            issued_at + chrono::Duration::minutes(50)
        ));
    }

    #[test]
    fn retry_delay_honors_provider_guidance_and_caps_abuse() {
        assert_eq!(retry_delay_seconds(1, None), 2);
        assert_eq!(retry_delay_seconds(2, Some(90)), 90);
        assert_eq!(retry_delay_seconds(20, Some(50_000)), 3_600);
        assert_eq!(provider_block_delay_seconds(None), 60);
        assert_eq!(provider_block_delay_seconds(Some(300)), 300);
        assert_eq!(provider_block_delay_seconds(Some(50_000)), 3_600);
    }
}

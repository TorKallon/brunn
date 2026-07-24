use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PageCursor {
    pub user_id: Uuid,
    pub resource: String,
    pub filter_hash: String,
    pub sort_time: DateTime<Utc>,
    pub sort_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

pub fn issue(
    secret: &str,
    user_id: Uuid,
    resource: &str,
    filter_hash: &str,
    sort_time: DateTime<Utc>,
    sort_id: Uuid,
) -> ApiResult<String> {
    encode(
        secret,
        &PageCursor {
            user_id,
            resource: resource.to_owned(),
            filter_hash: filter_hash.to_owned(),
            sort_time,
            sort_id,
            expires_at: Utc::now() + Duration::days(7),
        },
    )
}

pub fn decode(
    secret: &str,
    token: &str,
    user_id: Uuid,
    resource: &str,
    filter_hash: &str,
) -> ApiResult<PageCursor> {
    let (payload, signature) = token
        .split_once('.')
        .ok_or_else(|| invalid_cursor("cursor is malformed"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor("cursor payload is invalid"))?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid_cursor("cursor signature is invalid"))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|error| ApiError::Internal(format!("invalid pagination key: {error}")))?;
    mac.update(&payload);
    mac.verify_slice(&signature)
        .map_err(|_| invalid_cursor("cursor signature did not verify"))?;
    let cursor: PageCursor =
        serde_json::from_slice(&payload).map_err(|_| invalid_cursor("cursor is invalid"))?;
    if cursor.expires_at < Utc::now()
        || cursor.user_id != user_id
        || cursor.resource != resource
        || cursor.filter_hash != filter_hash
    {
        return Err(invalid_cursor(
            "cursor is expired or belongs to different filters",
        ));
    }
    Ok(cursor)
}

fn encode(secret: &str, cursor: &PageCursor) -> ApiResult<String> {
    let payload = serde_json::to_vec(cursor)?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|error| ApiError::Internal(format!("invalid pagination key: {error}")))?;
    mac.update(&payload);
    let signature = mac.finalize().into_bytes();
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn invalid_cursor(message: &str) -> ApiError {
    ApiError::public(
        http::StatusCode::BAD_REQUEST,
        "invalid_cursor",
        message.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_cursors_are_user_filter_and_signature_bound() {
        let user_id = Uuid::now_v7();
        let sort_id = Uuid::now_v7();
        let token = issue(
            "pagination secret long enough for tests",
            user_id,
            "sessions",
            "filters-a",
            Utc::now(),
            sort_id,
        )
        .unwrap();
        let decoded = decode(
            "pagination secret long enough for tests",
            &token,
            user_id,
            "sessions",
            "filters-a",
        )
        .unwrap();
        assert_eq!(decoded.sort_id, sort_id);
        assert!(
            decode(
                "pagination secret long enough for tests",
                &token,
                Uuid::now_v7(),
                "sessions",
                "filters-a"
            )
            .is_err()
        );
        assert!(
            decode(
                "pagination secret long enough for tests",
                &token,
                user_id,
                "sessions",
                "filters-b"
            )
            .is_err()
        );
        assert!(
            decode(
                "pagination secret long enough for tests",
                &format!("{token}x"),
                user_id,
                "sessions",
                "filters-a"
            )
            .is_err()
        );
    }
}

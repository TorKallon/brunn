use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{ApiError, ApiResult};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct ContinuationCodec {
    secret: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Continuation {
    pub session_id: String,
    pub corpus_revision: String,
    pub operation: String,
    pub request_hash: String,
    pub item_id: String,
    pub locator: serde_json::Value,
    pub sort_key: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl ContinuationCodec {
    pub fn new(secret: impl AsRef<[u8]>) -> Self {
        Self {
            secret: secret.as_ref().to_vec(),
        }
    }

    pub fn issue(
        &self,
        session_id: String,
        corpus_revision: String,
        operation: String,
        request_hash: String,
        item_id: String,
        locator: serde_json::Value,
        sort_key: Option<String>,
    ) -> ApiResult<String> {
        self.encode(&Continuation {
            session_id,
            corpus_revision,
            operation,
            request_hash,
            item_id,
            locator,
            sort_key,
            expires_at: Utc::now() + Duration::hours(24),
        })
    }

    pub fn encode(&self, continuation: &Continuation) -> ApiResult<String> {
        let payload = serde_json::to_vec(continuation)?;
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|error| ApiError::Internal(format!("invalid continuation key: {error}")))?;
        mac.update(&payload);
        let signature = mac.finalize().into_bytes();
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    pub fn decode(
        &self,
        token: &str,
        expected_session: &str,
        expected_revision: &str,
        expected_operation: &str,
    ) -> ApiResult<Continuation> {
        let (payload, signature) = token.split_once('.').ok_or_else(|| {
            ApiError::conflict(
                "continuation_mismatch",
                "continuation token is malformed",
                serde_json::json!({}),
            )
        })?;
        let payload = URL_SAFE_NO_PAD.decode(payload).map_err(|_| {
            ApiError::conflict(
                "continuation_mismatch",
                "continuation payload is invalid",
                serde_json::json!({}),
            )
        })?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| {
            ApiError::conflict(
                "continuation_mismatch",
                "continuation signature is invalid",
                serde_json::json!({}),
            )
        })?;
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|error| ApiError::Internal(format!("invalid continuation key: {error}")))?;
        mac.update(&payload);
        mac.verify_slice(&signature).map_err(|_| {
            ApiError::conflict(
                "continuation_mismatch",
                "continuation signature did not verify",
                serde_json::json!({}),
            )
        })?;
        let continuation: Continuation = serde_json::from_slice(&payload)?;
        if continuation.expires_at < Utc::now()
            || continuation.session_id != expected_session
            || continuation.corpus_revision != expected_revision
            || continuation.operation != expected_operation
        {
            return Err(ApiError::conflict(
                "continuation_mismatch",
                "continuation is expired or belongs to another pinned operation",
                serde_json::json!({
                    "expected_session": expected_session,
                    "expected_revision": expected_revision,
                    "expected_operation": expected_operation
                }),
            ));
        }
        Ok(continuation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuations_are_revision_pinned_and_tamper_evident() {
        let codec = ContinuationCodec::new("a secret long enough for tests");
        let token = codec
            .issue(
                "session:1".to_owned(),
                "revision:1".to_owned(),
                "read".to_owned(),
                "hash".to_owned(),
                "item".to_owned(),
                serde_json::json!({"line": 20}),
                None,
            )
            .unwrap();
        assert!(
            codec
                .decode(&token, "session:1", "revision:1", "read")
                .is_ok()
        );
        assert!(
            codec
                .decode(&token, "session:1", "revision:2", "read")
                .is_err()
        );
        let tampered = format!("{}x", token);
        assert!(
            codec
                .decode(&tampered, "session:1", "revision:1", "read")
                .is_err()
        );
    }
}

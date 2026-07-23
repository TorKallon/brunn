use std::{collections::HashSet, str::FromStr};

use axum::{
    extract::{Request, State},
    http::header::AUTHORIZATION,
    middleware::Next,
    response::Response,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    db::AppState,
    error::{ApiError, ApiResult},
    models::{Capability, CredentialId, UserId},
};

#[derive(Clone, Debug, Serialize)]
pub struct AuthContext {
    pub credential_id: CredentialId,
    pub user_id: UserId,
    pub capabilities: HashSet<String>,
    pub scope_refs: Vec<String>,
    pub read_only: bool,
}

impl AuthContext {
    pub fn can(&self, capability: Capability) -> bool {
        self.capabilities.contains(capability.as_str())
    }

    pub fn require(&self, capability: Capability) -> ApiResult<()> {
        if self.can(capability) {
            Ok(())
        } else {
            Err(ApiError::capability(capability.as_str()))
        }
    }

    pub fn capability_guc(&self) -> String {
        let mut values: Vec<_> = self.capabilities.iter().cloned().collect();
        values.sort();
        values.join(",")
    }

    pub fn scope_guc(&self) -> String {
        self.scope_refs.join(",")
    }
}

pub async fn middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(ApiError::unauthenticated)?;
    let auth = authenticate(&state, token).await?;
    request.extensions_mut().insert(auth);
    Ok(next.run(request).await)
}

pub async fn authenticate(state: &AppState, token: &str) -> ApiResult<AuthContext> {
    let token_hash = hash_token(token);
    let row = sqlx::query(
        r#"
        SELECT credential_id, user_id, capabilities, scope_refs
        FROM straylight_auth.authenticate_credential($1)
        "#,
    )
    .bind(token_hash)
    .fetch_optional(&state.auth_pool)
    .await?
    .ok_or_else(ApiError::unauthenticated)?;

    let capabilities: Vec<String> = row.try_get("capabilities")?;
    let scope_refs: Vec<String> = row.try_get("scope_refs")?;
    let capabilities: HashSet<_> = capabilities.into_iter().collect();
    let read_only = !capabilities.contains(Capability::Save.as_str())
        && !capabilities.contains(Capability::Checkpoint.as_str());
    Ok(AuthContext {
        credential_id: CredentialId(row.try_get::<Uuid, _>("credential_id")?),
        user_id: UserId(row.try_get::<Uuid, _>("user_id")?),
        capabilities,
        scope_refs,
        read_only,
    })
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn parse_capability(value: &str) -> Option<Capability> {
    Capability::from_str(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_does_not_store_bearer_secret() {
        let hash = hash_token("straylight-test-secret");
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("secret"));
    }
}

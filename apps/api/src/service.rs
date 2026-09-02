use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{
    account_service, admin_service,
    auth::AuthContext,
    control_service,
    db::AppState,
    error::ApiResult,
    foreground_latency::ForegroundLatencySnapshot,
    models::{Capability, ListQuery},
};

pub async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "brunn",
        "version": env!("CARGO_PKG_VERSION"),
        "build_revision": build_revision()
    }))
}

pub async fn foreground_latency(State(state): State<AppState>) -> Json<ForegroundLatencySnapshot> {
    Json(state.foreground_latency.snapshot())
}

pub async fn ready(State(state): State<AppState>) -> Response {
    let timeout = state.config.readiness_timeout;
    let (database, object_store) = tokio::join!(
        tokio::time::timeout(
            timeout,
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.auth_pool)
        ),
        tokio::time::timeout(timeout, state.object_store.health_check())
    );
    let database_ready = matches!(database, Ok(Ok(1)));
    let object_store_ready = matches!(object_store, Ok(Ok(())));
    let embeddings_ready = !state.embedder.is_degraded();
    // Embeddings accelerate retrieval and all foreground read/write paths have
    // exact+lexical fallbacks. Keep their state visible, but never make an
    // optional provider outage remove an otherwise healthy API replica from
    // service or block a deployment.
    let ready = readiness_is_available(database_ready, object_store_ready);
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "status": if ready { "ready" } else { "unavailable" },
            "dependencies": {
                "database": if database_ready { "ready" } else { "unavailable" },
                "object_store": if object_store_ready { "ready" } else { "unavailable" },
                "embeddings": if embeddings_ready { "ready" } else { "degraded" }
            },
            "embedding_provider": state.embedder.provider(),
            "embedding_model": state.embedder.model(),
            "runtime_features": runtime_features(&state),
            "build_revision": build_revision()
        })),
    )
        .into_response()
}

fn readiness_is_available(database_ready: bool, object_store_ready: bool) -> bool {
    database_ready && object_store_ready
}

pub async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {"title": "Brunn API", "version": env!("CARGO_PKG_VERSION")},
        "servers": [{"url": "/"}],
        "paths": {
            "/v1/memory/open": {"post": {"operationId": "memoryOpen"}},
            "/v1/memory/query": {"post": {"operationId": "memoryQuery"}},
            "/v1/memory/read": {"post": {"operationId": "memoryRead"}},
            "/v1/memory/compute": {"post": {"operationId": "memoryCompute"}},
            "/v1/memory/verify": {"post": {"operationId": "memoryVerify"}},
            "/v1/memory/capture": {"post": {"operationId": "memoryCapture"}},
            "/v1/memory/save": {"post": {"operationId": "memorySave"}},
            "/v1/memory/stage": {"post": {"operationId": "memoryStage"}},
            "/v1/memory/checkpoint": {"post": {"operationId": "memoryCheckpoint"}},
            "/v1/assets": {"get": {"operationId": "listAssets"}},
            "/v1/assets/{asset_ref}": {"get": {"operationId": "getAsset"}},
            "/v1/assets/{asset_ref}/versions/{version}/content": {
                "get": {"operationId": "downloadAssetVersion"}
            },
            "/v1/asset-uploads": {"post": {"operationId": "createAssetUpload"}},
            "/v1/asset-uploads/{upload_ref}": {
                "get": {"operationId": "getAssetUpload"},
                "delete": {"operationId": "abortAssetUpload"}
            },
            "/v1/asset-uploads/{upload_ref}/parts/{part_number}": {
                "put": {"operationId": "putAssetUploadPart"}
            },
            "/v1/asset-uploads/{upload_ref}/complete": {
                "post": {"operationId": "completeAssetUpload"}
            },
            "/v1/vault/manifest": {"get": {"operationId": "getVaultManifest"}},
            "/v1/vault/assets/{asset_ref}/versions/{version}/content": {
                "get": {"operationId": "downloadVaultAssetVersion"}
            },
            "/v1/stages/{stage_ref}": {"get": {"operationId": "getStage"}},
            "/v1/usage": {"get": {"operationId": "getDataUsage"}},
            "/v1/workspace/dashboard": {"get": {"operationId": "getWorkspaceDashboard"}},
            "/v1/deletions/{deletion_ref}": {"get": {"operationId": "getDeletion"}}
        },
        "components": {"securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}}},
        "security": [{"bearer": []}]
    }))
}

pub async fn me(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    Ok(Json(control_service::me(&state, &auth).await?))
}

pub async fn status(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::Status)?;
    Ok(Json(json!({
        "status": "ready",
        "build_revision": build_revision(),
        "corpus_revision": Option::<String>::None,
        "revision_sequence": Option::<i64>::None,
        "read_only": auth.read_only,
        "feature_flags": runtime_feature_flags(&state),
        "embeddings": {
            "provider": state.embedder.provider(),
            "model": state.embedder.model(),
            "dimensions": state.embedder.dimensions(),
            "status": if state.embedder.is_degraded() { "degraded" } else { "ready" }
        },
        "runtime_features": runtime_features(&state),
        "semantic_runtime": state.semantic_runtime.snapshot()
    })))
}

fn runtime_feature_flags(state: &AppState) -> Value {
    json!({
        "allow_degraded_embeddings": state.config.allow_degraded_embeddings,
        "embed_cache": state.config.embed_cache,
        "embedding_backfill_guard": state.config.embedding_backfill_guard,
        "embedding_backfill_foreground_status_url_configured":
            state.config.embedding_backfill_foreground_status_url.is_some(),
        "intention_ledger": state.config.intention_ledger,
        "lexical_single_scan": state.config.lexical_single_scan,
        "location_pings_enabled": state.config.location_pings_enabled,
        "location_presence_in_open": state.config.location_presence_in_open,
        "messaging_enabled": state.config.messaging_enabled,
        "observability_timings_ms": state.config.observability_timings_ms,
        "read_path_roundtrip_v1": state.config.read_path_roundtrip_v1,
        "resume_deltas": state.config.resume_deltas,
        "search_char_cap": state.config.search_char_cap,
        "search_fair_share": state.config.search_fair_share,
        "search_top1_hydration": state.config.search_top1_hydration,
        "semantic_lane": state.config.semantic_lane,
        "supersession_demotion": state.config.supersession_demotion,
        "verbatim_spans": state.config.verbatim_spans
    })
}

fn runtime_features(state: &AppState) -> Value {
    json!({
        "allow_degraded_embeddings": state.config.allow_degraded_embeddings,
        "embed_cache": state.config.embed_cache,
        "semantic_lane": state.config.semantic_lane,
        "semantic_deadline_ms": state.config.semantic_deadline.map(|value| {
            u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
        }),
        "semantic_query_provider_timeout_ms": u64::try_from(
            state.config.semantic_query_provider_timeout.as_millis()
        ).unwrap_or(u64::MAX),
        "semantic_query_concurrency": state.config.semantic_query_concurrency,
        "embedding_backfill_guard": state.config.embedding_backfill_guard,
        "embedding_backfill_batch_chunks": state.config.embedding_backfill_batch_chunks,
        "embedding_backfill_inter_batch_ms": u64::try_from(
            state.config.embedding_backfill_inter_batch_delay.as_millis()
        ).unwrap_or(u64::MAX),
        "embedding_backfill_open_p95_limit_ms":
            state.config.embedding_backfill_open_p95_limit_ms,
        "embedding_backfill_search_p95_limit_ms":
            state.config.embedding_backfill_search_p95_limit_ms,
        "embedding_backfill_foreground_status_url_configured":
            state.config.embedding_backfill_foreground_status_url.is_some(),
        "embedding_backfill_foreground_status_timeout_ms": u64::try_from(
            state.config.embedding_backfill_foreground_status_timeout.as_millis()
        ).unwrap_or(u64::MAX),
        "intention_ledger": state.config.intention_ledger,
        "lexical_single_scan": state.config.lexical_single_scan,
        "location_pings_enabled": state.config.location_pings_enabled,
        "location_presence_in_open": state.config.location_presence_in_open,
        "materialize_token_budget": state.config.materialize_token_budget,
        "observability_timings_ms": state.config.observability_timings_ms,
        "read_path_roundtrip_v1": state.config.read_path_roundtrip_v1,
        "resume_deltas": state.config.resume_deltas,
        "search_char_cap": state.config.search_char_cap,
        "search_fair_share": state.config.search_fair_share,
        "search_section_demotion_top_n": state.config.search_section_demotion_top_n,
        "search_top1_hydration": state.config.search_top1_hydration,
        "supersession_demotion": state.config.supersession_demotion,
        "supersession_demotion_weight": state.config.supersession_demotion_weight,
        "verbatim_spans": state.config.verbatim_spans
    })
}

fn build_revision() -> &'static str {
    option_env!("BRUNN_BUILD_REVISION").unwrap_or("unknown")
}

pub async fn list_credentials(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(_query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::list_credentials(&state, &auth).await?,
    ))
}

pub async fn create_credential(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<Value>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::create_credential(&state, &auth, &request).await?,
    ))
}

pub async fn admin_provision_user(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<admin_service::ProvisionUserRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        admin_service::provision_user(&state, &auth, request).await?,
    ))
}

pub async fn admin_recover_credential(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(user_ref): Path<String>,
    Json(request): Json<admin_service::RecoverCredentialRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        admin_service::recover_credential(&state, &auth, &user_ref, request).await?,
    ))
}

pub async fn revoke_credential(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(credential_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::revoke_credential(&state, &auth, &credential_ref).await?,
    ))
}

pub async fn request_account_export(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    Ok(Json(account_service::request_export(&state, &auth).await?))
}

pub async fn list_account_exports(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    Ok(Json(account_service::list_exports(&state, &auth).await?))
}

pub async fn get_account_export(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(export_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        account_service::get_export(&state, &auth, &export_ref).await?,
    ))
}

pub async fn download_account_export(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(export_ref): Path<String>,
) -> ApiResult<Response> {
    account_service::download_export(&state, &auth, &export_ref).await
}

pub async fn delete_account_export(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(export_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        account_service::delete_export(&state, &auth, &export_ref).await?,
    ))
}

pub async fn request_account_deletion(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<account_service::DeleteAccountRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        account_service::request_deletion(&state, &auth, request).await?,
    ))
}

pub async fn get_latest_account_deletion(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        account_service::get_deletion(&state, &auth, None).await?,
    ))
}

pub async fn get_account_deletion(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(request_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        account_service::get_deletion(&state, &auth, Some(&request_ref)).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_durable_dependencies_but_not_optional_embeddings() {
        assert!(readiness_is_available(true, true));
        assert!(!readiness_is_available(false, true));
        assert!(!readiness_is_available(true, false));
    }
}

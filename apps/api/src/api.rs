use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue, Method, StatusCode},
    middleware,
    routing::{delete, get, post, put},
};
use tower::{ServiceBuilder, limit::ConcurrencyLimitLayer};
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, cors::CorsLayer,
    timeout::TimeoutLayer, trace::TraceLayer,
};

use crate::{
    auth, briefing_service, db::AppState, dreams, eval_service, request_context, service,
    simple_core, telemetry,
};

pub fn router(state: AppState) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    let allowed_origins: Vec<HeaderValue> = state
        .config
        .allowed_origins
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    let workspace_ordinary = Router::new()
        .route("/me", get(service::me))
        .route("/status", get(service::status))
        .route("/workspace/open", post(simple_core::open))
        .route("/workspace/search", post(simple_core::search))
        .route("/workspace/read", post(simple_core::read))
        .route(
            "/workspace/write",
            post(simple_core::write).layer(DefaultBodyLimit::max(5 * 1024 * 1024)),
        )
        .route(
            "/workspace/capture",
            post(simple_core::capture).layer(DefaultBodyLimit::max(5 * 1024 * 1024)),
        )
        .route(
            "/workspace/briefings/publish",
            post(briefing_service::publish).layer(DefaultBodyLimit::max(5 * 1024 * 1024)),
        )
        .route(
            "/workspace/briefings/dedupe-check",
            post(briefing_service::dedupe_check),
        )
        .route("/workspace/changes", get(simple_core::changes))
        .route("/workspace/checkpoint", post(simple_core::checkpoint))
        .route("/workspace/binaries", get(simple_core::list_binaries))
        .route("/workspace/manifest", get(simple_core::manifest))
        .route("/workspace/usage", get(simple_core::usage))
        .route("/workspace/jobs", get(simple_core::list_jobs))
        .route("/workspace/dreams", post(simple_core::queue_dream))
        .route(
            "/workspace/entries/{entry_ref}",
            delete(simple_core::delete_entry),
        )
        .route(
            "/workspace/binaries/{entry_ref}",
            get(simple_core::binary_metadata),
        )
        .route("/admin/users", post(service::admin_provision_user))
        .route(
            "/admin/users/{user_ref}/recover",
            post(service::admin_recover_credential),
        )
        .route(
            "/credentials",
            get(service::list_credentials).post(service::create_credential),
        )
        .route(
            "/credentials/{credential_ref}",
            delete(service::revoke_credential),
        );
    let legacy_ordinary = Router::new()
        .route("/memory/open", post(service::open))
        .route("/memory/query", post(service::query))
        .route("/memory/read", post(service::read))
        .route("/memory/compute", post(service::compute))
        .route("/memory/verify", post(service::verify))
        .route("/memory/capture", post(service::capture))
        .route("/memory/checkpoint", post(service::checkpoint))
        .route("/asset-uploads", post(service::create_asset_upload))
        .route(
            "/asset-uploads/{upload_ref}",
            get(service::get_asset_upload).delete(service::abort_asset_upload),
        )
        .route("/sessions", get(service::list_sessions))
        .route("/sessions/{session_id}", get(service::get_session))
        .route(
            "/sessions/{session_id}/refresh",
            post(service::refresh_session),
        )
        .route(
            "/checkpoints/{checkpoint_ref}",
            get(service::get_checkpoint),
        )
        .route("/objects", get(service::list_objects))
        .route("/objects/{object_ref}", get(service::get_object))
        .route("/sources", get(service::list_sources))
        .route("/sources/{source_ref}", get(service::get_source))
        .route(
            "/sources/{source_ref}/content",
            get(service::get_source_content),
        )
        .route("/assets", get(service::list_assets))
        .route("/assets/{asset_ref}", get(service::get_asset))
        .route("/vault/manifest", get(service::get_vault_manifest))
        .route("/stages/{stage_ref}", get(service::get_stage))
        .route("/audit", get(service::list_audit))
        .route("/usage", get(service::data_usage))
        .route("/deletions/{deletion_ref}", get(service::get_deletion))
        .route("/scopes", get(service::list_scopes))
        .route("/policies", get(service::list_policies))
        .route(
            "/account/exports",
            get(service::list_account_exports).post(service::request_account_export),
        )
        .route(
            "/account/exports/{export_ref}",
            get(service::get_account_export).delete(service::delete_account_export),
        )
        .route(
            "/account/deletion",
            get(service::get_latest_account_deletion).post(service::request_account_deletion),
        )
        .route(
            "/account/deletions/{request_ref}",
            get(service::get_account_deletion),
        )
        .merge(dreams::routes());
    let evaluation_ordinary = Router::new()
        .route(
            "/workspace/admin/eval/imports/{import_id}",
            get(simple_core::evaluation_status)
                .delete(simple_core::cleanup_evaluation),
        )
        .route(
            "/admin/eval/imports/{import_id}",
            get(eval_service::get_evaluation_import),
        );
    let mut ordinary = workspace_ordinary;
    if state.config.legacy_api_enabled {
        ordinary = ordinary.merge(legacy_ordinary);
    }
    if state.config.evaluation_api_enabled {
        ordinary = ordinary.merge(evaluation_ordinary);
    }
    let ordinary = ordinary.layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        state.config.request_timeout,
    ));
    let workspace_transfers = Router::new()
        .route(
            "/workspace/binaries",
            post(simple_core::upload_binary).layer(DefaultBodyLimit::max(72 * 1024 * 1024)),
        )
        .route(
            "/workspace/binaries/content",
            put(simple_core::upload_binary_stream).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/workspace/binaries/{entry_ref}/content",
            get(simple_core::fetch_binary),
        );
    let legacy_transfers = Router::new()
        .route("/memory/save", post(service::save))
        .route(
            "/memory/stage",
            post(service::stage).layer(DefaultBodyLimit::max(72 * 1024 * 1024)),
        )
        .route(
            "/asset-uploads/{upload_ref}/parts/{part_number}",
            put(service::put_asset_upload_part).layer(DefaultBodyLimit::max(9 * 1024 * 1024)),
        )
        .route(
            "/asset-uploads/{upload_ref}/complete",
            post(service::complete_asset_upload),
        )
        .route(
            "/assets/{asset_ref}/versions/{version}/content",
            get(service::get_asset_content),
        )
        .route(
            "/vault/assets/{asset_ref}/versions/{version}/content",
            get(service::get_vault_asset_content),
        )
        .route(
            "/account/exports/{export_ref}/content",
            get(service::download_account_export),
        );
    let evaluation_transfers = Router::new()
        .route(
            "/admin/eval/import",
            post(eval_service::import_evaluation).layer(DefaultBodyLimit::max(192 * 1024 * 1024)),
        )
        .route(
            "/workspace/admin/eval/import",
            post(simple_core::import_evaluation).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        );
    let mut transfers = workspace_transfers;
    if state.config.legacy_api_enabled {
        transfers = transfers.merge(legacy_transfers);
    }
    if state.config.evaluation_api_enabled {
        transfers = transfers.merge(evaluation_transfers);
    }
    let transfers = transfers
        .layer(ConcurrencyLimitLayer::new(
            state.config.max_concurrent_transfers,
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            state.config.transfer_timeout,
        ));
    let protected = ordinary
        .merge(transfers)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::middleware,
        ));

    Router::new()
        .route("/health", get(service::health))
        .route(
            "/health/foreground-latency",
            get(service::foreground_latency),
        )
        .route("/ready", get(service::ready))
        .route("/openapi.json", get(service::openapi))
        .nest("/v1", protected)
        .with_state(state.clone())
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(request_context::middleware))
                .layer(middleware::from_fn(telemetry::http_middleware))
                .layer(TraceLayer::new_for_http())
                .layer(CompressionLayer::new())
                .layer(CatchPanicLayer::new())
                .layer(
                    CorsLayer::new()
                        .allow_origin(allowed_origins)
                        .allow_headers([
                            http::header::AUTHORIZATION,
                            http::header::CONTENT_TYPE,
                            http::header::RANGE,
                            request_id.clone(),
                            HeaderName::from_static("x-carrystate-part-sha256"),
                        ])
                        .expose_headers([
                            request_id,
                            HeaderName::from_static("x-carrystate-sha256"),
                            HeaderName::from_static("x-carrystate-integrity"),
                            HeaderName::from_static("x-carrystate-asset-ref"),
                            HeaderName::from_static("x-carrystate-asset-version"),
                            http::header::ACCEPT_RANGES,
                            http::header::CONTENT_DISPOSITION,
                            http::header::CONTENT_RANGE,
                        ])
                        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE]),
                ),
        )
}

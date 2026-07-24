use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue, Method, StatusCode},
    middleware,
    routing::{delete, get, post},
};
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer, compression::CompressionLayer, cors::CorsLayer,
    timeout::TimeoutLayer, trace::TraceLayer,
};

use crate::{auth, db::AppState, dreams, eval_service, request_context, service, telemetry};

pub fn router(state: AppState) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    let allowed_origins: Vec<HeaderValue> = state
        .config
        .allowed_origins
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    let protected = Router::new()
        .route("/me", get(service::me))
        .route("/status", get(service::status))
        .route("/memory/open", post(service::open))
        .route("/memory/query", post(service::query))
        .route("/memory/read", post(service::read))
        .route("/memory/compute", post(service::compute))
        .route("/memory/verify", post(service::verify))
        .route("/memory/capture", post(service::capture))
        .route("/memory/save", post(service::save))
        .route(
            "/memory/stage",
            post(service::stage).layer(DefaultBodyLimit::max(72 * 1024 * 1024)),
        )
        .route("/memory/checkpoint", post(service::checkpoint))
        .route(
            "/admin/eval/import",
            post(eval_service::import_evaluation).layer(DefaultBodyLimit::max(192 * 1024 * 1024)),
        )
        .route(
            "/admin/eval/imports/{import_id}",
            get(eval_service::get_evaluation_import),
        )
        .route("/admin/users", post(service::admin_provision_user))
        .route(
            "/admin/users/{user_ref}/recover",
            post(service::admin_recover_credential),
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
        .route("/audit", get(service::list_audit))
        .route("/usage", get(service::data_usage))
        .route("/deletions/{deletion_ref}", get(service::get_deletion))
        .route("/scopes", get(service::list_scopes))
        .route("/policies", get(service::list_policies))
        .route(
            "/credentials",
            get(service::list_credentials).post(service::create_credential),
        )
        .route(
            "/credentials/{credential_ref}",
            delete(service::revoke_credential),
        )
        .route(
            "/account/exports",
            get(service::list_account_exports).post(service::request_account_export),
        )
        .route(
            "/account/exports/{export_ref}",
            get(service::get_account_export).delete(service::delete_account_export),
        )
        .route(
            "/account/exports/{export_ref}/content",
            get(service::download_account_export),
        )
        .route(
            "/account/deletion",
            get(service::get_latest_account_deletion).post(service::request_account_deletion),
        )
        .route(
            "/account/deletions/{request_ref}",
            get(service::get_account_deletion),
        )
        .merge(dreams::routes())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::middleware,
        ));

    Router::new()
        .route("/health", get(service::health))
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
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    state.config.request_timeout,
                ))
                .layer(CatchPanicLayer::new())
                .layer(
                    CorsLayer::new()
                        .allow_origin(allowed_origins)
                        .allow_headers([
                            http::header::AUTHORIZATION,
                            http::header::CONTENT_TYPE,
                            request_id.clone(),
                        ])
                        .expose_headers([request_id])
                        .allow_methods([Method::GET, Method::POST, Method::DELETE]),
                ),
        )
}

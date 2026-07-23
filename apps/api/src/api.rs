use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, Method, StatusCode},
    middleware,
    routing::{delete, get, post},
};
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::{auth, db::AppState, dreams, eval_service, service};

pub fn router(state: AppState) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
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
            post(service::stage).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
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
                .layer(SetRequestIdLayer::new(request_id.clone(), MakeRequestUuid))
                .layer(PropagateRequestIdLayer::new(request_id))
                .layer(TraceLayer::new_for_http())
                .layer(CompressionLayer::new())
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    state.config.request_timeout,
                ))
                .layer(CatchPanicLayer::new())
                .layer(
                    CorsLayer::new()
                        .allow_origin(Any)
                        .allow_headers(Any)
                        .allow_methods([Method::GET, Method::POST, Method::DELETE]),
                ),
        )
}

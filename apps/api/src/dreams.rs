use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde_json::Value;

use crate::{
    auth::AuthContext,
    db::AppState,
    dream_service::{self, DreamRollbackRequest, DreamSchedulerControlRequest},
    error::ApiResult,
    models::{DreamCreateRequest, DreamReviewRequest, ListQuery},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dreams", get(list).post(create))
        .route("/dreams/run-now", post(run_now))
        .route("/dreams/scheduler", get(scheduler).post(update_scheduler))
        .route("/dreams/{dream_id}", get(get_dream))
        .route("/dreams/{dream_id}/cancel", post(cancel))
        .route("/dreams/{dream_id}/review", post(review))
        .route("/dreams/{dream_id}/rollback", post(rollback))
}

async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(dream_service::list_jobs(&state, &auth, &query).await?))
}

async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DreamCreateRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::create_job(&state, &auth, request).await?,
    ))
}

async fn run_now(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DreamCreateRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::run_now_job(&state, &auth, request).await?,
    ))
}

async fn scheduler(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::list_scheduler_controls(&state, &auth, query.scope.as_deref()).await?,
    ))
}

async fn update_scheduler(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DreamSchedulerControlRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::update_scheduler_control(&state, &auth, request).await?,
    ))
}

async fn get_dream(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(dream_id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::get_job(&state, &auth, &dream_id).await?,
    ))
}

async fn cancel(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(dream_id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::cancel_job(&state, &auth, &dream_id).await?,
    ))
}

async fn review(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(dream_id): Path<String>,
    Json(request): Json<DreamReviewRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::review_job(&state, &auth, &dream_id, request).await?,
    ))
}

async fn rollback(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(dream_id): Path<String>,
    Json(request): Json<DreamRollbackRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        dream_service::rollback_job(&state, &auth, &dream_id, request).await?,
    ))
}

use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{
    auth::AuthContext,
    capture_service,
    continuation::ContinuationCodec,
    control_service,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{
        ApiEnvelope, Capability, CaptureRequest, CheckpointRequest, ComputeRequest, ListQuery,
        OpenRequest, QueryRequest, ReadRequest, ResponseStatus, SaveRequest, VerifyRequest,
    },
    read_service, session_service,
    write_service::{self, StageUpload},
};

pub async fn health() -> Json<Value> {
    Json(json!({"status": "ok", "service": "straylight", "version": env!("CARGO_PKG_VERSION")}))
}

pub async fn ready(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.auth_pool)
        .await?;
    Ok(Json(json!({
        "status": "ready",
        "database": "ready",
        "object_store": "ready",
        "embeddings": if state.embedder.is_degraded() { "degraded" } else { "ready" },
        "embedding_provider": state.embedder.provider(),
        "embedding_model": state.embedder.model()
    })))
}

pub async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {"title": "Straylight API", "version": env!("CARGO_PKG_VERSION")},
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
    let mut tx = state.begin_read(&auth).await?;
    let revision: Option<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT id, revision_number FROM straylight.corpus_revisions WHERE user_id = $1 ORDER BY revision_number DESC LIMIT 1",
    )
    .bind(auth.user_id.0)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "status": "ready",
        "corpus_revision": revision.as_ref().map(|value| format!("revision:{}", value.0)),
        "revision_sequence": revision.map(|value| value.1),
        "read_only": auth.read_only,
        "embeddings": {
            "provider": state.embedder.provider(),
            "model": state.embedder.model(),
            "dimensions": state.embedder.dimensions(),
            "status": if state.embedder.is_degraded() { "degraded" } else { "ready" }
        }
    })))
}

pub async fn open(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<OpenRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let session_id = session_service::provision(&state, &auth, &request).await?;
    let result = read_service::open(&state, &auth, &session_id, &request).await?;
    let mut data = session_service::project_response(
        &state,
        &auth,
        &session_id,
        "memory.open",
        "read",
        serde_json::to_value(&result)?,
    )
    .await?;
    let projected_conflicts = data
        .get("conflicts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let projected_gaps = data
        .get("gaps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let projected_ambiguities = data
        .get("ambiguities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    remove_envelope_duplicates(&mut data, true);
    let mut envelope = ApiEnvelope::complete(data);
    envelope.session_id = Some(result.session_id);
    envelope.corpus_revision = Some(result.corpus_revision);
    envelope.freshness = result.freshness;
    envelope.coverage = result.coverage;
    envelope.conflicts = projected_conflicts;
    envelope.gaps = projected_gaps;
    envelope.ambiguities = projected_ambiguities;
    Ok(Json(envelope))
}

pub async fn query(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<QueryRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let result = read_service::query(&state, &auth, &request).await?;
    let mut data = serde_json::to_value(&result)?;
    if result.items.len() == 1
        && let Some(object) = data.as_object_mut()
    {
        let flattened: Vec<Value> = result
            .items
            .iter()
            .flat_map(|item| {
                item.results
                    .iter()
                    .filter_map(|candidate| serde_json::to_value(candidate).ok())
            })
            .collect();
        object.insert("results".to_owned(), Value::Array(flattened));
    }
    let mut data = session_service::project_response(
        &state,
        &auth,
        &request.session_id,
        "memory.query",
        "read",
        data,
    )
    .await?;
    remove_envelope_duplicates(&mut data, false);
    let mut envelope = ApiEnvelope::complete(data);
    envelope.session_id = Some(result.session_id);
    envelope.corpus_revision = Some(result.corpus_revision);
    envelope.status = result.status;
    envelope.freshness = result.freshness;
    envelope.coverage = result.coverage;
    Ok(Json(envelope))
}

pub async fn read(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ReadRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let codec = ContinuationCodec::new(&state.config.continuation_secret);
    let result = read_service::read(&state, &auth, &codec, &request).await?;
    let mut data = session_service::project_response(
        &state,
        &auth,
        &request.session_id,
        "memory.read",
        "read",
        serde_json::to_value(&result)?,
    )
    .await?;
    remove_envelope_duplicates(&mut data, false);
    let mut envelope = ApiEnvelope::complete(data);
    envelope.session_id = Some(result.session_id);
    envelope.corpus_revision = Some(result.corpus_revision);
    envelope.status = result.status;
    envelope.freshness = result.freshness;
    envelope.coverage = result.coverage;
    Ok(Json(envelope))
}

pub async fn compute(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ComputeRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let codec = ContinuationCodec::new(&state.config.continuation_secret);
    let result = read_service::compute(&state, &auth, &codec, &request).await?;
    let mut data = session_service::project_response(
        &state,
        &auth,
        &request.session_id,
        "memory.compute",
        "derive",
        serde_json::to_value(&result)?,
    )
    .await?;
    remove_envelope_duplicates(&mut data, false);
    let mut envelope = ApiEnvelope::complete(data);
    envelope.session_id = Some(result.session_id);
    envelope.corpus_revision = Some(result.corpus_revision);
    envelope.status = result.status;
    envelope.freshness = result.freshness;
    Ok(Json(envelope))
}

pub async fn verify(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<VerifyRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let result = read_service::verify(&state, &auth, &request).await?;
    let mut data = serde_json::to_value(&result)?;
    if let Some(object) = data.as_object_mut() {
        object.insert("results".to_owned(), serde_json::to_value(&result.claims)?);
    }
    let mut data = session_service::project_response(
        &state,
        &auth,
        &request.session_id,
        "memory.verify",
        "derive",
        data,
    )
    .await?;
    remove_envelope_duplicates(&mut data, false);
    let mut envelope = ApiEnvelope::complete(data);
    envelope.session_id = Some(result.session_id);
    envelope.corpus_revision = Some(result.corpus_revision);
    envelope.status = result.status;
    envelope.freshness = result.freshness;
    envelope.coverage = result.coverage;
    Ok(Json(envelope))
}

pub async fn save(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<SaveRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let mut result = write_service::save(&state, &auth, request).await?;
    add_write_receipt_aliases(&mut result);
    let mut envelope = ApiEnvelope::complete(result.clone());
    envelope.status = ResponseStatus::Committed;
    envelope.corpus_revision = result
        .get("corpus_revision")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok(Json(envelope))
}

pub async fn capture(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CaptureRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let mut outcome = capture_service::capture(&state, &auth, request).await?;
    if outcome.status == ResponseStatus::Committed {
        add_write_receipt_aliases(&mut outcome.data);
    }
    let mut envelope = ApiEnvelope::complete(outcome.data);
    envelope.status = outcome.status;
    envelope.corpus_revision = outcome.corpus_revision;
    Ok(Json(envelope))
}

fn add_write_receipt_aliases(receipt: &mut Value) {
    let Some(object) = receipt.as_object_mut() else {
        return;
    };
    if !object.contains_key("id") {
        let id = object
            .get("receipt")
            .or_else(|| object.get("operation_id"))
            .cloned();
        if let Some(id) = id {
            object.insert("id".to_owned(), id);
        }
    }
    if let Some(revision) = object.get("corpus_revision").cloned() {
        object.insert("resulting_corpus_revision".to_owned(), revision);
    }
    if let Some(indexing) = object.get("search_status").cloned() {
        object.insert("indexing".to_owned(), indexing);
    }
    let items = object
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (alias, dispositions) in [
        ("created", &["created", "committed"][..]),
        ("revised", &["revised", "superseded", "retracted"][..]),
        ("deduplicated", &["deduplicated", "no_op"][..]),
        ("review_required", &["needs_review"][..]),
    ] {
        let selected: Vec<_> = items
            .iter()
            .filter(|item| {
                item.get("disposition")
                    .and_then(Value::as_str)
                    .is_some_and(|value| dispositions.contains(&value))
            })
            .map(|item| {
                json!({
                    "kind": item.get("kind").cloned().unwrap_or(Value::Null),
                    "id": item.get("ref").cloned().unwrap_or(Value::Null),
                    "version": item.get("resulting_version").cloned().unwrap_or(Value::Null)
                })
            })
            .collect();
        object.insert(alias.to_owned(), Value::Array(selected));
    }
}

fn remove_envelope_duplicates(data: &mut Value, include_diagnostics: bool) {
    let Some(object) = data.as_object_mut() else {
        return;
    };
    for field in [
        "session_id",
        "corpus_revision",
        "status",
        "freshness",
        "coverage",
    ] {
        object.remove(field);
    }
    if include_diagnostics {
        for field in ["conflicts", "gaps", "ambiguities"] {
            object.remove(field);
        }
    }
}

pub async fn checkpoint(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CheckpointRequest>,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    let session_id = request.session_id.clone();
    let mut result = write_service::checkpoint(&state, &auth, request).await?;
    add_write_receipt_aliases(&mut result);
    let checkpoint_id = result
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("kind").and_then(Value::as_str) == Some("checkpoint"))
        })
        .and_then(|item| item.get("ref"))
        .cloned();
    if let (Some(object), Some(checkpoint_id)) = (result.as_object_mut(), checkpoint_id) {
        object.insert("id".to_owned(), checkpoint_id.clone());
        object.insert("checkpoint_id".to_owned(), checkpoint_id);
        object.insert("session_id".to_owned(), Value::String(session_id.clone()));
    }
    let mut envelope = ApiEnvelope::complete(result.clone());
    envelope.status = ResponseStatus::Committed;
    envelope.session_id = Some(session_id);
    envelope.corpus_revision = result
        .get("corpus_revision")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok(Json(envelope))
}

pub async fn stage(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    mut multipart: Multipart,
) -> ApiResult<Json<ApiEnvelope<Value>>> {
    auth.require(Capability::Stage)?;
    let mut scope = None;
    let mut stable_import_id = None;
    let mut uploads = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::invalid(format!("invalid multipart stage: {error}")))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "scope" {
            scope = Some(
                field
                    .text()
                    .await
                    .map_err(|error| ApiError::invalid(format!("invalid stage scope: {error}")))?,
            );
        } else if name == "stable_import_id" {
            stable_import_id = Some(
                field
                    .text()
                    .await
                    .map_err(|error| ApiError::invalid(format!("invalid import ID: {error}")))?,
            );
        } else if name == "file" || field.file_name().is_some() {
            let path = field.file_name().unwrap_or("upload.bin").to_owned();
            let media_type = field.content_type().map(ToOwned::to_owned);
            let bytes = field.bytes().await.map_err(|error| {
                ApiError::invalid(format!("could not read staged file: {error}"))
            })?;
            uploads.push(StageUpload {
                path,
                media_type,
                bytes,
            });
        }
    }
    let scope = scope
        .or_else(|| auth.scope_refs.first().cloned())
        .ok_or_else(|| ApiError::invalid("stage requires scope"))?;
    let result =
        write_service::stage_uploads(&state, &auth, &scope, stable_import_id, uploads).await?;
    let stage_ref = result
        .get("stage_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::Internal("stage response lacks stage_ref".to_owned()))?
        .to_owned();
    let data = session_service::project_response(
        &state,
        &auth,
        &stage_ref,
        "memory.stage",
        "read",
        result,
    )
    .await?;
    let mut envelope = ApiEnvelope::complete(data);
    envelope.status = ResponseStatus::Complete;
    Ok(Json(envelope))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::list_sessions(&state, &auth, &query).await?,
    ))
}

pub async fn list_objects(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::list_objects(&state, &auth, &query).await?,
    ))
}

pub async fn list_sources(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::list_sources(&state, &auth, &query).await?,
    ))
}

pub async fn list_audit(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::list_audit(&state, &auth, &query).await?,
    ))
}

pub async fn get_deletion(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(deletion_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::get_deletion(&state, &auth, &deletion_ref).await?,
    ))
}

pub async fn list_scopes(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(_query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(control_service::list_scopes(&state, &auth).await?))
}

pub async fn list_policies(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(_query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(control_service::list_policies(&state, &auth).await?))
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

pub async fn get_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::get_session(&state, &auth, &session_id).await?,
    ))
}

pub async fn refresh_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let refreshed = session_service::refresh(&state, &auth, &session_id).await?;
    Ok(Json(
        control_service::get_session(&state, &auth, &refreshed).await?,
    ))
}

pub async fn get_checkpoint(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(checkpoint_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::get_checkpoint(&state, &auth, &checkpoint_ref).await?,
    ))
}

pub async fn get_object(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(object_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::get_object(&state, &auth, &object_ref).await?,
    ))
}

pub async fn get_source(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(source_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::get_source(&state, &auth, &source_ref).await?,
    ))
}

pub async fn get_source_content(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(source_ref): Path<String>,
) -> ApiResult<Response> {
    let (media_type, content) = control_service::source_content(&state, &auth, &source_ref).await?;
    Ok(([(axum::http::header::CONTENT_TYPE, media_type)], content).into_response())
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

pub async fn revoke_credential(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(credential_ref): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        control_service::revoke_credential(&state, &auth, &credential_ref).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_metadata_is_not_repeated_inside_data() {
        let mut data = json!({
            "session_id": "session:1",
            "corpus_revision": "revision:1",
            "status": "complete",
            "freshness": {"source_updated_at": "now"},
            "coverage": {"searched": []},
            "conflicts": [{"kind": "conflict"}],
            "gaps": [],
            "ambiguities": [],
            "initial_evidence": [{"content": "evidence"}],
            "projection": {"audit_receipt": "audit:1"}
        });

        remove_envelope_duplicates(&mut data, true);

        let object = data.as_object().expect("object");
        for key in [
            "session_id",
            "corpus_revision",
            "status",
            "freshness",
            "coverage",
            "conflicts",
            "gaps",
            "ambiguities",
        ] {
            assert!(!object.contains_key(key), "duplicate field {key}");
        }
        assert!(object.contains_key("initial_evidence"));
        assert!(object.contains_key("projection"));
    }
}

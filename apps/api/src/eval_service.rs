//! Native provisioning for the frozen evaluation harness.
//!
//! This module deliberately does not call the generic save path. Evaluation
//! imports need a new, isolated user before ordinary row-level security can be
//! applied, and the base/delta/checkpoint state must become visible together.
//!
//! Bearer-token replay rule: only a hash of the derived evaluation token is
//! stored. The plaintext token is returned after the first successful commit
//! and is never placed in a receipt, audit row, trace, or status response. A
//! replay made with the issued read/write token receives the converged receipt
//! without another token. A replay made with the provisioning credential gets
//! `eval_token_unavailable_on_replay` instead of a successful response, because
//! `native_eval.py` would otherwise substitute that broader caller token for
//! the missing isolated token. The signed status URL remains usable in either
//! case.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Path, State},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::{AuthContext, hash_token},
    db::{AppState, set_context},
    error::{ApiError, ApiResult},
    ingest::normalize_document,
    models::{Capability, CredentialId, UserId, canonical_json},
    quota::{self, UsageReservation},
};

const EVAL_SCHEMA: &str = "straylight-eval-import@v1";
const EFFECTIVE_SCOPE_REF: &str = "scope:root";
const EMBEDDING_DIMENSIONS: usize = 1_536;
const EMBEDDING_BATCH_SIZE: usize = 128;
const MAX_DOCUMENTS: usize = 5_000;
const MAX_TOTAL_TEXT_BYTES: usize = 128 * 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_IDEMPOTENCY_BYTES: usize = 240;

const READ_CAPABILITIES: &[&str] = &["open", "query", "read", "compute", "verify", "status"];
const WRITE_CAPABILITIES: &[&str] = &[
    "open",
    "query",
    "read",
    "compute",
    "verify",
    "status",
    "checkpoint",
    "save",
    "stage",
    "correct",
    "delete",
    "dream",
];

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Deserialize, Serialize)]
pub struct EvalImportRequest {
    pub schema: String,
    pub run_id: String,
    pub case_id: String,
    pub authorization_scope: String,
    pub display_scope: String,
    pub access_mode: String,
    pub documents: Vec<EvalDocument>,
    #[serde(default)]
    pub delta_documents: Vec<EvalDocument>,
    pub seed_checkpoint: Option<EvalSeedCheckpoint>,
    pub idempotency_key: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EvalDocument {
    pub path: String,
    pub content: String,
    pub content_sha256: String,
    pub media_type: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EvalSeedCheckpoint {
    pub state: Value,
    #[serde(default)]
    pub source_refs: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EvalImportResponse {
    pub status: String,
    pub ready_for_evaluation: bool,
    pub import_id: String,
    pub status_url: String,
    pub authorization_scope: String,
    pub requested_authorization_scope: String,
    pub display_scope: String,
    pub access_mode: String,
    pub user_id: String,
    pub scope_id: String,
    pub credential_id: String,
    pub base_corpus_revision: String,
    pub corpus_revision: String,
    pub checkpoint_id: Option<String>,
    pub seed_session_id: Option<String>,
    pub index_status: EvalIndexStatus,
    pub index_counts: EvalIndexCounts,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_degraded: bool,
    pub documents: usize,
    pub delta_documents: usize,
    pub replayed: bool,
    pub token_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_token: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct EvalIndexStatus {
    pub exact: String,
    pub lexical: String,
    pub semantic: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct EvalIndexCounts {
    pub documents: usize,
    pub chunks: usize,
    pub embeddings: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl AccessMode {
    fn parse(value: &str) -> ApiResult<Self> {
        match value {
            "read_only" => Ok(Self::ReadOnly),
            "read_write" => Ok(Self::ReadWrite),
            _ => Err(ApiError::invalid(
                "access_mode must be read_only or read_write",
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
        }
    }

    fn capabilities(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => READ_CAPABILITIES,
            Self::ReadWrite => WRITE_CAPABILITIES,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ImportPhase {
    Base,
    Delta,
}

impl ImportPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Delta => "delta",
        }
    }
}

#[derive(Clone)]
struct PreparedChunk {
    id: Uuid,
    ordinal: i32,
    heading: String,
    start_line: usize,
    end_line: usize,
    content: String,
    content_hash: String,
    token_count: i32,
}

#[derive(Clone)]
struct PreparedDocument {
    phase: ImportPhase,
    staged_entry_id: Uuid,
    path: String,
    content: String,
    content_hash: String,
    media_type: String,
    title: String,
    headings: Vec<String>,
    source_id: Uuid,
    evidence_id: Uuid,
    document_id: Uuid,
    document_version: i32,
    chunks: Vec<PreparedChunk>,
    duplicate: bool,
}

#[derive(Clone)]
struct PathState {
    document_id: Uuid,
    document_version: i32,
    content_hash: String,
    source_id: Uuid,
    evidence_id: Uuid,
    chunk_ids: Vec<Uuid>,
}

#[derive(Clone)]
struct CorpusMember {
    kind: &'static str,
    version: Option<i32>,
    disposition: &'static str,
}

struct PreparedImport {
    documents: Vec<PreparedDocument>,
    base_members: BTreeMap<Uuid, CorpusMember>,
    final_members: BTreeMap<Uuid, CorpusMember>,
    base_source_ids: HashMap<String, Uuid>,
    checkpoint_id: Option<Uuid>,
}

#[derive(Clone, Deserialize, Serialize)]
struct ImportLocator {
    version: u8,
    user_id: Uuid,
    credential_id: Uuid,
    scope_id: Uuid,
    receipt_id: Uuid,
    result_revision_id: Uuid,
    access_mode: String,
}

/// Axum-ready `POST /v1/admin/eval/import` handler.
pub async fn import_evaluation(
    State(state): State<AppState>,
    Extension(caller): Extension<AuthContext>,
    Json(request): Json<EvalImportRequest>,
) -> ApiResult<Json<EvalImportResponse>> {
    Ok(Json(provision_evaluation(&state, &caller, request).await?))
}

/// Axum-ready `GET /v1/admin/eval/imports/{import_id}` handler.
pub async fn get_evaluation_import(
    State(state): State<AppState>,
    Extension(caller): Extension<AuthContext>,
    Path(import_id): Path<String>,
) -> ApiResult<Json<EvalImportResponse>> {
    Ok(Json(
        evaluation_import_status(&state, &caller, &import_id).await?,
    ))
}

/// Provision one isolated evaluation corpus and issue its bearer token once.
pub async fn provision_evaluation(
    state: &AppState,
    caller: &AuthContext,
    request: EvalImportRequest,
) -> ApiResult<EvalImportResponse> {
    caller.require(Capability::Admin)?;
    let access_mode = request.validate()?;
    if state.embedder.dimensions() != EMBEDDING_DIMENSIONS {
        return Err(ApiError::public(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "dependency_unavailable",
            format!(
                "the current schema requires {EMBEDDING_DIMENSIONS}-dimension embeddings; configured embedder reports {}",
                state.embedder.dimensions()
            ),
        ));
    }

    let request_hash = request_hash(&request)?;
    let external_ref = isolated_external_ref(&request);
    let display_name = bounded_display_name(&request.case_id);
    let token = derive_bearer_token(&state.config.continuation_secret, &external_ref)?;
    let token_hash = hash_token(&token);
    let prepared = prepare_import(&request)?;

    let mut tx = state.rw_pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("straylight-eval:{external_ref}"))
        .execute(&mut *tx)
        .await?;
    set_context(&mut tx, caller).await?;

    // Bootstrap with write capabilities so read-only evaluation corpora can be
    // built under ordinary RLS. The credential is atomically reduced to its
    // requested capabilities before commit.
    let (user_id, credential_id, scope_id, policy_id) = bootstrap_user(
        &mut tx,
        &external_ref,
        &display_name,
        "Evaluation provisioning",
        &token_hash,
        WRITE_CAPABILITIES,
    )
    .await?;
    let provisioning_auth = evaluation_auth(user_id, credential_id, WRITE_CAPABILITIES, false);
    set_context(&mut tx, &provisioning_auth).await?;

    if let Some(mut replay) = existing_receipt(
        &mut tx,
        user_id,
        scope_id,
        &request.idempotency_key,
        &request_hash,
    )
    .await?
    {
        replay.replayed = true;
        replay.token_status = "not_reissued".to_owned();
        replay.credential_token = None;

        if caller.user_id.0 != user_id || caller.credential_id.0 != credential_id {
            return Err(ApiError::conflict(
                "eval_token_unavailable_on_replay",
                "the evaluation import already committed and its one-time bearer token cannot be reissued",
                json!({
                    "import": replay,
                    "recovery": "reuse the originally issued token, or use a new run/idempotency key"
                }),
            ));
        }

        finalize_credential(
            &mut tx,
            &external_ref,
            &display_name,
            access_mode,
            &token_hash,
        )
        .await?;
        let final_auth = evaluation_auth(
            user_id,
            credential_id,
            access_mode.capabilities(),
            access_mode == AccessMode::ReadOnly,
        );
        set_context(&mut tx, &final_auth).await?;
        tx.commit().await?;
        return Ok(replay);
    }

    let embedding_input_chars = prepared
        .documents
        .iter()
        .filter(|document| !document.duplicate)
        .flat_map(|document| &document.chunks)
        .map(|chunk| chunk.content.len())
        .sum::<usize>();
    quota::reserve_in_tx(
        &mut tx,
        user_id,
        UsageReservation {
            embedding_input_chars: embedding_input_chars as u64,
            ..UsageReservation::default()
        },
    )
    .await?;
    let embeddings = embed_prepared_chunks(state, &prepared).await?;
    for document in &prepared.documents {
        insert_prepared_document(
            &mut tx,
            user_id,
            scope_id,
            policy_id,
            document,
            &embeddings,
            state.embedder.model(),
        )
        .await?;
    }

    let receipt_id = Uuid::now_v7();
    let empty_revision_id = Uuid::now_v7();
    let base_revision_id = Uuid::now_v7();
    let empty_manifest_hash = manifest_hash(&BTreeMap::new());
    let base_manifest_hash = manifest_hash(&prepared.base_members);

    insert_revision(
        &mut tx,
        user_id,
        scope_id,
        empty_revision_id,
        None,
        1,
        &empty_manifest_hash,
        receipt_id,
    )
    .await?;
    insert_revision(
        &mut tx,
        user_id,
        scope_id,
        base_revision_id,
        Some(empty_revision_id),
        2,
        &base_manifest_hash,
        receipt_id,
    )
    .await?;
    insert_members(
        &mut tx,
        user_id,
        scope_id,
        base_revision_id,
        &prepared.base_members,
        true,
    )
    .await?;

    let seed_session_id = if let (Some(seed), Some(checkpoint_id)) =
        (&request.seed_checkpoint, prepared.checkpoint_id)
    {
        Some(
            insert_seed_checkpoint(
                &mut tx,
                user_id,
                credential_id,
                scope_id,
                policy_id,
                base_revision_id,
                checkpoint_id,
                access_mode,
                seed,
                &prepared.base_source_ids,
            )
            .await?,
        )
    } else {
        None
    };

    let (result_revision_id, result_manifest_hash) = if request.delta_documents.is_empty() {
        (base_revision_id, base_manifest_hash.clone())
    } else {
        let result_revision_id = Uuid::now_v7();
        let result_manifest_hash = manifest_hash(&prepared.final_members);
        insert_revision(
            &mut tx,
            user_id,
            scope_id,
            result_revision_id,
            Some(base_revision_id),
            3,
            &result_manifest_hash,
            receipt_id,
        )
        .await?;
        insert_members(
            &mut tx,
            user_id,
            scope_id,
            result_revision_id,
            &prepared.final_members,
            false,
        )
        .await?;
        (result_revision_id, result_manifest_hash)
    };

    insert_active_manifest(
        &mut tx,
        user_id,
        scope_id,
        result_revision_id,
        &result_manifest_hash,
    )
    .await?;
    disable_automatic_dreaming_for_evaluation(
        &mut tx,
        user_id,
        credential_id,
        scope_id,
        result_revision_id,
    )
    .await?;

    let locator = ImportLocator {
        version: 1,
        user_id,
        credential_id,
        scope_id,
        receipt_id,
        result_revision_id,
        access_mode: access_mode.as_str().to_owned(),
    };
    let import_id = sign_import_locator(&state.config.continuation_secret, &locator)?;
    let status_url = format!("/v1/admin/eval/imports/{import_id}");
    let index_counts = active_index_counts(&prepared.final_members);
    let mut response = EvalImportResponse {
        status: "ready".to_owned(),
        ready_for_evaluation: true,
        import_id,
        status_url,
        // bootstrap_user intentionally creates one private root scope per
        // isolated user. Returning the effective ref keeps open/query aligned
        // with the credential projection; the requested label remains below.
        authorization_scope: EFFECTIVE_SCOPE_REF.to_owned(),
        requested_authorization_scope: request.authorization_scope.clone(),
        display_scope: request.display_scope.clone(),
        access_mode: access_mode.as_str().to_owned(),
        user_id: user_ref(user_id),
        scope_id: scope_ref(scope_id),
        credential_id: credential_ref(credential_id),
        base_corpus_revision: revision_ref(base_revision_id),
        corpus_revision: revision_ref(result_revision_id),
        checkpoint_id: prepared.checkpoint_id.map(checkpoint_ref),
        seed_session_id: seed_session_id.map(session_ref),
        index_status: ready_index_status(),
        index_counts,
        embedding_provider: state.embedder.provider().to_owned(),
        embedding_model: state.embedder.model().to_owned(),
        embedding_degraded: state.embedder.is_degraded(),
        documents: request.documents.len(),
        delta_documents: request.delta_documents.len(),
        replayed: false,
        token_status: "issued_once".to_owned(),
        credential_token: None,
    };

    let stage_id = Uuid::now_v7();
    let inventory_hash = inventory_hash(&request);
    insert_import_receipt(
        &mut tx,
        user_id,
        credential_id,
        scope_id,
        policy_id,
        empty_revision_id,
        result_revision_id,
        stage_id,
        receipt_id,
        &request,
        &prepared,
        &request_hash,
        &inventory_hash,
        &result_manifest_hash,
        &response,
    )
    .await?;

    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,actor_ref,action,request_id,details,content_free
        ) VALUES ($1,$2,$3,$4,$5,'eval.import.provision',$6,$7,true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(credential_id)
    .bind(caller.user_id.ref_string())
    .bind(format!("eval:{}:{}", request.run_id, request.case_id))
    .bind(json!({
        "schema": &request.schema,
        "run_id": &request.run_id,
        "case_id": &request.case_id,
        "requested_authorization_scope": &request.authorization_scope,
        "effective_authorization_scope": EFFECTIVE_SCOPE_REF,
        "access_mode": access_mode.as_str(),
        "caller_credential_id": caller.credential_id.0,
        "import_id": &response.import_id,
        "request_hash": &request_hash
    }))
    .execute(&mut *tx)
    .await?;

    finalize_credential(
        &mut tx,
        &external_ref,
        &display_name,
        access_mode,
        &token_hash,
    )
    .await?;
    let final_auth = evaluation_auth(
        user_id,
        credential_id,
        access_mode.capabilities(),
        access_mode == AccessMode::ReadOnly,
    );
    set_context(&mut tx, &final_auth).await?;
    tx.commit().await?;

    response.credential_token = Some(token);
    Ok(response)
}

/// Return a token-free status receipt using a signed, RLS-bound locator.
pub async fn evaluation_import_status(
    state: &AppState,
    caller: &AuthContext,
    import_id: &str,
) -> ApiResult<EvalImportResponse> {
    caller.require(Capability::Admin)?;
    let locator = verify_import_locator(&state.config.continuation_secret, import_id)?;
    let access_mode = AccessMode::parse(&locator.access_mode)?;
    let auth = evaluation_auth(
        locator.user_id,
        locator.credential_id,
        access_mode.capabilities(),
        access_mode == AccessMode::ReadOnly,
    );
    let mut tx = state.begin_read(&auth).await?;
    let receipt: Value = sqlx::query_scalar(
        r#"
        SELECT receipt
        FROM straylight.import_receipts
        WHERE user_id=$1 AND scope_id=$2 AND id=$3 AND result_corpus_revision_id=$4
        "#,
    )
    .bind(locator.user_id)
    .bind(locator.scope_id)
    .bind(locator.receipt_id)
    .bind(locator.result_revision_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("eval_import_not_found", import_id))?;
    let mut response: EvalImportResponse = serde_json::from_value(receipt)?;
    if response.import_id != import_id {
        return Err(ApiError::not_found("eval_import_not_found", import_id));
    }

    let row = sqlx::query(
        r#"
        SELECT
          count(DISTINCT cm.record_id) FILTER (
            WHERE cm.record_kind='document' AND cm.disposition='active'
          )::bigint AS documents,
          count(DISTINCT cm.record_id) FILTER (
            WHERE cm.record_kind='chunk' AND cm.disposition='active'
          )::bigint AS chunks,
          count(DISTINCT embedding.target_record_id) FILTER (
            WHERE cm.record_kind='chunk' AND cm.disposition='active'
              AND embedding.model=$3
          )::bigint AS embeddings
        FROM straylight.corpus_members AS cm
        LEFT JOIN straylight.embeddings AS embedding
          ON embedding.user_id=cm.user_id
         AND embedding.scope_id=cm.scope_id
         AND embedding.target_record_id=cm.record_id
         AND embedding.granularity='chunk'
        WHERE cm.user_id=$1 AND cm.corpus_revision_id=$2
        "#,
    )
    .bind(locator.user_id)
    .bind(locator.result_revision_id)
    .bind(&response.embedding_model)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let documents = usize_from_i64(row.try_get("documents")?);
    let chunks = usize_from_i64(row.try_get("chunks")?);
    let embeddings = usize_from_i64(row.try_get("embeddings")?);
    let semantic_ready = chunks == embeddings;
    response.index_counts = EvalIndexCounts {
        documents,
        chunks,
        embeddings,
    };
    response.index_status = EvalIndexStatus {
        exact: "ready".to_owned(),
        lexical: "ready".to_owned(),
        semantic: if semantic_ready {
            "ready"
        } else {
            "inconsistent"
        }
        .to_owned(),
    };
    response.ready_for_evaluation = semantic_ready;
    response.status = if semantic_ready {
        "ready"
    } else {
        "inconsistent"
    }
    .to_owned();
    response.credential_token = None;
    response.token_status = "not_available_from_status".to_owned();
    Ok(response)
}

impl EvalImportRequest {
    fn validate(&self) -> ApiResult<AccessMode> {
        if self.schema != EVAL_SCHEMA {
            return Err(ApiError::invalid(format!("schema must be {EVAL_SCHEMA}")));
        }
        validate_nonempty("run_id", &self.run_id, MAX_IDENTIFIER_BYTES)?;
        validate_nonempty("case_id", &self.case_id, MAX_IDENTIFIER_BYTES)?;
        validate_nonempty(
            "authorization_scope",
            &self.authorization_scope,
            MAX_IDENTIFIER_BYTES,
        )?;
        if self.authorization_scope.contains(',') {
            return Err(ApiError::invalid(
                "authorization_scope cannot contain a comma",
            ));
        }
        validate_nonempty("display_scope", &self.display_scope, MAX_IDENTIFIER_BYTES)?;
        validate_nonempty(
            "idempotency_key",
            &self.idempotency_key,
            MAX_IDEMPOTENCY_BYTES,
        )?;
        let access_mode = AccessMode::parse(&self.access_mode)?;
        if self.documents.is_empty() {
            return Err(ApiError::invalid(
                "documents must contain at least one base document",
            ));
        }
        let document_count = self
            .documents
            .len()
            .checked_add(self.delta_documents.len())
            .ok_or_else(|| ApiError::invalid("document count overflow"))?;
        if document_count > MAX_DOCUMENTS {
            return Err(ApiError::public(
                http::StatusCode::PAYLOAD_TOO_LARGE,
                "batch_limit_exceeded",
                format!("evaluation imports are limited to {MAX_DOCUMENTS} documents"),
            ));
        }

        let mut total_bytes = 0usize;
        validate_document_group("documents", &self.documents, &mut total_bytes)?;
        validate_document_group("delta_documents", &self.delta_documents, &mut total_bytes)?;
        if total_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(ApiError::public(
                http::StatusCode::PAYLOAD_TOO_LARGE,
                "batch_limit_exceeded",
                format!("evaluation text is limited to {MAX_TOTAL_TEXT_BYTES} UTF-8 bytes"),
            ));
        }
        if let Some(seed) = &self.seed_checkpoint {
            if !seed.state.is_object() {
                return Err(ApiError::invalid(
                    "seed_checkpoint.state must be a JSON object",
                ));
            }
            for source in &seed.source_refs {
                validate_logical_path("seed checkpoint source", source)?;
            }
        }
        Ok(access_mode)
    }
}

fn validate_nonempty(name: &str, value: &str, max_bytes: usize) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Err(ApiError::invalid(format!("{name} cannot be empty")));
    }
    if value.len() > max_bytes {
        return Err(ApiError::invalid(format!(
            "{name} is limited to {max_bytes} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_document_group(
    name: &str,
    documents: &[EvalDocument],
    total_bytes: &mut usize,
) -> ApiResult<()> {
    let mut paths = HashSet::new();
    for document in documents {
        validate_logical_path("document path", &document.path)?;
        if !paths.insert(document.path.as_str()) {
            return Err(ApiError::invalid(format!(
                "{name} contains duplicate path {}",
                document.path
            )));
        }
        if !is_text_media_type(&document.media_type) {
            return Err(ApiError::invalid(format!(
                "{} is not a supported text media type",
                document.media_type
            )));
        }
        let supplied = document
            .content_sha256
            .strip_prefix("sha256:")
            .unwrap_or(&document.content_sha256);
        let actual = sha256_hex(document.content.as_bytes());
        if supplied != actual {
            return Err(ApiError::invalid(format!(
                "content_sha256 does not match document {}",
                document.path
            )));
        }
        *total_bytes = total_bytes
            .checked_add(document.content.len())
            .ok_or_else(|| ApiError::invalid("document byte count overflow"))?;
    }
    Ok(())
}

fn validate_logical_path(name: &str, path: &str) -> ApiResult<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path.split('/').any(|part| part == "..")
    {
        return Err(ApiError::invalid(format!(
            "{name} must be a safe relative POSIX path"
        )));
    }
    Ok(())
}

fn is_text_media_type(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json"
                | "application/x-ndjson"
                | "application/sql"
                | "application/yaml"
                | "application/toml"
                | "application/xml"
        )
}

fn prepare_import(request: &EvalImportRequest) -> ApiResult<PreparedImport> {
    let mut documents = Vec::with_capacity(request.documents.len() + request.delta_documents.len());
    let mut base_members = BTreeMap::new();
    let mut path_state = HashMap::<String, PathState>::new();
    let mut base_source_ids = HashMap::new();

    for source in &request.documents {
        let prepared = prepare_document(source, ImportPhase::Base, Uuid::now_v7(), 1);
        add_document_members(&mut base_members, &prepared);
        base_source_ids.insert(prepared.path.clone(), prepared.source_id);
        path_state.insert(prepared.path.clone(), state_from_document(&prepared));
        documents.push(prepared);
    }

    let checkpoint_id = request.seed_checkpoint.as_ref().map(|_| Uuid::now_v7());
    if let Some(checkpoint_id) = checkpoint_id {
        base_members.insert(
            checkpoint_id,
            CorpusMember {
                kind: "checkpoint",
                version: None,
                disposition: "active",
            },
        );
    }
    let mut final_members = base_members.clone();

    for source in &request.delta_documents {
        let content_hash = sha256_hex(source.content.as_bytes());
        if let Some(prior) = path_state.get(&source.path).cloned() {
            if prior.content_hash == content_hash {
                let normalized = normalize_document(&source.path, &source.content);
                documents.push(PreparedDocument {
                    phase: ImportPhase::Delta,
                    staged_entry_id: Uuid::now_v7(),
                    path: source.path.clone(),
                    content: source.content.clone(),
                    content_hash,
                    media_type: source.media_type.clone(),
                    title: normalized.title,
                    headings: normalized.headings,
                    source_id: prior.source_id,
                    evidence_id: prior.evidence_id,
                    document_id: prior.document_id,
                    document_version: prior.document_version,
                    chunks: Vec::new(),
                    duplicate: true,
                });
                continue;
            }

            mark_superseded(&mut final_members, prior.source_id);
            mark_superseded(&mut final_members, prior.evidence_id);
            for chunk_id in prior.chunk_ids {
                mark_superseded(&mut final_members, chunk_id);
            }
            let prepared = prepare_document(
                source,
                ImportPhase::Delta,
                prior.document_id,
                prior.document_version + 1,
            );
            add_document_members(&mut final_members, &prepared);
            path_state.insert(prepared.path.clone(), state_from_document(&prepared));
            documents.push(prepared);
        } else {
            let prepared = prepare_document(source, ImportPhase::Delta, Uuid::now_v7(), 1);
            add_document_members(&mut final_members, &prepared);
            path_state.insert(prepared.path.clone(), state_from_document(&prepared));
            documents.push(prepared);
        }
    }

    if let Some(seed) = &request.seed_checkpoint {
        for source_ref in &seed.source_refs {
            if !base_source_ids.contains_key(source_ref) {
                return Err(ApiError::invalid(format!(
                    "seed checkpoint source is not in the base corpus: {source_ref}"
                )));
            }
        }
    }

    Ok(PreparedImport {
        documents,
        base_members,
        final_members,
        base_source_ids,
        checkpoint_id,
    })
}

fn prepare_document(
    source: &EvalDocument,
    phase: ImportPhase,
    document_id: Uuid,
    document_version: i32,
) -> PreparedDocument {
    let normalized = normalize_document(&source.path, &source.content);
    let chunks = normalized
        .chunks
        .into_iter()
        .map(|chunk| PreparedChunk {
            id: Uuid::now_v7(),
            ordinal: i32::try_from(chunk.ordinal).unwrap_or(i32::MAX),
            heading: chunk.heading,
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            content: chunk.content,
            content_hash: unprefixed_hash(&chunk.content_hash).to_owned(),
            token_count: i32::try_from(chunk.estimated_tokens).unwrap_or(i32::MAX),
        })
        .collect();
    PreparedDocument {
        phase,
        staged_entry_id: Uuid::now_v7(),
        path: source.path.clone(),
        content: source.content.clone(),
        content_hash: sha256_hex(source.content.as_bytes()),
        media_type: source.media_type.clone(),
        title: normalized.title,
        headings: normalized.headings,
        source_id: Uuid::now_v7(),
        evidence_id: Uuid::now_v7(),
        document_id,
        document_version,
        chunks,
        duplicate: false,
    }
}

fn state_from_document(document: &PreparedDocument) -> PathState {
    PathState {
        document_id: document.document_id,
        document_version: document.document_version,
        content_hash: document.content_hash.clone(),
        source_id: document.source_id,
        evidence_id: document.evidence_id,
        chunk_ids: document.chunks.iter().map(|chunk| chunk.id).collect(),
    }
}

fn add_document_members(members: &mut BTreeMap<Uuid, CorpusMember>, document: &PreparedDocument) {
    members.insert(
        document.source_id,
        CorpusMember {
            kind: "source_episode",
            version: None,
            disposition: "active",
        },
    );
    members.insert(
        document.evidence_id,
        CorpusMember {
            kind: "evidence",
            version: None,
            disposition: "active",
        },
    );
    members.insert(
        document.document_id,
        CorpusMember {
            kind: "document",
            version: Some(document.document_version),
            disposition: "active",
        },
    );
    for chunk in &document.chunks {
        members.insert(
            chunk.id,
            CorpusMember {
                kind: "chunk",
                version: None,
                disposition: "active",
            },
        );
    }
}

fn mark_superseded(members: &mut BTreeMap<Uuid, CorpusMember>, record_id: Uuid) {
    if let Some(member) = members.get_mut(&record_id) {
        member.disposition = "superseded";
    }
}

async fn embed_prepared_chunks(
    state: &AppState,
    prepared: &PreparedImport,
) -> ApiResult<HashMap<Uuid, Vec<f32>>> {
    let inputs: Vec<_> = prepared
        .documents
        .iter()
        .filter(|document| !document.duplicate)
        .flat_map(|document| {
            document
                .chunks
                .iter()
                .map(|chunk| (chunk.id, chunk.content.clone()))
        })
        .collect();
    let mut result = HashMap::with_capacity(inputs.len());
    for batch in inputs.chunks(EMBEDDING_BATCH_SIZE) {
        let texts: Vec<String> = batch.iter().map(|(_, text)| text.clone()).collect();
        let vectors = state.embedder.embed(&texts).await?;
        if vectors.len() != batch.len()
            || vectors
                .iter()
                .any(|vector| vector.len() != EMBEDDING_DIMENSIONS)
        {
            return Err(ApiError::Internal(
                "embedding provider returned an unexpected batch shape".to_owned(),
            ));
        }
        for ((chunk_id, _), vector) in batch.iter().zip(vectors) {
            result.insert(*chunk_id, vector);
        }
    }
    Ok(result)
}

async fn bootstrap_user(
    tx: &mut Transaction<'_, Postgres>,
    external_ref: &str,
    display_name: &str,
    credential_label: &str,
    token_hash: &str,
    capabilities: &[&str],
) -> ApiResult<(Uuid, Uuid, Uuid, Uuid)> {
    let capabilities: Vec<String> = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    Ok(sqlx::query_as(
        "SELECT * FROM straylight_auth.bootstrap_evaluation_user($1, $2, $3, $4, $5)",
    )
    .bind(external_ref)
    .bind(display_name)
    .bind(credential_label)
    .bind(token_hash)
    .bind(&capabilities)
    .fetch_one(&mut **tx)
    .await?)
}

async fn finalize_credential(
    tx: &mut Transaction<'_, Postgres>,
    external_ref: &str,
    display_name: &str,
    access_mode: AccessMode,
    token_hash: &str,
) -> ApiResult<()> {
    let label = match access_mode {
        AccessMode::ReadOnly => "Evaluation read-only",
        AccessMode::ReadWrite => "Evaluation read/write",
    };
    let _ = bootstrap_user(
        tx,
        external_ref,
        display_name,
        label,
        token_hash,
        access_mode.capabilities(),
    )
    .await?;
    Ok(())
}

fn evaluation_auth(
    user_id: Uuid,
    credential_id: Uuid,
    capabilities: &[&str],
    read_only: bool,
) -> AuthContext {
    AuthContext {
        credential_id: CredentialId(credential_id),
        user_id: UserId(user_id),
        capabilities: capabilities
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        scope_refs: vec![EFFECTIVE_SCOPE_REF.to_owned()],
        read_only,
    }
}

async fn existing_receipt(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> ApiResult<Option<EvalImportResponse>> {
    let row = sqlx::query(
        r#"
        SELECT input_hash, receipt
        FROM straylight.import_receipts
        WHERE user_id=$1 AND scope_id=$2 AND stable_import_id=$3
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let existing_hash: String = row.try_get("input_hash")?;
    if existing_hash != request_hash {
        return Err(ApiError::conflict(
            "idempotency_conflict",
            "the evaluation idempotency key was already used for a different request",
            json!({"idempotency_key": idempotency_key}),
        ));
    }
    let receipt: Value = row.try_get("receipt")?;
    Ok(Some(serde_json::from_value(receipt)?))
}

async fn insert_prepared_document(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    policy_id: Uuid,
    document: &PreparedDocument,
    embeddings: &HashMap<Uuid, Vec<f32>>,
    embedding_model: &str,
) -> ApiResult<()> {
    if document.duplicate {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.source_episodes (
          id,user_id,scope_id,source_ref,source_kind,source_version,title,
          captured_at,content_hash,native_locator,metadata,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,'evaluation_document',$5,$6,clock_timestamp(),$5,$7,$8,$9,1)
        "#,
    )
    .bind(document.source_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(&document.path)
    .bind(&document.content_hash)
    .bind(&document.title)
    .bind(json!({"path": document.path, "media_type": document.media_type}))
    .bind(json!({
        "evaluation_phase": document.phase.as_str(),
        "raw_task_persisted": false
    }))
    .bind(policy_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO straylight.evidence_items (
          id,user_id,scope_id,source_episode_id,evidence_kind,locator,
          text_content,content_hash,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,'source_native',$5,$6,$7,$8,1)
        "#,
    )
    .bind(document.evidence_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(document.source_id)
    .bind(json!({
        "path": document.path,
        "start_line": 1,
        "end_line": document.content.lines().count().max(1)
    }))
    .bind(&document.content)
    .bind(&document.content_hash)
    .bind(policy_id)
    .execute(&mut **tx)
    .await?;

    if document.document_version == 1 {
        sqlx::query(
            r#"
            INSERT INTO straylight.documents (
              id,user_id,scope_id,current_version,policy_id,policy_version
            ) VALUES ($1,$2,$3,1,$4,1)
            "#,
        )
        .bind(document.document_id)
        .bind(user_id)
        .bind(scope_id)
        .bind(policy_id)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.document_revisions (
          user_id,document_id,version,previous_version,source_episode_id,
          title,media_type,content_hash,native_locator,byte_size
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
        "#,
    )
    .bind(user_id)
    .bind(document.document_id)
    .bind(document.document_version)
    .bind((document.document_version > 1).then_some(document.document_version - 1))
    .bind(document.source_id)
    .bind(&document.title)
    .bind(&document.media_type)
    .bind(&document.content_hash)
    .bind(json!({
        "path": document.path,
        "source_ref": document.path,
        "headings": document.headings
    }))
    .bind(i64::try_from(document.content.len()).unwrap_or(i64::MAX))
    .execute(&mut **tx)
    .await?;
    if document.document_version > 1 {
        let result = sqlx::query(
            r#"
            UPDATE straylight.documents
            SET current_version=$3
            WHERE user_id=$1 AND id=$2 AND current_version=$3-1
            "#,
        )
        .bind(user_id)
        .bind(document.document_id)
        .bind(document.document_version)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ApiError::conflict(
                "document_version_conflict",
                "evaluation document head did not advance exactly once",
                json!({"path": document.path}),
            ));
        }
    }

    for chunk in &document.chunks {
        sqlx::query(
            r#"
            INSERT INTO straylight.chunks (
              id,user_id,scope_id,document_id,document_version,ordinal,
              chunk_kind,heading_path,locator,content,content_hash,token_count
            ) VALUES ($1,$2,$3,$4,$5,$6,'section',$7,$8,$9,$10,$11)
            "#,
        )
        .bind(chunk.id)
        .bind(user_id)
        .bind(scope_id)
        .bind(document.document_id)
        .bind(document.document_version)
        .bind(chunk.ordinal)
        .bind(vec![chunk.heading.clone()])
        .bind(json!({
            "path": document.path,
            "start_line": chunk.start_line,
            "end_line": chunk.end_line,
            "ordinal": chunk.ordinal
        }))
        .bind(&chunk.content)
        .bind(&chunk.content_hash)
        .bind(chunk.token_count)
        .execute(&mut **tx)
        .await?;

        let vector = embeddings.get(&chunk.id).ok_or_else(|| {
            ApiError::Internal(format!("embedding missing for chunk {}", chunk.id))
        })?;
        sqlx::query(
            r#"
            INSERT INTO straylight.embeddings (
              id,user_id,scope_id,target_record_id,granularity,model,
              dimensions,embedding,source_content_hash
            ) VALUES ($1,$2,$3,$4,'chunk',$5,$6,$7,$8)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(scope_id)
        .bind(chunk.id)
        .bind(embedding_model)
        .bind(i32::try_from(EMBEDDING_DIMENSIONS).unwrap_or(1_536))
        .bind(Vector::from(vector.clone()))
        .bind(&chunk.content_hash)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_revision(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
    parent_revision_id: Option<Uuid>,
    revision_number: i64,
    manifest_hash: &str,
    operation_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.corpus_revisions (
          id,user_id,scope_id,parent_revision_id,revision_number,manifest_hash,operation_id
        ) VALUES ($1,$2,$3,$4,$5,$6,$7)
        "#,
    )
    .bind(revision_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(parent_revision_id)
    .bind(revision_number)
    .bind(manifest_hash)
    .bind(operation_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_members(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
    members: &BTreeMap<Uuid, CorpusMember>,
    skip_checkpoint: bool,
) -> ApiResult<()> {
    for (record_id, member) in members {
        if skip_checkpoint && member.kind == "checkpoint" {
            continue;
        }
        sqlx::query(
            r#"
            INSERT INTO straylight.corpus_members (
              user_id,scope_id,corpus_revision_id,record_id,record_kind,
              record_version,disposition
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
            "#,
        )
        .bind(user_id)
        .bind(scope_id)
        .bind(revision_id)
        .bind(record_id)
        .bind(member.kind)
        .bind(member.version)
        .bind(member.disposition)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_seed_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    scope_id: Uuid,
    policy_id: Uuid,
    base_revision_id: Uuid,
    checkpoint_id: Uuid,
    access_mode: AccessMode,
    seed: &EvalSeedCheckpoint,
    base_sources: &HashMap<String, Uuid>,
) -> ApiResult<Uuid> {
    let session_id = Uuid::now_v7();
    let empty_task_hash = sha256_hex(b"");
    let projection_hash = sha256_hex(format!("{user_id}:{scope_id}:{policy_id}:1").as_bytes());
    let final_capabilities: Vec<String> = access_mode
        .capabilities()
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    sqlx::query(
        r#"
        INSERT INTO straylight.sessions (
          id,user_id,scope_id,credential_id,corpus_revision_id,task_hash,
          task_size_bytes,capability_snapshot,authorization_scope_refs,
          policy_projection_hash,mode,expires_at
        ) VALUES ($1,$2,$3,$4,$5,$6,0,$7,$8,$9,$10,clock_timestamp()+interval '30 days')
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(credential_id)
    .bind(base_revision_id)
    .bind(empty_task_hash)
    .bind(&final_capabilities)
    .bind(vec![EFFECTIVE_SCOPE_REF.to_owned()])
    .bind(projection_hash)
    .bind(access_mode.as_str())
    .execute(&mut **tx)
    .await?;

    let summary = canonical_json(&seed.state)?;
    let title = seed
        .state
        .get("objective")
        .and_then(Value::as_str)
        .or_else(|| seed.state.get("title").and_then(Value::as_str));
    sqlx::query(
        r#"
        INSERT INTO straylight.checkpoints (
          id,user_id,scope_id,parent_checkpoint_id,corpus_revision_id,
          session_id,title,summary,policy_id,policy_version
        ) VALUES ($1,$2,$3,NULL,$4,$5,$6,$7,$8,1)
        "#,
    )
    .bind(checkpoint_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(session_id)
    .bind(title)
    .bind(summary)
    .bind(policy_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO straylight.corpus_members (
          user_id,scope_id,corpus_revision_id,record_id,record_kind,disposition
        ) VALUES ($1,$2,$3,$4,'checkpoint','active')
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(checkpoint_id)
    .execute(&mut **tx)
    .await?;

    let source_paths: Vec<String> = if seed.source_refs.is_empty() {
        base_sources.keys().min().cloned().into_iter().collect()
    } else {
        seed.source_refs.clone()
    };
    for (position, path) in source_paths.iter().enumerate() {
        let source_id = base_sources.get(path).ok_or_else(|| {
            ApiError::invalid(format!(
                "seed checkpoint source is not in base corpus: {path}"
            ))
        })?;
        sqlx::query(
            r#"
            INSERT INTO straylight.checkpoint_source_refs (
              user_id,checkpoint_id,corpus_revision_id,position,
              source_record_id,source_version,locator
            ) VALUES ($1,$2,$3,$4,$5,NULL,$6)
            "#,
        )
        .bind(user_id)
        .bind(checkpoint_id)
        .bind(base_revision_id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .bind(source_id)
        .bind(json!({"path": path}))
        .execute(&mut **tx)
        .await?;
    }
    Ok(session_id)
}

async fn insert_active_manifest(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
    manifest_hash: &str,
) -> ApiResult<()> {
    let manifest_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.active_manifests (
          id,user_id,scope_id,active_corpus_revision_id,manifest_hash,generation
        ) VALUES ($1,$2,$3,$4,$5,1)
        "#,
    )
    .bind(manifest_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(revision_id)
    .bind(manifest_hash)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.active_manifest_history (
          id,user_id,scope_id,manifest_id,generation,corpus_revision_id,
          manifest_hash,change_kind
        ) VALUES ($1,$2,$3,$4,1,$5,$6,'initial')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(manifest_id)
    .bind(revision_id)
    .bind(manifest_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn disable_automatic_dreaming_for_evaluation(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.dream_scheduler_controls (
          user_id,scope_id,enabled,paused,dirty_score,dirty_reasons,
          observed_corpus_revision_id,updated_by_credential_id
        ) VALUES ($1,$2,false,true,0,ARRAY['evaluation_scope']::text[],$3,$4)
        ON CONFLICT (user_id,scope_id) DO UPDATE SET
          enabled=false,paused=true,dirty_score=0,
          dirty_reasons=ARRAY['evaluation_scope']::text[],
          dirty_root_ids='{}'::uuid[],dirty_since=NULL,
          observed_corpus_revision_id=$3,updated_by_credential_id=$4,
          updated_at=clock_timestamp()
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(revision_id)
    .bind(credential_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_import_receipt(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    credential_id: Uuid,
    scope_id: Uuid,
    policy_id: Uuid,
    empty_revision_id: Uuid,
    result_revision_id: Uuid,
    stage_id: Uuid,
    receipt_id: Uuid,
    request: &EvalImportRequest,
    prepared: &PreparedImport,
    request_hash: &str,
    inventory_hash: &str,
    manifest_hash: &str,
    response: &EvalImportResponse,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.stages (
          id,user_id,scope_id,credential_id,stable_import_id,
          base_corpus_revision_id,input_hash,inventory_hash,status,
          policy_id,policy_version,expires_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'promoted',$9,1,clock_timestamp()+interval '7 days')
        "#,
    )
    .bind(stage_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(credential_id)
    .bind(&request.idempotency_key)
    .bind(empty_revision_id)
    .bind(request_hash)
    .bind(inventory_hash)
    .bind(policy_id)
    .execute(&mut **tx)
    .await?;

    for document in &prepared.documents {
        sqlx::query(
            r#"
            INSERT INTO straylight.staged_entries (
              id,user_id,scope_id,stage_id,path,entry_kind,media_type,size_bytes,
              content_hash,readability,inspection
            ) VALUES ($1,$2,$3,$4,$5,'file',$6,$7,$8,'readable',$9)
            "#,
        )
        .bind(document.staged_entry_id)
        .bind(user_id)
        .bind(scope_id)
        .bind(stage_id)
        .bind(format!("{}/{}", document.phase.as_str(), document.path))
        .bind(&document.media_type)
        .bind(i64::try_from(document.content.len()).unwrap_or(i64::MAX))
        .bind(&document.content_hash)
        .bind(json!({
            "logical_path": document.path,
            "phase": document.phase.as_str(),
            "duplicate": document.duplicate
        }))
        .execute(&mut **tx)
        .await?;
    }

    let persisted_receipt = persisted_receipt(response)?;
    sqlx::query(
        r#"
        INSERT INTO straylight.import_receipts (
          id,user_id,scope_id,stable_import_id,stage_id,input_hash,inventory_hash,
          manifest_hash,base_corpus_revision_id,result_corpus_revision_id,
          replay_outcome,policy_id,policy_version,indexing_state,receipt
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'new_import',$11,1,$12,$13)
        "#,
    )
    .bind(receipt_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(&request.idempotency_key)
    .bind(stage_id)
    .bind(request_hash)
    .bind(inventory_hash)
    .bind(manifest_hash)
    .bind(empty_revision_id)
    .bind(result_revision_id)
    .bind(policy_id)
    .bind(json!({"exact": "ready", "lexical": "ready", "semantic": "ready"}))
    .bind(persisted_receipt)
    .execute(&mut **tx)
    .await?;

    for document in &prepared.documents {
        sqlx::query(
            r#"
            INSERT INTO straylight.import_entry_dispositions (
              user_id,import_receipt_id,staged_entry_id,disposition,
              resulting_record_id,details
            ) VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(user_id)
        .bind(receipt_id)
        .bind(document.staged_entry_id)
        .bind(if document.duplicate {
            "deduplicated"
        } else {
            "committed"
        })
        .bind(document.source_id)
        .bind(json!({
            "path": document.path,
            "phase": document.phase.as_str(),
            "document_id": document.document_id,
            "document_version": document.document_version
        }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn ready_index_status() -> EvalIndexStatus {
    EvalIndexStatus {
        exact: "ready".to_owned(),
        lexical: "ready".to_owned(),
        semantic: "ready".to_owned(),
    }
}

fn active_index_counts(members: &BTreeMap<Uuid, CorpusMember>) -> EvalIndexCounts {
    let documents = members
        .values()
        .filter(|member| member.kind == "document" && member.disposition == "active")
        .count();
    let chunks = members
        .values()
        .filter(|member| member.kind == "chunk" && member.disposition == "active")
        .count();
    EvalIndexCounts {
        documents,
        chunks,
        embeddings: chunks,
    }
}

fn persisted_receipt(response: &EvalImportResponse) -> ApiResult<Value> {
    let mut safe = response.clone();
    safe.credential_token = None;
    Ok(serde_json::to_value(safe)?)
}

fn request_hash(request: &EvalImportRequest) -> ApiResult<String> {
    let value = serde_json::to_value(request)?;
    Ok(sha256_hex(canonical_json(&value)?.as_bytes()))
}

fn inventory_hash(request: &EvalImportRequest) -> String {
    let mut rows = Vec::with_capacity(request.documents.len() + request.delta_documents.len());
    for (phase, documents) in [
        ("base", request.documents.as_slice()),
        ("delta", request.delta_documents.as_slice()),
    ] {
        for document in documents {
            rows.push(format!(
                "{phase}\0{}\0{}\0{}",
                document.path,
                document
                    .content_sha256
                    .strip_prefix("sha256:")
                    .unwrap_or(&document.content_sha256),
                document.media_type
            ));
        }
    }
    rows.sort();
    sha256_hex(rows.join("\n").as_bytes())
}

fn manifest_hash(members: &BTreeMap<Uuid, CorpusMember>) -> String {
    let mut hasher = Sha256::new();
    for (record_id, member) in members {
        hasher.update(record_id.as_bytes());
        hasher.update([0]);
        hasher.update(member.kind.as_bytes());
        hasher.update([0]);
        hasher.update(member.version.unwrap_or(0).to_be_bytes());
        hasher.update([0]);
        hasher.update(member.disposition.as_bytes());
        hasher.update([b'\n']);
    }
    hex::encode(hasher.finalize())
}

fn isolated_external_ref(request: &EvalImportRequest) -> String {
    let identity = format!(
        "{}\0{}\0{}\0{}\0{}",
        EVAL_SCHEMA,
        request.run_id,
        request.case_id,
        request.authorization_scope,
        request.idempotency_key
    );
    format!("eval-user:{}", sha256_hex(identity.as_bytes()))
}

fn bounded_display_name(case_id: &str) -> String {
    let suffix: String = case_id.chars().take(160).collect();
    format!("Straylight evaluation: {suffix}")
}

fn derive_bearer_token(secret: &str, external_ref: &str) -> ApiResult<String> {
    let signature = keyed_digest(
        secret,
        b"straylight-eval-bearer-v1",
        external_ref.as_bytes(),
    )?;
    Ok(format!("seval_{}", URL_SAFE_NO_PAD.encode(signature)))
}

fn sign_import_locator(secret: &str, locator: &ImportLocator) -> ApiResult<String> {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(locator)?);
    let signature = keyed_digest(
        secret,
        b"straylight-eval-import-locator-v1",
        payload.as_bytes(),
    )?;
    Ok(format!(
        "eval_import.{payload}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn verify_import_locator(secret: &str, import_id: &str) -> ApiResult<ImportLocator> {
    let mut parts = import_id.split('.');
    let prefix = parts.next();
    let payload = parts.next();
    let signature = parts.next();
    if prefix != Some("eval_import")
        || payload.is_none()
        || signature.is_none()
        || parts.next().is_some()
    {
        return Err(ApiError::not_found("eval_import_not_found", import_id));
    }
    let payload = payload.unwrap_or_default();
    let signature = URL_SAFE_NO_PAD
        .decode(signature.unwrap_or_default())
        .map_err(|_| ApiError::not_found("eval_import_not_found", import_id))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::Internal("could not initialize import locator signer".to_owned()))?;
    mac.update(b"straylight-eval-import-locator-v1");
    mac.update(&[0]);
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| ApiError::not_found("eval_import_not_found", import_id))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ApiError::not_found("eval_import_not_found", import_id))?;
    let locator: ImportLocator = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::not_found("eval_import_not_found", import_id))?;
    if locator.version != 1 || AccessMode::parse(&locator.access_mode).is_err() {
        return Err(ApiError::not_found("eval_import_not_found", import_id));
    }
    Ok(locator)
}

fn keyed_digest(secret: &str, domain: &[u8], payload: &[u8]) -> ApiResult<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::Internal("could not initialize evaluation signer".to_owned()))?;
    mac.update(domain);
    mac.update(&[0]);
    mac.update(payload);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn unprefixed_hash(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn user_ref(id: Uuid) -> String {
    format!("user:{}", id.simple())
}

fn scope_ref(id: Uuid) -> String {
    format!("scope:{}", id.simple())
}

fn credential_ref(id: Uuid) -> String {
    format!("credential:{}", id.simple())
}

fn revision_ref(id: Uuid) -> String {
    format!("revision:{}", id.simple())
}

fn checkpoint_ref(id: Uuid) -> String {
    format!("checkpoint:{}", id.simple())
}

fn session_ref(id: Uuid) -> String {
    format!("session:{}", id.simple())
}

fn usize_from_i64(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn document(path: &str, content: &str) -> EvalDocument {
        EvalDocument {
            path: path.to_owned(),
            content: content.to_owned(),
            content_sha256: sha256_hex(content.as_bytes()),
            media_type: "text/markdown".to_owned(),
        }
    }

    fn request(case_id: &str) -> EvalImportRequest {
        EvalImportRequest {
            schema: EVAL_SCHEMA.to_owned(),
            run_id: "run-1".to_owned(),
            case_id: case_id.to_owned(),
            authorization_scope: format!("eval:run-1/{case_id}"),
            display_scope: "Test workload".to_owned(),
            access_mode: "read_write".to_owned(),
            documents: vec![document("base/context.md", "# Context\nOriginal")],
            delta_documents: vec![],
            seed_checkpoint: None,
            idempotency_key: "eval-import:abc".to_owned(),
        }
    }

    fn response(token: Option<String>) -> EvalImportResponse {
        EvalImportResponse {
            status: "ready".to_owned(),
            ready_for_evaluation: true,
            import_id: "eval_import.example.signature".to_owned(),
            status_url: "/v1/admin/eval/imports/eval_import.example.signature".to_owned(),
            authorization_scope: EFFECTIVE_SCOPE_REF.to_owned(),
            requested_authorization_scope: "eval:run/case".to_owned(),
            display_scope: "case".to_owned(),
            access_mode: "read_write".to_owned(),
            user_id: "user:1".to_owned(),
            scope_id: "scope:1".to_owned(),
            credential_id: "credential:1".to_owned(),
            base_corpus_revision: "revision:1".to_owned(),
            corpus_revision: "revision:2".to_owned(),
            checkpoint_id: None,
            seed_session_id: None,
            index_status: ready_index_status(),
            index_counts: EvalIndexCounts::default(),
            embedding_provider: "hashing".to_owned(),
            embedding_model: "test".to_owned(),
            embedding_degraded: true,
            documents: 1,
            delta_documents: 0,
            replayed: false,
            token_status: "issued_once".to_owned(),
            credential_token: token,
        }
    }

    #[test]
    fn accepts_native_eval_payload_and_rejects_tampered_content() {
        let valid = request("case-a");
        assert_eq!(valid.validate().unwrap(), AccessMode::ReadWrite);

        let mut tampered = valid;
        tampered.documents[0].content.push_str(" changed");
        let error = tampered.validate().unwrap_err().to_string();
        assert!(error.contains("content_sha256"));
    }

    #[test]
    fn rejects_unsafe_paths_and_duplicate_paths() {
        let mut unsafe_request = request("case-a");
        unsafe_request.documents[0].path = "../outside.md".to_owned();
        assert!(unsafe_request.validate().is_err());

        let mut duplicate = request("case-a");
        duplicate.documents.push(duplicate.documents[0].clone());
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn isolated_identity_is_stable_per_case_and_import_key() {
        let first = request("case-a");
        let same = request("case-a");
        let other = request("case-b");
        assert_eq!(isolated_external_ref(&first), isolated_external_ref(&same));
        assert_ne!(isolated_external_ref(&first), isolated_external_ref(&other));

        let first_token = derive_bearer_token(TEST_SECRET, &isolated_external_ref(&first)).unwrap();
        let same_token = derive_bearer_token(TEST_SECRET, &isolated_external_ref(&same)).unwrap();
        assert_eq!(first_token, same_token);
        assert!(first_token.starts_with("seval_"));
    }

    #[test]
    fn persisted_receipt_never_contains_bearer_token() {
        let safe = persisted_receipt(&response(Some("seval_do-not-store".to_owned()))).unwrap();
        let rendered = serde_json::to_string(&safe).unwrap();
        assert!(!rendered.contains("seval_do-not-store"));
        assert!(!rendered.contains("credential_token"));
        assert!(rendered.contains("issued_once"));
    }

    #[test]
    fn signed_status_locator_round_trips_and_rejects_tampering() {
        let locator = ImportLocator {
            version: 1,
            user_id: Uuid::now_v7(),
            credential_id: Uuid::now_v7(),
            scope_id: Uuid::now_v7(),
            receipt_id: Uuid::now_v7(),
            result_revision_id: Uuid::now_v7(),
            access_mode: "read_only".to_owned(),
        };
        let encoded = sign_import_locator(TEST_SECRET, &locator).unwrap();
        let decoded = verify_import_locator(TEST_SECRET, &encoded).unwrap();
        assert_eq!(decoded.user_id, locator.user_id);
        assert_eq!(decoded.receipt_id, locator.receipt_id);

        let mut tampered = encoded.into_bytes();
        let index = tampered.len() / 2;
        tampered[index] = if tampered[index] == b'a' { b'b' } else { b'a' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert!(verify_import_locator(TEST_SECRET, &tampered).is_err());
    }

    #[test]
    fn delta_revises_one_document_without_erasing_base_evidence() {
        let mut request = request("transition");
        request.delta_documents = vec![document("base/context.md", "# Context\nRevised")];
        request.seed_checkpoint = Some(EvalSeedCheckpoint {
            state: json!({"objective": "continue"}),
            source_refs: vec!["base/context.md".to_owned()],
        });
        request.validate().unwrap();
        let prepared = prepare_import(&request).unwrap();
        let base_document = prepared
            .base_members
            .values()
            .find(|member| member.kind == "document")
            .unwrap();
        let final_document = prepared
            .final_members
            .values()
            .find(|member| member.kind == "document")
            .unwrap();
        assert_eq!(base_document.version, Some(1));
        assert_eq!(final_document.version, Some(2));
        assert!(
            prepared
                .final_members
                .values()
                .any(|member| member.kind == "source_episode" && member.disposition == "superseded")
        );
        assert!(prepared.checkpoint_id.is_some());
    }

    #[test]
    fn manifest_hash_is_order_independent_and_disposition_sensitive() {
        let first_id = Uuid::now_v7();
        let second_id = Uuid::now_v7();
        let active = CorpusMember {
            kind: "chunk",
            version: None,
            disposition: "active",
        };
        let mut first = BTreeMap::new();
        first.insert(first_id, active.clone());
        first.insert(second_id, active.clone());
        let mut second = BTreeMap::new();
        second.insert(second_id, active.clone());
        second.insert(first_id, active);
        assert_eq!(manifest_hash(&first), manifest_hash(&second));
        second.get_mut(&first_id).unwrap().disposition = "superseded";
        assert_ne!(manifest_hash(&first), manifest_hash(&second));
    }
}

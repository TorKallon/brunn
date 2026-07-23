//! High-level, source-preserving capture compiled into the canonical save path.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{
        Capability, CaptureMode, CaptureRequest, CaptureSourceOrigin, ResponseStatus, SaveAction,
        SaveItem, SaveKind, SaveRequest, SourceReference, canonical_json,
    },
    refs::{RecordKind, RecordRef},
    write_service,
};

const CAPTURE_PROMPT_VERSION: &str = "prompt:source-bearing-capture-v1";
const CAPTURE_SCHEMA_VERSION: &str = "schema:source-bearing-capture-v1";
const MAX_CONTEXT_OBJECTS: i64 = 24;
const MAX_CONTEXT_CLAIMS: i64 = 96;
const MAX_CONTEXT_RELATIONS: i64 = 48;
const MIN_AUTO_COMMIT_CONFIDENCE: f64 = 0.90;

const CAPTURE_INSTRUCTIONS: &str = r#"You are Straylight's online memory formation compiler. Extract only durable information directly supported by the supplied source. Return no_op for routine chatter, assistant narration, duplicate recaps, or material with no durable value.

Produce a compact list of canonical save items. New records may use stable local refs beginning with capture:item_ so later items can refer to them. Use existing refs from context when identity is unambiguous. Never infer core.same_as, merge people from names or similarity, tombstone data, move scopes, or create policy. Preserve contradictions and uncertainty instead of choosing a winner.

Keep schedule, execution, participation response, attendance, notification, booking, payment, allocation, availability, use, work execution, and validation as independent state machines. Invitations, proposals, attempts, tool invocation, file presence, and plans do not prove acceptance, completion, validation, payment, delivery, or attendance.

Allowed item kinds are object, claim, relation, state_assignment, state_transition, temporal_spec, recurrence_spec, and checkpoint. Allowed actions are create, revise, supersede, and retract; create new relations with action=create. Each payload_json must be valid JSON for memory.save. Object payloads need a label and the exact JSON key type_profiles, whose value uses core.person, core.organization, core.group, core.place, core.event, core.arrangement, core.resource, core.work_item, core.artifact, or core.domain_object. Never rename type_profiles; use core.domain_object for generic subjects such as projects. Claims need about_refs, a namespaced predicate, value, support_state, and claim_mode. claim_mode is one of descriptive, preference, instruction, assessment, hypothesis, or proposal; asserted is a formation_method, not a claim_mode. Relations need a namespaced predicate and at least two role/ref endpoints. State items must use a registered state predicate and exact state value. Do not include source or evidence items; the server adds provenance and authority fields atomically.

For every item, copy an exact nonempty source_quote from the supplied source, set confidence conservatively, and put any consequential ambiguity in ambiguity. Use decision=commit only when every item is explicit, low risk, and fully grounded. Use decision=needs_review when identity, target, action, authority, supersession, recurrence, or state is consequentially ambiguous."#;

#[derive(Clone, Debug)]
pub struct CaptureOutcome {
    pub status: ResponseStatus,
    pub corpus_revision: Option<String>,
    pub data: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelCaptureItem {
    action: String,
    kind: String,
    reference: String,
    expected_version: i32,
    payload_json: String,
    source_quote: String,
    confidence: f64,
    ambiguity: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelCaptureOutput {
    decision: String,
    summary: String,
    items: Vec<ModelCaptureItem>,
    unresolved: Vec<String>,
}

#[derive(Clone, Debug)]
struct ModelRun {
    status: String,
    returned_model: Option<String>,
    response_id: Option<String>,
    usage: Value,
    degradation_reason: Option<String>,
    output: Option<ModelCaptureOutput>,
}

#[derive(Clone, Debug)]
struct CaptureCompilation {
    save_request: SaveRequest,
    issues: Vec<Value>,
    source_ref: String,
}

pub async fn capture(
    state: &AppState,
    auth: &AuthContext,
    request: CaptureRequest,
) -> ApiResult<CaptureOutcome> {
    auth.require(Capability::Save)?;
    request.validate()?;

    let content_hash = hex::encode(Sha256::digest(request.content.as_bytes()));
    if let Some(expected) = request.source.content_hash.as_deref()
        && expected.trim_start_matches("sha256:") != content_hash
    {
        return Err(ApiError::conflict(
            "content_hash_mismatch",
            "capture source content does not match its declared hash",
            json!({
                "expected": expected,
                "actual": format!("sha256:{content_hash}")
            }),
        ));
    }
    let request_hash = hash_value(&serde_json::to_value(&request)?)?;
    let (idempotency_key, conflict_prefix) = capture_idempotency(&request, &request_hash);

    if !auth.scope_refs.iter().any(|scope| scope == &request.scope) {
        return Err(ApiError::public(
            StatusCode::FORBIDDEN,
            "scope_denied",
            "credential does not grant the requested scope",
        ));
    }

    let mut tx = state.begin_read(auth).await?;
    let scope_row = sqlx::query(
        r#"
        SELECT scope_row.id AS scope_id,manifest.active_corpus_revision_id
        FROM straylight.scopes AS scope_row
        JOIN straylight.active_manifests AS manifest
          ON manifest.user_id=scope_row.user_id AND manifest.scope_id=scope_row.id
        WHERE scope_row.user_id=$1 AND scope_row.scope_ref=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(&request.scope)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", &request.scope))?;
    let scope_id: Uuid = scope_row.try_get("scope_id")?;
    let active_revision_id: Uuid = scope_row.try_get("active_corpus_revision_id")?;

    let replay_rows = sqlx::query(
        r#"
        SELECT key_row.idempotency_key,operation.status,operation.receipt
        FROM straylight.idempotency_keys AS key_row
        JOIN straylight.write_operations AS operation
          ON operation.user_id=key_row.user_id AND operation.id=key_row.operation_id
        WHERE key_row.user_id=$1 AND key_row.scope_id=$2
          AND key_row.idempotency_key LIKE ($3 || '%')
        ORDER BY operation.created_at DESC
        LIMIT 2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(&conflict_prefix)
    .fetch_all(&mut *tx)
    .await?;
    for row in &replay_rows {
        let existing_key: String = row.try_get("idempotency_key")?;
        if existing_key != idempotency_key {
            return Err(ApiError::conflict(
                "idempotency_conflict",
                "the capture idempotency key was already used for different content",
                json!({"idempotency_key_reused": true}),
            ));
        }
        if let Some(mut receipt) = row.try_get::<Option<Value>, _>("receipt")? {
            annotate_receipt(
                &mut receipt,
                "Idempotent replay of an already committed capture",
                &request_hash,
                true,
            );
            let source_ref = committed_source_ref(&receipt, request.source.reference.as_deref());
            if let (Some(object), Some(source_ref)) = (receipt.as_object_mut(), source_ref) {
                object.insert("source_ref".to_owned(), Value::String(source_ref));
            }
            let revision = receipt
                .get("corpus_revision")
                .and_then(Value::as_str)
                .map(str::to_owned);
            tx.commit().await?;
            return Ok(CaptureOutcome {
                status: ResponseStatus::Committed,
                corpus_revision: revision,
                data: receipt,
            });
        }
        let status: String = row.try_get("status")?;
        return Err(ApiError::conflict(
            "capture_in_progress",
            "an earlier capture with this idempotency key has not completed",
            json!({"status": status}),
        ));
    }

    if let Some(expected) = request.base_corpus_revision.as_deref() {
        let expected = parse_revision_ref(expected)?;
        if expected != active_revision_id {
            return Err(ApiError::conflict(
                "revision_conflict",
                "capture base corpus revision is stale",
                json!({
                    "expected": format!("revision:{expected}"),
                    "actual": format!("revision:{active_revision_id}")
                }),
            ));
        }
    }
    if let Some(reference) = request.source.reference.as_deref() {
        let source = RecordRef::parse(reference)
            .map_err(|_| ApiError::invalid("capture source ref must be a typed source reference"))?;
        if source.kind != RecordKind::SourceEpisode {
            return Err(ApiError::invalid(
                "capture source ref must identify a source episode",
            ));
        }
        let source_row = sqlx::query(
            r#"
            SELECT source.content_hash,
                   EXISTS (
                     SELECT 1
                     FROM straylight.evidence_items AS evidence
                     WHERE evidence.user_id=source.user_id
                       AND evidence.source_episode_id=source.id
                       AND evidence.text_content IS NOT NULL
                       AND strpos(evidence.text_content,$5) > 0
                   ) AS contains_text
            FROM straylight.corpus_members AS member
            JOIN straylight.source_episodes AS source
              ON source.user_id=member.user_id AND source.id=member.record_id
            WHERE member.user_id=$1 AND member.scope_id=$2
              AND member.corpus_revision_id=$3 AND member.record_id=$4
              AND member.record_kind='source_episode' AND member.disposition='active'
            "#,
        )
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(active_revision_id)
        .bind(source.id)
        .bind(&request.content)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(source_row) = source_row else {
            return Err(ApiError::not_found("source_not_found", reference));
        };
        let stored_hash: String = source_row.try_get("content_hash")?;
        let contains_text: bool = source_row.try_get("contains_text")?;
        if !contains_text && stored_hash != content_hash {
            return Err(ApiError::conflict(
                "source_content_mismatch",
                "capture content is not present in the referenced source",
                json!({"source_ref": reference}),
            ));
        }
    }
    let context = load_capture_context(
        &mut tx,
        auth.user_id.0,
        scope_id,
        active_revision_id,
        &request.root_refs,
        &request.content,
    )
    .await?;
    tx.commit().await?;

    let model_run = run_capture_model(state, auth, &request, &context).await;
    let model_receipt = model_receipt(&model_run, &state.config.capture_model);
    let Some(model_output) = model_run.output.as_ref() else {
        return Ok(CaptureOutcome {
            status: ResponseStatus::NeedsReview,
            corpus_revision: Some(format!("revision:{active_revision_id}")),
            data: json!({
                "status": "needs_review",
                "summary": "The source was not committed because structured extraction was unavailable.",
                "request_hash": format!("sha256:{request_hash}"),
                "source_hash": format!("sha256:{content_hash}"),
                "model": model_receipt,
                "issues": [{
                    "code": "structured_extraction_unavailable",
                    "message": model_run.degradation_reason
                }]
            }),
        });
    };

    if model_output.decision == "no_op" {
        return Ok(CaptureOutcome {
            status: ResponseStatus::NoOp,
            corpus_revision: Some(format!("revision:{active_revision_id}")),
            data: json!({
                "status": "no_op",
                "summary": model_output.summary,
                "unresolved": model_output.unresolved,
                "request_hash": format!("sha256:{request_hash}"),
                "source_hash": format!("sha256:{content_hash}"),
                "model": model_receipt
            }),
        });
    }

    let mut compilation = compile_capture(
        &request,
        model_output,
        &content_hash,
        idempotency_key,
        active_revision_id,
    );
    if model_output.decision != "commit" {
        compilation.issues.push(json!({
            "code": "model_requested_review",
            "message": "The extraction marked a consequential ambiguity."
        }));
    }
    if !model_output.unresolved.is_empty() {
        compilation.issues.push(json!({
            "code": "unresolved_capture_questions",
            "items": model_output.unresolved
        }));
    }
    if request.mode == CaptureMode::Draft {
        compilation.issues.push(json!({
            "code": "draft_mode",
            "message": "The caller requested extraction without commit."
        }));
    }

    if !compilation.issues.is_empty() {
        return Ok(CaptureOutcome {
            status: ResponseStatus::NeedsReview,
            corpus_revision: Some(format!("revision:{active_revision_id}")),
            data: json!({
                "status": "needs_review",
                "summary": model_output.summary,
                "source_ref": compilation.source_ref,
                "request_hash": format!("sha256:{request_hash}"),
                "source_hash": format!("sha256:{content_hash}"),
                "issues": compilation.issues,
                "unresolved": model_output.unresolved,
                "draft": {"save_request": compilation.save_request},
                "model": model_receipt
            }),
        });
    }

    let mut receipt = write_service::save(state, auth, compilation.save_request).await?;
    let committed_source_ref = committed_source_ref(
        &receipt,
        request.source.reference.as_deref(),
    )
    .unwrap_or(compilation.source_ref);
    annotate_receipt(
        &mut receipt,
        &model_output.summary,
        &request_hash,
        false,
    );
    if let Some(object) = receipt.as_object_mut() {
        object.insert("source_ref".to_owned(), Value::String(committed_source_ref));
        object.insert("model".to_owned(), model_receipt);
        object.insert(
            "unresolved".to_owned(),
            serde_json::to_value(&model_output.unresolved)?,
        );
    }
    let revision = receipt
        .get("corpus_revision")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(CaptureOutcome {
        status: ResponseStatus::Committed,
        corpus_revision: revision,
        data: receipt,
    })
}

fn capture_idempotency(request: &CaptureRequest, request_hash: &str) -> (String, String) {
    match request.idempotency_key.as_deref() {
        Some(key) => {
            let key_hash = hex::encode(Sha256::digest(key.as_bytes()));
            let prefix = format!("capture:{}:", &key_hash[..16]);
            (format!("{prefix}{request_hash}"), prefix)
        }
        None => {
            let prefix = format!("capture:auto:{request_hash}");
            (prefix.clone(), prefix)
        }
    }
}

async fn load_capture_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
    root_refs: &[String],
    content: &str,
) -> ApiResult<Value> {
    let mut root_ids = root_refs
        .iter()
        .filter_map(|reference| RecordRef::parse(reference).ok().map(|record| record.id))
        .collect::<Vec<_>>();
    root_ids.sort_unstable();
    root_ids.dedup();

    let object_rows = sqlx::query(
        r#"
        SELECT object.id,revision.version,revision.label,
               left(revision.properties::text,8000) AS properties,
               ARRAY(
                 SELECT profile.profile_ref
                 FROM straylight.object_revision_profiles AS profile
                 WHERE profile.user_id=revision.user_id
                   AND profile.object_id=revision.object_id
                   AND profile.object_version=revision.version
                 ORDER BY profile.profile_ref
               ) AS profiles
        FROM straylight.corpus_members AS member
        JOIN straylight.objects AS object
          ON object.user_id=member.user_id AND object.id=member.record_id
        JOIN straylight.object_revisions AS revision
          ON revision.user_id=object.user_id AND revision.object_id=object.id
         AND revision.version=member.record_version
        WHERE member.user_id=$1 AND member.scope_id=$2
          AND member.corpus_revision_id=$3 AND member.record_kind='object'
          AND member.disposition='active'
          AND (
            object.id=ANY($4::uuid[])
            OR to_tsvector('english',coalesce(revision.label,'')||' '||revision.properties::text)
               @@ websearch_to_tsquery('english',$5)
          )
        ORDER BY (object.id=ANY($4::uuid[])) DESC,revision.label NULLS LAST,object.id
        LIMIT $6
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(revision_id)
    .bind(&root_ids)
    .bind(content)
    .bind(MAX_CONTEXT_OBJECTS)
    .fetch_all(&mut **tx)
    .await?;
    let mut selected_object_ids = Vec::with_capacity(object_rows.len());
    let mut objects = Vec::with_capacity(object_rows.len());
    for row in object_rows {
        let id: Uuid = row.try_get("id")?;
        selected_object_ids.push(id);
        objects.push(json!({
            "ref": format!("object:{id}"),
            "version": row.try_get::<i32,_>("version")?,
            "label": row.try_get::<Option<String>,_>("label")?,
            "type_profiles": row.try_get::<Vec<String>,_>("profiles")?,
            "properties_json": row.try_get::<String,_>("properties")?
        }));
    }

    let claim_rows = if selected_object_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query(
            r#"
            SELECT DISTINCT claim.id,claim.predicate,left(claim.value::text,4000) AS value,
                   claim.claim_mode,claim.support_state,claim.authority,
                   claim.canonicality,claim.recorded_at
            FROM straylight.claims AS claim
            JOIN straylight.corpus_members AS member
              ON member.user_id=claim.user_id AND member.record_id=claim.id
             AND member.corpus_revision_id=$3 AND member.record_kind='claim'
             AND member.disposition='active'
            JOIN straylight.claim_about_targets AS target
              ON target.user_id=claim.user_id AND target.claim_id=claim.id
            WHERE claim.user_id=$1 AND claim.scope_id=$2
              AND target.target_record_id=ANY($4::uuid[])
            ORDER BY claim.recorded_at DESC,claim.id
            LIMIT $5
            "#,
        )
        .bind(user_id)
        .bind(scope_id)
        .bind(revision_id)
        .bind(&selected_object_ids)
        .bind(MAX_CONTEXT_CLAIMS)
        .fetch_all(&mut **tx)
        .await?
    };
    let claims = claim_rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            Ok(json!({
                "ref": format!("claim:{id}"),
                "predicate": row.try_get::<String,_>("predicate")?,
                "value_json": row.try_get::<String,_>("value")?,
                "claim_mode": row.try_get::<String,_>("claim_mode")?,
                "support_state": row.try_get::<String,_>("support_state")?,
                "authority": row.try_get::<String,_>("authority")?,
                "canonicality": row.try_get::<String,_>("canonicality")?,
                "recorded_at": row.try_get::<chrono::DateTime<chrono::Utc>,_>("recorded_at")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    let relation_rows = if selected_object_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query(
            r#"
            SELECT DISTINCT relation.id,revision.version,revision.predicate,
                   left(revision.qualifiers::text,4000) AS qualifiers,
                   ARRAY(
                     SELECT endpoint.role||'='||endpoint.target_record_id::text
                     FROM straylight.relation_endpoints AS endpoint
                     WHERE endpoint.user_id=revision.user_id
                       AND endpoint.relation_id=revision.relation_id
                       AND endpoint.relation_version=revision.version
                     ORDER BY endpoint.position
                   ) AS endpoints
            FROM straylight.relations AS relation
            JOIN straylight.corpus_members AS member
              ON member.user_id=relation.user_id AND member.record_id=relation.id
             AND member.corpus_revision_id=$3 AND member.record_kind='relation'
             AND member.disposition='active'
            JOIN straylight.relation_revisions AS revision
              ON revision.user_id=relation.user_id AND revision.relation_id=relation.id
             AND revision.version=member.record_version
            WHERE relation.user_id=$1 AND relation.scope_id=$2
              AND EXISTS (
                SELECT 1 FROM straylight.relation_endpoints AS endpoint
                WHERE endpoint.user_id=revision.user_id
                  AND endpoint.relation_id=revision.relation_id
                  AND endpoint.relation_version=revision.version
                  AND endpoint.target_record_id=ANY($4::uuid[])
              )
            ORDER BY relation.id
            LIMIT $5
            "#,
        )
        .bind(user_id)
        .bind(scope_id)
        .bind(revision_id)
        .bind(&selected_object_ids)
        .bind(MAX_CONTEXT_RELATIONS)
        .fetch_all(&mut **tx)
        .await?
    };
    let relations = relation_rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            Ok(json!({
                "ref": format!("relation:{id}"),
                "version": row.try_get::<i32,_>("version")?,
                "predicate": row.try_get::<String,_>("predicate")?,
                "qualifiers_json": row.try_get::<String,_>("qualifiers")?,
                "endpoints": row.try_get::<Vec<String>,_>("endpoints")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    Ok(json!({
        "corpus_revision": format!("revision:{revision_id}"),
        "root_refs": root_refs,
        "objects": objects,
        "claims": claims,
        "relations": relations,
        "context_is_bounded": true
    }))
}

async fn run_capture_model(
    state: &AppState,
    auth: &AuthContext,
    request: &CaptureRequest,
    context: &Value,
) -> ModelRun {
    let Some(api_key) = state.config.openai_api_key.as_deref() else {
        return degraded_model_run("OPENAI_API_KEY is unavailable");
    };
    let client = match Client::builder()
        .timeout(state.config.request_timeout)
        .build()
    {
        Ok(client) => client,
        Err(error) => return degraded_model_run(&format!("could not create OpenAI client: {error}")),
    };
    let input = json!({
        "intent": request.intent,
        "scope": request.scope,
        "root_refs": request.root_refs,
        "source": {
            "existing_ref": request.source.reference,
            "external_ref": request.source.external_ref,
            "title": request.source.title,
            "kind": request.source.kind,
            "origin": request.source.origin,
            "locator": request.source.locator,
        },
        "content": request.content,
        "bounded_current_context": context,
        "registered_state_machines": crate::models::STATE_REGISTRY,
    });
    let api_request = json!({
        "model": state.config.capture_model,
        "store": false,
        "safety_identifier": safety_identifier(
            &state.config.continuation_secret,
            auth.user_id.0,
        ),
        "instructions": CAPTURE_INSTRUCTIONS,
        "input": input.to_string(),
        "max_output_tokens": state.config.capture_max_output_tokens.clamp(512, 32_768),
        "text": {
            "format": {
                "type": "json_schema",
                "name": "straylight_source_capture",
                "strict": true,
                "schema": capture_output_schema()
            }
        }
    });
    let response = match client
        .post(format!(
            "{}/responses",
            state.config.openai_base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&api_request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return degraded_model_run(&format!("OpenAI request failed: {error}")),
    };
    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => return degraded_model_run(&format!("could not read OpenAI response: {error}")),
    };
    if !status.is_success() {
        let error_body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        let code = error_body
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let message = error_body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("OpenAI returned no structured error")
            .chars()
            .take(1_000)
            .collect::<String>();
        return degraded_model_run(&format!("OpenAI returned HTTP {status} ({code}): {message}"));
    }
    let response: Value = match serde_json::from_slice(&body) {
        Ok(response) => response,
        Err(error) => return degraded_model_run(&format!("OpenAI response was invalid JSON: {error}")),
    };
    let returned_model = response.get("model").and_then(Value::as_str).map(str::to_owned);
    let response_id = response.get("id").and_then(Value::as_str).map(str::to_owned);
    let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
    let mut refusal = None;
    let mut output_text = None;
    for content in response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
    {
        match content.get("type").and_then(Value::as_str) {
            Some("output_text") => {
                output_text = content.get("text").and_then(Value::as_str).map(str::to_owned)
            }
            Some("refusal") => {
                refusal = content.get("refusal").and_then(Value::as_str).map(str::to_owned)
            }
            _ => {}
        }
    }
    if let Some(refusal) = refusal {
        return ModelRun {
            status: "refused".to_owned(),
            returned_model,
            response_id,
            usage,
            degradation_reason: Some(refusal),
            output: None,
        };
    }
    let Some(output_text) = output_text else {
        return degraded_model_run("OpenAI response did not contain output_text");
    };
    match serde_json::from_str::<ModelCaptureOutput>(&output_text) {
        Ok(output) => ModelRun {
            status: "completed".to_owned(),
            returned_model,
            response_id,
            usage,
            degradation_reason: None,
            output: Some(output),
        },
        Err(error) => ModelRun {
            status: "invalid_output".to_owned(),
            returned_model,
            response_id,
            usage,
            degradation_reason: Some(format!("structured output was invalid: {error}")),
            output: None,
        },
    }
}

fn degraded_model_run(reason: &str) -> ModelRun {
    ModelRun {
        status: "degraded_unavailable".to_owned(),
        returned_model: None,
        response_id: None,
        usage: json!({}),
        degradation_reason: Some(reason.to_owned()),
        output: None,
    }
}

fn model_receipt(model: &ModelRun, requested_model: &str) -> Value {
    json!({
        "provider": "openai",
        "requested_model": requested_model,
        "returned_model": model.returned_model,
        "response_id": model.response_id,
        "status": model.status,
        "prompt_version": CAPTURE_PROMPT_VERSION,
        "schema_version": CAPTURE_SCHEMA_VERSION,
        "usage": model.usage,
        "degradation_reason": model.degradation_reason
    })
}

fn capture_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "decision": {"type": "string", "enum": ["commit", "no_op", "needs_review"]},
            "summary": {"type": "string"},
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "action": {"type": "string", "enum": ["create", "revise", "supersede", "retract"]},
                        "kind": {"type": "string", "enum": [
                            "object", "claim", "relation", "state_assignment",
                            "state_transition", "temporal_spec", "recurrence_spec", "checkpoint"
                        ]},
                        "reference": {"type": "string"},
                        "expected_version": {"type": "integer", "minimum": 0},
                        "payload_json": {"type": "string"},
                        "source_quote": {"type": "string"},
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                        "ambiguity": {"type": "string"}
                    },
                    "required": [
                        "action", "kind", "reference", "expected_version", "payload_json",
                        "source_quote", "confidence", "ambiguity"
                    ]
                }
            },
            "unresolved": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["decision", "summary", "items", "unresolved"]
    })
}

fn compile_capture(
    request: &CaptureRequest,
    output: &ModelCaptureOutput,
    content_hash: &str,
    idempotency_key: String,
    active_revision_id: Uuid,
) -> CaptureCompilation {
    let mut issues = Vec::new();
    if output.items.is_empty() {
        issues.push(json!({
            "code": "empty_capture",
            "message": "The extraction selected no durable items."
        }));
    }
    if output.items.len() > 64 {
        issues.push(json!({
            "code": "capture_item_limit_exceeded",
            "actual": output.items.len(),
            "maximum": 64
        }));
    }

    let source_ref = request
        .source
        .reference
        .clone()
        .unwrap_or_else(|| format!("capture:source_{}", &content_hash[..32]));
    let evidence_ref = format!("capture:evidence_{}", &content_hash[..32]);
    let mut items = Vec::with_capacity(output.items.len() + 2);
    if request.source.reference.is_none() {
        let mut metadata = request
            .source
            .metadata
            .clone()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        metadata.insert(
            "external_ref".to_owned(),
            request
                .source
                .external_ref
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        metadata.insert(
            "origin".to_owned(),
            serde_json::to_value(request.source.origin).unwrap_or(Value::Null),
        );
        items.push(SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::Source,
            reference: Some(source_ref.clone()),
            expected_version: None,
            payload: json!({
                "source_ref": source_ref,
                "source_kind": request.source.kind,
                "title": request.source.title,
                "path": format!("capture/{content_hash}.txt"),
                "media_type": request.source.media_type.as_deref().unwrap_or("text/plain"),
                "content": request.content,
                "content_hash": format!("sha256:{content_hash}"),
                "metadata": metadata
            }),
        });
    }
    items.push(SaveItem {
        action: SaveAction::Create,
        kind: SaveKind::Evidence,
        reference: Some(evidence_ref.clone()),
        expected_version: None,
        payload: json!({
            "source_ref": source_ref,
            "evidence_kind": "source_span",
            "locator": request.source.locator.clone().unwrap_or_else(|| json!({
                "start_char": 0,
                "end_char": request.content.len()
            })),
            "text_content": request.content,
            "source_text": request.content
        }),
    });

    let mut local_refs = BTreeSet::new();
    for (index, model_item) in output.items.iter().enumerate() {
        let reference = if model_item.reference.trim().is_empty() {
            format!("capture:item_{index}")
        } else {
            model_item.reference.trim().to_owned()
        };
        if model_item.action == "create" && !local_refs.insert(reference.clone()) {
            issues.push(json!({
                "code": "duplicate_local_ref",
                "item": index,
                "ref": reference
            }));
        }
    }

    for (index, model_item) in output.items.iter().take(64).enumerate() {
        let Some(action) = parse_action(&model_item.action) else {
            issues.push(json!({"code": "unsupported_action", "item": index}));
            continue;
        };
        let Some(kind) = parse_kind(&model_item.kind) else {
            issues.push(json!({"code": "unsupported_kind", "item": index}));
            continue;
        };
        let mut payload = match serde_json::from_str::<Value>(&model_item.payload_json) {
            Ok(Value::Object(payload)) => payload,
            Ok(_) => {
                issues.push(json!({"code": "payload_not_object", "item": index}));
                continue;
            }
            Err(error) => {
                issues.push(json!({
                    "code": "payload_json_invalid",
                    "item": index,
                    "message": error.to_string()
                }));
                continue;
            }
        };
        let quote = model_item.source_quote.trim();
        if quote.is_empty() || !request.content.contains(quote) {
            issues.push(json!({
                "code": "source_quote_not_exact",
                "item": index
            }));
        }
        if !model_item.confidence.is_finite()
            || model_item.confidence < MIN_AUTO_COMMIT_CONFIDENCE
        {
            issues.push(json!({
                "code": "capture_confidence_below_auto_commit",
                "item": index,
                "confidence": model_item.confidence,
                "minimum": MIN_AUTO_COMMIT_CONFIDENCE
            }));
        }
        if !model_item.ambiguity.trim().is_empty() {
            issues.push(json!({
                "code": "consequential_ambiguity",
                "item": index,
                "message": model_item.ambiguity
            }));
        }
        let reference = if model_item.reference.trim().is_empty() {
            format!("capture:item_{index}")
        } else {
            model_item.reference.trim().to_owned()
        };
        if action != SaveAction::Create && model_item.reference.trim().is_empty() {
            issues.push(json!({
                "code": "existing_target_required",
                "item": index,
                "action": model_item.action
            }));
        }
        if action == SaveAction::Revise
            && matches!(kind, SaveKind::Object | SaveKind::Relation)
            && model_item.expected_version <= 0
        {
            issues.push(json!({
                "code": "expected_version_required",
                "item": index
            }));
        }

        let span = request.content.find(quote).map(|start| [start, start + quote.len()]);
        normalize_payload(
            &mut payload,
            kind,
            request.source.origin,
            &source_ref,
            &evidence_ref,
            quote,
            span,
            model_item.confidence,
        );
        if kind == SaveKind::Relation {
            match payload.get("predicate").and_then(Value::as_str) {
                Some("core.same_as") | Some("core.possibly_same_as") => {
                    issues.push(json!({
                        "code": "identity_review_required",
                        "item": index,
                        "predicate": payload.get("predicate")
                    }));
                }
                _ => {}
            }
        }
        if matches!(kind, SaveKind::StateAssignment | SaveKind::StateTransition)
            && completion_without_evidence(&request.content, &payload)
        {
            issues.push(json!({
                "code": "attempt_does_not_prove_completion",
                "item": index,
                "predicate": payload.get("predicate"),
                "value": payload.get("value")
            }));
        }

        let item = SaveItem {
            action,
            kind,
            reference: Some(reference),
            expected_version: (model_item.expected_version > 0)
                .then_some(model_item.expected_version),
            payload: Value::Object(payload),
        };
        if let Err(error) = item.validate() {
            issues.push(json!({
                "code": "canonical_item_validation_failed",
                "item": index,
                "message": error.to_string()
            }));
        }
        items.push(item);
    }

    let save_request = SaveRequest {
        intent: request
            .intent
            .clone()
            .unwrap_or_else(|| format!("capture:{}", request.source.kind)),
        scope: request.scope.clone(),
        root_refs: request.root_refs.clone(),
        source_refs: vec![SourceReference {
            reference: source_ref.clone(),
            span: Some([0, request.content.len()]),
            content_hash: Some(format!("sha256:{content_hash}")),
        }],
        items,
        idempotency_key,
        operation_id: None,
        base_corpus_revision: Some(format!("revision:{active_revision_id}")),
        confirmation_token: None,
    };
    if let Err(error) = save_request.validate() {
        issues.push(json!({
            "code": "canonical_save_validation_failed",
            "message": error.to_string()
        }));
    }
    CaptureCompilation {
        save_request,
        issues,
        source_ref,
    }
}

fn normalize_payload(
    payload: &mut Map<String, Value>,
    kind: SaveKind,
    origin: CaptureSourceOrigin,
    source_ref: &str,
    evidence_ref: &str,
    source_quote: &str,
    source_span: Option<[usize; 2]>,
    confidence: f64,
) {
    if kind == SaveKind::Object {
        normalize_object_profiles(payload);
    }
    payload.insert("source_ref".to_owned(), Value::String(source_ref.to_owned()));
    payload.insert("source_text".to_owned(), Value::String(source_quote.to_owned()));
    if let Some(span) = source_span {
        payload.insert("source_span".to_owned(), json!(span));
    }
    if matches!(kind, SaveKind::Claim | SaveKind::StateAssignment | SaveKind::StateTransition) {
        normalize_claim_mode(payload, kind);
        payload.insert(
            "producer_ref".to_owned(),
            Value::String(source_ref.to_owned()),
        );
        payload.insert("evidence_refs".to_owned(), json!([evidence_ref]));
        payload.insert("confidence".to_owned(), json!(confidence));
        let claim_mode = payload
            .get("claim_mode")
            .and_then(Value::as_str)
            .unwrap_or("descriptive");
        let (formation, authority, canonicality) = match origin {
            CaptureSourceOrigin::User => (
                "asserted",
                "user_explicit",
                if matches!(claim_mode, "hypothesis" | "proposal" | "assessment") {
                    "provisional"
                } else {
                    "canonical"
                },
            ),
            CaptureSourceOrigin::External => ("extracted", "source_reported", "reported"),
            CaptureSourceOrigin::Agent => ("synthesized", "agent_generated", "derived"),
            CaptureSourceOrigin::Tool => ("action_result", "tool_output", "reported"),
            CaptureSourceOrigin::System => ("observed", "system_observed", "reported"),
        };
        payload.insert("formation_method".to_owned(), Value::String(formation.to_owned()));
        payload.insert("authority".to_owned(), Value::String(authority.to_owned()));
        payload.insert(
            "canonicality".to_owned(),
            Value::String(canonicality.to_owned()),
        );
        payload
            .entry("support_state".to_owned())
            .or_insert_with(|| Value::String("reported".to_owned()));
    } else if kind == SaveKind::Relation {
        payload.insert("evidence_refs".to_owned(), json!([evidence_ref]));
    }
}

fn normalize_object_profiles(payload: &mut Map<String, Value>) {
    let profiles = payload
        .get("type_profiles")
        .or_else(|| payload.get("seeded_type_profiles"))
        .or_else(|| payload.get("profiles"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    payload.remove("seeded_type_profiles");
    payload.remove("profiles");
    let mut normalized = BTreeSet::new();
    let mut hints = BTreeSet::new();
    for profile in profiles.iter().filter_map(Value::as_str) {
        let profile = profile.trim();
        if profile.is_empty() {
            continue;
        }
        if profile.contains('.') {
            normalized.insert(profile.to_owned());
            continue;
        }
        hints.insert(profile.to_owned());
        let core_profile = match profile.to_ascii_lowercase().as_str() {
            "person" => "core.person",
            "organization" | "organisation" | "company" => "core.organization",
            "group" | "team" | "family" => "core.group",
            "place" | "location" | "venue" => "core.place",
            "event" => "core.event",
            "arrangement" | "reservation" | "booking" => "core.arrangement",
            "resource" => "core.resource",
            "work_item" | "task" | "todo" | "action_item" => "core.work_item",
            "artifact" | "document" | "file" | "decision" | "decision_record" => {
                "core.artifact"
            }
            "event_series" => "core.event_series",
            "event_occurrence" => "core.event_occurrence",
            _ => "core.domain_object",
        };
        normalized.insert(core_profile.to_owned());
    }
    if normalized.is_empty() {
        normalized.insert("core.domain_object".to_owned());
    }
    payload.insert(
        "type_profiles".to_owned(),
        Value::Array(normalized.into_iter().map(Value::String).collect()),
    );
    if !hints.is_empty() {
        payload.insert(
            "capture_type_hints".to_owned(),
            Value::Array(hints.into_iter().map(Value::String).collect()),
        );
    }
}

fn normalize_claim_mode(payload: &mut Map<String, Value>, kind: SaveKind) {
    let normalized = match kind {
        SaveKind::StateAssignment => Some("state_assignment"),
        SaveKind::StateTransition => Some("state_transition"),
        SaveKind::Claim => match payload.get("claim_mode").and_then(Value::as_str) {
            None
            | Some("asserted" | "extracted" | "observed" | "inferred" | "synthesized") => {
                Some("descriptive")
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(normalized) = normalized {
        payload.insert(
            "claim_mode".to_owned(),
            Value::String(normalized.to_owned()),
        );
    }
}

fn parse_action(value: &str) -> Option<SaveAction> {
    match value {
        "create" => Some(SaveAction::Create),
        "revise" => Some(SaveAction::Revise),
        "supersede" => Some(SaveAction::Supersede),
        "retract" => Some(SaveAction::Retract),
        _ => None,
    }
}

fn parse_kind(value: &str) -> Option<SaveKind> {
    match value {
        "object" => Some(SaveKind::Object),
        "claim" => Some(SaveKind::Claim),
        "relation" => Some(SaveKind::Relation),
        "state_assignment" => Some(SaveKind::StateAssignment),
        "state_transition" => Some(SaveKind::StateTransition),
        "temporal_spec" => Some(SaveKind::TemporalSpec),
        "recurrence_spec" => Some(SaveKind::RecurrenceSpec),
        "checkpoint" => Some(SaveKind::Checkpoint),
        _ => None,
    }
}

fn completion_without_evidence(content: &str, payload: &Map<String, Value>) -> bool {
    let Some(value) = payload.get("value").and_then(Value::as_str) else {
        return false;
    };
    const COMPLETION_STATES: &[&str] = &[
        "completed",
        "passed",
        "paid",
        "confirmed",
        "delivered",
        "acknowledged",
        "attended",
        "used",
        "allocated",
    ];
    if !COMPLETION_STATES.contains(&value) {
        return false;
    }
    let content = content.to_lowercase();
    let attempted = [
        "attempted",
        "tried to",
        "started to",
        "submitted",
        "invited",
        "proposed",
        "planned to",
    ]
    .iter()
    .any(|marker| content.contains(marker));
    let explicit_completion = [
        "completed",
        "finished",
        "passed",
        "paid",
        "confirmed",
        "delivered",
        "acknowledged",
        "attended",
        "booked",
        "done",
        "succeeded",
    ]
    .iter()
    .any(|marker| content.contains(marker));
    attempted && !explicit_completion
}

fn annotate_receipt(receipt: &mut Value, summary: &str, request_hash: &str, replayed: bool) {
    if let Some(object) = receipt.as_object_mut() {
        object.insert("capture_status".to_owned(), Value::String("committed".to_owned()));
        object.insert("capture_summary".to_owned(), Value::String(summary.to_owned()));
        object.insert(
            "capture_request_hash".to_owned(),
            Value::String(format!("sha256:{request_hash}")),
        );
        object.insert("capture_replayed".to_owned(), Value::Bool(replayed));
    }
}

fn committed_source_ref(receipt: &Value, existing_source_ref: Option<&str>) -> Option<String> {
    receipt
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("kind").and_then(Value::as_str) == Some("source"))
                    .then(|| item.get("ref").and_then(Value::as_str).map(str::to_owned))
                    .flatten()
            })
        })
        .or_else(|| existing_source_ref.map(str::to_owned))
}

fn parse_revision_ref(value: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.trim_start_matches("revision:"))
        .map_err(|_| ApiError::invalid("base_corpus_revision is invalid"))
}

fn hash_value(value: &Value) -> ApiResult<String> {
    Ok(hex::encode(Sha256::digest(canonical_json(value)?.as_bytes())))
}

fn safety_identifier(secret: &str, user_id: Uuid) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts continuation secrets of any length");
    mac.update(b"straylight:capture-safety-identifier:v1:");
    mac.update(user_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CaptureSource, CaptureSourceOrigin};

    fn request(content: &str, origin: CaptureSourceOrigin) -> CaptureRequest {
        CaptureRequest {
            content: content.to_owned(),
            source: CaptureSource {
                title: Some("Test source".to_owned()),
                origin,
                ..CaptureSource::default()
            },
            scope: "scope:test".to_owned(),
            root_refs: vec![],
            intent: Some("remember_this".to_owned()),
            idempotency_key: Some("capture-test".to_owned()),
            base_corpus_revision: None,
            mode: CaptureMode::Auto,
        }
    }

    fn item(kind: &str, payload: Value, quote: &str) -> ModelCaptureItem {
        ModelCaptureItem {
            action: "create".to_owned(),
            kind: kind.to_owned(),
            reference: "capture:item_subject".to_owned(),
            expected_version: 0,
            payload_json: payload.to_string(),
            source_quote: quote.to_owned(),
            confidence: 0.98,
            ambiguity: String::new(),
        }
    }

    #[test]
    fn low_risk_user_capture_compiles_to_source_evidence_and_canonical_items() {
        let request = request("The Switzerland trip starts on August 12.", CaptureSourceOrigin::User);
        let output = ModelCaptureOutput {
            decision: "commit".to_owned(),
            summary: "Trip start date".to_owned(),
            items: vec![
                item(
                    "object",
                    json!({
                        "type_profiles": ["core.domain_object"],
                        "label": "Switzerland trip"
                    }),
                    "Switzerland trip",
                ),
                ModelCaptureItem {
                    reference: "capture:item_start-date".to_owned(),
                    ..item(
                        "claim",
                        json!({
                            "about_refs": ["capture:item_subject"],
                            "predicate": "travel.start_date",
                            "value": "2026-08-12",
                            "producer_ref": "source:placeholder",
                            "formation_method": "asserted",
                            "claim_mode": "descriptive",
                            "support_state": "reported",
                            "authority": "placeholder",
                            "canonicality": "placeholder"
                        }),
                        "starts on August 12",
                    )
                },
            ],
            unresolved: vec![],
        };
        let compilation = compile_capture(
            &request,
            &output,
            &hex::encode(Sha256::digest(request.content.as_bytes())),
            "capture:test".to_owned(),
            Uuid::nil(),
        );
        assert!(compilation.issues.is_empty(), "{:?}", compilation.issues);
        assert_eq!(compilation.save_request.items.len(), 4);
        let claim = &compilation.save_request.items[3];
        assert_eq!(claim.payload["authority"], "user_explicit");
        assert_eq!(claim.payload["canonicality"], "canonical");
        assert_eq!(claim.payload["producer_ref"], compilation.source_ref);
        assert_eq!(claim.payload["evidence_refs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn compiler_repairs_mechanical_profile_and_claim_mode_errors() {
        let mut object = json!({
            "seeded_type_profiles": ["project", "decision_record"],
            "label": "Straylight"
        })
        .as_object()
        .unwrap()
        .clone();
        normalize_payload(
            &mut object,
            SaveKind::Object,
            CaptureSourceOrigin::User,
            "capture:source_test",
            "capture:evidence_test",
            "Straylight",
            Some([0, 10]),
            0.99,
        );
        assert_eq!(
            object["type_profiles"],
            json!(["core.artifact", "core.domain_object"])
        );
        assert_eq!(
            object["capture_type_hints"],
            json!(["decision_record", "project"])
        );
        assert!(!object.contains_key("seeded_type_profiles"));

        let mut claim = json!({
            "about_refs": ["capture:item_project"],
            "predicate": "project.backend_language",
            "value": "Rust",
            "claim_mode": "asserted"
        })
        .as_object()
        .unwrap()
        .clone();
        normalize_payload(
            &mut claim,
            SaveKind::Claim,
            CaptureSourceOrigin::User,
            "capture:source_test",
            "capture:evidence_test",
            "is Rust",
            Some([0, 7]),
            0.99,
        );
        assert_eq!(claim["claim_mode"], "descriptive");
        assert_eq!(claim["formation_method"], "asserted");
    }

    #[test]
    fn identity_equivalence_never_auto_commits() {
        let request = request("Pat Lee may be the same person as P. Lee.", CaptureSourceOrigin::User);
        let output = ModelCaptureOutput {
            decision: "commit".to_owned(),
            summary: "Possible identity".to_owned(),
            items: vec![item(
                "relation",
                json!({
                    "predicate": "core.possibly_same_as",
                    "endpoints": [
                        {"role": "left", "ref": "object:00000000-0000-0000-0000-000000000001"},
                        {"role": "right", "ref": "object:00000000-0000-0000-0000-000000000002"}
                    ]
                }),
                "may be the same person",
            )],
            unresolved: vec![],
        };
        let compilation = compile_capture(
            &request,
            &output,
            &hex::encode(Sha256::digest(request.content.as_bytes())),
            "capture:test".to_owned(),
            Uuid::nil(),
        );
        assert!(
            compilation
                .issues
                .iter()
                .any(|issue| issue["code"] == "identity_review_required")
        );
    }

    #[test]
    fn an_attempt_does_not_become_completed_work() {
        let request = request(
            "I attempted the migration but it timed out.",
            CaptureSourceOrigin::User,
        );
        let output = ModelCaptureOutput {
            decision: "commit".to_owned(),
            summary: "Migration state".to_owned(),
            items: vec![item(
                "state_assignment",
                json!({
                    "about_refs": ["object:00000000-0000-0000-0000-000000000001"],
                    "predicate": "state:work.execution@v1",
                    "value": "completed",
                    "producer_ref": "source:placeholder",
                    "formation_method": "asserted",
                    "claim_mode": "state_assignment",
                    "support_state": "reported",
                    "authority": "placeholder",
                    "canonicality": "placeholder"
                }),
                "attempted the migration",
            )],
            unresolved: vec![],
        };
        let compilation = compile_capture(
            &request,
            &output,
            &hex::encode(Sha256::digest(request.content.as_bytes())),
            "capture:test".to_owned(),
            Uuid::nil(),
        );
        assert!(
            compilation
                .issues
                .iter()
                .any(|issue| issue["code"] == "attempt_does_not_prove_completion")
        );
    }

    #[test]
    fn external_sources_cannot_assign_user_authority() {
        let mut payload = Map::new();
        payload.insert("claim_mode".to_owned(), Value::String("descriptive".to_owned()));
        normalize_payload(
            &mut payload,
            SaveKind::Claim,
            CaptureSourceOrigin::External,
            "source:test",
            "evidence:test",
            "reported fact",
            Some([0, 13]),
            0.95,
        );
        assert_eq!(payload["authority"], "source_reported");
        assert_eq!(payload["canonicality"], "reported");
        assert_eq!(payload["formation_method"], "extracted");
    }
}

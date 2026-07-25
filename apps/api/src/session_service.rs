use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::{Capability, OpenRequest},
    policy::{self, PolicyDocument, ProjectionReceipt, ScopedPolicy},
    refs::RecordRef,
    telemetry, usage_service,
};

const SESSION_TTL_HOURS: i64 = 24;
const PROJECTION_AUDIENCE: &str = "principal:self";
const PROJECTION_PURPOSE: &str = "task_reasoning";

#[derive(Clone, Debug)]
struct LoadedPolicyRevision {
    id: Uuid,
    document: PolicyDocument,
    snapshot_hash: String,
}

#[derive(Debug)]
struct ProjectionContext {
    scope_id: Uuid,
    source_corpus_revision_id: Uuid,
    subject_ref: String,
    primary_policy: LoadedPolicyRevision,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RecordOccurrence {
    record_id: Uuid,
    path: String,
}

pub async fn project_response(
    state: &AppState,
    auth: &AuthContext,
    subject_ref: &str,
    operation: &'static str,
    action: &'static str,
    data: Value,
) -> ApiResult<Value> {
    let started = Instant::now();
    if data.get("projection").is_some() {
        return Err(ApiError::Internal(
            "memory response already contains a top-level projection field".to_owned(),
        ));
    }

    let occurrences = collect_record_occurrences(&data);
    let mut tx = state.begin_write(auth).await?;
    let context = load_projection_context(&mut tx, auth, subject_ref).await?;
    let scoped_policies =
        load_scoped_policies(&mut tx, auth.user_id.0, context.scope_id, &occurrences).await?;
    let projection = policy::project_scoped(
        data,
        std::slice::from_ref(&context.primary_policy.document),
        &scoped_policies,
        PROJECTION_AUDIENCE,
        PROJECTION_PURPOSE,
        action,
    )?;

    let receipt_id = Uuid::now_v7();
    let audit_id = Uuid::now_v7();
    let output_hash = projection
        .receipt
        .output_hash
        .strip_prefix("sha256:")
        .ok_or_else(|| {
            ApiError::Internal("projection output hash lacks sha256 prefix".to_owned())
        })?;
    let generated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        INSERT INTO straylight.projection_receipts (
          id,user_id,scope_id,source_corpus_revision_id,policy_id,policy_version,
          audience,purpose,included_paths,withheld,transforms,output_hash
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
        RETURNING generated_at
        "#,
    )
    .bind(receipt_id)
    .bind(auth.user_id.0)
    .bind(context.scope_id)
    .bind(context.source_corpus_revision_id)
    .bind(context.primary_policy.id)
    .bind(context.primary_policy.document.version)
    .bind(PROJECTION_AUDIENCE)
    .bind(PROJECTION_PURPOSE)
    .bind(&projection.receipt.included_paths)
    .bind(serde_json::to_value(&projection.receipt.withheld)?)
    .bind(&projection.receipt.transforms)
    .bind(output_hash)
    .fetch_one(&mut *tx)
    .await?;

    let audit_details =
        projection_audit_details(receipt_id, operation, action, &context, &projection.receipt);
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,actor_ref,action,details,content_free
        ) VALUES ($1,$2,$3,$4,$5,'memory.project',$6,true)
        "#,
    )
    .bind(audit_id)
    .bind(auth.user_id.0)
    .bind(context.scope_id)
    .bind(auth.credential_id.0)
    .bind(format!("credential:{}", auth.credential_id.0))
    .bind(audit_details)
    .execute(&mut *tx)
    .await?;

    let data = attach_projection_receipt(
        projection.data,
        &projection.receipt,
        receipt_id,
        audit_id,
        context.source_corpus_revision_id,
        generated_at,
    )?;
    tx.commit().await?;

    metrics::counter!(
        "projection.responses",
        "operation" => operation,
        "action" => action
    )
    .increment(1);
    metrics::histogram!(
        "projection.duration_ms",
        "operation" => operation,
        "action" => action
    )
    .record(telemetry::elapsed_ms(started));
    metrics::histogram!(
        "projection.input_record_occurrences",
        "operation" => operation
    )
    .record(occurrences.len() as f64);
    metrics::histogram!(
        "projection.included_paths",
        "operation" => operation
    )
    .record(projection.receipt.included_paths.len() as f64);
    metrics::histogram!(
        "projection.withheld_paths",
        "operation" => operation
    )
    .record(projection.receipt.withheld.len() as f64);
    metrics::histogram!(
        "projection.transforms",
        "operation" => operation
    )
    .record(projection.receipt.transforms.len() as f64);

    if let Ok(session_id) = parse_session_ref(&context.subject_ref)
        && let Err(error) = usage_service::record_projection_accesses(
            state,
            auth,
            context.scope_id,
            session_id,
            context.source_corpus_revision_id,
            receipt_id,
            operation,
            &data,
        )
        .await
    {
        metrics::counter!(
            "usage_tracking.errors",
            "operation" => operation
        )
        .increment(1);
        tracing::warn!(
            operation,
            projection_receipt = %receipt_id,
            error = %error,
            "post-policy record access telemetry was not persisted"
        );
    }
    Ok(data)
}

pub async fn provision(
    state: &AppState,
    auth: &AuthContext,
    request: &OpenRequest,
) -> ApiResult<String> {
    auth.require(Capability::Open)?;
    request.validate()?;
    let scope_ref = select_scope(auth, request)?;
    let mut tx = state.begin_write(auth).await?;
    let scope_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.scopes WHERE user_id=$1 AND scope_ref=$2",
    )
    .bind(auth.user_id.0)
    .bind(&scope_ref)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", &scope_ref))?;

    let (corpus_revision_id, manifest_hash) =
        resolve_revision(&mut tx, auth.user_id.0, scope_id, &request.as_of).await?;
    let (resumed_checkpoint_id, parent_session_id) = resolve_resume(
        &mut tx,
        auth.user_id.0,
        scope_id,
        request.resume_checkpoint_ref.as_deref(),
    )
    .await?;
    let policy_projection_hash = policy_projection_hash(&mut tx, auth.user_id.0).await?;

    let session_id = Uuid::now_v7();
    let task_hash = hex::encode(Sha256::digest(request.task.as_bytes()));
    let mut capabilities: Vec<_> = auth.capabilities.iter().cloned().collect();
    capabilities.sort();
    let mode = if auth.read_only {
        "read_only"
    } else {
        "read_write"
    };
    sqlx::query(
        r#"
        INSERT INTO straylight.sessions (
          id, user_id, scope_id, credential_id, corpus_revision_id,
          parent_session_id, task_hash, task_size_bytes, capability_snapshot,
          authorization_scope_refs, policy_projection_hash, mode,
          expires_at, resumed_checkpoint_id
        ) VALUES (
          $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14
        )
        "#,
    )
    .bind(session_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(corpus_revision_id)
    .bind(parent_session_id)
    .bind(task_hash)
    .bind(request.task.len() as i32)
    .bind(&capabilities)
    .bind(vec![scope_ref.clone()])
    .bind(policy_projection_hash)
    .bind(mode)
    .bind(Utc::now() + Duration::hours(SESSION_TTL_HOURS))
    .bind(resumed_checkpoint_id)
    .execute(&mut *tx)
    .await?;

    let roots: BTreeSet<_> = request
        .hints
        .root_refs
        .iter()
        .chain(&request.hints.open_object_refs)
        .cloned()
        .collect();
    for (position, root) in roots.iter().enumerate() {
        let record = RecordRef::parse(root)?;
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM straylight.record_keys AS key
              JOIN straylight.corpus_members AS member
                ON member.user_id=key.user_id AND member.record_id=key.record_id
              WHERE key.user_id=$1 AND key.scope_id=$2 AND key.record_id=$3
                AND member.corpus_revision_id=$4
            )
            "#,
        )
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(record.id)
        .bind(corpus_revision_id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(ApiError::not_found("ref_not_found", root));
        }
        sqlx::query(
            "INSERT INTO straylight.session_root_refs (user_id,session_id,position,record_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(auth.user_id.0)
        .bind(session_id)
        .bind(position as i32)
        .bind(record.id)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,actor_ref,action,details
        ) VALUES ($1,$2,$3,$4,$5,'session.open',$6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(format!("credential:{}", auth.credential_id.0))
    .bind(json!({
        "session_id": format!("session:{session_id}"),
        "corpus_revision": format!("revision:{corpus_revision_id}"),
        "manifest_hash": manifest_hash,
        "resumed_checkpoint": resumed_checkpoint_id.map(|id| format!("checkpoint:{id}")),
        "mode": mode,
        "root_count": roots.len()
    }))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    metrics::counter!(
        "session.created",
        "mode" => mode,
        "resumed" => if resumed_checkpoint_id.is_some() { "true" } else { "false" }
    )
    .increment(1);
    metrics::histogram!("session.root_count", "mode" => mode).record(roots.len() as f64);
    metrics::histogram!("session.task_bytes", "mode" => mode).record(request.task.len() as f64);
    Ok(format!("session:{session_id}"))
}

pub async fn refresh(state: &AppState, auth: &AuthContext, session_ref: &str) -> ApiResult<String> {
    auth.require(Capability::Open)?;
    let raw = session_ref.strip_prefix("session:").unwrap_or(session_ref);
    let previous_session_id =
        Uuid::parse_str(raw).map_err(|_| ApiError::invalid("invalid session reference"))?;
    let mut tx = state.begin_write(auth).await?;
    let previous = sqlx::query(
        r#"
        SELECT session.scope_id,scope.scope_ref,session.task_hash,
               session.task_size_bytes,session.resumed_checkpoint_id,
               session.corpus_revision_id
        FROM straylight.sessions AS session
        JOIN straylight.scopes AS scope
          ON scope.user_id=session.user_id AND scope.id=session.scope_id
        WHERE session.user_id=$1 AND session.id=$2
          AND session.credential_id=$3
          AND scope.scope_ref=ANY($4::text[])
        "#,
    )
    .bind(auth.user_id.0)
    .bind(previous_session_id)
    .bind(auth.credential_id.0)
    .bind(&auth.scope_refs)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", session_ref))?;
    let scope_id: Uuid = previous.try_get("scope_id")?;
    let scope_ref: String = previous.try_get("scope_ref")?;
    let latest_revision_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT active_corpus_revision_id FROM straylight.active_manifests WHERE user_id=$1 AND scope_id=$2",
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("corpus_not_found", &scope_ref))?;
    let latest_checkpoint = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.checkpoints WHERE user_id=$1 AND session_id=$2 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(auth.user_id.0)
    .bind(previous_session_id)
    .fetch_optional(&mut *tx)
    .await?
    .or(previous.try_get::<Option<Uuid>, _>("resumed_checkpoint_id")?);
    let policy_projection_hash = policy_projection_hash(&mut tx, auth.user_id.0).await?;
    let mut capabilities: Vec<_> = auth.capabilities.iter().cloned().collect();
    capabilities.sort();
    let mode = if auth.read_only {
        "read_only"
    } else {
        "read_write"
    };
    let new_session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.sessions (
          id,user_id,scope_id,credential_id,corpus_revision_id,parent_session_id,
          task_hash,task_size_bytes,capability_snapshot,authorization_scope_refs,
          policy_projection_hash,mode,expires_at,resumed_checkpoint_id
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        "#,
    )
    .bind(new_session_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(latest_revision_id)
    .bind(previous_session_id)
    .bind(previous.try_get::<String, _>("task_hash")?)
    .bind(previous.try_get::<i32, _>("task_size_bytes")?)
    .bind(&capabilities)
    .bind(vec![scope_ref.clone()])
    .bind(policy_projection_hash)
    .bind(mode)
    .bind(Utc::now() + Duration::hours(SESSION_TTL_HOURS))
    .bind(latest_checkpoint)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.session_root_refs (user_id,session_id,position,record_id)
        SELECT user_id,$1,position,record_id
        FROM straylight.session_root_refs
        WHERE user_id=$2 AND session_id=$3
        "#,
    )
    .bind(new_session_id)
    .bind(auth.user_id.0)
    .bind(previous_session_id)
    .execute(&mut *tx)
    .await?;
    let previous_revision_id: Uuid = previous.try_get("corpus_revision_id")?;
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,actor_ref,action,details
        ) VALUES ($1,$2,$3,$4,$5,'session.refresh',$6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(format!("credential:{}", auth.credential_id.0))
    .bind(json!({
        "session_id": format!("session:{new_session_id}"),
        "parent_session_id": format!("session:{previous_session_id}"),
        "from_revision": format!("revision:{previous_revision_id}"),
        "to_revision": format!("revision:{latest_revision_id}"),
        "resumed_checkpoint": latest_checkpoint.map(|id| format!("checkpoint:{id}"))
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    metrics::counter!(
        "session.refreshed",
        "mode" => mode,
        "revision_changed" => if previous_revision_id != latest_revision_id {
            "true"
        } else {
            "false"
        },
        "checkpoint_resumed" => if latest_checkpoint.is_some() { "true" } else { "false" }
    )
    .increment(1);
    Ok(format!("session:{new_session_id}"))
}

async fn load_projection_context(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    subject_ref: &str,
) -> ApiResult<ProjectionContext> {
    if subject_ref.starts_with("stage:") {
        load_stage_projection_context(tx, auth, subject_ref).await
    } else {
        load_session_projection_context(tx, auth, subject_ref).await
    }
}

async fn load_session_projection_context(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    session_ref: &str,
) -> ApiResult<ProjectionContext> {
    let session_id = parse_session_ref(session_ref)?;
    let row = sqlx::query(
        r#"
        SELECT session.scope_id,session.corpus_revision_id,
               session.policy_projection_hash
        FROM straylight.sessions AS session
        JOIN straylight.scopes AS scope
          ON scope.user_id=session.user_id AND scope.id=session.scope_id
        WHERE session.user_id=$1 AND session.id=$2
          AND scope.scope_ref=ANY($3::text[])
          AND session.expires_at > clock_timestamp()
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(&auth.scope_refs)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", session_ref))?;
    let primary_policy = load_default_policy_revision(tx, auth.user_id.0).await?;
    let session_hash: String = row.try_get("policy_projection_hash")?;
    ensure_policy_snapshot_matches(
        &session_hash,
        &primary_policy.snapshot_hash,
        &format!("session:{}", session_id.simple()),
    )?;
    Ok(ProjectionContext {
        scope_id: row.try_get("scope_id")?,
        source_corpus_revision_id: row.try_get("corpus_revision_id")?,
        subject_ref: format!("session:{}", session_id.simple()),
        primary_policy,
    })
}

async fn load_stage_projection_context(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    stage_ref: &str,
) -> ApiResult<ProjectionContext> {
    let (stage_id, requested_inventory_hash) = parse_stage_ref(stage_ref)?;
    let row = sqlx::query(
        r#"
        SELECT stage.scope_id,stage.base_corpus_revision_id,stage.inventory_hash,
               policy.id AS policy_id,policy.policy_ref,
               revision.version,revision.default_effect,revision.rules
        FROM straylight.stages AS stage
        JOIN straylight.scopes AS scope
          ON scope.user_id=stage.user_id AND scope.id=stage.scope_id
        JOIN straylight.policies AS policy
          ON policy.user_id=stage.user_id AND policy.id=stage.policy_id
        JOIN straylight.policy_revisions AS revision
          ON revision.user_id=stage.user_id
         AND revision.policy_id=stage.policy_id
         AND revision.version=stage.policy_version
        WHERE stage.user_id=$1 AND stage.id=$2
          AND scope.scope_ref=ANY($3::text[])
          AND stage.inventory_hash IS NOT NULL
          AND stage.status IN ('ready','promoted')
          AND stage.expires_at > clock_timestamp()
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id)
    .bind(&auth.scope_refs)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("stage_not_found", stage_ref))?;
    let inventory_hash: String = row.try_get("inventory_hash")?;
    if let Some(requested) = requested_inventory_hash {
        let requested = requested.strip_prefix("sha256:").unwrap_or(&requested);
        if requested != inventory_hash {
            return Err(ApiError::conflict(
                "stage_revision_mismatch",
                "stage reference does not match the immutable inventory hash",
                json!({
                    "requested": requested,
                    "actual": format!("sha256:{inventory_hash}")
                }),
            ));
        }
    }
    Ok(ProjectionContext {
        scope_id: row.try_get("scope_id")?,
        source_corpus_revision_id: row.try_get("base_corpus_revision_id")?,
        subject_ref: format!("stage:{}", stage_id.simple()),
        primary_policy: loaded_policy_from_row(&row)?,
    })
}

async fn load_default_policy_revision(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<LoadedPolicyRevision> {
    let row = sqlx::query(
        r#"
        SELECT policy.id AS policy_id,policy.policy_ref,
               revision.version,revision.default_effect,revision.rules
        FROM straylight.policies AS policy
        JOIN straylight.policy_revisions AS revision
          ON revision.user_id=policy.user_id
         AND revision.policy_id=policy.id
         AND revision.version=policy.current_version
        WHERE policy.user_id=$1 AND policy.is_default
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::configuration("the user has no default policy"))?;
    loaded_policy_from_row(&row)
}

fn loaded_policy_from_row(row: &sqlx::postgres::PgRow) -> ApiResult<LoadedPolicyRevision> {
    loaded_policy(
        row.try_get("policy_id")?,
        row.try_get("policy_ref")?,
        row.try_get("version")?,
        row.try_get("default_effect")?,
        row.try_get("rules")?,
    )
}

fn loaded_policy(
    id: Uuid,
    policy_ref: String,
    version: i32,
    default_effect: String,
    rules: Value,
) -> ApiResult<LoadedPolicyRevision> {
    let snapshot_hash = policy_snapshot_hash(id, version, &default_effect, &rules)?;
    let document = serde_json::from_value(json!({
        "policy_ref": policy_ref,
        "version": version,
        "default_effect": default_effect,
        "rules": rules
    }))?;
    Ok(LoadedPolicyRevision {
        id,
        document,
        snapshot_hash,
    })
}

fn policy_snapshot_hash(
    policy_id: Uuid,
    version: i32,
    default_effect: &str,
    rules: &Value,
) -> ApiResult<String> {
    let projection = json!({
        "policy_id": policy_id,
        "version": version,
        "default_effect": default_effect,
        "rules": rules
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &projection,
    )?)))
}

fn ensure_policy_snapshot_matches(
    session_hash: &str,
    current_hash: &str,
    session_ref: &str,
) -> ApiResult<()> {
    let session_hash = session_hash.strip_prefix("sha256:").unwrap_or(session_hash);
    let current_hash = current_hash.strip_prefix("sha256:").unwrap_or(current_hash);
    if session_hash == current_hash {
        return Ok(());
    }
    Err(ApiError::conflict(
        "policy_projection_mismatch",
        "the current effective default policy differs from this session snapshot; refresh the session before reading",
        json!({
            "session_id": session_ref,
            "required_action": "refresh_session"
        }),
    ))
}

async fn load_scoped_policies(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    occurrences: &[RecordOccurrence],
) -> ApiResult<Vec<ScopedPolicy>> {
    let mut occurrence_paths = BTreeMap::<Uuid, Vec<String>>::new();
    for occurrence in occurrences {
        occurrence_paths
            .entry(occurrence.record_id)
            .or_default()
            .push(occurrence.path.clone());
    }
    if occurrence_paths.is_empty() {
        return Ok(Vec::new());
    }
    let record_ids: Vec<_> = occurrence_paths.keys().copied().collect();
    let record_rows = sqlx::query(
        r#"
        SELECT binding.target_record_id,policy.id AS policy_id,policy.policy_ref,
               revision.version,revision.default_effect,revision.rules
        FROM straylight.policy_bindings AS binding
        JOIN straylight.policies AS policy
          ON policy.user_id=binding.user_id AND policy.id=binding.policy_id
        JOIN straylight.policy_revisions AS revision
          ON revision.user_id=binding.user_id
         AND revision.policy_id=binding.policy_id
         AND revision.version=binding.policy_version
        WHERE binding.user_id=$1 AND binding.scope_id=$2
          AND binding.target_record_id=ANY($3::uuid[])
        ORDER BY binding.target_record_id,binding.binding_level,binding.id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(&record_ids)
    .fetch_all(&mut **tx)
    .await?;
    let field_rows = sqlx::query(
        r#"
        SELECT binding.target_record_id,binding.json_pointer,
               policy.id AS policy_id,policy.policy_ref,
               revision.version,revision.default_effect,revision.rules
        FROM straylight.field_policy_bindings AS binding
        JOIN straylight.policies AS policy
          ON policy.user_id=binding.user_id AND policy.id=binding.policy_id
        JOIN straylight.policy_revisions AS revision
          ON revision.user_id=binding.user_id
         AND revision.policy_id=binding.policy_id
         AND revision.version=binding.policy_version
        WHERE binding.user_id=$1 AND binding.scope_id=$2
          AND binding.target_record_id=ANY($3::uuid[])
        ORDER BY binding.target_record_id,binding.json_pointer,binding.id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(&record_ids)
    .fetch_all(&mut **tx)
    .await?;

    let mut scoped = Vec::new();
    for row in record_rows {
        let record_id: Uuid = row.try_get("target_record_id")?;
        let policy = loaded_policy_from_row(&row)?.document;
        if let Some(paths) = occurrence_paths.get(&record_id) {
            for path in paths {
                scoped.push(ScopedPolicy {
                    path: path.clone(),
                    reason: "record_policy".to_owned(),
                    policy: policy.clone(),
                });
            }
        }
    }
    for row in field_rows {
        let record_id: Uuid = row.try_get("target_record_id")?;
        let field_pointer: String = row.try_get("json_pointer")?;
        let policy = loaded_policy_from_row(&row)?.document;
        if let Some(paths) = occurrence_paths.get(&record_id) {
            for path in paths {
                scoped.push(ScopedPolicy {
                    path: join_response_pointer(path, &field_pointer),
                    reason: "field_policy".to_owned(),
                    policy: policy.clone(),
                });
            }
        }
    }
    Ok(scoped)
}

fn collect_record_occurrences(value: &Value) -> Vec<RecordOccurrence> {
    fn visit(value: &Value, path: &str, occurrences: &mut BTreeSet<RecordOccurrence>) {
        match value {
            Value::Object(object) => {
                for key in ["ref", "reference", "evidence_ref"] {
                    let Some(reference) = object.get(key).and_then(Value::as_str) else {
                        continue;
                    };
                    if let Ok(reference) = RecordRef::parse(reference) {
                        occurrences.insert(RecordOccurrence {
                            record_id: reference.id,
                            path: path.to_owned(),
                        });
                        break;
                    }
                }
                for (key, child) in object {
                    visit(
                        child,
                        &join_response_pointer(path, &format!("/{}", escape_pointer_segment(key))),
                        occurrences,
                    );
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    visit(
                        child,
                        &join_response_pointer(path, &format!("/{index}")),
                        occurrences,
                    );
                }
            }
            _ => {}
        }
    }

    let mut occurrences = BTreeSet::new();
    visit(value, "", &mut occurrences);
    let mut occurrences: Vec<_> = occurrences.into_iter().collect();
    occurrences.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.record_id.cmp(&right.record_id))
    });
    occurrences
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn join_response_pointer(parent: &str, child: &str) -> String {
    match (parent.is_empty(), child.is_empty()) {
        (true, _) => child.to_owned(),
        (_, true) => parent.to_owned(),
        _ => format!("{parent}{child}"),
    }
}

fn parse_session_ref(value: &str) -> ApiResult<Uuid> {
    let raw = value
        .strip_prefix("session:")
        .or_else(|| value.strip_prefix("ms:"))
        .or_else(|| value.strip_prefix("ms_"))
        .unwrap_or(value);
    Uuid::parse_str(raw)
        .map_err(|_| ApiError::invalid(format!("invalid session reference: {value}")))
}

fn parse_stage_ref(value: &str) -> ApiResult<(Uuid, Option<String>)> {
    let raw = value
        .strip_prefix("stage:")
        .ok_or_else(|| ApiError::invalid(format!("invalid stage reference: {value}")))?;
    let (id, revision) = raw
        .split_once('@')
        .map(|(id, revision)| (id, Some(revision.to_owned())))
        .unwrap_or((raw, None));
    if revision.as_deref() == Some("") {
        return Err(ApiError::invalid("stage revision suffix cannot be empty"));
    }
    let id = Uuid::parse_str(id)
        .map_err(|_| ApiError::invalid(format!("invalid stage reference: {value}")))?;
    Ok((id, revision))
}

fn projection_audit_details(
    receipt_id: Uuid,
    operation: &str,
    action: &str,
    context: &ProjectionContext,
    receipt: &ProjectionReceipt,
) -> Value {
    json!({
        "projection_receipt": format!("projection:{}", receipt_id.simple()),
        "operation": operation,
        "subject_ref": context.subject_ref,
        "source_corpus_revision": format!("revision:{}", context.source_corpus_revision_id.simple()),
        "primary_policy": format!(
            "{}@v{}",
            context.primary_policy.document.policy_ref,
            context.primary_policy.document.version
        ),
        "effective_policy": receipt.policy_ref,
        "audience": PROJECTION_AUDIENCE,
        "purpose": PROJECTION_PURPOSE,
        "policy_action": action,
        "output_hash": receipt.output_hash,
        "included_path_count": receipt.included_paths.len(),
        "withheld_path_count": receipt.withheld.len(),
        "transform_count": receipt.transforms.len()
    })
}

fn attach_projection_receipt(
    mut data: Value,
    receipt: &ProjectionReceipt,
    receipt_id: Uuid,
    audit_id: Uuid,
    source_corpus_revision_id: Uuid,
    generated_at: DateTime<Utc>,
) -> ApiResult<Value> {
    let mut receipt = serde_json::to_value(receipt)?;
    let receipt_object = receipt.as_object_mut().ok_or_else(|| {
        ApiError::Internal("projection receipt did not serialize to an object".to_owned())
    })?;
    receipt_object.insert(
        "id".to_owned(),
        Value::String(format!("projection:{}", receipt_id.simple())),
    );
    receipt_object.insert(
        "source_corpus_revision".to_owned(),
        Value::String(format!("revision:{}", source_corpus_revision_id.simple())),
    );
    receipt_object.insert(
        "generated_at".to_owned(),
        serde_json::to_value(generated_at)?,
    );
    receipt_object.insert(
        "audit_receipt".to_owned(),
        Value::String(format!("audit:{}", audit_id.simple())),
    );
    let data_object = data.as_object_mut().ok_or_else(|| {
        ApiError::Internal("memory response data must serialize to an object".to_owned())
    })?;
    data_object.insert("projection".to_owned(), receipt);
    Ok(data)
}

fn select_scope(auth: &AuthContext, request: &OpenRequest) -> ApiResult<String> {
    let selected = request
        .hints
        .authorization_scope
        .clone()
        .or_else(|| auth.scope_refs.first().cloned())
        .ok_or_else(|| ApiError::capability("open requires an authorized scope"))?;
    if !auth.scope_refs.iter().any(|scope| scope == &selected) {
        return Err(ApiError::capability(
            "requested scope is not granted to this credential",
        ));
    }
    Ok(selected)
}

async fn resolve_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    as_of: &str,
) -> ApiResult<(Uuid, String)> {
    if as_of == "latest" {
        let row = sqlx::query(
            "SELECT active_corpus_revision_id,manifest_hash FROM straylight.active_manifests WHERE user_id=$1 AND scope_id=$2",
        )
        .bind(user_id)
        .bind(scope_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::not_found("corpus_not_found", "latest"))?;
        return Ok((
            row.try_get("active_corpus_revision_id")?,
            row.try_get("manifest_hash")?,
        ));
    }
    let raw = as_of.strip_prefix("revision:").unwrap_or(as_of);
    let revision_id = Uuid::parse_str(raw)
        .map_err(|_| ApiError::invalid("as_of must be latest or a revision reference"))?;
    let row = sqlx::query(
        "SELECT id,manifest_hash FROM straylight.corpus_revisions WHERE user_id=$1 AND scope_id=$2 AND id=$3",
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(revision_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("revision_not_found", as_of))?;
    Ok((row.try_get("id")?, row.try_get("manifest_hash")?))
}

async fn resolve_resume(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    checkpoint_ref: Option<&str>,
) -> ApiResult<(Option<Uuid>, Option<Uuid>)> {
    let Some(checkpoint_ref) = checkpoint_ref else {
        return Ok((None, None));
    };
    let raw = checkpoint_ref
        .strip_prefix("checkpoint:")
        .unwrap_or(checkpoint_ref);
    let checkpoint_id = Uuid::parse_str(raw)
        .map_err(|_| ApiError::invalid("resume checkpoint has an invalid reference"))?;
    let parent_session_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT session_id FROM straylight.checkpoints WHERE user_id=$1 AND scope_id=$2 AND id=$3",
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(checkpoint_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("checkpoint_not_found", checkpoint_ref))?;
    Ok((Some(checkpoint_id), Some(parent_session_id)))
}

async fn policy_projection_hash(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<String> {
    Ok(load_default_policy_revision(tx, user_id)
        .await?
        .snapshot_hash)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::policy::{Effect, WithheldPath};

    use super::*;

    #[test]
    fn policy_snapshot_mismatch_fails_closed_without_exposing_hashes() {
        let error = ensure_policy_snapshot_matches(
            "sha256:old",
            "sha256:new",
            "session:018f0f3d8c2d7a2bb5077d5f6e5e0001",
        )
        .unwrap_err();
        match error {
            ApiError::Public {
                code,
                message,
                details,
                ..
            } => {
                assert_eq!(code, "policy_projection_mismatch");
                assert!(message.contains("refresh"));
                let details = details.unwrap().to_string();
                assert!(!details.contains("old"));
                assert!(!details.contains("new"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn record_occurrences_require_an_unambiguous_self_reference() {
        let chunk_id = Uuid::now_v7();
        let evidence_id = Uuid::now_v7();
        let related_source_id = Uuid::now_v7();
        let value = json!({
            "source_ref": format!("source:{}", related_source_id.simple()),
            "items": [
                {
                    "reference": format!("chunk:{}", chunk_id.simple()),
                    "source_ref": format!("source:{}", related_source_id.simple()),
                    "content": "model-visible"
                },
                {
                    "data": {
                        "evidence_ref": format!("evidence:{}", evidence_id.simple()),
                        "text": "model-visible"
                    }
                }
            ]
        });
        assert_eq!(
            collect_record_occurrences(&value),
            vec![
                RecordOccurrence {
                    record_id: chunk_id,
                    path: "/items/0".to_owned(),
                },
                RecordOccurrence {
                    record_id: evidence_id,
                    path: "/items/1/data".to_owned(),
                }
            ]
        );
    }

    #[test]
    fn stage_references_preserve_the_optional_inventory_pin() {
        let stage_id = Uuid::now_v7();
        let (parsed, inventory) =
            parse_stage_ref(&format!("stage:{}@sha256:inventory", stage_id.simple())).unwrap();
        assert_eq!(parsed, stage_id);
        assert_eq!(inventory.as_deref(), Some("sha256:inventory"));
    }

    #[test]
    fn projection_audit_details_are_content_free() {
        let policy_id = Uuid::now_v7();
        let context = ProjectionContext {
            scope_id: Uuid::now_v7(),
            source_corpus_revision_id: Uuid::now_v7(),
            subject_ref: format!("session:{}", Uuid::now_v7().simple()),
            primary_policy: LoadedPolicyRevision {
                id: policy_id,
                document: PolicyDocument {
                    policy_ref: "policy:private-default".to_owned(),
                    version: 1,
                    default_effect: Effect::Allow,
                    rules: vec![],
                },
                snapshot_hash: "hash".to_owned(),
            },
        };
        let receipt = ProjectionReceipt {
            policy_ref: "policy:private-default@v1".to_owned(),
            policy_version: 1,
            audience: PROJECTION_AUDIENCE.to_owned(),
            purpose: PROJECTION_PURPOSE.to_owned(),
            included_paths: vec!["/safe".to_owned()],
            withheld: vec![WithheldPath {
                path: "/secret".to_owned(),
                reason: "field_policy".to_owned(),
            }],
            transforms: vec![],
            output_hash: "sha256:output".to_owned(),
        };
        let details =
            projection_audit_details(Uuid::now_v7(), "memory.read", "read", &context, &receipt);
        let serialized = details.to_string();
        assert_eq!(details["withheld_path_count"], 1);
        assert!(!serialized.contains("/secret"));
        assert!(!serialized.contains("field_policy"));
    }

    #[test]
    fn projection_receipt_is_attached_without_replacing_response_fields() {
        let receipt = ProjectionReceipt {
            policy_ref: "policy:private-default@v1".to_owned(),
            policy_version: 1,
            audience: PROJECTION_AUDIENCE.to_owned(),
            purpose: PROJECTION_PURPOSE.to_owned(),
            included_paths: vec![],
            withheld: vec![],
            transforms: vec![],
            output_hash: "sha256:output".to_owned(),
        };
        let data = attach_projection_receipt(
            json!({"items": [1, 2]}),
            &receipt,
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            "2026-07-12T09:00:00Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(data["items"], json!([1, 2]));
        assert_eq!(data["projection"]["output_hash"], "sha256:output");
        assert_eq!(data["projection"]["audience"], PROJECTION_AUDIENCE);
        assert_eq!(data["projection"]["purpose"], PROJECTION_PURPOSE);
    }
}

use std::collections::{BTreeMap, HashMap, HashSet};

use bytes::Bytes;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    asset_description,
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    ingest::{normalize_document, sha256_prefixed},
    models::{
        Capability, CheckpointRequest, RecurrenceSpecV1, SaveAction, SaveItem, SaveKind,
        SaveRequest, TemporalSpecV1, canonical_json,
    },
    quota,
    refs::{RecordKind, RecordRef},
};

#[derive(Clone, Debug)]
pub struct StageUpload {
    pub path: String,
    pub media_type: Option<String>,
    pub import_metadata: Value,
    pub content: StageUploadContent,
}

#[derive(Clone, Debug)]
pub enum StageUploadContent {
    Bytes(Bytes),
    Stored {
        object_key: String,
        object_version_id: Option<String>,
        content_hash: String,
        size_bytes: u64,
    },
}

const MAX_STAGE_ENTRIES: usize = 2_000;
const MAX_STAGE_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_STAGE_PATH_BYTES: usize = 1_024;
const MAX_STAGE_TEXT_INDEX_CHARS: usize = 256 * 1024;

#[derive(Clone, Debug)]
struct PreparedStageEntry {
    path: String,
    entry_kind: String,
    media_type: Option<String>,
    size_bytes: u64,
    content_hash: Option<String>,
    readability: String,
    inspection: Value,
    bytes: Option<Bytes>,
    stored_object_key: Option<String>,
    object_version_id: Option<String>,
}

#[derive(Clone, Debug)]
struct Member {
    record_id: Uuid,
    record_kind: String,
    record_version: Option<i32>,
    disposition: String,
}

#[derive(Clone, Debug)]
struct ItemReceipt {
    order: usize,
    action: String,
    kind: String,
    reference: String,
    record_id: Option<Uuid>,
    prior_version: Option<i32>,
    resulting_version: Option<i32>,
    disposition: String,
    details: Value,
}

#[derive(Clone, Debug)]
struct StagePromotionEntry {
    staged_entry_id: Uuid,
    item_order: usize,
    path: String,
    entry_kind: String,
    readability: String,
    content_hash: String,
    asset_id: Uuid,
    asset_version: i32,
}

#[derive(Clone, Debug)]
struct StagePromotion {
    stage_id: Uuid,
    stable_import_id: String,
    generation_id: String,
    input_hash: String,
    inventory_hash: String,
    selection_hash: String,
    snapshot_paths: Option<Vec<String>>,
    snapshot_hash: Option<String>,
    base_revision_id: Uuid,
    entries: Vec<StagePromotionEntry>,
}

enum StageExpansion {
    Fresh {
        items: Vec<SaveItem>,
        promotion: StagePromotion,
    },
    Replay {
        receipt: Value,
        import_receipt: String,
    },
}

#[derive(Clone, Debug)]
struct StagedAssetBinding {
    staged_entry_id: Uuid,
    asset_id: Uuid,
    asset_version: i32,
    content_hash: String,
    media_type: String,
}

#[derive(Clone, Debug)]
struct DescribedAssetBinding {
    asset_id: Uuid,
    asset_version: i32,
}

struct WriteContext<'a> {
    auth: &'a AuthContext,
    scope_id: Uuid,
    policy_id: Uuid,
    policy_version: i32,
    operation_id: Uuid,
    operation_source_id: Uuid,
    request_hash: String,
    local_refs: HashMap<String, (Uuid, RecordKind)>,
    item_ids: HashMap<usize, Uuid>,
    members: BTreeMap<Uuid, Member>,
    receipts: Vec<ItemReceipt>,
    checkpoint_items: Vec<(usize, SaveItem, Uuid)>,
}

pub async fn save(state: &AppState, auth: &AuthContext, request: SaveRequest) -> ApiResult<Value> {
    auth.require(Capability::Save)?;
    request.validate()?;
    let request_value = serde_json::to_value(&request)?;
    let request_hash = hash_json(&request_value)?;
    let (request, stage_promotions, stable_replay) =
        expand_stage_promotion(state, auth, request).await?;
    if let Some(receipt) = stable_replay {
        return Ok(receipt);
    }
    validate_expanded_save(&request, !stage_promotions.is_empty())?;
    let receipt_scope = request.scope.clone();
    let receipt_idempotency_key = request.idempotency_key.clone();
    let mut tx = state.begin_transfer_write(auth).await?;
    let scope_id = resolve_scope(&mut tx, auth, &request.scope).await?;

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "{}:{scope_id}:{}",
            auth.user_id.0, request.idempotency_key
        ))
        .execute(&mut *tx)
        .await?;

    if let Some(existing) = replay_receipt(
        &mut tx,
        auth.user_id.0,
        scope_id,
        &request.idempotency_key,
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(existing);
    }

    let (base_revision_id, base_revision_number, base_manifest_hash, manifest_generation) =
        active_revision(&mut tx, auth.user_id.0, scope_id).await?;
    if let Some(expected) = &request.base_corpus_revision {
        let expected = parse_revision_ref(expected)?;
        if expected != base_revision_id {
            return Err(ApiError::conflict(
                "revision_conflict",
                "base corpus revision is stale",
                json!({
                    "expected": revision_ref(expected),
                    "actual": revision_ref(base_revision_id)
                }),
            ));
        }
    }
    for promotion in &stage_promotions {
        lock_stage_promotion(&mut tx, auth, scope_id, promotion).await?;
    }
    let (policy_id, policy_version) = default_policy(&mut tx, auth.user_id.0).await?;
    let operation_id = request
        .operation_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value.trim_start_matches("operation:")).ok())
        .unwrap_or_else(Uuid::now_v7);
    sqlx::query(
        r#"
        INSERT INTO straylight.write_operations (
          id, user_id, scope_id, credential_id, operation_kind, intent,
          request_hash, base_corpus_revision_id, status
        ) VALUES ($1, $2, $3, $4, 'save', $5, $6, $7, 'pending')
        "#,
    )
    .bind(operation_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(&request.intent)
    .bind(&request_hash)
    .bind(base_revision_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.idempotency_keys (
          user_id, scope_id, idempotency_key, request_hash, operation_id
        ) VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(&request.idempotency_key)
    .bind(&request_hash)
    .bind(operation_id)
    .execute(&mut *tx)
    .await?;

    let operation_source_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.source_episodes (
          id, user_id, scope_id, source_ref, source_kind, source_version,
          title, captured_at, content_hash, native_locator, metadata,
          policy_id, policy_version
        ) VALUES (
          $1, $2, $3, $4, 'memory_save', $5, $6, clock_timestamp(), $7,
          $8, $9, $10, $11
        )
        "#,
    )
    .bind(operation_source_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(format!("operation:{operation_id}"))
    .bind(&request_hash)
    .bind(format!("Straylight save: {}", request.intent))
    .bind(&request_hash)
    .bind(json!({"operation_id": format!("operation:{operation_id}")}))
    .bind(json!({
        "root_refs": request.root_refs.clone(),
        "source_refs": request.source_refs.clone(),
        "raw_task_persisted": false
    }))
    .bind(policy_id)
    .bind(policy_version)
    .execute(&mut *tx)
    .await?;

    let mut context = WriteContext {
        auth,
        scope_id,
        policy_id,
        policy_version,
        operation_id,
        operation_source_id,
        request_hash: request_hash.clone(),
        local_refs: HashMap::new(),
        item_ids: HashMap::new(),
        members: load_members(&mut tx, auth.user_id.0, base_revision_id).await?,
        receipts: Vec::with_capacity(request.items.len()),
        checkpoint_items: vec![],
    };
    context.members.insert(
        operation_source_id,
        Member {
            record_id: operation_source_id,
            record_kind: "source_episode".to_owned(),
            record_version: None,
            disposition: "active".to_owned(),
        },
    );

    preallocate_refs(&request.items, &mut context)?;
    let mut ordered: Vec<_> = request.items.into_iter().enumerate().collect();
    ordered.sort_by_key(|(_, item)| processing_order(item.kind));
    for (order, item) in ordered {
        let id = allocated_id(order, &context)?;
        if item.kind == SaveKind::Checkpoint && item.action != SaveAction::Tombstone {
            context.checkpoint_items.push((order, item, id));
            continue;
        }
        process_item(&mut tx, &mut context, order, item, id).await?;
    }
    if let Some(promotion) = stage_promotions
        .iter()
        .find(|promotion| promotion.snapshot_paths.is_some())
        && let Some(snapshot_paths) = promotion.snapshot_paths.as_deref()
    {
        reconcile_import_snapshot(
            &mut tx,
            &mut context,
            &promotion.stable_import_id,
            snapshot_paths,
        )
        .await?;
    }

    for (_, item, id) in &context.checkpoint_items {
        let reference = item.reference.clone().unwrap_or_else(|| {
            RecordRef {
                kind: RecordKind::Checkpoint,
                id: *id,
            }
            .to_string()
        });
        context.members.insert(
            *id,
            Member {
                record_id: *id,
                record_kind: "checkpoint".to_owned(),
                record_version: None,
                disposition: "active".to_owned(),
            },
        );
        context
            .local_refs
            .insert(reference, (*id, RecordKind::Checkpoint));
    }

    let manifest_hash = manifest_hash(&context.members);
    let result_revision_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.corpus_revisions (
          id, user_id, scope_id, parent_revision_id, revision_number,
          manifest_hash, operation_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(result_revision_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(base_revision_number + 1)
    .bind(&manifest_hash)
    .bind(operation_id)
    .execute(&mut *tx)
    .await?;
    insert_members(
        &mut tx,
        auth.user_id.0,
        scope_id,
        result_revision_id,
        &context.members,
    )
    .await?;

    for (order, item, id) in std::mem::take(&mut context.checkpoint_items) {
        process_checkpoint(&mut tx, &mut context, order, item, id, result_revision_id).await?;
    }

    context.receipts.sort_by_key(|item| item.order);
    for item in &context.receipts {
        sqlx::query(
            r#"
            INSERT INTO straylight.write_operation_items (
              user_id, operation_id, item_order, action, item_kind,
              target_record_id, prior_version, resulting_version, disposition, details
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            "#,
        )
        .bind(auth.user_id.0)
        .bind(operation_id)
        .bind(item.order as i32)
        .bind(&item.action)
        .bind(&item.kind)
        .bind(item.record_id)
        .bind(item.prior_version)
        .bind(item.resulting_version)
        .bind(&item.disposition)
        .bind(&item.details)
        .execute(&mut *tx)
        .await?;
    }

    let mut import_receipt_ids = Vec::with_capacity(stage_promotions.len());
    for promotion in &stage_promotions {
        import_receipt_ids.push(
            record_stage_promotion(
                &mut tx,
                &context,
                promotion,
                base_revision_id,
                result_revision_id,
                &manifest_hash,
            )
            .await?,
        );
    }
    let import_receipt_id = import_receipt_ids.first().copied();

    let saved: Vec<_> = context
        .receipts
        .iter()
        .map(|item| item.reference.clone())
        .collect();
    let receipt = json!({
        "status": "committed",
        "operation_id": format!("operation:{operation_id}"),
        "receipt": format!("commit:{operation_id}"),
        "user_id": auth.user_id.ref_string(),
        "credential_id": auth.credential_id.0,
        "scope": receipt_scope,
        "idempotency_key": receipt_idempotency_key,
        "request_hash": format!("sha256:{request_hash}"),
        "base_corpus_revision": revision_ref(base_revision_id),
        "corpus_revision": revision_ref(result_revision_id),
        "manifest_hash": format!("sha256:{manifest_hash}"),
        "import_receipt": import_receipt_id.map(|id| format!("import:{id}")),
        "import_receipts": import_receipt_ids.iter()
            .map(|id| format!("import:{id}"))
            .collect::<Vec<_>>(),
        "saved": saved,
        "items": context.receipts.iter().map(|item| json!({
            "order": item.order,
            "action": item.action,
            "kind": item.kind,
            "ref": item.reference,
            "prior_version": item.prior_version,
            "resulting_version": item.resulting_version,
            "disposition": item.disposition,
            "details": item.details
        })).collect::<Vec<_>>(),
        "search_status": {
            "exact": "ready",
            "lexical": "ready",
            "semantic": "pending"
        },
        "policy": {
            "policy_id": policy_id,
            "policy_version": policy_version
        },
        "replayed": false
    });
    sqlx::query(
        r#"
        UPDATE straylight.write_operations
        SET result_corpus_revision_id = $1, status = 'committed', receipt = $2,
            completed_at = clock_timestamp()
        WHERE user_id = $3 AND id = $4
        "#,
    )
    .bind(result_revision_id)
    .bind(&receipt)
    .bind(auth.user_id.0)
    .bind(operation_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE straylight.active_manifests
        SET active_corpus_revision_id = $1, manifest_hash = $2,
            generation = generation + 1, updated_at = clock_timestamp()
        WHERE user_id = $3 AND scope_id = $4
          AND active_corpus_revision_id = $5 AND generation = $6
        "#,
    )
    .bind(result_revision_id)
    .bind(&manifest_hash)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(base_revision_id)
    .bind(manifest_generation)
    .execute(&mut *tx)
    .await?
    .rows_affected()
    .eq(&1)
    .then_some(())
    .ok_or_else(|| {
        ApiError::conflict(
            "revision_conflict",
            "active manifest changed during save",
            json!({"base_manifest_hash": format!("sha256:{base_manifest_hash}")}),
        )
    })?;
    let manifest_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.active_manifests WHERE user_id=$1 AND scope_id=$2",
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.active_manifest_history (
          id,user_id,scope_id,manifest_id,generation,corpus_revision_id,
          manifest_hash,change_kind
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,'write')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(manifest_id)
    .bind(manifest_generation + 1)
    .bind(result_revision_id)
    .bind(&manifest_hash)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,action,request_id,details,content_free
        ) VALUES ($1,$2,$3,$4,'memory.save',$5,$6,true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(format!("operation:{operation_id}"))
    .bind(json!({
        "base_revision": revision_ref(base_revision_id),
        "result_revision": revision_ref(result_revision_id),
        "item_count": context.receipts.len()
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(receipt)
}

fn validate_expanded_save(request: &SaveRequest, stage_promotion: bool) -> ApiResult<()> {
    if !stage_promotion {
        return request.validate();
    }
    if request.items.is_empty() || request.items.len() > 100_000 {
        return Err(ApiError::invalid(
            "an expanded staged import must contain between 1 and 100,000 files",
        ));
    }
    for item in &request.items {
        item.validate()?;
    }
    Ok(())
}

async fn lock_stage_promotion(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope_id: Uuid,
    promotion: &StagePromotion,
) -> ApiResult<()> {
    let row = sqlx::query(
        r#"
        SELECT scope_id,credential_id,base_corpus_revision_id,input_hash,
               inventory_hash,status,expires_at
        FROM straylight.stages
        WHERE user_id=$1 AND id=$2
        FOR UPDATE
        "#,
    )
    .bind(auth.user_id.0)
    .bind(promotion.stage_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ApiError::not_found("stage_not_found", &format!("stage:{}", promotion.stage_id))
    })?;
    let status: String = row.try_get("status")?;
    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    let current_inventory: Option<String> = row.try_get("inventory_hash")?;
    if row.try_get::<Uuid, _>("scope_id")? != scope_id
        || row.try_get::<Uuid, _>("credential_id")? != auth.credential_id.0
        || row.try_get::<Uuid, _>("base_corpus_revision_id")? != promotion.base_revision_id
        || row.try_get::<String, _>("input_hash")? != promotion.input_hash
        || current_inventory.as_deref() != Some(promotion.inventory_hash.as_str())
        || status != "ready"
        || expires_at <= Utc::now()
    {
        return Err(ApiError::conflict(
            "stage_changed",
            "the immutable stage snapshot changed or is no longer promotable",
            json!({
                "stage_ref": format!("stage:{}", promotion.stage_id),
                "status": status,
                "expires_at": expires_at
            }),
        ));
    }
    let entry_ids: Vec<Uuid> = promotion
        .entries
        .iter()
        .map(|entry| entry.staged_entry_id)
        .collect();
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.entry_kind,entry.readability,
               entry.content_hash,entry.asset_id,entry.asset_version
        FROM straylight.staged_entries AS entry
        WHERE entry.user_id=$1 AND entry.scope_id=$2 AND entry.stage_id=$3
          AND entry.id=ANY($4::uuid[])
        ORDER BY entry.path
        FOR SHARE
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(promotion.stage_id)
    .bind(&entry_ids)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != promotion.entries.len() {
        return Err(ApiError::conflict(
            "stage_changed",
            "one or more selected staged entries disappeared",
            json!({"stage_ref": format!("stage:{}", promotion.stage_id)}),
        ));
    }
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let expected = promotion
            .entries
            .iter()
            .find(|entry| entry.staged_entry_id == id)
            .ok_or_else(|| ApiError::Internal("locked an unselected stage entry".to_owned()))?;
        let unchanged = row.try_get::<String, _>("path")? == expected.path
            && row.try_get::<String, _>("entry_kind")? == expected.entry_kind
            && row.try_get::<String, _>("readability")? == expected.readability
            && row.try_get::<Option<String>, _>("content_hash")?.as_deref()
                == Some(expected.content_hash.as_str())
            && row.try_get::<Option<Uuid>, _>("asset_id")? == Some(expected.asset_id)
            && row.try_get::<Option<i32>, _>("asset_version")? == Some(expected.asset_version);
        if !unchanged {
            return Err(ApiError::conflict(
                "stage_changed",
                "selected staged entry metadata changed before commit",
                json!({"path": expected.path}),
            ));
        }
    }
    Ok(())
}

async fn record_stage_promotion(
    tx: &mut Transaction<'_, Postgres>,
    context: &WriteContext<'_>,
    promotion: &StagePromotion,
    base_revision_id: Uuid,
    result_revision_id: Uuid,
    manifest_hash: &str,
) -> ApiResult<Uuid> {
    if promotion.base_revision_id != base_revision_id {
        return Err(ApiError::conflict(
            "revision_conflict",
            "stage promotion base revision changed during commit",
            json!({
                "stage_revision": revision_ref(promotion.base_revision_id),
                "commit_revision": revision_ref(base_revision_id)
            }),
        ));
    }
    let receipt_id = Uuid::now_v7();
    let receipt = json!({
        "stage_ref": format!("stage:{}", promotion.stage_id),
        "generation_id": promotion.generation_id,
        "operation_id": format!("operation:{}", context.operation_id),
        "selected_entries": promotion.entries.iter().map(|entry| &entry.path).collect::<Vec<_>>(),
        "selected_manifest_hash": format!("sha256:{}", promotion.selection_hash),
        "snapshot_path_count": promotion.snapshot_paths.as_ref().map(Vec::len),
        "snapshot_manifest_hash": promotion.snapshot_hash.as_ref()
            .map(|hash| format!("sha256:{hash}")),
        "result_corpus_revision": revision_ref(result_revision_id)
    });
    sqlx::query(
        r#"
        INSERT INTO straylight.import_receipts (
          id,user_id,scope_id,stable_import_id,stage_id,input_hash,inventory_hash,
          manifest_hash,base_corpus_revision_id,result_corpus_revision_id,
          replay_outcome,policy_id,policy_version,indexing_state,receipt
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'new_import',$11,$12,$13,$14)
        "#,
    )
    .bind(receipt_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(&promotion.stable_import_id)
    .bind(promotion.stage_id)
    .bind(&promotion.input_hash)
    .bind(&promotion.inventory_hash)
    .bind(manifest_hash)
    .bind(base_revision_id)
    .bind(result_revision_id)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .bind(json!({"exact": "ready", "lexical": "ready", "semantic": "pending"}))
    .bind(&receipt)
    .execute(&mut **tx)
    .await?;
    for entry in &promotion.entries {
        let item = context
            .receipts
            .iter()
            .find(|item| item.order == entry.item_order)
            .ok_or_else(|| ApiError::Internal("stage promotion lost an item receipt".to_owned()))?;
        sqlx::query(
            r#"
            INSERT INTO straylight.import_entry_dispositions (
              user_id,import_receipt_id,staged_entry_id,disposition,
              resulting_record_id,details
            ) VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(receipt_id)
        .bind(entry.staged_entry_id)
        .bind(if item.disposition == "deduplicated" {
            "deduplicated"
        } else {
            "committed"
        })
        .bind(item.record_id)
        .bind(json!({"path": entry.path, "result_ref": item.reference}))
        .execute(&mut **tx)
        .await?;
    }
    let updated = sqlx::query(
        "UPDATE straylight.stages SET status='promoted' WHERE user_id=$1 AND id=$2 AND status='ready'",
    )
    .bind(context.auth.user_id.0)
    .bind(promotion.stage_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(ApiError::conflict(
            "stage_changed",
            "stage was promoted concurrently",
            json!({"stage_ref": format!("stage:{}", promotion.stage_id)}),
        ));
    }
    Ok(receipt_id)
}

pub async fn checkpoint(
    state: &AppState,
    auth: &AuthContext,
    request: CheckpointRequest,
) -> ApiResult<Value> {
    auth.require(Capability::Checkpoint)?;
    let session_id = request.session_id.clone();
    let scope = session_scope_ref(state, auth, &session_id).await?;
    let idempotency_key = request.idempotency_key.clone().unwrap_or_else(|| {
        let digest =
            hash_json(&request.state).unwrap_or_else(|_| Uuid::now_v7().simple().to_string());
        format!("checkpoint:{session_id}:{digest}")
    });
    let payload = json!({
        "session_id": request.session_id,
        "parent_checkpoint_id": request.parent_checkpoint_id,
        "summary": request.state,
        "source_refs": request.source_refs
    });
    save(
        state,
        auth,
        SaveRequest {
            intent: "checkpoint".to_owned(),
            scope,
            root_refs: vec![],
            source_refs: vec![],
            items: vec![SaveItem {
                action: SaveAction::Create,
                kind: SaveKind::Checkpoint,
                reference: None,
                expected_version: None,
                payload,
            }],
            idempotency_key,
            operation_id: None,
            base_corpus_revision: None,
            confirmation_token: None,
        },
    )
    .await
}

pub async fn stage_uploads(
    state: &AppState,
    auth: &AuthContext,
    scope_ref: &str,
    stable_import_id: Option<String>,
    describe_binaries: bool,
    uploads: Vec<StageUpload>,
) -> ApiResult<Value> {
    let mut uploaded_keys = Vec::new();
    let result = stage_uploads_inner(
        state,
        auth,
        scope_ref,
        stable_import_id,
        describe_binaries,
        uploads,
        &mut uploaded_keys,
    )
    .await;
    if result.is_err()
        && !uploaded_keys.is_empty()
        && let Err(cleanup_error) = reconcile_stage_commit(state, auth, &uploaded_keys).await
    {
        tracing::error!(
            ?cleanup_error,
            "failed to reconcile uploaded objects after stage failure"
        );
        return Err(ApiError::Internal(
            "stage failed and uploaded-object cleanup could not be verified".to_owned(),
        ));
    }
    result
}

async fn stage_uploads_inner(
    state: &AppState,
    auth: &AuthContext,
    scope_ref: &str,
    stable_import_id: Option<String>,
    describe_binaries: bool,
    uploads: Vec<StageUpload>,
    uploaded_keys: &mut Vec<String>,
) -> ApiResult<Value> {
    auth.require(Capability::Stage)?;
    if uploads.is_empty() || uploads.len() > 1_000 {
        return Err(ApiError::invalid(
            "stage requires between 1 and 1,000 files",
        ));
    }
    let stable_import_id = stable_import_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if stable_import_id
        .as_ref()
        .is_some_and(|value| value.len() > 240)
    {
        return Err(ApiError::invalid(
            "stable_import_id is limited to 240 characters",
        ));
    }
    let input_count = uploads.len();
    let (prepared, raw_input_hash, inventory_hash, mut warnings) = prepare_stage_entries(uploads)?;
    let input_hash = hex::encode(Sha256::digest(
        format!("{raw_input_hash}\0describe_binaries={describe_binaries}").as_bytes(),
    ));
    let description_count = prepared
        .iter()
        .filter(|entry| {
            describe_binaries
                && entry.readability == "unsupported"
                && matches!(entry.entry_kind.as_str(), "file" | "archive")
                && entry.content_hash.is_some()
        })
        .count();
    let stage_status = if description_count > 0 {
        "inspecting"
    } else {
        "ready"
    };
    let pending_blobs: HashMap<String, usize> = prepared
        .iter()
        .filter_map(|entry| {
            Some((
                strip_sha256(entry.content_hash.as_deref()?),
                usize::try_from(entry.size_bytes).ok()?,
            ))
        })
        .map(|(hash, size)| (hash.to_owned(), size))
        .collect();
    let pending_blobs: Vec<_> = pending_blobs.into_iter().collect();
    let mut tx = state.begin_write(auth).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(auth.user_id.0)
        .execute(&mut *tx)
        .await?;
    validate_stored_stage_objects(state, auth, &prepared).await?;
    quota::ensure_storage_capacity(&mut tx, auth.user_id.0, &pending_blobs).await?;
    let scope_id = resolve_scope(&mut tx, auth, scope_ref).await?;
    let (base_revision, _, _, _) = active_revision(&mut tx, auth.user_id.0, scope_id).await?;
    let (policy_id, policy_version) = default_policy(&mut tx, auth.user_id.0).await?;
    let mut replay_outcome = "new_import";
    let mut prior_stage_ref = None;
    if let Some(stable_import_id) = &stable_import_id {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "stage:{}:{scope_id}:{stable_import_id}",
                auth.user_id.0
            ))
            .execute(&mut *tx)
            .await?;
        if let Some(row) = sqlx::query(
            r#"
            SELECT id,input_hash,inventory_hash,base_corpus_revision_id,status,expires_at
            FROM straylight.stages
            WHERE user_id=$1 AND scope_id=$2 AND stable_import_id=$3
            ORDER BY created_at DESC LIMIT 1
            "#,
        )
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(stable_import_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let stage_id: Uuid = row.try_get("id")?;
            prior_stage_ref = Some(format!("stage:{stage_id}"));
            let prior_hash: String = row.try_get("input_hash")?;
            let prior_inventory_hash: Option<String> = row.try_get("inventory_hash")?;
            let prior_base: Uuid = row.try_get("base_corpus_revision_id")?;
            let status: String = row.try_get("status")?;
            let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
            if prior_hash == input_hash
                && prior_base == base_revision
                && matches!(status.as_str(), "inspecting" | "ready" | "promoted")
                && expires_at > Utc::now()
            {
                let import_receipt = sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM straylight.import_receipts WHERE user_id=$1 AND stage_id=$2 ORDER BY created_at DESC LIMIT 1",
                )
                .bind(auth.user_id.0)
                .bind(stage_id)
                .fetch_optional(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(json!({
                    "id": format!("stage:{stage_id}"),
                    "stage_ref": format!("stage:{stage_id}"),
                    "status": status,
                    "input_hash": format!("sha256:{input_hash}"),
                    "inventory_hash": prior_inventory_hash
                        .map(|hash| format!("sha256:{hash}")),
                    "base_corpus_revision": revision_ref(base_revision),
                    "import_receipt": import_receipt.map(|id| format!("import:{id}")),
                    "replay_outcome": "exact_replay",
                    "replayed": true
                }));
            }
            if matches!(status.as_str(), "uploading" | "inspecting" | "ready")
                && expires_at > Utc::now()
            {
                return Err(ApiError::conflict(
                    "stage_pending",
                    "a changed import cannot replace an unpromoted stage; finish or expire the existing stage first",
                    json!({
                        "stage_ref": format!("stage:{stage_id}"),
                        "status": status,
                        "expires_at": expires_at
                    }),
                ));
            }
            replay_outcome = "revision_delta";
        }
    }
    let stage_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.stages (
          id,user_id,scope_id,credential_id,stable_import_id,
          base_corpus_revision_id,input_hash,inventory_hash,status,
          policy_id,policy_version,expires_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,clock_timestamp()+interval '7 days')
        "#,
    )
    .bind(stage_id)
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(&stable_import_id)
    .bind(base_revision)
    .bind(&input_hash)
    .bind(if stage_status == "ready" {
        Some(inventory_hash.as_str())
    } else {
        None
    })
    .bind(stage_status)
    .bind(policy_id)
    .bind(policy_version)
    .execute(&mut *tx)
    .await?;
    let mut response_entries = Vec::with_capacity(prepared.len());
    for entry in &prepared {
        let asset_binding = if let Some(content_hash) = &entry.content_hash {
            if entry.bytes.is_some() || entry.stored_object_key.is_some() {
                let digest = strip_sha256(content_hash);
                let object_key = entry
                    .stored_object_key
                    .clone()
                    .unwrap_or_else(|| format!("{}/blobs/{digest}", auth.user_id.0));
                let entry_size = i64::try_from(entry.size_bytes).unwrap_or(i64::MAX);
                let entry_media_type = entry
                    .media_type
                    .as_deref()
                    .unwrap_or("application/octet-stream");
                let entry_import_metadata = entry
                    .inspection
                    .get("import_metadata")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let prior_asset = if let Some(stable_import_id) = stable_import_id.as_deref() {
                    let source_ref = staged_source_ref(scope_ref, stable_import_id, &entry.path);
                    sqlx::query(
                        r#"
                        SELECT asset.id AS asset_id,
                               asset.current_version AS asset_head_version,
                               link.asset_version AS current_version,
                               version.content_hash,version.size_bytes,version.media_type,
                               source.metadata->'import_metadata' AS prior_import_metadata
                        FROM straylight.source_episodes AS source
                        JOIN straylight.source_asset_links AS link
                          ON link.user_id=source.user_id
                         AND link.source_episode_id=source.id
                         AND link.role='source_native'
                        JOIN straylight.assets AS asset
                          ON asset.user_id=link.user_id AND asset.id=link.asset_id
                        JOIN straylight.asset_versions AS version
                          ON version.user_id=asset.user_id
                         AND version.asset_id=asset.id
                         AND version.version=link.asset_version
                        WHERE source.user_id=$1 AND source.scope_id=$2
                          AND source.source_ref=$3 AND asset.scope_id=$2
                        ORDER BY source.captured_at DESC,source.id DESC
                        LIMIT 1
                        FOR UPDATE OF asset
                        "#,
                    )
                    .bind(auth.user_id.0)
                    .bind(scope_id)
                    .bind(source_ref)
                    .fetch_optional(&mut *tx)
                    .await?
                } else {
                    None
                };
                let binding = if let Some(prior) = prior_asset {
                    let asset_id: Uuid = prior.try_get("asset_id")?;
                    let current_version: i32 = prior.try_get("current_version")?;
                    let asset_head_version: i32 = prior.try_get("asset_head_version")?;
                    if asset_head_version != current_version {
                        return Err(ApiError::conflict(
                            "stage_cleanup_pending",
                            "the prior imported asset has an unpromoted version awaiting cleanup",
                            json!({
                                "asset_ref": format!("asset:{asset_id}"),
                                "active_version": current_version,
                                "staged_head_version": asset_head_version
                            }),
                        ));
                    }
                    let prior_hash: String = prior.try_get("content_hash")?;
                    let prior_size: i64 = prior.try_get("size_bytes")?;
                    let prior_media_type: String = prior.try_get("media_type")?;
                    let prior_import_metadata: Option<Value> =
                        prior.try_get("prior_import_metadata")?;
                    if prior_hash == digest
                        && prior_size == entry_size
                        && prior_media_type == entry_media_type
                        && prior_import_metadata.as_ref() == Some(&entry_import_metadata)
                    {
                        (asset_id, current_version)
                    } else {
                        let next_version = current_version.checked_add(1).ok_or_else(|| {
                            ApiError::Internal("asset version overflow".to_owned())
                        })?;
                        let object_version_id = persist_stage_entry_blob(
                            state,
                            auth,
                            entry,
                            &object_key,
                            content_hash,
                            uploaded_keys,
                        )
                        .await?;
                        sqlx::query(
                            r#"
                            INSERT INTO straylight.asset_versions (
                              user_id,asset_id,version,previous_version,bucket,object_key,
                              object_version_id,content_hash,size_bytes,media_type,metadata
                            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
                            "#,
                        )
                        .bind(auth.user_id.0)
                        .bind(asset_id)
                        .bind(next_version)
                        .bind(current_version)
                        .bind(&state.config.s3_bucket)
                        .bind(&object_key)
                        .bind(&object_version_id)
                        .bind(digest)
                        .bind(entry_size)
                        .bind(entry_media_type)
                        .bind(json!({
                            "stage_ref": format!("stage:{stage_id}"),
                            "stable_import_id": stable_import_id,
                            "original_path": entry.path,
                            "import_metadata": entry_import_metadata
                        }))
                        .execute(&mut *tx)
                        .await?;
                        sqlx::query(
                            r#"
                            UPDATE straylight.assets
                            SET current_version=$1
                            WHERE user_id=$2 AND id=$3 AND current_version=$4
                            "#,
                        )
                        .bind(next_version)
                        .bind(auth.user_id.0)
                        .bind(asset_id)
                        .bind(current_version)
                        .execute(&mut *tx)
                        .await?;
                        (asset_id, next_version)
                    }
                } else {
                    let object_version_id = persist_stage_entry_blob(
                        state,
                        auth,
                        entry,
                        &object_key,
                        content_hash,
                        uploaded_keys,
                    )
                    .await?;
                    let asset_id = Uuid::now_v7();
                    sqlx::query(
                        r#"
                        INSERT INTO straylight.assets (
                          id,user_id,scope_id,current_version,policy_id,policy_version
                        ) VALUES ($1,$2,$3,1,$4,$5)
                        "#,
                    )
                    .bind(asset_id)
                    .bind(auth.user_id.0)
                    .bind(scope_id)
                    .bind(policy_id)
                    .bind(policy_version)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO straylight.asset_versions (
                          user_id,asset_id,version,bucket,object_key,content_hash,
                          size_bytes,media_type,metadata,object_version_id
                        ) VALUES ($1,$2,1,$3,$4,$5,$6,$7,$8,$9)
                        "#,
                    )
                    .bind(auth.user_id.0)
                    .bind(asset_id)
                    .bind(&state.config.s3_bucket)
                    .bind(&object_key)
                    .bind(digest)
                    .bind(entry_size)
                    .bind(entry_media_type)
                    .bind(json!({
                        "stage_ref": format!("stage:{stage_id}"),
                        "stable_import_id": stable_import_id,
                        "original_path": entry.path,
                        "import_metadata": entry_import_metadata
                    }))
                    .bind(&object_version_id)
                    .execute(&mut *tx)
                    .await?;
                    (asset_id, 1)
                };
                Some(binding)
            } else {
                None
            }
        } else {
            None
        };
        let staged_entry_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO straylight.staged_entries (
              id,user_id,scope_id,stage_id,path,entry_kind,media_type,size_bytes,
              content_hash,asset_id,asset_version,readability,inspection
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
            "#,
        )
        .bind(staged_entry_id)
        .bind(auth.user_id.0)
        .bind(scope_id)
        .bind(stage_id)
        .bind(&entry.path)
        .bind(&entry.entry_kind)
        .bind(&entry.media_type)
        .bind(entry.size_bytes as i64)
        .bind(entry.content_hash.as_deref().map(strip_sha256))
        .bind(asset_binding.map(|binding| binding.0))
        .bind(asset_binding.map(|binding| binding.1))
        .bind(&entry.readability)
        .bind(&entry.inspection)
        .execute(&mut *tx)
        .await?;
        if describe_binaries
            && entry.readability == "unsupported"
            && matches!(entry.entry_kind.as_str(), "file" | "archive")
            && let Some((asset_id, asset_version)) = asset_binding
            && entry.content_hash.is_some()
        {
            sqlx::query(
                r#"
                INSERT INTO straylight.background_jobs (
                  id,user_id,scope_id,job_kind,status,payload
                ) VALUES ($1,$2,$3,'asset_description','queued',$4)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(auth.user_id.0)
            .bind(scope_id)
            .bind(json!({
                "stage_id": stage_id,
                "staged_entry_id": staged_entry_id,
                "asset_id": asset_id,
                "asset_version": asset_version,
                "content_hash": entry.content_hash,
                "path": entry.path,
                "media_type": entry.media_type,
                "size_bytes": entry.size_bytes
            }))
            .execute(&mut *tx)
            .await?;
        }
        response_entries.push(json!({
            "ref": format!("stage-entry:{staged_entry_id}"),
            "path": entry.path,
            "entry_kind": entry.entry_kind,
            "content_type": entry.media_type,
            "size_bytes": entry.size_bytes,
            "hash": entry.content_hash,
            "asset_ref": asset_binding.map(|binding| format!("asset:{}", binding.0)),
            "asset_version": asset_binding.map(|binding| binding.1),
            "disposition": "staged",
            "status": entry.readability
        }));
    }
    warnings.sort();
    warnings.dedup();
    sqlx::query(
        r#"
        INSERT INTO straylight.audit_events (
          id,user_id,scope_id,credential_id,action,target_record_id,details,content_free
        ) VALUES ($1,$2,$3,$4,'memory.stage',$5,$6,true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(stage_id)
    .bind(json!({
        "input_count": input_count,
        "entry_count": prepared.len(),
        "input_hash": input_hash,
        "inventory_hash": if stage_status == "ready" {
            Some(inventory_hash.as_str())
        } else {
            None
        },
        "describe_binaries": describe_binaries,
        "description_jobs": description_count,
        "replay_outcome": replay_outcome
    }))
    .execute(&mut *tx)
    .await?;
    let files = prepared
        .iter()
        .filter(|entry| entry.entry_kind == "file")
        .count();
    let directories = prepared
        .iter()
        .filter(|entry| entry.entry_kind == "directory")
        .count();
    let archives = prepared
        .iter()
        .filter(|entry| entry.entry_kind == "archive")
        .count();
    let readable = prepared
        .iter()
        .filter(|entry| entry.readability == "readable")
        .count();
    let quarantined = prepared
        .iter()
        .filter(|entry| entry.readability == "quarantined")
        .count();
    let opaque = prepared
        .iter()
        .filter(|entry| entry.readability == "unsupported" && entry.entry_kind != "directory")
        .count();
    tx.commit().await?;
    Ok(json!({
        "id": format!("stage:{stage_id}"),
        "stage_ref": format!("stage:{stage_id}"),
        "status": stage_status,
        "input_hash": format!("sha256:{input_hash}"),
        "inventory_hash": if stage_status == "ready" {
            Some(format!("sha256:{inventory_hash}"))
        } else {
            None
        },
        "base_corpus_revision": revision_ref(base_revision),
        "inventory_summary": {
            "files": files,
            "directories": directories,
            "archives": archives,
            "readable": readable,
            "opaque": opaque,
            "quarantined": quarantined
        },
        "warnings": warnings,
        "description_jobs": description_count,
        "entries": response_entries,
        "replay_outcome": replay_outcome,
        "prior_stage_ref": prior_stage_ref,
        "replayed": false
    }))
}

pub async fn stage_status(
    state: &AppState,
    auth: &AuthContext,
    stage_ref: &str,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    let stage_id = Uuid::parse_str(stage_ref.trim_start_matches("stage:"))
        .map_err(|_| ApiError::invalid("stage reference is invalid"))?;
    let mut tx = state.begin_read(auth).await?;
    let stage = sqlx::query(
        r#"
        SELECT stage.id,stage.stable_import_id,stage.input_hash,
               stage.inventory_hash,stage.status,stage.created_at,stage.expires_at,
               stage.base_corpus_revision_id,scope.scope_ref
        FROM straylight.stages AS stage
        JOIN straylight.scopes AS scope
          ON scope.user_id=stage.user_id AND scope.id=stage.scope_id
        WHERE stage.user_id=$1 AND stage.id=$2
          AND stage.credential_id=$3
          AND scope.scope_ref=ANY($4::text[])
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id)
    .bind(auth.credential_id.0)
    .bind(&auth.scope_refs)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("stage_not_found", stage_ref))?;
    let entries = sqlx::query(
        r#"
        SELECT id,path,entry_kind,media_type,size_bytes,content_hash,
               asset_id,asset_version,readability,inspection,created_at
        FROM straylight.staged_entries
        WHERE user_id=$1 AND stage_id=$2
        ORDER BY path,id
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id)
    .fetch_all(&mut *tx)
    .await?;
    let jobs = sqlx::query(
        r#"
        SELECT id,job_kind,status,attempts,max_attempts,available_at,
               result,created_at,completed_at
        FROM straylight.background_jobs
        WHERE user_id=$1 AND payload->>'stage_id'=$2
        ORDER BY created_at,id
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let entry_values = entries
        .into_iter()
        .map(|row| {
            let entry_id: Uuid = row.try_get("id")?;
            let asset_id: Option<Uuid> = row.try_get("asset_id")?;
            let content_hash = row
                .try_get::<Option<String>, _>("content_hash")?
                .map(|hash| format!("sha256:{hash}"));
            let mut inspection: Value = row.try_get("inspection")?;
            if let Some(object) = inspection.as_object_mut() {
                object.remove("search_text");
            }
            Ok(json!({
                "ref": format!("stage-entry:{entry_id}"),
                "path": row.try_get::<String,_>("path")?,
                "entry_kind": row.try_get::<String,_>("entry_kind")?,
                "media_type": row.try_get::<Option<String>,_>("media_type")?,
                "size_bytes": row.try_get::<i64,_>("size_bytes")?,
                "hash": content_hash.clone(),
                "content_hash": content_hash,
                "asset_ref": asset_id.map(|id| format!("asset:{id}")),
                "asset_version": row.try_get::<Option<i32>,_>("asset_version")?,
                "readability": row.try_get::<String,_>("readability")?,
                "inspection": inspection,
                "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let job_values = jobs
        .into_iter()
        .map(|row| {
            let job_id: Uuid = row.try_get("id")?;
            Ok(json!({
                "ref": format!("job:{job_id}"),
                "kind": row.try_get::<String,_>("job_kind")?,
                "status": row.try_get::<String,_>("status")?,
                "attempts": row.try_get::<i32,_>("attempts")?,
                "max_attempts": row.try_get::<i32,_>("max_attempts")?,
                "available_at": row.try_get::<DateTime<Utc>,_>("available_at")?,
                "result": row.try_get::<Option<Value>,_>("result")?,
                "created_at": row.try_get::<DateTime<Utc>,_>("created_at")?,
                "completed_at": row.try_get::<Option<DateTime<Utc>>,_>("completed_at")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let status: String = stage.try_get("status")?;
    Ok(json!({
        "stage_ref": format!("stage:{stage_id}"),
        "scope": stage.try_get::<String,_>("scope_ref")?,
        "stable_import_id": stage.try_get::<Option<String>,_>("stable_import_id")?,
        "status": status,
        "promotable": status == "ready",
        "input_hash": format!("sha256:{}", stage.try_get::<String,_>("input_hash")?),
        "inventory_hash": stage.try_get::<Option<String>,_>("inventory_hash")?
            .map(|hash| format!("sha256:{hash}")),
        "base_corpus_revision": format!(
            "revision:{}",
            stage.try_get::<Uuid,_>("base_corpus_revision_id")?
        ),
        "created_at": stage.try_get::<DateTime<Utc>,_>("created_at")?,
        "expires_at": stage.try_get::<DateTime<Utc>,_>("expires_at")?,
        "entries": entry_values,
        "jobs": job_values
    }))
}

async fn persist_stage_entry_blob(
    state: &AppState,
    auth: &AuthContext,
    entry: &PreparedStageEntry,
    expected_object_key: &str,
    expected_hash: &str,
    uploaded_keys: &mut Vec<String>,
) -> ApiResult<Option<String>> {
    let Some(bytes) = &entry.bytes else {
        return Ok(entry.object_version_id.clone());
    };
    let stored = state
        .object_store
        .put_user_blob(auth.user_id, entry.media_type.as_deref(), bytes.clone())
        .await?;
    uploaded_keys.push(stored.object_key.clone());
    if stored.sha256 != expected_hash
        || stored.object_key != expected_object_key
        || stored.size_bytes != bytes.len()
    {
        return Err(ApiError::Internal(
            "staged object identity changed during upload".to_owned(),
        ));
    }
    Ok(stored.object_version_id)
}

async fn validate_stored_stage_objects(
    state: &AppState,
    auth: &AuthContext,
    entries: &[PreparedStageEntry],
) -> ApiResult<()> {
    for entry in entries {
        let Some(object_key) = entry.stored_object_key.as_deref() else {
            continue;
        };
        let content_hash = entry.content_hash.as_deref().ok_or_else(|| {
            ApiError::Internal("stored stage object has no content hash".to_owned())
        })?;
        let digest = strip_sha256(content_hash);
        let expected_object_key = format!("{}/blobs/{digest}", auth.user_id.0);
        if object_key != expected_object_key {
            return Err(ApiError::Internal(
                "stored stage object key does not match its immutable identity".to_owned(),
            ));
        }
        let object_version_id = entry
            .object_version_id
            .as_deref()
            .filter(|version_id| !version_id.is_empty() && *version_id != "null")
            .ok_or_else(|| {
                ApiError::Internal("stored stage object has no exact storage version ID".to_owned())
            })?;
        state
            .object_store
            .verify_object_identity_version(
                object_key,
                Some(object_version_id),
                digest,
                entry.size_bytes,
            )
            .await?;
    }
    Ok(())
}

async fn reconcile_stage_commit(
    state: &AppState,
    auth: &AuthContext,
    object_keys: &[String],
) -> ApiResult<()> {
    let mut tx = state.begin_write(auth).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('storage:' || $1::text,0))")
        .bind(auth.user_id.0)
        .execute(&mut *tx)
        .await?;
    for object_key in object_keys {
        let referenced = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM straylight.asset_versions
              WHERE user_id=$1 AND object_key=$2 AND size_bytes > 0
              UNION ALL
              SELECT 1
              FROM straylight.asset_uploads
              WHERE user_id=$1 AND canonical_object_key=$2
                AND status IN ('completed','consumed')
            )
            "#,
        )
        .bind(auth.user_id.0)
        .bind(object_key)
        .fetch_one(&mut *tx)
        .await?;
        if !referenced {
            state.object_store.purge_all_versions(object_key).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

fn prepare_stage_entries(
    mut uploads: Vec<StageUpload>,
) -> ApiResult<(Vec<PreparedStageEntry>, String, String, Vec<String>)> {
    uploads.sort_by(|left, right| left.path.cmp(&right.path));
    let mut input_hasher = Sha256::new();
    let mut prepared = Vec::new();
    let mut stage_paths = HashSet::new();
    let mut warnings = Vec::new();
    let mut expanded_bytes = 0u64;
    let mut previous_path = None::<String>;

    for upload in uploads {
        validate_stage_path(&upload.path)?;
        validate_stage_import_metadata(&upload.import_metadata)?;
        if previous_path.as_deref() == Some(upload.path.as_str()) {
            return Err(ApiError::invalid("stage input paths must be unique"));
        }
        previous_path = Some(upload.path.clone());
        input_hasher.update((upload.path.len() as u64).to_be_bytes());
        input_hasher.update(upload.path.as_bytes());
        let (
            bytes,
            stored_object_key,
            object_version_id,
            declared_size_bytes,
            content_hash,
            upload_source,
        ) = match upload.content {
            StageUploadContent::Bytes(bytes) => {
                let digest = hex::encode(Sha256::digest(&bytes));
                let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                (
                    Some(bytes),
                    None,
                    None,
                    size_bytes,
                    format!("sha256:{digest}"),
                    "multipart_upload",
                )
            }
            StageUploadContent::Stored {
                object_key,
                object_version_id,
                content_hash,
                size_bytes,
            } => {
                let digest = strip_sha256(&content_hash);
                if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(ApiError::invalid(
                        "stored stage content_hash must be a SHA-256 hex digest",
                    ));
                }
                if object_key.is_empty() || object_key.chars().any(char::is_control) {
                    return Err(ApiError::invalid("stored stage object key is invalid"));
                }
                (
                    None,
                    Some(object_key),
                    object_version_id,
                    size_bytes,
                    format!("sha256:{}", digest.to_ascii_lowercase()),
                    "resumable_upload",
                )
            }
        };
        let digest_bytes = hex::decode(strip_sha256(&content_hash))
            .map_err(|_| ApiError::invalid("stage content hash is invalid"))?;
        input_hasher.update(digest_bytes);

        let archive = if let Some(bytes) = bytes.as_deref() {
            crate::object_store::extract_archive(&upload.path, bytes)?
        } else {
            crate::object_store::ArchiveInspection::default()
        };
        let archive_inventory = archive.inventory.clone();
        let is_archive = archive.inventory.format.is_some();
        let media_type = detect_media_type(
            &upload.path,
            upload.media_type.as_deref(),
            bytes.as_deref().unwrap_or_default(),
        );
        let (readability, text_index) = if is_archive {
            ("unsupported".to_owned(), None)
        } else if let Some(bytes) = bytes.as_deref() {
            stage_text_index(bytes)
        } else {
            ("unsupported".to_owned(), None)
        };
        let mut inspection = json!({
            "source": upload_source,
            "archive": archive_inventory,
            "import_metadata": upload.import_metadata,
            "text_index_complete": false
        });
        if let Some((text, complete)) = text_index {
            inspection["search_text"] = Value::String(text);
            inspection["text_index_complete"] = Value::Bool(complete);
        }
        insert_prepared_path(&mut stage_paths, &upload.path)?;
        if stored_object_key.is_none() {
            expanded_bytes = expanded_bytes.saturating_add(declared_size_bytes);
        }
        prepared.push(PreparedStageEntry {
            path: upload.path.clone(),
            entry_kind: if is_archive { "archive" } else { "file" }.to_owned(),
            media_type,
            size_bytes: declared_size_bytes,
            content_hash: Some(content_hash),
            readability,
            inspection,
            bytes,
            stored_object_key,
            object_version_id,
        });

        warnings.extend(
            archive
                .inventory
                .warnings
                .iter()
                .map(|warning| format!("{}: {warning}", upload.path)),
        );
        for member in archive.members {
            let inventory = archive
                .inventory
                .entries
                .get(member.inventory_index)
                .ok_or_else(|| ApiError::Internal("archive inventory index was lost".to_owned()))?;
            let path = if inventory.quarantined {
                quarantine_stage_path(&upload.path, member.inventory_index, &inventory.path)
            } else {
                format!("{}!/{}", upload.path, inventory.path)
            };
            insert_prepared_path(&mut stage_paths, &path)?;
            let media_type = member
                .bytes
                .as_deref()
                .map(|bytes| detect_media_type(&inventory.path, None, bytes))
                .unwrap_or(None);
            let (readability, text_index) = if inventory.quarantined {
                ("quarantined".to_owned(), None)
            } else if inventory.entry_kind != "file" {
                ("unsupported".to_owned(), None)
            } else if let Some(bytes) = member.bytes.as_deref() {
                stage_text_index(bytes)
            } else {
                ("unsupported".to_owned(), None)
            };
            let mut inspection = json!({
                "source": "archive_member",
                "archive_path": upload.path,
                "member_path": inventory.path,
                "compressed_size_bytes": inventory.compressed_size_bytes,
                "quarantined": inventory.quarantined,
                "quarantine_reason": inventory.reason,
                "nested_archive_expanded": false,
                "text_index_complete": false
            });
            if let Some((text, complete)) = text_index {
                inspection["search_text"] = Value::String(text);
                inspection["text_index_complete"] = Value::Bool(complete);
            }
            if let Some(bytes) = &member.bytes {
                expanded_bytes = expanded_bytes.saturating_add(bytes.len() as u64);
            }
            prepared.push(PreparedStageEntry {
                path,
                entry_kind: if inventory.quarantined {
                    match inventory.reason.as_deref() {
                        Some("symlink") => "symlink".to_owned(),
                        Some("special_file") => "unreadable".to_owned(),
                        _ => inventory.entry_kind.clone(),
                    }
                } else {
                    inventory.entry_kind.clone()
                },
                media_type,
                size_bytes: inventory.size_bytes,
                content_hash: inventory.content_hash.clone(),
                readability,
                inspection,
                bytes: member.bytes,
                stored_object_key: None,
                object_version_id: None,
            });
        }
        if prepared.len() > MAX_STAGE_ENTRIES {
            return Err(ApiError::invalid(
                "stage inventory contains more than 2,000 entries",
            ));
        }
        if expanded_bytes > MAX_STAGE_EXPANDED_BYTES {
            return Err(ApiError::invalid(
                "stage inventory exceeds the 256 MiB expanded-size limit",
            ));
        }
    }

    prepared.sort_by(|left, right| left.path.cmp(&right.path));
    let inventory_value = Value::Array(
        prepared
            .iter()
            .map(|entry| {
                json!({
                    "path": entry.path,
                    "entry_kind": entry.entry_kind,
                    "media_type": entry.media_type,
                    "size_bytes": entry.size_bytes,
                    "content_hash": entry.content_hash,
                    "readability": entry.readability,
                    "quarantined": entry.readability == "quarantined"
                })
            })
            .collect(),
    );
    Ok((
        prepared,
        hex::encode(input_hasher.finalize()),
        hash_json(&inventory_value)?,
        warnings,
    ))
}

fn validate_stage_import_metadata(metadata: &Value) -> ApiResult<()> {
    let object = metadata
        .as_object()
        .ok_or_else(|| ApiError::invalid("staged import metadata must be a JSON object"))?;
    for name in ["modified_unix_ns", "mode"] {
        if let Some(value) = object.get(name)
            && !value.is_null()
            && value.as_u64().is_none()
        {
            return Err(ApiError::invalid(format!(
                "staged import metadata {name} must be an unsigned integer"
            )));
        }
    }
    if object
        .get("mode")
        .and_then(Value::as_u64)
        .is_some_and(|mode| mode > 0o777)
    {
        return Err(ApiError::invalid(
            "staged import metadata mode must contain only ordinary portable permission bits",
        ));
    }
    Ok(())
}

pub(crate) fn validate_stage_path(path: &str) -> ApiResult<()> {
    let parts = path.split('/').collect::<Vec<_>>();
    let reserved_description_path = parts.windows(3).any(|window| {
        window[0].eq_ignore_ascii_case(".carrystate")
            && window[1].eq_ignore_ascii_case("generated")
            && window[2].eq_ignore_ascii_case("descriptions")
    });
    let invalid = path.is_empty()
        || path.len() > MAX_STAGE_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path.contains("!/")
        || reserved_description_path
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."));
    if invalid {
        Err(ApiError::invalid(
            "stage paths must be safe relative paths of at most 1,024 bytes",
        ))
    } else {
        Ok(())
    }
}

fn is_canonical_generated_description_path(path: &str) -> bool {
    let Some(digest) = path
        .strip_prefix(".carrystate/generated/descriptions/")
        .and_then(|value| value.strip_suffix(".md"))
    else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_snapshot_paths(paths: &[String]) -> ApiResult<()> {
    for path in paths {
        if !is_canonical_generated_description_path(path) {
            validate_stage_path(path)?;
        }
    }
    let expected_descriptions = paths
        .iter()
        .filter(|path| !is_canonical_generated_description_path(path))
        .map(|path| asset_description::description_path(path))
        .collect::<HashSet<_>>();
    if paths.iter().any(|path| {
        is_canonical_generated_description_path(path) && !expected_descriptions.contains(path)
    }) {
        return Err(ApiError::invalid(
            "snapshot_paths may retain only generated descriptions for source paths in the same snapshot",
        ));
    }
    Ok(())
}

fn insert_prepared_path(paths: &mut HashSet<String>, path: &str) -> ApiResult<()> {
    if path.len() > MAX_STAGE_PATH_BYTES {
        return Err(ApiError::invalid(format!(
            "stage inventory path exceeds 1,024 bytes: {path}"
        )));
    }
    if paths.insert(path.to_owned()) {
        Ok(())
    } else {
        Err(ApiError::invalid(format!(
            "stage inventory contains a duplicate path: {path}"
        )))
    }
}

fn quarantine_stage_path(archive_path: &str, index: usize, original_path: &str) -> String {
    let digest = hex::encode(Sha256::digest(original_path.as_bytes()));
    format!(
        "{archive_path}!/.straylight-quarantine/{index:05}-{}",
        &digest[..12]
    )
}

fn staged_source_ref(scope_ref: &str, stable_import_id: &str, path: &str) -> String {
    let mut source_identity = Sha256::new();
    source_identity.update(scope_ref.as_bytes());
    source_identity.update([0]);
    source_identity.update(stable_import_id.as_bytes());
    source_identity.update([0]);
    source_identity.update(path.as_bytes());
    format!("staged-import:{}", hex::encode(source_identity.finalize()))
}

fn staged_source_version(
    content_hash: &str,
    media_type: &str,
    import_metadata: Option<&Value>,
) -> ApiResult<String> {
    Ok(format!(
        "sha256:{}",
        hash_json(&json!({
            "content_hash": strip_sha256(content_hash),
            "media_type": media_type,
            "import_metadata": import_metadata.cloned().unwrap_or_else(|| json!({}))
        }))?
    ))
}

fn detect_media_type(path: &str, supplied: Option<&str>, bytes: &[u8]) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let extension_type = if lower.ends_with(".docx") {
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
    } else if lower.ends_with(".xlsx") {
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
    } else if lower.ends_with(".pptx") {
        Some("application/vnd.openxmlformats-officedocument.presentationml.presentation")
    } else if lower.ends_with(".sqlite") || lower.ends_with(".sqlite3") || lower.ends_with(".db") {
        Some("application/vnd.sqlite3")
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        Some("text/markdown")
    } else if lower.ends_with(".json") || lower.ends_with(".jsonl") {
        Some("application/json")
    } else if lower.ends_with(".csv") {
        Some("text/csv")
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        Some("text/html")
    } else if lower.ends_with(".xml") {
        Some("application/xml")
    } else if lower.ends_with(".sql") {
        Some("application/sql")
    } else if [
        ".txt", ".log", ".yaml", ".yml", ".toml", ".ini", ".conf", ".rs", ".go", ".py", ".sh",
        ".js", ".jsx", ".ts", ".tsx", ".css",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
    {
        Some("text/plain")
    } else {
        None
    };
    if let Some(kind) = infer::get(bytes) {
        if kind.mime_type() == "application/zip"
            && let Some(extension_type) = extension_type
            && extension_type.starts_with("application/vnd.openxmlformats")
        {
            return Some(extension_type.to_owned());
        }
        return Some(kind.mime_type().to_owned());
    }
    if let Some(extension_type) = extension_type {
        return Some(extension_type.to_owned());
    }
    supplied
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .map(|value| value.essence_str().to_owned())
}

fn stage_text_index(bytes: &[u8]) -> (String, Option<(String, bool)>) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ("unsupported".to_owned(), None);
    };
    if text.contains('\0') {
        return ("unsupported".to_owned(), None);
    }
    let end = text
        .char_indices()
        .nth(MAX_STAGE_TEXT_INDEX_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    (
        "readable".to_owned(),
        Some((text[..end].to_owned(), end == text.len())),
    )
}

async fn expand_stage_promotion(
    state: &AppState,
    auth: &AuthContext,
    mut request: SaveRequest,
) -> ApiResult<(SaveRequest, Vec<StagePromotion>, Option<Value>)> {
    let promotion_payloads: Vec<Value> = request
        .items
        .iter()
        .filter(|item| item.kind == SaveKind::ImportReceipt)
        .map(|item| item.payload.clone())
        .collect();
    if promotion_payloads.is_empty() {
        return Ok((request, Vec::new(), None));
    }
    if promotion_payloads.len() != request.items.len() {
        return Err(ApiError::invalid(
            "stage promotions cannot be mixed with ordinary save items",
        ));
    }
    let mut promotions = Vec::with_capacity(promotion_payloads.len());
    let mut expanded_items = Vec::new();
    let mut replays = Vec::new();
    for payload in &promotion_payloads {
        match expand_single_stage_promotion(state, auth, &request, payload, expanded_items.len())
            .await?
        {
            StageExpansion::Fresh { items, promotion } => {
                expanded_items.extend(items);
                promotions.push(promotion);
            }
            StageExpansion::Replay {
                receipt,
                import_receipt,
            } => replays.push((receipt, import_receipt)),
        }
    }
    if !replays.is_empty() {
        if !promotions.is_empty() {
            return Err(ApiError::conflict(
                "import_generation_inconsistent",
                "an atomic import generation cannot mix committed and uncommitted stages",
                json!({"replayed_stages": replays.len(), "fresh_stages": promotions.len()}),
            ));
        }
        let operation_id = replays[0]
            .0
            .get("operation_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::Internal("replayed import receipt lacks operation_id".into()))?
            .to_owned();
        if replays.iter().any(|(receipt, _)| {
            receipt.get("operation_id").and_then(Value::as_str) != Some(operation_id.as_str())
        }) {
            return Err(ApiError::conflict(
                "import_generation_inconsistent",
                "the import generation was previously committed by multiple operations",
                json!({}),
            ));
        }
        let import_receipts = replays
            .iter()
            .map(|(_, import_receipt)| import_receipt.clone())
            .collect::<Vec<_>>();
        let mut receipt = replays.remove(0).0;
        if let Some(object) = receipt.as_object_mut() {
            object.insert("import_receipts".to_owned(), json!(import_receipts));
            object.insert("replayed".to_owned(), Value::Bool(true));
            object.insert("stable_replay".to_owned(), Value::Bool(true));
            object.insert("replay_outcome".to_owned(), json!("exact_replay"));
        }
        return Ok((request, Vec::new(), Some(receipt)));
    }
    let base_revision_id = promotions
        .first()
        .map(|promotion| promotion.base_revision_id)
        .ok_or_else(|| ApiError::Internal("stage expansion produced no promotion".to_owned()))?;
    let stable_import_id = promotions[0].stable_import_id.clone();
    let generation_id = promotions[0].generation_id.clone();
    if promotions.iter().any(|promotion| {
        promotion.base_revision_id != base_revision_id
            || promotion.stable_import_id != stable_import_id
            || promotion.generation_id != generation_id
    }) {
        return Err(ApiError::conflict(
            "import_generation_mismatch",
            "all stages in an atomic import must share a revision, vault identity, and generation",
            json!({}),
        ));
    }
    if promotions
        .iter()
        .filter(|promotion| promotion.snapshot_paths.is_some())
        .count()
        > 1
    {
        return Err(ApiError::invalid(
            "an atomic import generation may provide snapshot_paths only once",
        ));
    }
    if expanded_items.len() > 100_000 {
        return Err(ApiError::invalid(
            "an atomic import generation is limited to 100,000 promoted files",
        ));
    }
    request.items = expanded_items;
    request.base_corpus_revision = Some(revision_ref(base_revision_id));
    Ok((request, promotions, None))
}

async fn expand_single_stage_promotion(
    state: &AppState,
    auth: &AuthContext,
    request: &SaveRequest,
    payload: &Value,
    item_order_offset: usize,
) -> ApiResult<StageExpansion> {
    let stage_ref = payload_string(payload, "stage_ref")
        .ok_or_else(|| ApiError::invalid("stage promotion requires stage_ref"))?;
    let raw_stage_ref = stage_ref
        .strip_prefix("stage:")
        .ok_or_else(|| ApiError::invalid("stage_ref is invalid"))?;
    let (raw_stage_id, requested_inventory_hash) = raw_stage_ref
        .split_once('@')
        .map(|(id, revision)| (id, Some(revision.trim_start_matches("sha256:").to_owned())))
        .unwrap_or((raw_stage_ref, None));
    let stage_id =
        Uuid::parse_str(raw_stage_id).map_err(|_| ApiError::invalid("stage_ref is invalid"))?;
    let mut selected_paths = payload_strings(payload, "selected_entries");
    selected_paths.sort();
    let unique_paths: HashSet<_> = selected_paths.iter().collect();
    if unique_paths.len() != selected_paths.len() {
        return Err(ApiError::invalid(
            "selected_entries cannot contain duplicate paths",
        ));
    }
    let (snapshot_paths, snapshot_hash) = match payload.get("snapshot_paths") {
        None => (None, None),
        Some(Value::Array(values)) => {
            if values.len() > 100_000 {
                return Err(ApiError::invalid(
                    "snapshot_paths is limited to 100,000 paths",
                ));
            }
            let mut paths = values
                .iter()
                .map(|value| {
                    value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                        ApiError::invalid("snapshot_paths must contain only strings")
                    })
                })
                .collect::<ApiResult<Vec<_>>>()?;
            if paths.iter().map(String::len).sum::<usize>() > 4 * 1024 * 1024 {
                return Err(ApiError::invalid(
                    "snapshot_paths exceeds the 4 MiB path budget",
                ));
            }
            validate_snapshot_paths(&paths)?;
            paths.sort();
            let original_len = paths.len();
            paths.dedup();
            if paths.len() != original_len {
                return Err(ApiError::invalid(
                    "snapshot_paths cannot contain duplicate paths",
                ));
            }
            let hash = hash_json(&json!(&paths))?;
            (Some(paths), Some(hash))
        }
        Some(_) => return Err(ApiError::invalid("snapshot_paths must be an array")),
    };

    let mut tx = state.begin_read(auth).await?;
    let stage = sqlx::query(
        r#"
        SELECT stage.scope_id,scope.scope_ref,stage.stable_import_id,
               stage.base_corpus_revision_id,stage.input_hash,stage.inventory_hash,
               stage.status,stage.expires_at
        FROM straylight.stages AS stage
        JOIN straylight.scopes AS scope
          ON scope.user_id=stage.user_id AND scope.id=stage.scope_id
        WHERE stage.user_id=$1 AND stage.id=$2
          AND scope.scope_ref=ANY($3::text[])
          AND stage.credential_id=$4
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id)
    .bind(&auth.scope_refs)
    .bind(auth.credential_id.0)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("stage_not_found", &stage_ref))?;
    let scope_ref: String = stage.try_get("scope_ref")?;
    let stage_scope_id: Uuid = stage.try_get("scope_id")?;
    if scope_ref != request.scope {
        return Err(ApiError::public(
            http::StatusCode::FORBIDDEN,
            "scope_denied",
            "stage and save request scopes differ",
        ));
    }
    let base_revision_id: Uuid = stage.try_get("base_corpus_revision_id")?;
    let stage_import_id = stage
        .try_get::<Option<String>, _>("stable_import_id")?
        .unwrap_or_else(|| format!("stage:{stage_id}"));
    let stable_import_id =
        payload_string(payload, "stable_import_id").unwrap_or_else(|| stage_import_id.clone());
    let generation_id =
        payload_string(payload, "generation_id").unwrap_or_else(|| stage_import_id.clone());
    for (name, value) in [
        ("stable_import_id", stable_import_id.as_str()),
        ("generation_id", generation_id.as_str()),
    ] {
        if value.is_empty() || value.len() > 240 || value.chars().any(char::is_control) {
            return Err(ApiError::invalid(format!(
                "{name} must contain 1 to 240 non-control characters"
            )));
        }
    }
    let input_hash: String = stage.try_get("input_hash")?;
    let inventory_hash = stage
        .try_get::<Option<String>, _>("inventory_hash")?
        .ok_or_else(|| ApiError::invalid("stage inventory is incomplete"))?;
    if requested_inventory_hash
        .as_deref()
        .is_some_and(|requested| requested != inventory_hash)
    {
        return Err(ApiError::conflict(
            "stage_revision_mismatch",
            "stage_ref does not match the immutable inventory hash",
            json!({
                "requested": requested_inventory_hash,
                "actual": format!("sha256:{inventory_hash}")
            }),
        ));
    }
    let rows = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.entry_kind,entry.media_type,
               entry.size_bytes,entry.content_hash,entry.readability,entry.asset_id,
               entry.asset_version,entry.inspection,asset.object_key,
               asset.object_version_id
        FROM straylight.staged_entries AS entry
        LEFT JOIN straylight.asset_versions AS asset
          ON asset.user_id=entry.user_id AND asset.asset_id=entry.asset_id
         AND asset.version=entry.asset_version
        WHERE entry.user_id=$1 AND entry.stage_id=$2
          AND (
            (cardinality($3::text[]) = 0
              AND entry.entry_kind IN ('file','archive')
              AND entry.readability IN ('readable','unsupported')
              AND entry.asset_id IS NOT NULL)
            OR (cardinality($3::text[]) > 0 AND entry.path=ANY($3))
          )
        ORDER BY entry.path
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_id)
    .bind(&selected_paths)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        return Err(ApiError::invalid("stage promotion selected no entries"));
    }
    if !selected_paths.is_empty() && rows.len() != selected_paths.len() {
        return Err(ApiError::invalid(
            "one or more selected stage entries do not exist",
        ));
    }
    let selected_paths: Vec<String> = rows
        .iter()
        .map(|row| row.try_get("path"))
        .collect::<Result<_, sqlx::Error>>()?;

    let prior_receipts = sqlx::query(
        r#"
        SELECT receipt.id,receipt.receipt AS import_receipt,
               operation.receipt AS commit_receipt
        FROM straylight.import_receipts AS receipt
        JOIN straylight.write_operations AS operation
          ON operation.user_id=receipt.user_id
         AND operation.id=replace(receipt.receipt->>'operation_id','operation:','')::uuid
        WHERE receipt.user_id=$1 AND receipt.scope_id=$2
          AND receipt.stable_import_id=$3 AND receipt.input_hash=$4
          AND receipt.inventory_hash=$5
          AND coalesce(
            receipt.receipt->>'generation_id',
            receipt.stable_import_id
          )=$6
        ORDER BY receipt.created_at DESC
        "#,
    )
    .bind(auth.user_id.0)
    .bind(stage_scope_id)
    .bind(&stable_import_id)
    .bind(&input_hash)
    .bind(&inventory_hash)
    .bind(&generation_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut prior_selection = None;
    for prior in prior_receipts {
        let import_receipt: Value = prior.try_get("import_receipt")?;
        let prior_paths = import_receipt
            .get("selected_entries")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let prior_snapshot_hash = import_receipt
            .get("snapshot_manifest_hash")
            .and_then(Value::as_str)
            .map(|value| strip_sha256(value).to_owned());
        if prior_paths == selected_paths && prior_snapshot_hash == snapshot_hash {
            let receipt_id: Uuid = prior.try_get("id")?;
            let mut receipt: Value = prior.try_get("commit_receipt")?;
            if let Some(object) = receipt.as_object_mut() {
                object.insert(
                    "import_receipt".to_owned(),
                    json!(format!("import:{receipt_id}")),
                );
                object.insert("replayed".to_owned(), Value::Bool(true));
                object.insert("stable_replay".to_owned(), Value::Bool(true));
                object.insert("replay_outcome".to_owned(), json!("exact_replay"));
            }
            tx.commit().await?;
            return Ok(StageExpansion::Replay {
                receipt,
                import_receipt: format!("import:{receipt_id}"),
            });
        }
        if prior_paths != selected_paths || prior_snapshot_hash != snapshot_hash {
            prior_selection.get_or_insert(prior_paths);
        }
    }
    if let Some(prior_selection) = prior_selection {
        return Err(ApiError::conflict(
            "import_revision_delta",
            "stable import identity was already promoted with a different selection",
            json!({
                "stage_ref": stage_ref,
                "prior_selection": prior_selection,
                "requested_selection": selected_paths,
                "replay_outcome": "revision_delta"
            }),
        ));
    }

    let status: String = stage.try_get("status")?;
    let expires_at: DateTime<Utc> = stage.try_get("expires_at")?;
    if status != "ready" || expires_at <= Utc::now() {
        return Err(ApiError::conflict(
            "stage_not_ready",
            "the stage is not promotable",
            json!({"stage_ref": stage_ref, "status": status, "expires_at": expires_at}),
        ));
    }
    if let Some(expected) = &request.base_corpus_revision
        && parse_revision_ref(expected)? != base_revision_id
    {
        return Err(ApiError::conflict(
            "revision_conflict",
            "stage promotion must use the revision captured when staging began",
            json!({
                "stage_revision": revision_ref(base_revision_id),
                "requested_revision": expected
            }),
        ));
    }
    tx.commit().await?;

    let mut save_items = Vec::with_capacity(rows.len());
    let mut entries = Vec::with_capacity(rows.len());
    for (local_order, row) in rows.into_iter().enumerate() {
        let item_order = item_order_offset
            .checked_add(local_order)
            .ok_or_else(|| ApiError::Internal("stage item order overflow".to_owned()))?;
        let staged_entry_id: Uuid = row.try_get("id")?;
        let path: String = row.try_get("path")?;
        let entry_kind: String = row.try_get("entry_kind")?;
        let readability: String = row.try_get("readability")?;
        if !matches!(entry_kind.as_str(), "file" | "archive")
            || !matches!(readability.as_str(), "readable" | "unsupported")
        {
            return Err(ApiError::invalid(format!(
                "stage entry is not a safe promotable asset: {path} ({entry_kind}/{readability})"
            )));
        }
        let object_key: Option<String> = row.try_get("object_key")?;
        let object_key = object_key.ok_or_else(|| {
            ApiError::invalid(format!("stage entry has no immutable asset: {path}"))
        })?;
        let object_version_id: Option<String> = row.try_get("object_version_id")?;
        let expected_hash: String = row.try_get("content_hash")?;
        let expected_size = u64::try_from(row.try_get::<i64, _>("size_bytes")?)
            .map_err(|_| ApiError::Internal("staged asset size is negative".to_owned()))?;
        let media_type = row
            .try_get::<Option<String>, _>("media_type")?
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let inspection: Value = row.try_get("inspection")?;
        let generated_description =
            inspection.get("source").and_then(Value::as_str) == Some("asset_description");
        let content = if readability == "readable" {
            let bytes = state
                .object_store
                .get_version(&object_key, object_version_id.as_deref())
                .await?;
            let actual_hash = hex::encode(Sha256::digest(&bytes));
            if actual_hash != expected_hash
                || u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_size
            {
                return Err(ApiError::conflict(
                    "content_hash_mismatch",
                    "staged content no longer matches its inspected identity",
                    json!({"path": path}),
                ));
            }
            let content = String::from_utf8(bytes.to_vec()).map_err(|_| {
                ApiError::conflict(
                    "content_classification_mismatch",
                    "readable staged content is no longer valid UTF-8",
                    json!({"path": path}),
                )
            })?;
            if content.contains('\0') {
                return Err(ApiError::conflict(
                    "content_classification_mismatch",
                    "readable staged content contains binary NUL bytes",
                    json!({"path": path}),
                ));
            }
            Some(content)
        } else {
            state
                .object_store
                .verify_object_content_version(
                    &object_key,
                    object_version_id.as_deref(),
                    &expected_hash,
                    expected_size,
                )
                .await
                .map_err(|error| {
                    tracing::warn!(?error, %path, "opaque staged asset verification failed");
                    error
                })?;
            None
        };
        let asset_id: Uuid = row
            .try_get::<Option<Uuid>, _>("asset_id")?
            .ok_or_else(|| ApiError::invalid(format!("stage entry has no asset id: {path}")))?;
        let asset_version: i32 =
            row.try_get::<Option<i32>, _>("asset_version")?
                .ok_or_else(|| {
                    ApiError::invalid(format!("stage entry has no asset version: {path}"))
                })?;
        let source_ref = staged_source_ref(&request.scope, &stable_import_id, &path);
        let source_version = staged_source_version(
            &expected_hash,
            &media_type,
            inspection.get("import_metadata"),
        )?;
        let mut source_payload = json!({
            "path": &path,
            "source_ref": source_ref,
            "source_kind": if generated_description {
                "derived_asset_description"
            } else {
                "content_pack"
            },
            "source_version": source_version,
            "content_hash": &expected_hash,
            "title": path.rsplit('/').next().unwrap_or(&path),
            "media_type": &media_type,
            "stage_ref": &stage_ref,
            "metadata": {
                "stable_import_id": &stable_import_id,
                "stage_entry_ref": format!("stage-entry:{staged_entry_id}"),
                "input_hash": format!("sha256:{input_hash}"),
                "inventory_hash": format!("sha256:{inventory_hash}"),
                "readability": &readability,
                "entry_kind": &entry_kind,
                "import_metadata": inspection.get("import_metadata"),
                "generated": generated_description,
                "derivative": generated_description,
                "authoritative": !generated_description,
                "description_status": inspection.get("description_status"),
                "description_method": inspection.get("description_method"),
                "prompt_version": inspection.get("prompt_version"),
                "original_path": inspection.get("original_path")
            },
            "staged_asset": {
                "entry_ref": format!("stage-entry:{staged_entry_id}"),
                "asset_ref": format!("asset:{asset_id}"),
                "asset_version": asset_version,
                "content_hash": format!("sha256:{expected_hash}"),
                "media_type": &media_type
            }
        });
        if generated_description {
            let describes_asset_ref = inspection
                .get("describes_asset_ref")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ApiError::Internal(
                        "generated asset description lacks describes_asset_ref".to_owned(),
                    )
                })?;
            let describes_asset_version = inspection
                .get("describes_asset_version")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| {
                    ApiError::Internal(
                        "generated asset description lacks describes_asset_version".to_owned(),
                    )
                })?;
            source_payload["describes_asset"] = json!({
                "asset_ref": describes_asset_ref,
                "asset_version": describes_asset_version,
                "description_of_entry_ref": inspection.get("description_of_entry_ref")
            });
        }
        if let Some(content) = content {
            source_payload["content"] = Value::String(content);
        }
        save_items.push(SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::Source,
            reference: Some(format!("stage-entry:{staged_entry_id}")),
            expected_version: None,
            payload: source_payload,
        });
        entries.push(StagePromotionEntry {
            staged_entry_id,
            item_order,
            path,
            entry_kind,
            readability,
            content_hash: expected_hash,
            asset_id,
            asset_version,
        });
    }
    let selection_hash = hash_json(&Value::Array(
        entries
            .iter()
            .map(|entry| {
                json!({
                    "path": entry.path,
                    "content_hash": entry.content_hash,
                    "asset_id": entry.asset_id,
                    "asset_version": entry.asset_version
                })
            })
            .collect(),
    ))?;
    Ok(StageExpansion::Fresh {
        items: save_items,
        promotion: StagePromotion {
            stage_id,
            stable_import_id,
            generation_id,
            input_hash,
            inventory_hash,
            selection_hash,
            snapshot_paths,
            snapshot_hash,
            base_revision_id,
            entries,
        },
    })
}

fn processing_order(kind: SaveKind) -> u8 {
    match kind {
        SaveKind::Source => 0,
        SaveKind::Evidence => 1,
        SaveKind::TemporalSpec => 2,
        SaveKind::Object | SaveKind::Policy | SaveKind::Asset => 3,
        SaveKind::RecurrenceSpec => 4,
        SaveKind::Claim | SaveKind::StateAssignment | SaveKind::StateTransition => 5,
        SaveKind::IdentityReview => 6,
        SaveKind::Relation => 7,
        SaveKind::ImportReceipt => 8,
        SaveKind::Checkpoint => 9,
    }
}

fn preallocate_refs(items: &[SaveItem], context: &mut WriteContext<'_>) -> ApiResult<()> {
    for (order, item) in items.iter().enumerate() {
        let kind = match item.kind {
            SaveKind::Object => RecordKind::Object,
            SaveKind::Claim | SaveKind::StateAssignment | SaveKind::StateTransition => {
                RecordKind::Claim
            }
            SaveKind::Evidence => RecordKind::Evidence,
            SaveKind::Relation => RecordKind::Relation,
            SaveKind::TemporalSpec => RecordKind::TemporalSpec,
            SaveKind::RecurrenceSpec => RecordKind::RecurrenceSpec,
            SaveKind::Checkpoint => RecordKind::Checkpoint,
            SaveKind::Source => RecordKind::SourceEpisode,
            SaveKind::Asset => RecordKind::Asset,
            SaveKind::Policy | SaveKind::IdentityReview | SaveKind::ImportReceipt => continue,
        };
        let parsed = item
            .reference
            .as_deref()
            .and_then(|reference| RecordRef::parse(reference).ok())
            .filter(|reference| reference.kind == kind);
        let stable_revision = matches!(kind, RecordKind::Object | RecordKind::Relation)
            && item.action != SaveAction::Create;
        let id = if item.action == SaveAction::Create || stable_revision {
            parsed
                .map(|reference| reference.id)
                .unwrap_or_else(Uuid::now_v7)
        } else {
            Uuid::now_v7()
        };
        let canonical = RecordRef { kind, id }.to_string();
        context.item_ids.insert(order, id);
        context.local_refs.insert(canonical, (id, kind));
        if let Some(reference) = &item.reference
            && (item.action == SaveAction::Create || stable_revision)
        {
            context.local_refs.insert(reference.clone(), (id, kind));
        }
    }
    Ok(())
}

fn allocated_id(order: usize, context: &WriteContext<'_>) -> ApiResult<Uuid> {
    context
        .item_ids
        .get(&order)
        .copied()
        .or_else(|| Some(Uuid::now_v7()))
        .ok_or_else(|| ApiError::Internal("preallocated item ID was lost".to_owned()))
}

async fn process_item(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    id: Uuid,
) -> ApiResult<()> {
    if item.action == SaveAction::Tombstone {
        return process_tombstone(tx, context, order, item, id).await;
    }
    match item.kind {
        SaveKind::Source => process_source(tx, context, order, item, id).await,
        SaveKind::Evidence => process_evidence(tx, context, order, item, id).await,
        SaveKind::TemporalSpec => process_temporal(tx, context, order, item, id).await,
        SaveKind::Object => process_object(tx, context, order, item, id).await,
        SaveKind::Claim | SaveKind::StateAssignment | SaveKind::StateTransition => {
            process_claim(tx, context, order, item, id).await
        }
        SaveKind::Relation => process_relation(tx, context, order, item, id).await,
        SaveKind::RecurrenceSpec => process_recurrence(tx, context, order, item, id).await,
        SaveKind::Policy => process_policy(tx, context, order, item, id).await,
        SaveKind::Asset => Err(ApiError::invalid(
            "assets enter through memory.stage; save references a staged asset",
        )),
        SaveKind::IdentityReview => process_identity_review(tx, context, order, item, id).await,
        SaveKind::ImportReceipt => process_import_receipt(tx, context, order, item, id).await,
        SaveKind::Checkpoint => unreachable!("checkpoints are processed after revision creation"),
    }
}

async fn resolve_scope(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope_ref: &str,
) -> ApiResult<Uuid> {
    if !auth.scope_refs.iter().any(|scope| scope == scope_ref) {
        return Err(ApiError::public(
            http::StatusCode::FORBIDDEN,
            "scope_denied",
            "credential does not grant the requested scope",
        ));
    }
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.scopes WHERE user_id=$1 AND scope_ref=$2",
    )
    .bind(auth.user_id.0)
    .bind(scope_ref)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("scope_not_found", scope_ref))
}

async fn default_policy(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> ApiResult<(Uuid, i32)> {
    sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT id,current_version FROM straylight.policies WHERE user_id=$1 AND is_default",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(ApiError::from)
}

async fn active_revision(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
) -> ApiResult<(Uuid, i64, String, i64)> {
    let row = sqlx::query(
        r#"
        SELECT revision.id, revision.revision_number, revision.manifest_hash,
               manifest.generation
        FROM straylight.active_manifests AS manifest
        JOIN straylight.corpus_revisions AS revision
          ON revision.user_id=manifest.user_id
         AND revision.id=manifest.active_corpus_revision_id
        WHERE manifest.user_id=$1 AND manifest.scope_id=$2
        FOR UPDATE OF manifest
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok((
        row.try_get("id")?,
        row.try_get("revision_number")?,
        row.try_get("manifest_hash")?,
        row.try_get("generation")?,
    ))
}

async fn load_members(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    revision_id: Uuid,
) -> ApiResult<BTreeMap<Uuid, Member>> {
    let rows = sqlx::query(
        r#"
        SELECT record_id,record_kind,record_version,disposition
        FROM straylight.corpus_members
        WHERE user_id=$1 AND corpus_revision_id=$2
        "#,
    )
    .bind(user_id)
    .bind(revision_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut members = BTreeMap::new();
    for row in rows {
        let record_id = row.try_get("record_id")?;
        members.insert(
            record_id,
            Member {
                record_id,
                record_kind: row.try_get("record_kind")?,
                record_version: row.try_get("record_version")?,
                disposition: row.try_get("disposition")?,
            },
        );
    }
    Ok(members)
}

async fn insert_members(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
    members: &BTreeMap<Uuid, Member>,
) -> ApiResult<()> {
    for member in members.values() {
        if member.record_kind == "checkpoint" {
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
        .bind(member.record_id)
        .bind(&member.record_kind)
        .bind(member.record_version)
        .bind(&member.disposition)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn replay_receipt(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    key: &str,
    request_hash: &str,
) -> ApiResult<Option<Value>> {
    let row = sqlx::query(
        r#"
        SELECT key_row.request_hash, operation.receipt, operation.status
        FROM straylight.idempotency_keys AS key_row
        JOIN straylight.write_operations AS operation
          ON operation.user_id=key_row.user_id AND operation.id=key_row.operation_id
        WHERE key_row.user_id=$1 AND key_row.scope_id=$2 AND key_row.idempotency_key=$3
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let existing_hash: String = row.try_get("request_hash")?;
    if existing_hash != request_hash {
        return Err(ApiError::conflict(
            "idempotency_conflict",
            "idempotency key was already used for a different request",
            json!({"idempotency_key": key}),
        ));
    }
    let mut receipt: Value = row
        .try_get::<Option<Value>, _>("receipt")?
        .unwrap_or_else(|| json!({"status": row.try_get::<String,_>("status").unwrap_or_else(|_| "pending".to_owned())}));
    if let Some(object) = receipt.as_object_mut() {
        object.insert("replayed".to_owned(), Value::Bool(true));
    }
    Ok(Some(receipt))
}

fn manifest_hash(members: &BTreeMap<Uuid, Member>) -> String {
    let mut hasher = Sha256::new();
    for member in members.values() {
        hasher.update(member.record_id.as_bytes());
        hasher.update([0]);
        hasher.update(member.record_kind.as_bytes());
        hasher.update([0]);
        hasher.update(member.record_version.unwrap_or(0).to_be_bytes());
        hasher.update([0]);
        hasher.update(member.disposition.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

fn hash_json(value: &Value) -> ApiResult<String> {
    Ok(hex::encode(Sha256::digest(
        canonical_json(value)?.as_bytes(),
    )))
}

fn revision_ref(id: Uuid) -> String {
    format!("revision:{id}")
}

fn parse_revision_ref(value: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.trim_start_matches("revision:"))
        .map_err(|_| ApiError::invalid("invalid corpus revision ref"))
}

fn action_name(action: SaveAction) -> &'static str {
    match action {
        SaveAction::Create => "create",
        SaveAction::Revise => "revise",
        SaveAction::Supersede => "supersede",
        SaveAction::Retract => "retract",
        SaveAction::Relate => "relate",
        SaveAction::Tombstone => "tombstone",
    }
}

fn kind_name(kind: SaveKind) -> &'static str {
    match kind {
        SaveKind::Object => "object",
        SaveKind::Claim => "claim",
        SaveKind::Evidence => "evidence",
        SaveKind::Relation => "relation",
        SaveKind::StateAssignment => "state_assignment",
        SaveKind::StateTransition => "state_transition",
        SaveKind::TemporalSpec => "temporal_spec",
        SaveKind::RecurrenceSpec => "recurrence_spec",
        SaveKind::Checkpoint => "checkpoint",
        SaveKind::Source => "source",
        SaveKind::Policy => "policy",
        SaveKind::Asset => "asset",
        SaveKind::IdentityReview => "identity_review",
        SaveKind::ImportReceipt => "import_receipt",
    }
}

fn payload_object(payload: &Value) -> ApiResult<&Map<String, Value>> {
    payload
        .as_object()
        .ok_or_else(|| ApiError::invalid("save item payload must be an object"))
}

fn payload_string(payload: &Value, name: &str) -> Option<String> {
    payload
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn payload_strings(payload: &Value, name: &str) -> Vec<String> {
    payload
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn source_text(payload: &Value) -> Option<String> {
    payload_string(payload, "source_text").or_else(|| {
        payload
            .get("content")
            .and_then(|content| content.get("source_text"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

async fn session_scope_ref(
    state: &AppState,
    auth: &AuthContext,
    session_ref: &str,
) -> ApiResult<String> {
    let session_id = Uuid::parse_str(session_ref.trim_start_matches("session:"))
        .map_err(|_| ApiError::invalid("invalid session ref"))?;
    let mut tx = state.begin_read(auth).await?;
    let scope_ref = sqlx::query_scalar::<_, String>(
        r#"
        SELECT scope.scope_ref
        FROM straylight.sessions AS session
        JOIN straylight.scopes AS scope ON scope.user_id=session.user_id AND scope.id=session.scope_id
        WHERE session.user_id=$1 AND session.id=$2
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", session_ref))?;
    tx.commit().await?;
    Ok(scope_ref)
}

// Item processors follow below. They stay in this module so every mutation shares
// one transaction, idempotency record, corpus revision, and audit receipt.

async fn staged_asset_binding(
    tx: &mut Transaction<'_, Postgres>,
    context: &WriteContext<'_>,
    payload: &Map<String, Value>,
) -> ApiResult<Option<StagedAssetBinding>> {
    let Some(staged) = payload.get("staged_asset").and_then(Value::as_object) else {
        return Ok(None);
    };
    let entry_ref = staged
        .get("entry_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("staged_asset requires entry_ref"))?;
    let staged_entry_id = Uuid::parse_str(entry_ref.trim_start_matches("stage-entry:"))
        .map_err(|_| ApiError::invalid("staged_asset entry_ref is invalid"))?;
    let asset_ref = staged
        .get("asset_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("staged_asset requires asset_ref"))?;
    let asset_id = Uuid::parse_str(asset_ref.trim_start_matches("asset:"))
        .map_err(|_| ApiError::invalid("staged_asset asset_ref is invalid"))?;
    let asset_version = staged
        .get("asset_version")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| ApiError::invalid("staged_asset requires a valid asset_version"))?;
    let expected_hash = staged
        .get("content_hash")
        .and_then(Value::as_str)
        .map(strip_sha256)
        .ok_or_else(|| ApiError::invalid("staged_asset requires content_hash"))?;
    let row = sqlx::query(
        r#"
        SELECT entry.content_hash,asset.media_type
        FROM straylight.staged_entries AS entry
        JOIN straylight.stages AS stage
          ON stage.user_id=entry.user_id AND stage.id=entry.stage_id
        JOIN straylight.asset_versions AS asset
          ON asset.user_id=entry.user_id AND asset.asset_id=entry.asset_id
         AND asset.version=entry.asset_version
        WHERE entry.user_id=$1 AND entry.scope_id=$2 AND entry.id=$3
          AND entry.asset_id=$4 AND entry.asset_version=$5
          AND entry.entry_kind IN ('file','archive')
          AND entry.readability IN ('readable','unsupported')
          AND stage.status IN ('ready','promoted')
          AND stage.expires_at > clock_timestamp()
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(staged_entry_id)
    .bind(asset_id)
    .bind(asset_version)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("staged_asset_not_found", entry_ref))?;
    let content_hash: String = row.try_get("content_hash")?;
    if content_hash != expected_hash {
        return Err(ApiError::conflict(
            "content_hash_mismatch",
            "staged asset binding does not match the immutable entry hash",
            json!({"entry_ref": entry_ref}),
        ));
    }
    Ok(Some(StagedAssetBinding {
        staged_entry_id,
        asset_id,
        asset_version,
        content_hash,
        media_type: row.try_get("media_type")?,
    }))
}

async fn link_staged_asset(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    source_id: Uuid,
    binding: &StagedAssetBinding,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.source_asset_links (
          user_id,source_episode_id,asset_id,asset_version,role
        ) VALUES ($1,$2,$3,$4,'source_native')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(source_id)
    .bind(binding.asset_id)
    .bind(binding.asset_version)
    .execute(&mut **tx)
    .await?;
    add_member(
        context,
        binding.asset_id,
        "asset",
        Some(binding.asset_version),
        "active",
    );
    context.local_refs.insert(
        format!("asset:{}", binding.asset_id),
        (binding.asset_id, RecordKind::Asset),
    );
    Ok(())
}

async fn described_asset_binding(
    tx: &mut Transaction<'_, Postgres>,
    context: &WriteContext<'_>,
    payload: &Map<String, Value>,
    staged_asset: Option<&StagedAssetBinding>,
) -> ApiResult<Option<DescribedAssetBinding>> {
    let Some(described) = payload.get("describes_asset").and_then(Value::as_object) else {
        return Ok(None);
    };
    let staged_asset = staged_asset.ok_or_else(|| {
        ApiError::invalid("describes_asset is only valid for a promoted staged source")
    })?;
    let asset_id = described
        .get("asset_ref")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value.trim_start_matches("asset:")).ok())
        .ok_or_else(|| ApiError::invalid("describes_asset asset_ref is invalid"))?;
    let asset_version = described
        .get("asset_version")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| ApiError::invalid("describes_asset asset_version is invalid"))?;
    let raw_entry_id = described
        .get("description_of_entry_ref")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value.trim_start_matches("stage-entry:")).ok())
        .ok_or_else(|| ApiError::invalid("describes_asset description_of_entry_ref is invalid"))?;
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1
          FROM straylight.staged_entries AS description_entry
          JOIN straylight.staged_entries AS raw_entry
            ON raw_entry.user_id=description_entry.user_id
           AND raw_entry.stage_id=description_entry.stage_id
          JOIN straylight.assets AS asset
            ON asset.user_id=raw_entry.user_id AND asset.id=raw_entry.asset_id
          JOIN straylight.stages AS stage
            ON stage.user_id=raw_entry.user_id AND stage.id=raw_entry.stage_id
          WHERE description_entry.user_id=$1
            AND description_entry.scope_id=$2
            AND description_entry.id=$3
            AND raw_entry.id=$4
            AND raw_entry.asset_id=$5
            AND raw_entry.asset_version=$6
            AND asset.scope_id=$2
            AND stage.status IN ('ready','promoted')
            AND stage.expires_at > clock_timestamp()
        )
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(staged_asset.staged_entry_id)
    .bind(raw_entry_id)
    .bind(asset_id)
    .bind(asset_version)
    .fetch_one(&mut **tx)
    .await?;
    if !exists {
        return Err(ApiError::not_found(
            "described_asset_not_found",
            &format!("asset:{asset_id}"),
        ));
    }
    Ok(Some(DescribedAssetBinding {
        asset_id,
        asset_version,
    }))
}

async fn link_described_asset(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    source_id: Uuid,
    binding: &DescribedAssetBinding,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.source_asset_links (
          user_id,source_episode_id,asset_id,asset_version,role
        ) VALUES ($1,$2,$3,$4,'describes')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(source_id)
    .bind(binding.asset_id)
    .bind(binding.asset_version)
    .execute(&mut **tx)
    .await?;
    add_member(
        context,
        binding.asset_id,
        "asset",
        Some(binding.asset_version),
        "active",
    );
    Ok(())
}

async fn supersede_prior_source_versions(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    source_ref: &str,
    retained_version: &str,
) -> ApiResult<()> {
    let rows = sqlx::query(
        r#"
        WITH prior_sources AS (
          SELECT source.id
          FROM straylight.source_episodes AS source
          WHERE source.user_id=$1 AND source.scope_id=$2
            AND source.source_ref=$3
            AND source.source_version IS DISTINCT FROM $4
        ), prior_documents AS (
          SELECT revision.document_id
          FROM straylight.document_revisions AS revision
          WHERE revision.user_id=$1
            AND revision.source_episode_id IN (SELECT id FROM prior_sources)
        )
        SELECT id,'source_episode'::text AS record_kind
        FROM prior_sources
        UNION ALL
        SELECT evidence.id,'evidence'
        FROM straylight.evidence_items AS evidence
        WHERE evidence.user_id=$1
          AND evidence.source_episode_id IN (SELECT id FROM prior_sources)
        UNION ALL
        SELECT document_id,'document'
        FROM prior_documents
        UNION ALL
        SELECT chunk.id,'chunk'
        FROM straylight.chunks AS chunk
        WHERE chunk.user_id=$1
          AND chunk.document_id IN (SELECT document_id FROM prior_documents)
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(source_ref)
    .bind(retained_version)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let record_id: Uuid = row.try_get("id")?;
        if let Some(member) = context.members.get_mut(&record_id) {
            member.disposition = "superseded".to_owned();
        }
    }
    Ok(())
}

async fn reconcile_import_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    stable_import_id: &str,
    snapshot_paths: &[String],
) -> ApiResult<()> {
    let stale_source_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT source.id
        FROM straylight.source_episodes AS source
        WHERE source.user_id=$1 AND source.scope_id=$2
          AND source.metadata->>'stable_import_id'=$3
          AND NOT (
            coalesce(source.native_locator->>'path','') = ANY($4::text[])
          )
        ORDER BY source.id
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(stable_import_id)
    .bind(snapshot_paths)
    .fetch_all(&mut **tx)
    .await?;
    if stale_source_ids.is_empty() {
        return Ok(());
    }
    let stale_sources: HashSet<Uuid> = stale_source_ids.iter().copied().collect();
    let dependent_rows = sqlx::query(
        r#"
        WITH stale_sources AS (
          SELECT unnest($2::uuid[]) AS id
        ), stale_documents AS (
          SELECT DISTINCT revision.document_id
          FROM straylight.document_revisions AS revision
          WHERE revision.user_id=$1
            AND revision.source_episode_id IN (SELECT id FROM stale_sources)
        )
        SELECT id,'source_episode'::text AS record_kind
        FROM stale_sources
        UNION ALL
        SELECT evidence.id,'evidence'
        FROM straylight.evidence_items AS evidence
        WHERE evidence.user_id=$1
          AND evidence.source_episode_id IN (SELECT id FROM stale_sources)
        UNION ALL
        SELECT document_id,'document'
        FROM stale_documents
        UNION ALL
        SELECT chunk.id,'chunk'
        FROM straylight.chunks AS chunk
        WHERE chunk.user_id=$1
          AND chunk.document_id IN (SELECT document_id FROM stale_documents)
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(&stale_source_ids)
    .fetch_all(&mut **tx)
    .await?;
    for row in dependent_rows {
        let record_id: Uuid = row.try_get("id")?;
        if let Some(member) = context.members.get_mut(&record_id) {
            member.disposition = "superseded".to_owned();
        }
    }

    let affected_asset_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT DISTINCT link.asset_id
        FROM straylight.source_asset_links AS link
        WHERE link.user_id=$1 AND link.source_episode_id=ANY($2::uuid[])
        ORDER BY link.asset_id
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(&stale_source_ids)
    .fetch_all(&mut **tx)
    .await?;
    for asset_id in affected_asset_ids {
        let linked_source_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT DISTINCT link.source_episode_id
            FROM straylight.source_asset_links AS link
            WHERE link.user_id=$1 AND link.asset_id=$2
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(asset_id)
        .fetch_all(&mut **tx)
        .await?;
        let retained = linked_source_ids.iter().any(|source_id| {
            !stale_sources.contains(source_id)
                && context
                    .members
                    .get(source_id)
                    .is_some_and(|member| member.disposition == "active")
        });
        if !retained && let Some(member) = context.members.get_mut(&asset_id) {
            member.disposition = "superseded".to_owned();
        }
    }
    metrics::counter!("import.snapshot.superseded_sources")
        .increment(u64::try_from(stale_source_ids.len()).unwrap_or(u64::MAX));
    Ok(())
}

async fn process_source(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    proposed_id: Uuid,
) -> ApiResult<()> {
    let payload = payload_object(&item.payload)?;
    let staged_asset = staged_asset_binding(tx, context, payload).await?;
    let described_asset =
        described_asset_binding(tx, context, payload, staged_asset.as_ref()).await?;
    let supplied_content = payload
        .get("content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let content = if staged_asset.is_some() {
        supplied_content
    } else {
        Some(supplied_content.unwrap_or_default())
    };
    let content_text = content.as_deref().unwrap_or_default();
    let path = payload
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| payload.get("source_ref").and_then(Value::as_str))
        .or(item.reference.as_deref())
        .unwrap_or("source.txt")
        .to_owned();
    let source_ref = payload
        .get("source_ref")
        .and_then(Value::as_str)
        .or(item.reference.as_deref())
        .unwrap_or(&path)
        .to_owned();
    let calculated_hash = strip_sha256(&sha256_prefixed(content_text.as_bytes())).to_owned();
    let content_hash = if let Some(binding) = &staged_asset {
        if content.is_some() && calculated_hash != binding.content_hash {
            return Err(ApiError::conflict(
                "content_hash_mismatch",
                "promoted source text does not match its staged asset",
                json!({"path": path}),
            ));
        }
        binding.content_hash.clone()
    } else {
        calculated_hash
    };
    let source_version = payload
        .get("source_version")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| content_hash.clone());
    supersede_prior_source_versions(tx, context, &source_ref, &source_version).await?;
    if let Some(existing_id) = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM straylight.source_episodes
        WHERE user_id=$1 AND source_ref=$2 AND source_version=$3
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(&source_ref)
    .bind(&source_version)
    .fetch_optional(&mut **tx)
    .await?
    {
        if let Some(binding) = &staged_asset {
            link_staged_asset(tx, context, existing_id, binding).await?;
        }
        if let Some(binding) = &described_asset {
            link_described_asset(tx, context, existing_id, binding).await?;
        }
        context
            .local_refs
            .insert(source_ref.clone(), (existing_id, RecordKind::SourceEpisode));
        context.members.insert(
            existing_id,
            Member {
                record_id: existing_id,
                record_kind: "source_episode".to_owned(),
                record_version: None,
                disposition: "active".to_owned(),
            },
        );
        context.receipts.push(ItemReceipt {
            order,
            action: action_name(item.action).to_owned(),
            kind: kind_name(item.kind).to_owned(),
            reference: format!("source:{existing_id}"),
            record_id: Some(existing_id),
            prior_version: None,
            resulting_version: None,
            disposition: "deduplicated".to_owned(),
            details: json!({"source_ref": source_ref, "content_hash": format!("sha256:{content_hash}")}),
        });
        return Ok(());
    }

    let media_type = payload
        .get("media_type")
        .and_then(Value::as_str)
        .or_else(|| {
            staged_asset
                .as_ref()
                .map(|binding| binding.media_type.as_str())
        })
        .unwrap_or("text/markdown");
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path));
    let source_kind = payload
        .get("source_kind")
        .and_then(Value::as_str)
        .unwrap_or("document");
    sqlx::query(
        r#"
        INSERT INTO straylight.source_episodes (
          id,user_id,scope_id,source_ref,source_kind,source_version,title,
          captured_at,content_hash,native_locator,metadata,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,clock_timestamp(),$8,$9,$10,$11,$12)
        "#,
    )
    .bind(proposed_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(&source_ref)
    .bind(source_kind)
    .bind(&source_version)
    .bind(title)
    .bind(&content_hash)
    .bind(json!({
        "path": path,
        "media_type": media_type,
        "asset_ref": staged_asset.as_ref().map(|binding| format!("asset:{}", binding.asset_id)),
        "stage_entry_ref": staged_asset.as_ref().map(|binding| format!("stage-entry:{}", binding.staged_entry_id))
    }))
    .bind(
        payload
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;

    let evidence_id = Uuid::now_v7();
    let evidence_kind = if source_kind == "derived_asset_description" {
        "derived_non_authoritative"
    } else {
        "source_native"
    };
    sqlx::query(
        r#"
        INSERT INTO straylight.evidence_items (
          id,user_id,scope_id,source_episode_id,evidence_kind,locator,
          text_content,content_hash,asset_id,asset_version,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
        "#,
    )
    .bind(evidence_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(proposed_id)
    .bind(evidence_kind)
    .bind(json!({"path": path, "start_line": 1, "end_line": content_text.lines().count().max(1)}))
    .bind(content.as_deref())
    .bind(&content_hash)
    .bind(staged_asset.as_ref().map(|binding| binding.asset_id))
    .bind(staged_asset.as_ref().map(|binding| binding.asset_version))
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;

    let mut created_refs = vec![
        format!("source:{proposed_id}"),
        format!("evidence:{evidence_id}"),
    ];
    add_member(context, proposed_id, "source_episode", None, "active");
    add_member(context, evidence_id, "evidence", None, "active");
    if let Some(binding) = &staged_asset {
        link_staged_asset(tx, context, proposed_id, binding).await?;
        created_refs.push(format!("asset:{}", binding.asset_id));
    }
    if let Some(binding) = &described_asset {
        link_described_asset(tx, context, proposed_id, binding).await?;
        created_refs.push(format!("asset:{}", binding.asset_id));
    }
    context
        .local_refs
        .insert(source_ref.clone(), (proposed_id, RecordKind::SourceEpisode));
    context
        .local_refs
        .insert(path.clone(), (proposed_id, RecordKind::SourceEpisode));
    context.local_refs.insert(
        format!("evidence:{evidence_id}"),
        (evidence_id, RecordKind::Evidence),
    );

    let text_like = media_type.starts_with("text/")
        || media_type == "application/json"
        || media_type == "application/sql";
    if text_like && !content_text.is_empty() {
        let document_id = Uuid::now_v7();
        let normalized = normalize_document(&path, content_text);
        sqlx::query(
            r#"
            INSERT INTO straylight.documents (
              id,user_id,scope_id,current_version,policy_id,policy_version
            ) VALUES ($1,$2,$3,1,$4,$5)
            "#,
        )
        .bind(document_id)
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(context.policy_id)
        .bind(context.policy_version)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.document_revisions (
              user_id,document_id,version,previous_version,source_episode_id,
              asset_id,asset_version,title,media_type,content_hash,native_locator,byte_size
            ) VALUES ($1,$2,1,NULL,$3,$4,$5,$6,$7,$8,$9,$10)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(document_id)
        .bind(proposed_id)
        .bind(staged_asset.as_ref().map(|binding| binding.asset_id))
        .bind(staged_asset.as_ref().map(|binding| binding.asset_version))
        .bind(&normalized.title)
        .bind(media_type)
        .bind(strip_sha256(&normalized.content_hash))
        .bind(json!({"path": path, "source_ref": source_ref, "headings": normalized.headings}))
        .bind(content_text.len() as i64)
        .execute(&mut **tx)
        .await?;
        add_member(context, document_id, "document", Some(1), "active");
        context
            .local_refs
            .insert(path.clone(), (document_id, RecordKind::Document));
        created_refs.push(format!("document:{document_id}"));
        let mut chunk_ids = Vec::new();
        for chunk in normalized.chunks {
            let chunk_id = document_chunk_id(document_id, &chunk.chunk_ref);
            let chunk_ref = format!("chunk:{chunk_id}");
            sqlx::query(
                r#"
                INSERT INTO straylight.chunks (
                  id,user_id,scope_id,document_id,document_version,ordinal,
                  chunk_kind,heading_path,locator,content,content_hash,token_count
                ) VALUES ($1,$2,$3,$4,1,$5,'section',$6,$7,$8,$9,$10)
                "#,
            )
            .bind(chunk_id)
            .bind(context.auth.user_id.0)
            .bind(context.scope_id)
            .bind(document_id)
            .bind(chunk.ordinal as i32)
            .bind(vec![chunk.heading.clone()])
            .bind(json!({
                "path": path,
                "start_line": chunk.start_line,
                "end_line": chunk.end_line,
                "ordinal": chunk.ordinal,
                "normalized_chunk_ref": chunk.chunk_ref
            }))
            .bind(&chunk.content)
            .bind(strip_sha256(&chunk.content_hash))
            .bind(chunk.estimated_tokens as i32)
            .execute(&mut **tx)
            .await?;
            add_member(context, chunk_id, "chunk", None, "active");
            context
                .local_refs
                .insert(chunk_ref, (chunk_id, RecordKind::Chunk));
            chunk_ids.push(chunk_id);
        }
        if !chunk_ids.is_empty() {
            sqlx::query(
                r#"
                INSERT INTO straylight.background_jobs (
                  id,user_id,scope_id,job_kind,status,payload
                ) VALUES ($1,$2,$3,'index_embeddings','queued',$4)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(context.auth.user_id.0)
            .bind(context.scope_id)
            .bind(json!({
                "chunk_ids": chunk_ids,
                "model": context.request_hash,
                "document_id": document_id
            }))
            .execute(&mut **tx)
            .await?;
        }
    }
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference: format!("source:{proposed_id}"),
        record_id: Some(proposed_id),
        prior_version: None,
        resulting_version: None,
        disposition: "created".to_owned(),
        details: json!({
            "source_ref": source_ref,
            "content_hash": format!("sha256:{content_hash}"),
            "created_refs": created_refs
        }),
    });
    Ok(())
}

fn document_chunk_id(document_id: Uuid, normalized_chunk_ref: &str) -> Uuid {
    let mut identity = Sha256::new();
    identity.update(b"carrystate:document-chunk:v1\0");
    identity.update(document_id.as_bytes());
    identity.update([0]);
    identity.update(normalized_chunk_ref.as_bytes());
    let digest = identity.finalize();
    let mut uuid_bytes = [0u8; 16];
    uuid_bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(uuid_bytes)
}

async fn process_evidence(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    id: Uuid,
) -> ApiResult<()> {
    let text = source_text(&item.payload)
        .or_else(|| payload_string(&item.payload, "text_content"))
        .ok_or_else(|| ApiError::invalid("evidence requires source_text or text_content"))?;
    let source_id = if let Some(reference) = payload_string(&item.payload, "source_ref") {
        resolve_record(tx, context, &reference, Some(RecordKind::SourceEpisode))
            .await?
            .0
    } else {
        context.operation_source_id
    };
    let content_hash = strip_sha256(&sha256_prefixed(text.as_bytes())).to_owned();
    sqlx::query(
        r#"
        INSERT INTO straylight.evidence_items (
          id,user_id,scope_id,source_episode_id,evidence_kind,locator,
          text_content,content_hash,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
        "#,
    )
    .bind(id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(source_id)
    .bind(
        payload_string(&item.payload, "evidence_kind").unwrap_or_else(|| "source_span".to_owned()),
    )
    .bind(
        item.payload
            .get("locator")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .bind(&text)
    .bind(&content_hash)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    add_member(context, id, "evidence", None, "active");
    let reference = format!("evidence:{id}");
    context
        .local_refs
        .insert(reference.clone(), (id, RecordKind::Evidence));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(id),
        prior_version: None,
        resulting_version: None,
        disposition: "created".to_owned(),
        details: json!({"content_hash": format!("sha256:{content_hash}")}),
    });
    Ok(())
}

async fn process_object(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    proposed_id: Uuid,
) -> ApiResult<()> {
    if item.action == SaveAction::Tombstone {
        return process_tombstone(tx, context, order, item, proposed_id).await;
    }
    let profiles = payload_strings(&item.payload, "type_profiles");
    let label =
        payload_string(&item.payload, "label").or_else(|| payload_string(&item.payload, "title"));
    let (object_id, prior_version, version) = if item.action == SaveAction::Create {
        sqlx::query(
            r#"
            INSERT INTO straylight.objects (
              id,user_id,scope_id,current_version,policy_id,policy_version
            ) VALUES ($1,$2,$3,1,$4,$5)
            "#,
        )
        .bind(proposed_id)
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(context.policy_id)
        .bind(context.policy_version)
        .execute(&mut **tx)
        .await?;
        (proposed_id, None, 1)
    } else {
        let reference = item
            .reference
            .as_deref()
            .ok_or_else(|| ApiError::invalid("object revision requires ref"))?;
        let (object_id, _) =
            resolve_record(tx, context, reference, Some(RecordKind::Object)).await?;
        let current = sqlx::query_scalar::<_, i32>(
            "SELECT current_version FROM straylight.objects WHERE user_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(context.auth.user_id.0)
        .bind(object_id)
        .fetch_one(&mut **tx)
        .await?;
        if item
            .expected_version
            .is_some_and(|expected| expected != current)
        {
            return Err(ApiError::conflict(
                "object_version_conflict",
                "object current version does not match expected_version",
                json!({"ref": reference, "expected": item.expected_version, "actual": current}),
            ));
        }
        (object_id, Some(current), current + 1)
    };
    for profile in &profiles {
        sqlx::query("SELECT straylight.ensure_profile_schema($1)")
            .bind(profile)
            .execute(&mut **tx)
            .await?;
    }
    let source_episode_id = if let Some(source_ref) =
        payload_string(&item.payload, "canonical_source_ref")
            .or_else(|| payload_string(&item.payload, "source_ref"))
    {
        resolve_record(tx, context, &source_ref, Some(RecordKind::SourceEpisode))
            .await?
            .0
    } else {
        context.operation_source_id
    };
    sqlx::query(
        r#"
        INSERT INTO straylight.object_revisions (
          user_id,object_id,version,previous_version,label,properties,
          source_episode_id,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(object_id)
    .bind(version)
    .bind(prior_version)
    .bind(&label)
    .bind(&item.payload)
    .bind(source_episode_id)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    for profile in &profiles {
        sqlx::query(
            r#"
            INSERT INTO straylight.object_revision_profiles (
              user_id,object_id,object_version,profile_ref,profile_version
            ) VALUES ($1,$2,$3,$4,1)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(object_id)
        .bind(version)
        .bind(profile)
        .execute(&mut **tx)
        .await?;
    }
    if prior_version.is_some() {
        sqlx::query("UPDATE straylight.objects SET current_version=$1 WHERE user_id=$2 AND id=$3")
            .bind(version)
            .bind(context.auth.user_id.0)
            .bind(object_id)
            .execute(&mut **tx)
            .await?;
    }
    if let Some(handle) = item.reference.as_deref()
        && RecordRef::parse(handle).is_err()
    {
        let evidence_id = create_inline_evidence(
            tx,
            context,
            source_text(&item.payload).unwrap_or_else(|| {
                label
                    .clone()
                    .unwrap_or_else(|| format!("Object handle {handle}"))
            }),
            json!({"field": "object_handle", "handle": handle}),
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.object_handles (
              id,user_id,scope_id,handle_kind,handle,object_id,evidence_id,
              policy_id,policy_version
            ) VALUES ($1,$2,$3,'adapter_ref',$4,$5,$6,$7,$8)
            ON CONFLICT (user_id,scope_id,handle_kind,handle) WHERE valid_to IS NULL
            DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(handle)
        .bind(object_id)
        .bind(evidence_id)
        .bind(context.policy_id)
        .bind(context.policy_version)
        .execute(&mut **tx)
        .await?;
        context
            .local_refs
            .insert(handle.to_owned(), (object_id, RecordKind::Object));
    }
    let identity_claim_refs =
        process_identity_fields(tx, context, object_id, &profiles, &item.payload).await?;
    process_event_fields(
        tx,
        context,
        object_id,
        version,
        prior_version,
        &profiles,
        &item.payload,
    )
    .await?;
    add_member(context, object_id, "object", Some(version), "active");
    let reference = format!("object:{object_id}");
    context
        .local_refs
        .insert(reference.clone(), (object_id, RecordKind::Object));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(object_id),
        prior_version,
        resulting_version: Some(version),
        disposition: if prior_version.is_some() {
            "revised"
        } else {
            "created"
        }
        .to_owned(),
        details: json!({
            "type_profiles": profiles,
            "label": label,
            "identity_claim_refs": identity_claim_refs
        }),
    });
    Ok(())
}

async fn process_temporal(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    id: Uuid,
) -> ApiResult<()> {
    let temporal: TemporalSpecV1 = serde_json::from_value(item.payload.clone())?;
    temporal.validate()?;
    let kind: String;
    let mut date_value: Option<NaiveDate> = None;
    let mut start_date: Option<NaiveDate> = None;
    let mut end_date: Option<NaiveDate> = None;
    let mut local_value: Option<NaiveDateTime> = None;
    let mut instant_value: Option<DateTime<Utc>> = None;
    let mut start_instant: Option<DateTime<Utc>> = None;
    let mut end_instant: Option<DateTime<Utc>> = None;
    let mut start_local: Option<NaiveDateTime> = None;
    let mut end_local: Option<NaiveDateTime> = None;
    let mut time_zone: Option<String> = None;
    match temporal {
        TemporalSpecV1::Date { date } => {
            kind = "date".to_owned();
            date_value = Some(date);
        }
        TemporalSpecV1::DateInterval {
            start,
            end_exclusive,
        } => {
            kind = "date_interval".to_owned();
            start_date = Some(start);
            end_date = end_exclusive;
        }
        TemporalSpecV1::LocalDatetime {
            local,
            time_zone: zone,
        } => {
            kind = "local_datetime".to_owned();
            local_value = Some(local);
            time_zone = Some(zone);
        }
        TemporalSpecV1::FloatingDatetime { local } => {
            kind = "floating_datetime".to_owned();
            local_value = Some(local);
        }
        TemporalSpecV1::Instant { at } => {
            kind = "instant".to_owned();
            instant_value = Some(at);
        }
        TemporalSpecV1::InstantInterval { start, end } => {
            kind = "instant_interval".to_owned();
            start_instant = Some(start);
            end_instant = end;
        }
        TemporalSpecV1::LocalInterval {
            start_local: start,
            end_local: end,
            time_zone: zone,
        } => {
            kind = "local_interval".to_owned();
            start_local = Some(start);
            end_local = Some(end);
            time_zone = Some(zone);
        }
        TemporalSpecV1::Due {
            local,
            time_zone: zone,
        } => {
            kind = "due".to_owned();
            local_value = Some(local);
            time_zone = Some(zone);
        }
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.temporal_specs (
          id,user_id,scope_id,kind,date_value,start_date,end_date_exclusive,
          local_value,instant_value,start_instant,end_instant,start_local,end_local,time_zone
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        "#,
    )
    .bind(id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(&kind)
    .bind(date_value)
    .bind(start_date)
    .bind(end_date)
    .bind(local_value)
    .bind(instant_value)
    .bind(start_instant)
    .bind(end_instant)
    .bind(start_local)
    .bind(end_local)
    .bind(time_zone)
    .execute(&mut **tx)
    .await?;
    add_member(context, id, "temporal_spec", None, "active");
    let reference = format!("temporal:{id}");
    context
        .local_refs
        .insert(reference.clone(), (id, RecordKind::TemporalSpec));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(id),
        prior_version: None,
        resulting_version: None,
        disposition: "created".to_owned(),
        details: json!({"temporal_kind": kind}),
    });
    Ok(())
}

async fn process_embedded_temporal(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    temporal: TemporalSpecV1,
) -> ApiResult<(Uuid, String)> {
    let id = Uuid::now_v7();
    let receipts_before = context.receipts.len();
    process_temporal(
        tx,
        context,
        usize::MAX,
        SaveItem {
            action: SaveAction::Create,
            kind: SaveKind::TemporalSpec,
            reference: None,
            expected_version: None,
            payload: serde_json::to_value(temporal)?,
        },
        id,
    )
    .await?;
    if context.receipts.len() != receipts_before + 1 {
        return Err(ApiError::Internal(
            "embedded temporal write produced an unexpected receipt count".to_owned(),
        ));
    }
    context.receipts.pop();
    Ok((id, format!("temporal:{id}")))
}

fn strip_sha256(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn add_member(
    context: &mut WriteContext<'_>,
    record_id: Uuid,
    record_kind: &str,
    record_version: Option<i32>,
    disposition: &str,
) {
    context.members.insert(
        record_id,
        Member {
            record_id,
            record_kind: record_kind.to_owned(),
            record_version,
            disposition: disposition.to_owned(),
        },
    );
}

async fn create_inline_evidence(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    text: String,
    locator: Value,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    let content_hash = strip_sha256(&sha256_prefixed(text.as_bytes())).to_owned();
    sqlx::query(
        r#"
        INSERT INTO straylight.evidence_items (
          id,user_id,scope_id,source_episode_id,evidence_kind,locator,
          text_content,content_hash,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,'authenticated_input',$5,$6,$7,$8,$9)
        "#,
    )
    .bind(id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(context.operation_source_id)
    .bind(locator)
    .bind(text)
    .bind(content_hash)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    add_member(context, id, "evidence", None, "active");
    context
        .local_refs
        .insert(format!("evidence:{id}"), (id, RecordKind::Evidence));
    Ok(id)
}

async fn resolve_record(
    tx: &mut Transaction<'_, Postgres>,
    context: &WriteContext<'_>,
    reference: &str,
    expected: Option<RecordKind>,
) -> ApiResult<(Uuid, RecordKind)> {
    if let Some((id, kind)) = context.local_refs.get(reference).copied()
        && expected.is_none_or(|expected| expected == kind)
    {
        return Ok((id, kind));
    }
    if let Ok(parsed) = RecordRef::parse(reference) {
        if expected.is_some_and(|expected| expected != parsed.kind) {
            return Err(ApiError::invalid(format!(
                "{reference} has the wrong record kind"
            )));
        }
        let active_in_base_revision = context.members.get(&parsed.id).is_some_and(|member| {
            member.record_kind == parsed.kind.database_kind() && member.disposition == "active"
        });
        if active_in_base_revision {
            return Ok((parsed.id, parsed.kind));
        }
    }
    if expected.is_none_or(|kind| kind == RecordKind::Object)
        && let Some(id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT object_id FROM straylight.object_handles
            WHERE user_id=$1 AND scope_id=$2 AND handle=$3 AND valid_to IS NULL
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(reference)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok((id, RecordKind::Object));
    }
    if expected.is_none_or(|kind| kind == RecordKind::SourceEpisode)
        && let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM straylight.source_episodes WHERE user_id=$1 AND scope_id=$2 AND source_ref=$3 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(reference)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok((id, RecordKind::SourceEpisode));
    }
    if expected.is_none_or(|kind| kind == RecordKind::Document)
        && let Some(id) = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT document.id
            FROM straylight.documents AS document
            JOIN straylight.document_revisions AS revision
              ON revision.user_id=document.user_id AND revision.document_id=document.id
             AND revision.version=document.current_version
            WHERE document.user_id=$1 AND document.scope_id=$2
              AND revision.native_locator->>'path'=$3
            LIMIT 1
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(reference)
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok((id, RecordKind::Document));
    }
    Err(ApiError::not_found("ref_not_found", reference))
}

#[derive(Debug, PartialEq)]
struct ClaimAuthorityDimensions {
    producer_ref: String,
    formation_method: String,
    support_state: String,
    authority: String,
    canonicality: String,
}

fn claim_authority_dimensions(payload: &Value) -> ApiResult<ClaimAuthorityDimensions> {
    fn required(payload: &Value, field: &str) -> ApiResult<String> {
        payload_string(payload, field)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ApiError::invalid(format!("claim requires {field}")))
    }

    Ok(ClaimAuthorityDimensions {
        producer_ref: required(payload, "producer_ref")?,
        formation_method: required(payload, "formation_method")?,
        support_state: required(payload, "support_state")?,
        authority: required(payload, "authority")?,
        canonicality: required(payload, "canonicality")?,
    })
}

async fn process_claim(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    id: Uuid,
) -> ApiResult<()> {
    if item.action == SaveAction::Tombstone {
        return process_tombstone(tx, context, order, item, id).await;
    }
    let predicate = payload_string(&item.payload, "predicate")
        .ok_or_else(|| ApiError::invalid("claim requires predicate"))?;
    let value = item.payload.get("value").cloned().unwrap_or(Value::Null);
    let about_refs = payload_strings(&item.payload, "about_refs");
    let mut about_ids = Vec::new();
    for reference in &about_refs {
        about_ids.push(resolve_record(tx, context, reference, None).await?.0);
    }
    if about_ids.is_empty() {
        return Err(ApiError::invalid(
            "claim requires at least one resolvable about ref",
        ));
    }
    let authority = claim_authority_dimensions(&item.payload)?;
    let producer_id = resolve_record(tx, context, &authority.producer_ref, None)
        .await?
        .0;
    let valid_temporal_id = optional_record_id(
        tx,
        context,
        payload_string(&item.payload, "valid_temporal_ref"),
        RecordKind::TemporalSpec,
    )
    .await?;
    let observed_temporal_id = optional_record_id(
        tx,
        context,
        payload_string(&item.payload, "observed_temporal_ref"),
        RecordKind::TemporalSpec,
    )
    .await?;
    let claim_mode = match item.kind {
        SaveKind::StateAssignment => "state_assignment",
        SaveKind::StateTransition => "state_transition",
        _ => item
            .payload
            .get("claim_mode")
            .and_then(Value::as_str)
            .unwrap_or("descriptive"),
    };
    let evidence_ids = evidence_for_payload(tx, context, &item.payload).await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.claims (
          id,user_id,scope_id,predicate,value,producer_ref,formation_method,
          claim_mode,support_state,authority,canonicality,confidence,
          valid_temporal_id,observed_temporal_id,source_episode_id,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
        "#,
    )
    .bind(id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(&predicate)
    .bind(&value)
    .bind(producer_id)
    .bind(&authority.formation_method)
    .bind(claim_mode)
    .bind(&authority.support_state)
    .bind(&authority.authority)
    .bind(&authority.canonicality)
    .bind(item.payload.get("confidence").and_then(Value::as_f64))
    .bind(valid_temporal_id)
    .bind(observed_temporal_id)
    .bind(context.operation_source_id)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    for target in about_ids {
        sqlx::query(
            "INSERT INTO straylight.claim_about_targets (user_id,claim_id,target_record_id,role) VALUES ($1,$2,$3,'about')",
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .bind(target)
        .execute(&mut **tx)
        .await?;
    }
    for evidence_id in &evidence_ids {
        sqlx::query(
            "INSERT INTO straylight.claim_evidence_links (user_id,claim_id,evidence_id,support_kind) VALUES ($1,$2,$3,'supporting')",
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .bind(evidence_id)
        .execute(&mut **tx)
        .await?;
    }

    let mut lineage = item
        .payload
        .get("lineage")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if item.action != SaveAction::Create
        && let Some(reference) = &item.reference
    {
        let lineage_kind = match item.action {
            SaveAction::Supersede => "supersedes",
            SaveAction::Retract => "retracts",
            SaveAction::Revise => "corrects",
            _ => "derives_from",
        };
        lineage.push(json!({"kind": lineage_kind, "claim_ref": reference}));
    }
    for entry in lineage {
        let predecessor_ref = entry
            .get("claim_ref")
            .or_else(|| entry.get("ref"))
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("claim lineage requires claim_ref"))?;
        let predecessor = resolve_record(tx, context, predecessor_ref, Some(RecordKind::Claim))
            .await?
            .0;
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("derives_from");
        sqlx::query(
            r#"
            INSERT INTO straylight.claim_lineage (
              user_id,claim_id,predecessor_claim_id,lineage_kind,evidence_id
            ) VALUES ($1,$2,$3,$4,$5)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .bind(predecessor)
        .bind(kind)
        .bind(evidence_ids.first().copied())
        .execute(&mut **tx)
        .await?;
    }

    if matches!(
        item.kind,
        SaveKind::StateAssignment | SaveKind::StateTransition
    ) {
        let state_value = value
            .as_str()
            .ok_or_else(|| ApiError::invalid("state assignment value must be a string"))?;
        let target = payload_strings(&item.payload, "about_refs")
            .first()
            .ok_or_else(|| ApiError::invalid("state assignment requires one target"))?
            .to_owned();
        let target_id = resolve_record(tx, context, &target, None).await?.0;
        let previous = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT current_claim_id FROM straylight.state_heads
            WHERE user_id=$1 AND target_record_id=$2 AND state_machine_ref=$3
            FOR UPDATE
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(target_id)
        .bind(&predicate)
        .fetch_optional(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.state_assignments (
              user_id,scope_id,claim_id,target_record_id,state_machine_ref,
              state_value,previous_assignment_claim_id
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(id)
        .bind(target_id)
        .bind(&predicate)
        .bind(state_value)
        .bind(previous)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.state_heads (
              user_id,scope_id,target_record_id,state_machine_ref,current_claim_id
            ) VALUES ($1,$2,$3,$4,$5)
            ON CONFLICT (user_id,target_record_id,state_machine_ref)
            DO UPDATE SET current_claim_id=EXCLUDED.current_claim_id,updated_at=clock_timestamp()
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(target_id)
        .bind(&predicate)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    }
    add_member(context, id, "claim", None, "active");
    let reference = format!("claim:{id}");
    context
        .local_refs
        .insert(reference.clone(), (id, RecordKind::Claim));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(id),
        prior_version: None,
        resulting_version: None,
        disposition: match item.action {
            SaveAction::Supersede => "superseded",
            SaveAction::Retract => "retracted",
            _ => "created",
        }
        .to_owned(),
        details: json!({"predicate": predicate, "evidence_refs": evidence_ids}),
    });
    Ok(())
}

async fn process_relation(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    proposed_id: Uuid,
) -> ApiResult<()> {
    if item.action == SaveAction::Tombstone {
        return process_tombstone(tx, context, order, item, proposed_id).await;
    }
    let predicate = payload_string(&item.payload, "predicate")
        .ok_or_else(|| ApiError::invalid("relation requires predicate"))?;
    let (relation_id, prior_version, version) = if item.action == SaveAction::Create {
        sqlx::query(
            r#"
            INSERT INTO straylight.relations (
              id,user_id,scope_id,current_version,policy_id,policy_version
            ) VALUES ($1,$2,$3,1,$4,$5)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(proposed_id)
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(context.policy_id)
        .bind(context.policy_version)
        .execute(&mut **tx)
        .await?;
        (proposed_id, None, 1)
    } else {
        let reference = item
            .reference
            .as_deref()
            .ok_or_else(|| ApiError::invalid("relation revision requires ref"))?;
        let (id, _) = resolve_record(tx, context, reference, Some(RecordKind::Relation)).await?;
        let current = sqlx::query_scalar::<_, i32>(
            "SELECT current_version FROM straylight.relations WHERE user_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        if item
            .expected_version
            .is_some_and(|expected| expected != current)
        {
            return Err(ApiError::conflict(
                "object_version_conflict",
                "relation current version does not match expected_version",
                json!({"ref": reference, "expected": item.expected_version, "actual": current}),
            ));
        }
        (id, Some(current), current + 1)
    };
    sqlx::query("SELECT straylight.ensure_relation_schema($1)")
        .bind(&predicate)
        .execute(&mut **tx)
        .await?;
    let evidence_ids = evidence_for_payload(tx, context, &item.payload).await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.relation_revisions (
          user_id,relation_id,version,previous_version,predicate,schema_version,
          qualifiers,valid_temporal_id,source_episode_id,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,1,$6,$7,$8,$9,$10)
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(relation_id)
    .bind(version)
    .bind(prior_version)
    .bind(&predicate)
    .bind(
        item.payload
            .get("qualifiers")
            .cloned()
            .unwrap_or_else(|| json!({})),
    )
    .bind(
        optional_record_id(
            tx,
            context,
            payload_string(&item.payload, "valid_temporal_ref"),
            RecordKind::TemporalSpec,
        )
        .await?,
    )
    .bind(context.operation_source_id)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    let endpoints = item
        .payload
        .get("endpoints")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::invalid("relation requires endpoints"))?;
    for (position, endpoint) in endpoints.iter().enumerate() {
        let role = endpoint
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("related");
        let reference = endpoint
            .get("ref")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("relation endpoint requires ref"))?;
        let target = resolve_record(tx, context, reference, None).await?.0;
        sqlx::query(
            r#"
            INSERT INTO straylight.relation_endpoints (
              user_id,relation_id,relation_version,role,position,target_record_id
            ) VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(relation_id)
        .bind(version)
        .bind(role)
        .bind(position as i32)
        .bind(target)
        .execute(&mut **tx)
        .await?;
    }
    for evidence_id in &evidence_ids {
        sqlx::query(
            r#"
            INSERT INTO straylight.relation_evidence_links (
              user_id,relation_id,relation_version,evidence_id,support_kind
            ) VALUES ($1,$2,$3,$4,'supporting')
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(relation_id)
        .bind(version)
        .bind(evidence_id)
        .execute(&mut **tx)
        .await?;
    }
    if prior_version.is_some() {
        sqlx::query(
            "UPDATE straylight.relations SET current_version=$1 WHERE user_id=$2 AND id=$3",
        )
        .bind(version)
        .bind(context.auth.user_id.0)
        .bind(relation_id)
        .execute(&mut **tx)
        .await?;
    }
    add_member(context, relation_id, "relation", Some(version), "active");
    let reference = format!("relation:{relation_id}");
    context
        .local_refs
        .insert(reference.clone(), (relation_id, RecordKind::Relation));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(relation_id),
        prior_version,
        resulting_version: Some(version),
        disposition: if prior_version.is_some() {
            "revised"
        } else {
            "created"
        }
        .to_owned(),
        details: json!({"predicate": predicate, "evidence_refs": evidence_ids}),
    });
    Ok(())
}

async fn process_recurrence(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    id: Uuid,
) -> ApiResult<()> {
    if item.action != SaveAction::Create {
        return Err(ApiError::invalid(
            "recurrence specifications are immutable; create a linked successor series",
        ));
    }
    let recurrence: RecurrenceSpecV1 = serde_json::from_value(item.payload.clone())?;
    recurrence.validate()?;
    let (start_id, start_ref) = process_embedded_temporal(tx, context, recurrence.start).await?;
    let split_from = optional_record_id(
        tx,
        context,
        recurrence.split_from_series_ref,
        RecordKind::Object,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.recurrence_specs (
          id,user_id,scope_id,start_temporal_id,duration,split_from_series_object_id
        ) VALUES ($1,$2,$3,$4,$5::interval,$6)
        "#,
    )
    .bind(id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(start_id)
    .bind(recurrence.duration)
    .bind(split_from)
    .execute(&mut **tx)
    .await?;
    for (rule_order, rule) in recurrence.rules.iter().enumerate() {
        sqlx::query(
            "INSERT INTO straylight.recurrence_rules (user_id,recurrence_spec_id,rule_order,rrule) VALUES ($1,$2,$3,$4)",
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .bind(rule_order as i32)
        .bind(rule)
        .execute(&mut **tx)
        .await?;
    }
    let recurrence_evidence = create_inline_evidence(
        tx,
        context,
        source_text(&item.payload).unwrap_or(canonical_json(&item.payload)?),
        json!({"kind": "recurrence_spec", "recurrence_ref": format!("recurrence:{id}")}),
    )
    .await?;
    let mut exclusion_refs = Vec::new();
    for exclusion in recurrence.excluded_recurrence_ids {
        let (exclusion_id, exclusion_ref) =
            process_embedded_temporal(tx, context, exclusion).await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.recurrence_exclusions (
              user_id,recurrence_spec_id,recurrence_id_temporal_id,evidence_id
            ) VALUES ($1,$2,$3,$4)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .bind(exclusion_id)
        .bind(recurrence_evidence)
        .execute(&mut **tx)
        .await?;
        exclusion_refs.push(exclusion_ref);
    }
    let mut addition_refs = Vec::new();
    for occurrence_ref in recurrence.added_occurrence_refs {
        let occurrence_id = resolve_record(tx, context, &occurrence_ref, Some(RecordKind::Object))
            .await?
            .0;
        sqlx::query(
            r#"
            INSERT INTO straylight.recurrence_additions (
              user_id,recurrence_spec_id,occurrence_object_id,evidence_id
            ) VALUES ($1,$2,$3,$4)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(id)
        .bind(occurrence_id)
        .bind(recurrence_evidence)
        .execute(&mut **tx)
        .await?;
        addition_refs.push(occurrence_ref);
    }
    add_member(context, id, "recurrence_spec", None, "active");
    let reference = format!("recurrence:{id}");
    context
        .local_refs
        .insert(reference.clone(), (id, RecordKind::RecurrenceSpec));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(id),
        prior_version: None,
        resulting_version: None,
        disposition: "created".to_owned(),
        details: json!({
            "start_temporal_ref": start_ref,
            "excluded_recurrence_refs": exclusion_refs,
            "added_occurrence_refs": addition_refs,
            "evidence_ref": format!("evidence:{recurrence_evidence}")
        }),
    });
    Ok(())
}

async fn process_policy(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    id: Uuid,
) -> ApiResult<()> {
    let policy_ref = payload_string(&item.payload, "policy_ref")
        .or(item.reference.clone())
        .unwrap_or_else(|| format!("policy:{id}"));
    let rules = item
        .payload
        .get("rules")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let default_effect =
        payload_string(&item.payload, "default_effect").unwrap_or_else(|| "deny".to_owned());
    sqlx::query(
        r#"
        INSERT INTO straylight.policies (
          id,user_id,policy_ref,name,current_version,is_default
        ) VALUES ($1,$2,$3,$4,1,false)
        "#,
    )
    .bind(id)
    .bind(context.auth.user_id.0)
    .bind(&policy_ref)
    .bind(payload_string(&item.payload, "name").unwrap_or_else(|| policy_ref.clone()))
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO straylight.policy_revisions (user_id,policy_id,version,default_effect,rules) VALUES ($1,$2,1,$3,$4)",
    )
    .bind(context.auth.user_id.0)
    .bind(id)
    .bind(&default_effect)
    .bind(&rules)
    .execute(&mut **tx)
    .await?;
    if let Some(rule_rows) = rules.as_array() {
        for (index, rule) in rule_rows.iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO straylight.policy_rules (
                  user_id,policy_id,policy_version,rule_order,effect,principals,
                  purposes,actions,paths,conditions
                ) VALUES ($1,$2,1,$3,$4,$5,$6,$7,$8,$9)
                "#,
            )
            .bind(context.auth.user_id.0)
            .bind(id)
            .bind(index as i32)
            .bind(rule.get("effect").and_then(Value::as_str).unwrap_or("deny"))
            .bind(json_array_strings(rule.get("principals")))
            .bind(json_array_strings(rule.get("purposes")))
            .bind(json_array_strings(rule.get("actions")))
            .bind(json_array_strings(rule.get("paths")))
            .bind(rule.get("conditions").cloned().unwrap_or_else(|| json!({})))
            .execute(&mut **tx)
            .await?;
        }
    }
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference: policy_ref,
        record_id: None,
        prior_version: None,
        resulting_version: Some(1),
        disposition: "created".to_owned(),
        details: json!({"default_effect": default_effect}),
    });
    Ok(())
}

fn require_identity_review_approval_capability(
    auth: &AuthContext,
    decision: &str,
) -> ApiResult<()> {
    if decision != "approved"
        || auth.can(Capability::CredentialManage)
        || auth.can(Capability::Admin)
    {
        Ok(())
    } else {
        Err(ApiError::capability("credential:manage or admin"))
    }
}

async fn process_identity_review(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    receipt_id: Uuid,
) -> ApiResult<()> {
    let relation_ref = payload_string(&item.payload, "relation_ref")
        .ok_or_else(|| ApiError::invalid("identity review requires relation_ref"))?;
    let decision = payload_string(&item.payload, "decision")
        .ok_or_else(|| ApiError::invalid("identity review requires decision"))?;
    require_identity_review_approval_capability(context.auth, &decision)?;
    let reviewer_ref = payload_string(&item.payload, "reviewer_ref")
        .ok_or_else(|| ApiError::invalid("identity review requires reviewer_ref"))?;
    let reason = payload_string(&item.payload, "reason")
        .ok_or_else(|| ApiError::invalid("identity review requires reason"))?;
    let relation_id = context
        .local_refs
        .get(&relation_ref)
        .filter(|(_, kind)| *kind == RecordKind::Relation)
        .map(|(id, _)| *id)
        .or_else(|| {
            RecordRef::parse(&relation_ref)
                .ok()
                .map(|reference| reference.id)
        })
        .ok_or_else(|| ApiError::invalid("identity review relation_ref is invalid"))?;
    sqlx::query(
        r#"
        INSERT INTO straylight.relations (
          id,user_id,scope_id,current_version,policy_id,policy_version
        ) VALUES ($1,$2,$3,1,$4,$5)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(relation_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    let evidence_ids = evidence_for_payload(tx, context, &item.payload).await?;
    let evidence_id = evidence_ids
        .first()
        .copied()
        .ok_or_else(|| ApiError::invalid("identity review requires direct evidence_refs"))?;
    let reviewer = resolve_record(tx, context, &reviewer_ref, None).await?.0;
    sqlx::query(
        r#"
        INSERT INTO straylight.identity_review_receipts (
          id,user_id,scope_id,relation_id,decision,reviewer_ref,reason,evidence_id
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(receipt_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(relation_id)
    .bind(&decision)
    .bind(reviewer)
    .bind(&reason)
    .bind(evidence_id)
    .execute(&mut **tx)
    .await?;
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference: format!("identity-review:{receipt_id}"),
        record_id: None,
        prior_version: None,
        resulting_version: None,
        disposition: "created".to_owned(),
        details: json!({"relation_ref": relation_ref, "decision": decision}),
    });
    Ok(())
}

async fn process_import_receipt(
    _tx: &mut Transaction<'_, Postgres>,
    _context: &mut WriteContext<'_>,
    _order: usize,
    _item: SaveItem,
    _id: Uuid,
) -> ApiResult<()> {
    Err(ApiError::invalid(
        "import receipts are created by stage promotion, not supplied directly",
    ))
}

async fn process_tombstone(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    _id: Uuid,
) -> ApiResult<()> {
    context.auth.require(Capability::Delete)?;
    let target_ref = item
        .reference
        .as_deref()
        .ok_or_else(|| ApiError::invalid("tombstone requires target ref"))?;
    let (target_id, target_kind) = resolve_record(tx, context, target_ref, None).await?;
    let reason = payload_string(&item.payload, "reason")
        .ok_or_else(|| ApiError::invalid("tombstone requires reason"))?;
    if target_kind == RecordKind::Object
        && reason.to_ascii_lowercase().contains("cancel")
        && object_has_profile(tx, context.auth.user_id.0, target_id, "core.event").await?
    {
        return Err(ApiError::invalid(
            "event cancellation is a state:event.schedule@v1 assignment, never deletion",
        ));
    }
    let evidence_id = create_inline_evidence(
        tx,
        context,
        source_text(&item.payload).unwrap_or_else(|| reason.clone()),
        json!({"action": "tombstone", "target_ref": target_ref}),
    )
    .await?;
    let tombstone_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.tombstones (
          id,user_id,scope_id,target_record_id,source_episode_id,evidence_id,
          authorized_credential_id,reason
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(tombstone_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(target_id)
    .bind(context.operation_source_id)
    .bind(evidence_id)
    .bind(context.auth.credential_id.0)
    .bind(&reason)
    .execute(&mut **tx)
    .await?;
    let deletion_job_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO straylight.deletion_jobs (id,user_id,scope_id,tombstone_id,status) VALUES ($1,$2,$3,$4,'queued')",
    )
    .bind(deletion_job_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(tombstone_id)
    .execute(&mut **tx)
    .await?;
    for target_kind in [
        "source",
        "chunk",
        "lexical_index",
        "embedding",
        "cache",
        "export",
        "replica",
        "asset",
        "derived_view",
    ] {
        sqlx::query(
            r#"
            INSERT INTO straylight.derivative_cleanup_targets (
              id,user_id,scope_id,deletion_job_id,target_kind,target_ref,status
            ) VALUES ($1,$2,$3,$4,$5,$6,'pending')
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(deletion_job_id)
        .bind(target_kind)
        .bind(target_ref)
        .execute(&mut **tx)
        .await?;
    }
    if let Some(member) = context.members.get_mut(&target_id) {
        member.disposition = "tombstoned".to_owned();
    }
    add_member(context, tombstone_id, "tombstone", None, "active");
    context.receipts.push(ItemReceipt {
        order,
        action: "tombstone".to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference: format!("tombstone:{tombstone_id}"),
        record_id: Some(tombstone_id),
        prior_version: None,
        resulting_version: None,
        disposition: "tombstoned".to_owned(),
        details: json!({
            "target_ref": target_ref,
            "deletion_job_id": format!("deletion:{deletion_job_id}")
        }),
    });
    Ok(())
}

async fn evidence_for_payload(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    payload: &Value,
) -> ApiResult<Vec<Uuid>> {
    let mut ids = Vec::new();
    for reference in payload_strings(payload, "evidence_refs") {
        ids.push(
            resolve_record(tx, context, &reference, Some(RecordKind::Evidence))
                .await?
                .0,
        );
    }
    if ids.is_empty() {
        let text = source_text(payload).or_else(|| payload.get("value").map(Value::to_string));
        let text = text.ok_or_else(|| {
            ApiError::invalid("claim and relation writes require evidence_refs or source_text")
        })?;
        ids.push(create_inline_evidence(tx, context, text, json!({"inline": true})).await?);
    }
    Ok(ids)
}

async fn optional_record_id(
    tx: &mut Transaction<'_, Postgres>,
    context: &WriteContext<'_>,
    reference: Option<String>,
    kind: RecordKind,
) -> ApiResult<Option<Uuid>> {
    match reference {
        Some(reference) => Ok(Some(
            resolve_record(tx, context, &reference, Some(kind)).await?.0,
        )),
        None => Ok(None),
    }
}

async fn object_has_profile(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    object_id: Uuid,
    profile: &str,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM straylight.objects AS object
          JOIN straylight.object_revision_profiles AS profile_row
            ON profile_row.user_id=object.user_id AND profile_row.object_id=object.id
           AND profile_row.object_version=object.current_version
          WHERE object.user_id=$1 AND object.id=$2 AND profile_row.profile_ref=$3
        )
        "#,
    )
    .bind(user_id)
    .bind(object_id)
    .bind(profile)
    .fetch_one(&mut **tx)
    .await?)
}

fn json_array_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

async fn process_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    order: usize,
    item: SaveItem,
    checkpoint_id: Uuid,
    corpus_revision_id: Uuid,
) -> ApiResult<()> {
    let session_ref = payload_string(&item.payload, "session_id")
        .or_else(|| payload_string(&item.payload, "session_ref"))
        .ok_or_else(|| ApiError::invalid("checkpoint requires session_id"))?;
    let session_id = Uuid::parse_str(session_ref.trim_start_matches("session:"))
        .map_err(|_| ApiError::invalid("checkpoint session_id is invalid"))?;
    let session_row = sqlx::query(
        r#"
        SELECT scope_id,corpus_revision_id,mode,expires_at,resumed_checkpoint_id
        FROM straylight.sessions
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", &session_ref))?;
    let session_scope: Uuid = session_row.try_get("scope_id")?;
    if session_scope != context.scope_id {
        return Err(ApiError::public(
            http::StatusCode::FORBIDDEN,
            "scope_denied",
            "checkpoint scope differs from its session",
        ));
    }
    let mode: String = session_row.try_get("mode")?;
    let expires_at: DateTime<Utc> = session_row.try_get("expires_at")?;
    if mode == "read_only" || expires_at < Utc::now() {
        return Err(ApiError::capability("checkpoint"));
    }
    let parent_ref = payload_string(&item.payload, "parent_checkpoint_id")
        .or_else(|| payload_string(&item.payload, "parent_checkpoint_ref"));
    let parent_id = parent_ref
        .as_deref()
        .map(|reference| {
            Uuid::parse_str(reference.trim_start_matches("checkpoint:"))
                .map_err(|_| ApiError::invalid("parent checkpoint ref is invalid"))
        })
        .transpose()?;
    if let Some(parent_id) = parent_id {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM straylight.checkpoints WHERE user_id=$1 AND id=$2)",
        )
        .bind(context.auth.user_id.0)
        .bind(parent_id)
        .fetch_one(&mut **tx)
        .await?;
        if !exists {
            return Err(ApiError::not_found(
                "checkpoint_not_found",
                &format!("checkpoint:{parent_id}"),
            ));
        }
    }
    let expected_parent = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM straylight.checkpoints
        WHERE user_id=$1 AND session_id=$2
        ORDER BY created_at DESC,id DESC
        LIMIT 1
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?
    .or(session_row.try_get::<Option<Uuid>, _>("resumed_checkpoint_id")?);
    if expected_parent != parent_id {
        return Err(ApiError::conflict(
            "checkpoint_parent_conflict",
            "checkpoint must extend the latest durable state for this session",
            json!({
                "expected_parent": expected_parent.map(|id| format!("checkpoint:{id}")),
                "provided_parent": parent_id.map(|id| format!("checkpoint:{id}"))
            }),
        ));
    }
    let summary = item
        .payload
        .get("summary")
        .or_else(|| item.payload.get("state"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let summary_text = canonical_json(&summary)?;
    sqlx::query(
        r#"
        INSERT INTO straylight.checkpoints (
          id,user_id,scope_id,parent_checkpoint_id,corpus_revision_id,
          session_id,title,summary,policy_id,policy_version
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
        "#,
    )
    .bind(checkpoint_id)
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(parent_id)
    .bind(corpus_revision_id)
    .bind(session_id)
    .bind(payload_string(&item.payload, "title").or_else(|| payload_string(&summary, "objective")))
    .bind(&summary_text)
    .bind(context.policy_id)
    .bind(context.policy_version)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO straylight.corpus_members (
          user_id,scope_id,corpus_revision_id,record_id,record_kind,disposition
        ) VALUES ($1,$2,$3,$4,'checkpoint','active')
        "#,
    )
    .bind(context.auth.user_id.0)
    .bind(context.scope_id)
    .bind(corpus_revision_id)
    .bind(checkpoint_id)
    .execute(&mut **tx)
    .await?;

    let mut source_refs = payload_strings(&item.payload, "source_refs");
    if source_refs.is_empty() {
        source_refs = payload_strings(&summary, "source_refs");
    }
    if source_refs.is_empty() {
        source_refs.push(format!("source:{}", context.operation_source_id));
    }
    let mut seen_sources = HashSet::new();
    for (position, source_ref) in source_refs.iter().enumerate() {
        let (source_id, _) = resolve_record(tx, context, source_ref, None).await?;
        if !seen_sources.insert(source_id) {
            continue;
        }
        let member = context.members.get(&source_id).ok_or_else(|| {
            ApiError::invalid(format!(
                "checkpoint source is not in the pinned revision: {source_ref}"
            ))
        })?;
        if member.disposition == "tombstoned" {
            return Err(ApiError::invalid(format!(
                "checkpoint source is tombstoned: {source_ref}"
            )));
        }
        sqlx::query(
            r#"
            INSERT INTO straylight.checkpoint_source_refs (
              user_id,checkpoint_id,corpus_revision_id,position,source_record_id,
              source_version,locator
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(corpus_revision_id)
        .bind(position as i32)
        .bind(source_id)
        .bind(member.record_version)
        .bind(json!({"ref": source_ref}))
        .execute(&mut **tx)
        .await?;
    }

    let mut goals = summary
        .get("ordered_goals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if goals.is_empty()
        && let Some(objective) = summary.get("objective")
    {
        goals.push(objective.clone());
    }
    for (position, goal) in goals.iter().enumerate() {
        let text = value_text(goal);
        if text.trim().is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO straylight.checkpoint_goals (user_id,checkpoint_id,position,goal_text) VALUES ($1,$2,$3,$4)",
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(position as i32)
        .bind(text)
        .execute(&mut **tx)
        .await?;
    }
    let gaps = summary
        .get("unresolved_gaps")
        .or_else(|| summary.get("open_questions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (position, gap) in gaps.iter().enumerate() {
        let description = value_text(gap);
        if description.trim().is_empty() {
            continue;
        }
        sqlx::query(
            r#"
            INSERT INTO straylight.checkpoint_gaps (
              user_id,checkpoint_id,position,gap_kind,description
            ) VALUES ($1,$2,$3,'unresolved',$4)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(position as i32)
        .bind(description)
        .execute(&mut **tx)
        .await?;
    }
    let actions = summary
        .get("next_actions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (position, action) in actions.iter().enumerate() {
        let action_text = value_text(action);
        if action_text.trim().is_empty() {
            continue;
        }
        let responsible = action
            .get("responsible_ref")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let responsible_id = match responsible {
            Some(reference) => Some(resolve_record(tx, context, &reference, None).await?.0),
            None => None,
        };
        let due_id = optional_record_id(
            tx,
            context,
            action
                .get("due_temporal_ref")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            RecordKind::TemporalSpec,
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.checkpoint_actions (
              user_id,checkpoint_id,position,action_text,responsible_record_id,
              due_temporal_id,preconditions
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(position as i32)
        .bind(action_text)
        .bind(responsible_id)
        .bind(due_id)
        .bind(
            action
                .get("preconditions")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .execute(&mut **tx)
        .await?;
    }
    let focus_links = summary
        .get("focus_links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (position, focus) in focus_links.iter().enumerate() {
        let Some(reference) = focus.get("ref").and_then(Value::as_str) else {
            continue;
        };
        let lens = focus
            .get("lens")
            .and_then(Value::as_str)
            .unwrap_or("activity");
        let target = resolve_record(tx, context, reference, None).await?.0;
        sqlx::query(
            r#"
            INSERT INTO straylight.checkpoint_focus_links (
              user_id,checkpoint_id,position,target_record_id,lens
            ) VALUES ($1,$2,$3,$4,$5)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(position as i32)
        .bind(target)
        .bind(lens)
        .execute(&mut **tx)
        .await?;
    }
    for (position, state_ref) in payload_strings(&summary, "state_refs").iter().enumerate() {
        let state_id = resolve_record(tx, context, state_ref, Some(RecordKind::Claim))
            .await?
            .0;
        let is_assignment = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM straylight.state_assignments WHERE user_id=$1 AND claim_id=$2)",
        )
        .bind(context.auth.user_id.0)
        .bind(state_id)
        .fetch_one(&mut **tx)
        .await?;
        if !is_assignment {
            return Err(ApiError::invalid(format!(
                "checkpoint state ref is not a state assignment: {state_ref}"
            )));
        }
        sqlx::query(
            "INSERT INTO straylight.checkpoint_state_refs (user_id,checkpoint_id,position,state_claim_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(position as i32)
        .bind(state_id)
        .execute(&mut **tx)
        .await?;
    }
    let gates = summary
        .get("acceptance_gates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (position, gate) in gates.iter().enumerate() {
        let description = value_text(gate);
        let state_ref = gate.get("state_ref").and_then(Value::as_str);
        let state_claim_id = match state_ref {
            Some(reference) => Some(
                resolve_record(tx, context, reference, Some(RecordKind::Claim))
                    .await?
                    .0,
            ),
            None => None,
        };
        let evidence_refs = json_array_strings(gate.get("evidence_refs"));
        let passed = if let Some(id) = state_claim_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM straylight.claims WHERE user_id=$1 AND id=$2 AND predicate='state:gate.validation@v1' AND value='\"passed\"'::jsonb)",
            )
            .bind(context.auth.user_id.0)
            .bind(id)
            .fetch_one(&mut **tx)
            .await?
        } else {
            false
        };
        if passed && evidence_refs.is_empty() {
            return Err(ApiError::invalid(
                "a passed acceptance gate requires direct evidence_refs",
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO straylight.checkpoint_gates (
              user_id,checkpoint_id,position,description,state_claim_id
            ) VALUES ($1,$2,$3,$4,$5)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(checkpoint_id)
        .bind(position as i32)
        .bind(description)
        .bind(state_claim_id)
        .execute(&mut **tx)
        .await?;
        for evidence_ref in evidence_refs {
            let evidence_id =
                resolve_record(tx, context, &evidence_ref, Some(RecordKind::Evidence))
                    .await?
                    .0;
            sqlx::query(
                "INSERT INTO straylight.checkpoint_gate_evidence (user_id,checkpoint_id,gate_position,evidence_id) VALUES ($1,$2,$3,$4)",
            )
            .bind(context.auth.user_id.0)
            .bind(checkpoint_id)
            .bind(position as i32)
            .bind(evidence_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    let reference = format!("checkpoint:{checkpoint_id}");
    context
        .local_refs
        .insert(reference.clone(), (checkpoint_id, RecordKind::Checkpoint));
    context.receipts.push(ItemReceipt {
        order,
        action: action_name(item.action).to_owned(),
        kind: kind_name(item.kind).to_owned(),
        reference,
        record_id: Some(checkpoint_id),
        prior_version: None,
        resulting_version: None,
        disposition: "created".to_owned(),
        details: json!({
            "parent_checkpoint_id": parent_id.map(|id| format!("checkpoint:{id}")),
            "corpus_revision": revision_ref(corpus_revision_id),
            "source_refs": source_refs
        }),
    });
    Ok(())
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("description"))
            .or_else(|| object.get("action"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    }
}

async fn process_identity_fields(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    object_id: Uuid,
    profiles: &[String],
    payload: &Value,
) -> ApiResult<Vec<String>> {
    if !profiles.iter().any(|profile| {
        matches!(
            profile.as_str(),
            "core.person" | "core.organization" | "core.group"
        )
    }) {
        return Ok(vec![]);
    }
    let mut claim_refs = Vec::new();
    let names = payload
        .get("names")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for name in names {
        let (kind, value, locale) = match &name {
            Value::String(value) => ("display".to_owned(), value.clone(), None),
            Value::Object(object) => {
                let value = object
                    .get("value")
                    .or_else(|| object.get("name"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::invalid("name object requires value"))?
                    .to_owned();
                (
                    object
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("display")
                        .to_owned(),
                    value,
                    object
                        .get("locale")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                )
            }
            _ => return Err(ApiError::invalid("names must be strings or objects")),
        };
        let evidence_id = create_inline_evidence(
            tx,
            context,
            format!("{kind} name: {value}"),
            json!({"field": "names", "name_kind": kind}),
        )
        .await?;
        let claim_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO straylight.claims (
              id,user_id,scope_id,predicate,value,producer_ref,formation_method,
              claim_mode,support_state,authority,canonicality,source_episode_id,
              policy_id,policy_version
            ) VALUES (
              $1,$2,$3,'identity.name',$4,$5,'asserted','descriptive','reported',
              'authenticated_input','canonical',$5,$6,$7
            )
            "#,
        )
        .bind(claim_id)
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(json!({"kind": kind, "value": value, "locale": locale}))
        .bind(context.operation_source_id)
        .bind(context.policy_id)
        .bind(context.policy_version)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO straylight.claim_about_targets (user_id,claim_id,target_record_id,role) VALUES ($1,$2,$3,'about')",
        )
        .bind(context.auth.user_id.0)
        .bind(claim_id)
        .bind(object_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO straylight.claim_evidence_links (user_id,claim_id,evidence_id,support_kind) VALUES ($1,$2,$3,'supporting')",
        )
        .bind(context.auth.user_id.0)
        .bind(claim_id)
        .bind(evidence_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.identity_name_claims (
              user_id,claim_id,object_id,name_kind,name_value,locale
            ) VALUES ($1,$2,$3,$4,$5,$6)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(claim_id)
        .bind(object_id)
        .bind(&kind)
        .bind(&value)
        .bind(&locale)
        .execute(&mut **tx)
        .await?;
        add_member(context, claim_id, "claim", None, "active");
        let reference = format!("claim:{claim_id}");
        context
            .local_refs
            .insert(reference.clone(), (claim_id, RecordKind::Claim));
        claim_refs.push(reference);
    }
    let identifiers = payload
        .get("external_identifiers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for identifier in identifiers {
        let namespace = identifier
            .get("namespace")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("external identifier requires namespace"))?;
        let value = identifier
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("external identifier requires value"))?;
        let evidence_id = create_inline_evidence(
            tx,
            context,
            format!("External identifier {namespace}: {value}"),
            json!({"field": "external_identifiers", "namespace": namespace}),
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO straylight.external_identifiers (
              id,user_id,scope_id,object_id,namespace,identifier_value,issuer,
              source_episode_id,evidence_id,policy_id,policy_version
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(context.auth.user_id.0)
        .bind(context.scope_id)
        .bind(object_id)
        .bind(namespace)
        .bind(value)
        .bind(identifier.get("issuer").and_then(Value::as_str))
        .bind(context.operation_source_id)
        .bind(evidence_id)
        .bind(context.policy_id)
        .bind(context.policy_version)
        .execute(&mut **tx)
        .await?;
    }
    Ok(claim_refs)
}

async fn process_event_fields(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut WriteContext<'_>,
    object_id: Uuid,
    object_version: i32,
    prior_version: Option<i32>,
    profiles: &[String],
    payload: &Value,
) -> ApiResult<()> {
    if profiles
        .iter()
        .any(|profile| profile == "core.event_series")
    {
        let event = payload.get("event_series").unwrap_or(payload);
        if let (Some(source_uid), Some(original_ref), Some(current_ref)) = (
            event.get("source_uid").and_then(Value::as_str),
            event.get("original_temporal_ref").and_then(Value::as_str),
            event.get("current_temporal_ref").and_then(Value::as_str),
        ) {
            let original =
                resolve_record(tx, context, original_ref, Some(RecordKind::TemporalSpec))
                    .await?
                    .0;
            let current = resolve_record(tx, context, current_ref, Some(RecordKind::TemporalSpec))
                .await?
                .0;
            let recurrence = optional_record_id(
                tx,
                context,
                event
                    .get("recurrence_spec_ref")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                RecordKind::RecurrenceSpec,
            )
            .await?;
            sqlx::query(
                r#"
                INSERT INTO straylight.event_series_revisions (
                  user_id,object_id,object_version,source_uid,recurrence_spec_id,
                  original_temporal_id,current_temporal_id,scheduling_sequence
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                "#,
            )
            .bind(context.auth.user_id.0)
            .bind(object_id)
            .bind(object_version)
            .bind(source_uid)
            .bind(recurrence)
            .bind(original)
            .bind(current)
            .bind(
                event
                    .get("scheduling_sequence")
                    .and_then(Value::as_i64)
                    .map(|value| value as i32),
            )
            .execute(&mut **tx)
            .await?;
        }
    }
    if profiles
        .iter()
        .any(|profile| profile == "core.event_occurrence")
    {
        let occurrence = payload.get("event_occurrence").unwrap_or(payload);
        let series_ref = occurrence
            .get("series_ref")
            .or_else(|| occurrence.get("occurrence_of_ref"))
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("event occurrence requires series_ref"))?;
        let series_id = resolve_record(tx, context, series_ref, Some(RecordKind::Object))
            .await?
            .0;
        let recurrence_ref = occurrence
            .get("recurrence_temporal_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ApiError::invalid("event occurrence requires recurrence_temporal_ref")
            })?;
        let original_ref = occurrence
            .get("original_temporal_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("event occurrence requires original_temporal_ref"))?;
        let current_ref = occurrence
            .get("current_temporal_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::invalid("event occurrence requires current_temporal_ref"))?;
        let recurrence_temporal =
            resolve_record(tx, context, recurrence_ref, Some(RecordKind::TemporalSpec))
                .await?
                .0;
        let original_temporal =
            resolve_record(tx, context, original_ref, Some(RecordKind::TemporalSpec))
                .await?
                .0;
        let current_temporal =
            resolve_record(tx, context, current_ref, Some(RecordKind::TemporalSpec))
                .await?
                .0;
        let actual_temporal = optional_record_id(
            tx,
            context,
            occurrence
                .get("actual_temporal_ref")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            RecordKind::TemporalSpec,
        )
        .await?;
        if prior_version.is_none() {
            sqlx::query(
                r#"
                INSERT INTO straylight.event_occurrences (
                  user_id,scope_id,occurrence_object_id,series_object_id,
                  canonical_recurrence_id,recurrence_id_temporal_id,original_temporal_id
                ) VALUES ($1,$2,$3,$4,$5,$6,$7)
                "#,
            )
            .bind(context.auth.user_id.0)
            .bind(context.scope_id)
            .bind(object_id)
            .bind(series_id)
            .bind(
                occurrence
                    .get("canonical_recurrence_id")
                    .and_then(Value::as_str)
                    .unwrap_or(recurrence_ref),
            )
            .bind(recurrence_temporal)
            .bind(original_temporal)
            .execute(&mut **tx)
            .await?;
        }
        let evidence_id = evidence_for_payload(tx, context, occurrence).await?[0];
        sqlx::query(
            r#"
            INSERT INTO straylight.event_occurrence_revisions (
              user_id,occurrence_object_id,object_version,current_temporal_id,
              actual_temporal_id,exception_kind,evidence_id
            ) VALUES ($1,$2,$3,$4,$5,$6,$7)
            "#,
        )
        .bind(context.auth.user_id.0)
        .bind(object_id)
        .bind(object_version)
        .bind(current_temporal)
        .bind(actual_temporal)
        .bind(
            occurrence
                .get("exception_kind")
                .and_then(Value::as_str)
                .unwrap_or("generated"),
        )
        .bind(evidence_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use crate::models::{CredentialId, UserId};

    use super::*;

    fn test_auth(capabilities: &[Capability]) -> AuthContext {
        AuthContext {
            credential_id: CredentialId(Uuid::nil()),
            user_id: UserId(Uuid::nil()),
            capabilities: capabilities
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect(),
            scope_refs: vec!["scope:test".to_owned()],
            read_only: false,
        }
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Bytes {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            for (path, content) in entries {
                writer.start_file(*path, options).unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        Bytes::from(bytes)
    }

    #[test]
    fn staged_archive_members_are_hash_pinned_and_searchable() {
        let bytes = zip_bytes(&[("docs/readme.txt", b"immutable staged text")]);
        let (entries, input_hash, inventory_hash, warnings) =
            prepare_stage_entries(vec![StageUpload {
                path: "pack.zip".to_owned(),
                media_type: Some("application/zip".to_owned()),
                import_metadata: json!({}),
                content: StageUploadContent::Bytes(bytes),
            }])
            .unwrap();

        let member = entries
            .iter()
            .find(|entry| entry.path == "pack.zip!/docs/readme.txt")
            .unwrap();
        assert_eq!(member.entry_kind, "file");
        assert_eq!(member.readability, "readable");
        assert_eq!(
            member.content_hash.as_deref(),
            Some(sha256_prefixed(b"immutable staged text").as_str())
        );
        assert_eq!(
            member.inspection.get("search_text").and_then(Value::as_str),
            Some("immutable staged text")
        );
        assert_eq!(input_hash.len(), 64);
        assert_eq!(inventory_hash.len(), 64);
        assert!(warnings.is_empty());
    }

    #[test]
    fn staged_input_hash_is_stable_across_multipart_order() {
        let left = vec![
            StageUpload {
                path: "b.txt".to_owned(),
                media_type: None,
                import_metadata: json!({}),
                content: StageUploadContent::Bytes(Bytes::from_static(b"second")),
            },
            StageUpload {
                path: "a.txt".to_owned(),
                media_type: None,
                import_metadata: json!({}),
                content: StageUploadContent::Bytes(Bytes::from_static(b"first")),
            },
        ];
        let mut right = left.clone();
        right.reverse();

        let (_, left_input, left_inventory, _) = prepare_stage_entries(left).unwrap();
        let (_, right_input, right_inventory, _) = prepare_stage_entries(right).unwrap();
        assert_eq!(left_input, right_input);
        assert_eq!(left_inventory, right_inventory);
    }

    #[test]
    fn staged_paths_and_binary_text_indexes_remain_safe() {
        assert!(validate_stage_path("../escape.txt").is_err());
        assert!(validate_stage_path("archive.zip!/escape.txt").is_err());
        assert!(validate_stage_path(".carrystate/generated/descriptions/owned.md").is_err());
        assert!(validate_stage_path("nested/.CARRYSTATE/Generated/Descriptions/owned.md").is_err());
        assert!(validate_stage_path("safe/path.txt").is_ok());

        let (readability, text) = stage_text_index(b"not text\0with a payload");
        assert_eq!(readability, "unsupported");
        assert!(text.is_none());
    }

    #[test]
    fn snapshot_paths_allow_only_canonical_companions_for_present_sources() {
        let source = "Trips/receipt.jpg".to_owned();
        let description = asset_description::description_path(&source);
        assert!(validate_stage_path(&description).is_err());
        assert!(validate_snapshot_paths(&[source.clone(), description.clone()]).is_ok());
        assert!(validate_snapshot_paths(std::slice::from_ref(&description)).is_err());
        assert!(
            validate_snapshot_paths(&[
                source,
                ".carrystate/generated/descriptions/owned.md".to_owned()
            ])
            .is_err()
        );
    }

    #[test]
    fn staged_import_metadata_rejects_special_permission_bits() {
        assert!(validate_stage_import_metadata(&json!({"mode": 0o640})).is_ok());
        assert!(validate_stage_import_metadata(&json!({"mode": 0o4755})).is_err());
        assert!(validate_stage_import_metadata(&json!({"mode": "0644"})).is_err());
    }

    #[test]
    fn chunk_ids_are_stable_within_and_isolated_across_documents() {
        let first_document = Uuid::from_u128(1);
        let second_document = Uuid::from_u128(2);
        let normalized_ref = "chunk:00000000-0000-0000-0000-000000000003";

        assert_eq!(
            document_chunk_id(first_document, normalized_ref),
            document_chunk_id(first_document, normalized_ref)
        );
        assert_ne!(
            document_chunk_id(first_document, normalized_ref),
            document_chunk_id(second_document, normalized_ref)
        );
    }

    #[test]
    fn staged_source_versions_include_portable_file_metadata() {
        let content_hash = "a".repeat(64);
        let first = staged_source_version(
            &content_hash,
            "application/octet-stream",
            Some(&json!({"modified_unix_ns": 1, "mode": 420})),
        )
        .unwrap();
        let same = staged_source_version(
            &content_hash,
            "application/octet-stream",
            Some(&json!({"mode": 420, "modified_unix_ns": 1})),
        )
        .unwrap();
        let changed_mtime = staged_source_version(
            &content_hash,
            "application/octet-stream",
            Some(&json!({"modified_unix_ns": 2, "mode": 420})),
        )
        .unwrap();
        let changed_media_type = staged_source_version(
            &content_hash,
            "application/pdf",
            Some(&json!({
                "modified_unix_ns": 1,
                "mode": 420
            })),
        )
        .unwrap();

        assert_eq!(first, same);
        assert_ne!(first, changed_mtime);
        assert_ne!(first, changed_media_type);
    }

    #[test]
    fn claim_write_path_requires_every_authority_dimension() {
        let payload = json!({
            "producer_ref": "agent:writer",
            "formation_method": "extracted",
            "support_state": "supported",
            "authority": "source_attributed",
            "canonicality": "candidate"
        });
        assert!(claim_authority_dimensions(&payload).is_ok());

        for field in [
            "producer_ref",
            "formation_method",
            "support_state",
            "authority",
            "canonicality",
        ] {
            let mut missing = payload.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                claim_authority_dimensions(&missing).is_err(),
                "{field} must not receive a write-service default"
            );
        }
    }

    #[test]
    fn approved_identity_reviews_require_privileged_capability() {
        let save_only = test_auth(&[Capability::Save]);
        assert!(require_identity_review_approval_capability(&save_only, "rejected").is_ok());
        assert!(require_identity_review_approval_capability(&save_only, "approved").is_err());

        let credential_manager = test_auth(&[Capability::Save, Capability::CredentialManage]);
        assert!(
            require_identity_review_approval_capability(&credential_manager, "approved").is_ok()
        );
        let admin = test_auth(&[Capability::Save, Capability::Admin]);
        assert!(require_identity_review_approval_capability(&admin, "approved").is_ok());
    }
}

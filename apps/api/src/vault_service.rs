use std::time::Instant;

use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    asset_service::{self, AuthorizedAsset},
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
    session_service,
};

#[derive(Clone, Debug, Deserialize)]
pub struct VaultManifestQuery {
    pub session_id: String,
    pub stable_import_id: String,
    #[serde(default)]
    pub include_history: bool,
    #[serde(default)]
    pub include_scope_memory: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VaultContentQuery {
    pub session_id: String,
    pub stable_import_id: String,
}

pub async fn manifest(
    state: &AppState,
    auth: &AuthContext,
    query: &VaultManifestQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Read)?;
    validate_stable_import_id(&query.stable_import_id)?;
    let session_id = parse_ref(&query.session_id, "session")?;
    let mut tx = state.begin_read(auth).await?;
    let rows = sqlx::query(
        r#"
        WITH target_session AS (
          SELECT session.user_id,session.id,session.credential_id,
                 session.scope_id,session.corpus_revision_id,
                 revision.revision_number
          FROM straylight.sessions AS session
          JOIN straylight.corpus_revisions AS revision
            ON revision.user_id=session.user_id
           AND revision.id=session.corpus_revision_id
          WHERE session.user_id=$1 AND session.id=$2
            AND session.credential_id=$3
            AND session.expires_at > clock_timestamp()
            AND session.scope_id=ANY(
              SELECT scope.id
              FROM straylight.scopes AS scope
              WHERE scope.user_id=session.user_id
                AND scope.scope_ref=ANY($4::text[])
            )
        )
        SELECT source.id AS source_id,source.source_ref,source.source_kind,
               source.native_locator->>'path' AS path,source.captured_at,
               source.metadata->'import_metadata' AS import_metadata,
               source.metadata->>'original_path' AS original_path,
               asset.id AS asset_id,version.version,version.previous_version,
               version.content_hash,version.size_bytes,version.media_type,
               version.stored_at,
               EXISTS (
                 SELECT 1
                 FROM target_session AS session
                 JOIN straylight.corpus_members AS source_member
                   ON source_member.user_id=session.user_id
                  AND source_member.scope_id=session.scope_id
                  AND source_member.corpus_revision_id=session.corpus_revision_id
                  AND source_member.record_kind='source_episode'
                  AND source_member.record_id=source.id
                  AND source_member.disposition='active'
                 JOIN straylight.corpus_members AS asset_member
                   ON asset_member.user_id=session.user_id
                  AND asset_member.scope_id=session.scope_id
                  AND asset_member.corpus_revision_id=session.corpus_revision_id
                  AND asset_member.record_kind='asset'
                  AND asset_member.record_id=asset.id
                  AND asset_member.record_version=version.version
                  AND asset_member.disposition='active'
               ) AS active
        FROM target_session AS session
        JOIN straylight.source_episodes AS source
          ON source.user_id=session.user_id AND source.scope_id=session.scope_id
        JOIN straylight.source_asset_links AS link
          ON link.user_id=source.user_id AND link.source_episode_id=source.id
         AND link.role='source_native'
        JOIN straylight.assets AS asset
          ON asset.user_id=link.user_id AND asset.id=link.asset_id
         AND asset.scope_id=session.scope_id
        JOIN straylight.asset_versions AS version
          ON version.user_id=link.user_id AND version.asset_id=link.asset_id
         AND version.version=link.asset_version
        WHERE source.metadata->>'stable_import_id'=$5
          AND (
            (
              $6
              AND EXISTS (
                SELECT 1
                FROM straylight.corpus_revisions AS historical_revision
                JOIN straylight.corpus_members AS historical_source
                  ON historical_source.user_id=historical_revision.user_id
                 AND historical_source.scope_id=historical_revision.scope_id
                 AND historical_source.corpus_revision_id=historical_revision.id
                 AND historical_source.record_kind='source_episode'
                 AND historical_source.record_id=source.id
                 AND historical_source.disposition='active'
                JOIN straylight.corpus_members AS historical_asset
                  ON historical_asset.user_id=historical_revision.user_id
                 AND historical_asset.scope_id=historical_revision.scope_id
                 AND historical_asset.corpus_revision_id=historical_revision.id
                 AND historical_asset.record_kind='asset'
                 AND historical_asset.record_id=asset.id
                 AND historical_asset.record_version=version.version
                 AND historical_asset.disposition='active'
                WHERE historical_revision.user_id=session.user_id
                  AND historical_revision.scope_id=session.scope_id
                  AND historical_revision.revision_number <= session.revision_number
              )
            )
            OR (
              NOT $6
              AND
              EXISTS (
                SELECT 1
                FROM straylight.corpus_members AS member
                WHERE member.user_id=session.user_id
                  AND member.scope_id=session.scope_id
                  AND member.corpus_revision_id=session.corpus_revision_id
                  AND member.record_kind='source_episode'
                  AND member.record_id=source.id
                  AND member.disposition='active'
              )
              AND EXISTS (
                SELECT 1
                FROM straylight.corpus_members AS member
                WHERE member.user_id=session.user_id
                  AND member.scope_id=session.scope_id
                  AND member.corpus_revision_id=session.corpus_revision_id
                  AND member.record_kind='asset'
                  AND member.record_id=asset.id
                  AND member.record_version=version.version
                  AND member.disposition='active'
              )
            )
          )
        ORDER BY source.native_locator->>'path',source.captured_at,
                 source.id,version.version
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(auth.credential_id.0)
    .bind(&auth.scope_refs)
    .bind(&query.stable_import_id)
    .bind(query.include_history)
    .fetch_all(&mut *tx)
    .await?;
    let session_row = sqlx::query(
        r#"
        SELECT scope_id,corpus_revision_id
        FROM straylight.sessions
        WHERE user_id=$1 AND id=$2 AND credential_id=$3
          AND expires_at > clock_timestamp()
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(auth.credential_id.0)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("session_not_found", &query.session_id))?;
    let scope_id: Uuid = session_row.try_get("scope_id")?;
    let revision_id: Uuid = session_row.try_get("corpus_revision_id")?;
    let native_records = if query.include_scope_memory {
        load_native_records(&mut tx, auth.user_id.0, scope_id, revision_id).await?
    } else {
        Vec::new()
    };
    tx.commit().await?;
    let entries = rows
        .into_iter()
        .map(|row| {
            let source_id: Uuid = row.try_get("source_id")?;
            let asset_id: Uuid = row.try_get("asset_id")?;
            let source_kind: String = row.try_get("source_kind")?;
            Ok(json!({
                "path": row.try_get::<Option<String>,_>("path")?,
                "original_path": row.try_get::<Option<String>,_>("original_path")?,
                "import_metadata": row.try_get::<Option<Value>,_>("import_metadata")?,
                "category": if source_kind == "derived_asset_description" {
                    "generated_description"
                } else {
                    "source"
                },
                "source_ref": format!("source:{source_id}"),
                "source_kind": source_kind,
                "asset_ref": format!("asset:{asset_id}"),
                "version": row.try_get::<i32,_>("version")?,
                "previous_version": row.try_get::<Option<i32>,_>("previous_version")?,
                "content_hash": format!(
                    "sha256:{}",
                    row.try_get::<String,_>("content_hash")?
                ),
                "size_bytes": row.try_get::<i64,_>("size_bytes")?,
                "media_type": row.try_get::<String,_>("media_type")?,
                "captured_at": row.try_get::<chrono::DateTime<chrono::Utc>,_>("captured_at")?,
                "stored_at": row.try_get::<chrono::DateTime<chrono::Utc>,_>("stored_at")?,
                "active": row.try_get::<bool,_>("active")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let result = json!({
        "format": "carrystate-vault-manifest@v1",
        "stable_import_id": query.stable_import_id,
        "corpus_revision": format!("revision:{revision_id}"),
        "include_history": query.include_history,
        "include_scope_memory": query.include_scope_memory,
        "entries": entries,
        "native_records": native_records
    });
    let projected = session_service::project_response(
        state,
        auth,
        &query.session_id,
        "vault.export.manifest",
        "read",
        result,
    )
    .await?;
    metrics::counter!(
        "vault.export.manifests",
        "history" => if query.include_history { "true" } else { "false" },
        "scope_memory" => if query.include_scope_memory { "true" } else { "false" },
        "result" => "success"
    )
    .increment(1);
    Ok(projected)
}

async fn load_native_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    scope_id: Uuid,
    revision_id: Uuid,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        WITH member AS (
          SELECT record_id,record_kind,record_version
          FROM straylight.corpus_members
          WHERE user_id=$1 AND scope_id=$2 AND corpus_revision_id=$3
            AND disposition='active'
        ), native AS (
          SELECT member.record_id,member.record_kind,member.record_version,
                 coalesce(revision.label,'Object') AS title,
                 jsonb_build_object(
                   'record',to_jsonb(object_row),
                   'revision',to_jsonb(revision),
                   'profiles',coalesce((
                     SELECT jsonb_agg(to_jsonb(profile) ORDER BY profile.profile_ref)
                     FROM straylight.object_revision_profiles AS profile
                     WHERE profile.user_id=object_row.user_id
                       AND profile.object_id=object_row.id
                       AND profile.object_version=revision.version
                   ),'[]'::jsonb),
                   'handles',coalesce((
                     SELECT jsonb_agg(to_jsonb(handle) ORDER BY handle.valid_from,handle.id)
                     FROM straylight.object_handles AS handle
                     WHERE handle.user_id=object_row.user_id
                       AND handle.object_id=object_row.id
                   ),'[]'::jsonb),
                   'artifact_asset_links',coalesce((
                     SELECT jsonb_agg(to_jsonb(link) ORDER BY link.role,link.asset_id)
                     FROM straylight.artifact_asset_links AS link
                     WHERE link.user_id=object_row.user_id
                       AND link.object_id=object_row.id
                       AND link.object_version=revision.version
                   ),'[]'::jsonb),
                   'external_identifiers',coalesce((
                     SELECT jsonb_agg(to_jsonb(identifier) ORDER BY identifier.created_at,identifier.id)
                     FROM straylight.external_identifiers AS identifier
                     WHERE identifier.user_id=object_row.user_id
                       AND identifier.object_id=object_row.id
                   ),'[]'::jsonb),
                   'event_series_revision',(
                     SELECT to_jsonb(series)
                     FROM straylight.event_series_revisions AS series
                     WHERE series.user_id=object_row.user_id
                       AND series.object_id=object_row.id
                       AND series.object_version=revision.version
                   ),
                   'event_occurrence',(
                     SELECT to_jsonb(occurrence)
                     FROM straylight.event_occurrences AS occurrence
                     WHERE occurrence.user_id=object_row.user_id
                       AND occurrence.occurrence_object_id=object_row.id
                   ),
                   'event_occurrence_revision',(
                     SELECT to_jsonb(occurrence_revision)
                     FROM straylight.event_occurrence_revisions AS occurrence_revision
                     WHERE occurrence_revision.user_id=object_row.user_id
                       AND occurrence_revision.occurrence_object_id=object_row.id
                       AND occurrence_revision.object_version=revision.version
                   )
                 ) AS payload
          FROM member
          JOIN straylight.objects AS object_row
            ON member.record_kind='object'
           AND object_row.user_id=$1 AND object_row.id=member.record_id
          JOIN straylight.object_revisions AS revision
            ON revision.user_id=object_row.user_id
           AND revision.object_id=object_row.id
           AND revision.version=coalesce(member.record_version,object_row.current_version)
          UNION ALL
          SELECT member.record_id,member.record_kind,member.record_version,
                 claim.predicate::text,
                 jsonb_build_object(
                   'record',to_jsonb(claim),
                   'about_targets',coalesce((
                     SELECT jsonb_agg(to_jsonb(target) ORDER BY target.role,target.target_record_id)
                     FROM straylight.claim_about_targets AS target
                     WHERE target.user_id=claim.user_id AND target.claim_id=claim.id
                   ),'[]'::jsonb),
                   'evidence_links',coalesce((
                     SELECT jsonb_agg(to_jsonb(link) ORDER BY link.support_kind,link.evidence_id)
                     FROM straylight.claim_evidence_links AS link
                     WHERE link.user_id=claim.user_id AND link.claim_id=claim.id
                   ),'[]'::jsonb),
                   'lineage',coalesce((
                     SELECT jsonb_agg(to_jsonb(lineage) ORDER BY lineage.lineage_kind,lineage.predecessor_claim_id)
                     FROM straylight.claim_lineage AS lineage
                     WHERE lineage.user_id=claim.user_id AND lineage.claim_id=claim.id
                   ),'[]'::jsonb),
                   'state_assignment',(
                     SELECT to_jsonb(assignment)
                     FROM straylight.state_assignments AS assignment
                     WHERE assignment.user_id=claim.user_id AND assignment.claim_id=claim.id
                   ),
                   'identity_name_claim',(
                     SELECT to_jsonb(name_claim)
                     FROM straylight.identity_name_claims AS name_claim
                     WHERE name_claim.user_id=claim.user_id AND name_claim.claim_id=claim.id
                   )
                 )
          FROM member
          JOIN straylight.claims AS claim
            ON member.record_kind='claim'
           AND claim.user_id=$1 AND claim.id=member.record_id
          UNION ALL
          SELECT member.record_id,member.record_kind,member.record_version,
                 revision.predicate::text,
                 jsonb_build_object(
                   'record',to_jsonb(relation),
                   'revision',to_jsonb(revision),
                   'endpoints',coalesce((
                     SELECT jsonb_agg(to_jsonb(endpoint) ORDER BY endpoint.role,endpoint.position)
                     FROM straylight.relation_endpoints AS endpoint
                     WHERE endpoint.user_id=relation.user_id
                       AND endpoint.relation_id=relation.id
                       AND endpoint.relation_version=revision.version
                   ),'[]'::jsonb),
                   'evidence_links',coalesce((
                     SELECT jsonb_agg(to_jsonb(link) ORDER BY link.support_kind,link.evidence_id)
                     FROM straylight.relation_evidence_links AS link
                     WHERE link.user_id=relation.user_id
                       AND link.relation_id=relation.id
                       AND link.relation_version=revision.version
                   ),'[]'::jsonb)
                 )
          FROM member
          JOIN straylight.relations AS relation
            ON member.record_kind='relation'
           AND relation.user_id=$1 AND relation.id=member.record_id
          JOIN straylight.relation_revisions AS revision
            ON revision.user_id=relation.user_id
           AND revision.relation_id=relation.id
           AND revision.version=coalesce(member.record_version,relation.current_version)
          UNION ALL
          SELECT member.record_id,member.record_kind,member.record_version,
                 evidence.evidence_kind,
                 jsonb_build_object('record',to_jsonb(evidence))
          FROM member
          JOIN straylight.evidence_items AS evidence
            ON member.record_kind='evidence'
           AND evidence.user_id=$1 AND evidence.id=member.record_id
          UNION ALL
          SELECT member.record_id,member.record_kind,member.record_version,
                 coalesce(checkpoint.title,'Checkpoint'),
                 jsonb_build_object(
                   'record',to_jsonb(checkpoint),
                   'focus_links',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_focus_links AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'goals',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_goals AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'state_refs',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_state_refs AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'gaps',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_gaps AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'decisions',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_decisions AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'gates',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_gates AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'gate_evidence',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.gate_position,item.evidence_id)
                     FROM straylight.checkpoint_gate_evidence AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'actions',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_actions AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb),
                   'source_refs',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.position)
                     FROM straylight.checkpoint_source_refs AS item
                     WHERE item.user_id=checkpoint.user_id AND item.checkpoint_id=checkpoint.id
                   ),'[]'::jsonb)
                 )
          FROM member
          JOIN straylight.checkpoints AS checkpoint
            ON member.record_kind='checkpoint'
           AND checkpoint.user_id=$1 AND checkpoint.id=member.record_id
          UNION ALL
          SELECT member.record_id,member.record_kind,member.record_version,
                 temporal.kind,
                 jsonb_build_object('record',to_jsonb(temporal))
          FROM member
          JOIN straylight.temporal_specs AS temporal
            ON member.record_kind='temporal_spec'
           AND temporal.user_id=$1 AND temporal.id=member.record_id
          UNION ALL
          SELECT member.record_id,member.record_kind,member.record_version,
                 'Recurrence',
                 jsonb_build_object(
                   'record',to_jsonb(recurrence),
                   'rules',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.rule_order)
                     FROM straylight.recurrence_rules AS item
                     WHERE item.user_id=recurrence.user_id
                       AND item.recurrence_spec_id=recurrence.id
                   ),'[]'::jsonb),
                   'exclusions',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.recurrence_id_temporal_id)
                     FROM straylight.recurrence_exclusions AS item
                     WHERE item.user_id=recurrence.user_id
                       AND item.recurrence_spec_id=recurrence.id
                   ),'[]'::jsonb),
                   'additions',coalesce((
                     SELECT jsonb_agg(to_jsonb(item) ORDER BY item.occurrence_object_id)
                     FROM straylight.recurrence_additions AS item
                     WHERE item.user_id=recurrence.user_id
                       AND item.recurrence_spec_id=recurrence.id
                   ),'[]'::jsonb)
                 )
          FROM member
          JOIN straylight.recurrence_specs AS recurrence
            ON member.record_kind='recurrence_spec'
           AND recurrence.user_id=$1 AND recurrence.id=member.record_id
        )
        SELECT record_id,record_kind,record_version,title,payload
        FROM native
        ORDER BY record_kind,record_id
        "#,
    )
    .bind(user_id)
    .bind(scope_id)
    .bind(revision_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let record_id: Uuid = row.try_get("record_id")?;
            let record_kind: String = row.try_get("record_kind")?;
            Ok(json!({
                "record_ref": format!("{record_kind}:{record_id}"),
                "record_kind": record_kind,
                "record_version": row.try_get::<Option<i32>,_>("record_version")?,
                "title": row.try_get::<String,_>("title")?,
                "payload": row.try_get::<Value,_>("payload")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::from)
}

pub async fn content(
    state: &AppState,
    auth: &AuthContext,
    asset_ref: &str,
    version: i32,
    query: &VaultContentQuery,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    auth.require(Capability::Read)?;
    if version <= 0 {
        return Err(ApiError::invalid("asset version must be positive"));
    }
    validate_stable_import_id(&query.stable_import_id)?;
    let session_id = parse_ref(&query.session_id, "session")?;
    let asset_id = parse_ref(asset_ref, "asset")?;
    let mut tx = state.begin_read(auth).await?;
    let row = sqlx::query(
        r#"
        SELECT asset.id AS asset_id,session.scope_id,session.corpus_revision_id,
               version.version,version.object_key,version.content_hash,
               version.object_version_id,
               version.size_bytes,version.media_type,version.etag,version.metadata,
               source.native_locator->>'path' AS path
        FROM straylight.sessions AS session
        JOIN straylight.corpus_revisions AS pinned_revision
          ON pinned_revision.user_id=session.user_id
         AND pinned_revision.id=session.corpus_revision_id
        JOIN straylight.source_episodes AS source
          ON source.user_id=session.user_id AND source.scope_id=session.scope_id
         AND source.metadata->>'stable_import_id'=$7
        JOIN straylight.source_asset_links AS link
          ON link.user_id=source.user_id AND link.source_episode_id=source.id
         AND link.role='source_native'
        JOIN straylight.assets AS asset
          ON asset.user_id=link.user_id AND asset.id=link.asset_id
         AND asset.scope_id=session.scope_id
        JOIN straylight.asset_versions AS version
          ON version.user_id=link.user_id AND version.asset_id=link.asset_id
         AND version.version=link.asset_version
        WHERE session.user_id=$1 AND session.id=$2
          AND session.credential_id=$3
          AND session.expires_at > clock_timestamp()
          AND asset.id=$4 AND version.version=$5
          AND EXISTS (
            SELECT 1
            FROM straylight.corpus_revisions AS historical_revision
            JOIN straylight.corpus_members AS historical_source
              ON historical_source.user_id=historical_revision.user_id
             AND historical_source.scope_id=historical_revision.scope_id
             AND historical_source.corpus_revision_id=historical_revision.id
             AND historical_source.record_kind='source_episode'
             AND historical_source.record_id=source.id
             AND historical_source.disposition='active'
            JOIN straylight.corpus_members AS historical_asset
              ON historical_asset.user_id=historical_revision.user_id
             AND historical_asset.scope_id=historical_revision.scope_id
             AND historical_asset.corpus_revision_id=historical_revision.id
             AND historical_asset.record_kind='asset'
             AND historical_asset.record_id=asset.id
             AND historical_asset.record_version=version.version
             AND historical_asset.disposition='active'
            WHERE historical_revision.user_id=session.user_id
              AND historical_revision.scope_id=session.scope_id
              AND historical_revision.revision_number <= pinned_revision.revision_number
          )
          AND session.scope_id=ANY(
            SELECT scope.id
            FROM straylight.scopes AS scope
            WHERE scope.user_id=session.user_id
              AND scope.scope_ref=ANY($6::text[])
          )
        ORDER BY source.captured_at DESC,source.id DESC
        LIMIT 1
        "#,
    )
    .bind(auth.user_id.0)
    .bind(session_id)
    .bind(auth.credential_id.0)
    .bind(asset_id)
    .bind(version)
    .bind(&auth.scope_refs)
    .bind(&query.stable_import_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("asset_not_found", asset_ref))?;
    let asset = AuthorizedAsset {
        asset_id: row.try_get("asset_id")?,
        scope_id: row.try_get("scope_id")?,
        revision_id: row.try_get("corpus_revision_id")?,
        version: row.try_get("version")?,
        object_key: row.try_get("object_key")?,
        object_version_id: row.try_get("object_version_id")?,
        content_hash: row.try_get("content_hash")?,
        size_bytes: row.try_get("size_bytes")?,
        media_type: row.try_get("media_type")?,
        etag: row.try_get("etag")?,
        metadata: row.try_get("metadata")?,
        path: row.try_get("path")?,
    };
    tx.commit().await?;
    let projection = session_service::project_response(
        state,
        auth,
        &query.session_id,
        "vault.export.content",
        "read",
        json!({
            "content": true,
            "asset_ref": format!("asset:{}", asset.asset_id),
            "version": asset.version,
            "content_hash": format!("sha256:{}", asset.content_hash)
        }),
    )
    .await?;
    if projection.get("content").and_then(Value::as_bool) != Some(true) {
        return Err(ApiError::public(
            StatusCode::FORBIDDEN,
            "policy_denied",
            "the effective policy does not allow atomic vault export",
        ));
    }
    asset_service::stream_authorized_content(state, auth, asset, headers, Instant::now()).await
}

fn validate_stable_import_id(value: &str) -> ApiResult<()> {
    if value.trim().is_empty() || value.len() > 240 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid("stable_import_id is invalid"));
    }
    Ok(())
}

fn parse_ref(value: &str, prefix: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value.trim_start_matches(&format!("{prefix}:")))
        .map_err(|_| ApiError::invalid(format!("{prefix} reference is invalid")))
}

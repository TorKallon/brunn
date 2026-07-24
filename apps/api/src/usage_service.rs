use std::{collections::BTreeMap, time::Instant};

use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::ApiResult,
    models::{Capability, ListQuery},
    refs::RecordRef,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisibleRecordAccess {
    record_id: Uuid,
    occurrence_count: i32,
}

pub async fn record_projection_accesses(
    state: &AppState,
    auth: &AuthContext,
    scope_id: Uuid,
    session_id: Uuid,
    corpus_revision_id: Uuid,
    projection_receipt_id: Uuid,
    operation: &str,
    projected_data: &Value,
) -> ApiResult<u64> {
    let started = Instant::now();
    let Some(operation_kind) = tracked_operation_kind(operation) else {
        return Ok(0);
    };
    let accesses = collect_visible_record_accesses(operation_kind, projected_data);
    if accesses.is_empty() {
        metrics::counter!(
            "usage_tracking.responses",
            "operation" => operation_kind,
            "has_records" => "false"
        )
        .increment(1);
        return Ok(0);
    }

    let record_ids: Vec<_> = accesses.iter().map(|access| access.record_id).collect();
    let occurrence_counts: Vec<_> = accesses
        .iter()
        .map(|access| access.occurrence_count)
        .collect();
    let mut tx = state.begin_write(auth).await?;
    let inserted = sqlx::query(
        r#"
        INSERT INTO straylight.record_access_events (
          id,user_id,scope_id,credential_id,session_id,corpus_revision_id,
          projection_receipt_id,target_record_id,operation_kind,occurrence_count,
          accessed_at
        )
        SELECT
          gen_random_uuid(),$1,$2,$3,$4,$5,$6,access.record_id,$7,
          access.occurrence_count,statement_timestamp()
        FROM unnest($8::uuid[],$9::integer[])
          AS access(record_id,occurrence_count)
        JOIN straylight.record_keys AS record_key
          ON record_key.user_id=$1
         AND record_key.scope_id=$2
         AND record_key.record_id=access.record_id
        ON CONFLICT (user_id,projection_receipt_id,target_record_id) DO NOTHING
        "#,
    )
    .bind(auth.user_id.0)
    .bind(scope_id)
    .bind(auth.credential_id.0)
    .bind(session_id)
    .bind(corpus_revision_id)
    .bind(projection_receipt_id)
    .bind(operation_kind)
    .bind(&record_ids)
    .bind(&occurrence_counts)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    metrics::counter!(
        "usage_tracking.responses",
        "operation" => operation_kind,
        "has_records" => "true"
    )
    .increment(1);
    metrics::histogram!(
        "usage_tracking.duration_ms",
        "operation" => operation_kind
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!(
        "usage_tracking.record_exposures",
        "operation" => operation_kind
    )
    .record(inserted as f64);
    metrics::histogram!(
        "usage_tracking.reference_occurrences",
        "operation" => operation_kind
    )
    .record(occurrence_counts.iter().sum::<i32>() as f64);
    Ok(inserted)
}

pub async fn data_usage(
    state: &AppState,
    auth: &AuthContext,
    query: &ListQuery,
) -> ApiResult<Value> {
    let started = Instant::now();
    auth.require(Capability::Read)?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100) as i64;
    let mut tx = state.begin_read(auth).await?;
    let result = sqlx::query(SOURCE_USAGE_QUERY)
        .bind(auth.user_id.0)
        .bind(query.scope.as_deref())
        .bind(query.query.as_deref())
        .bind(limit)
        .fetch_one(&mut *tx)
        .await?
        .try_get("usage")?;
    tx.commit().await?;
    metrics::counter!("usage_queries").increment(1);
    metrics::histogram!("usage_query.duration_ms")
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    Ok(result)
}

fn tracked_operation_kind(operation: &str) -> Option<&'static str> {
    match operation {
        "memory.open" => Some("open"),
        "memory.query" => Some("query"),
        "memory.read" => Some("read"),
        "memory.compute" => Some("compute"),
        "memory.verify" => Some("verify"),
        _ => None,
    }
}

fn collect_visible_record_accesses(
    operation_kind: &str,
    value: &Value,
) -> Vec<VisibleRecordAccess> {
    fn visit(value: &Value, counts: &mut BTreeMap<Uuid, i32>) {
        match value {
            Value::String(value) => {
                if let Ok(reference) = RecordRef::parse(value) {
                    let count = counts.entry(reference.id).or_default();
                    *count = count.saturating_add(1);
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, counts);
                }
            }
            Value::Object(object) => {
                let unavailable_read_item = object.get("data").is_some_and(Value::is_null)
                    && object
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|status| matches!(status, "failed" | "unsupported"));
                for (key, value) in object {
                    if unavailable_read_item && matches!(key.as_str(), "ref" | "reference") {
                        continue;
                    }
                    visit(value, counts);
                }
            }
            _ => {}
        }
    }

    let mut counts = BTreeMap::new();
    let fields: &[&str] = match operation_kind {
        "open" => &[
            "resume_checkpoint",
            "revision_delta",
            "learned_context",
            "initial_evidence",
            "hydrated_sources",
            "initial_case_file",
        ],
        "query" => &["items"],
        "read" => &["items"],
        "compute" => &["steps"],
        "verify" => &["claims"],
        _ => &[],
    };
    let mut selected = false;
    for field in fields {
        if let Some(field_value) = value.get(field) {
            selected = true;
            visit(field_value, &mut counts);
        }
    }
    if !selected {
        visit(value, &mut counts);
    }
    counts
        .into_iter()
        .map(|(record_id, occurrence_count)| VisibleRecordAccess {
            record_id,
            occurrence_count,
        })
        .collect()
}

const SOURCE_USAGE_QUERY: &str = r#"
WITH mapped_access AS (
  SELECT access.id AS event_id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,source.id AS source_id
  FROM straylight.record_access_events AS access
  JOIN straylight.source_episodes AS source
    ON source.user_id=access.user_id AND source.id=access.target_record_id
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,evidence.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.evidence_items AS evidence
    ON evidence.user_id=access.user_id AND evidence.id=access.target_record_id
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,revision.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.chunks AS chunk
    ON chunk.user_id=access.user_id AND chunk.id=access.target_record_id
  JOIN straylight.document_revisions AS revision
    ON revision.user_id=chunk.user_id
   AND revision.document_id=chunk.document_id
   AND revision.version=chunk.document_version
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,revision.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.documents AS document
    ON document.user_id=access.user_id AND document.id=access.target_record_id
  LEFT JOIN straylight.corpus_members AS member
    ON member.user_id=access.user_id
   AND member.corpus_revision_id=access.corpus_revision_id
   AND member.record_id=access.target_record_id
  JOIN straylight.document_revisions AS revision
    ON revision.user_id=document.user_id
   AND revision.document_id=document.id
   AND revision.version=coalesce(member.record_version,document.current_version)
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,claim.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.claims AS claim
    ON claim.user_id=access.user_id AND claim.id=access.target_record_id
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,revision.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.objects AS object
    ON object.user_id=access.user_id AND object.id=access.target_record_id
  LEFT JOIN straylight.corpus_members AS member
    ON member.user_id=access.user_id
   AND member.corpus_revision_id=access.corpus_revision_id
   AND member.record_id=access.target_record_id
  JOIN straylight.object_revisions AS revision
    ON revision.user_id=object.user_id
   AND revision.object_id=object.id
   AND revision.version=coalesce(member.record_version,object.current_version)
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,revision.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.relations AS relation
    ON relation.user_id=access.user_id AND relation.id=access.target_record_id
  LEFT JOIN straylight.corpus_members AS member
    ON member.user_id=access.user_id
   AND member.corpus_revision_id=access.corpus_revision_id
   AND member.record_id=access.target_record_id
  JOIN straylight.relation_revisions AS revision
    ON revision.user_id=relation.user_id
   AND revision.relation_id=relation.id
   AND revision.version=coalesce(member.record_version,relation.current_version)
  WHERE access.user_id=$1

  UNION ALL

  SELECT access.id,access.user_id,access.projection_receipt_id,
         access.session_id,access.operation_kind,access.occurrence_count,
         access.accessed_at,link.source_episode_id
  FROM straylight.record_access_events AS access
  JOIN straylight.source_asset_links AS link
    ON link.user_id=access.user_id AND link.asset_id=access.target_record_id
  WHERE access.user_id=$1
),
source_events AS (
  SELECT DISTINCT event_id,user_id,projection_receipt_id,session_id,
         operation_kind,occurrence_count,accessed_at,source_id
  FROM mapped_access
),
active_sources AS (
  SELECT source.id,source.source_ref,source.source_kind,source.source_version,
         source.title,source.captured_at,scope.scope_ref
  FROM straylight.active_manifests AS manifest
  JOIN straylight.corpus_members AS member
    ON member.user_id=manifest.user_id
   AND member.scope_id=manifest.scope_id
   AND member.corpus_revision_id=manifest.active_corpus_revision_id
   AND member.record_kind='source_episode'
   AND member.disposition='active'
  JOIN straylight.source_episodes AS source
    ON source.user_id=member.user_id AND source.id=member.record_id
  JOIN straylight.scopes AS scope
    ON scope.user_id=source.user_id AND scope.id=source.scope_id
  WHERE manifest.user_id=$1
    AND ($2::text IS NULL OR scope.scope_ref=$2)
    AND (
      $3::text IS NULL
      OR source.title ILIKE '%'||$3||'%'
      OR source.source_ref ILIKE '%'||$3||'%'
    )
),
source_totals AS (
  SELECT event.source_id,
         count(DISTINCT event.projection_receipt_id)::bigint AS access_count,
         count(*)::bigint AS record_exposure_count,
         sum(event.occurrence_count)::bigint AS reference_occurrence_count,
         count(DISTINCT event.session_id)::bigint AS session_count,
         min(event.accessed_at) AS first_used_at,
         max(event.accessed_at) AS last_used_at,
         count(DISTINCT event.projection_receipt_id)
           FILTER (WHERE event.operation_kind='open')::bigint AS open_count,
         count(DISTINCT event.projection_receipt_id)
           FILTER (WHERE event.operation_kind='query')::bigint AS query_count,
         count(DISTINCT event.projection_receipt_id)
           FILTER (WHERE event.operation_kind='read')::bigint AS read_count,
         count(DISTINCT event.projection_receipt_id)
           FILTER (WHERE event.operation_kind='compute')::bigint AS compute_count,
         count(DISTINCT event.projection_receipt_id)
           FILTER (WHERE event.operation_kind='verify')::bigint AS verify_count
  FROM source_events AS event
  GROUP BY event.source_id
),
usage_values AS (
  SELECT source.id,source.source_ref,source.source_kind,source.source_version,
         source.title,source.captured_at,source.scope_ref,
         coalesce(total.access_count,0)::bigint AS access_count,
         coalesce(total.record_exposure_count,0)::bigint AS record_exposure_count,
         coalesce(total.reference_occurrence_count,0)::bigint
           AS reference_occurrence_count,
         coalesce(total.session_count,0)::bigint AS session_count,
         total.first_used_at,total.last_used_at,
         coalesce(total.open_count,0)::bigint AS open_count,
         coalesce(total.query_count,0)::bigint AS query_count,
         coalesce(total.read_count,0)::bigint AS read_count,
         coalesce(total.compute_count,0)::bigint AS compute_count,
         coalesce(total.verify_count,0)::bigint AS verify_count
  FROM active_sources AS source
  LEFT JOIN source_totals AS total ON total.source_id=source.id
),
usage_rows AS (
  SELECT value.*,
         jsonb_build_object(
           'id','source:'||value.id::text,
           'source_ref',value.source_ref,
           'title',coalesce(value.title,'Untitled source'),
           'kind',value.source_kind,
           'version',value.source_version,
           'scope_id',value.scope_ref,
           'captured_at',value.captured_at,
           'access_count',value.access_count,
           'record_exposure_count',value.record_exposure_count,
           'reference_occurrence_count',value.reference_occurrence_count,
           'session_count',value.session_count,
           'first_used_at',value.first_used_at,
           'last_used_at',value.last_used_at,
           'never_used',value.access_count=0,
           'operation_counts',jsonb_build_object(
             'open',value.open_count,
             'query',value.query_count,
             'read',value.read_count,
             'compute',value.compute_count,
             'verify',value.verify_count
           )
         ) AS item
  FROM usage_values AS value
),
operation_values AS (
  SELECT event.operation_kind,
         count(DISTINCT (event.source_id,event.projection_receipt_id))::bigint
           AS source_uses,
         count(*)::bigint AS record_exposures,
         sum(event.occurrence_count)::bigint AS reference_occurrences,
         count(DISTINCT event.projection_receipt_id)::bigint AS response_count
  FROM source_events AS event
  JOIN active_sources AS source ON source.id=event.source_id
  GROUP BY event.operation_kind
)
SELECT jsonb_build_object(
  'summary',jsonb_build_object(
    'active_source_count',(SELECT count(*) FROM usage_values),
    'used_source_count',(
      SELECT count(*) FROM usage_values WHERE access_count > 0
    ),
    'never_used_source_count',(
      SELECT count(*) FROM usage_values WHERE access_count = 0
    ),
    'source_uses',(
      SELECT coalesce(sum(access_count),0) FROM usage_values
    ),
    'record_exposures',(
      SELECT coalesce(sum(record_exposure_count),0) FROM usage_values
    ),
    'reference_occurrences',(
      SELECT coalesce(sum(reference_occurrence_count),0) FROM usage_values
    ),
    'tracked_response_count',(
      SELECT count(DISTINCT event.projection_receipt_id)
      FROM source_events AS event
      JOIN active_sources AS source ON source.id=event.source_id
    ),
    'tracked_since',(SELECT min(first_used_at) FROM usage_values),
    'last_used_at',(SELECT max(last_used_at) FROM usage_values)
  ),
  'most_used',coalesce((
    SELECT jsonb_agg(ranked.item ORDER BY ranked.access_count DESC,
                     ranked.last_used_at DESC NULLS LAST,ranked.title,ranked.id)
    FROM (
      SELECT * FROM usage_rows
      ORDER BY access_count DESC,last_used_at DESC NULLS LAST,title,id
      LIMIT $4
    ) AS ranked
  ),'[]'::jsonb),
  'least_used',coalesce((
    SELECT jsonb_agg(ranked.item ORDER BY ranked.access_count,
                     ranked.last_used_at ASC NULLS FIRST,ranked.title,ranked.id)
    FROM (
      SELECT * FROM usage_rows
      ORDER BY access_count,last_used_at ASC NULLS FIRST,title,id
      LIMIT $4
    ) AS ranked
  ),'[]'::jsonb),
  'least_recently_used',coalesce((
    SELECT jsonb_agg(ranked.item ORDER BY ranked.last_used_at ASC NULLS FIRST,
                     ranked.access_count,ranked.title,ranked.id)
    FROM (
      SELECT * FROM usage_rows
      ORDER BY last_used_at ASC NULLS FIRST,access_count,title,id
      LIMIT $4
    ) AS ranked
  ),'[]'::jsonb),
  'by_operation',coalesce((
    SELECT jsonb_agg(
      jsonb_build_object(
        'operation',operation.operation_kind,
        'source_uses',operation.source_uses,
        'record_exposures',operation.record_exposures,
        'reference_occurrences',operation.reference_occurrences,
        'response_count',operation.response_count
      )
      ORDER BY operation.operation_kind
    )
    FROM operation_values AS operation
  ),'[]'::jsonb)
) AS usage
"#;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn visible_accesses_include_array_references_and_deduplicate_records() {
        let chunk_id = Uuid::now_v7();
        let evidence_id = Uuid::now_v7();
        let accesses = collect_visible_record_accesses(
            "query",
            &json!({
                "items": [{
                    "reference": format!("chunk:{chunk_id}"),
                    "selected_references": [
                        format!("chunk:{chunk_id}"),
                        format!("evidence:{evidence_id}")
                    ],
                    "evidence_refs": [format!("evidence:{evidence_id}")]
                }]
            }),
        );

        assert_eq!(
            accesses,
            vec![
                VisibleRecordAccess {
                    record_id: chunk_id,
                    occurrence_count: 2,
                },
                VisibleRecordAccess {
                    record_id: evidence_id,
                    occurrence_count: 2,
                },
            ]
        );
    }

    #[test]
    fn failed_exact_reads_do_not_count_the_requested_record_as_used() {
        let requested = Uuid::now_v7();
        let alternative = Uuid::now_v7();
        let accesses = collect_visible_record_accesses(
            "read",
            &json!({
                "items": [{
                    "reference": format!("object:{requested}"),
                    "view": "structured",
                    "status": "failed",
                    "data": null,
                    "error": {
                        "details": {
                            "candidate": format!("object:{alternative}")
                        }
                    }
                }]
            }),
        );

        assert_eq!(
            accesses,
            vec![VisibleRecordAccess {
                record_id: alternative,
                occurrence_count: 1,
            }]
        );
    }

    #[test]
    fn only_agent_read_operations_are_tracked() {
        assert_eq!(tracked_operation_kind("memory.open"), Some("open"));
        assert_eq!(tracked_operation_kind("memory.stage"), None);
        assert_eq!(tracked_operation_kind("memory.save"), None);
    }

    #[test]
    fn open_inventory_samples_are_not_counted_as_reasoning_use() {
        let inventory = Uuid::now_v7();
        let evidence = Uuid::now_v7();
        let accesses = collect_visible_record_accesses(
            "open",
            &json!({
                "corpus_map": {
                    "sample": {"ref": format!("source:{inventory}")}
                },
                "initial_evidence": [{
                    "reference": format!("chunk:{evidence}")
                }],
                "projection": {
                    "source_corpus_revision": format!("revision:{}", Uuid::now_v7())
                }
            }),
        );

        assert_eq!(
            accesses,
            vec![VisibleRecordAccess {
                record_id: evidence,
                occurrence_count: 1,
            }]
        );
    }
}

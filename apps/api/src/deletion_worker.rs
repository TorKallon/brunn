//! Per-record deletion propagation restored from the pre-simplify worker
//! (checkpoint 97f6f64). Tombstone saves enqueue `deletion_jobs` plus nine
//! `derivative_cleanup_targets` surfaces; this executor claims those jobs,
//! plans the affected legacy-model records, cleans each surface with a
//! per-surface receipt, and finalizes the job so account deletion can
//! observe completion.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

use crate::{
    db::AppState,
    error::{ApiError, ApiResult},
};

const REQUIRED_DELETION_SURFACES: [&str; 9] = [
    "source",
    "chunk",
    "lexical_index",
    "embedding",
    "cache",
    "asset",
    "derived_view",
    "export",
    "replica",
];

#[derive(Clone, Debug)]
struct DeletionJob {
    id: Uuid,
    user_id: Uuid,
    scope_id: Uuid,
    tombstone_id: Uuid,
    target_id: Uuid,
    target_kind: String,
    authorization_source_id: Uuid,
    authorization_evidence_id: Uuid,
}

#[derive(Clone, Debug)]
struct AssetBlob {
    asset_id: Uuid,
    version: i32,
    bucket: String,
    object_key: String,
}

#[derive(Debug)]
struct DeletionPlan {
    object_ids: Vec<Uuid>,
    claim_ids: Vec<Uuid>,
    relation_ids: Vec<Uuid>,
    document_ids: Vec<Uuid>,
    chunk_ids: Vec<Uuid>,
    checkpoint_ids: Vec<Uuid>,
    stage_ids: Vec<Uuid>,
    staged_entry_ids: Vec<Uuid>,
    redacted_source_ids: Vec<Uuid>,
    retained_source_ids: Vec<Uuid>,
    redacted_evidence_ids: Vec<Uuid>,
    retained_evidence_ids: Vec<Uuid>,
    removable_assets: Vec<AssetBlob>,
    retained_assets: Vec<AssetBlob>,
    affected_record_ids: Vec<Uuid>,
    embedding_target_ids: Vec<Uuid>,
    dream_job_ids: Vec<Uuid>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceStatus {
    Removed,
    RetainedByPolicy,
}

impl SurfaceStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::RetainedByPolicy => "retained_by_policy",
        }
    }
}

#[derive(Debug)]
struct SurfaceOutcome {
    status: SurfaceStatus,
    details: Value,
}

impl SurfaceOutcome {
    fn removed(details: Value) -> Self {
        Self {
            status: SurfaceStatus::Removed,
            details,
        }
    }

    fn retained(details: Value) -> Self {
        Self {
            status: SurfaceStatus::RetainedByPolicy,
            details,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeletionDisposition {
    Complete,
    InProgress,
    RetryRequired,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CleanupSummary {
    pending: usize,
    running: usize,
    removed: usize,
    retained_by_policy: usize,
    failed: usize,
}

impl CleanupSummary {
    fn total(&self) -> usize {
        self.pending + self.running + self.removed + self.retained_by_policy + self.failed
    }

    fn disposition(&self) -> DeletionDisposition {
        if self.failed > 0 {
            DeletionDisposition::RetryRequired
        } else if self.pending > 0 || self.running > 0 {
            DeletionDisposition::InProgress
        } else {
            DeletionDisposition::Complete
        }
    }
}

fn summarize_cleanup_statuses<'a>(statuses: impl IntoIterator<Item = &'a str>) -> CleanupSummary {
    let mut summary = CleanupSummary::default();
    for status in statuses {
        match status {
            "pending" => summary.pending += 1,
            "running" => summary.running += 1,
            "removed" => summary.removed += 1,
            "retained_by_policy" => summary.retained_by_policy += 1,
            "failed" => summary.failed += 1,
            _ => summary.failed += 1,
        }
    }
    summary
}

pub async fn process_deletion_job(state: &AppState) -> ApiResult<bool> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let Some(job) = claim_deletion_job(pool).await? else {
        return Ok(false);
    };

    let plan = match DeletionPlan::load(pool, &job).await {
        Ok(plan) => plan,
        Err(error) => {
            record_planning_failure(pool, &job, &error).await?;
            return Ok(true);
        }
    };

    for surface in REQUIRED_DELETION_SURFACES {
        if !begin_cleanup_surface(pool, &job, surface).await? {
            continue;
        }
        let result = process_cleanup_surface(state, &job, &plan, surface).await;
        match result {
            Ok(outcome) => complete_cleanup_surface(pool, &job, surface, outcome).await?,
            Err(error) => fail_cleanup_surface(pool, &job, surface, &error).await?,
        }
    }

    finalize_deletion_job(pool, &job, &plan).await?;
    Ok(true)
}

async fn claim_deletion_job(pool: &PgPool) -> ApiResult<Option<DeletionJob>> {
    let mut transaction = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT
          job.id,job.user_id,job.scope_id,job.tombstone_id,
          tombstone.target_record_id,tombstone.source_episode_id,
          tombstone.evidence_id,record_key.record_kind
        FROM straylight.deletion_jobs AS job
        JOIN straylight.tombstones AS tombstone
          ON tombstone.user_id=job.user_id AND tombstone.id=job.tombstone_id
        JOIN straylight.record_keys AS record_key
          ON record_key.user_id=job.user_id
         AND record_key.record_id=tombstone.target_record_id
        WHERE
          job.status='queued'
          OR (
            job.status='failed'
            AND coalesce((
              SELECT max(event.recorded_at)
              FROM straylight.deletion_job_events AS event
              WHERE event.user_id=job.user_id AND event.deletion_job_id=job.id
            ),job.completed_at,job.created_at) <= clock_timestamp()-interval '5 seconds'
          )
          OR (
            job.status='propagating'
            AND coalesce((
              SELECT max(event.recorded_at)
              FROM straylight.deletion_job_events AS event
              WHERE event.user_id=job.user_id AND event.deletion_job_id=job.id
            ),job.started_at,job.created_at) <= clock_timestamp()-interval '5 minutes'
          )
          OR (
            job.status='completed'
            AND EXISTS (
              SELECT 1
              FROM straylight.derivative_cleanup_targets AS cleanup
              WHERE cleanup.user_id=job.user_id
                AND cleanup.deletion_job_id=job.id
                AND cleanup.details->>'mode'='removed_from_active_surfaces'
            )
          )
        ORDER BY job.created_at
        FOR UPDATE OF job SKIP LOCKED
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Ok(None);
    };

    let job = DeletionJob {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        scope_id: row.try_get("scope_id")?,
        tombstone_id: row.try_get("tombstone_id")?,
        target_id: row.try_get("target_record_id")?,
        target_kind: row.try_get("record_kind")?,
        authorization_source_id: row.try_get("source_episode_id")?,
        authorization_evidence_id: row.try_get("evidence_id")?,
    };
    let target_ref = canonical_target_ref(&job.target_kind, job.target_id);

    sqlx::query(
        r#"
        INSERT INTO straylight.derivative_cleanup_targets (
          id,user_id,scope_id,deletion_job_id,target_kind,target_ref,status
        )
        SELECT gen_random_uuid(),$1,$2,$3,required.kind,$4,'pending'
        FROM unnest($5::text[]) AS required(kind)
        WHERE NOT EXISTS (
          SELECT 1
          FROM straylight.derivative_cleanup_targets AS existing
          WHERE existing.user_id=$1
            AND existing.deletion_job_id=$3
            AND existing.target_kind=required.kind
        )
        "#,
    )
    .bind(job.user_id)
    .bind(job.scope_id)
    .bind(job.id)
    .bind(&target_ref)
    .bind(REQUIRED_DELETION_SURFACES.as_slice())
    .execute(&mut *transaction)
    .await?;

    let legacy_rows = sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status='pending',
            details=jsonb_build_object(
              'reason','legacy_blanket_receipt_requires_revalidation'
            ),
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND deletion_job_id=$2
          AND status='removed'
          AND details->>'mode'='removed_from_active_surfaces'
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status='pending',
            details=jsonb_build_object('reason','stale_running_target_recovered'),
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND deletion_job_id=$2
          AND status='running'
          AND updated_at <= clock_timestamp()-interval '5 minutes'
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
        UPDATE straylight.deletion_jobs
        SET status='propagating',started_at=coalesce(started_at,clock_timestamp()),
            completed_at=NULL,terminal_result=NULL
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *transaction)
    .await?;
    insert_deletion_event(
        &mut transaction,
        &job,
        "attempt_started",
        json!({
            "target_record_id": job.target_id,
            "target_kind": job.target_kind,
            "legacy_targets_reopened": legacy_rows
        }),
    )
    .await?;
    transaction.commit().await?;
    Ok(Some(job))
}

fn canonical_target_ref(target_kind: &str, target_id: Uuid) -> String {
    let prefix = match target_kind {
        "source_episode" => "source",
        "temporal_spec" => "temporal",
        "recurrence_spec" => "recurrence",
        "corpus_revision" => "revision",
        "dream_job" => "dream",
        other => other,
    };
    format!("{prefix}:{target_id}")
}

async fn insert_deletion_event(
    transaction: &mut Transaction<'_, Postgres>,
    job: &DeletionJob,
    event_kind: &str,
    details: Value,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO straylight.deletion_job_events (
          id,user_id,scope_id,deletion_job_id,event_kind,details
        ) VALUES ($1,$2,$3,$4,$5,$6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job.user_id)
    .bind(job.scope_id)
    .bind(job.id)
    .bind(event_kind)
    .bind(details)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn set_deletion_context(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> ApiResult<()> {
    sqlx::query("SELECT set_config('straylight.deletion_job_id',$1,true)")
        .bind(job_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn begin_cleanup_surface(pool: &PgPool, job: &DeletionJob, surface: &str) -> ApiResult<bool> {
    let result = sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status='running',attempt_count=attempt_count+1,
            details=jsonb_build_object('attempt_started_at',clock_timestamp()),
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND deletion_job_id=$2 AND target_kind=$3
          AND (
            status IN ('pending','failed')
            OR (status='running' AND updated_at <= clock_timestamp()-interval '5 minutes')
          )
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .bind(surface)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

async fn complete_cleanup_surface(
    pool: &PgPool,
    job: &DeletionJob,
    surface: &str,
    outcome: SurfaceOutcome,
) -> ApiResult<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status=$1,details=$2,updated_at=clock_timestamp()
        WHERE user_id=$3 AND deletion_job_id=$4 AND target_kind=$5
          AND status='running'
        "#,
    )
    .bind(outcome.status.as_str())
    .bind(&outcome.details)
    .bind(job.user_id)
    .bind(job.id)
    .bind(surface)
    .execute(&mut *transaction)
    .await?;
    insert_deletion_event(
        &mut transaction,
        job,
        match outcome.status {
            SurfaceStatus::Removed => "target_removed",
            SurfaceStatus::RetainedByPolicy => "target_retained_by_policy",
        },
        json!({"target_kind": surface, "result": outcome.details}),
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn fail_cleanup_surface(
    pool: &PgPool,
    job: &DeletionJob,
    surface: &str,
    error: &ApiError,
) -> ApiResult<()> {
    let details = json!({
        "reason": "surface_cleanup_failed",
        "error": error.to_string()
    });
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status='failed',details=$1,updated_at=clock_timestamp()
        WHERE user_id=$2 AND deletion_job_id=$3 AND target_kind=$4
          AND status='running'
        "#,
    )
    .bind(&details)
    .bind(job.user_id)
    .bind(job.id)
    .bind(surface)
    .execute(&mut *transaction)
    .await?;
    insert_deletion_event(
        &mut transaction,
        job,
        "target_failed",
        json!({"target_kind": surface, "failure": details}),
    )
    .await?;
    transaction.commit().await?;
    tracing::warn!(job_id=%job.id, target_kind=surface, ?error, "deletion surface failed");
    Ok(())
}

async fn record_planning_failure(
    pool: &PgPool,
    job: &DeletionJob,
    error: &ApiError,
) -> ApiResult<()> {
    let details = json!({
        "reason": "deletion_plan_failed",
        "error": error.to_string(),
        "target_kind": job.target_kind
    });
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status='failed',attempt_count=attempt_count+1,details=$1,
            updated_at=clock_timestamp()
        WHERE user_id=$2 AND deletion_job_id=$3 AND target_kind='source'
          AND status IN ('pending','running','failed')
        "#,
    )
    .bind(&details)
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE straylight.deletion_jobs
        SET status='failed',completed_at=clock_timestamp(),terminal_result=$1
        WHERE user_id=$2 AND id=$3
        "#,
    )
    .bind(&details)
    .bind(job.user_id)
    .bind(job.id)
    .execute(&mut *transaction)
    .await?;
    insert_deletion_event(&mut transaction, job, "planning_failed", details).await?;
    transaction.commit().await?;
    tracing::warn!(job_id=%job.id, ?error, "deletion planning failed");
    Ok(())
}

impl DeletionPlan {
    async fn load(pool: &PgPool, job: &DeletionJob) -> ApiResult<Self> {
        let mut object_ids = BTreeSet::new();
        let mut claim_ids = BTreeSet::new();
        let mut relation_ids = BTreeSet::new();
        let mut document_ids = BTreeSet::new();
        let mut chunk_ids = BTreeSet::new();
        let mut checkpoint_ids = BTreeSet::new();
        let mut stage_ids = BTreeSet::new();
        let direct_asset_id = match job.target_kind.as_str() {
            "object" => {
                object_ids.insert(job.target_id);
                None
            }
            "claim" => {
                claim_ids.insert(job.target_id);
                None
            }
            "relation" => {
                relation_ids.insert(job.target_id);
                None
            }
            "source_episode" | "evidence" | "document" | "chunk" | "tombstone" | "dream_job" => {
                None
            }
            "asset" => Some(job.target_id),
            "checkpoint" => {
                checkpoint_ids.insert(job.target_id);
                None
            }
            "stage" => {
                stage_ids.insert(job.target_id);
                None
            }
            unsupported => {
                return Err(ApiError::Internal(format!(
                    "deletion target kind is not safely redacted: {unsupported}"
                )));
            }
        };
        if job.target_kind == "document" {
            document_ids.insert(job.target_id);
        }
        if job.target_kind == "chunk" {
            chunk_ids.insert(job.target_id);
        }

        if job.target_kind == "source_episode" {
            let rows = sqlx::query(
                r#"
                SELECT 'object'::text AS record_kind,object_id AS id
                FROM straylight.object_revisions
                WHERE user_id=$1 AND source_episode_id=$2
                UNION
                SELECT 'claim',id FROM straylight.claims
                WHERE user_id=$1 AND source_episode_id=$2
                UNION
                SELECT 'relation',relation_id FROM straylight.relation_revisions
                WHERE user_id=$1 AND source_episode_id=$2
                UNION
                SELECT 'document',document_id FROM straylight.document_revisions
                WHERE user_id=$1 AND source_episode_id=$2
                "#,
            )
            .bind(job.user_id)
            .bind(job.target_id)
            .fetch_all(pool)
            .await?;
            extend_typed_record_sets(
                rows,
                &mut object_ids,
                &mut claim_ids,
                &mut relation_ids,
                &mut document_ids,
            )?;
        }

        if job.target_kind == "evidence" {
            extend_uuid_set(
                &mut claim_ids,
                sqlx::query(
                    r#"
                    SELECT claim_id AS id FROM straylight.claim_evidence_links
                    WHERE user_id=$1 AND evidence_id=$2
                    UNION
                    SELECT claim_id FROM straylight.claim_lineage
                    WHERE user_id=$1 AND evidence_id=$2
                    "#,
                )
                .bind(job.user_id)
                .bind(job.target_id)
                .fetch_all(pool)
                .await?,
            )?;
            extend_uuid_set(
                &mut relation_ids,
                sqlx::query(
                    r#"
                    SELECT relation_id AS id FROM straylight.relation_evidence_links
                    WHERE user_id=$1 AND evidence_id=$2
                    "#,
                )
                .bind(job.user_id)
                .bind(job.target_id)
                .fetch_all(pool)
                .await?,
            )?;
        }

        let mut dependency_roots = BTreeSet::from([job.target_id]);
        dependency_roots.extend(object_ids.iter().copied());
        dependency_roots.extend(relation_ids.iter().copied());
        let dependency_roots = sorted_ids(&dependency_roots);
        extend_uuid_set(
            &mut claim_ids,
            sqlx::query(
                r#"
                SELECT claim_id AS id
                FROM straylight.claim_about_targets
                WHERE user_id=$1 AND target_record_id=ANY($2)
                UNION
                SELECT id
                FROM straylight.claims
                WHERE user_id=$1 AND producer_ref=ANY($2)
                UNION
                SELECT claim_id
                FROM straylight.state_assignments
                WHERE user_id=$1 AND target_record_id=ANY($2)
                "#,
            )
            .bind(job.user_id)
            .bind(&dependency_roots)
            .fetch_all(pool)
            .await?,
        )?;
        extend_uuid_set(
            &mut relation_ids,
            sqlx::query(
                r#"
                SELECT DISTINCT relation_id AS id
                FROM straylight.relation_endpoints
                WHERE user_id=$1 AND target_record_id=ANY($2)
                "#,
            )
            .bind(job.user_id)
            .bind(&dependency_roots)
            .fetch_all(pool)
            .await?,
        )?;

        let seed_claim_ids = sorted_ids(&claim_ids);
        extend_uuid_set(
            &mut claim_ids,
            sqlx::query(
                r#"
                WITH RECURSIVE affected(id) AS (
                  SELECT unnest($2::uuid[])
                  UNION
                  SELECT lineage.claim_id
                  FROM straylight.claim_lineage AS lineage
                  JOIN affected ON affected.id=lineage.predecessor_claim_id
                  WHERE lineage.user_id=$1
                )
                SELECT id FROM affected
                "#,
            )
            .bind(job.user_id)
            .bind(&seed_claim_ids)
            .fetch_all(pool)
            .await?,
        )?;

        if !object_ids.is_empty() {
            extend_uuid_set(
                &mut document_ids,
                sqlx::query(
                    "SELECT id FROM straylight.documents WHERE user_id=$1 AND object_id=ANY($2)",
                )
                .bind(job.user_id)
                .bind(sorted_ids(&object_ids))
                .fetch_all(pool)
                .await?,
            )?;
        }
        if !document_ids.is_empty() {
            extend_uuid_set(
                &mut chunk_ids,
                sqlx::query(
                    "SELECT id FROM straylight.chunks WHERE user_id=$1 AND document_id=ANY($2)",
                )
                .bind(job.user_id)
                .bind(sorted_ids(&document_ids))
                .fetch_all(pool)
                .await?,
            )?;
        }

        let object_ids_vec = sorted_ids(&object_ids);
        let claim_ids_vec = sorted_ids(&claim_ids);
        let relation_ids_vec = sorted_ids(&relation_ids);
        let document_ids_vec = sorted_ids(&document_ids);
        let checkpoint_ids_vec = sorted_ids(&checkpoint_ids);

        let mut evidence_candidates = BTreeSet::from([job.authorization_evidence_id]);
        if job.target_kind == "evidence" {
            evidence_candidates.insert(job.target_id);
        }
        extend_uuid_set(
            &mut evidence_candidates,
            sqlx::query(
                r#"
                SELECT evidence_id AS id FROM straylight.claim_evidence_links
                WHERE user_id=$1 AND claim_id=ANY($2)
                UNION
                SELECT evidence_id FROM straylight.claim_lineage
                WHERE user_id=$1
                  AND evidence_id IS NOT NULL
                  AND (claim_id=ANY($2) OR predecessor_claim_id=ANY($2))
                UNION
                SELECT evidence_id FROM straylight.relation_evidence_links
                WHERE user_id=$1 AND relation_id=ANY($3)
                UNION
                SELECT evidence_id FROM straylight.object_handles
                WHERE user_id=$1 AND object_id=ANY($4)
                UNION
                SELECT evidence_id FROM straylight.external_identifiers
                WHERE user_id=$1 AND object_id=ANY($4)
                UNION
                SELECT evidence_id FROM straylight.event_occurrence_revisions
                WHERE user_id=$1 AND occurrence_object_id=ANY($4)
                UNION
                SELECT evidence_id FROM straylight.checkpoint_gate_evidence
                WHERE user_id=$1 AND checkpoint_id=ANY($5)
                "#,
            )
            .bind(job.user_id)
            .bind(&claim_ids_vec)
            .bind(&relation_ids_vec)
            .bind(&object_ids_vec)
            .bind(&checkpoint_ids_vec)
            .fetch_all(pool)
            .await?,
        )?;

        let mut source_candidates = BTreeSet::from([job.authorization_source_id]);
        if job.target_kind == "source_episode" {
            source_candidates.insert(job.target_id);
        }
        extend_uuid_set(
            &mut source_candidates,
            sqlx::query(
                r#"
                SELECT source_episode_id AS id FROM straylight.object_revisions
                WHERE user_id=$1 AND object_id=ANY($2)
                UNION
                SELECT source_episode_id FROM straylight.claims
                WHERE user_id=$1 AND id=ANY($3)
                UNION
                SELECT source_episode_id FROM straylight.relation_revisions
                WHERE user_id=$1 AND relation_id=ANY($4)
                UNION
                SELECT source_episode_id FROM straylight.document_revisions
                WHERE user_id=$1 AND document_id=ANY($5)
                UNION
                SELECT source_episode_id FROM straylight.external_identifiers
                WHERE user_id=$1 AND object_id=ANY($2)
                UNION
                SELECT source_episode_id FROM straylight.evidence_items
                WHERE user_id=$1 AND id=ANY($6)
                "#,
            )
            .bind(job.user_id)
            .bind(&object_ids_vec)
            .bind(&claim_ids_vec)
            .bind(&relation_ids_vec)
            .bind(&document_ids_vec)
            .bind(sorted_ids(&evidence_candidates))
            .fetch_all(pool)
            .await?,
        )?;

        let evidence_candidates_vec = sorted_ids(&evidence_candidates);
        let mut redacted_sources = BTreeSet::new();
        let mut retained_sources = BTreeSet::new();
        for source_id in &source_candidates {
            let force_removal = job.target_kind == "source_episode" && *source_id == job.target_id;
            let shared = if force_removal {
                false
            } else {
                sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT
                      EXISTS (
                        SELECT 1 FROM straylight.object_revisions
                        WHERE user_id=$1 AND source_episode_id=$2
                          AND NOT (object_id=ANY($3))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.claims
                        WHERE user_id=$1 AND source_episode_id=$2
                          AND NOT (id=ANY($4))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.relation_revisions
                        WHERE user_id=$1 AND source_episode_id=$2
                          AND NOT (relation_id=ANY($5))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.document_revisions
                        WHERE user_id=$1 AND source_episode_id=$2
                          AND NOT (document_id=ANY($6))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.external_identifiers
                        WHERE user_id=$1 AND source_episode_id=$2
                          AND NOT (object_id=ANY($3))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.evidence_items
                        WHERE user_id=$1 AND source_episode_id=$2
                          AND NOT (id=ANY($7))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.tombstones
                        WHERE user_id=$1 AND source_episode_id=$2 AND id<>$8
                      )
                    "#,
                )
                .bind(job.user_id)
                .bind(*source_id)
                .bind(&object_ids_vec)
                .bind(&claim_ids_vec)
                .bind(&relation_ids_vec)
                .bind(&document_ids_vec)
                .bind(&evidence_candidates_vec)
                .bind(job.tombstone_id)
                .fetch_one(pool)
                .await?
            };
            if shared {
                retained_sources.insert(*source_id);
            } else {
                redacted_sources.insert(*source_id);
            }
        }

        if !redacted_sources.is_empty() {
            extend_uuid_set(
                &mut evidence_candidates,
                sqlx::query(
                    "SELECT id FROM straylight.evidence_items WHERE user_id=$1 AND source_episode_id=ANY($2)",
                )
                .bind(job.user_id)
                .bind(sorted_ids(&redacted_sources))
                .fetch_all(pool)
                .await?,
            )?;
        }

        let evidence_rows = sqlx::query(
            r#"
            SELECT id,source_episode_id
            FROM straylight.evidence_items
            WHERE user_id=$1 AND id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(sorted_ids(&evidence_candidates))
        .fetch_all(pool)
        .await?;
        let mut evidence_sources = BTreeMap::new();
        for row in evidence_rows {
            evidence_sources.insert(
                row.try_get::<Uuid, _>("id")?,
                row.try_get::<Uuid, _>("source_episode_id")?,
            );
        }

        let mut redacted_evidence = BTreeSet::new();
        let mut retained_evidence = BTreeSet::new();
        for evidence_id in &evidence_candidates {
            let source_id = evidence_sources.get(evidence_id).copied();
            let force_removal = *evidence_id == job.authorization_evidence_id
                || (job.target_kind == "evidence" && *evidence_id == job.target_id)
                || source_id
                    .is_some_and(|id| job.target_kind == "source_episode" && id == job.target_id);
            let shared = if force_removal {
                false
            } else {
                sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT
                      EXISTS (
                        SELECT 1 FROM straylight.claim_evidence_links
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (claim_id=ANY($3))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.claim_lineage
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (claim_id=ANY($3))
                          AND NOT (predecessor_claim_id=ANY($3))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.relation_evidence_links
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (relation_id=ANY($4))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.object_handles
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (object_id=ANY($5))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.external_identifiers
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (object_id=ANY($5))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.event_occurrence_revisions
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (occurrence_object_id=ANY($5))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.recurrence_exclusions
                        WHERE user_id=$1 AND evidence_id=$2
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.recurrence_additions
                        WHERE user_id=$1 AND evidence_id=$2
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.checkpoint_gate_evidence
                        WHERE user_id=$1 AND evidence_id=$2
                          AND NOT (checkpoint_id=ANY($6))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.tombstones
                        WHERE user_id=$1 AND evidence_id=$2 AND id<>$7
                      )
                    "#,
                )
                .bind(job.user_id)
                .bind(*evidence_id)
                .bind(&claim_ids_vec)
                .bind(&relation_ids_vec)
                .bind(&object_ids_vec)
                .bind(&checkpoint_ids_vec)
                .bind(job.tombstone_id)
                .fetch_one(pool)
                .await?
            };
            if shared {
                retained_evidence.insert(*evidence_id);
            } else {
                redacted_evidence.insert(*evidence_id);
            }
        }

        if job.target_kind != "source_episode" {
            for evidence_id in &retained_evidence {
                if let Some(source_id) = evidence_sources.get(evidence_id) {
                    redacted_sources.remove(source_id);
                    retained_sources.insert(*source_id);
                }
            }
        }

        let mut affected_record_ids = BTreeSet::from([job.target_id]);
        affected_record_ids.extend(object_ids.iter().copied());
        affected_record_ids.extend(claim_ids.iter().copied());
        affected_record_ids.extend(relation_ids.iter().copied());
        affected_record_ids.extend(document_ids.iter().copied());
        affected_record_ids.extend(chunk_ids.iter().copied());
        affected_record_ids.extend(checkpoint_ids.iter().copied());
        affected_record_ids.extend(stage_ids.iter().copied());
        affected_record_ids.extend(redacted_sources.iter().copied());
        affected_record_ids.extend(redacted_evidence.iter().copied());

        let mut staged_entry_ids = BTreeSet::new();
        extend_uuid_set(
            &mut staged_entry_ids,
            sqlx::query(
                r#"
                SELECT DISTINCT disposition.staged_entry_id AS id
                FROM straylight.import_entry_dispositions AS disposition
                WHERE disposition.user_id=$1
                  AND disposition.resulting_record_id=ANY($2)
                UNION
                SELECT entry.id
                FROM straylight.staged_entries AS entry
                WHERE entry.user_id=$1 AND entry.stage_id=ANY($3)
                "#,
            )
            .bind(job.user_id)
            .bind(sorted_ids(&affected_record_ids))
            .bind(sorted_ids(&stage_ids))
            .fetch_all(pool)
            .await?,
        )?;
        if !staged_entry_ids.is_empty() {
            extend_uuid_set(
                &mut stage_ids,
                sqlx::query(
                    "SELECT DISTINCT stage_id AS id FROM straylight.staged_entries WHERE user_id=$1 AND id=ANY($2)",
                )
                .bind(job.user_id)
                .bind(sorted_ids(&staged_entry_ids))
                .fetch_all(pool)
                .await?,
            )?;
        }

        let redacted_source_ids = sorted_ids(&redacted_sources);
        let redacted_evidence_ids = sorted_ids(&redacted_evidence);
        let staged_entry_ids_vec = sorted_ids(&staged_entry_ids);
        let asset_rows = sqlx::query(
            r#"
            WITH referenced_assets(asset_id,asset_version) AS (
              SELECT asset_id,asset_version FROM straylight.source_asset_links
              WHERE user_id=$1 AND source_episode_id=ANY($2)
              UNION
              SELECT asset_id,asset_version FROM straylight.evidence_items
              WHERE user_id=$1 AND id=ANY($3) AND asset_id IS NOT NULL
              UNION
              SELECT asset_id,asset_version FROM straylight.document_revisions
              WHERE user_id=$1 AND document_id=ANY($4) AND asset_id IS NOT NULL
              UNION
              SELECT asset_id,asset_version FROM straylight.artifact_asset_links
              WHERE user_id=$1 AND object_id=ANY($5)
              UNION
              SELECT asset_id,asset_version FROM straylight.staged_entries
              WHERE user_id=$1 AND id=ANY($6) AND asset_id IS NOT NULL
            )
            SELECT DISTINCT
              version.asset_id,version.version,version.bucket,version.object_key
            FROM straylight.asset_versions AS version
            LEFT JOIN referenced_assets AS reference
              ON reference.asset_id=version.asset_id
             AND reference.asset_version=version.version
            WHERE version.user_id=$1
              AND (
                reference.asset_id IS NOT NULL
                OR ($7::uuid IS NOT NULL AND version.asset_id=$7)
              )
            ORDER BY version.asset_id,version.version
            "#,
        )
        .bind(job.user_id)
        .bind(&redacted_source_ids)
        .bind(&redacted_evidence_ids)
        .bind(&document_ids_vec)
        .bind(&object_ids_vec)
        .bind(&staged_entry_ids_vec)
        .bind(direct_asset_id)
        .fetch_all(pool)
        .await?;
        let mut removable_assets = Vec::new();
        let mut retained_assets = Vec::new();
        for row in asset_rows {
            let asset = AssetBlob {
                asset_id: row.try_get("asset_id")?,
                version: row.try_get("version")?,
                bucket: row.try_get("bucket")?,
                object_key: row.try_get("object_key")?,
            };
            let force_removal = direct_asset_id == Some(asset.asset_id);
            let shared = if force_removal {
                false
            } else {
                sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT
                      EXISTS (
                        SELECT 1 FROM straylight.source_asset_links
                        WHERE user_id=$1 AND asset_id=$2 AND asset_version=$3
                          AND NOT (source_episode_id=ANY($4))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.evidence_items
                        WHERE user_id=$1 AND asset_id=$2 AND asset_version=$3
                          AND NOT (id=ANY($5))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.document_revisions
                        WHERE user_id=$1 AND asset_id=$2 AND asset_version=$3
                          AND NOT (document_id=ANY($6))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.artifact_asset_links
                        WHERE user_id=$1 AND asset_id=$2 AND asset_version=$3
                          AND NOT (object_id=ANY($7))
                      )
                      OR EXISTS (
                        SELECT 1 FROM straylight.staged_entries
                        WHERE user_id=$1 AND asset_id=$2 AND asset_version=$3
                          AND NOT (id=ANY($8))
                      )
                    "#,
                )
                .bind(job.user_id)
                .bind(asset.asset_id)
                .bind(asset.version)
                .bind(&redacted_source_ids)
                .bind(&redacted_evidence_ids)
                .bind(&document_ids_vec)
                .bind(&object_ids_vec)
                .bind(&staged_entry_ids_vec)
                .fetch_one(pool)
                .await?
            };
            if shared {
                retained_assets.push(asset);
            } else {
                removable_assets.push(asset);
            }
        }

        let retained_asset_ids: BTreeSet<Uuid> =
            retained_assets.iter().map(|asset| asset.asset_id).collect();
        let removable_asset_ids: BTreeSet<Uuid> = removable_assets
            .iter()
            .map(|asset| asset.asset_id)
            .filter(|asset_id| !retained_asset_ids.contains(asset_id))
            .collect();
        affected_record_ids.extend(removable_asset_ids.iter().copied());

        let affected_record_ids_vec = sorted_ids(&affected_record_ids);
        let mut dream_job_ids = BTreeSet::new();
        extend_uuid_set(
            &mut dream_job_ids,
            sqlx::query(
                r#"
                SELECT DISTINCT job.id
                FROM straylight.dream_jobs AS job
                JOIN straylight.dream_region_members AS member
                  ON member.user_id=job.user_id AND member.region_id=job.region_id
                WHERE job.user_id=$1 AND member.record_id=ANY($2)
                "#,
            )
            .bind(job.user_id)
            .bind(&affected_record_ids_vec)
            .fetch_all(pool)
            .await?,
        )?;
        if job.target_kind == "dream_job" {
            dream_job_ids.insert(job.target_id);
        }

        let mut embedding_target_ids = affected_record_ids.clone();
        embedding_target_ids.extend(chunk_ids.iter().copied());

        Ok(Self {
            object_ids: object_ids_vec,
            claim_ids: claim_ids_vec,
            relation_ids: relation_ids_vec,
            document_ids: document_ids_vec,
            chunk_ids: sorted_ids(&chunk_ids),
            checkpoint_ids: checkpoint_ids_vec,
            stage_ids: sorted_ids(&stage_ids),
            staged_entry_ids: staged_entry_ids_vec,
            redacted_source_ids,
            retained_source_ids: sorted_ids(&retained_sources),
            redacted_evidence_ids,
            retained_evidence_ids: sorted_ids(&retained_evidence),
            removable_assets,
            retained_assets,
            affected_record_ids: affected_record_ids_vec,
            embedding_target_ids: sorted_ids(&embedding_target_ids),
            dream_job_ids: sorted_ids(&dream_job_ids),
        })
    }
}

fn extend_typed_record_sets(
    rows: Vec<PgRow>,
    object_ids: &mut BTreeSet<Uuid>,
    claim_ids: &mut BTreeSet<Uuid>,
    relation_ids: &mut BTreeSet<Uuid>,
    document_ids: &mut BTreeSet<Uuid>,
) -> ApiResult<()> {
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        match row.try_get::<String, _>("record_kind")?.as_str() {
            "object" => {
                object_ids.insert(id);
            }
            "claim" => {
                claim_ids.insert(id);
            }
            "relation" => {
                relation_ids.insert(id);
            }
            "document" => {
                document_ids.insert(id);
            }
            _ => {}
        }
    }
    Ok(())
}

fn extend_uuid_set(set: &mut BTreeSet<Uuid>, rows: Vec<PgRow>) -> ApiResult<()> {
    for row in rows {
        set.insert(row.try_get("id")?);
    }
    Ok(())
}

fn sorted_ids(set: &BTreeSet<Uuid>) -> Vec<Uuid> {
    set.iter().copied().collect()
}

async fn process_cleanup_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
    surface: &str,
) -> ApiResult<SurfaceOutcome> {
    match surface {
        "source" => redact_source_surface(state, job, plan).await,
        "chunk" => redact_chunk_surface(state, job, plan).await,
        "lexical_index" => verify_lexical_surface(state, job, plan).await,
        "embedding" => remove_embedding_surface(state, job, plan).await,
        "cache" => remove_cache_surface(state, job).await,
        "asset" => remove_asset_surface(state, job, plan).await,
        "derived_view" => redact_derived_surface(state, job, plan).await,
        "export" | "replica" => verify_unmanaged_surface(state, surface).await,
        unknown => Err(ApiError::Internal(format!(
            "unknown deletion cleanup surface: {unknown}"
        ))),
    }
}

async fn redact_source_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut transaction = pool.begin().await?;
    set_deletion_context(&mut transaction, job.id).await?;
    let mut counts = BTreeMap::new();

    counts.insert(
        "tombstones",
        sqlx::query(
            r#"
            UPDATE straylight.tombstones
            SET reason='authorized deletion; request content redacted'
            WHERE user_id=$1 AND id=$2
            "#,
        )
        .bind(job.user_id)
        .bind(job.tombstone_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "object_revisions",
        sqlx::query(
            r#"
            UPDATE straylight.object_revisions
            SET label=NULL,
                properties=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND object_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.object_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "object_handles",
        sqlx::query(
            r#"
            UPDATE straylight.object_handles
            SET handle_kind='deleted',handle='deleted:' || id::text
            WHERE user_id=$1 AND object_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.object_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "external_identifiers",
        sqlx::query(
            r#"
            UPDATE straylight.external_identifiers
            SET namespace='deleted',identifier_value=id::text,issuer=NULL
            WHERE user_id=$1 AND object_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.object_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "identity_name_claims",
        sqlx::query(
            r#"
            UPDATE straylight.identity_name_claims
            SET name_kind='deleted',name_value='deleted',locale=NULL
            WHERE user_id=$1
              AND (object_id=ANY($2) OR claim_id=ANY($3))
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.object_ids)
        .bind(&plan.claim_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "event_series_revisions",
        sqlx::query(
            r#"
            UPDATE straylight.event_series_revisions
            SET source_uid='deleted:' || object_id::text,scheduling_sequence=NULL
            WHERE user_id=$1 AND object_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.object_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "event_occurrences",
        sqlx::query(
            r#"
            UPDATE straylight.event_occurrences
            SET canonical_recurrence_id='deleted:' || occurrence_object_id::text
            WHERE user_id=$1
              AND (occurrence_object_id=ANY($2) OR series_object_id=ANY($2))
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.object_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "claims",
        sqlx::query(
            r#"
            UPDATE straylight.claims
            SET predicate='deleted.redacted',
                value=jsonb_build_object('redacted_by_deletion_job',$3::text),
                authority='deleted',canonicality='deleted',confidence=NULL,
                valid_temporal_id=NULL,observed_temporal_id=NULL
            WHERE user_id=$1 AND id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.claim_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "relation_revisions",
        sqlx::query(
            r#"
            UPDATE straylight.relation_revisions
            SET predicate='deleted.redacted',schema_version=1,
                qualifiers=jsonb_build_object('redacted_by_deletion_job',$3::text),
                valid_temporal_id=NULL
            WHERE user_id=$1 AND relation_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.relation_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "source_episodes",
        sqlx::query(
            r#"
            UPDATE straylight.source_episodes
            SET source_ref='deleted:' || id::text,source_version=NULL,title=NULL,
                content_hash=NULL,
                native_locator=jsonb_build_object('redacted_by_deletion_job',$3::text),
                metadata=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.redacted_source_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "evidence_items",
        sqlx::query(
            r#"
            UPDATE straylight.evidence_items
            SET locator=jsonb_build_object('redacted_by_deletion_job',$4::text),
                text_content='',content_hash=$3
            WHERE user_id=$1 AND id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.redacted_evidence_ids)
        .bind(EMPTY_SHA256)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "document_revisions",
        sqlx::query(
            r#"
            UPDATE straylight.document_revisions
            SET title=NULL,content_hash=$3,
                native_locator=jsonb_build_object('redacted_by_deletion_job',$4::text),
                byte_size=0
            WHERE user_id=$1 AND document_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.document_ids)
        .bind(EMPTY_SHA256)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "checkpoints",
        sqlx::query(
            "UPDATE straylight.checkpoints SET title=NULL,summary='' WHERE user_id=$1 AND id=ANY($2)",
        )
        .bind(job.user_id)
        .bind(&plan.checkpoint_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "checkpoint_goals",
        sqlx::query(
            "UPDATE straylight.checkpoint_goals SET goal_text='deleted' WHERE user_id=$1 AND checkpoint_id=ANY($2)",
        )
        .bind(job.user_id)
        .bind(&plan.checkpoint_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "checkpoint_gaps",
        sqlx::query(
            "UPDATE straylight.checkpoint_gaps SET gap_kind='deleted',description='deleted' WHERE user_id=$1 AND checkpoint_id=ANY($2)",
        )
        .bind(job.user_id)
        .bind(&plan.checkpoint_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "checkpoint_gates",
        sqlx::query(
            "UPDATE straylight.checkpoint_gates SET description='deleted' WHERE user_id=$1 AND checkpoint_id=ANY($2)",
        )
        .bind(job.user_id)
        .bind(&plan.checkpoint_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "checkpoint_actions",
        sqlx::query(
            "UPDATE straylight.checkpoint_actions SET action_text='deleted',preconditions='[]'::jsonb WHERE user_id=$1 AND checkpoint_id=ANY($2)",
        )
        .bind(job.user_id)
        .bind(&plan.checkpoint_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "checkpoint_source_refs",
        sqlx::query(
            r#"
            UPDATE straylight.checkpoint_source_refs
            SET locator=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND checkpoint_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.checkpoint_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "stages",
        sqlx::query(
            r#"
            UPDATE straylight.stages
            SET stable_import_id=NULL,input_hash=$3,inventory_hash=NULL,status='expired'
            WHERE user_id=$1 AND id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.stage_ids)
        .bind(EMPTY_SHA256)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "staged_entries",
        sqlx::query(
            r#"
            UPDATE straylight.staged_entries
            SET path='.deleted/' || id::text,media_type=NULL,size_bytes=0,
                content_hash=NULL,readability='unreadable',
                inspection=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.staged_entry_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    transaction.commit().await?;

    let details = json!({
        "mode": "content_redacted_minimal_history_retained",
        "target_kind": job.target_kind,
        "row_counts": counts,
        "redacted_source_ids": plan.redacted_source_ids,
        "redacted_evidence_ids": plan.redacted_evidence_ids,
        "retained_source_ids": plan.retained_source_ids,
        "retained_evidence_ids": plan.retained_evidence_ids
    });
    if plan.retained_source_ids.is_empty() && plan.retained_evidence_ids.is_empty() {
        Ok(SurfaceOutcome::removed(details))
    } else {
        Ok(SurfaceOutcome::retained(json!({
            "reason": "shared_with_unaffected_canonical_records",
            "mode": "shared_source_or_evidence_retained_after_target_redaction",
            "row_counts": counts,
            "redacted_source_ids": plan.redacted_source_ids,
            "redacted_evidence_ids": plan.redacted_evidence_ids,
            "retained_source_ids": plan.retained_source_ids,
            "retained_evidence_ids": plan.retained_evidence_ids
        })))
    }
}

async fn redact_chunk_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut transaction = pool.begin().await?;
    set_deletion_context(&mut transaction, job.id).await?;
    let rows = sqlx::query(
        r#"
        UPDATE straylight.chunks
        SET heading_path='{}'::text[],
            locator=jsonb_build_object('redacted_by_deletion_job',$4::text),
            content='',content_hash=$3,token_count=0
        WHERE user_id=$1 AND id=ANY($2)
        "#,
    )
    .bind(job.user_id)
    .bind(&plan.chunk_ids)
    .bind(EMPTY_SHA256)
    .bind(job.id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;
    Ok(SurfaceOutcome::removed(json!({
        "mode": "chunk_content_redacted",
        "chunk_ids": plan.chunk_ids,
        "rows": rows
    })))
}

async fn verify_lexical_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let remaining = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM straylight.chunks
        WHERE user_id=$1 AND id=ANY($2)
          AND (
            content<>'' OR heading_path<>'{}'::text[]
            OR search_vector<>''::tsvector
          )
        "#,
    )
    .bind(job.user_id)
    .bind(&plan.chunk_ids)
    .fetch_one(pool)
    .await?;
    if remaining > 0 {
        return Err(ApiError::Internal(format!(
            "{remaining} affected chunks remain in the lexical index"
        )));
    }
    Ok(SurfaceOutcome::removed(json!({
        "mode": "generated_lexical_index_verified_empty",
        "chunk_ids": plan.chunk_ids,
        "remaining": remaining
    })))
}

async fn remove_embedding_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let chunk_id_text: Vec<String> = plan.chunk_ids.iter().map(Uuid::to_string).collect();
    let canceled_jobs = sqlx::query(
        r#"
        UPDATE straylight.background_jobs AS job
        SET status='canceled',completed_at=clock_timestamp(),
            result=jsonb_build_object(
              'reason','target_deleted','deletion_job_id',$4::text
            ),locked_at=NULL,locked_by=NULL
        WHERE job.user_id=$1 AND job.scope_id=$2
          AND job.job_kind='index_embeddings'
          AND job.status IN ('queued','retry_wait')
          AND EXISTS (
            SELECT 1
            FROM jsonb_array_elements_text(
              CASE
                WHEN jsonb_typeof(job.payload->'chunk_ids')='array'
                  THEN job.payload->'chunk_ids'
                ELSE '[]'::jsonb
              END
            ) AS chunk(value)
            WHERE chunk.value=ANY($3)
          )
        "#,
    )
    .bind(job.user_id)
    .bind(job.scope_id)
    .bind(&chunk_id_text)
    .bind(job.id)
    .execute(pool)
    .await?
    .rows_affected();
    let running_jobs = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM straylight.background_jobs AS job
        WHERE job.user_id=$1 AND job.scope_id=$2
          AND job.job_kind='index_embeddings' AND job.status='running'
          AND EXISTS (
            SELECT 1
            FROM jsonb_array_elements_text(
              CASE
                WHEN jsonb_typeof(job.payload->'chunk_ids')='array'
                  THEN job.payload->'chunk_ids'
                ELSE '[]'::jsonb
              END
            ) AS chunk(value)
            WHERE chunk.value=ANY($3)
          )
        "#,
    )
    .bind(job.user_id)
    .bind(job.scope_id)
    .bind(&chunk_id_text)
    .fetch_one(pool)
    .await?;
    let removed = sqlx::query(
        "DELETE FROM straylight.embeddings WHERE user_id=$1 AND target_record_id=ANY($2)",
    )
    .bind(job.user_id)
    .bind(&plan.embedding_target_ids)
    .execute(pool)
    .await?
    .rows_affected();
    if running_jobs > 0 {
        return Err(ApiError::Internal(format!(
            "{running_jobs} embedding jobs are still running for affected chunks"
        )));
    }
    let remaining = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.embeddings WHERE user_id=$1 AND target_record_id=ANY($2)",
    )
    .bind(job.user_id)
    .bind(&plan.embedding_target_ids)
    .fetch_one(pool)
    .await?;
    if remaining > 0 {
        return Err(ApiError::Internal(format!(
            "{remaining} affected embeddings remain after deletion"
        )));
    }
    Ok(SurfaceOutcome::removed(json!({
        "mode": "embeddings_removed_and_reindex_jobs_canceled",
        "target_record_ids": plan.embedding_target_ids,
        "embeddings_removed": removed,
        "background_jobs_canceled": canceled_jobs,
        "remaining": remaining
    })))
}

async fn remove_cache_surface(state: &AppState, job: &DeletionJob) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let materializations = sqlx::query(
        "DELETE FROM straylight.materialization_cache WHERE user_id=$1 AND scope_id=$2",
    )
    .bind(job.user_id)
    .bind(job.scope_id)
    .execute(pool)
    .await?
    .rows_affected();
    let continuation_tokens =
        sqlx::query("DELETE FROM straylight.continuation_tokens WHERE user_id=$1 AND scope_id=$2")
            .bind(job.user_id)
            .bind(job.scope_id)
            .execute(pool)
            .await?
            .rows_affected();
    let remaining = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT
          (SELECT count(*) FROM straylight.materialization_cache
           WHERE user_id=$1 AND scope_id=$2)
          +
          (SELECT count(*) FROM straylight.continuation_tokens
           WHERE user_id=$1 AND scope_id=$2)
        "#,
    )
    .bind(job.user_id)
    .bind(job.scope_id)
    .fetch_one(pool)
    .await?;
    if remaining > 0 {
        return Err(ApiError::Internal(format!(
            "{remaining} active cache rows remain after deletion"
        )));
    }
    Ok(SurfaceOutcome::removed(json!({
        "mode": "scope_active_caches_invalidated",
        "materializations_removed": materializations,
        "continuation_tokens_removed": continuation_tokens,
        "remaining": remaining
    })))
}

async fn remove_asset_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<SurfaceOutcome> {
    let mut object_deletes = 0usize;
    for asset in &plan.removable_assets {
        if asset.bucket != state.config.s3_bucket {
            return Err(ApiError::Internal(format!(
                "asset {} version {} belongs to unmanaged bucket {}",
                asset.asset_id, asset.version, asset.bucket
            )));
        }
        let redacted_key = redacted_asset_key(job.user_id, asset.asset_id, asset.version);
        if asset.object_key != redacted_key {
            state.object_store.delete_object(&asset.object_key).await?;
            object_deletes += 1;
        }
    }

    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut transaction = pool.begin().await?;
    set_deletion_context(&mut transaction, job.id).await?;
    let mut metadata_redactions = 0u64;
    for asset in &plan.removable_assets {
        let redacted_key = redacted_asset_key(job.user_id, asset.asset_id, asset.version);
        metadata_redactions += sqlx::query(
            r#"
            UPDATE straylight.asset_versions
            SET object_key=$4,content_hash=$5,size_bytes=0,
                media_type='application/x-straylight-deleted',etag=NULL,
                encryption='{}'::jsonb,
                metadata=jsonb_build_object('redacted_by_deletion_job',$6::text)
            WHERE user_id=$1 AND asset_id=$2 AND version=$3
            "#,
        )
        .bind(job.user_id)
        .bind(asset.asset_id)
        .bind(asset.version)
        .bind(redacted_key)
        .bind(EMPTY_SHA256)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    }
    transaction.commit().await?;

    let removable = asset_receipts(&plan.removable_assets);
    let retained = asset_receipts(&plan.retained_assets);
    if plan.retained_assets.is_empty() {
        Ok(SurfaceOutcome::removed(json!({
            "mode": "object_blobs_deleted_and_asset_metadata_redacted",
            "removed_assets": removable,
            "object_deletes": object_deletes,
            "metadata_rows_redacted": metadata_redactions
        })))
    } else {
        Ok(SurfaceOutcome::retained(json!({
            "reason": "asset_version_is_referenced_by_unaffected_records",
            "mode": "exclusive_blobs_deleted_shared_blobs_retained",
            "removed_assets": removable,
            "retained_assets": retained,
            "object_deletes": object_deletes,
            "metadata_rows_redacted": metadata_redactions
        })))
    }
}

fn redacted_asset_key(user_id: Uuid, asset_id: Uuid, version: i32) -> String {
    format!("{user_id}/deleted-assets/{asset_id}/{version}")
}

fn asset_receipts(assets: &[AssetBlob]) -> Vec<Value> {
    assets
        .iter()
        .map(|asset| {
            json!({
                "asset_id": asset.asset_id,
                "version": asset.version,
                "bucket": asset.bucket
            })
        })
        .collect()
}

async fn redact_derived_surface(
    state: &AppState,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let mut transaction = pool.begin().await?;
    set_deletion_context(&mut transaction, job.id).await?;
    let mut counts = BTreeMap::new();

    counts.insert(
        "retrieval_result_items",
        sqlx::query(
            r#"
            UPDATE straylight.retrieval_result_items
            SET lane_scores='{}'::jsonb,why_selected='[]'::jsonb,
                evidence_refs='{}'::uuid[],
                payload=jsonb_build_object('redacted_by_deletion_job',$4::text)
            WHERE user_id=$1
              AND (
                target_record_id=ANY($2)
                OR evidence_refs && $3::uuid[]
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .bind(&plan.redacted_evidence_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_region_members",
        sqlx::query(
            r#"
            UPDATE straylight.dream_region_members
            SET expansion_reason='deleted'
            WHERE user_id=$1 AND record_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_job_events",
        sqlx::query(
            r#"
            UPDATE straylight.dream_job_events
            SET details=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND dream_job_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.dream_job_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_candidate_items",
        sqlx::query(
            r#"
            UPDATE straylight.dream_candidate_items AS item
            SET disposition='keep',requires_review=false,
                source_refs=ARRAY[item.target_record_id]::uuid[],
                proposal=jsonb_build_object('redacted_by_deletion_job',$4::text)
            WHERE item.user_id=$1
              AND (
                item.target_record_id=ANY($2)
                OR item.source_refs && $2::uuid[]
                OR item.candidate_revision_id IN (
                  SELECT candidate.id
                  FROM straylight.dream_candidate_revisions AS candidate
                  WHERE candidate.user_id=$1 AND candidate.dream_job_id=ANY($3)
                )
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .bind(&plan.dream_job_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_gate_results",
        sqlx::query(
            r#"
            UPDATE straylight.dream_gate_results AS gate
            SET evidence_refs='{}'::uuid[],
                details=jsonb_build_object('redacted_by_deletion_job',$4::text)
            WHERE gate.user_id=$1
              AND (
                gate.evidence_refs && $2::uuid[]
                OR gate.candidate_revision_id IN (
                  SELECT candidate.id
                  FROM straylight.dream_candidate_revisions AS candidate
                  WHERE candidate.user_id=$1 AND candidate.dream_job_id=ANY($3)
                )
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.redacted_evidence_ids)
        .bind(&plan.dream_job_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_evaluations",
        sqlx::query(
            r#"
            UPDATE straylight.dream_evaluations AS evaluation
            SET result=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE evaluation.user_id=$1
              AND evaluation.candidate_revision_id IN (
                SELECT candidate.id
                FROM straylight.dream_candidate_revisions AS candidate
                WHERE candidate.user_id=$1 AND candidate.dream_job_id=ANY($2)
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.dream_job_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_reviews",
        sqlx::query(
            r#"
            UPDATE straylight.dream_reviews AS review
            SET reason='deleted'
            WHERE review.user_id=$1
              AND (
                review.target_record_id=ANY($2)
                OR review.candidate_revision_id IN (
                  SELECT candidate.id
                  FROM straylight.dream_candidate_revisions AS candidate
                  WHERE candidate.user_id=$1 AND candidate.dream_job_id=ANY($3)
                )
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .bind(&plan.dream_job_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_model_receipts",
        sqlx::query(
            r#"
            UPDATE straylight.dream_model_receipts
            SET output=jsonb_build_object('redacted_by_deletion_job',$3::text),
                validation=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND dream_job_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.dream_job_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "dream_canary_observations",
        sqlx::query(
            r#"
            UPDATE straylight.dream_canary_observations AS observation
            SET old_selection=jsonb_build_object('redacted_by_deletion_job',$3::text),
                new_selection=jsonb_build_object('redacted_by_deletion_job',$3::text),
                correction=NULL,source_invalidations='{}'::uuid[]
            WHERE observation.user_id=$1
              AND observation.promotion_receipt_id IN (
                SELECT receipt.id
                FROM straylight.dream_promotion_receipts AS receipt
                WHERE receipt.user_id=$1 AND receipt.dream_job_id=ANY($2)
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.dream_job_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "import_receipts",
        sqlx::query(
            r#"
            UPDATE straylight.import_receipts AS receipt
            SET stable_import_id='deleted:' || receipt.id::text,
                input_hash=$4,inventory_hash=$4,manifest_hash=$4,
                indexing_state=jsonb_build_object('redacted_by_deletion_job',$5::text),
                receipt=jsonb_build_object('redacted_by_deletion_job',$5::text)
            WHERE receipt.user_id=$1
              AND (
                receipt.stage_id=ANY($2)
                OR receipt.id IN (
                  SELECT disposition.import_receipt_id
                  FROM straylight.import_entry_dispositions AS disposition
                  WHERE disposition.user_id=$1
                    AND disposition.resulting_record_id=ANY($3)
                )
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.stage_ids)
        .bind(&plan.affected_record_ids)
        .bind(EMPTY_SHA256)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "import_entry_dispositions",
        sqlx::query(
            r#"
            UPDATE straylight.import_entry_dispositions AS disposition
            SET details=jsonb_build_object('redacted_by_deletion_job',$4::text)
            WHERE disposition.user_id=$1
              AND (
                disposition.resulting_record_id=ANY($2)
                OR disposition.import_receipt_id IN (
                  SELECT receipt.id
                  FROM straylight.import_receipts AS receipt
                  WHERE receipt.user_id=$1 AND receipt.stage_id=ANY($3)
                )
              )
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .bind(&plan.stage_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "write_operation_items",
        sqlx::query(
            r#"
            UPDATE straylight.write_operation_items
            SET details=jsonb_build_object('redacted_by_deletion_job',$3::text)
            WHERE user_id=$1 AND target_record_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    counts.insert(
        "audit_events",
        sqlx::query(
            r#"
            UPDATE straylight.audit_events
            SET details=jsonb_build_object('redacted_by_deletion_job',$3::text),
                content_free=true
            WHERE user_id=$1 AND target_record_id=ANY($2)
            "#,
        )
        .bind(job.user_id)
        .bind(&plan.affected_record_ids)
        .bind(job.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected(),
    );
    transaction.commit().await?;

    Ok(SurfaceOutcome::removed(json!({
        "mode": "derived_payloads_redacted_lineage_retained",
        "affected_record_ids": plan.affected_record_ids,
        "dream_job_ids": plan.dream_job_ids,
        "row_counts": counts
    })))
}

async fn verify_unmanaged_surface(state: &AppState, surface: &str) -> ApiResult<SurfaceOutcome> {
    let pool = state.admin_pool.as_ref().expect("checked at worker start");
    let table_names: &[&str] = match surface {
        "export" => &["exports", "export_jobs", "export_artifacts"],
        "replica" => &["replicas", "replica_state", "replica_jobs"],
        _ => &[],
    };
    let registered = sqlx::query_scalar::<_, String>(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema='straylight' AND table_name=ANY($1)
        ORDER BY table_name
        LIMIT 1
        "#,
    )
    .bind(table_names)
    .fetch_optional(pool)
    .await?;
    if let Some(table) = registered {
        return Err(ApiError::Internal(format!(
            "managed {surface} surface {table} exists but has no deletion adapter"
        )));
    }
    Ok(SurfaceOutcome::removed(json!({
        "mode": "managed_surface_verified_absent",
        "surface": surface,
        "checked_tables": table_names
    })))
}

async fn finalize_deletion_job(
    pool: &PgPool,
    job: &DeletionJob,
    plan: &DeletionPlan,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        UPDATE straylight.derivative_cleanup_targets
        SET status='failed',
            details=jsonb_build_object(
              'reason','retained_by_policy_requires_concrete_reason'
            ),
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND deletion_job_id=$2
          AND status='retained_by_policy'
          AND coalesce(nullif(btrim(details->>'reason'),''),'')=''
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .execute(pool)
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT target_kind,target_ref,status,details
        FROM straylight.derivative_cleanup_targets
        WHERE user_id=$1 AND deletion_job_id=$2
        ORDER BY target_kind,target_ref
        "#,
    )
    .bind(job.user_id)
    .bind(job.id)
    .fetch_all(pool)
    .await?;
    let mut statuses = Vec::with_capacity(rows.len());
    let mut present_kinds = BTreeSet::new();
    let mut receipts = Vec::with_capacity(rows.len());
    for row in rows {
        let target_kind: String = row.try_get("target_kind")?;
        let target_ref: String = row.try_get("target_ref")?;
        let status: String = row.try_get("status")?;
        let details: Value = row.try_get("details")?;
        present_kinds.insert(target_kind.clone());
        statuses.push(status.clone());
        receipts.push(json!({
            "target_kind": target_kind,
            "target_ref": target_ref,
            "status": status,
            "details": details
        }));
    }
    let missing_required: Vec<&str> = REQUIRED_DELETION_SURFACES
        .iter()
        .copied()
        .filter(|kind| !present_kinds.contains(*kind))
        .collect();
    let mut summary = summarize_cleanup_statuses(statuses.iter().map(String::as_str));
    summary.failed += missing_required.len();
    let disposition = summary.disposition();
    let status = match disposition {
        DeletionDisposition::Complete => "completed",
        DeletionDisposition::InProgress => "propagating",
        DeletionDisposition::RetryRequired => "failed",
    };
    let result = json!({
        "status": status,
        "target_record_id": job.target_id,
        "target_kind": job.target_kind,
        "minimal_tombstone_id": job.tombstone_id,
        "surface_summary": {
            "total": summary.total(),
            "pending": summary.pending,
            "running": summary.running,
            "removed": summary.removed,
            "retained_by_policy": summary.retained_by_policy,
            "failed": summary.failed
        },
        "missing_required_targets": missing_required,
        "identified": {
            "objects": plan.object_ids.len(),
            "claims": plan.claim_ids.len(),
            "relations": plan.relation_ids.len(),
            "documents": plan.document_ids.len(),
            "chunks": plan.chunk_ids.len(),
            "sources_redacted": plan.redacted_source_ids.len(),
            "sources_retained": plan.retained_source_ids.len(),
            "evidence_redacted": plan.redacted_evidence_ids.len(),
            "evidence_retained": plan.retained_evidence_ids.len(),
            "assets_removed": plan.removable_assets.len(),
            "assets_retained": plan.retained_assets.len()
        },
        "surface_receipts": receipts
    });

    let mut transaction = pool.begin().await?;
    match disposition {
        DeletionDisposition::Complete => {
            sqlx::query(
                r#"
                UPDATE straylight.deletion_jobs
                SET status='completed',completed_at=clock_timestamp(),terminal_result=$1
                WHERE user_id=$2 AND id=$3
                "#,
            )
            .bind(&result)
            .bind(job.user_id)
            .bind(job.id)
            .execute(&mut *transaction)
            .await?;
            insert_deletion_event(&mut transaction, job, "completed", result).await?;
        }
        DeletionDisposition::RetryRequired => {
            sqlx::query(
                r#"
                UPDATE straylight.deletion_jobs
                SET status='failed',completed_at=clock_timestamp(),terminal_result=$1
                WHERE user_id=$2 AND id=$3
                "#,
            )
            .bind(&result)
            .bind(job.user_id)
            .bind(job.id)
            .execute(&mut *transaction)
            .await?;
            insert_deletion_event(&mut transaction, job, "retry_required", result).await?;
        }
        DeletionDisposition::InProgress => {
            sqlx::query(
                r#"
                UPDATE straylight.deletion_jobs
                SET status='propagating',completed_at=NULL,terminal_result=$1
                WHERE user_id=$2 AND id=$3
                "#,
            )
            .bind(&result)
            .bind(job.user_id)
            .bind(job.id)
            .execute(&mut *transaction)
            .await?;
            insert_deletion_event(&mut transaction, job, "targets_in_progress", result).await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod deletion_tests {
    use super::*;

    #[test]
    fn completion_requires_every_target_to_be_terminal() {
        let summary =
            summarize_cleanup_statuses(["removed", "retained_by_policy", "pending", "running"]);
        assert_eq!(summary.disposition(), DeletionDisposition::InProgress);
        assert_eq!(summary.total(), 4);
    }

    #[test]
    fn any_failed_target_requires_retry() {
        let summary =
            summarize_cleanup_statuses(["removed", "retained_by_policy", "failed", "pending"]);
        assert_eq!(summary.disposition(), DeletionDisposition::RetryRequired);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn removed_and_policy_retained_targets_can_complete() {
        let summary = summarize_cleanup_statuses(["removed", "retained_by_policy", "removed"]);
        assert_eq!(summary.disposition(), DeletionDisposition::Complete);
        assert_eq!(summary.retained_by_policy, 1);
    }

    #[test]
    fn unknown_status_never_allows_completion() {
        let summary = summarize_cleanup_statuses(["removed", "future_status"]);
        assert_eq!(summary.disposition(), DeletionDisposition::RetryRequired);
        assert_eq!(summary.failed, 1);
    }
}

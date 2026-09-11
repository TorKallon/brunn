//! Persistent, exact-evidence subject research. Only scheduling pointers live in
//! the global run state; source bodies are never copied into research records.
use super::*;
use crate::dreamer::research::{MAX_RESEARCH_NOTES_BYTES, RepairFeedback, RepairPhase};
use crate::dreamer_subject::{
    SubjectDependency, SubjectScope, check_scope, create_scope, research_change_page,
};
use std::collections::{BTreeMap, BTreeSet};
pub(super) mod checkpoints;
mod discovery_audit;
pub(super) mod drafts;

const MAX_SOURCES: usize = 256;
const MAX_RECEIPTS: usize = 24;
const MAX_JOB_BYTES: usize = 192 * 1024;
const REVALIDATION_LOOKBACK: i64 = 16;
/// Model rounds one pass may consume across interruptions and resumes. An
/// exhausted pass keeps its notes and draft; the next selection starts a new
/// pass at a newer cutoff instead of refilling the old allowance.
pub(crate) const PASS_ROUND_LIMIT: usize = 48;
const MAX_PASS_CHANGES: usize = 64;

/// One bounded research pass. Its evidence cutoff is fixed when the pass
/// starts and survives interruption: discovery rounds, retries and new writes
/// never advance it. Evidence written after the cutoff waits for the next pass.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Pass {
    pub cutoff: i64,
    pub started_at: DateTime<Utc>,
    pub rounds: usize,
    /// Reliance evidence that changed since the last completed review, plus
    /// newly relevant leads. Bounded identities only; never source text.
    #[serde(default)]
    pub changed: Vec<PassChange>,
    /// The model explicitly yielded. The next selection re-pins at a newer
    /// cutoff so queued evidence becomes available, but the round allowance is
    /// carried forward rather than refilled. A crash or timeout without a
    /// yield resumes the same cutoff.
    #[serde(default)]
    pub yielded: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct PassChange {
    pub entry_ref: String,
    pub path: String,
    /// `version` (a reviewed or cited source moved), `new_relevant` (a lead
    /// matching the subject appeared) or `unavailable`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<i64>,
    pub version: i64,
    /// Required changes must be reviewed at `version` before the pass completes.
    pub required: bool,
}

/// The last completed pass: what the retained overview was reviewed through.
/// Foreground reads report this status; they never compute it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Reviewed {
    pub generation: i64,
    pub at: DateTime<Utc>,
    pub disposition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_hash: Option<String>,
    pub dependencies: Vec<SubjectDependency>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RevalidationCheckpoint {
    None,
    Retained { version: i64 },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Scheduler {
    pub active_subject: Option<String>,
    #[serde(default)]
    pub active_attempt: Option<String>,
    pub people_after: String,
    pub project_after: String,
    #[serde(default)]
    pub resume_after: String,
    pub input_after: usize,
    pub lane: usize,
    pub service_sequence: i64,
    pub completed: usize,
    #[serde(default)]
    pub requested_subject_refs: Vec<String>,
    #[serde(default)]
    pub follow_ups: Vec<research_comparison::FollowUp>,
    #[serde(default)]
    pub follow_up_priorities: Vec<String>,
    #[serde(default)]
    pub skipped_subjects: Vec<Value>,
    #[serde(default)]
    pub receipts: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Job {
    pub schema: String,
    pub subject_ref: String,
    pub subject_path: String,
    pub title: String,
    pub output_path: String,
    pub scope: SubjectScope,
    pub snapshot_generation: i64,
    pub sources: Vec<Input>,
    #[serde(serialize_with = "serialize_reviewed_selectors")]
    pub reviewed_sources: Vec<Source>,
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_feedback: Option<RepairFeedback>,
    // Absence is legacy/uninitialized, distinct from an intentionally cleared
    // checkpoint. Only the server may select an immutable notebook version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revalidation_checkpoint: Option<RevalidationCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_versions: Option<Vec<i64>>,
    pub pending_queries: Vec<String>,
    pub pending_targets: Vec<String>,
    pub status: String,
    pub round: usize,
    pub last_served: i64,
    #[serde(default)]
    pub last_attempt: Option<String>,
    pub retry_at: DateTime<Utc>,
    pub accepted_candidate_ids: Vec<String>,
    pub discoveries: Vec<Value>,
    pub receipts: Vec<Value>,
    pub coverage: Value,
    #[serde(default)]
    pub change_scan: Option<ChangeScan>,
    #[serde(default)]
    pub pending_change_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<Pass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_through: Option<Reviewed>,
}

/// Research retains validated selectors, not another copy of source bodies.
/// Apply this projection at serialization so old excerpt-bearing records also
/// become compact when returned or saved. Candidate evidence remains hydrated.
fn serialize_reviewed_selectors<S>(sources: &[Source], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;

    #[derive(Serialize)]
    struct Selector<'a> {
        entry_ref: &'a str,
        version: i64,
        start_line: usize,
        end_line: usize,
        path: &'a str,
    }

    let mut sequence = serializer.serialize_seq(Some(sources.len()))?;
    for source in sources {
        sequence.serialize_element(&Selector {
            entry_ref: &source.entry_ref,
            version: source.version,
            start_line: source.start_line,
            end_line: source.end_line,
            path: &source.path,
        })?;
    }
    sequence.end()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ChangeScan {
    pub cursor: i64,
    pub upper: i64,
}

pub(super) fn path(reference: &str) -> ApiResult<String> {
    Ok(format!("dreams/research/{}.md", entry_id(reference)?))
}

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    reference: &str,
) -> ApiResult<Option<(Job, i64)>> {
    let Some(entry) = load_entry(tx, auth.user_id.0, &path(reference)?).await? else {
        return Ok(None);
    };
    let job: Job =
        serde_json::from_value(entry.metadata["dreamer_research"].clone()).map_err(|_| {
            ApiError::invalid("Retained research is invalid; refusing to discard progress")
        })?;
    if !matches!(
        job.schema.as_str(),
        "dream.research.v1" | "dream.research.v2"
    ) || job.subject_ref != reference
        || (job.schema == "dream.research.v2" && job.checkpoint_versions.is_none())
    {
        return Err(ApiError::invalid("Retained research identity is invalid"));
    }
    Ok(Some((job, entry.version)))
}

pub(super) async fn save(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
) -> ApiResult<i64> {
    let mut job = job.clone();
    discovery_audit::invalidate_changed_authority(&mut job);
    checkpoints::reserve(tx, auth, &job, version + 1).await?;
    let metadata = json!({"kind":"dreamer_research","dreamer_research":job});
    if serde_json::to_vec(&metadata)?.len() > MAX_JOB_BYTES {
        return Err(ApiError::invalid(
            "Research record is full; evidence and progress remain retained",
        ));
    }
    let receipt = put_entry(state, tx, auth, &path(&job.subject_ref)?,
        format!("# {} research\n\nStatus: {}\nRound: {}\nAdmitted sources: {}\nReviewed selectors: {}\n\n{}\n", job.title, job.status, job.round, job.sources.len(), job.reviewed_sources.len(), job.notes),
        metadata, version).await?;
    Ok(receipt["version"]
        .as_i64()
        .expect("written research version"))
}

pub(super) fn clear_revalidation(job: &mut Job) {
    if job.schema == "dream.research.v2" {
        return;
    }
    job.revalidation_checkpoint = Some(RevalidationCheckpoint::None);
}

fn capture_revalidation(job: &mut Job, version: i64) -> ApiResult<()> {
    if version > 0 && !job.notes.trim().is_empty() && !job.reviewed_sources.is_empty() {
        if job.schema == "dream.research.v2" {
            let versions = job
                .checkpoint_versions
                .as_mut()
                .ok_or_else(checkpoints::capacity)?;
            if !versions.contains(&version) {
                versions.push(version);
            }
            if versions.len() > 4 {
                return Err(checkpoints::capacity());
            }
        } else {
            job.revalidation_checkpoint = Some(RevalidationCheckpoint::Retained { version });
        }
    }
    Ok(())
}

async fn initialize_revalidation(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &mut Job,
    version: i64,
) -> ApiResult<()> {
    if job.revalidation_checkpoint.is_some() {
        return Ok(());
    }
    clear_revalidation(job);
    if !job.notes.trim().is_empty() && !job.reviewed_sources.is_empty() {
        return Ok(());
    }
    // The index bounds versions before inspecting their metadata. Empty heads
    // cannot cause an unbounded search for an older nonempty notebook.
    let rows = sqlx::query(
        "SELECT v.version,v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id WHERE e.user_id=$1 AND e.path=$2 AND e.deleted_at IS NULL AND v.version<$3 ORDER BY v.version DESC LIMIT $4",
    )
    .bind(auth.user_id.0)
    .bind(path(&job.subject_ref)?)
    .bind(version)
    .bind(REVALIDATION_LOOKBACK)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let metadata: Value = row.get("metadata");
        if crate::dreamer_summary::research_revalidation_candidate(&metadata, &job.subject_ref) {
            // Retain the newest candidate once. Its complete access/contract
            // audit happens at projection; do not search around a failed audit.
            job.revalidation_checkpoint = Some(RevalidationCheckpoint::Retained {
                version: row.get("version"),
            });
            break;
        }
    }
    Ok(())
}

fn explicit_revalidation_replacement(job: &Job, body: &Value) -> bool {
    !job.notes.trim().is_empty()
        && !job.reviewed_sources.is_empty()
        && body["notes"]
            .as_str()
            .is_some_and(|notes| !notes.trim().is_empty())
        && body["reviewed_sources"]
            .as_array()
            .is_some_and(|sources| !sources.is_empty())
}

/// CAS may be refreshed after an owner decision. Every semantic field remains
/// payload-bound, including attempt/fence, subject and research version.
pub(super) fn operation(body: &Value, kind: &str) -> ApiResult<(String, String)> {
    let id = string(body, "operation_id")?;
    Uuid::parse_str(id).map_err(|_| ApiError::invalid("operation_id must be a UUID"))?;
    let mut payload = body.clone();
    payload
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("request must be an object"))?
        .remove("expected_state_version");
    Ok((id.into(), digest(&json!({"kind":kind,"payload":payload}))))
}

pub(super) async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    record_path: &str,
    field: &str,
    id: &str,
    hash: &str,
) -> ApiResult<Option<Value>> {
    // Keep the head bounded without expiring replay identities: immutable
    // versions retain every receipt, including A after B and across restarts.
    let selector = if field == "dreamer_state" {
        json!({"dreamer_state":{"research":{"receipts":[{"operation_id":id}]}}})
    } else {
        json!({(field):{"receipts":[{"operation_id":id}]}})
    };
    let metadata: Option<Value> = sqlx::query_scalar(
        "SELECT v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id WHERE e.user_id=$1 AND e.path=$2 AND e.deleted_at IS NULL AND v.metadata @> $3 ORDER BY v.version DESC LIMIT 1")
        .bind(auth.user_id.0).bind(record_path).bind(selector).fetch_optional(&mut **tx).await?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let receipts = if field == "dreamer_state" {
        &metadata[field]["research"]["receipts"]
    } else {
        &metadata[field]["receipts"]
    };
    let retained = receipts
        .as_array()
        .and_then(|values| values.iter().find(|v| v["operation_id"] == id))
        .ok_or_else(|| ApiError::invalid("Research replay receipt is invalid"))?;
    if retained["request_hash"] != hash || retained["producer"] != auth.credential_id.0.to_string()
    {
        return Err(ApiError::invalid(
            "operation_id was already accepted with a different payload or producer",
        ));
    }
    Ok(Some(retained.clone()))
}

pub(super) fn remember(
    receipts: &mut Vec<Value>,
    auth: &AuthContext,
    id: &str,
    hash: &str,
    result: Value,
) {
    if receipts.len() >= MAX_RECEIPTS {
        receipts.remove(0);
    }
    receipts.push(json!({"operation_id":id,"request_hash":hash,"producer":auth.credential_id.0,"result":result}));
}

pub(super) fn checked_attempt(
    data: &RunState,
    body: &Value,
    auth: &AuthContext,
    version: i64,
    replay: bool,
) -> ApiResult<Attempt> {
    let mut checked = body.clone();
    if replay {
        checked["expected_state_version"] = json!(version);
    }
    active(data, &checked, auth, version)
}

pub(super) fn check_job(
    data: &RunState,
    body: &Value,
    job: &Job,
    job_version: i64,
) -> ApiResult<()> {
    if data.research.active_subject.as_deref() != Some(job.subject_ref.as_str()) {
        return Err(ApiError::invalid(
            "subject_ref is not the active research subject",
        ));
    }
    if integer(body, "research_version")? != job_version {
        return Err(ApiError::conflict(
            "dreamer_research_conflict",
            "Research changed; reload its exact version before retrying",
            json!({"actual_version":job_version}),
        ));
    }
    Ok(())
}

pub(super) async fn generation(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
) -> ApiResult<i64> {
    crate::db::lock_workspace_commit(tx, auth.user_id.0).await?;
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(MAX(generation),0) FROM brunn.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut **tx)
    .await?)
}

pub(super) async fn headers(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    ids: &[Uuid],
    upper: i64,
) -> ApiResult<Vec<Input>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // Pin each source at its latest version admitted no later than `upper`.
    // With `upper` at the current generation this is the head; with a pass
    // cutoff it is the exact evidence that pass may rely on.
    let rows = sqlx::query(r#"
        SELECT e.id,e.path,c.entry_version AS current_version,v.metadata,head.metadata AS head_metadata,v.content_sha256,c.generation
        FROM brunn.entries e
        CROSS JOIN LATERAL (SELECT entry_version,generation,operation FROM brunn.workspace_changes
            WHERE user_id=e.user_id AND entry_id=e.id AND generation<=$3
            ORDER BY generation DESC LIMIT 1) c
        JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=c.entry_version
        JOIN brunn.entry_versions head ON head.user_id=e.user_id AND head.entry_id=e.id AND head.version=e.current_version
        WHERE e.user_id=$1 AND e.id=ANY($2) AND e.deleted_at IS NULL AND e.kind='markdown'
          AND v.content IS NOT NULL AND v.size_bytes<=1048576 AND c.operation<>'delete'
    "#).bind(auth.user_id.0).bind(ids).bind(upper).fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let path: String = row.get("path");
            // Source policy applies to the record, not one version: a source
            // excluded at its current head is not evidence at an older pin.
            if location_discovery::excluded(&path, &row.get::<Value, _>("metadata"))
                || location_discovery::excluded(&path, &row.get::<Value, _>("head_metadata"))
            {
                return None;
            }
            Some(Input {
                entry_ref: format!("entry:{}", row.get::<Uuid, _>("id")),
                path,
                version: row.get("current_version"),
                generation: row.get("generation"),
                operation: "research".into(),
                content_hash: format!("sha256:{}", row.get::<String, _>("content_sha256")),
            })
        })
        .collect())
}

fn dependencies(job: &Job) -> ApiResult<Vec<(Uuid, i64)>> {
    job.sources
        .iter()
        .map(|s| Ok((entry_id(&s.entry_ref)?, s.version)))
        .collect()
}

pub(super) fn cutoff(job: &Job) -> i64 {
    job.pass
        .as_ref()
        .map_or(job.snapshot_generation, |pass| pass.cutoff)
}

fn relied_on(job: &Job, source: &Input) -> bool {
    source.entry_ref == job.subject_ref
        || job.reviewed_sources.iter().any(|reviewed| {
            reviewed.entry_ref == source.entry_ref && reviewed.version == source.version
        })
        || job.pass.is_none()
            && job.reviewed_through.as_ref().is_some_and(|reviewed| {
                reviewed
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.entry_ref == source.entry_ref)
            })
}

/// Reliance evidence: the canonical subject and every source the retained
/// work actually reviewed or cited, at its exact pinned version. Admitted but
/// unreviewed discovery leads are not dependencies of that work.
fn reliance(job: &Job) -> ApiResult<Vec<(Uuid, i64)>> {
    let mut pins = BTreeMap::new();
    if job.pass.is_none()
        && let Some(reviewed) = &job.reviewed_through
    {
        for dependency in &reviewed.dependencies {
            pins.insert(entry_id(&dependency.entry_ref)?, dependency.version);
        }
    }
    for source in &job.sources {
        if relied_on(job, source) {
            pins.insert(entry_id(&source.entry_ref)?, source.version);
        }
    }
    Ok(pins.into_iter().collect())
}

/// Reliance evidence remains available at its pinned versions and the bounded
/// change scan for this pass has finished. Version drift after the cutoff is
/// refresh work for a later pass, not a reason to reject supported work.
pub(super) async fn fresh(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
) -> ApiResult<bool> {
    Ok(job.change_scan.is_none()
        && check_scope(tx, auth, &job.scope, &reliance(job)?)
            .await?
            .status
            == "fresh")
}

/// A completed subject is due again when its reliance evidence moved or lost
/// access. This is one bounded header check, never a corpus scan.
async fn refresh_due(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
) -> ApiResult<bool> {
    let check = check_scope(tx, auth, &job.scope, &reliance(job)?).await?;
    Ok(job.change_scan.is_some() || check.status != "fresh" || !check.changed.is_empty())
}

/// Changed reliance evidence must be reread at its admitted version before a
/// pass can complete; unchanged citations alone cannot certify it. Newly
/// relevant leads are offered, not required, so peripheral churn cannot move
/// the finish line indefinitely.
pub(super) fn require_changed_reviewed(job: &Job, cited: &[Source]) -> ApiResult<()> {
    let Some(pass) = &job.pass else {
        return Ok(());
    };
    let missing = pass
        .changed
        .iter()
        .filter(|change| {
            change.required
                && change.kind == "version"
                && job.sources.iter().any(|source| {
                    source.entry_ref == change.entry_ref && source.version == change.version
                })
                && !job.reviewed_sources.iter().chain(cited).any(|source| {
                    source.entry_ref == change.entry_ref && source.version == change.version
                })
        })
        .map(|change| format!("{} v{}", change.entry_ref, change.version))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(repair_error(
        ApiError::invalid(format!(
            "changed reliance evidence must be reviewed at its current admitted version before this pass completes: {}",
            missing.join(", ")
        )),
        RepairPhase::CandidateValidation,
    ))
}

/// Finish the pass: record what the retained overview was reviewed through so
/// reads and scheduling can report refresh status without model work.
pub(super) fn complete_pass(
    job: &mut Job,
    disposition: &str,
    candidate_hash: Option<String>,
    cited: &[Source],
) {
    let generation = cutoff(job);
    let mut dependencies = BTreeMap::new();
    for source in &job.sources {
        let relied = relied_on(job, source)
            || cited
                .iter()
                .any(|cite| cite.entry_ref == source.entry_ref && cite.version == source.version)
            || job.reviewed_through.as_ref().is_some_and(|reviewed| {
                reviewed
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.entry_ref == source.entry_ref)
            });
        if relied {
            dependencies.insert(
                source.entry_ref.clone(),
                SubjectDependency {
                    entry_ref: source.entry_ref.clone(),
                    version: source.version,
                    path: source.path.clone(),
                },
            );
        }
    }
    if let Some(pass) = job.pass.take() {
        job.coverage["last_pass"] = json!({"cutoff":pass.cutoff,"rounds":pass.rounds,
            "outcome":disposition,"changes_offered":pass.changed.len()});
    }
    job.reviewed_through = Some(Reviewed {
        generation,
        at: Utc::now(),
        disposition: disposition.into(),
        candidate_hash,
        dependencies: dependencies.into_values().collect(),
    });
}

fn count_round(job: &mut Job) {
    if let Some(pass) = &mut job.pass {
        pass.rounds += 1;
    }
}

/// Rehydration is always preceded by checking the current exact headers and
/// reviewed selectors. Notes cannot outlive permission, version or scope loss.
async fn safe_view(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
) -> ApiResult<Value> {
    let ids = dependencies(job)?
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    let current = headers(tx, auth, &ids, cutoff(job)).await?;
    let available = |source: &Input| current.iter().any(|head| head == source);
    let valid = job
        .sources
        .iter()
        .filter(|source| relied_on(job, source))
        .all(available);
    let mut reviewed = job.reviewed_sources.clone();
    let selectors_valid = valid
        && source_versions(
            tx,
            auth.user_id.0,
            &mut reviewed,
            job.snapshot_generation,
            false,
        )
        .await
        .is_ok();
    let scope_fresh = selectors_valid && fresh(tx, auth, job).await?;
    let mut view = serde_json::to_value(job)?;
    view["version"] = json!(version);
    view["output_version"] = json!(
        load_entry(tx, auth.user_id.0, &job.output_path)
            .await?
            .map_or(0, |e| e.version)
    );
    let object = view.as_object_mut().expect("research object");
    for field in [
        "receipts",
        "discoveries",
        "scope",
        "last_served",
        "retry_at",
        "revalidation_checkpoint",
        "checkpoint_versions",
    ] {
        object.remove(field);
    }
    if !scope_fresh {
        view["notes"] = json!("");
        view["repair_feedback"] = Value::Null;
        view["reviewed_sources"] = json!([]);
        view["status"] = json!("researching");
        view["needs_refresh"] = json!(true);
    }
    // Withhold inaccessible identities and cached titles as well as conclusions.
    // The next selection refreshes durable state; a read never advances coverage.
    if !job.sources.iter().all(available) {
        view["sources"] = json!(
            current
                .into_iter()
                .filter(|c| job.sources.contains(c))
                .collect::<Vec<_>>()
        );
        if !valid {
            view["pending_queries"] = json!([]);
            view["pending_targets"] = json!([]);
        }
        if !view["sources"]
            .as_array()
            .is_some_and(|sources| sources.iter().any(|s| s["entry_ref"] == job.subject_ref))
        {
            return Ok(Value::Null);
        }
    }
    if let Some(pass) = view.get_mut("pass").and_then(Value::as_object_mut) {
        let rounds = pass["rounds"].as_u64().unwrap_or(0) as usize;
        pass.insert(
            "rounds_remaining".into(),
            json!(PASS_ROUND_LIMIT.saturating_sub(rounds)),
        );
    }
    let origin = if job.schema == "dream.research.v2" {
        None
    } else if !scope_fresh && !job.notes.trim().is_empty() && !job.reviewed_sources.is_empty() {
        Some(version)
    } else {
        match job.revalidation_checkpoint {
            Some(RevalidationCheckpoint::Retained { version: prior })
                if prior > 0 && prior < version =>
            {
                Some(prior)
            }
            _ => None,
        }
    };
    view["revalidation_context"] = match origin {
        Some(origin) => crate::dreamer_summary::research_revalidation_context(
            tx,
            auth,
            &path(&job.subject_ref)?,
            &job.subject_ref,
            origin,
        )
        .await?
        .unwrap_or(Value::Null),
        None => Value::Null,
    };
    checkpoints::project(tx, auth, job, version, scope_fresh, &mut view).await?;
    discovery_audit::project(tx, auth, job, version, &mut view).await?;
    drafts::project(tx, auth, job, &mut view).await?;
    Ok(view)
}

pub(super) async fn view_active(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
) -> ApiResult<Value> {
    let Some(reference) = &data.research.active_subject else {
        return Ok(Value::Null);
    };
    let Some((job, version)) = load(tx, auth, reference).await? else {
        return Ok(Value::Null);
    };
    let mut view = safe_view(tx, auth, &job, version).await?;
    if !view.is_null() {
        let (work, targets) = research_comparison::routed_view(tx, auth, data, &job).await?;
        view["follow_up_protocol"] = json!(research_comparison::FOLLOW_UP_PROTOCOL);
        view["routed_work"] = json!(work);
        view["routed_targets"] = json!(targets);
    }
    Ok(view)
}

async fn create(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    input: Input,
    upper: i64,
) -> ApiResult<Job> {
    let scope = create_scope(tx, auth, &input.entry_ref, upper).await?;
    let id = entry_id(&input.entry_ref)?;
    let title: Option<String> = sqlx::query_scalar(
        "SELECT title FROM brunn.entries WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL",
    )
    .bind(auth.user_id.0)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(Job {
        schema: "dream.research.v1".into(),
        subject_ref: input.entry_ref.clone(),
        subject_path: input.path.clone(),
        title: title.unwrap_or_else(|| input.path.clone()),
        output_path: format!("derived/entities/{id}.md"),
        scope,
        snapshot_generation: upper,
        sources: vec![input],
        reviewed_sources: Vec::new(),
        notes: String::new(),
        repair_feedback: None,
        revalidation_checkpoint: Some(RevalidationCheckpoint::None),
        checkpoint_versions: None,
        pending_queries: Vec::new(),
        pending_targets: Vec::new(),
        status: "researching".into(),
        round: 0,
        last_served: 0,
        last_attempt: None,
        retry_at: Utc::now(),
        accepted_candidate_ids: Vec::new(),
        discoveries: Vec::new(),
        receipts: Vec::new(),
        coverage: json!({"evidence_cutoff":upper}),
        change_scan: None,
        pending_change_refs: Vec::new(),
        pass: Some(Pass {
            cutoff: upper,
            started_at: Utc::now(),
            rounds: 0,
            changed: Vec::new(),
            yielded: false,
        }),
        reviewed_through: None,
    })
}

async fn held(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    reference: &str,
) -> ApiResult<bool> {
    let output = format!("derived/entities/{}.md", entry_id(reference)?);
    for item in data.items.iter().filter(|item| {
        item.candidate.path.as_deref() == Some(output.as_str())
            && matches!(
                item.status.as_str(),
                "approved_held" | "deferred" | "rejected"
            )
    }) {
        // Research must never mutate a retained approval or rejected identity.
        // Changed approval evidence is handled by the existing owner review flow.
        if item.status != "approved_held" || !item_stale(tx, auth, item).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn due(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    reference: &str,
) -> ApiResult<bool> {
    if held(tx, auth, data, reference).await? {
        return Ok(false);
    }
    let Some((job, _)) = load(tx, auth, reference).await? else {
        return Ok(true);
    };
    if data
        .active
        .as_ref()
        .is_some_and(|attempt| job.last_attempt.as_deref() == Some(attempt.attempt_id.as_str()))
    {
        return Ok(false);
    }
    // Serve any one subject at most once per selection cycle, even if a failed
    // model keeps it researching. The persisted sequence survives restarts.
    if data.research.active_subject.as_deref() == Some(reference)
        && data
            .active
            .as_ref()
            .is_some_and(|a| data.research.active_attempt.as_deref() == Some(a.attempt_id.as_str()))
    {
        return Ok(false);
    }
    Ok(job.status == "researching"
        || job.retry_at <= Utc::now()
        || refresh_due(tx, auth, &job).await?)
}

async fn automatic_due(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    reference: &str,
    upper: i64,
) -> ApiResult<bool> {
    let Some(input) = headers(tx, auth, &[entry_id(reference)?], upper)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    if data
        .research
        .skipped_subjects
        .iter()
        .any(|skip| skip["entry_ref"] == reference && skip["version"] == input.version)
    {
        return Ok(false);
    }
    match create_scope(tx, auth, reference, upper).await {
        Ok(_) => due(tx, auth, data, reference).await,
        Err(ApiError::Public {
            status, message, ..
        }) if status == axum::http::StatusCode::BAD_REQUEST
            && matches!(
                message.as_str(),
                "canonical subject aliases must be strings"
                    | "canonical subject aliases are invalid"
                    | "canonical subject names exceed the supported bounds"
            ) =>
        {
            if data.research.skipped_subjects.len() >= 32 {
                data.research.skipped_subjects.remove(0);
            }
            data.research
                .skipped_subjects
                .push(json!({"entry_ref":reference,"version":input.version,"reason":message}));
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

async fn seed_lane(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    lane: usize,
    upper: i64,
) -> ApiResult<Option<Input>> {
    let ids: Vec<Uuid> = match lane {
        0 | 2 => {
            let after = if lane == 0 {
                &data.research.people_after
            } else {
                &data.research.project_after
            };
            let query = if lane == 0 {
                "SELECT e.id,e.path FROM brunn.entries e WHERE e.user_id=$1 AND e.deleted_at IS NULL AND e.kind='markdown' AND e.path ~ '^(sources/)?People/[^/]+[.]md$' AND e.path>$2 ORDER BY e.path LIMIT 16"
            } else {
                "SELECT e.id,e.path FROM brunn.entries e WHERE e.user_id=$1 AND e.deleted_at IS NULL AND e.kind='markdown' AND e.path>$2 AND EXISTS(SELECT 1 FROM brunn.task_projects p WHERE p.user_id=e.user_id AND p.hub_path=e.path AND p.archived_at IS NULL) ORDER BY e.path LIMIT 16"
            };
            let rows = sqlx::query(query)
                .bind(auth.user_id.0)
                .bind(after)
                .fetch_all(&mut **tx)
                .await?;
            if rows.is_empty() {
                if lane == 0 {
                    data.research.people_after.clear();
                } else {
                    data.research.project_after.clear();
                }
            }
            let mut selected = None;
            for row in rows {
                let path: String = row.get("path");
                if lane == 0 {
                    data.research.people_after = path;
                } else {
                    data.research.project_after = path;
                }
                let id: Uuid = row.get("id");
                if automatic_due(tx, auth, data, &format!("entry:{id}"), upper).await? {
                    selected = Some(id);
                    break;
                }
            }
            selected.into_iter().collect()
        }
        1 => {
            let mut selected = None;
            for _ in 0..data.inputs.len() {
                let index = data.research.input_after % data.inputs.len();
                data.research.input_after = (index + 1) % data.inputs.len();
                let input = data.inputs[index].clone();
                if automatic_due(tx, auth, data, &input.entry_ref, upper).await? {
                    selected = Some(entry_id(&input.entry_ref)?);
                    break;
                }
            }
            selected.into_iter().collect()
        }
        _ => {
            let rows=sqlx::query("SELECT e.path,v.metadata->'dreamer_research'->>'subject_ref' AS subject_ref FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.deleted_at IS NULL AND starts_with(e.path,'dreams/research/') AND v.metadata ? 'dreamer_research' AND e.path>$2 ORDER BY e.path LIMIT 32")
                .bind(auth.user_id.0).bind(&data.research.resume_after).fetch_all(&mut **tx).await?;
            if rows.is_empty() {
                data.research.resume_after.clear();
            }
            let mut selected = None;
            for row in rows {
                data.research.resume_after = row.get("path");
                let reference: String = row.get("subject_ref");
                if automatic_due(tx, auth, data, &reference, upper).await? {
                    selected = Some(entry_id(&reference)?);
                    break;
                }
            }
            selected.into_iter().collect()
        }
    };
    Ok(headers(tx, auth, &ids, upper).await?.into_iter().next())
}

/// Bring the retained job to its pass cutoff. A selection starts a new pass
/// only when none is active or the active one exhausted its allowance;
/// discovery rounds and resumed turns keep the pinned cutoff, so new writes
/// cannot move the finish line. Changed reliance evidence is listed for the
/// model to reread; unreviewed lead churn is queued without discarding work.
async fn refresh(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &mut Job,
    job_version: i64,
    upper: i64,
    selection: bool,
) -> ApiResult<()> {
    initialize_revalidation(tx, auth, job, job_version).await?;
    let exhausted = job
        .pass
        .as_ref()
        .is_some_and(|pass| pass.rounds >= PASS_ROUND_LIMIT);
    let yielded = job.pass.as_ref().is_some_and(|pass| pass.yielded);
    let new_pass = job.pass.is_none() || (selection && (exhausted || yielded));
    if new_pass {
        let mut carried = Vec::new();
        let mut rounds = 0;
        let mut started_at = Utc::now();
        if let Some(pass) = &job.pass {
            if exhausted {
                job.coverage["last_pass"] = json!({"cutoff":pass.cutoff,"rounds":pass.rounds,
                    "outcome":"exhausted","changes_offered":pass.changed.len()});
            } else {
                rounds = pass.rounds;
                started_at = pass.started_at;
                carried = pass.changed.clone();
            }
        }
        job.pass = Some(Pass {
            cutoff: upper,
            started_at,
            rounds,
            changed: carried,
            yielded: false,
        });
    }
    let cutoff = cutoff(job);
    let deps = dependencies(job)?;
    let mut scan = job.change_scan.clone().unwrap_or(ChangeScan {
        cursor: job.scope.checked_generation,
        upper: cutoff,
    });
    let mut scan_scope = job.scope.clone();
    scan_scope.checked_generation = scan.cursor.min(cutoff);
    let changes = research_change_page(tx, auth, &scan_scope, &deps, cutoff).await?;
    scan.upper = changes.through_generation.min(cutoff);
    let mut ids = deps.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
    for reference in &job.pending_change_refs {
        let id = entry_id(reference)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    for id in &changes.relevant_ids {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    let mut current = headers(tx, auth, &ids, cutoff).await?;
    // Keep operational corrections across eligible version changes, but never
    // let a cached identity reappear after its inaccessible dependency is
    // removed from the manifest used by later safe projections.
    if job
        .sources
        .iter()
        .any(|old| !current.iter().any(|head| head.entry_ref == old.entry_ref))
    {
        job.repair_feedback = None;
    }
    let mut unresolved = Vec::new();
    for id in &ids {
        if current
            .iter()
            .any(|input| input.entry_ref == format!("entry:{id}"))
        {
            continue;
        }
        let head=sqlx::query("SELECT e.path,e.deleted_at,v.metadata FROM brunn.entries e LEFT JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.id=$2")
            .bind(auth.user_id.0).bind(id).fetch_optional(&mut **tx).await?;
        let dispositioned = head.as_ref().is_some_and(|row| {
            row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some()
                || location_discovery::excluded(
                    row.get("path"),
                    &row.get::<Option<Value>, _>("metadata")
                        .unwrap_or(Value::Null),
                )
        });
        if !dispositioned {
            unresolved.push(format!("entry:{id}"));
        }
    }
    current.sort_by_key(|input| {
        (
            input.entry_ref != job.subject_ref,
            !job.sources
                .iter()
                .any(|old| old.entry_ref == input.entry_ref),
            input.generation,
        )
    });
    let mut pending = current
        .iter()
        .skip(MAX_SOURCES)
        .map(|input| input.entry_ref.clone())
        .collect::<Vec<_>>();
    pending.extend(unresolved.iter().cloned());
    if pending.len() > 2048 {
        return Err(ApiError::invalid(
            "Research pending source queue is full; the change cursor and exact source versions remain retained",
        ));
    }
    job.pending_change_refs = pending;
    current.truncate(MAX_SOURCES);
    // Evidence delta against the previously pinned sources. Reliance loss
    // withholds cached conclusions; reliance drift keeps the prose and lists
    // the exact versions to reread; unreviewed lead churn is not a change to
    // the retained work at all.
    let mut delta = Vec::new();
    let mut lost_reliance = false;
    for old in &job.sources {
        let relied = relied_on(job, old);
        match current.iter().find(|head| head.entry_ref == old.entry_ref) {
            None if relied => {
                lost_reliance = true;
                delta.push(PassChange {
                    entry_ref: old.entry_ref.clone(),
                    path: old.path.clone(),
                    kind: "unavailable".into(),
                    from_version: Some(old.version),
                    version: old.version,
                    required: false,
                });
            }
            Some(head) if head.version != old.version => delta.push(PassChange {
                entry_ref: old.entry_ref.clone(),
                path: head.path.clone(),
                kind: "version".into(),
                from_version: Some(old.version),
                version: head.version,
                required: relied,
            }),
            _ => {}
        }
    }
    for id in &changes.relevant_ids {
        let reference = format!("entry:{id}");
        if job.sources.iter().any(|old| old.entry_ref == reference) {
            continue;
        }
        if let Some(head) = current.iter().find(|head| head.entry_ref == reference) {
            delta.push(PassChange {
                entry_ref: reference,
                path: head.path.clone(),
                kind: "new_relevant".into(),
                from_version: None,
                version: head.version,
                required: false,
            });
        }
    }
    if lost_reliance {
        discovery_audit::invalidate(job);
        capture_revalidation(job, job_version)?;
        job.notes.clear();
        job.reviewed_sources.clear();
        job.discoveries.clear();
        job.status = "researching".into();
    } else {
        let surviving = job
            .reviewed_sources
            .iter()
            .filter(|reviewed| {
                current.iter().any(|head| {
                    head.entry_ref == reviewed.entry_ref && head.version == reviewed.version
                })
            })
            .count();
        if surviving != job.reviewed_sources.len() {
            discovery_audit::invalidate(job);
        }
        if surviving == 0 && !job.reviewed_sources.is_empty() && !job.notes.trim().is_empty() {
            // Notes cannot outlive every selector that supported them; keep
            // the immutable notebook version as reconcilable history instead.
            capture_revalidation(job, job_version)?;
            job.notes.clear();
            job.reviewed_sources.clear();
        } else {
            job.reviewed_sources.retain(|reviewed| {
                current.iter().any(|head| {
                    head.entry_ref == reviewed.entry_ref && head.version == reviewed.version
                })
            });
            if job.notes.trim().is_empty() {
                job.reviewed_sources.clear();
            }
        }
        if !delta.is_empty() {
            job.status = "researching".into();
        }
    }
    if let Some(pass) = &mut job.pass {
        for change in delta {
            if pass.changed.len() < MAX_PASS_CHANGES
                && !pass
                    .changed
                    .iter()
                    .any(|known| known.entry_ref == change.entry_ref)
            {
                pass.changed.push(change);
            }
        }
    }
    job.sources = current;
    let canonical = job
        .sources
        .iter()
        .find(|s| s.entry_ref == job.subject_ref)
        .ok_or_else(|| ApiError::invalid("Canonical subject is missing or inaccessible"))?;
    job.subject_path = canonical.path.clone();
    job.title = sqlx::query_scalar(
        "SELECT title FROM brunn.entries WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL",
    )
    .bind(auth.user_id.0)
    .bind(entry_id(&job.subject_ref)?)
    .fetch_one(&mut **tx)
    .await?;
    job.snapshot_generation = cutoff;
    scan.cursor = scan.cursor.max(changes.scanned_generation).min(cutoff);
    let cap_reached = !job.pending_change_refs.is_empty();
    let complete = changes.status == "complete";
    if complete {
        job.scope = create_scope(tx, auth, &job.subject_ref, cutoff).await?;
        job.change_scan = None;
    } else {
        job.change_scan = Some(scan.clone());
    }
    job.coverage["change_status"] = json!(if complete { "complete" } else { "unchecked" });
    job.coverage["change_reason"] = json!(if !complete {
        changes.reason
    } else if !unresolved.is_empty() {
        "research_sources_unresolved"
    } else if cap_reached {
        "research_source_cap_reached"
    } else {
        changes.reason
    });
    job.coverage["changed_source_count"] = json!(changes.relevant_ids.len());
    job.coverage["change_cursor"] = json!(scan.cursor);
    job.coverage["change_upper"] = json!(scan.upper);
    job.coverage["pending_source_refs"] = json!(job.pending_change_refs);
    job.coverage["evidence_cutoff"] = json!(cutoff);
    Ok(())
}

pub(super) async fn enqueue_requested(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    body: &Value,
    upper: i64,
) -> ApiResult<()> {
    let requested: Vec<String> = serde_json::from_value(
        body.get("requested_subject_refs")
            .cloned()
            .unwrap_or(json!([])),
    )?;
    if requested.len() > 16 {
        return Err(ApiError::invalid(
            "at most 16 requested subject references are allowed",
        ));
    }
    for reference in requested {
        let id = entry_id(&reference)?;
        let reference = format!("entry:{id}");
        // An explicit owner request survives retirement of a routed source.
        data.research
            .follow_up_priorities
            .retain(|routed| routed != &reference);
        if headers(tx, auth, &[id], upper).await?.is_empty() {
            return Err(ApiError::invalid(
                "requested subject is not an accessible ordinary source",
            ));
        }
        create_scope(tx, auth, &reference, upper).await?;
        if !data.research.requested_subject_refs.contains(&reference) {
            data.research.requested_subject_refs.push(reference);
        }
    }
    if data.research.requested_subject_refs.len() > 16 {
        return Err(ApiError::invalid(
            "requested subject queue is full; pending requests were retained",
        ));
    }
    Ok(())
}

pub(super) async fn invalidate_held(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
) -> ApiResult<()> {
    for index in 0..data.items.len() {
        let item = &data.items[index];
        if item.status != "approved_held" || item.candidate.subject_scope.is_none() {
            continue;
        }
        if item_stale(tx, auth, item).await? {
            data.candidate_dispositions.push(json!({"disposition":"held_approval_invalidated", "reason":"subject_evidence_changed", "item_id":item.id,"candidate_hash":item.candidate_hash,"run_entry_ref":item.run_entry_ref,"run_version":item.run_version}));
            data.items[index].status = "needs_changes".into();
        }
    }
    Ok(())
}

pub(super) async fn next(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let (operation_id, hash) = operation(&body, "research-next")?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    let replay = receipt(
        &mut tx,
        &auth,
        STATE_PATH,
        "dreamer_state",
        &operation_id,
        &hash,
    )
    .await?;
    checked_attempt(&data, &body, &auth, version, replay.is_some())?;
    if let Some(replay) = replay {
        return Ok(Json(
            json!({"data":admission_response(&mut tx,&auth,&data,version).await?,"no_op":true,"operation_receipt":replay["result"]}),
        ));
    }
    let upper = generation(&mut tx, &auth).await?;
    invalidate_held(&mut tx, &auth, &mut data).await?;
    let mut selected = None;
    for reference in data.research.requested_subject_refs.clone() {
        let served = load(&mut tx, &auth, &reference)
            .await?
            .is_some_and(|(job, _)| {
                data.active.as_ref().is_some_and(|attempt| {
                    job.last_attempt.as_deref() == Some(attempt.attempt_id.as_str())
                })
            });
        if !served && !held(&mut tx, &auth, &data, &reference).await? {
            selected = headers(&mut tx, &auth, &[entry_id(&reference)?], upper)
                .await?
                .into_iter()
                .next();
            if selected.is_some() {
                break;
            }
        }
    }
    for _ in 0..if selected.is_some() { 0 } else { 4 } {
        let lane = data.research.lane % 4;
        data.research.lane = (lane + 1) % 4;
        if let Some(input) = seed_lane(&mut tx, &auth, &mut data, lane, upper).await? {
            selected = Some(input);
            break;
        }
    }
    // Releasing the previous pointer makes it due in the following selection,
    // after every other scheduling lane has received its turn.
    data.research.active_subject = None;
    if let Some(input) = selected {
        let (mut job, job_version) = match load(&mut tx, &auth, &input.entry_ref).await? {
            Some((mut job, version)) => {
                refresh(&mut tx, &auth, &mut job, version, upper, true).await?;
                (job, version)
            }
            None => (create(&mut tx, &auth, input, upper).await?, 0),
        };
        data.research.service_sequence += 1;
        job.last_served = data.research.service_sequence;
        job.last_attempt = data
            .active
            .as_ref()
            .map(|attempt| attempt.attempt_id.clone());
        job.status = "researching".into();
        data.research.active_subject = Some(job.subject_ref.clone());
        data.research.active_attempt = data.active.as_ref().map(|a| a.attempt_id.clone());
        save(&state, &mut tx, &auth, &job, job_version).await?;
    }
    let selected_ref = data.research.active_subject.clone();
    remember(
        &mut data.research.receipts,
        &auth,
        &operation_id,
        &hash,
        json!({"subject_ref":selected_ref}),
    );
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    let response = admission_response(&mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(json!({"data":response})))
}

fn strings(body: &Value, field: &str, count: usize, bytes: usize) -> ApiResult<Vec<String>> {
    let values: Vec<String> =
        serde_json::from_value(body.get(field).cloned().unwrap_or(json!([])))?;
    if values.len() > count
        || values
            .iter()
            .any(|v| v.trim().is_empty() || v.len() > bytes || v.contains(['\n', '\r']))
    {
        return Err(ApiError::invalid(format!(
            "{field} exceeds its research bounds"
        )));
    }
    Ok(values)
}

/// Mark only an actual model contract failure after its authority checks.
/// Source access, freshness, owner decisions and fences keep their own errors.
pub(super) fn repair_error(mut error: ApiError, phase: RepairPhase) -> ApiError {
    if let ApiError::Public {
        status,
        code: "invalid_request",
        details,
        ..
    } = &mut error
        && *status == axum::http::StatusCode::BAD_REQUEST
    {
        let details = details.get_or_insert_with(|| json!({}));
        if let Some(fields) = details.as_object_mut() {
            fields.insert("dreamer_repair_phase".into(), json!(phase));
        }
    }
    error
}

fn apply_repair_feedback(job: &mut Job, body: &Value) -> ApiResult<()> {
    let allowed = [
        "attempt_id",
        "fence",
        "expected_state_version",
        "operation_id",
        "subject_ref",
        "research_version",
        "status",
        "repair_feedback",
        "processed_inputs",
    ];
    if body
        .as_object()
        .is_none_or(|fields| fields.keys().any(|key| !allowed.contains(&key.as_str())))
        || body["processed_inputs"] != json!([])
    {
        return Err(ApiError::invalid(
            "repair feedback requires operational-only progress with no evidence or input disposition",
        ));
    }
    let mut feedback: RepairFeedback = serde_json::from_value(body["repair_feedback"].clone())
        .map_err(|_| ApiError::invalid("repair feedback must have a valid phase and message"))?;
    if !feedback.valid() {
        return Err(ApiError::invalid(
            "repair feedback exceeds its message bounds",
        ));
    }
    let status = string(body, "status")?;
    if !matches!(status, "researching" | "waiting") {
        return Err(ApiError::invalid(
            "repair feedback cannot complete a research subject",
        ));
    }
    if let Some(prior) = &job.repair_feedback
        && prior.phase == RepairPhase::CandidateValidation
        && feedback.phase != RepairPhase::CandidateValidation
    {
        // A later notebook/JSON repair does not discharge the outstanding
        // candidate defect. Retain both bounded corrections with candidate
        // lifetime, including when the previous hint is currently withheld.
        let mut prefix_end = prior.message.len().min(2048);
        while !prior.message.is_char_boundary(prefix_end) {
            prefix_end -= 1;
        }
        let phase = match feedback.phase {
            RepairPhase::ResponseValidation => "response_validation",
            RepairPhase::CheckpointValidation => "checkpoint_validation",
            RepairPhase::CandidateValidation => unreachable!("lower-phase correction"),
        };
        feedback = RepairFeedback::new(
            RepairPhase::CandidateValidation,
            &format!(
                "{}\n\nAdditional {phase} correction: {}",
                &prior.message[..prefix_end],
                feedback.message
            ),
        );
    }
    job.repair_feedback = Some(feedback);
    job.status = status.into();
    job.retry_at = Utc::now()
        + if status == "waiting" {
            Duration::hours(6)
        } else {
            Duration::hours(24)
        };
    Ok(())
}

pub(super) async fn apply_progress(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &mut Job,
    job_version: i64,
    body: &Value,
) -> ApiResult<()> {
    if [
        "revalidation_checkpoint",
        "revalidation_context",
        "checkpoint_versions",
        "checkpoint_contexts",
        "current_checkpoint",
        "checkpoint_receipt",
        "discovery_audit",
        "draft_candidate",
        "draft_protocol",
        "draft_pointer",
        "replaces_draft",
        "unaccepted_draft",
        "draft_receipt",
    ]
    .iter()
    .any(|field| body.get(field).is_some())
    {
        return Err(ApiError::invalid(
            "research revalidation context is server-owned",
        ));
    }
    if body.get("repair_feedback").is_some() {
        return Err(ApiError::invalid(
            "repair feedback requires an operational-only research-progress request",
        ));
    }
    let invalid = |message: &str| {
        repair_error(
            ApiError::invalid(message),
            RepairPhase::CheckpointValidation,
        )
    };
    let status = string(body, "status")?;
    if status == "waiting" && !fresh(tx, auth, job).await? {
        discovery_audit::invalidate(job);
        initialize_revalidation(tx, auth, job, job_version).await?;
        capture_revalidation(job, job_version)?;
        // Yielding unavailable/unchecked evidence is scheduling, not a claim.
        // Clear conclusions and cached leads; keep durable evidence/cursors so
        // another subject can proceed and this interval can be retried later.
        job.notes.clear();
        job.reviewed_sources.clear();
        job.pending_queries.clear();
        job.pending_targets.clear();
        job.status = "waiting".into();
        job.retry_at = Utc::now() + Duration::hours(6);
        if let Some(pass) = &mut job.pass {
            pass.yielded = true;
        }
        return Ok(());
    }
    let notes = body
        .get("notes")
        .and_then(Value::as_str)
        .unwrap_or(&job.notes);
    if notes.len() > MAX_RESEARCH_NOTES_BYTES {
        return Err(invalid(&format!(
            "research notes contain {} UTF-8 bytes; maximum is {MAX_RESEARCH_NOTES_BYTES}",
            notes.len()
        )));
    }
    let status = string(body, "status")?;
    if !matches!(status, "researching" | "waiting" | "no_change") {
        return Err(invalid("invalid research progress status"));
    }
    let mut reviewed: Vec<Source> = serde_json::from_value(
        body.get("reviewed_sources")
            .cloned()
            .unwrap_or_else(|| json!(job.reviewed_sources)),
    )?;
    if reviewed.len() > 64 {
        return Err(invalid(
            "reviewed selectors must resolve in admitted research evidence",
        ));
    }
    if reviewed.iter().any(|source| {
        !job.sources
            .iter()
            .any(|input| input.entry_ref == source.entry_ref && input.version == source.version)
    }) {
        return Err(ApiError::invalid(
            "reviewed selectors must resolve in admitted research evidence",
        ));
    }
    if !notes.trim().is_empty() && reviewed.is_empty() {
        return Err(invalid(
            "research notes require reviewed exact source selectors",
        ));
    }
    if status == "no_change" && !reviewed.iter().any(|s| s.entry_ref == job.subject_ref) {
        return Err(invalid("no_change must review the canonical subject"));
    }
    if status == "no_change"
        && let Some(pass) = &job.pass
        && pass.changed.iter().any(|change| {
            change.required
                && change.kind == "version"
                && job.sources.iter().any(|source| {
                    source.entry_ref == change.entry_ref && source.version == change.version
                })
                && !reviewed.iter().any(|source| {
                    source.entry_ref == change.entry_ref && source.version == change.version
                })
        })
    {
        return Err(invalid(
            "no_change must review every changed reliance source at its current admitted version",
        ));
    }
    // Selectors resolve against the pass's pinned versions, not current heads.
    source_versions_with_policy(
        tx,
        auth.user_id.0,
        &mut reviewed,
        job.snapshot_generation,
        false,
        SourceSelectorPolicy::CheckpointEndOfDocument,
    )
    .await?;
    if !fresh(tx, auth, job).await? {
        return Err(ApiError::public(
            axum::http::StatusCode::BAD_REQUEST,
            "research_refresh_required",
            "research evidence or relevant subject scope changed; rediscover before saving conclusions",
        ));
    }
    let notes = notes.to_owned();
    job.notes = notes;
    job.reviewed_sources = reviewed;
    count_round(job);
    if body.get("pending_queries").is_some() {
        job.pending_queries = strings(body, "pending_queries", 12, 160)
            .map_err(|error| repair_error(error, RepairPhase::CheckpointValidation))?;
    }
    if body.get("pending_targets").is_some() {
        job.pending_targets = strings(body, "pending_targets", 32, 1024)
            .map_err(|error| repair_error(error, RepairPhase::CheckpointValidation))?;
    }
    if status == "no_change"
        || body["reviewed_sources"]
            .as_array()
            .is_some_and(|sources| !sources.is_empty())
            && job
                .repair_feedback
                .as_ref()
                .is_some_and(|feedback| feedback.phase != RepairPhase::CandidateValidation)
    {
        job.repair_feedback = None;
    }
    job.status = status.into();
    job.retry_at = Utc::now()
        + if status == "waiting" {
            Duration::hours(6)
        } else {
            Duration::hours(24)
        };
    if status == "waiting"
        && let Some(pass) = &mut job.pass
    {
        pass.yielded = true;
    }
    Ok(())
}

pub(super) async fn progress(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    if body.get("draft_candidate").is_some() {
        return drafts::custody(&state, &auth, &body).await.map(Json);
    }
    let reference = string(&body, "subject_ref")?;
    let (operation_id, hash) = operation(&body, "research-progress")?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (mut data, version) = load_state(&mut tx, auth.user_id.0).await?;
    let (mut job, job_version) = load(&mut tx, &auth, reference)
        .await?
        .ok_or_else(|| ApiError::invalid("research subject is not retained"))?;
    let replay = receipt(
        &mut tx,
        &auth,
        &path(reference)?,
        "dreamer_research",
        &operation_id,
        &hash,
    )
    .await?;
    checked_attempt(&data, &body, &auth, version, replay.is_some())?;
    if let Some(replay) = replay {
        let mut response = admission_response(&mut tx, &auth, &data, version).await?;
        if let Some(ack) = replay["result"].get("repair_feedback_receipt") {
            response["repair_feedback_receipt"] = ack.clone();
        }
        if let Some(ack) = replay["result"].get("checkpoint_receipt") {
            response["checkpoint_receipt"] = ack.clone();
            checkpoints::replay(&mut response["checkpoint_receipt"]);
        }
        if let Some(ack) = replay["result"].get("follow_up_receipt") {
            response["follow_up_receipt"] = ack.clone();
            research_comparison::replay_resolution(&mut response["follow_up_receipt"]);
        }
        return Ok(Json(json!({"data":response,"no_op":true})));
    }
    check_job(&data, &body, &job, job_version)?;
    let source_resolutions =
        research_comparison::prepare_resolutions(&mut tx, &auth, &data, &job, &body, false).await?;
    if body.get("repair_feedback").is_some() {
        apply_repair_feedback(&mut job, &body)?;
        let ack = json!({"operation_id":operation_id,"recorded":true});
        remember(
            &mut job.receipts,
            &auth,
            &operation_id,
            &hash,
            json!({"status":job.status,"repair_feedback_receipt":ack}),
        );
        save(&state, &mut tx, &auth, &job, job_version).await?;
        let version = save_state(&state, &mut tx, &auth, &data, version).await?;
        let mut response = admission_response(&mut tx, &auth, &data, version).await?;
        response["repair_feedback_receipt"] = ack;
        tx.commit().await?;
        return Ok(Json(json!({"data":response})));
    }
    let before_job = job.clone();
    apply_progress(&mut tx, &auth, &mut job, job_version, &body).await?;
    let completing = job.status == "no_change";
    let mut checkpoint_receipt = checkpoints::apply(
        &mut tx,
        &auth,
        &before_job,
        &mut job,
        job_version,
        &body,
        true,
        completing,
    )
    .await?;
    if let Some(ack) = &mut checkpoint_receipt {
        ack["operation_id"] = json!(operation_id);
    }
    let processed = body
        .get("processed_inputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !processed.is_empty() && job.status != "no_change" {
        return Err(ApiError::invalid(
            "only supported no_change progress can consume reviewed input without a candidate",
        ));
    }
    validate_processed(&job, &processed, Some(&body))?;
    for p in &processed {
        if !data.inputs.iter().any(|i| {
            p["entry_ref"] == i.entry_ref
                && p["version"] == i.version
                && p["generation"] == i.generation
        }) {
            return Err(ApiError::invalid(
                "processed input was not durably admitted",
            ));
        }
    }
    research_comparison::validate_progress(
        &state, &mut tx, &auth, &mut data, &job, &body, &processed,
    )
    .await?;
    let dispositions = if job.status == "no_change" {
        research_comparison::resolve_processed(
            &mut tx,
            &auth,
            &mut data,
            &job,
            &processed,
            Some(&body),
            None,
        )
        .await?
    } else {
        Vec::new()
    };
    let follow_up_receipt = research_comparison::resolve_sources(
        &state,
        &mut tx,
        &auth,
        &mut data,
        &job,
        source_resolutions,
        &body,
        None,
        &operation_id,
    )
    .await?;
    let before = data.inputs.len();
    data.inputs.retain(|i| {
        !processed.iter().any(|p| {
            p["entry_ref"] == i.entry_ref
                && p["version"] == i.version
                && p["generation"] == i.generation
        })
    });
    data.processed_count += before - data.inputs.len();
    data.processed_generation = data
        .inputs
        .iter()
        .map(|i| i.generation - 1)
        .min()
        .unwrap_or(data.scanned_generation)
        .min(data.scanned_generation);
    if job.status == "no_change"
        && research_comparison::retains_source_work(&data, &job.subject_ref)
    {
        job.status = "researching".into();
        job.retry_at = Utc::now();
        if let Some(ack) = &mut checkpoint_receipt {
            ack["subject_complete"] = json!(false);
        }
    }
    if job.status == "no_change" {
        if !research_comparison::retains_priority(&data, &job.subject_ref) {
            data.research
                .requested_subject_refs
                .retain(|reference| reference != &job.subject_ref);
            data.research
                .follow_up_priorities
                .retain(|reference| reference != &job.subject_ref);
        }
        data.research.completed += 1;
    }
    if job.status == "no_change" || explicit_revalidation_replacement(&job, &body) {
        clear_revalidation(&mut job);
    }
    if job.status == "no_change" {
        complete_pass(&mut job, "no_change", None, &[]);
    }
    remember(
        &mut job.receipts,
        &auth,
        &operation_id,
        &hash,
        json!({"status":job.status,"follow_up_dispositions":dispositions,"checkpoint_receipt":checkpoint_receipt,"follow_up_receipt":follow_up_receipt}),
    );
    save(&state, &mut tx, &auth, &job, job_version).await?;
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    let mut response = admission_response(&mut tx, &auth, &data, version).await?;
    if let Some(ack) = checkpoint_receipt {
        response["checkpoint_receipt"] = ack;
    }
    if let Some(ack) = follow_up_receipt {
        response["follow_up_receipt"] = ack;
    }
    tx.commit().await?;
    Ok(Json(json!({"data":response})))
}

async fn resolve_target(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    target: &str,
) -> ApiResult<Option<Uuid>> {
    if target.starts_with("entry:") {
        return entry_id(target).map(Some);
    }
    // These are workspace identifiers, never filesystem paths or remote URLs.
    if target.starts_with(['/', '\\', '~']) || url::Url::parse(target).is_ok() {
        return Ok(None);
    }
    if target.contains(['\\', '\n', '\r']) || target.split('/').any(|part| part == "..") {
        return Err(ApiError::invalid(
            "research targets must be exact source paths or entry references",
        ));
    }
    // An exact identity wins even when headers later exclude it. A deleted or
    // generated exact source must not silently redirect to another document.
    if let Some(id) =
        sqlx::query_scalar("SELECT id FROM brunn.entries WHERE user_id=$1 AND path=$2")
            .bind(auth.user_id.0)
            .bind(target)
            .fetch_optional(&mut **tx)
            .await?
    {
        return Ok(Some(id));
    }
    let stem = target
        .strip_suffix(".markdown")
        .or_else(|| target.strip_suffix(".md"))
        .unwrap_or(target);
    let mut variants = BTreeSet::new();
    for suffix in ["", ".md", ".markdown"] {
        let path = format!("{stem}{suffix}");
        if !path.starts_with("sources/") {
            variants.insert(format!("sources/{path}"));
        }
        variants.insert(path);
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM brunn.entries WHERE user_id=$1 AND path=ANY($2) AND deleted_at IS NULL",
    )
    .bind(auth.user_id.0)
    .bind(variants.into_iter().collect::<Vec<_>>())
    .fetch_all(&mut **tx)
    .await?;
    // Resolve the complete bounded set together; variant order is not evidence.
    Ok((ids.len() == 1).then(|| ids[0]))
}

pub(super) async fn discover(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let auth = runner_auth(&auth)?;
    let reference = string(&body, "subject_ref")?.to_owned();
    if body.get("resolved_follow_ups").is_some() || body.get("follow_up_protocol").is_some() {
        return Err(ApiError::invalid(
            "source follow-up acknowledgements are not supported during discovery",
        ));
    }
    let (operation_id, hash) = operation(&body, "narrative-discover")?;
    let queries = strings(&body, "queries", 6, 160)?;
    let targets = strings(&body, "targets", 32, 1024)?;
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (data, version) = load_state(&mut tx, auth.user_id.0).await?;
    let (job, job_version) = load(&mut tx, &auth, &reference)
        .await?
        .ok_or_else(|| ApiError::invalid("research subject is not retained"))?;
    let replay = receipt(
        &mut tx,
        &auth,
        &path(&reference)?,
        "dreamer_research",
        &operation_id,
        &hash,
    )
    .await?;
    checked_attempt(&data, &body, &auth, version, replay.is_some())?;
    if replay.is_some() {
        return Ok(Json(
            json!({"data":admission_response(&mut tx,&auth,&data,version).await?,"no_op":true}),
        ));
    }
    check_job(&data, &body, &job, job_version)?;
    let normalized = discovery_audit::normalize(&queries)?;
    let target_set = targets
        .iter()
        .map(|t| t.trim().to_owned())
        .collect::<BTreeSet<_>>();
    let query_hash = digest(&json!({"queries":normalized,"targets":target_set,
        "retrieval_policy":simple_core::DREAMER_LEXICAL_POLICY_VERSION}));
    let prior_fresh = fresh(&mut tx, &auth, &job).await?;
    let duplicate = if normalized.is_empty() {
        job.discoveries
            .iter()
            .any(|d| d["query_hash"] == query_hash)
            && prior_fresh
    } else {
        discovery_audit::matches(&mut tx, &auth, &job, job_version, &normalized).await?
    };
    let searched_generation = generation(&mut tx, &auth).await?;
    let search_basis = job.clone();
    tx.commit().await?;
    // Search is read-only and does not hold the owner write lock. Exact targets
    // are resolved again under RLS when the operation commits.
    let results = if !duplicate && !normalized.is_empty() {
        simple_core::search_headers_for_dreamer(&state, &auth, &normalized).await?
    } else {
        Vec::new()
    };
    let mut leads = Vec::<Uuid>::new();
    let mut expected = BTreeMap::new();
    for rank in 0..8 {
        for result in &results {
            let Some(hit) = result["candidates"].get(rank) else {
                continue;
            };
            let (Some(reference), Some(version)) =
                (hit["reference"].as_str(), hit["version"].as_i64())
            else {
                continue;
            };
            let Ok(id) = entry_id(reference) else {
                continue;
            };
            if expected.insert(id, version).is_none() {
                leads.push(id);
            }
        }
    }
    let mut tx = begin_runner_write(&state, &auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (data, version) = load_state(&mut tx, auth.user_id.0).await?;
    checked_attempt(&data, &body, &auth, version, false)?;
    let (mut job, job_version) = load(&mut tx, &auth, &reference)
        .await?
        .ok_or_else(|| ApiError::invalid("research subject is not retained"))?;
    check_job(&data, &body, &job, job_version)?;
    let upper = generation(&mut tx, &auth).await?;
    let mut exact = BTreeSet::new();
    let mut target_ids = BTreeMap::new();
    let mut unresolved = Vec::new();
    for target in &targets {
        if let Some(id) = resolve_target(&mut tx, &auth, target).await? {
            target_ids.insert(target.clone(), id);
            exact.insert(id);
            if !leads.contains(&id) {
                leads.push(id);
            }
        }
    }
    let canonical = entry_id(&job.subject_ref)?;
    if !leads.contains(&canonical) {
        leads.push(canonical);
        exact.insert(canonical);
    }
    refresh(&mut tx, &auth, &mut job, job_version, upper, false).await?;
    let admitted = headers(&mut tx, &auth, &leads, cutoff(&job)).await?;
    let search_results_current = expected.iter().all(|(id, version)| {
        admitted
            .iter()
            .any(|input| input.entry_ref == format!("entry:{id}") && input.version == *version)
    });
    let before = job.sources.clone();
    for input in admitted {
        let id = entry_id(&input.entry_ref)?;
        // A hit is admitted at its pinned version even when its head moved
        // past the cutoff; the newer content is queued for a later pass.
        if !exact.contains(&id) && !expected.contains_key(&id) {
            continue;
        }
        if let Some(old) = job
            .sources
            .iter_mut()
            .find(|s| s.entry_ref == input.entry_ref)
        {
            *old = input;
        } else if job.sources.len() < MAX_SOURCES {
            job.sources.push(input);
        }
    }
    for target in &targets {
        let resolved = target_ids.get(target).is_some_and(|id| {
            job.sources
                .iter()
                .any(|s| s.entry_ref == format!("entry:{id}"))
        });
        if !resolved && !unresolved.contains(target) {
            unresolved.push(target.clone());
        }
    }
    let changed = before != job.sources;
    // Additional headers are unreviewed leads, not a change to the exact
    // evidence supporting saved work. Preserve its notes and selectors unless
    // an earlier header changed or disappeared; refresh separately invalidates
    // conclusions when newer relevant corpus changes affect the subject scope.
    if before
        .iter()
        .any(|source| !job.sources.contains(source) && relied_on(&job, source))
    {
        discovery_audit::invalidate(&mut job);
        capture_revalidation(&mut job, job_version)?;
        job.notes.clear();
        job.reviewed_sources.clear();
    }
    job.round += usize::from(!duplicate || changed);
    if !normalized.is_empty() || !targets.is_empty() {
        count_round(&mut job);
    }
    let change_coverage = job.coverage.clone();
    job.coverage = json!({"change_status":change_coverage["change_status"],"change_reason":change_coverage["change_reason"],"changed_source_count":change_coverage["changed_source_count"],"change_cursor":change_coverage["change_cursor"],"change_upper":change_coverage["change_upper"],"pending_source_refs":change_coverage["pending_source_refs"],"query_results":results.iter().map(|r|json!({"id":r["id"],"returned":r["candidates"].as_array().map_or(0,Vec::len),"query_status":r.get("query_status").cloned().unwrap_or(json!("complete"))})).collect::<Vec<_>>(),
        "source_cap_reached":job.sources.len()>=MAX_SOURCES,"unresolved_targets":unresolved,
        "meaning":"Bounded matching evidence; unresolved targets and search limits are not proof of absence."});
    if let Some(audit) = change_coverage.get("discovery_audit") {
        job.coverage["discovery_audit"] = audit.clone();
    }
    if !duplicate && !normalized.is_empty() {
        discovery_audit::record(
            &mut tx,
            &auth,
            &search_basis,
            &mut job,
            job_version,
            searched_generation,
            normalized,
            &results,
            search_results_current,
        )
        .await?;
    }
    if job.discoveries.len() >= 64 {
        job.discoveries.remove(0);
    }
    job.discoveries
        .push(json!({"query_hash":query_hash,"generation":upper}));
    remember(
        &mut job.receipts,
        &auth,
        &operation_id,
        &hash,
        json!({"round":job.round,"no_op":duplicate&&!changed}),
    );
    save(&state, &mut tx, &auth, &job, job_version).await?;
    let version = save_state(&state, &mut tx, &auth, &data, version).await?;
    let response = admission_response(&mut tx, &auth, &data, version).await?;
    tx.commit().await?;
    Ok(Json(json!({"data":response,"no_op":duplicate&&!changed})))
}

/// Routing/discovery alone never disposes historical input. A terminal research
/// response can acknowledge only the exact inputs it actually reviewed.
pub(super) fn validate_processed(
    job: &Job,
    processed: &[Value],
    progress: Option<&Value>,
) -> ApiResult<()> {
    if processed.is_empty() {
        return Ok(());
    }
    let reviewed: Vec<Source> = serde_json::from_value(
        progress
            .and_then(|v| v.get("reviewed_sources"))
            .cloned()
            .unwrap_or(json!([])),
    )?;
    for p in processed {
        let reference = string(p, "entry_ref")?;
        let version = integer(p, "version")?;
        if !job
            .sources
            .iter()
            .any(|s| s.entry_ref == reference && s.version == version)
            || !reviewed
                .iter()
                .any(|s| s.entry_ref == reference && s.version == version)
        {
            return Err(ApiError::invalid(
                "processed research input must be admitted and explicitly reviewed at its exact version",
            ));
        }
    }
    Ok(())
}

//! Bounded custody of independently unfinished immutable notebook units.
//! Projection is historical context only; admission and source validation stay
//! in the ordinary research transaction.
use super::*;

pub use crate::dreamer::research::CHECKPOINT_PROTOCOL as PROTOCOL;
use crate::dreamer::research::CheckpointIdentity as Origin;
const MAX_UNITS: usize = 4;
const MAX_BYTES: usize = 96 * 1024;

pub(in crate::dreamer_review) fn capacity() -> ApiError {
    repair_error(
        ApiError::invalid(
            "Research checkpoint capacity is full; reconcile offered unfinished checkpoints before saving more work",
        ),
        RepairPhase::CheckpointValidation,
    )
}

fn invalid(message: &str) -> ApiError {
    repair_error(
        ApiError::invalid(message),
        RepairPhase::CheckpointValidation,
    )
}

fn head(job: &Job) -> bool {
    !job.notes.trim().is_empty() && !job.reviewed_sources.is_empty()
}

pub(in crate::dreamer_review) fn unresolved(job: &Job) -> bool {
    job.schema == "dream.research.v2"
        && job
            .checkpoint_versions
            .as_ref()
            .is_some_and(|versions| !versions.is_empty())
}

fn versions(job: &Job, version: i64) -> ApiResult<Vec<i64>> {
    let result = if job.schema == "dream.research.v2" {
        job.checkpoint_versions.clone().ok_or_else(capacity)?
    } else {
        match job.revalidation_checkpoint {
            Some(RevalidationCheckpoint::Retained { version }) => vec![version],
            _ => Vec::new(),
        }
    };
    let unique = result.iter().copied().collect::<BTreeSet<_>>();
    if result.len() > MAX_UNITS
        || result.len() != unique.len()
        || result.iter().any(|prior| *prior < 1 || *prior >= version)
    {
        return Err(capacity());
    }
    Ok(result)
}

async fn notebook_id(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
) -> ApiResult<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM brunn.entries WHERE user_id=$1 AND path=$2 AND deleted_at IS NULL",
    )
    .bind(auth.user_id.0)
    .bind(path(&job.subject_ref)?)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| invalid("Research notebook is unavailable"))
}

async fn metadata(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    id: Uuid,
    version: i64,
) -> ApiResult<Value> {
    sqlx::query_scalar(
        "SELECT metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3",
    )
    .bind(auth.user_id.0)
    .bind(id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| invalid("Retained research checkpoint is unavailable"))
}

/// Reserve growth from operational version/round changes and discovery groups.
/// Notes/selectors/leads may grow only through a separately reserved semantic
/// write. This prevents a later refresh from needing an unreserved extra slot.
fn reserved(mut context: Value) -> Value {
    context["origin"]["version"] = json!(i64::MAX);
    context["origin"]["snapshot_generation"] = json!(i64::MAX);
    context["prior_progress"] = json!({"round":u64::MAX,"admitted_source_count":MAX_SOURCES,
        "reviewed_selector_count":64,"source_cap_reached":false,
        "latest_discovery_groups":(0..12).map(|_|json!({"id":"\0".repeat(64),"returned":8,"query_status":"\0".repeat(64)})).collect::<Vec<_>>()});
    context
}

pub(in crate::dreamer_review) async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    next_version: i64,
) -> ApiResult<()> {
    if job.schema != "dream.research.v2" {
        return Ok(());
    }
    let prior = versions(job, next_version)?;
    if prior.len() + usize::from(head(job)) > MAX_UNITS {
        return Err(capacity());
    }
    let id = notebook_id(tx, auth, job).await?;
    let mut contexts = Vec::new();
    for version in prior {
        let metadata = metadata(tx, auth, id, version).await?;
        let context = crate::dreamer_summary::research_checkpoint_projection(
            &metadata,
            &job.subject_ref,
            id,
            version,
        )
        .ok_or_else(capacity)?;
        contexts.push(reserved(context));
    }
    if head(job) {
        let metadata = json!({"kind":"dreamer_research","dreamer_research":job});
        let context = crate::dreamer_summary::research_checkpoint_projection(
            &metadata,
            &job.subject_ref,
            id,
            next_version,
        )
        .ok_or_else(capacity)?;
        contexts.push(reserved(context));
    }
    // Include the protocol envelope as well as every unit, even unavailable
    // ones. Access denial must never make a checkpoint occupy zero capacity.
    if serde_json::to_vec(
        &json!({"checkpoint_protocol":PROTOCOL,"checkpoint_context_status":"unavailable",
        "checkpoint_contexts":contexts,"current_checkpoint":null}),
    )?
    .len()
        > MAX_BYTES
    {
        return Err(capacity());
    }
    Ok(())
}

// Historical units are one all-or-nothing collection. A fresh, independently
// audited current head remains usable while that collection is withheld.
async fn offered(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
    current_fresh: bool,
) -> ApiResult<(Option<Vec<Value>>, Option<Value>)> {
    let Ok(prior) = versions(job, version) else {
        return Ok((None, None));
    };
    if prior.len() + usize::from(head(job)) > MAX_UNITS {
        return Ok((None, None));
    }
    let mut contexts = Some(Vec::new());
    for origin in prior {
        match crate::dreamer_summary::research_revalidation_context(
            tx,
            auth,
            &path(&job.subject_ref)?,
            &job.subject_ref,
            origin,
        )
        .await?
        {
            Some(context) => contexts
                .as_mut()
                .expect("available collection")
                .push(context),
            None => {
                contexts = None;
                break;
            }
        }
    }
    let mut current = None;
    if head(job) && (current_fresh || contexts.is_some()) {
        let context = crate::dreamer_summary::research_revalidation_context(
            tx,
            auth,
            &path(&job.subject_ref)?,
            &job.subject_ref,
            version,
        )
        .await?;
        if current_fresh {
            current = context;
        } else if let Some(context) = context {
            contexts
                .as_mut()
                .expect("available collection")
                .push(context);
        } else {
            contexts = None;
        }
    }
    let all = json!({"checkpoint_protocol":PROTOCOL,"checkpoint_context_status":"unavailable",
        "checkpoint_contexts":contexts,"current_checkpoint":current});
    if serde_json::to_vec(&all)?.len() > MAX_BYTES {
        return Ok((None, None));
    }
    Ok((contexts, current))
}

pub(in crate::dreamer_review) async fn project(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
    current_fresh: bool,
    view: &mut Value,
) -> ApiResult<()> {
    let (contexts, current) = offered(tx, auth, job, version, current_fresh).await?;
    view["checkpoint_protocol"] = json!(PROTOCOL);
    view["checkpoint_context_status"] = json!(if contexts.is_some() {
        "available"
    } else {
        "unavailable"
    });
    view["checkpoint_contexts"] = json!(contexts.unwrap_or_default());
    view["current_checkpoint"] = current.map_or(Value::Null, |context| context["origin"].clone());
    if job.schema == "dream.research.v2" {
        view["revalidation_context"] = Value::Null;
    }
    Ok(())
}

fn selectors(job: &Job) -> BTreeMap<(String, i64), BTreeSet<usize>> {
    let mut result = BTreeMap::<_, BTreeSet<_>>::new();
    for source in &job.reviewed_sources {
        result
            .entry((source.entry_ref.clone(), source.version))
            .or_default()
            .extend(source.start_line..=source.end_line);
    }
    result
}

fn semantic(job: &Job) -> Value {
    json!({"notes":job.notes,"reviewed":selectors(job).into_iter().map(|(identity,lines)|json!([identity,lines])).collect::<Vec<_>>(),
        "pending_queries":job.pending_queries.iter().collect::<BTreeSet<_>>(),"pending_targets":job.pending_targets.iter().collect::<BTreeSet<_>>()})
}

/// Called after ordinary evidence validation, before committing any outcome.
/// `retire` is false for zero-candidate results; no request can discharge work
/// merely by returning a structurally valid but unaccepted submission.
pub(in crate::dreamer_review) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    before: &Job,
    job: &mut Job,
    version: i64,
    body: &Value,
    retire: bool,
    completing: bool,
) -> ApiResult<Option<Value>> {
    let protocol = body.get("checkpoint_protocol");
    if protocol.is_some_and(|value| value != PROTOCOL) {
        return Err(invalid("Unsupported research checkpoint protocol"));
    }
    if protocol.is_none() && body.get("reconciled_checkpoints").is_some() {
        return Err(invalid("Checkpoint reconciliation requires its protocol"));
    }
    if protocol.is_none() && before.schema != "dream.research.v2" {
        return Ok(None);
    }
    let requested: Vec<Origin> = serde_json::from_value(
        body.get("reconciled_checkpoints")
            .cloned()
            .unwrap_or(json!([])),
    )
    .map_err(|_| invalid("Checkpoint reconciliation requires exact offered origins"))?;
    if requested.len() > MAX_UNITS
        || requested
            .iter()
            .enumerate()
            .any(|(index, origin)| requested[..index].contains(origin))
    {
        return Err(invalid(
            "Checkpoint reconciliation must name at most four distinct offered origins",
        ));
    }
    if !requested.is_empty()
        && !body["findings"].as_array().is_some_and(|findings| {
            findings.iter().any(|finding| {
                finding
                    .as_str()
                    .is_some_and(|finding| !finding.trim().is_empty())
            })
        })
    {
        return Err(invalid(
            "Checkpoint reconciliation requires an explicit finding",
        ));
    }
    if !requested.is_empty()
        && retire
        && !head(job)
        && (job.status != "no_change" || job.reviewed_sources.is_empty())
    {
        return Err(invalid(
            "Checkpoint reconciliation requires accepted current research notes and selectors",
        ));
    }
    let mut basis = before.clone();
    initialize_revalidation(tx, auth, &mut basis, version).await?;
    let mut prior = versions(&basis, version)?;
    let mut accepted = Vec::new();
    if !requested.is_empty() {
        let current_fresh = fresh(tx, auth, &basis).await?;
        let (contexts, current) = offered(tx, auth, &basis, version, current_fresh).await?;
        let origins = contexts
            .iter()
            .flatten()
            .chain(current.iter())
            .map(|context| serde_json::from_value::<Origin>(context["origin"].clone()))
            .collect::<Result<Vec<_>, _>>()?;
        if requested.iter().any(|origin| !origins.contains(origin)) {
            return Err(invalid(
                "Checkpoint reconciliation must name exact currently offered origins",
            ));
        }
        if retire {
            accepted = requested;
        }
    }
    let changed = semantic(before) != semantic(job);
    if head(before) && changed && !accepted.iter().any(|origin| origin.version == version) {
        prior.push(version);
    }
    prior.retain(|version| !accepted.iter().any(|origin| origin.version == *version));
    prior.sort_unstable();
    prior.dedup();
    // A no-change finding may complete the newly validated head, but every
    // earlier current/historical unit must be explicitly accounted for.
    if job.status == "no_change"
        && (!prior.is_empty()
            || (head(before) && !accepted.iter().any(|origin| origin.version == version)))
    {
        return Err(invalid(
            "No-change must explicitly reconcile all unfinished checkpoints",
        ));
    }
    job.schema = "dream.research.v2".into();
    job.checkpoint_versions = Some(prior);
    job.revalidation_checkpoint = Some(RevalidationCheckpoint::None);
    let old_coverage = selectors(before);
    let new_coverage = selectors(job).iter().any(|(identity, lines)| {
        old_coverage
            .get(identity)
            .is_none_or(|old| !lines.is_subset(old))
    });
    let old_count = versions(&basis, version)?.len() + usize::from(head(before));
    let new_count = job.checkpoint_versions.as_ref().map_or(0, Vec::len) + usize::from(head(job));
    let complete = completing
        && retire
        && !unresolved(job)
        && (!head(before) || accepted.iter().any(|origin| origin.version == version));
    if completing && !complete {
        job.status = "researching".into();
        job.retry_at = Utc::now();
    } else if complete && job.status != "no_change" {
        job.status = "waiting".into();
        job.retry_at = Utc::now() + Duration::hours(24);
    }
    reserve(tx, auth, job, version + 1).await?;
    Ok(protocol.map(|_| {
        json!({"protocol":PROTOCOL,"recorded":true,"replayed":false,
        "new_source_coverage":new_coverage,"reconciled":old_count>new_count,
        "subject_complete":complete})
    }))
}

pub(in crate::dreamer_review) fn replay(receipt: &mut Value) {
    if receipt.is_object() {
        receipt["replayed"] = json!(true);
        receipt["new_source_coverage"] = json!(false);
        receipt["reconciled"] = json!(false);
    }
}

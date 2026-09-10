//! One unaccepted candidate in protected immutable custody. This is neither a
//! Review item nor current evidence; original authority is never rebound.
use super::*;

pub(in crate::dreamer_review) use crate::dreamer::research::DRAFT_PROTOCOL as PROTOCOL;
use crate::dreamer::research::DraftPointer as Pointer;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_DELTA: usize = 16;
const MAX_DRAFT_RECEIPTS: usize = 8;

fn contract_error(message: &str) -> ApiError {
    repair_error(
        ApiError::invalid(message),
        RepairPhase::CheckpointValidation,
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    entry_ref: String,
    version: i64,
    snapshot_generation: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: String,
    subject_ref: String,
    status: String,
    candidate: Value,
    candidate_hash: String,
    custody_version: i64,
    origin: Origin,
    historical_versions: Vec<i64>,
    replaces: Option<Pointer>,
    findings: Vec<String>,
    retired_by: Option<Value>,
    receipts: Vec<Value>,
}

pub(in crate::dreamer_review) struct Active {
    record: Record,
    id: Uuid,
    head_version: i64,
}

fn draft_path(reference: &str) -> ApiResult<String> {
    Ok(format!(
        "dreams/reviews/research-draft-{}.md",
        entry_id(reference)?
    ))
}

fn pointer(active: &Active) -> Pointer {
    Pointer {
        entry_ref: format!("entry:{}", active.id),
        version: active.record.custody_version,
        candidate_hash: active.record.candidate_hash.clone(),
    }
}

fn protocol(body: &Value) -> ApiResult<()> {
    if body["draft_protocol"] != PROTOCOL {
        return Err(ApiError::invalid(
            "draft custody requires its advertised protocol",
        ));
    }
    Ok(())
}

fn candidate(value: &Value, job: &Job) -> ApiResult<Candidate> {
    if serde_json::to_vec(value)?.len() > MAX_CANDIDATE_BYTES {
        return Err(ApiError::invalid("draft candidate exceeds 32 KiB"));
    }
    let candidate: Candidate = serde_json::from_value(value.clone())
        .map_err(|_| ApiError::invalid("draft candidate fields are invalid"))?;
    if !matches!(candidate.kind.as_str(), "summary" | "related" | "question")
        || candidate.subject_ref.as_deref() != Some(job.subject_ref.as_str())
        || candidate.subject_scope.is_some()
        || candidate.evidence_scope.is_some()
        || !candidate.raw_sources.is_empty()
        || candidate.sources.len() > 64
        || candidate.sources.iter().any(|source| {
            source.start_line == 0
                || source.end_line < source.start_line
                || source.end_line - source.start_line > 400
                || !source.path.is_empty()
                || !source.excerpt.is_empty()
                || !job.sources.iter().any(|head| {
                    head.entry_ref == source.entry_ref && head.version == source.version
                })
        })
        || (candidate.kind == "summary"
            && (candidate.path.as_deref() != Some(job.output_path.as_str())
                || !candidate
                    .sources
                    .iter()
                    .any(|source| source.entry_ref == job.subject_ref)))
    {
        return Err(ApiError::invalid(
            "draft requires its selected subject, admitted exact selectors and ordinary destination",
        ));
    }
    // Citation formatting, target CAS and current freshness remain exclusively
    // candidate acceptance checks. Custody never certifies the proposed facts.
    Ok(candidate)
}

async fn load_active(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    reference: &str,
) -> ApiResult<Option<Active>> {
    let Some(entry) = load_entry(tx, auth.user_id.0, &draft_path(reference)?).await? else {
        return Ok(None);
    };
    if serde_json::to_vec(&entry.metadata)?.len() > MAX_RECORD_BYTES {
        return Err(ApiError::invalid("retained draft record exceeds its bound"));
    }
    let record: Record =
        serde_json::from_value(entry.metadata["dreamer_review"].clone()).map_err(|_| {
            ApiError::invalid("retained draft record is invalid; custody was preserved")
        })?;
    if record.schema != PROTOCOL
        || record.subject_ref != reference
        || !matches!(record.status.as_str(), "unaccepted" | "retired")
        || record.custody_version < 1
        || record.custody_version > entry.version
        || record.candidate_hash != digest(&record.candidate)
        || record.origin.version < 1
        || record.origin.snapshot_generation < 0
        || record.historical_versions.len() > 4
        || record
            .historical_versions
            .iter()
            .any(|v| *v < 1 || *v >= record.origin.version)
        || record
            .historical_versions
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != record.historical_versions.len()
        || record.receipts.len() > MAX_DRAFT_RECEIPTS
    {
        return Err(ApiError::invalid(
            "retained draft identity is invalid; custody was preserved",
        ));
    }
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM brunn.entries WHERE user_id=$1 AND path=$2 AND deleted_at IS NULL",
    )
    .bind(auth.user_id.0)
    .bind(draft_path(reference)?)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(Active {
        record,
        id,
        head_version: entry.version,
    }))
}

async fn authority(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    active: &Active,
) -> ApiResult<Option<Job>> {
    let record = &active.record;
    let Some((id, metadata)) = crate::dreamer_summary::research_notebook_authority(
        tx,
        auth,
        &path(&record.subject_ref)?,
        &record.subject_ref,
        record.origin.version,
    )
    .await?
    else {
        return Ok(None);
    };
    let frozen: Job = serde_json::from_value(metadata["dreamer_research"].clone())?;
    if record.origin.entry_ref != format!("entry:{id}")
        || frozen.snapshot_generation != record.origin.snapshot_generation
        || candidate(&record.candidate, &frozen).is_err()
    {
        return Ok(None);
    }
    for version in &record.historical_versions {
        if crate::dreamer_summary::research_revalidation_context(
            tx,
            auth,
            &path(&record.subject_ref)?,
            &record.subject_ref,
            *version,
        )
        .await?
        .is_none()
        {
            return Ok(None);
        }
    }
    Ok(Some(frozen))
}

async fn safe_view(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    active: &Active,
) -> ApiResult<Value> {
    let Some(frozen) = authority(tx, auth, active).await? else {
        return Ok(json!({"status":"unavailable"}));
    };
    // The active job may itself await refresh. Never expose cached identities
    // introduced after the draft when those newer sources have lost access.
    let ids = job
        .sources
        .iter()
        .map(|source| entry_id(&source.entry_ref))
        .collect::<ApiResult<Vec<_>>>()?;
    let visible = headers(tx, auth, &ids, job.snapshot_generation).await?;
    let current_sources = visible
        .into_iter()
        .filter(|source| job.sources.contains(source))
        .collect::<Vec<_>>();
    let mut changed = Vec::new();
    let mut new = Vec::new();
    let mut missing = Vec::new();
    let mut count = 0;
    let mut truncated = false;
    for current in &current_sources {
        let original = frozen
            .sources
            .iter()
            .find(|old| old.entry_ref == current.entry_ref);
        if original.is_none() || original.is_some_and(|old| old != current) {
            if count == MAX_DELTA {
                truncated = true;
                continue;
            }
            count += 1;
            if let Some(old) = original {
                changed.push(json!({"entry_ref":current.entry_ref,"path":current.path,
                    "from_version":old.version,"to_version":current.version}));
            } else {
                new.push(json!({"entry_ref":current.entry_ref,"path":current.path,"version":current.version}));
            }
        }
    }
    for old in &frozen.sources {
        if !current_sources
            .iter()
            .any(|current| current.entry_ref == old.entry_ref)
        {
            if count == MAX_DELTA {
                truncated = true;
                continue;
            }
            count += 1;
            missing.push(json!({"entry_ref":old.entry_ref,"path":old.path,"version":old.version}));
        }
    }
    Ok(
        json!({"status":"unaccepted_revalidation_only","pointer":pointer(active),
        "candidate":active.record.candidate,"origin":active.record.origin,
        "source_delta":{"changed":changed,"new":new,"missing":missing,"truncated":truncated,
        "coverage_complete":!truncated && job.snapshot_generation>=frozen.snapshot_generation && fresh(tx,auth,job).await?}}),
    )
}

pub(super) async fn project(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    view: &mut Value,
) -> ApiResult<()> {
    view["draft_protocol"] = json!(PROTOCOL);
    view["unaccepted_draft"] = match load_active(tx, auth, &job.subject_ref).await? {
        Some(active) if active.record.status == "unaccepted" => {
            safe_view(tx, auth, job, &active).await?
        }
        _ => Value::Null,
    };
    Ok(())
}

fn ack(operation_id: &str, pointer: &Pointer, retired: bool) -> Value {
    json!({"protocol":PROTOCOL,"operation_id":operation_id,"recorded":true,
        "replayed":false,"pointer":pointer,"retired":retired})
}

pub(in crate::dreamer_review) fn replay(value: &mut Value) {
    if value.is_object() {
        value["replayed"] = json!(true);
    }
}

async fn write_record(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    record: &Record,
    expected: i64,
) -> ApiResult<Value> {
    let metadata = json!({"kind":"dreamer_review","dreamer_review":record});
    if serde_json::to_vec(&metadata)?.len() > MAX_RECORD_BYTES {
        return Err(contract_error(
            "draft custody record exceeds 64 KiB; prior work remains retained",
        ));
    }
    put_entry(state,tx,auth,&draft_path(&record.subject_ref)?,
        "# Unaccepted research draft\n\nProtected candidate custody. This is not a Review proposal or accepted evidence.\n".into(),metadata,expected).await
}

pub(super) async fn custody(
    state: &AppState,
    auth: &AuthContext,
    body: &Value,
) -> ApiResult<Value> {
    protocol(body)?;
    let allowed = [
        "attempt_id",
        "fence",
        "expected_state_version",
        "operation_id",
        "subject_ref",
        "research_version",
        "draft_protocol",
        "draft_candidate",
        "replaces_draft",
        "findings",
        "processed_inputs",
    ];
    if body
        .as_object()
        .is_none_or(|object| object.keys().any(|key| !allowed.contains(&key.as_str())))
        || body["processed_inputs"] != json!([])
    {
        return Err(ApiError::invalid(
            "draft custody cannot change notebook evidence or dispose work",
        ));
    }
    let reference = string(body, "subject_ref")?;
    let (operation_id, hash) = operation(body, "draft-custody")?;
    let mut tx = begin_runner_write(state, auth).await?;
    if mode(&mut tx, auth.user_id.0).await?.is_none() {
        return Err(ApiError::invalid("CONTROL is paused"));
    }
    let (data, state_version) = load_state(&mut tx, auth.user_id.0).await?;
    let (job, job_version) = load(&mut tx, auth, reference)
        .await?
        .ok_or_else(|| ApiError::invalid("research subject is not retained"))?;
    let replayed = receipt(
        &mut tx,
        auth,
        &draft_path(reference)?,
        "dreamer_review",
        &operation_id,
        &hash,
    )
    .await?;
    checked_attempt(&data, body, auth, state_version, replayed.is_some())?;
    if let Some(replayed) = replayed {
        let current = load_active(&mut tx, auth, reference)
            .await?
            .ok_or_else(|| ApiError::invalid("retained draft receipt is unavailable"))?;
        let recorded = &replayed["result"];
        let pointer = Pointer {
            entry_ref: format!("entry:{}", current.id),
            version: integer(recorded, "draft_version")?,
            candidate_hash: string(recorded, "candidate_hash")?.into(),
        };
        let mut response = admission_response(&mut tx, auth, &data, state_version).await?;
        let mut receipt = ack(&operation_id, &pointer, false);
        replay(&mut receipt);
        response["draft_receipt"] = receipt;
        return Ok(json!({"data":response,"no_op":true}));
    }
    check_job(&data, body, &job, job_version)?;
    // Only model-controlled contract failures are repairable, and only after
    // operation replay and the current attempt/state/notebook fences passed.
    let findings: Vec<String> =
        serde_json::from_value(body.get("findings").cloned().unwrap_or(json!([])))
            .map_err(|_| contract_error("draft incorporation findings must be strings"))?;
    if serde_json::to_vec(&findings)?.len() > 16_000 {
        return Err(contract_error("draft incorporation findings exceed 16 KiB"));
    }
    candidate(&body["draft_candidate"], &job)
        .map_err(|error| repair_error(error, RepairPhase::CheckpointValidation))?;
    let old = load_active(&mut tx, auth, reference).await?;
    let replaces: Option<Pointer> = body
        .get("replaces_draft")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()?;
    let raw_hash = digest(&body["draft_candidate"]);
    let expected = old.as_ref().map_or(0, |active| active.head_version);
    let mut record = if let Some(active) = &old
        && active.record.status == "unaccepted"
    {
        if authority(&mut tx, auth, active).await?.is_none() {
            return Err(ApiError::invalid(
                "retained draft is unavailable; preserve it and use ordinary candidate validation",
            ));
        }
        if raw_hash == active.record.candidate_hash {
            if replaces
                .as_ref()
                .is_some_and(|value| value != &pointer(active))
            {
                return Err(ApiError::invalid(
                    "draft replacement must name the exact offered pointer",
                ));
            }
            active.record.clone()
        } else {
            if replaces.as_ref() != Some(&pointer(active)) {
                return Err(ApiError::invalid(
                    "replace a draft only with its exact offered pointer",
                ));
            }
            new_record(
                &mut tx,
                auth,
                &job,
                job_version,
                body,
                raw_hash,
                expected + 1,
                replaces,
                findings,
            )
            .await?
        }
    } else {
        if replaces.is_some() {
            return Err(ApiError::invalid(
                "no active draft matches the replacement pointer",
            ));
        }
        new_record(
            &mut tx,
            auth,
            &job,
            job_version,
            body,
            raw_hash,
            expected + 1,
            None,
            findings,
        )
        .await?
    };
    if record.receipts.len() >= MAX_DRAFT_RECEIPTS {
        record.receipts.remove(0);
    }
    remember(
        &mut record.receipts,
        auth,
        &operation_id,
        &hash,
        json!({"draft_version":record.custody_version,"candidate_hash":record.candidate_hash}),
    );
    let written = write_record(state, &mut tx, auth, &record, expected).await?;
    let pointer = Pointer {
        entry_ref: string(&written, "entry_ref")?.into(),
        version: record.custody_version,
        candidate_hash: record.candidate_hash.clone(),
    };
    let mut response = admission_response(&mut tx, auth, &data, state_version).await?;
    response["draft_receipt"] = ack(&operation_id, &pointer, false);
    tx.commit().await?;
    Ok(json!({"data":response}))
}

#[allow(clippy::too_many_arguments)]
async fn new_record(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    job_version: i64,
    body: &Value,
    hash: String,
    custody_version: i64,
    replaces: Option<Pointer>,
    findings: Vec<String>,
) -> ApiResult<Record> {
    let id: Uuid = sqlx::query_scalar("SELECT id FROM brunn.entries WHERE user_id=$1 AND path=$2 AND deleted_at IS NULL AND current_version=$3")
        .bind(auth.user_id.0).bind(path(&job.subject_ref)?).bind(job_version).fetch_one(&mut **tx).await?;
    let historical_versions = if job.schema == "dream.research.v2" {
        job.checkpoint_versions
            .clone()
            .ok_or_else(|| ApiError::invalid("research checkpoint authority is invalid"))?
    } else {
        match job.revalidation_checkpoint {
            Some(RevalidationCheckpoint::Retained { version }) => vec![version],
            _ => Vec::new(),
        }
    };
    if historical_versions.len() > 4
        || historical_versions
            .iter()
            .any(|v| *v < 1 || *v >= job_version)
        || historical_versions.iter().collect::<BTreeSet<_>>().len() != historical_versions.len()
    {
        return Err(ApiError::invalid(
            "draft historical authority exceeds its bounds",
        ));
    }
    Ok(Record {
        schema: PROTOCOL.into(),
        subject_ref: job.subject_ref.clone(),
        status: "unaccepted".into(),
        candidate: body["draft_candidate"].clone(),
        candidate_hash: hash,
        custody_version,
        origin: Origin {
            entry_ref: format!("entry:{id}"),
            version: job_version,
            snapshot_generation: job.snapshot_generation,
        },
        historical_versions,
        replaces,
        findings,
        retired_by: None,
        receipts: Vec::new(),
    })
}

pub(in crate::dreamer_review) async fn validate_submission(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    body: &Value,
) -> ApiResult<Option<Active>> {
    if body.get("draft_pointer").is_none() && body.get("draft_protocol").is_none() {
        return Ok(None);
    }
    protocol(body)?;
    let requested: Pointer = serde_json::from_value(body["draft_pointer"].clone())?;
    let active = load_active(tx, auth, &job.subject_ref)
        .await?
        .ok_or_else(|| ApiError::invalid("submitted draft is not retained"))?;
    if active.record.status != "unaccepted"
        || requested != pointer(&active)
        || body["candidates"]
            .as_array()
            .is_none_or(|values| values.len() != 1 || values[0] != active.record.candidate)
        || authority(tx, auth, &active).await?.is_none()
    {
        return Err(ApiError::invalid(
            "candidate must match the exact available draft in custody",
        ));
    }
    Ok(Some(active))
}

pub(in crate::dreamer_review) async fn accepted(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    active: Option<Active>,
    ids: &[String],
    operation_id: &str,
) -> ApiResult<Option<Value>> {
    let Some(mut active) = active else {
        return Ok(None);
    };
    let pointer = pointer(&active);
    if !ids.is_empty() {
        active.record.status = "retired".into();
        active.record.retired_by =
            Some(json!({"operation_id":operation_id,"accepted_candidate_ids":ids}));
        write_record(state, tx, auth, &active.record, active.head_version).await?;
    }
    Ok(Some(ack(operation_id, &pointer, !ids.is_empty())))
}

//! Existing proposals are comparison context, never another evidence source.
//! Follow-ups retain identities only and are independent of model-written notes.
use super::*;
use std::collections::BTreeSet;
mod source_routes;
pub(super) use source_routes::{prepare_resolutions, resolve_sources};

pub(super) const FOLLOW_UP_PROTOCOL: &str = "dream.research.follow_up.v1";

const MAX_COMPARISONS: usize = 4;
const MAX_COMPARISON_BYTES: usize = 64 * 1024;
const MAX_FOLLOW_UPS: usize = 32;
const MAX_TARGETS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pointer {
    item_id: String,
    candidate_hash: String,
    run_entry_ref: String,
    run_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InputIdentity {
    entry_ref: String,
    version: i64,
    generation: i64,
}

impl InputIdentity {
    fn matches(&self, input: &Input) -> bool {
        self.entry_ref == input.entry_ref
            && self.version == input.version
            && self.generation == input.generation
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceIdentity {
    entry_ref: String,
    version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteIdentity {
    route_id: Uuid,
    revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "FollowUpWire")]
pub(super) struct FollowUp {
    comparison: Pointer,
    subject_ref: String,
    origin_subject_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_input: Option<InputIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_source: Option<SourceIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<RouteIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_snapshot_generation: Option<i64>,
    targets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FollowUpWire {
    comparison: Pointer,
    subject_ref: String,
    origin_subject_ref: String,
    #[serde(default)]
    origin_input: Option<InputIdentity>,
    #[serde(default)]
    origin_source: Option<SourceIdentity>,
    #[serde(default)]
    route: Option<RouteIdentity>,
    #[serde(default)]
    origin_snapshot_generation: Option<i64>,
    targets: Vec<String>,
}

impl TryFrom<FollowUpWire> for FollowUp {
    type Error = &'static str;

    fn try_from(value: FollowUpWire) -> Result<Self, Self::Error> {
        let event = value.origin_input.is_some()
            && value.origin_source.is_none()
            && value.route.is_none()
            && value.origin_snapshot_generation.is_none();
        let source = value.origin_input.is_none()
            && value
                .origin_source
                .as_ref()
                .is_some_and(|source| source.version > 0 && entry_id(&source.entry_ref).is_ok())
            && value.route.as_ref().is_some_and(|route| route.revision > 0)
            && value
                .origin_snapshot_generation
                .is_some_and(|generation| generation >= 0);
        if !event && !source {
            return Err("retained follow-up requires exactly one valid origin kind");
        }
        Ok(Self {
            comparison: value.comparison,
            subject_ref: value.subject_ref,
            origin_subject_ref: value.origin_subject_ref,
            origin_input: value.origin_input,
            origin_source: value.origin_source,
            route: value.route,
            origin_snapshot_generation: value.origin_snapshot_generation,
            targets: value.targets,
        })
    }
}

impl FollowUp {
    fn origin_ref(&self) -> &str {
        self.origin_input
            .as_ref()
            .map(|origin| origin.entry_ref.as_str())
            .or_else(|| {
                self.origin_source
                    .as_ref()
                    .map(|origin| origin.entry_ref.as_str())
            })
            .expect("validated follow-up origin")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FollowUpRequest {
    comparison: Pointer,
    #[serde(default)]
    origin_input: Option<InputIdentity>,
    #[serde(default)]
    origin_source: Option<SourceIdentity>,
    targets: Vec<String>,
}

fn pointer(item: &Item) -> Pointer {
    Pointer {
        item_id: item.id.clone(),
        candidate_hash: item.candidate_hash.clone(),
        run_entry_ref: item.run_entry_ref.clone(),
        run_version: item.run_version,
    }
}

fn optional<'a>(body: &'a Value, field: &str) -> Option<&'a Value> {
    body.get(field).filter(|value| !value.is_null())
}

pub(super) fn reject_candidate_fields(body: &Value) -> ApiResult<()> {
    if body.get("resolved_follow_ups").is_some() || body.get("follow_up_protocol").is_some() {
        return Err(ApiError::invalid(
            "source route acknowledgements belong in research_progress",
        ));
    }
    if [body, &body["research_progress"]].iter().any(|body| {
        optional(body, "covers_existing").is_some()
            || optional(body, "follow_up").is_some()
            || optional(body, "supersedes_existing").is_some()
    }) {
        return Err(ApiError::invalid(
            "comparison completion and follow-up routing require research-progress",
        ));
    }
    Ok(())
}

async fn never_reviewed(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    item: &Item,
) -> ApiResult<bool> {
    // Positive modern immutable-run provenance is required. Check every review
    // audit version, including deleted entries; compact history cannot prove
    // that the owner never reviewed an earlier version of this item identity.
    // Public upserts identify an entry by its normalized path, not by UUID;
    // case/NFC spelling changes cannot move it outside this managed namespace.
    // Select that small historical manifest before inspecting audit versions.
    let modern: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3 AND metadata @> $4)")
        .bind(auth.user_id.0).bind(entry_id(&item.run_entry_ref)?).bind(item.run_version)
        .bind(json!({"dreamer_run":{"schema":"dream.run.v1","accepted":true,"items":[{"id":item.id,"candidate_hash":item.candidate_hash}]}}))
        .fetch_one(&mut **tx).await?;
    if !modern {
        return Ok(false);
    }
    let reviewed: bool = sqlx::query_scalar(
        r#"
        WITH audit_entries AS MATERIALIZED (
            SELECT id FROM brunn.entries
            WHERE user_id=$1
              AND starts_with(lower(normalize(path,NFC)), 'dreams/reviews/')
        )
        SELECT EXISTS (
            SELECT 1 FROM audit_entries audit
            CROSS JOIN LATERAL (
                SELECT 1 FROM brunn.entry_versions
                WHERE user_id=$1 AND entry_id=audit.id AND metadata @> $2
                LIMIT 1
            ) reviewed
        )
    "#,
    )
    .bind(auth.user_id.0)
    .bind(json!({"dreamer_review":{"decision":{"item_id":item.id}}}))
    .fetch_one(&mut **tx)
    .await?;
    Ok(!reviewed)
}

fn score(item: &Item, job: &research::Job) -> Option<(bool, bool, usize)> {
    if item.status != "pending"
        || !item.reviewable
        || item.candidate.kind != "summary"
        || item.candidate.evidence_scope.is_some()
        || !item.candidate.raw_sources.is_empty()
        || item.run_version <= 0
    {
        return None;
    }
    let scope = item.candidate.subject_scope.as_ref()?;
    if item.candidate.subject_ref.as_deref() != Some(scope.subject_ref.as_str())
        || item.candidate.path.as_deref()
            != Some(format!("derived/entities/{}.md", entry_id(&scope.subject_ref).ok()?).as_str())
    {
        return None;
    }
    let overlap: BTreeSet<_> = item
        .candidate
        .sources
        .iter()
        .filter(|source| {
            job.sources
                .iter()
                .any(|input| source.entry_ref == input.entry_ref && source.version == input.version)
        })
        .map(|source| (&source.entry_ref, source.version))
        .collect();
    let canonical = overlap
        .iter()
        .any(|(reference, _)| reference.as_str() == job.subject_ref);
    // Reserve the selected subject's complete draft for whole-draft retirement
    // comparison even when several other proposals share the same evidence.
    (canonical || overlap.len() >= 2).then_some((
        canonical,
        scope.subject_ref == job.subject_ref,
        overlap.len(),
    ))
}

pub(super) async fn proposals(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    job: &research::Job,
) -> ApiResult<Vec<Value>> {
    let mut ranked: Vec<_> = data
        .items
        .iter()
        .filter_map(|item| score(item, job).map(|score| (score, item)))
        .collect();
    ranked.sort_by(|(left, a), (right, b)| right.cmp(left).then_with(|| a.id.cmp(&b.id)));
    let mut shown = Vec::new();
    let mut bytes = 0;
    for (_, item) in ranked {
        let Some(content) = item
            .candidate
            .content
            .as_ref()
            .filter(|text| !text.trim().is_empty())
        else {
            continue;
        };
        if content.len() > MAX_CANDIDATE_BYTES {
            continue;
        }
        if !item_available(tx, auth.user_id.0, item).await? || item_stale(tx, auth, item).await? {
            continue;
        }
        let scope = item
            .candidate
            .subject_scope
            .as_ref()
            .expect("ranked subject summary");
        let sources: Vec<_> = item.candidate.sources.iter().map(|source| json!({
            "entry_ref":source.entry_ref,"version":source.version,"start_line":source.start_line,
            "end_line":source.end_line,"path":source.path
        })).collect();
        let mut value = json!({"pointer":pointer(item),"subject_ref":scope.subject_ref,"retirable":false,
            "path":item.candidate.path,"title":item.candidate.title,"content":content,"sources":sources});
        let size = serde_json::to_vec(&value)?.len();
        // Never present a partial body as proof of coverage.
        if size > MAX_CANDIDATE_BYTES || bytes + size > MAX_COMPARISON_BYTES {
            continue;
        }
        value["retirable"] = json!(never_reviewed(tx, auth, item).await?);
        bytes += size;
        shown.push(value);
        if shown.len() == MAX_COMPARISONS {
            break;
        }
    }
    Ok(shown)
}

async fn checked_comparison(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    job: &research::Job,
    requested: &Pointer,
) -> ApiResult<Item> {
    let offered = proposals(tx, auth, data, job).await?;
    if !offered
        .iter()
        .any(|value| value["pointer"] == json!(requested))
    {
        return Err(ApiError::invalid(
            "comparison proposal changed, is unavailable, or does not overlap this subject; refresh before disposition",
        ));
    }
    data.items
        .iter()
        .find(|item| pointer(item) == *requested)
        .cloned()
        .ok_or_else(|| ApiError::invalid("comparison proposal is no longer retained"))
}

pub(super) async fn validate_progress(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    job: &research::Job,
    body: &Value,
    processed: &[Value],
) -> ApiResult<()> {
    source_routes::check_protocol(body)?;
    let covers = optional(body, "covers_existing");
    let follow_up = optional(body, "follow_up");
    let supersedes = optional(body, "supersedes_existing");
    if supersedes.is_some()
        && (covers.is_none() || follow_up.is_some() || job.status != "no_change")
    {
        return Err(ApiError::invalid(
            "supersedes_existing requires no_change with covers_existing",
        ));
    }
    if covers.is_some() && follow_up.is_some() {
        return Err(ApiError::invalid(
            "coverage completion and enrichment routing are separate dispositions",
        ));
    }
    if let Some(value) = covers {
        if job.status != "no_change" {
            return Err(ApiError::invalid(
                "covers_existing requires no_change progress",
            ));
        }
        let requested: Pointer = serde_json::from_value(value.clone())?;
        let item = checked_comparison(tx, auth, data, job, &requested).await?;
        let mut identities: Vec<_> = processed
            .iter()
            .map(|input| {
                Ok((
                    string(input, "entry_ref")?.to_owned(),
                    integer(input, "version")?,
                ))
            })
            .collect::<ApiResult<_>>()?;
        let canonical = job
            .sources
            .iter()
            .find(|source| source.entry_ref == job.subject_ref)
            .ok_or_else(|| ApiError::invalid("canonical research source is missing"))?;
        identities.push((canonical.entry_ref.clone(), canonical.version));
        if identities.iter().any(|(reference, version)| {
            !item
                .candidate
                .sources
                .iter()
                .any(|source| &source.entry_ref == reference && source.version == *version)
        }) {
            return Err(ApiError::invalid(
                "comparison must cite the canonical and every processed input at their exact versions",
            ));
        }
        if let Some(value) = supersedes {
            if serde_json::to_vec(&body["findings"])?.len() > 16_000 {
                return Err(ApiError::invalid(
                    "comparison retirement findings exceed 16 KiB",
                ));
            }
            let duplicate_pointer: Pointer = serde_json::from_value(value.clone())?;
            let mut duplicate = checked_comparison(tx, auth, data, job, &duplicate_pointer).await?;
            if data.research.follow_ups.iter().any(|route| {
                route.comparison.item_id == duplicate.id || route.subject_ref == job.subject_ref
            }) {
                return Err(ApiError::invalid(
                    "complete incoming enrichment before retiring its destination proposal",
                ));
            }
            if duplicate.id == item.id
                || duplicate.candidate.subject_ref.as_deref() != Some(job.subject_ref.as_str())
                || !never_reviewed(tx, auth, &duplicate).await?
                || body["findings"].as_array().is_none_or(|findings| {
                    !findings
                        .iter()
                        .any(|value| value.as_str().is_some_and(|text| !text.trim().is_empty()))
                })
                || duplicate.candidate.sources.iter().any(|source| {
                    !item.candidate.sources.iter().any(|cover| {
                        cover.entry_ref == source.entry_ref && cover.version == source.version
                    })
                })
            {
                return Err(ApiError::invalid(
                    "retirement requires a distinct never-reviewed selected-subject draft, whole-draft coverage finding, and exact covering citations",
                ));
            }
            duplicate.status = "superseded".into();
            let disposition = json!({"disposition":"superseded_by_comparison","duplicate":duplicate_pointer,
                "covering":requested,"findings":body["findings"],"at":Utc::now()});
            let audit_path = format!(
                "dreams/reviews/comparison-{}.md",
                string(body, "operation_id")?
            );
            put_entry(state, tx, auth, &audit_path, "# Proposal comparison disposition\n\nA never-reviewed duplicate was superseded; exact proposal versions remain retained.\n".into(),
                json!({"kind":"dreamer_review","dreamer_review":{"schema":"dream.review.v1","disposition":disposition,"item":duplicate}}), 0).await?;
            data.items
                .iter_mut()
                .find(|candidate| candidate.id == duplicate.id)
                .expect("checked retained draft")
                .status = "superseded".into();
            data.candidate_dispositions.push(disposition);
        }
    }
    if let Some(value) = follow_up {
        if job.status != "waiting" || !processed.is_empty() {
            return Err(ApiError::invalid(
                "follow_up requires waiting progress with no processed inputs",
            ));
        }
        let mut request: FollowUpRequest = serde_json::from_value(value.clone())?;
        if request.origin_input.is_some() == request.origin_source.is_some() {
            return Err(ApiError::invalid(
                "follow-up requires exactly one origin_input or origin_source",
            ));
        }
        if let Some(origin) = &mut request.origin_source {
            source_routes::require_protocol(body)?;
            origin.entry_ref = format!("entry:{}", entry_id(&origin.entry_ref)?);
            let reviewed: Vec<Source> = serde_json::from_value(body["reviewed_sources"].clone())?;
            if origin.version < 1
                || !job.sources.iter().any(|source| {
                    source.entry_ref == origin.entry_ref && source.version == origin.version
                })
                || !reviewed.iter().any(|source| {
                    source.entry_ref == origin.entry_ref && source.version == origin.version
                })
                || !source_routes::has_finding(body)
            {
                return Err(ApiError::invalid(
                    "follow-up source origin requires exact admitted reviewed evidence and an explicit finding",
                ));
            }
        }
        if request.targets.len() > MAX_TARGETS {
            return Err(ApiError::invalid("follow-up targets exceed 16 references"));
        }
        let item = checked_comparison(tx, auth, data, job, &request.comparison).await?;
        let destination = item
            .candidate
            .subject_ref
            .as_ref()
            .expect("checked subject")
            .clone();
        if destination == job.subject_ref {
            return Err(ApiError::invalid(
                "follow-up must target a different existing canonical subject",
            ));
        }
        let mut reachable = BTreeSet::from([destination.clone()]);
        loop {
            let before = reachable.len();
            for route in &data.research.follow_ups {
                if reachable.contains(&route.origin_subject_ref) {
                    reachable.insert(route.subject_ref.clone());
                }
            }
            if reachable.contains(&job.subject_ref) {
                return Err(ApiError::invalid(
                    "follow-up would create a retained routing cycle",
                ));
            }
            if reachable.len() == before {
                break;
            }
        }
        if let Some(origin) = &request.origin_input {
            if !data.inputs.iter().any(|input| origin.matches(input)) {
                return Err(ApiError::invalid(
                    "follow-up origin must be an exact retained input",
                ));
            }
            research::validate_processed(job, &[json!(origin)], Some(body))?;
        }
        // Waiting can safely checkpoint a stale job, but it cannot transfer a
        // conclusion about another proposal without checked primary evidence.
        if !research::fresh(tx, auth, job).await? {
            return Err(ApiError::invalid(
                "follow-up requires current exact research evidence",
            ));
        }
        let origin_ref = request
            .origin_input
            .as_ref()
            .map(|origin| &origin.entry_ref)
            .or_else(|| {
                request
                    .origin_source
                    .as_ref()
                    .map(|origin| &origin.entry_ref)
            })
            .expect("one checked origin");
        let mut targets = BTreeSet::new();
        for reference in request.targets.iter().chain(std::iter::once(origin_ref)) {
            let reference = format!("entry:{}", entry_id(reference)?);
            if !job
                .sources
                .iter()
                .any(|source| source.entry_ref == reference)
            {
                return Err(ApiError::invalid(
                    "follow-up targets must be admitted primary source references",
                ));
            }
            targets.insert(reference);
        }
        let ids = targets
            .iter()
            .map(|reference| entry_id(reference))
            .collect::<ApiResult<Vec<_>>>()?;
        let current = research::headers(tx, auth, &ids, job.snapshot_generation).await?;
        if current.len() != targets.len() || current.iter().any(|head| !job.sources.contains(head))
        {
            return Err(ApiError::invalid(
                "follow-up targets changed or are unavailable",
            ));
        }
        if data.research.follow_ups.iter().any(|route| {
            route.origin_ref() == origin_ref
                && route.origin_input.is_some() == request.origin_input.is_some()
                && route.subject_ref != destination
        }) {
            return Err(ApiError::invalid(
                "this exact input already has a retained follow-up destination",
            ));
        }
        if let Some(route) = data.research.follow_ups.iter_mut().find(|route| {
            route.origin_input == request.origin_input
                && route.origin_source == request.origin_source
                && route.subject_ref == destination
        }) {
            targets.extend(route.targets.iter().cloned());
            if targets.len() > MAX_TARGETS {
                return Err(ApiError::invalid(
                    "retained follow-up targets exceed 16 references",
                ));
            }
            let targets: Vec<_> = targets.into_iter().collect();
            if route.targets != targets {
                if let Some(identity) = &mut route.route {
                    identity.revision = identity
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| ApiError::invalid("follow-up revision is exhausted"))?;
                }
                route.targets = targets;
            }
        } else {
            if data.research.follow_ups.len() >= MAX_FOLLOW_UPS || targets.len() > MAX_TARGETS {
                return Err(ApiError::invalid(
                    "research follow-up queue is full; input remains retained",
                ));
            }
            data.research.follow_ups.push(FollowUp {
                comparison: request.comparison,
                subject_ref: destination.clone(),
                origin_subject_ref: job.subject_ref.clone(),
                route: request.origin_source.as_ref().map(|_| RouteIdentity {
                    route_id: Uuid::now_v7(),
                    revision: 1,
                }),
                origin_snapshot_generation: request
                    .origin_source
                    .as_ref()
                    .map(|_| job.snapshot_generation),
                origin_input: request.origin_input,
                origin_source: request.origin_source,
                targets: targets.into_iter().collect(),
            });
        }
        if !data.research.requested_subject_refs.contains(&destination) {
            if data.research.requested_subject_refs.len() >= 16 {
                return Err(ApiError::invalid(
                    "requested subject queue is full; follow-up and input remain unchanged",
                ));
            }
            data.research.follow_up_priorities.push(destination.clone());
            data.research.requested_subject_refs.push(destination);
        }
    }
    Ok(())
}

pub(super) fn retains_priority(data: &RunState, subject_ref: &str) -> bool {
    data.research
        .follow_ups
        .iter()
        .any(|route| route.subject_ref == subject_ref)
}

pub(super) fn retains_source_work(data: &RunState, subject_ref: &str) -> bool {
    data.research
        .follow_ups
        .iter()
        .any(|route| route.subject_ref == subject_ref && route.origin_source.is_some())
}

pub(super) fn replay_resolution(ack: &mut Value) {
    if ack.is_object() {
        ack["replayed"] = json!(true);
    }
}

/// A proven intake exclusion retires an obligation, not model work. Neither a
/// missing input nor temporary inability to read a source is such a proof.
pub(super) async fn retire_excluded(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
) -> ApiResult<()> {
    let mut retired = Vec::new();
    for route in &data.research.follow_ups {
        if route.origin_source.is_some() {
            if let Some(disposition) = source_routes::policy_exclusion(tx, auth, route).await? {
                let path = format!(
                    "dreams/reviews/research-exclusion-{}.md",
                    digest(&disposition)
                );
                if load_entry(tx, auth.user_id.0, &path).await?.is_none() {
                    put_entry(state, tx, auth, &path,
                        "# Research source exclusion\n\nAn explicit current source-policy exclusion retired routing without model completion.\n".into(),
                        json!({"kind":"dreamer_review","dreamer_review":{"schema":"dream.review.v1","disposition":disposition}}), 0).await?;
                }
                retired.push((route.clone(), disposition));
            }
            continue;
        }
        let origin_input = route.origin_input.as_ref().expect("event route");
        if data
            .inputs
            .iter()
            .any(|input| input.entry_ref == origin_input.entry_ref)
        {
            continue;
        }
        let Some(exclusion) = data.source_dispositions.iter().find(|disposition| {
            disposition["entry_ref"] == origin_input.entry_ref
                && matches!(
                    disposition["disposition"].as_str(),
                    Some(
                        "deleted_source"
                            | "excluded_generated_briefing"
                            | "excluded_credential_record"
                    )
                )
        }) else {
            continue;
        };
        let disposition = json!({"disposition":"excluded_by_source_policy","route":route,
            "source_disposition":exclusion,"model_processed":false});
        let path = format!(
            "dreams/reviews/research-exclusion-{}.md",
            digest(&disposition)
        );
        if load_entry(tx, auth.user_id.0, &path).await?.is_none() {
            put_entry(state, tx, auth, &path,
                "# Research source exclusion\n\nAn explicit source policy exclusion retired retained routing. No model completion or input processing is claimed.\n".into(),
                json!({"kind":"dreamer_review","dreamer_review":{"schema":"dream.review.v1","disposition":disposition}}), 0).await?;
        }
        retired.push((route.clone(), disposition));
    }
    data.research.follow_ups.retain(|route| {
        !retired.iter().any(|(old, _)| {
            route.origin_input == old.origin_input
                && route.origin_source == old.origin_source
                && route.subject_ref == old.subject_ref
        })
    });
    for (route, disposition) in retired {
        let destination = route.subject_ref;
        data.source_dispositions.push(disposition);
        if !retains_priority(data, &destination)
            && data.research.follow_up_priorities.contains(&destination)
        {
            data.research
                .requested_subject_refs
                .retain(|reference| reference != &destination);
            data.research
                .follow_up_priorities
                .retain(|reference| reference != &destination);
        }
    }
    Ok(())
}

pub(super) fn validate_legacy_processing(data: &RunState, processed: &[Value]) -> ApiResult<()> {
    if data.research.follow_ups.iter().any(|route| {
        route.origin_input.as_ref().is_some_and(|origin| {
            processed
                .iter()
                .any(|input| input["entry_ref"] == origin.entry_ref)
        })
    }) {
        return Err(ApiError::invalid(
            "routed inputs require disposition by their canonical research destination",
        ));
    }
    Ok(())
}

/// Only an accepted, exact primary-input disposition resolves a route. Current
/// replacement identities may reconcile an older version, never mere pruning.
pub(super) async fn resolve_processed(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    job: &research::Job,
    processed: &[Value],
    progress: Option<&Value>,
    accepted_ids: Option<&[String]>,
) -> ApiResult<Vec<Value>> {
    let reviewed: Vec<Source> = serde_json::from_value(
        progress
            .and_then(|value| value.get("reviewed_sources"))
            .cloned()
            .unwrap_or(json!([])),
    )?;
    let mut dispositions = Vec::new();
    // Do not consume the origin and leave a dangling route. A reviewed header
    // alone is insufficient evidence that an accepted overview uses it.
    for route in &data.research.follow_ups {
        let Some(origin_input) = &route.origin_input else {
            continue;
        };
        for input in processed
            .iter()
            .filter(|input| input["entry_ref"] == origin_input.entry_ref)
        {
            let destination = data
                .items
                .iter()
                .find(|item| item.id == route.comparison.item_id);
            if destination.is_none_or(|item| {
                item.status != "pending"
                    || item.candidate.subject_ref.as_deref() != Some(route.subject_ref.as_str())
            }) {
                return Err(ApiError::invalid(
                    "routed destination is owner-held or terminal; origin input remains retained",
                ));
            }
            if accepted_ids.is_none() {
                let destination = destination.expect("checked retained destination");
                if !item_available(tx, auth.user_id.0, destination).await?
                    || item_stale(tx, auth, destination).await?
                {
                    return Err(ApiError::invalid(
                        "routed no_change requires a current available fresh destination proposal; origin input remains retained",
                    ));
                }
            }
            let cited = accepted_ids.is_none_or(|ids| {
                data.items.iter().any(|item| {
                    ids.contains(&item.id)
                        && item.candidate.kind == "summary"
                        && item.candidate.subject_ref.as_deref() == Some(route.subject_ref.as_str())
                        && item.candidate.sources.iter().any(|source| {
                            input["entry_ref"] == source.entry_ref
                                && input["version"] == source.version
                        })
                })
            });
            let reviewed_all = route.targets.iter().all(|reference| {
                job.sources.iter().any(|head| {
                    &head.entry_ref == reference
                        && reviewed.iter().any(|source| {
                            source.entry_ref == head.entry_ref && source.version == head.version
                        })
                })
            });
            if route.subject_ref != job.subject_ref || !cited || !reviewed_all {
                return Err(ApiError::invalid(
                    "routed input requires its destination's accepted cited overview or explicit no_change after reviewing all routed primary targets",
                ));
            }
        }
    }
    data.research.follow_ups.retain(|route| {
        let Some(origin_input) = &route.origin_input else { return true };
        if route.subject_ref != job.subject_ref {
            return true;
        }
        let replacement = processed.iter().find(|input| input["entry_ref"] == origin_input.entry_ref
            && input["version"].as_i64().is_some_and(|version| version >= origin_input.version)
            && input["generation"].as_i64().is_some_and(|generation| generation >= origin_input.generation));
        let reviewed_all = route.targets.iter().all(|reference| job.sources.iter().any(|head|
            &head.entry_ref == reference && reviewed.iter().any(|source|
                source.entry_ref == head.entry_ref && source.version == head.version)));
        if let Some(replacement) = replacement.filter(|_| reviewed_all) {
            dispositions.push(json!({"disposition":"research_follow_up_completed","subject_ref":route.subject_ref,
                "comparison":route.comparison,"origin_input":route.origin_input,"processed_input":replacement}));
            false
        } else {
            true
        }
    });
    data.source_dispositions.extend(dispositions.clone());
    Ok(dispositions)
}

pub(super) async fn routed_view(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    job: &research::Job,
) -> ApiResult<(Vec<Value>, Vec<String>)> {
    let mut work = Vec::new();
    let mut targets = BTreeSet::new();
    for route in data
        .research
        .follow_ups
        .iter()
        .filter(|route| route.subject_ref == job.subject_ref)
    {
        if route.origin_source.is_some() {
            let (view, missing) = source_routes::view(tx, auth, data, job, route).await?;
            work.push(view);
            targets.extend(missing);
            continue;
        }
        let origin_input = route.origin_input.as_ref().expect("event route");
        let ids = route
            .targets
            .iter()
            .map(|reference| entry_id(reference))
            .collect::<ApiResult<Vec<_>>>()?;
        let heads = research::headers(tx, auth, &ids, i64::MAX).await?;
        let origin = heads
            .iter()
            .find(|head| head.entry_ref == origin_input.entry_ref);
        let current = data.inputs.iter().find(|input| {
            origin.is_some_and(|head| {
                head.entry_ref == input.entry_ref && head.version == input.version
            })
        });
        let destination = data
            .items
            .iter()
            .find(|item| item.id == route.comparison.item_id);
        let held = destination.is_some_and(|item| {
            matches!(
                item.status.as_str(),
                "rejected" | "deferred" | "approved_held" | "applied"
            )
        });
        let status = if held {
            "owner_held"
        } else if heads.len() != route.targets.len() {
            "source_unavailable"
        } else if current.is_none() {
            "input_not_retained"
        } else if !current.is_some_and(|input| origin_input.matches(input)) {
            "source_changed"
        } else {
            "pending"
        };
        let visible_targets: Vec<_> = heads.iter().map(|head| head.entry_ref.clone()).collect();
        if !held {
            targets.extend(
                heads
                    .iter()
                    .filter(|head| !job.sources.contains(head))
                    .map(|head| head.entry_ref.clone()),
            );
        }
        // Retain unresolved identities internally, but never re-expose an old
        // path, title, comparison body or reference after its access is lost.
        let comparison = if let Some(item) = destination {
            if item.status == "pending"
                && pointer(item) == route.comparison
                && item_available(tx, auth.user_id.0, item).await?
                && !item_stale(tx, auth, item).await?
            {
                Some(&route.comparison)
            } else {
                None
            }
        } else {
            None
        };
        work.push(json!({"comparison":comparison,"origin_input":origin.map(|_| &route.origin_input),
            "current_input":current.map(|input| json!({"entry_ref":input.entry_ref,"version":input.version,"generation":input.generation})),
            "targets":visible_targets,"status":status}));
    }
    Ok((work, targets.into_iter().collect()))
}

#[cfg(test)]
mod origin_wire_tests {
    use super::*;

    #[test]
    fn source_origin_wire_fails_closed_on_old_reader_and_rejects_ambiguous_origins() {
        #[derive(Deserialize)]
        struct OldFollowUp {
            origin_input: InputIdentity,
        }
        let source = "entry:019fba27-687b-7582-8b99-e9371dbe2ce8";
        let mut event = json!({"comparison":{"item_id":"2030-01-01/1","candidate_hash":"a".repeat(64),
            "run_entry_ref":source,"run_version":1},"subject_ref":source,"origin_subject_ref":source,
            "origin_input":{"entry_ref":source,"version":1,"generation":1},"targets":[source]});
        let legacy: FollowUp = serde_json::from_value(event.clone()).unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), event);
        assert_eq!(
            serde_json::from_value::<OldFollowUp>(event.clone())
                .unwrap()
                .origin_input
                .version,
            1
        );
        let input = event
            .as_object_mut()
            .unwrap()
            .remove("origin_input")
            .unwrap();
        event["origin_source"] = json!({"entry_ref":source,"version":1});
        event["origin_snapshot_generation"] = json!(1);
        event["route"] = json!({"route_id":Uuid::now_v7(),"revision":1});
        let retained: FollowUp = serde_json::from_value(event.clone()).unwrap();
        let serialized = serde_json::to_value(retained).unwrap();
        assert!(serialized.get("origin_input").is_none());
        assert!(serde_json::from_value::<OldFollowUp>(serialized).is_err());
        event["origin_input"] = input;
        assert!(serde_json::from_value::<FollowUp>(event.clone()).is_err());
        event.as_object_mut().unwrap().remove("origin_input");
        event["route"]["revision"] = json!(0);
        assert!(serde_json::from_value::<FollowUp>(event.clone()).is_err());
        event.as_object_mut().unwrap().remove("origin_source");
        assert!(serde_json::from_value::<FollowUp>(event).is_err());
    }
}

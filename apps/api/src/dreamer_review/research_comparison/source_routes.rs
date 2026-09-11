//! Source-origin obligations do not manufacture or consume intake events.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Token {
    route_id: Uuid,
    revision: i64,
    comparison: Pointer,
}

pub(in crate::dreamer_review) struct PreparedResolution {
    route: FollowUp,
    token: Token,
    current_source: SourceIdentity,
}

pub(super) fn check_protocol(body: &Value) -> ApiResult<()> {
    if body
        .get("follow_up_protocol")
        .is_some_and(|value| value != FOLLOW_UP_PROTOCOL)
    {
        return Err(ApiError::invalid("unsupported research follow-up protocol"));
    }
    Ok(())
}

pub(super) fn require_protocol(body: &Value) -> ApiResult<()> {
    check_protocol(body)?;
    if body["follow_up_protocol"] != FOLLOW_UP_PROTOCOL {
        return Err(ApiError::invalid(
            "source follow-ups require the advertised follow-up protocol",
        ));
    }
    Ok(())
}

pub(super) fn has_finding(body: &Value) -> bool {
    body["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .any(|finding| finding.as_str().is_some_and(|text| !text.trim().is_empty()))
    })
}

fn token(route: &FollowUp, destination: &Item) -> Token {
    let identity = route.route.as_ref().expect("source route identity");
    Token {
        route_id: identity.route_id,
        revision: identity.revision,
        comparison: pointer(destination),
    }
}

fn current_origin(route: &FollowUp, heads: &[Input]) -> Option<SourceIdentity> {
    let original = route.origin_source.as_ref()?;
    heads
        .iter()
        .find(|head| head.entry_ref == original.entry_ref && head.version >= original.version)
        .map(|head| SourceIdentity {
            entry_ref: head.entry_ref.clone(),
            version: head.version,
        })
}

async fn destination<'a>(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &'a RunState,
    route: &FollowUp,
) -> ApiResult<Option<&'a Item>> {
    let Some(item) = data.items.iter().find(|item| {
        item.id == route.comparison.item_id
            && item.status == "pending"
            && item.reviewable
            && item.candidate.kind == "summary"
            && item.candidate.evidence_scope.is_none()
            && item.candidate.raw_sources.is_empty()
            && item.candidate.subject_ref.as_deref() == Some(route.subject_ref.as_str())
    }) else {
        return Ok(None);
    };
    if !item_available(tx, auth.user_id.0, item).await? {
        return Ok(None);
    }
    Ok(Some(item))
}

pub(super) async fn view(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    job: &research::Job,
    route: &FollowUp,
) -> ApiResult<(Value, Vec<String>)> {
    let ids = route
        .targets
        .iter()
        .map(|reference| entry_id(reference))
        .collect::<ApiResult<Vec<_>>>()?;
    let heads = research::headers(tx, auth, &ids, i64::MAX).await?;
    let origin = current_origin(route, &heads);
    let item = destination(tx, auth, data, route).await?;
    let available = heads.len() == route.targets.len() && origin.is_some();
    let admitted = available && heads.iter().all(|head| job.sources.contains(head));
    let status = if item.is_none() {
        "destination_unavailable"
    } else if !available {
        "source_unavailable"
    } else if !admitted {
        "source_changed"
    } else {
        "pending"
    };
    let offered = item.filter(|_| admitted).map(|item| token(route, item));
    let comparison = item.map(pointer);
    let missing = if item.is_some() {
        heads
            .iter()
            .filter(|head| !job.sources.contains(head))
            .map(|head| head.entry_ref.clone())
            .collect()
    } else {
        Vec::new()
    };
    Ok((
        json!({"route":offered,"comparison":comparison,
        "origin_source":origin.as_ref().map(|_| &route.origin_source),"current_source":origin,
        "targets":heads.iter().map(|head| &head.entry_ref).collect::<Vec<_>>(),"status":status}),
        missing,
    ))
}

/// Validate against the transaction's offered state BEFORE any candidate is
/// revised. A stable route identity is not permission to overwrite a changed
/// pending proposal using an old comparison and a newly supplied state CAS.
pub(in crate::dreamer_review) async fn prepare_resolutions(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &RunState,
    job: &research::Job,
    body: &Value,
    candidate: bool,
) -> ApiResult<Vec<PreparedResolution>> {
    check_protocol(body)?;
    let Some(value) = body.get("resolved_follow_ups") else {
        return Ok(Vec::new());
    };
    require_protocol(body)?;
    if (!candidate && body["status"] != "no_change")
        || optional(body, "follow_up").is_some()
        || optional(body, "supersedes_existing").is_some()
        || body.get("repair_feedback").is_some()
    {
        return Err(ApiError::invalid(
            "source routes may be resolved only by a candidate or separate supported no_change",
        ));
    }
    let tokens: Vec<Token> = serde_json::from_value(value.clone())?;
    if tokens.len() > MAX_FOLLOW_UPS
        || tokens.iter().enumerate().any(|(index, token)| {
            tokens[..index]
                .iter()
                .any(|old| old.route_id == token.route_id)
        })
    {
        return Err(ApiError::invalid(
            "source route resolution requires at most 32 distinct offered routes",
        ));
    }
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    if !has_finding(body) || !research::fresh(tx, auth, job).await? {
        return Err(ApiError::invalid(
            "source route resolution requires current research evidence and an explicit finding",
        ));
    }
    let reviewed: Vec<Source> = serde_json::from_value(body["reviewed_sources"].clone())?;
    let mut prepared = Vec::new();
    for requested in tokens {
        let route = data
            .research
            .follow_ups
            .iter()
            .find(|route| {
                route.subject_ref == job.subject_ref
                    && route.route.as_ref().is_some_and(|identity| {
                        identity.route_id == requested.route_id
                            && identity.revision == requested.revision
                    })
            })
            .ok_or_else(|| ApiError::invalid("source route token is not currently offered"))?;
        let (offered, _) = view(tx, auth, data, job, route).await?;
        if offered["route"] != json!(requested) {
            return Err(ApiError::invalid(
                "source route or destination changed or is unavailable; refresh before disposition",
            ));
        }
        let item = destination(tx, auth, data, route)
            .await?
            .expect("offered destination");
        if !candidate
            && (item_stale(tx, auth, item).await?
                || !item.candidate.sources.iter().any(|source| {
                    offered["current_source"]["entry_ref"] == source.entry_ref
                        && offered["current_source"]["version"] == source.version
                }))
        {
            return Err(ApiError::invalid(
                "source-route no_change requires a fresh destination proposal citing the current origin",
            ));
        }
        if !route.targets.iter().all(|reference| {
            job.sources.iter().any(|head| {
                &head.entry_ref == reference
                    && reviewed.iter().any(|source| {
                        source.entry_ref == head.entry_ref && source.version == head.version
                    })
            })
        }) {
            return Err(ApiError::invalid(
                "source route completion requires every current primary target to be explicitly reviewed",
            ));
        }
        prepared.push(PreparedResolution {
            route: route.clone(),
            token: requested,
            current_source: serde_json::from_value(offered["current_source"].clone())?,
        });
    }
    Ok(prepared)
}

/// Called within the same transaction after candidate validation. Zero-ID
/// submissions retain the obligation even when their requested token was valid.
pub(in crate::dreamer_review) async fn resolve_sources(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    data: &mut RunState,
    job: &research::Job,
    prepared: Vec<PreparedResolution>,
    body: &Value,
    accepted_ids: Option<&[String]>,
    operation_id: &str,
) -> ApiResult<Option<Value>> {
    if body["follow_up_protocol"] != FOLLOW_UP_PROTOCOL {
        return Ok(None);
    }
    let mut resolved = Vec::new();
    if accepted_ids.is_some_and(|ids| ids.is_empty()) {
        return Ok(Some(
            json!({"protocol":FOLLOW_UP_PROTOCOL,"operation_id":operation_id,
            "recorded":true,"resolved_follow_ups":resolved,"replayed":false}),
        ));
    }
    for prepared in prepared {
        let destination = data
            .items
            .iter()
            .find(|item| {
                item.id == prepared.route.comparison.item_id
                    && item.status == "pending"
                    && item.candidate.kind == "summary"
                    && item.candidate.subject_ref.as_deref() == Some(job.subject_ref.as_str())
            })
            .ok_or_else(|| {
                ApiError::invalid("source route destination no longer permits completion")
            })?;
        if let Some(ids) = accepted_ids {
            if !ids.contains(&destination.id)
                || !destination.candidate.sources.iter().any(|source| {
                    source.entry_ref == prepared.current_source.entry_ref
                        && source.version == prepared.current_source.version
                })
            {
                return Err(ApiError::invalid(
                    "source route requires an accepted destination summary citing the exact current origin",
                ));
            }
        } else if pointer(destination) != prepared.token.comparison
            || !item_available(tx, auth.user_id.0, destination).await?
            || item_stale(tx, auth, destination).await?
            || !destination.candidate.sources.iter().any(|source| {
                source.entry_ref == prepared.current_source.entry_ref
                    && source.version == prepared.current_source.version
            })
        {
            return Err(ApiError::invalid(
                "source-route no_change requires its unchanged fresh available destination citing the current origin",
            ));
        }
        let destination_pointer = pointer(destination);
        let ids = prepared
            .route
            .targets
            .iter()
            .map(|reference| entry_id(reference))
            .collect::<ApiResult<Vec<_>>>()?;
        let heads = research::headers(tx, auth, &ids, job.snapshot_generation).await?;
        if heads.len() != prepared.route.targets.len()
            || !heads.iter().all(|head| job.sources.contains(head))
            || current_origin(&prepared.route, &heads).as_ref() != Some(&prepared.current_source)
        {
            return Err(ApiError::invalid(
                "source route evidence changed before completion",
            ));
        }
        let (mut origin_job, version) =
            research::load(tx, auth, &prepared.route.origin_subject_ref)
                .await?
                .ok_or_else(|| {
                    ApiError::invalid(
                        "source route origin research is unavailable; work remains retained",
                    )
                })?;
        // Existing resume selection enforces owner holds and last_attempt. A
        // due timestamp needs no queue slot and does not give another turn now.
        origin_job.retry_at = Utc::now();
        research::save(state, tx, auth, &origin_job, version).await?;
        let disposition = json!({"disposition":"research_source_follow_up_completed",
            "route":prepared.route,"current_source":prepared.current_source,
            "destination":destination_pointer,"findings":body["findings"],"model_processed":false});
        let audit_path = format!(
            "dreams/reviews/source-route-{}-{}.md",
            operation_id, prepared.token.route_id
        );
        put_entry(state, tx, auth, &audit_path,
            "# Research source follow-up\n\nCurrent primary evidence resolved a retained enrichment obligation. No intake event was consumed.\n".into(),
            json!({"kind":"dreamer_review","dreamer_review":{"schema":"dream.review.v1","disposition":disposition}}), 0).await?;
        data.source_dispositions.push(disposition);
        data.research.follow_ups.retain(|route| {
            route
                .route
                .as_ref()
                .is_none_or(|identity| identity.route_id != prepared.token.route_id)
        });
        resolved.push(json!(prepared.token));
    }
    Ok(Some(
        json!({"protocol":FOLLOW_UP_PROTOCOL,"operation_id":operation_id,"recorded":true,
        "resolved_follow_ups":resolved,"replayed":false}),
    ))
}

/// Source routes have no retained-input guard. Inspect the current visible
/// primary instead of treating an old exclusion audit as a current deletion.
pub(super) async fn policy_exclusion(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    route: &FollowUp,
) -> ApiResult<Option<Value>> {
    let row = sqlx::query("SELECT e.path,e.deleted_at,e.current_version,v.metadata FROM brunn.entries e LEFT JOIN LATERAL (SELECT metadata FROM brunn.entry_versions WHERE user_id=e.user_id AND entry_id=e.id AND version=e.current_version LIMIT 1) v ON true WHERE e.user_id=$1 AND e.id=$2")
        .bind(auth.user_id.0).bind(entry_id(route.origin_ref())?).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(None) };
    let reason = if row.get::<Option<DateTime<Utc>>, _>("deleted_at").is_some() {
        Some("deleted_source")
    } else if row
        .get::<Option<Value>, _>("metadata")
        .as_ref()
        .is_some_and(crate::dreamer_summary::generated_briefing_metadata)
    {
        Some("excluded_generated_briefing")
    } else if sensitive_input_path(&row.get::<String, _>("path")) {
        Some("excluded_credential_record")
    } else {
        None
    };
    Ok(reason.map(|reason| json!({"disposition":"excluded_by_source_policy","route":route,
        "source_disposition":{"disposition":reason,"entry_ref":route.origin_ref(),"version":row.get::<i64,_>("current_version")},
        "model_processed":false})))
}

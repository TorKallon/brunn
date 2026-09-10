//! One disposable proof of the latest bounded query execution. It never joins
//! historical checkpoint projections or supplies primary-source evidence.
use super::*;

const SCHEMA: &str = "dream.research.discovery.v1";
const MAX_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    schema: String,
    retrieval_policy: u8,
    searched_generation: i64,
    queries: Vec<String>,
    groups: Vec<Group>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    query_index: usize,
    sort: String,
    returned: usize,
    limit: usize,
    execution_status: String,
    output_limit_reached: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Stored {
    Invalidated,
    Recorded {
        search: Search,
        scope: Box<SubjectScope>,
        dependencies: Vec<(Uuid, i64)>,
        historical_versions: Vec<i64>,
    },
}

pub(super) fn normalize(queries: &[String]) -> ApiResult<Vec<String>> {
    let normalized = queries
        .iter()
        .map(|query| {
            query
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if normalized.len() > 6
        || normalized
            .iter()
            .any(|query| query.is_empty() || query.len() > 160)
    {
        return Err(ApiError::invalid(
            "normalized discovery queries exceed the supported bounds",
        ));
    }
    Ok(normalized)
}

fn history(job: &Job) -> Vec<i64> {
    if job.schema == "dream.research.v2" {
        job.checkpoint_versions.clone().unwrap_or_default()
    } else {
        match job.revalidation_checkpoint {
            Some(RevalidationCheckpoint::Retained { version }) => vec![version],
            _ => Vec::new(),
        }
    }
}

fn parse(job: &Job) -> Option<Stored> {
    let value = job.coverage.get("discovery_audit")?;
    if serde_json::to_vec(value).ok()?.len() > MAX_BYTES {
        return None;
    }
    let audit: Stored = serde_json::from_value(value.clone()).ok()?;
    if let Stored::Recorded {
        search,
        scope,
        dependencies,
        historical_versions,
    } = &audit
    {
        let unique = dependencies.iter().collect::<BTreeSet<_>>();
        if search.schema != SCHEMA
            || search.searched_generation < 0
            || scope.subject_ref != job.subject_ref
            || !scope.dependencies.is_empty()
            || scope.checked_generation != search.searched_generation
            || dependencies.is_empty()
            || dependencies.len() > MAX_SOURCES
            || unique.len() != dependencies.len()
            || dependencies
                .iter()
                .any(|(id, version)| id.is_nil() || *version < 1)
            || !dependencies
                .iter()
                .any(|(id, _)| format!("entry:{id}") == job.subject_ref)
            || historical_versions.len() > 4
            || historical_versions.iter().any(|version| *version < 1)
            || historical_versions.iter().collect::<BTreeSet<_>>().len()
                != historical_versions.len()
            || search.queries.is_empty()
            || normalize(&search.queries).ok().as_ref() != Some(&search.queries)
            || search.groups.len() != search.queries.len() * 2
            || search.groups.iter().enumerate().any(|(index, group)| {
                group.query_index != index / 2
                    || group.sort
                        != if index % 2 == 0 {
                            "best_match"
                        } else {
                            "last_modified"
                        }
                    || group.returned > 8
                    || group.limit != 8
                    || group.execution_status != "bounded"
                    || group.output_limit_reached != (group.returned == 8)
            })
        {
            return None;
        }
    }
    Some(audit)
}

pub(super) fn invalidate(job: &mut Job) {
    if job.coverage.get("discovery_audit").is_some() {
        job.coverage["discovery_audit"] = json!(Stored::Invalidated);
    }
}

pub(super) fn invalidate_changed_authority(job: &mut Job) {
    if let Some(Stored::Recorded {
        dependencies,
        historical_versions,
        ..
    }) = parse(job)
        && (historical_versions != history(job)
            || dependencies.iter().any(|(id, version)| {
                !job.sources.iter().any(|source| {
                    source.entry_ref == format!("entry:{id}") && source.version == *version
                })
            }))
    {
        invalidate(job);
    }
}

async fn historical_access(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
) -> ApiResult<bool> {
    for origin in history(job) {
        if origin < 1
            || origin >= version
            || crate::dreamer_summary::research_revalidation_context(
                tx,
                auth,
                &path(&job.subject_ref)?,
                &job.subject_ref,
                origin,
            )
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn current(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
    stored: &Stored,
) -> ApiResult<bool> {
    let Stored::Recorded {
        search,
        scope,
        dependencies,
        historical_versions,
    } = stored
    else {
        return Ok(false);
    };
    if search.retrieval_policy != simple_core::DREAMER_LEXICAL_POLICY_VERSION
        || historical_versions != &history(job)
        || dependencies.iter().any(|(id, version)| {
            !job.sources.iter().any(|source| {
                source.entry_ref == format!("entry:{id}") && source.version == *version
            })
        })
        || !fresh(tx, auth, job).await?
        || !historical_access(tx, auth, job, version).await?
    {
        return Ok(false);
    }
    Ok(check_scope(tx, auth, scope, dependencies).await?.status == "fresh")
}

pub(super) async fn matches(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
    queries: &[String],
) -> ApiResult<bool> {
    let Some(audit @ Stored::Recorded { .. }) = parse(job) else {
        return Ok(false);
    };
    let Stored::Recorded { search, .. } = &audit else {
        unreachable!()
    };
    Ok(search.queries == queries && current(tx, auth, job, version, &audit).await?)
}

pub(super) async fn project(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    job: &Job,
    version: i64,
    view: &mut Value,
) -> ApiResult<()> {
    if let Some(coverage) = view.get_mut("coverage").and_then(Value::as_object_mut) {
        coverage.remove("discovery_audit");
    }
    let Some(audit) = parse(job) else {
        view["discovery_audit"] = json!({"validity":"legacy_or_unknown","last_search":null});
        return Ok(());
    };
    if current(tx, auth, job, version, &audit).await? {
        let Stored::Recorded { search, .. } = audit else {
            unreachable!()
        };
        view["discovery_audit"] = json!({"validity":"current","last_search":search});
    } else {
        view["discovery_audit"] = json!({"validity":"outdated","last_search":null});
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    before: &Job,
    job: &mut Job,
    version: i64,
    searched_generation: i64,
    queries: Vec<String>,
    results: &[Value],
    results_current: bool,
) -> ApiResult<()> {
    let mut groups = Vec::new();
    for (query_index, _) in queries.iter().enumerate() {
        for sort in ["best_match", "last_modified"] {
            let id = format!("scope-{query_index}-{sort}");
            let Some(candidates) = results
                .iter()
                .find(|group| group["id"] == id)
                .and_then(|group| group["candidates"].as_array())
            else {
                return Err(ApiError::invalid(
                    "discovery search returned an invalid audit group",
                ));
            };
            if candidates.len() > 8 {
                return Err(ApiError::invalid(
                    "discovery search exceeded its audit limit",
                ));
            }
            groups.push(Group {
                query_index,
                sort: sort.into(),
                returned: candidates.len(),
                limit: 8,
                execution_status: "bounded".into(),
                output_limit_reached: candidates.len() == 8,
            });
        }
    }
    let mut scope = before.scope.clone();
    scope.checked_generation = searched_generation;
    scope.dependencies.clear();
    let stored = Stored::Recorded {
        search: Search {
            schema: SCHEMA.into(),
            retrieval_policy: simple_core::DREAMER_LEXICAL_POLICY_VERSION,
            searched_generation,
            queries,
            groups,
        },
        scope: Box::new(scope),
        dependencies: dependencies(before)?,
        historical_versions: history(before),
    };
    if serde_json::to_vec(&stored)?.len() > MAX_BYTES {
        return Err(ApiError::invalid(
            "discovery audit exceeds 32 KiB; research progress was retained",
        ));
    }
    // The search took place outside the write lock. A refreshed commit scope
    // must not erase an intervening relevant change or inaccessible dependency.
    job.coverage["discovery_audit"] =
        if results_current && current(tx, auth, job, version, &stored).await? {
            json!(stored)
        } else {
            json!(Stored::Invalidated)
        };
    Ok(())
}

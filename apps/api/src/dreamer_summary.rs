//! Snapshot-checked derived representations. No foreground generation or writes.
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthContext,
    db::set_context,
    error::ApiResult,
    location::evidence::{EvidenceQuery, evidence_in_tx},
    models::Capability,
    simple_core::ReadItem,
};

const MAX_ALTERNATIVES: usize = 3;
const MAX_SOURCES: usize = 64;
const MAX_CHANGES: i64 = 2_000;
const MAX_AUDITS: usize = 16;
const MAX_RESEARCH_SOURCES: usize = 256;

#[derive(Clone, Debug, Deserialize)]
struct Source {
    entry_ref: String,
    version: i64,
    #[serde(default)]
    excerpt: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    schema: String,
    state: String,
    frozen_generation: i64,
    sources: Vec<Source>,
    #[serde(default)]
    raw_sources: Vec<Value>,
    #[serde(alias = "covered_prefixes")]
    scope_prefixes: Vec<String>,
    #[serde(default)]
    evidence_scope: Option<Value>,
    #[serde(default)]
    subject_ref: Option<String>,
    #[serde(default)]
    subject_scope: Option<crate::dreamer_subject::SubjectScope>,
}

#[derive(Clone, Debug)]
struct Document {
    id: Uuid,
    path: String,
    title: String,
    version: i64,
    media_type: String,
    content: String,
    content_hash: String,
    metadata: Value,
    updated_at: DateTime<Utc>,
}

struct Check {
    status: &'static str,
    reason: &'static str,
    sources: Vec<Document>,
    inaccessible: bool,
}

pub fn managed_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.starts_with("derived/entities/") || path.starts_with("derived/location/")
}

pub fn protected_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    managed_path(&path)
        || matches!(
            path.as_str(),
            "dreams/state.md" | "dreams/latest-receipt.md"
        )
        || path.starts_with("dreams/runs/")
        || path.starts_with("dreams/reviews/")
        || path.starts_with("dreams/research/")
}

pub fn protected_metadata(metadata: &Value) -> bool {
    [
        "dreamer_summary",
        "dreamer_run",
        "dreamer_review",
        "dreamer_state",
        "dreamer_receipt",
        "dreamer_research",
    ]
    .iter()
    .any(|key| metadata.get(*key).is_some())
}

/// Generated editions are readable workspace documents, but not primary
/// Dreamer evidence. This classification deliberately grants no write protection.
pub(crate) fn generated_briefing_metadata(metadata: &Value) -> bool {
    metadata.get("kind").and_then(Value::as_str) == Some("briefing_edition")
}

fn source_id(source: &Source) -> Option<Uuid> {
    (source.version > 0).then_some(())?;
    Uuid::parse_str(source.entry_ref.strip_prefix("entry:")?).ok()
}

fn manifest(metadata: &Value) -> Option<Manifest> {
    let value: Manifest = serde_json::from_value(metadata.get("dreamer_summary")?.clone()).ok()?;
    if value.schema != "dream.summary.v1"
        || value.state != "published"
        || value.frozen_generation < 0
        || (value.sources.is_empty() && value.evidence_scope.is_none())
        || value.sources.len() > MAX_SOURCES
        || value.sources.len() + value.raw_sources.len() > MAX_SOURCES
        || (value.scope_prefixes.is_empty() && value.evidence_scope.is_none())
        || value.scope_prefixes.len() > MAX_SOURCES
        || value
            .sources
            .iter()
            .any(|source| source_id(source).is_none())
        || value
            .scope_prefixes
            .iter()
            .any(|prefix| prefix.is_empty() || prefix.len() > 1_024)
        || value.subject_ref.as_ref().is_some_and(|reference| {
            value
                .subject_scope
                .as_ref()
                .is_none_or(|scope| &scope.subject_ref != reference)
                || !value
                    .sources
                    .iter()
                    .any(|source| &source.entry_ref == reference)
        })
        || value
            .subject_scope
            .as_ref()
            .is_some_and(|scope| value.subject_ref.as_ref() != Some(&scope.subject_ref))
        || value.subject_scope.is_some() && value.evidence_scope.is_some()
    {
        return None;
    }
    Some(value)
}

async fn snapshot<'a>(
    state: &'a AppState,
    auth: &AuthContext,
) -> ApiResult<Transaction<'a, Postgres>> {
    // Raw location RLS grants Save on app_rw only. Never elevate a Read-only caller.
    let pool = if auth.can(Capability::Save) {
        &state.rw_pool
    } else {
        &state.ro_pool
    };
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    set_context(&mut tx, auth).await?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true)")
        .bind(format!("{}ms", state.config.request_timeout.as_millis()))
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

async fn document(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    id: Uuid,
    version: Option<i64>,
) -> ApiResult<Option<Document>> {
    let row = sqlx::query("SELECT e.id,e.path,e.title,e.media_type,v.version,v.content,v.content_sha256,v.metadata,v.created_at AS updated_at FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=coalesce($3,e.current_version) WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL")
        .bind(auth.user_id.0).bind(id).bind(version).fetch_optional(&mut **tx).await?;
    row.map(document_row).transpose()
}

fn document_row(row: PgRow) -> ApiResult<Document> {
    Ok(Document {
        id: row.try_get("id")?,
        path: row.try_get("path")?,
        title: row.try_get("title")?,
        version: row.try_get("version")?,
        media_type: row.try_get("media_type")?,
        content: row
            .try_get::<Option<String>, _>("content")?
            .unwrap_or_default(),
        content_hash: row.try_get("content_sha256")?,
        metadata: row.try_get("metadata")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn relevant_change(path: &str, prefixes: &[String]) -> bool {
    !path.starts_with("derived/")
        && !path.starts_with("dreams/")
        && !path.starts_with(".brunn/")
        && prefixes.iter().any(|prefix| path.starts_with(prefix))
}

async fn check(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    summary: &Document,
) -> ApiResult<Check> {
    let Some(manifest) = manifest(&summary.metadata) else {
        return Ok(Check {
            status: "unchecked",
            reason: "invalid_manifest",
            sources: vec![],
            inaccessible: true,
        });
    };
    let mut sources = Vec::new();
    let mut inaccessible = false;
    let mut changed = false;
    let ids = manifest
        .sources
        .iter()
        .filter_map(source_id)
        .collect::<Vec<_>>();
    // Dependency validation needs heads, not 64 complete source bodies. Fetch
    // one fallback body only if the validated representation cannot be used.
    let versions = manifest
        .sources
        .iter()
        .map(|source| source.version)
        .collect::<Vec<_>>();
    let rows = sqlx::query("SELECT e.id,e.path,e.title,e.media_type,v.version,NULL::text AS content,v.content_sha256,v.metadata,v.created_at AS updated_at,original.version IS NULL OR coalesce(original.metadata->>'kind','')='briefing_edition' AS source_excluded FROM unnest($2::uuid[],$3::bigint[]) selected(id,version) JOIN brunn.entries e ON e.user_id=$1 AND e.id=selected.id JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version LEFT JOIN LATERAL (SELECT original.version,original.metadata FROM brunn.entry_versions original WHERE original.user_id=e.user_id AND original.entry_id=e.id AND original.version=selected.version LIMIT 1) original ON true WHERE e.deleted_at IS NULL")
        .bind(auth.user_id.0).bind(ids).bind(versions).fetch_all(&mut **tx).await?;
    inaccessible |= rows.iter().any(|row| row.get::<bool, _>("source_excluded"));
    let heads = rows
        .into_iter()
        .map(document_row)
        .collect::<ApiResult<Vec<_>>>()?;
    for source in &manifest.sources {
        let Some(current) = heads.iter().find(|head| Some(head.id) == source_id(source)) else {
            inaccessible = true;
            continue;
        };
        if current.metadata.get("dreamer_summary").is_some()
            || generated_briefing_metadata(&current.metadata)
            || current.path.starts_with("derived/")
        {
            inaccessible = true;
            continue;
        }
        changed |= current.version != source.version;
        if !sources.iter().any(|item: &Document| item.id == current.id) {
            sources.push(current.clone());
        }
    }
    if inaccessible {
        return Ok(Check {
            status: "stale",
            reason: "source_unavailable",
            sources,
            inaccessible,
        });
    }
    if !manifest.raw_sources.is_empty() {
        let raw = raw_refs(&manifest.raw_sources);
        let available = match raw {
            Some(raw) => raw_access(tx, auth, &raw).await?,
            None => false,
        };
        if !available {
            return Ok(Check {
                status: "unchecked",
                reason: if auth.can(Capability::Save) {
                    "raw_source_unavailable"
                } else {
                    "location_evidence_requires_save"
                },
                sources,
                inaccessible: true,
            });
        }
    }
    if let Some(scope) = &manifest.evidence_scope {
        if !auth.can(Capability::Save) {
            return Ok(Check {
                status: "unchecked",
                reason: "location_evidence_requires_save",
                sources,
                inaccessible: false,
            });
        }
        let query = json!({ "from": scope.get("from"), "to": scope.get("to"), "timezone": scope.get("timezone") });
        let Ok(query) = serde_json::from_value::<EvidenceQuery>(query) else {
            return Ok(Check {
                status: "unchecked",
                reason: "invalid_evidence_scope",
                sources,
                inaccessible: false,
            });
        };
        if query.validate(Utc::now()).is_err()
            || scope.get("sources_validated").and_then(Value::as_bool) != Some(true)
        {
            return Ok(Check {
                status: "unchecked",
                reason: "unverified_evidence_scope",
                sources,
                inaccessible: false,
            });
        }
        // Publication verifies every cited canonical row belongs to this exact
        // packet. Its fingerprint tolerates unrelated month appends but covers
        // edits to selected rows, late reports, boundaries and Places versions.
        let packet = evidence_in_tx(tx, auth, &query).await?;
        let complete = packet.get("fingerprint_complete").and_then(Value::as_bool) == Some(true);
        let canonical_sources: Vec<_> = manifest
            .sources
            .iter()
            .filter(|source| {
                !scope["context_sources"].as_array().is_some_and(|context| {
                    context.iter().any(|item| {
                        item["entry_ref"] == source.entry_ref
                            && item["version"] == source.version
                            && item["excerpt"] == source.excerpt
                    })
                })
            })
            .cloned()
            .collect();
        let same = evidence_sources_match(&canonical_sources, &packet)
            && crate::location::summary::context_sources_current_in_tx(tx, auth, scope).await?
            && scope
                .get("fingerprint")
                .and_then(Value::as_str)
                .is_some_and(|expected| {
                    packet.get("evidence_fingerprint").and_then(Value::as_str) == Some(expected)
                });
        return Ok(Check {
            status: if complete && same {
                "fresh"
            } else if complete {
                "stale"
            } else {
                "unchecked"
            },
            reason: if complete && same {
                "evidence_scope_matches"
            } else if complete {
                "evidence_scope_changed"
            } else {
                "evidence_incomplete"
            },
            sources,
            inaccessible: false,
        });
    }
    if changed && manifest.subject_scope.is_none() {
        return Ok(Check {
            status: "stale",
            reason: "source_version_changed",
            sources,
            inaccessible: false,
        });
    }
    if let Some(scope) = &manifest.subject_scope {
        let dependencies = manifest
            .sources
            .iter()
            .filter_map(|source| source_id(source).map(|id| (id, source.version)))
            .collect::<Vec<_>>();
        let subject = crate::dreamer_subject::check_scope(tx, auth, scope, &dependencies).await?;
        return Ok(Check {
            status: subject.status,
            reason: subject.reason,
            sources,
            inaccessible: subject.reason == "subject_source_unavailable"
                || subject.reason.starts_with("invalid_subject_"),
        });
    }
    let changes = sqlx::query("SELECT change.path,previous.path AS previous_path FROM brunn.workspace_changes change LEFT JOIN LATERAL (SELECT old.path,old.entry_version FROM brunn.workspace_changes old WHERE old.user_id=change.user_id AND old.entry_id=change.entry_id AND old.generation<change.generation ORDER BY old.generation DESC LIMIT 1) previous ON true LEFT JOIN brunn.entry_versions change_v ON change_v.user_id=change.user_id AND change_v.entry_id=change.entry_id AND change_v.version=change.entry_version LEFT JOIN brunn.entry_versions previous_v ON previous_v.user_id=change.user_id AND previous_v.entry_id=change.entry_id AND previous_v.version=previous.entry_version WHERE change.user_id=$1 AND change.generation>$2 AND (coalesce(change_v.metadata->>'kind','')<>'briefing_edition' OR previous.path IS NOT NULL AND coalesce(previous_v.metadata->>'kind','')<>'briefing_edition') ORDER BY change.generation LIMIT $3")
        .bind(auth.user_id.0).bind(manifest.frozen_generation).bind(MAX_CHANGES + 1)
        .fetch_all(&mut **tx).await?;
    let relevant = changes.iter().any(|row| {
        row.try_get::<String, _>("path")
            .is_ok_and(|path| relevant_change(&path, &manifest.scope_prefixes))
            || row
                .try_get::<Option<String>, _>("previous_path")
                .ok()
                .flatten()
                .is_some_and(|path| relevant_change(&path, &manifest.scope_prefixes))
    });
    let (status, reason) = if relevant {
        ("stale", "covered_scope_changed")
    } else if changes.len() > MAX_CHANGES as usize {
        ("unchecked", "change_check_limit")
    } else {
        ("fresh", "sources_and_scope_match")
    };
    Ok(Check {
        status,
        reason,
        sources,
        inaccessible: false,
    })
}

fn evidence_sources_match(sources: &[Source], packet: &Value) -> bool {
    sources.iter().all(|source| {
        if let Some(canonical) = packet
            .get("canonical_months")
            .and_then(Value::as_array)
            .and_then(|documents| {
                documents.iter().find(|doc| {
                    doc.get("ref").and_then(Value::as_str) == Some(source.entry_ref.as_str())
                })
            })
        {
            !source.excerpt.is_empty()
                && source.excerpt.lines().all(|line| {
                    canonical
                        .get("selectors")
                        .and_then(Value::as_array)
                        .is_some_and(|selectors| {
                            selectors.iter().any(|selector| {
                                selector.get("text").and_then(Value::as_str) == Some(line)
                            })
                        })
                })
        } else {
            packet.get("places").is_some_and(|places| {
                places.get("ref").and_then(Value::as_str) == Some(source.entry_ref.as_str())
                    && places.get("version").and_then(Value::as_i64) == Some(source.version)
            })
        }
    })
}

fn render(document: &Document, request: &ReadItem, max_chars: usize) -> Value {
    let selected = match request.view.as_deref().unwrap_or("full") {
        "range" => {
            let start = request.start.unwrap_or(1).max(1);
            let end = request.end.unwrap_or(start.saturating_add(199)).max(start);
            document
                .content
                .lines()
                .skip(start - 1)
                .take(end - start + 1)
                .collect::<Vec<_>>()
                .join("\n")
        }
        "outline" => document
            .content
            .lines()
            .filter(|line| line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => document.content.clone(),
    };
    let mut value = json!({"reference":format!("entry:{}",document.id),"path":document.path,
        "title":document.title,"version":document.version,"media_type":document.media_type,
        "content_hash":format!("sha256:{}",document.content_hash),"updated_at":document.updated_at,
        "view":request.view.as_deref().unwrap_or("full"),"text":selected.chars().take(max_chars).collect::<String>()});
    if selected.chars().count() > max_chars {
        value["truncated"] = json!(true);
    }
    if document.path.starts_with("derived/location/") {
        // Full evidence is available through the immutable run, not injected
        // into every fast read. Validation above still uses the full manifest.
        let manifest = &document.metadata["dreamer_summary"];
        if let Some((id, version)) = exact_ref(manifest, "run_entry_ref", "run_version") {
            value["evidence"] =
                json!({"reference":format!("entry:{id}"),"version":version,"view":"full"});
        }
        // Independent exact source pointers remain usable if an unrelated
        // proposal's deleted dependency later withholds the whole-run audit.
        let mut pointers = Vec::new();
        for source in manifest["sources"].as_array().into_iter().flatten() {
            if let Some((id, version)) = exact_ref(source, "entry_ref", "version") {
                let pointer = json!({"reference":format!("entry:{id}"),"version":version});
                if !pointers.contains(&pointer) && pointers.len() < MAX_ALTERNATIVES {
                    pointers.push(pointer);
                }
            }
        }
        value["source_documents"] = json!(pointers);
        if let Some(scope) = manifest["evidence_scope"].as_object() {
            value["location_evidence_request"] = json!({"from":scope.get("from"),"to":scope.get("to"),"timezone":scope.get("timezone")});
        }
        value["metadata_omitted"] = json!(true);
        value["metadata_omitted_reason"] = json!("evidence_available_separately");
    } else if document.metadata != json!({}) {
        let remaining = max_chars.saturating_sub(selected.chars().count().min(max_chars));
        if document.metadata.to_string().chars().count() <= remaining {
            value["metadata"] = document.metadata.clone();
        } else {
            value["metadata_omitted"] = json!(true);
            value["metadata_omitted_reason"] = json!("response_budget");
        }
    }
    value
}

fn freshness(check: &Check, generation: i64) -> Value {
    json!({"status":check.status,"reason":check.reason,"checked_generation":generation,
        "sources":check.sources.iter().take(MAX_ALTERNATIVES).map(|source| json!({"reference":format!("entry:{}",source.id),"path":source.path,"version":source.version})).collect::<Vec<_>>()})
}

fn exact_ref(value: &Value, ref_key: &str, version_key: &str) -> Option<(Uuid, i64)> {
    let id = value
        .get(ref_key)?
        .as_str()?
        .strip_prefix("entry:")?
        .parse()
        .ok()?;
    let version = value
        .get(version_key)?
        .as_i64()
        .filter(|version| *version > 0)?;
    Some((id, version))
}

fn raw_refs(citations: &[Value]) -> Option<std::collections::BTreeSet<(DateTime<Utc>, String)>> {
    let mut result = std::collections::BTreeSet::new();
    if citations.len() > MAX_SOURCES {
        return None;
    }
    for citation in citations {
        let key = citation.get("natural_key")?;
        let at = DateTime::parse_from_rfc3339(key.get("at")?.as_str()?)
            .ok()?
            .with_timezone(&Utc);
        let kind = key
            .get("type")?
            .as_str()
            .filter(|value| matches!(*value, "ping" | "visit_arrival" | "visit_departure"))?;
        result.insert((at, kind.to_owned()));
    }
    Some(result)
}

async fn raw_access(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    raw: &std::collections::BTreeSet<(DateTime<Utc>, String)>,
) -> ApiResult<bool> {
    if raw.is_empty() {
        return Ok(true);
    }
    if !auth.can(Capability::Save) {
        return Ok(false);
    }
    let (ats, kinds): (Vec<_>, Vec<_>) = raw.iter().cloned().unzip();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM unnest($2::timestamptz[],$3::text[]) AS required(at,kind) JOIN brunn.location_reports report ON report.user_id=$1 AND report.at=required.at AND report.type::text=required.kind")
        .bind(auth.user_id.0).bind(ats).bind(kinds).fetch_one(&mut **tx).await?;
    Ok(visible == raw.len() as i64)
}

fn audit_item(
    item: &Value,
    sources: &mut std::collections::BTreeSet<(Uuid, i64)>,
    audits: &mut Vec<(Uuid, i64)>,
    targets: &mut std::collections::BTreeSet<(String, i64)>,
    raw_sources: &mut std::collections::BTreeSet<(DateTime<Utc>, String)>,
    source_limit: &mut usize,
) -> bool {
    let Some(candidate) = item.get("candidate") else {
        return false;
    };
    let Some(citations) = candidate.get("sources").and_then(Value::as_array) else {
        return false;
    };
    if citations.len() > MAX_SOURCES {
        return false;
    }
    if let Some(dependencies) = candidate
        .get("subject_scope")
        .and_then(|scope| scope.get("dependencies"))
    {
        let Some(dependencies) = dependencies
            .as_array()
            .filter(|items| items.len() <= MAX_RESEARCH_SOURCES)
        else {
            return false;
        };
        *source_limit = MAX_RESEARCH_SOURCES;
        for dependency in dependencies {
            let Some(reference) = exact_ref(dependency, "entry_ref", "version") else {
                return false;
            };
            sources.insert(reference);
        }
    }
    if !audit_location_context(&candidate["evidence_scope"], sources) {
        return false;
    }
    if let Some(version) = candidate
        .get("expected_version")
        .and_then(Value::as_i64)
        .filter(|version| *version > 0)
    {
        let Some(path) = candidate.get("path").and_then(Value::as_str) else {
            return false;
        };
        targets.insert((path.to_owned(), version));
    }
    if let Some(raw) = candidate.get("raw_sources") {
        let Some(references) = raw.as_array().and_then(|raw| raw_refs(raw)) else {
            return false;
        };
        raw_sources.extend(references);
    }
    if citations.is_empty()
        && candidate
            .get("raw_sources")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        let Some(reference) = exact_ref(item, "run_entry_ref", "run_version") else {
            return false;
        };
        audits.push(reference);
    } else {
        for citation in citations {
            let Some(reference) = exact_ref(citation, "entry_ref", "version") else {
                return false;
            };
            sources.insert(reference);
        }
    }
    if *source_limit == MAX_RESEARCH_SOURCES {
        // A full research manifest may still have an existing publication
        // target to audit; that target is not another research dependency.
        sources.len() <= *source_limit && targets.len() + raw_sources.len() <= MAX_SOURCES
    } else {
        sources.len() + targets.len() + raw_sources.len() <= *source_limit
    }
}

fn audit_location_context(
    scope: &Value,
    sources: &mut std::collections::BTreeSet<(Uuid, i64)>,
) -> bool {
    let Some(context) = scope.get("context_sources") else {
        return true;
    };
    let Some(context) = context.as_array().filter(|sources| sources.len() <= 12) else {
        return false;
    };
    for source in context {
        let Some(reference) = exact_ref(source, "entry_ref", "version") else {
            return false;
        };
        sources.insert(reference);
    }
    sources.len() <= MAX_RESEARCH_SOURCES
}

/// Validate the exact dependency graph retained in immutable run/review audits.
/// Current source visibility is mandatory even for an explicitly historical audit.
async fn audit_access(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    selected: &Document,
) -> ApiResult<bool> {
    let mut queue = vec![(selected.id, selected.version)];
    let mut visited = std::collections::BTreeSet::new();
    let mut sources = std::collections::BTreeSet::new();
    let mut research_sources = std::collections::BTreeSet::new();
    let mut targets = std::collections::BTreeSet::new();
    let mut raw_sources = std::collections::BTreeSet::new();
    let mut source_limit = MAX_SOURCES;
    while let Some(reference) = queue.pop() {
        if !visited.insert(reference) {
            continue;
        }
        if visited.len() > MAX_AUDITS || queue.len() > MAX_AUDITS * MAX_SOURCES {
            return Ok(false);
        }
        let Some(doc) = document(tx, auth, reference.0, Some(reference.1)).await? else {
            return Ok(false);
        };
        let metadata = &doc.metadata;
        if let Some(research) = metadata.get("dreamer_research") {
            // Work notes are generated evidence caches, even on historical
            // reads. Every retained dependency must remain visible. Research
            // may hold more source headers than one publication's citations.
            source_limit = MAX_RESEARCH_SOURCES;
            if research["schema"] != "dream.research.v1" {
                return Ok(false);
            }
            for key in ["sources", "reviewed_sources"] {
                let Some(dependencies) = research[key]
                    .as_array()
                    .filter(|items| items.len() <= source_limit)
                else {
                    return Ok(false);
                };
                for dependency in dependencies {
                    let Some(reference) = exact_ref(dependency, "entry_ref", "version") else {
                        return Ok(false);
                    };
                    sources.insert(reference);
                    research_sources.insert(reference);
                }
            }
        } else if let Some(receipt) = metadata.get("dreamer_receipt") {
            let Some(run) = exact_ref(receipt, "receipt_ref", "receipt_version") else {
                return Ok(false);
            };
            queue.push(run);
        } else if let Some(review) = metadata.get("dreamer_review") {
            if review.get("schema").and_then(Value::as_str) != Some("dream.review.v1")
                || !review.get("item").is_some_and(|item| {
                    audit_item(
                        item,
                        &mut sources,
                        &mut queue,
                        &mut targets,
                        &mut raw_sources,
                        &mut source_limit,
                    )
                })
            {
                return Ok(false);
            }
        } else if let Some(container) = metadata
            .get("dreamer_run")
            .or_else(|| metadata.get("dreamer_state"))
        {
            for key in ["location_work", "location_scopes"] {
                if let Some(scopes) = container.get(key) {
                    let Some(scopes) = scopes.as_array().filter(|scopes| scopes.len() <= 31) else {
                        return Ok(false);
                    };
                    for scope in scopes {
                        if !audit_location_context(scope, &mut sources) {
                            return Ok(false);
                        }
                    }
                }
            }
            if !audit_location_context(&container["active"]["location_work"], &mut sources) {
                return Ok(false);
            }
            if let Some(dispositions) = container.get("location_dispositions") {
                let Some(dispositions) = dispositions.as_array().filter(|items| items.len() <= 31)
                else {
                    return Ok(false);
                };
                for disposition in dispositions {
                    if !audit_location_context(&disposition["work"], &mut sources) {
                        return Ok(false);
                    }
                }
            }
            if metadata.get("dreamer_run").is_some()
                && container.get("schema").and_then(Value::as_str) != Some("dream.run.v1")
            {
                return Ok(false);
            }
            let Some(items) = container.get("items").and_then(Value::as_array) else {
                return Ok(false);
            };
            if items.len() > 96 {
                return Ok(false);
            }
            for item in items {
                if !audit_item(
                    item,
                    &mut sources,
                    &mut queue,
                    &mut targets,
                    &mut raw_sources,
                    &mut source_limit,
                ) {
                    return Ok(false);
                }
            }
            // Archived imported notes retain exact original-run dependencies,
            // including when their cached text has been compacted in state.
            if let Some(legacy) = container.get("legacy_items") {
                let Some(legacy) = legacy.as_array().filter(|items| items.len() <= 96) else {
                    return Ok(false);
                };
                for item in legacy {
                    if !audit_item(
                        item,
                        &mut sources,
                        &mut queue,
                        &mut targets,
                        &mut raw_sources,
                        &mut source_limit,
                    ) {
                        return Ok(false);
                    }
                }
            }
            if let Some(pending) = container.get("pending_refs").and_then(Value::as_array) {
                if pending.len() > 96 {
                    return Ok(false);
                }
                for item in pending {
                    let Some(reference) = exact_ref(item, "entry_ref", "version") else {
                        return Ok(false);
                    };
                    queue.push(reference);
                }
            }
            if let Some(history) = container.get("history").and_then(Value::as_array) {
                if history.len() > 32 {
                    return Ok(false);
                }
                for decision in history {
                    let Some(reference) = exact_ref(decision, "run_entry_ref", "run_version")
                    else {
                        return Ok(false);
                    };
                    queue.push(reference);
                }
            }
        } else if !protected_metadata(metadata)
            && !doc
                .path
                .to_ascii_lowercase()
                .starts_with("dreams/research/")
        {
            // Existing untyped legacy runs remain ordinary owner-visible sources.
            sources.insert(reference);
        } else {
            return Ok(false);
        }
        if sources.len() > source_limit {
            return Ok(false);
        }
    }
    if sources.is_empty() && raw_sources.is_empty() {
        return Ok(false);
    }
    let (ids, versions): (Vec<_>, Vec<_>) = sources.iter().copied().unzip();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM unnest($2::uuid[],$3::bigint[]) AS required(id,version) JOIN brunn.entries e ON e.user_id=$1 AND e.id=required.id AND e.deleted_at IS NULL JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=required.version")
        .bind(auth.user_id.0).bind(ids).bind(versions).fetch_one(&mut **tx).await?;
    if visible != sources.len() as i64 {
        return Ok(false);
    }
    if !research_sources.is_empty() {
        // Exact research notes and diagnostics are cached source-dependent
        // text. Visibility alone is insufficient: a source can still be
        // readable after becoming excluded from research. Check both its
        // original metadata and its current head without rewriting history.
        let (ids, versions): (Vec<_>, Vec<_>) = research_sources.iter().copied().unzip();
        let rows = sqlx::query(
            r#"
            SELECT e.path, exact.metadata, head.metadata AS current_metadata
            FROM unnest($2::uuid[],$3::bigint[]) AS required(id,version)
            JOIN brunn.entries e ON e.user_id=$1 AND e.id=required.id
                AND e.deleted_at IS NULL AND e.kind='markdown'
            JOIN brunn.entry_versions exact ON exact.user_id=e.user_id
                AND exact.entry_id=e.id AND exact.version=required.version
                AND exact.content IS NOT NULL
            JOIN brunn.entry_versions head ON head.user_id=e.user_id
                AND head.entry_id=e.id AND head.version=e.current_version
                AND head.content IS NOT NULL AND head.size_bytes<=1048576
        "#,
        )
        .bind(auth.user_id.0)
        .bind(ids)
        .bind(versions)
        .fetch_all(&mut **tx)
        .await?;
        if rows.len() != research_sources.len()
            || rows.iter().any(|row| {
                let path: &str = row.get("path");
                crate::dreamer_review::research_source_excluded(path, &row.get("metadata"))
                    || crate::dreamer_review::research_source_excluded(
                        path,
                        &row.get("current_metadata"),
                    )
            })
        {
            return Ok(false);
        }
    }
    let (paths, versions): (Vec<_>, Vec<_>) = targets.iter().cloned().unzip();
    let targets_visible: i64 = sqlx::query_scalar("SELECT count(*) FROM unnest($2::text[],$3::bigint[]) AS required(path,version) JOIN brunn.entries e ON e.user_id=$1 AND e.path=required.path AND e.deleted_at IS NULL JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=required.version")
        .bind(auth.user_id.0).bind(paths).bind(versions).fetch_one(&mut **tx).await?;
    if targets_visible != targets.len() as i64 {
        return Ok(false);
    }
    raw_access(tx, auth, &raw_sources).await
}

fn withheld(id: Uuid, version: Option<i64>, reason: &str, generation: i64) -> Value {
    json!({"reference":format!("entry:{id}"),"version":version,"title":"Summary unavailable",
        "representation":"summary_withheld","text":"","freshness":{"status":"unchecked","reason":reason,"checked_generation":generation}})
}

fn receipt_status(
    selected: &Document,
    request: &ReadItem,
    max_chars: usize,
    generation: i64,
) -> Option<Value> {
    let mut receipt = crate::dreamer::receipt::parse_latest(&selected.content).ok()?;
    for category in [
        "applied_writes",
        "entering_veto_window_today",
        "pending_owner",
        "pending_review_surfaces",
    ] {
        for row in receipt.get_mut(category)?.as_array_mut()? {
            row["summary"] = json!("Evidence unavailable; open Review for current details.");
            if row.get("reason").is_some() {
                row["reason"] = json!("Cached source-dependent text has been withheld.");
            }
        }
    }
    let mut sanitized = selected.clone();
    sanitized.title = "Dreamer latest receipt".to_owned();
    sanitized.content = crate::dreamer::receipt::render_latest(&receipt).ok()?;
    sanitized.metadata = json!({});
    let mut value = render(&sanitized, request, max_chars);
    value.as_object_mut()?.remove("content_hash");
    value["representation"] = json!("operational_receipt");
    value["content_redacted"] = json!(true);
    value["freshness"] = json!({"status":"source_unavailable","reason":"cached_recommendation_text_withheld","checked_generation":generation});
    Some(value)
}

async fn project(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    original: &Value,
    request: &ReadItem,
    max_chars: usize,
    alternatives: &mut usize,
) -> ApiResult<Value> {
    let Some(id) = original
        .get("reference")
        .and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("entry:"))
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return Ok(original.clone());
    };
    let generation: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(generation),0) FROM brunn.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut **tx)
    .await?;
    let Some(selected) = document(tx, auth, id, request.version).await? else {
        return Ok(withheld(
            id,
            request.version,
            "source_unavailable",
            generation,
        ));
    };
    let is_summary =
        selected.metadata.get("dreamer_summary").is_some() || managed_path(&selected.path);
    if !is_summary
        && (protected_metadata(&selected.metadata)
            || selected
                .path
                .to_ascii_lowercase()
                .starts_with("dreams/research/"))
    {
        if !audit_access(tx, auth, &selected).await? {
            if selected.metadata.get("dreamer_receipt").is_some()
                && let Some(value) = receipt_status(&selected, request, max_chars, generation)
            {
                return Ok(value);
            }
            let mut value = withheld(
                id,
                Some(selected.version),
                "audit_source_unavailable_or_unproven",
                generation,
            );
            value["title"] = json!("Audit unavailable");
            value["representation"] = json!("audit_withheld");
            return Ok(value);
        }
        let mut value = render(&selected, request, max_chars);
        value["representation"] = json!("audit_snapshot");
        value["freshness"] = json!({"status":"historical","reason":"source_access_checked","checked_generation":generation});
        return Ok(value);
    }
    if is_summary {
        if *alternatives == 0 {
            return Ok(withheld(
                id,
                Some(selected.version),
                "summary_check_limit",
                generation,
            ));
        }
        *alternatives -= 1;
        let check = check(tx, auth, &selected).await?;
        if check.inaccessible && request.version.is_some() {
            return Ok(withheld(
                id,
                Some(selected.version),
                "source_unavailable",
                generation,
            ));
        }
        if check.status == "fresh" || request.version.is_some() {
            let mut value = render(&selected, request, max_chars);
            value["representation"] = json!(if request.version.is_some() {
                "historical_summary"
            } else {
                "derived_summary"
            });
            value["freshness"] = freshness(&check, generation);
            return Ok(value);
        }
        if let Some(source) = check.sources.first() {
            // A range of a stale summary is not a range of a source document.
            let mut full = request.clone();
            full.view = Some("full".to_owned());
            let source = document(tx, auth, source.id, None)
                .await?
                .expect("source head exists in the same snapshot");
            let mut value = render(&source, &full, max_chars);
            value["representation"] = json!("current_source_fallback");
            value["freshness"] = freshness(&check, generation);
            return Ok(value);
        }
        return Ok(withheld(
            id,
            Some(selected.version),
            check.reason,
            generation,
        ));
    }
    let mut value = render(&selected, request, max_chars);
    if request.version.is_some() || request.view.as_deref() != Some("current_state") {
        return Ok(value);
    }
    let mut last_reason = if *alternatives == 0 {
        "summary_check_limit"
    } else {
        "no_published_summary"
    };
    let rows = sqlx::query("SELECT e.id FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.deleted_at IS NULL AND (e.path LIKE 'derived/entities/%' OR e.path LIKE 'derived/location/%') AND v.metadata @> $2 ORDER BY (v.metadata->'dreamer_summary'->>'subject_ref'=$4) DESC NULLS LAST,e.updated_at DESC,e.id LIMIT $3")
        .bind(auth.user_id.0).bind(json!({"dreamer_summary":{"sources":[{"entry_ref":format!("entry:{id}")}]}})).bind(*alternatives as i64)
        .bind(format!("entry:{id}"))
        .fetch_all(&mut **tx).await?;
    for row in rows {
        *alternatives -= 1;
        let summary_id: Uuid = row.try_get("id")?;
        let Some(summary) = document(tx, auth, summary_id, None).await? else {
            continue;
        };
        let check = check(tx, auth, &summary).await?;
        let canonical = summary.metadata["dreamer_summary"]["subject_ref"] == format!("entry:{id}");
        last_reason = check.reason;
        if check.status == "fresh" {
            let mut summary_value = render(&summary, request, max_chars);
            summary_value["representation"] = json!("derived_summary");
            summary_value["freshness"] = freshness(&check, generation);
            summary_value["requested_source"] =
                json!({"reference":format!("entry:{id}"),"version":selected.version});
            return Ok(summary_value);
        }
        if canonical {
            // Do not bypass a stale canonical overview with a narrow legacy
            // summary whose older freshness contract knows less.
            break;
        }
    }
    value["representation"] = json!("current_source_fallback");
    value["freshness"] =
        json!({"status":"current_source","reason":last_reason,"checked_generation":generation});
    Ok(value)
}

/// Called only for current_state reads or server-managed summaries. Historical
/// source reads remain on the existing exact-version path without substitution.
pub async fn project_read(
    state: &AppState,
    auth: &AuthContext,
    original: Value,
    request: &ReadItem,
    max_chars: usize,
    alternatives: &mut usize,
) -> ApiResult<Value> {
    let mut tx = snapshot(state, auth).await?;
    let mut request = request.clone();
    if !state.config.dreamer_summary_reads_enabled
        && request.view.as_deref() == Some("current_state")
    {
        request.view = Some("full".to_owned());
    }
    let value = project(&mut tx, auth, &original, &request, max_chars, alternatives).await?;
    tx.commit().await?;
    Ok(value)
}

/// Search accelerators can contain old summary snippets, headings and matches.
/// Replace the entire rendered candidate from one fresh validation snapshot;
/// never leave any of those cached fields beside a source fallback.
pub async fn protect_evidence(
    state: &AppState,
    auth: &AuthContext,
    items: &mut [Value],
    alternatives: &mut usize,
) -> ApiResult<()> {
    if !items.iter().any(|item| {
        item.get("path")
            .and_then(Value::as_str)
            .is_some_and(protected_path)
    }) {
        return Ok(());
    }
    let mut tx = snapshot(state, auth).await?;
    for item in items {
        if !item
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(protected_path)
        {
            continue;
        }
        let max_chars = item
            .get("text")
            .or_else(|| item.get("excerpt"))
            .and_then(Value::as_str)
            .map_or(0, |text| text.chars().count());
        let request: ReadItem =
            serde_json::from_value(json!({"view":"full"})).expect("static read request");
        *item = project(&mut tx, auth, item, &request, max_chars, alternatives).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Config,
        auth::hash_token,
        models::{CredentialId, UserId},
    };
    use sqlx::{PgPool, postgres::PgPoolOptions};
    #[test]
    fn generated_briefing_evidence_classification_does_not_protect_writes() {
        for metadata in [
            json!({"kind":"briefing_edition"}),
            json!({"kind":"briefing_edition","briefing":{"schema":"briefing.v1"}}),
        ] {
            assert!(generated_briefing_metadata(&metadata));
            assert!(!protected_metadata(&metadata));
        }
        for metadata in [
            json!({}),
            json!({"briefing":{"schema":"briefing.v1"}}),
            json!({"kind":"ordinary_note","briefing":true}),
            json!({"kind":"Briefing_Edition"}),
            json!({"kind":["briefing_edition"]}),
        ] {
            assert!(!generated_briefing_metadata(&metadata));
        }
        assert!(!protected_path("Briefings/2026/Edition.md"));
    }

    #[test]
    fn manifests_are_bounded_and_require_exact_sources() {
        let source = json!({"entry_ref":format!("entry:{}",Uuid::nil()),"version":1});
        let mut value = json!({"dreamer_summary":{"schema":"dream.summary.v1","state":"published","frozen_generation":1,"sources":[source.clone()],"scope_prefixes":["sources/"]}});
        assert!(manifest(&value).is_some());
        value["dreamer_summary"]["sources"][0]["version"] = json!(0);
        assert!(manifest(&value).is_none());
        value["dreamer_summary"]["sources"] = json!(vec![source; MAX_SOURCES + 1]);
        assert!(manifest(&value).is_none());
    }
    #[test]
    fn prefix_checks_are_literal_and_exclude_derived_churn() {
        let prefixes = vec!["sources/100%_done/".to_owned()];
        assert!(relevant_change("sources/100%_done/new.md", &prefixes));
        assert!(!relevant_change(
            "sources/100percent_done/new.md",
            &prefixes
        ));
        assert!(!relevant_change("derived/entities/a.md", &[String::new()]));
        assert!(!relevant_change("dreams/state.md", &[String::new()]));
    }

    #[test]
    fn a_full_research_dependency_manifest_can_audit_an_existing_output() {
        let dependencies = (0..MAX_RESEARCH_SOURCES)
            .map(|_| json!({"entry_ref":format!("entry:{}",Uuid::now_v7()),"version":1}))
            .collect::<Vec<_>>();
        let item = json!({"candidate":{"sources":[dependencies[0].clone()],"path":"derived/entities/aster.md","expected_version":1,"subject_scope":{"dependencies":dependencies}}});
        let mut sources = std::collections::BTreeSet::new();
        let mut targets = std::collections::BTreeSet::new();
        let mut audits = vec![];
        let mut raw = std::collections::BTreeSet::new();
        let mut source_limit = MAX_SOURCES;
        assert!(audit_item(
            &item,
            &mut sources,
            &mut audits,
            &mut targets,
            &mut raw,
            &mut source_limit
        ));
        assert_eq!(sources.len(), MAX_RESEARCH_SOURCES);
        assert_eq!(targets.len(), 1);
    }

    async fn fixture() -> Option<(PgPool, AppState, AuthContext)> {
        let Some(url) = std::env::var("BRUNN_TEST_DATABASE_URL")
            .ok()
            .filter(|value| !value.is_empty())
        else {
            eprintln!("BRUNN_TEST_DATABASE_URL unset; skipping summary database contract");
            return None;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,'Summary test')",
        )
        .bind(user_id)
        .bind(format!("summary-test:{user_id}"))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO brunn.api_credentials (id,user_id,label,token_hash,capabilities) VALUES ($1,$2,'Summary test',$3,ARRAY['read','query','open'])")
            .bind(credential_id).bind(user_id).bind(hash_token(&credential_id.to_string())).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO brunn.credential_scope_grants (credential_id,user_id,scope_id) SELECT $1,$2,id FROM brunn.scopes WHERE user_id=$2 AND scope_ref='scope:root'")
            .bind(credential_id).bind(user_id).execute(&pool).await.unwrap();
        let mut config = Config::from_env().unwrap();
        let mut role_url = url::Url::parse(&url).unwrap();
        role_url
            .query_pairs_mut()
            .append_pair("options", "-c role=app_ro");
        config.database_url_ro = role_url.to_string();
        role_url.set_query(None);
        role_url
            .query_pairs_mut()
            .append_pair("options", "-c role=app_rw");
        config.database_url_rw = role_url.to_string();
        config.database_url_admin = None;
        config.database_max_connections = 4;
        config.apns_delivery_enabled = false;
        config.dreamer_summary_reads_enabled = true;
        let state = AppState::connect(config).await.unwrap();
        Some((
            pool,
            state,
            AuthContext {
                user_id: UserId(user_id),
                credential_id: CredentialId(credential_id),
                capabilities: ["read", "query", "open"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                scope_refs: vec!["scope:root".to_owned()],
                read_only: true,
            },
        ))
    }

    async fn put(
        pool: &PgPool,
        auth: &AuthContext,
        id: Uuid,
        path: &str,
        version: i64,
        content: &str,
        metadata: Value,
    ) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("INSERT INTO brunn.entries (id,user_id,path,title,kind,media_type,current_version) VALUES ($1,$2,$3,'Summary contract source','markdown','text/markdown',$4) ON CONFLICT (id) DO UPDATE SET current_version=excluded.current_version,updated_at=clock_timestamp()")
            .bind(id).bind(auth.user_id.0).bind(path).bind(version).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO brunn.entry_versions (entry_id,user_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(id).bind(auth.user_id.0).bind(version).bind(hash_token(content)).bind(content)
            .bind(content.len() as i64).bind(metadata).bind(auth.credential_id.0).execute(&mut *tx).await.unwrap();
        let generation: i64 = sqlx::query_scalar("INSERT INTO brunn.workspace_changes (user_id,entry_id,entry_version,operation,path,content_sha256) VALUES ($1,$2,$3,$4,$5,$6) RETURNING generation")
            .bind(auth.user_id.0).bind(id).bind(version).bind(if version == 1 {"create"} else {"update"})
            .bind(path).bind(hash_token(content)).fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        generation
    }

    fn metadata(source: Uuid, frozen: i64) -> Value {
        json!({"kind":"derived_summary","dreamer_summary":{"schema":"dream.summary.v1","state":"published","frozen_generation":frozen,
            "sources":[{"entry_ref":format!("entry:{source}"),"version":1}],"scope_prefixes":["sources/topic/"]}})
    }

    async fn read_item(
        state: &AppState,
        auth: &AuthContext,
        id: Uuid,
        view: &str,
        version: Option<i64>,
    ) -> Value {
        let response = crate::simple_core::read(axum::extract::State(state.clone()), axum::Extension(auth.clone()),
            axum::Json(serde_json::from_value(json!({"requests":[{"ref":format!("entry:{id}"),"view":view,"version":version}]})).unwrap()))
            .await.unwrap().0;
        serde_json::to_value(response).unwrap()["data"]["items"][0].clone()
    }

    async fn subject_scope(
        state: &AppState,
        auth: &AuthContext,
        id: Uuid,
        generation: i64,
    ) -> crate::dreamer_subject::SubjectScope {
        let mut tx = snapshot(state, auth).await.unwrap();
        let scope =
            crate::dreamer_subject::create_scope(&mut tx, auth, &format!("entry:{id}"), generation)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        scope
    }

    async fn subject_check(
        state: &AppState,
        auth: &AuthContext,
        scope: &crate::dreamer_subject::SubjectScope,
        source: Uuid,
    ) -> crate::dreamer_subject::SubjectCheck {
        let mut tx = snapshot(state, auth).await.unwrap();
        let check = crate::dreamer_subject::check_scope(&mut tx, auth, scope, &[(source, 1)])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        check
    }

    #[tokio::test]
    async fn database_subject_generated_briefing_churn_is_excluded_before_coverage_limits() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        let generation = put(
            &pool,
            &auth,
            canonical,
            "Projects/Aster.md",
            1,
            "# Aster\nPrimary project evidence",
            json!({}),
        )
        .await;
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        let overview = Uuid::now_v7();
        let mut overview_metadata = metadata(canonical, generation);
        overview_metadata["dreamer_summary"]["subject_ref"] = json!(format!("entry:{canonical}"));
        overview_metadata["dreamer_summary"]["subject_scope"] = json!(scope);
        put(
            &pool,
            &auth,
            overview,
            "derived/entities/aster.md",
            1,
            "Supported Aster overview",
            overview_metadata,
        )
        .await;
        // Legacy prefix manifests use the same edition exclusion without
        // requiring a new subject_scope or silently rewriting old metadata.
        let legacy = Uuid::now_v7();
        let mut legacy_metadata = metadata(canonical, generation);
        legacy_metadata["dreamer_summary"]["scope_prefixes"] = json!(["Briefings/"]);
        put(
            &pool,
            &auth,
            legacy,
            "derived/entities/legacy-aster.md",
            1,
            "Legacy supported overview",
            legacy_metadata,
        )
        .await;
        for (index, briefing_metadata) in [
            json!({"kind":"briefing_edition"}),
            json!({"kind":"briefing_edition","briefing":{"schema":"briefing.v1"}}),
        ]
        .into_iter()
        .enumerate()
        {
            let briefing = Uuid::now_v7();
            let path = format!("Briefings/edition-{index}.md");
            let initial = "# Morning\nAster remains in an unchanged costs section.";
            let updated = format!("{initial}\nAn unrelated world-news addition.");
            put(
                &pool,
                &auth,
                briefing,
                &path,
                1,
                initial,
                briefing_metadata.clone(),
            )
            .await;
            assert_eq!(
                read_item(&state, &auth, briefing, "full", None).await["text"],
                initial
            );
            put(
                &pool,
                &auth,
                briefing,
                &path,
                2,
                &updated,
                briefing_metadata,
            )
            .await;
            assert_eq!(
                read_item(&state, &auth, briefing, "full", None).await["text"],
                updated
            );
            assert_eq!(
                read_item(&state, &auth, briefing, "full", Some(1)).await["text"],
                initial
            );
            // Real feed pages can contain more generated churn than the cap.
            // Excluding it after LIMIT would incorrectly make coverage unchecked.
            sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT $1,$2,2,'update',$3,$4 FROM generate_series(1,2100)")
                .bind(auth.user_id.0).bind(briefing).bind(&path).bind(hash_token(&updated)).execute(&pool).await.unwrap();
            sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
                .bind(briefing)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) VALUES($1,$2,2,'delete',$3,$4)")
                .bind(auth.user_id.0).bind(briefing).bind(&path).bind(hash_token(&updated)).execute(&pool).await.unwrap();
        }
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let changes =
            crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                .await
                .unwrap();
        assert_eq!(changes.status, "complete");
        assert!(changes.relevant_ids.is_empty());
        assert_eq!(changes.scanned_generation, changes.through_generation);
        tx.commit().await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "fresh"
        );
        assert_eq!(
            read_item(&state, &auth, canonical, "current_state", None).await["text"],
            "Supported Aster overview"
        );
        assert_eq!(
            read_item(&state, &auth, legacy, "full", None).await["freshness"]["status"],
            "fresh"
        );

        // Neither the directory nor a generic briefing annotation classifies
        // ordinary sources as generated editions.
        for (index, ordinary_metadata) in [
            json!({}),
            json!({"briefing":{"schema":"briefing.v1"}}),
            json!({"kind":"ordinary_note","briefing":true}),
        ]
        .into_iter()
        .enumerate()
        {
            let generation: i64 = sqlx::query_scalar(
                "SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1",
            )
            .bind(auth.user_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
            let scope = subject_scope(&state, &auth, canonical, generation).await;
            let ordinary = Uuid::now_v7();
            put(
                &pool,
                &auth,
                ordinary,
                &format!("Briefings/primary-{index}.md"),
                1,
                "Aster has a new primary outcome.",
                ordinary_metadata,
            )
            .await;
            let mut tx = snapshot(&state, &auth).await.unwrap();
            let changes =
                crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                    .await
                    .unwrap();
            assert_eq!(changes.status, "complete");
            assert_eq!(changes.relevant_ids, [ordinary]);
            tx.commit().await.unwrap();
            assert_eq!(
                subject_check(&state, &auth, &scope, canonical).await.reason,
                "subject_scope_changed"
            );
        }
        assert_eq!(
            read_item(&state, &auth, legacy, "full", None).await["freshness"]["status"],
            "stale"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_generated_briefing_transitions_keep_primary_changes_relevant() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        put(
            &pool,
            &auth,
            canonical,
            "Topics/Aster.md",
            1,
            "# Aster",
            json!({}),
        )
        .await;
        let changing = Uuid::now_v7();
        let path = "Briefings/transition.md";
        let generation = put(
            &pool,
            &auth,
            changing,
            path,
            1,
            "Aster primary evidence",
            json!({}),
        )
        .await;
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        let generation = put(
            &pool,
            &auth,
            changing,
            path,
            2,
            "Unrelated generated edition",
            json!({"kind":"briefing_edition"}),
        )
        .await;
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let changes =
            crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                .await
                .unwrap();
        assert_eq!(
            changes.relevant_ids,
            [changing],
            "removed primary mentions remain relevant through previous metadata"
        );
        tx.commit().await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_scope_changed"
        );

        let scope = subject_scope(&state, &auth, canonical, generation).await;
        let generation = put(
            &pool,
            &auth,
            changing,
            path,
            3,
            "Aster new primary outcome",
            json!({"briefing":{"schema":"briefing.v1"}}),
        )
        .await;
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let changes =
            crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                .await
                .unwrap();
        assert_eq!(
            changes.relevant_ids,
            [changing],
            "a generated-to-ordinary transition must be admitted"
        );
        tx.commit().await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_scope_changed"
        );

        let scope = subject_scope(&state, &auth, canonical, generation).await;
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(changing)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) VALUES($1,$2,3,'delete',$3,$4)")
            .bind(auth.user_id.0).bind(changing).bind(path).bind(hash_token("Aster new primary outcome")).execute(&pool).await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_scope_changed"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_generated_briefing_dependencies_remain_invalid_and_immutable() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        put(
            &pool,
            &auth,
            canonical,
            "Projects/Aster.md",
            1,
            "# Aster\nPrimary evidence",
            json!({}),
        )
        .await;
        let briefing = Uuid::now_v7();
        let path = "Briefings/legacy-edition.md";
        let generation = put(
            &pool,
            &auth,
            briefing,
            path,
            1,
            "Aster generated report",
            json!({"kind":"briefing_edition"}),
        )
        .await;
        let mut scope = subject_scope(&state, &auth, canonical, generation).await;
        let mut tx = snapshot(&state, &auth).await.unwrap();
        assert!(
            crate::dreamer_subject::create_scope(
                &mut tx,
                &auth,
                &format!("entry:{briefing}"),
                generation
            )
            .await
            .is_err()
        );
        let original_scope = json!(scope);
        assert!(
            crate::dreamer_subject::bind_dependencies(
                &mut tx,
                &auth,
                &mut scope,
                &[(canonical, 1), (briefing, 1)]
            )
            .await
            .is_err()
        );
        assert_eq!(
            json!(scope),
            original_scope,
            "failed binding must not partially mutate the scope"
        );
        tx.commit().await.unwrap();

        // Simulate an immutable proposal admitted before the policy correction.
        // Its generated dependency was researched but not cited in the claims.
        scope.dependencies = vec![
            crate::dreamer_subject::SubjectDependency {
                entry_ref: format!("entry:{canonical}"),
                version: 1,
                path: "Projects/Aster.md".into(),
            },
            crate::dreamer_subject::SubjectDependency {
                entry_ref: format!("entry:{briefing}"),
                version: 1,
                path: path.into(),
            },
        ];
        let original_scope = json!(scope);
        let mut overview_metadata = metadata(canonical, generation);
        overview_metadata["dreamer_summary"]["subject_ref"] = json!(format!("entry:{canonical}"));
        overview_metadata["dreamer_summary"]["subject_scope"] = original_scope.clone();
        let overview = Uuid::now_v7();
        let cached = "CACHED_GENERATED_OVERVIEW";
        put(
            &pool,
            &auth,
            overview,
            "derived/entities/aster.md",
            1,
            cached,
            overview_metadata.clone(),
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_source_unavailable"
        );
        assert_eq!(
            read_item(&state, &auth, canonical, "current_state", None).await["representation"],
            "current_source_fallback"
        );
        assert_eq!(
            read_item(&state, &auth, overview, "full", Some(1)).await["representation"],
            "summary_withheld"
        );

        // Changing the current kind cannot launder a generated exact version
        // retained in an older manifest into primary evidence.
        put(
            &pool,
            &auth,
            briefing,
            path,
            2,
            "Aster primary replacement",
            json!({}),
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_source_unavailable"
        );
        let legacy = Uuid::now_v7();
        let mut legacy_metadata = metadata(briefing, generation);
        legacy_metadata["dreamer_summary"]["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"entry_ref":format!("entry:{briefing}"),"version":2}));
        put(
            &pool,
            &auth,
            legacy,
            "derived/entities/legacy.md",
            1,
            cached,
            legacy_metadata,
        )
        .await;
        assert_eq!(
            read_item(&state, &auth, legacy, "full", Some(1)).await["representation"],
            "summary_withheld"
        );
        assert_eq!(
            read_item(&state, &auth, briefing, "full", Some(1)).await["text"],
            "Aster generated report"
        );
        assert_eq!(
            read_item(&state, &auth, briefing, "full", None).await["text"],
            "Aster primary replacement"
        );
        let saved = sqlx::query("SELECT metadata,content,content_sha256 FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=1")
            .bind(auth.user_id.0).bind(overview).fetch_one(&pool).await.unwrap();
        assert_eq!(saved.get::<Value, _>("metadata"), overview_metadata);
        assert_eq!(saved.get::<String, _>("content"), cached);
        assert_eq!(saved.get::<String, _>("content_sha256"), hash_token(cached));
        assert_eq!(json!(scope), original_scope);
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_identity_uses_explicit_names_and_preserves_old_manifest_dependencies()
    {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        let source = "## Purpose\n\nCanonical project source.";
        let mut generation = put(
            &pool,
            &auth,
            canonical,
            "sources/Projects/Ithrion.md",
            1,
            source,
            json!({"title":"Aurora Atlas", "aliases":["The Meridian"], "alias":"North Relay"}),
        )
        .await;
        // Simulate the normalizer's first-heading display title, retaining the
        // explicit structured metadata separately from that inferred label.
        let inferred =
            crate::ingest::normalize_document("sources/Projects/Ithrion.md", source).title;
        assert_eq!(inferred, "Purpose");
        sqlx::query("UPDATE brunn.entries SET title=$2 WHERE id=$1")
            .bind(canonical)
            .bind(inferred)
            .execute(&pool)
            .await
            .unwrap();
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        assert_eq!(
            scope.names,
            ["aurora atlas", "ithrion", "north relay", "the meridian"]
        );
        let unrelated = Uuid::now_v7();
        generation = put(
            &pool,
            &auth,
            unrelated,
            "Elsewhere/Other.md",
            1,
            "## Purpose\n\nAn unrelated project with the same section heading.",
            json!({}),
        )
        .await;
        sqlx::query("UPDATE brunn.entries SET title='Purpose' WHERE id=$1")
            .bind(unrelated)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let changes =
            crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                .await
                .unwrap();
        assert_eq!(changes.status, "complete");
        assert!(
            changes.relevant_ids.is_empty(),
            "an unrelated section heading must not be admitted as subject evidence"
        );
        tx.commit().await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "fresh"
        );

        // Each source-owned name and exact link remains independently relevant.
        for (index, mention) in [
            "Ithrion",
            "Aurora Atlas",
            "The Meridian",
            "North Relay",
            "[[sources/Projects/Ithrion.md]]",
            &format!("entry:{canonical}"),
        ]
        .into_iter()
        .enumerate()
        {
            let scope = subject_scope(&state, &auth, canonical, generation).await;
            let related = Uuid::now_v7();
            generation = put(
                &pool,
                &auth,
                related,
                &format!("Elsewhere/Related-{index}.md"),
                1,
                &format!("## Purpose\n\nAn outcome for {mention}."),
                json!({}),
            )
            .await;
            let mut tx = snapshot(&state, &auth).await.unwrap();
            let changes =
                crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                    .await
                    .unwrap();
            assert_eq!(
                changes.relevant_ids,
                [related],
                "missing relationship through {mention}"
            );
            tx.commit().await.unwrap();
            assert_eq!(
                subject_check(&state, &auth, &scope, canonical).await.reason,
                "subject_scope_changed"
            );
        }

        let mut scope = subject_scope(&state, &auth, canonical, generation).await;
        let mut tx = snapshot(&state, &auth).await.unwrap();
        crate::dreamer_subject::bind_dependencies(&mut tx, &auth, &mut scope, &[(canonical, 1)])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(scope.dependencies.len(), 1);
        let summary = Uuid::now_v7();
        let mut manifest = metadata(canonical, generation);
        manifest["dreamer_summary"]["subject_ref"] = json!(format!("entry:{canonical}"));
        manifest["dreamer_summary"]["subject_scope"] = json!(scope);
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/ithrion.md",
            1,
            "Current canonical overview",
            manifest,
        )
        .await;
        let selected = read_item(&state, &auth, canonical, "current_state", None).await;
        assert_eq!(selected["text"], "Current canonical overview");
        assert_eq!(selected["freshness"]["status"], "fresh");

        // Older admitted evidence remains conservative, even if it was found
        // through a former inferred name. Reading cannot rewrite approved bytes.
        let mut legacy = scope.clone();
        let mut tx = snapshot(&state, &auth).await.unwrap();
        crate::dreamer_subject::bind_dependencies(
            &mut tx,
            &auth,
            &mut legacy,
            &[(canonical, 1), (unrelated, 1)],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        legacy.names.push("purpose".into());
        legacy.names.sort();
        let original_legacy = json!(legacy);
        let mut manifest = metadata(canonical, generation);
        manifest["dreamer_summary"]["subject_ref"] = json!(format!("entry:{canonical}"));
        manifest["dreamer_summary"]["subject_scope"] = original_legacy.clone();
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/ithrion.md",
            2,
            "Older identity overview",
            manifest,
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &legacy, canonical)
                .await
                .reason,
            "subject_identity_changed"
        );
        let selected = read_item(&state, &auth, canonical, "current_state", None).await;
        assert_eq!(selected["representation"], "current_source_fallback");
        assert_eq!(selected["freshness"]["reason"], "subject_identity_changed");
        assert_eq!(selected["text"], source);
        assert_eq!(
            read_item(&state, &auth, summary, "full", Some(2)).await["representation"],
            "historical_summary"
        );
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(unrelated)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            subject_check(&state, &auth, &legacy, canonical)
                .await
                .reason,
            "subject_source_unavailable"
        );
        assert_eq!(
            read_item(&state, &auth, summary, "full", Some(2)).await["representation"],
            "summary_withheld"
        );
        let retained: Value = sqlx::query_scalar("SELECT metadata->'dreamer_summary'->'subject_scope' FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=2")
            .bind(auth.user_id.0).bind(summary).fetch_one(&pool).await.unwrap();
        assert_eq!(retained, original_legacy);
        assert_eq!(json!(legacy), original_legacy);
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_scope_checks_cross_directory_changes_and_coverage() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        let frozen = put(
            &pool,
            &auth,
            canonical,
            "People/Aster.md",
            1,
            "# Aster",
            json!({"aliases":["Aster Vale", "Pilot Nimbus"]}),
        )
        .await;
        sqlx::query("UPDATE brunn.entries SET title='Aster' WHERE id=$1")
            .bind(canonical)
            .execute(&pool)
            .await
            .unwrap();
        let scope = subject_scope(&state, &auth, canonical, frozen).await;
        let generated = Uuid::now_v7();
        put(
            &pool,
            &auth,
            generated,
            "dreams/research/job.md",
            1,
            "Aster research churn",
            json!({}),
        )
        .await;
        sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT $1,$2,1,'update','dreams/research/job.md',$3 FROM generate_series(1,2100)")
            .bind(auth.user_id.0).bind(generated).bind(hash_token("Aster research churn")).execute(&pool).await.unwrap();
        let generated_metadata = Uuid::now_v7();
        put(
            &pool,
            &auth,
            generated_metadata,
            "Elsewhere/generated-note.md",
            1,
            "Aster generated noise",
            json!({"dreamer_run":{"schema":"dream.run.v1"}}),
        )
        .await;
        sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT $1,$2,1,'update','Elsewhere/generated-note.md',$3 FROM generate_series(1,2100)")
            .bind(auth.user_id.0).bind(generated_metadata).bind(hash_token("Aster generated noise")).execute(&pool).await.unwrap();
        let unrelated = Uuid::now_v7();
        put(
            &pool,
            &auth,
            unrelated,
            "Elsewhere/unrelated.md",
            1,
            "An unrelated Orchid outcome",
            json!({}),
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "fresh"
        );

        let linked = Uuid::now_v7();
        let generation = put(
            &pool,
            &auth,
            linked,
            "Imported/Distant/outcome.md",
            1,
            "Pilot Nimbus completed the project.",
            json!({}),
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_scope_changed"
        );
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        let generation = put(
            &pool,
            &auth,
            linked,
            "Imported/Distant/outcome.md",
            2,
            "An unrelated replacement",
            json!({}),
        )
        .await;
        // Removed mentions remain changes to the researched scope.
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "stale"
        );
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let delta = crate::dreamer_subject::research_changes(
            &mut tx,
            &auth,
            &scope,
            &[(canonical, 1), (linked, 1)],
        )
        .await
        .unwrap();
        assert_eq!(delta.status, "complete");
        assert_eq!(delta.relevant_ids, vec![linked]);
        assert_eq!(delta.through_generation, generation);
        tx.commit().await.unwrap();
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "fresh"
        );

        let renamed = Uuid::now_v7();
        let generation = put(
            &pool,
            &auth,
            renamed,
            "Elsewhere/Aster Vale.md",
            1,
            "Outcome with no subject name",
            json!({}),
        )
        .await;
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        sqlx::query("UPDATE brunn.entries SET path='dreams/research/moved.md' WHERE id=$1")
            .bind(renamed)
            .execute(&pool)
            .await
            .unwrap();
        let generation:i64 = sqlx::query_scalar("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) VALUES($1,$2,1,'update','dreams/research/moved.md',$3) RETURNING generation")
            .bind(auth.user_id.0).bind(renamed).bind(hash_token("Outcome with no subject name")).fetch_one(&pool).await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "stale"
        );

        let scope = subject_scope(&state, &auth, canonical, generation).await;
        let deleted = Uuid::now_v7();
        let generation = put(
            &pool,
            &auth,
            deleted,
            "Imported/removed.md",
            1,
            "Aster's new evidence",
            json!({}),
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "stale"
        );
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(deleted)
            .execute(&pool)
            .await
            .unwrap();
        let generation:i64 = sqlx::query_scalar("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) VALUES($1,$2,1,'delete','Imported/removed.md',$3) RETURNING generation")
            .bind(auth.user_id.0).bind(deleted).bind(hash_token("Aster's new evidence")).fetch_one(&pool).await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.status,
            "stale"
        );
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let delta = crate::dreamer_subject::research_changes(
            &mut tx,
            &auth,
            &scope,
            &[(canonical, 1), (deleted, 1)],
        )
        .await
        .unwrap();
        assert_eq!(delta.status, "complete");
        assert_eq!(delta.relevant_ids, vec![deleted]);
        tx.commit().await.unwrap();

        let scope = subject_scope(&state, &auth, canonical, generation).await;
        sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT $1,$2,1,'update','Elsewhere/unrelated.md',$3 FROM generate_series(1,2001)")
            .bind(auth.user_id.0).bind(unrelated).bind(hash_token("An unrelated Orchid outcome")).execute(&pool).await.unwrap();
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_change_check_limit"
        );
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let first =
            crate::dreamer_subject::research_changes(&mut tx, &auth, &scope, &[(canonical, 1)])
                .await
                .unwrap();
        assert_eq!(first.status, "unchecked");
        assert!(first.scanned_generation > scope.checked_generation);
        assert!(first.scanned_generation < first.through_generation);
        tx.commit().await.unwrap();
        // A later commit is excluded from the pinned second page, and remains
        // detectable after that interval has been completely reconciled.
        put(
            &pool,
            &auth,
            Uuid::now_v7(),
            "Elsewhere/after-page.md",
            1,
            "Pilot Nimbus changed again",
            json!({}),
        )
        .await;
        let mut cursor_scope = scope.clone();
        cursor_scope.checked_generation = first.scanned_generation;
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let second = crate::dreamer_subject::research_change_page(
            &mut tx,
            &auth,
            &cursor_scope,
            &[(canonical, 1)],
            first.through_generation,
        )
        .await
        .unwrap();
        assert_eq!(second.status, "complete");
        assert_eq!(second.scanned_generation, first.through_generation);
        assert!(second.relevant_ids.is_empty());
        tx.commit().await.unwrap();
        cursor_scope.checked_generation = second.scanned_generation;
        assert_eq!(
            subject_check(&state, &auth, &cursor_scope, canonical)
                .await
                .status,
            "stale"
        );
        let generation: i64 = sqlx::query_scalar(
            "SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1",
        )
        .bind(auth.user_id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        put(
            &pool,
            &auth,
            Uuid::now_v7(),
            "Elsewhere/large.md",
            1,
            &"x".repeat(9 * 1024 * 1024),
            json!({}),
        )
        .await;
        assert_eq!(
            subject_check(&state, &auth, &scope, canonical).await.reason,
            "subject_change_coverage_incomplete"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_overview_wins_and_stale_overview_does_not_fall_through() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        let generation = put(
            &pool,
            &auth,
            canonical,
            "People/Aster.md",
            1,
            "# Aster\nOriginal canonical source",
            json!({}),
        )
        .await;
        sqlx::query("UPDATE brunn.entries SET title='Aster' WHERE id=$1")
            .bind(canonical)
            .execute(&pool)
            .await
            .unwrap();
        let scope = subject_scope(&state, &auth, canonical, generation).await;
        let mut summary_metadata = metadata(canonical, generation);
        summary_metadata["dreamer_summary"]["subject_ref"] = json!(format!("entry:{canonical}"));
        summary_metadata["dreamer_summary"]["subject_scope"] = json!(scope);
        let overview = Uuid::now_v7();
        put(
            &pool,
            &auth,
            overview,
            "derived/entities/aster.md",
            1,
            "Canonical broad overview",
            summary_metadata,
        )
        .await;
        put(
            &pool,
            &auth,
            Uuid::now_v7(),
            "derived/entities/aster-skiing.md",
            1,
            "Newer narrow skiing summary",
            metadata(canonical, generation),
        )
        .await;
        let selected = read_item(&state, &auth, canonical, "current_state", None).await;
        assert_eq!(selected["text"], "Canonical broad overview");
        assert_eq!(selected["freshness"]["status"], "fresh");
        put(
            &pool,
            &auth,
            Uuid::now_v7(),
            "Elsewhere/novel.md",
            1,
            "[[People/Aster.md]] has a newer outcome",
            json!({}),
        )
        .await;
        let selected = read_item(&state, &auth, canonical, "current_state", None).await;
        assert_eq!(selected["representation"], "current_source_fallback");
        assert_eq!(selected["freshness"]["reason"], "subject_scope_changed");
        assert!(!selected.to_string().contains("skiing summary"));
        assert_eq!(
            read_item(&state, &auth, canonical, "full", Some(1)).await["text"],
            "# Aster\nOriginal canonical source"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_uncited_dependencies_invalidate_and_protect_history() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let canonical = Uuid::now_v7();
        put(
            &pool,
            &auth,
            canonical,
            "People/Aster.md",
            1,
            "# Aster\nSupported canonical fact",
            json!({}),
        )
        .await;
        sqlx::query("UPDATE brunn.entries SET title='Aster' WHERE id=$1")
            .bind(canonical)
            .execute(&pool)
            .await
            .unwrap();
        let uncited = Uuid::now_v7();
        let generation = put(
            &pool,
            &auth,
            uncited,
            "Imported/Receipts/thread-918.md",
            1,
            "A source considered during research",
            json!({}),
        )
        .await;
        let mut scope = subject_scope(&state, &auth, canonical, generation).await;
        let mut tx = snapshot(&state, &auth).await.unwrap();
        crate::dreamer_subject::bind_dependencies(
            &mut tx,
            &auth,
            &mut scope,
            &[(canonical, 1), (uncited, 1)],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let original_scope = serde_json::to_value(&scope).unwrap();
        assert_eq!(scope.dependencies.len(), 2);
        let mut manifest = metadata(canonical, generation);
        manifest["dreamer_summary"]["subject_ref"] = json!(format!("entry:{canonical}"));
        manifest["dreamer_summary"]["subject_scope"] = json!(scope);
        assert_eq!(
            manifest["dreamer_summary"]["sources"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let summary = Uuid::now_v7();
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/aster-full.md",
            1,
            "CACHED_SCOPE_CONTEXT_MARKER",
            manifest,
        )
        .await;
        assert_eq!(
            read_item(&state, &auth, canonical, "current_state", None).await["text"],
            "CACHED_SCOPE_CONTEXT_MARKER"
        );
        put(
            &pool,
            &auth,
            Uuid::now_v7(),
            "Different/new-outcome.md",
            1,
            "[[thread-918#Outcome|that earlier work]] now has a completed outcome.",
            json!({}),
        )
        .await;
        assert_eq!(
            read_item(&state, &auth, canonical, "current_state", None).await["freshness"]["reason"],
            "subject_scope_changed"
        );
        // Changed-but-accessible evidence still permits an explicit historical
        // read. Losing another dependency must override that stale result.
        put(
            &pool,
            &auth,
            canonical,
            "People/Aster.md",
            2,
            "# Aster\nNew canonical fact",
            json!({}),
        )
        .await;
        assert_eq!(
            read_item(&state, &auth, summary, "full", Some(1)).await["representation"],
            "historical_summary"
        );
        let audit = Uuid::now_v7();
        put(&pool,&auth,audit,"dreams/runs/subject-history.md",1,"CACHED_SCOPE_CONTEXT_MARKER",json!({"dreamer_run":{"schema":"dream.run.v1","items":[{"candidate":{"sources":[{"entry_ref":format!("entry:{canonical}"),"version":1}],"subject_scope":scope}}]}})).await;
        assert_eq!(
            read_item(&state, &auth, audit, "full", None).await["representation"],
            "audit_snapshot"
        );
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(uncited)
            .execute(&pool)
            .await
            .unwrap();
        let history = read_item(&state, &auth, summary, "full", Some(1)).await;
        assert_eq!(history["representation"], "summary_withheld");
        assert!(!history.to_string().contains("CACHED_SCOPE_CONTEXT_MARKER"));
        let audit = read_item(&state, &auth, audit, "full", None).await;
        assert_eq!(audit["representation"], "audit_withheld");
        assert!(!audit.to_string().contains("CACHED_SCOPE_CONTEXT_MARKER"));
        assert_eq!(serde_json::to_value(&scope).unwrap(), original_scope);
        pool.close().await;
    }

    #[tokio::test]
    async fn database_subject_research_hides_cached_notes_after_dependency_loss() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        put(
            &pool,
            &auth,
            source,
            "People/Aster.md",
            1,
            "Private source",
            json!({}),
        )
        .await;
        let research = Uuid::now_v7();
        put(&pool,&auth,research,"dreams/research/aster.md",1,"CACHED_RESEARCH_SECRET",json!({"dreamer_research":{"schema":"dream.research.v1","sources":[{"entry_ref":format!("entry:{source}"),"version":1}],"reviewed_sources":[]}})).await;
        assert_eq!(
            read_item(&state, &auth, research, "full", None).await["representation"],
            "audit_snapshot"
        );
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        for version in [None, Some(1)] {
            let result = read_item(&state, &auth, research, "full", version).await;
            assert_eq!(result["representation"], "audit_withheld");
            assert!(!result.to_string().contains("CACHED_RESEARCH_SECRET"));
        }
        let mut hits = vec![
            json!({"reference":format!("entry:{research}"),"path":"dreams/research/aster.md","title":"CACHED_RESEARCH_SECRET","excerpt":"CACHED_RESEARCH_SECRET","heading":"CACHED_RESEARCH_SECRET"}),
        ];
        protect_evidence(&state, &auth, &mut hits, &mut 3)
            .await
            .unwrap();
        assert!(
            !serde_json::to_string(&hits)
                .unwrap()
                .contains("CACHED_RESEARCH_SECRET")
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn database_summary_reads_fall_back_and_keep_historical_reads_exact() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        let summary = Uuid::now_v7();
        let frozen = put(
            &pool,
            &auth,
            source,
            "sources/topic/source.md",
            1,
            "Source before change",
            json!({}),
        )
        .await;
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/topic.md",
            1,
            "Cached summary before change",
            metadata(source, frozen),
        )
        .await;
        let fresh = read_item(&state, &auth, source, "current_state", None).await;
        assert_eq!(fresh["representation"], "derived_summary");
        assert_eq!(fresh["freshness"]["status"], "fresh");
        assert_eq!(fresh["text"], "Cached summary before change");

        // A concurrent commit after the snapshot cannot change its dependency heads.
        let mut tx = snapshot(&state, &auth).await.unwrap();
        let selected = document(&mut tx, &auth, summary, None)
            .await
            .unwrap()
            .unwrap();
        put(
            &pool,
            &auth,
            source,
            "sources/topic/source.md",
            2,
            "Current source after change",
            json!({}),
        )
        .await;
        assert_eq!(
            check(&mut tx, &auth, &selected).await.unwrap().status,
            "fresh"
        );
        tx.commit().await.unwrap();
        let stale = read_item(&state, &auth, summary, "full", None).await;
        assert_eq!(stale["representation"], "current_source_fallback");
        assert_eq!(stale["freshness"]["reason"], "source_version_changed");
        assert_eq!(stale["text"], "Current source after change");
        assert!(!stale.to_string().contains("Cached summary before change"));
        let historical = read_item(&state, &auth, source, "current_state", Some(1)).await;
        assert_eq!(historical["version"], 1);
        assert_eq!(historical["text"], "Source before change");
        let historical_summary = read_item(&state, &auth, summary, "full", Some(1)).await;
        assert_eq!(historical_summary["text"], "Cached summary before change");
        assert_eq!(historical_summary["representation"], "historical_summary");
        assert_eq!(historical_summary["freshness"]["status"], "stale");
        let result = crate::simple_core::read(axum::extract::State(state.clone()), axum::Extension(auth.clone()),
            axum::Json(serde_json::from_value(json!({"requests":[{"ref":format!("entry:{source}"),"version":1,"view":"current_truth"}]})).unwrap())).await;
        assert!(result.is_err());

        // Source deletion blocks both cached body and metadata, including pinned history.
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        for version in [None, Some(1)] {
            let hidden = read_item(&state, &auth, summary, "full", version).await;
            assert_eq!(hidden["representation"], "summary_withheld");
            assert_eq!(hidden["text"], "");
            assert!(hidden.get("metadata").is_none());
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn database_scope_permission_and_retrieval_limits_do_not_leak_cached_evidence() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        let summary = Uuid::now_v7();
        let frozen = put(
            &pool,
            &auth,
            source,
            "sources/topic/source.md",
            1,
            "Authorized current source",
            json!({}),
        )
        .await;
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/topic.md",
            1,
            "Private cached summary",
            metadata(source, frozen),
        )
        .await;
        put(
            &pool,
            &auth,
            Uuid::now_v7(),
            "sources/topic/new.md",
            1,
            "New relevant source",
            json!({}),
        )
        .await;
        let stale = read_item(&state, &auth, summary, "full", None).await;
        assert_eq!(stale["freshness"]["reason"], "covered_scope_changed");
        let mut evidence = vec![
            json!({"reference":format!("entry:{summary}"),"path":"derived/entities/topic.md","title":"Private cached title","excerpt":"Private cached summary","heading":"Private cached heading","additional_sections":[{"excerpt":"Private cached section"}],"verbatim_matches":[{"text":"Private cached match"}]});
            4
        ];
        protect_evidence(&state, &auth, &mut evidence, &mut 3)
            .await
            .unwrap();
        assert_eq!(evidence[0]["text"], "Authorized current sou");
        assert!(
            !serde_json::to_string(&evidence)
                .unwrap()
                .contains("Private cached")
        );
        assert_eq!(evidence[3]["freshness"]["reason"], "summary_check_limit");

        // A reference to another user's row remains invisible under the real app_ro RLS.
        let other_user = Uuid::now_v7();
        let foreign_source = Uuid::now_v7();
        sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,'Other summary test owner')")
            .bind(other_user).bind(format!("summary-test:{other_user}")).execute(&pool).await.unwrap();
        let mut foreign_auth = auth.clone();
        foreign_auth.user_id = UserId(other_user);
        foreign_auth.credential_id = CredentialId(Uuid::now_v7());
        sqlx::query("INSERT INTO brunn.api_credentials (id,user_id,label,token_hash,capabilities) VALUES ($1,$2,'Other summary test',$3,ARRAY['read'])")
            .bind(foreign_auth.credential_id.0).bind(other_user).bind(hash_token(&foreign_auth.credential_id.0.to_string())).execute(&pool).await.unwrap();
        let foreign_frozen = put(
            &pool,
            &foreign_auth,
            foreign_source,
            "sources/topic/foreign.md",
            1,
            "Other owner's private source",
            json!({}),
        )
        .await;
        let foreign_summary = Uuid::now_v7();
        put(
            &pool,
            &auth,
            foreign_summary,
            "derived/entities/foreign.md",
            1,
            "Cached foreign secret",
            metadata(foreign_source, foreign_frozen),
        )
        .await;
        let hidden = read_item(&state, &auth, foreign_summary, "full", None).await;
        assert_eq!(hidden["representation"], "summary_withheld");
        assert!(!hidden.to_string().contains("foreign"));
        assert!(!hidden.to_string().contains(&foreign_source.to_string()));

        let location_summary = Uuid::now_v7();
        let mut location_metadata = metadata(source, frozen);
        location_metadata["dreamer_summary"]["evidence_scope"] = json!({"from":"2026-09-05T00:00:00-07:00","to":"2026-09-06T00:00:00-07:00","timezone":"America/Los_Angeles","fingerprint":"sha256:fixture","sources_validated":true});
        put(
            &pool,
            &auth,
            location_summary,
            "derived/location/day.md",
            1,
            "Unchecked location summary",
            location_metadata,
        )
        .await;
        let location = read_item(&state, &auth, location_summary, "full", None).await;
        assert_eq!(location["representation"], "current_source_fallback");
        assert_eq!(
            location["freshness"]["reason"],
            "location_evidence_requires_save"
        );
        assert!(!location.to_string().contains("Unchecked location summary"));
        pool.close().await;
    }

    #[tokio::test]
    async fn database_reserved_artifacts_reject_metadata_forgery_and_deletion() {
        let Some((pool, state, mut auth)) = fixture().await else {
            return;
        };
        auth.capabilities
            .extend(["save".to_owned(), "delete".to_owned()]);
        auth.read_only = false;
        sqlx::query("UPDATE brunn.api_credentials SET capabilities=ARRAY['read','query','open','save','delete'] WHERE id=$1")
            .bind(auth.credential_id.0).execute(&pool).await.unwrap();
        let run = Uuid::now_v7();
        let metadata = json!({"dreamer_run":{"schema":"dream.run.v1","accepted":true,"items":[]}});
        put(
            &pool,
            &auth,
            run,
            "dreams/runs/2026-09-07.md",
            1,
            "Immutable audit",
            metadata.clone(),
        )
        .await;
        for path in [
            "dreams/runs/2026-09-07.md",
            "DREAMS/RUNS/2026-09-07.md",
            "dreams/state.md",
            "dreams/latest-receipt.md",
            "dreams/reviews/fake.md",
            "derived/entities/fake.md",
            "derived/location/fake.md",
        ] {
            let result = crate::simple_core::write(axum::extract::State(state.clone()), axum::Extension(auth.clone()),
                axum::Json(serde_json::from_value(json!({"path":path,"content":"Immutable audit","metadata":{"portable":{}},"expected_version":1})).unwrap())).await;
            assert!(result.is_err(), "ordinary write must reject {path}");
        }
        for key in [
            "dreamer_run",
            "dreamer_state",
            "dreamer_review",
            "dreamer_receipt",
            "dreamer_summary",
        ] {
            let result = crate::simple_core::write(
                axum::extract::State(state.clone()),
                axum::Extension(auth.clone()),
                axum::Json(
                    serde_json::from_value(
                        json!({"path":"sources/forged.md","content":"Forged","metadata":{key:{}}}),
                    )
                    .unwrap(),
                ),
            )
            .await;
            assert!(result.is_err(), "ordinary write must reject {key}");
        }
        let deletion = crate::simple_core::delete_entry(
            axum::extract::State(state.clone()),
            axum::Extension(auth.clone()),
            axum::extract::Path(format!("entry:{run}")),
            axum::extract::Query(serde_json::from_value(json!({"expected_version":1})).unwrap()),
        )
        .await;
        assert!(deletion.is_err());
        let preserved: Value = sqlx::query_scalar("SELECT metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=1")
            .bind(auth.user_id.0).bind(run).fetch_one(&pool).await.unwrap();
        assert_eq!(preserved, metadata);
        let deleted: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM brunn.entries WHERE id=$1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!deleted);
        pool.close().await;
    }

    #[tokio::test]
    async fn database_audit_reads_hide_deleted_dependency_text_and_metadata() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        let run = Uuid::now_v7();
        let review = Uuid::now_v7();
        let receipt = Uuid::now_v7();
        put(
            &pool,
            &auth,
            source,
            "sources/topic/audit-source.md",
            1,
            "Sensitive source marker",
            json!({}),
        )
        .await;
        let item = json!({"candidate":{"sources":[{"entry_ref":format!("entry:{source}"),"version":1,"excerpt":"Sensitive source marker"}],"content":"Sensitive source marker"}});
        put(&pool, &auth, run, "dreams/runs/2026-09-07.md", 1, "Audit Sensitive source marker", json!({"dreamer_run":{"schema":"dream.run.v1","items":[item.clone()],"pending_refs":[],"history":[]}})).await;
        put(
            &pool,
            &auth,
            review,
            "dreams/reviews/audit.md",
            1,
            "Decision Sensitive source marker",
            json!({"dreamer_review":{"schema":"dream.review.v1","item":item}}),
        )
        .await;
        put(
            &pool,
            &auth,
            receipt,
            "dreams/latest-receipt.md",
            1,
            "Receipt Sensitive source marker",
            json!({"dreamer_receipt":{"receipt_ref":format!("entry:{run}"),"receipt_version":1}}),
        )
        .await;
        for id in [run, review, receipt] {
            let visible = read_item(&state, &auth, id, "full", Some(1)).await;
            assert_eq!(visible["representation"], "audit_snapshot");
            assert!(
                visible["text"]
                    .as_str()
                    .unwrap()
                    .contains("Sensitive source marker")
            );
        }
        let target = Uuid::now_v7();
        let target_review = Uuid::now_v7();
        put(
            &pool,
            &auth,
            target,
            "sources/topic/target.md",
            1,
            "Private before-target marker",
            json!({}),
        )
        .await;
        let item = json!({"before_md":"Private before-target marker","candidate":{"path":"sources/topic/target.md","expected_version":1,"sources":[{"entry_ref":format!("entry:{source}"),"version":1}]}});
        put(
            &pool,
            &auth,
            target_review,
            "dreams/reviews/target.md",
            1,
            "Private before-target marker",
            json!({"dreamer_review":{"schema":"dream.review.v1","item":item}}),
        )
        .await;
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(target)
            .execute(&pool)
            .await
            .unwrap();
        let target_hidden = read_item(&state, &auth, target_review, "full", Some(1)).await;
        assert_eq!(target_hidden["representation"], "audit_withheld");
        assert!(
            !target_hidden
                .to_string()
                .contains("Private before-target marker")
        );
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        for id in [run, review, receipt] {
            for version in [None, Some(1)] {
                let hidden = read_item(&state, &auth, id, "full", version).await;
                assert_eq!(hidden["representation"], "audit_withheld");
                assert!(!hidden.to_string().contains("Sensitive source marker"));
                assert!(hidden.get("metadata").is_none());
            }
        }
        let mut snippets = vec![
            json!({"reference":format!("entry:{run}"),"path":"dreams/runs/2026-09-07.md","title":"Sensitive source marker","excerpt":"Sensitive source marker","metadata":{"hidden":"Sensitive source marker"}}),
        ];
        protect_evidence(&state, &auth, &mut snippets, &mut 3)
            .await
            .unwrap();
        assert_eq!(snippets[0]["representation"], "audit_withheld");
        assert!(!snippets[0].to_string().contains("Sensitive source marker"));
        pool.close().await;
    }

    #[tokio::test]
    async fn database_receipt_retains_operational_status_when_cached_facts_are_withheld() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        let run = Uuid::now_v7();
        let receipt = Uuid::now_v7();
        put(
            &pool,
            &auth,
            source,
            "sources/topic/receipt-source.md",
            1,
            "Private receipt marker",
            json!({}),
        )
        .await;
        put(&pool, &auth, run, "dreams/runs/2026-09-07.md", 1, "Private receipt marker", json!({"dreamer_run":{"schema":"dream.run.v1","items":[{"candidate":{"sources":[{"entry_ref":format!("entry:{source}"),"version":1}]}}]}})).await;
        let payload = json!({"schema":"dream.latest-receipt.v2","run_id":"2026-09-07","receipt_ref":format!("entry:{run}"),"receipt_version":1,"receipt_path":"dreams/runs/2026-09-07.md","status":"completed","completed_at":"2026-09-07T10:10:00Z","mode":"report-only","runner":"brunn-rust-dreamer","mode_flip":false,"probe_monitoring":null,"applied_writes":[{"path":"derived/entities/topic.md","version":1,"summary":"Private receipt marker"}],"entering_veto_window_today":[],"pending_owner":[{"recommendation_id":"dream-2026-09-07-1","summary":"Private receipt marker","reason":"Private receipt marker","published_at":"2026-09-07T10:10:00Z","age_days":0}],"pending_review_surfaces":[],"next_run_at":"2026-09-08T10:00:00Z"});
        let content = crate::dreamer::receipt::render_latest(&payload).unwrap();
        put(
            &pool,
            &auth,
            receipt,
            "dreams/latest-receipt.md",
            1,
            &content,
            json!({"dreamer_receipt":payload}),
        )
        .await;
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        let result = read_item(&state, &auth, receipt, "full", Some(1)).await;
        let parsed =
            crate::dreamer::receipt::parse_latest(result["text"].as_str().unwrap()).unwrap();
        for key in [
            "status",
            "completed_at",
            "receipt_ref",
            "receipt_version",
            "receipt_path",
            "next_run_at",
        ] {
            assert_eq!(parsed[key], payload[key]);
        }
        assert_eq!(parsed.as_object().unwrap().len(), 16);
        assert_eq!(parsed["pending_owner"].as_array().unwrap().len(), 1);
        assert_eq!(
            parsed["applied_writes"][0]["path"],
            "derived/entities/topic.md"
        );
        assert!(!result.to_string().contains("Private receipt marker"));
        assert_eq!(result["representation"], "operational_receipt");
        assert_eq!(result["freshness"]["status"], "source_unavailable");
        pool.close().await;
    }

    #[tokio::test]
    async fn database_raw_audits_require_save_and_retained_natural_keys() {
        let Some((pool, state, mut auth)) = fixture().await else {
            return;
        };
        let run = Uuid::now_v7();
        let summary = Uuid::now_v7();
        let at = Utc::now() - chrono::Duration::hours(2);
        sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m) VALUES($1,$2,'ping',0,47,-122,5)").bind(auth.user_id.0).bind(at).execute(&pool).await.unwrap();
        // PostgreSQL retains microseconds; use the database's canonical natural key.
        let key: Value = sqlx::query_scalar("SELECT jsonb_build_object('at',at,'type',type) FROM brunn.location_reports WHERE user_id=$1 AND at=$2 AND type='ping'").bind(auth.user_id.0).bind(at).fetch_one(&pool).await.unwrap();
        let raw = json!([{"natural_key":key,"fields":["lat","lon"]}]);
        put(&pool, &auth, run, "dreams/runs/2026-09-07.md", 1, "Private raw marker", json!({"dreamer_run":{"schema":"dream.run.v1","items":[{"candidate":{"sources":[],"raw_sources":raw}}]}})).await;
        let raw_metadata = json!({"dreamer_summary":{"schema":"dream.summary.v1","state":"published","frozen_generation":0,"sources":[],"scope_prefixes":[],"raw_sources":raw,"evidence_scope":{"sources_validated":true}}});
        assert!(manifest(&raw_metadata).is_some());
        put(
            &pool,
            &auth,
            summary,
            "derived/location/raw.md",
            1,
            "Private raw marker",
            raw_metadata,
        )
        .await;
        for id in [run, summary] {
            let hidden = read_item(&state, &auth, id, "full", Some(1)).await;
            assert_eq!(hidden["text"], "");
            assert!(!hidden.to_string().contains("Private raw marker"));
        }
        auth.capabilities.insert("save".to_owned());
        auth.read_only = false;
        sqlx::query("UPDATE brunn.api_credentials SET capabilities=ARRAY['read','query','open','save'] WHERE id=$1").bind(auth.credential_id.0).execute(&pool).await.unwrap();
        let visible = read_item(&state, &auth, run, "full", Some(1)).await;
        assert_eq!(visible["representation"], "audit_snapshot");
        sqlx::query(
            "DELETE FROM brunn.location_reports WHERE user_id=$1 AND at=$2 AND type='ping'",
        )
        .bind(auth.user_id.0)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
        for id in [run, summary] {
            let hidden = read_item(&state, &auth, id, "full", Some(1)).await;
            assert_eq!(hidden["text"], "");
            assert!(!hidden.to_string().contains("Private raw marker"));
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn database_read_preference_flag_does_not_disable_cached_source_protection() {
        let Some((pool, mut state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        let summary = Uuid::now_v7();
        let frozen = put(
            &pool,
            &auth,
            source,
            "sources/topic/flag.md",
            1,
            "Exact canonical text",
            json!({}),
        )
        .await;
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/flag.md",
            1,
            "Derived flag marker",
            metadata(source, frozen),
        )
        .await;
        let mut status_auth = auth.clone();
        status_auth.capabilities.insert("status".to_owned());
        for enabled in [true, false] {
            state.config.dreamer_summary_reads_enabled = enabled;
            let axum::Json(status) = crate::service::status(
                axum::extract::State(state.clone()),
                axum::Extension(status_auth.clone()),
            )
            .await
            .unwrap();
            assert_eq!(
                status["feature_flags"]["dreamer_summary_reads_enabled"],
                enabled
            );
            assert_eq!(
                status["runtime_features"]["dreamer_summary_reads_enabled"],
                enabled
            );
        }
        state.config.dreamer_summary_reads_enabled = false;
        let source_read = read_item(&state, &auth, source, "current_state", None).await;
        assert_eq!(source_read["text"], "Exact canonical text");
        sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(source)
            .execute(&pool)
            .await
            .unwrap();
        let protected = read_item(&state, &auth, summary, "full", None).await;
        assert_eq!(protected["representation"], "summary_withheld");
        assert!(!protected.to_string().contains("Derived flag marker"));
        pool.close().await;
    }

    /// Explicit local measurement, not a timing assertion or quality benchmark.
    #[tokio::test]
    #[ignore = "manual isolated database latency measurement"]
    async fn measure_local_summary_read_overhead() {
        let Some((pool, state, auth)) = fixture().await else {
            return;
        };
        let source = Uuid::now_v7();
        let summary = Uuid::now_v7();
        let source_text = (0..600).map(|index| format!("Evidence row {index}: the test project retains exact source versions and requires current evidence before publication.\n")).collect::<String>();
        let summary_text = "# Test project summary\n\nCurrent evidence must be checked against the cited immutable source version before publication.\n".repeat(12);
        let frozen = put(
            &pool,
            &auth,
            source,
            "sources/topic/measurement.md",
            1,
            &source_text,
            json!({}),
        )
        .await;
        put(
            &pool,
            &auth,
            summary,
            "derived/entities/measurement.md",
            1,
            &summary_text,
            metadata(source, frozen),
        )
        .await;
        for _ in 0..3 {
            read_item(&state, &auth, source, "full", None).await;
            read_item(&state, &auth, source, "current_state", None).await;
        }
        let mut full_ms = Vec::new();
        let mut summary_ms = Vec::new();
        for index in 0..30 {
            let views = if index % 2 == 0 {
                ["full", "current_state"]
            } else {
                ["current_state", "full"]
            };
            for view in views {
                let start = std::time::Instant::now();
                let value = read_item(&state, &auth, source, view, None).await;
                let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
                if view == "full" {
                    assert_eq!(value["text"], source_text);
                    full_ms.push(elapsed);
                } else {
                    assert_eq!(value["text"], summary_text);
                    assert_eq!(value["freshness"]["status"], "fresh");
                    summary_ms.push(elapsed);
                }
            }
        }
        full_ms.sort_by(f64::total_cmp);
        summary_ms.sort_by(f64::total_cmp);
        eprintln!(
            "LOCAL_SUMMARY_MEASUREMENT {}",
            json!({"samples_per_arm":30,"warmups_per_arm":3,
            "source_chars":source_text.chars().count(),"summary_chars":summary_text.chars().count(),
            "source_full_ms":{"p50":full_ms[14],"p95":full_ms[28]},
            "summary_current_state_ms":{"p50":summary_ms[14],"p95":summary_ms[28]},
            "scope":"Isolated local PostgreSQL app_ro RLS fixture and direct API handler; excludes HTTP, hosted latency and reasoning quality."})
        );
        pool.close().await;
    }
}

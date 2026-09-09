//! Publication checks for historical location summaries. Raw records remain in
//! the existing retention store; durable citations contain only keys and fields.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{
    evidence::{EvidenceQuery, evidence_in_tx},
    store::lock_location_user,
};
use crate::{
    auth::AuthContext,
    dreamer_review::Source,
    error::{ApiError, ApiResult},
    models::Capability,
};

const MAX_CITATIONS: usize = 64;
const MAX_FIELDS: usize = 24;
const REPORT_FIELDS: &[&str] = &[
    "at",
    "type",
    "offset_min",
    "lat",
    "lon",
    "accuracy_m",
    "arrived_at",
    "departed_at",
    "city",
    "region",
    "country",
    "name",
    "first_received_at",
    "origin",
    "poi",
];
const POI_FIELDS: &[&str] = &["rank", "name", "category", "distance_m"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCitation {
    /// Exact {at,type} object copied from a packet report's natural_key.
    pub natural_key: Value,
    /// Top-level report fields or zero-based POI paths, such as poi.0.name.
    pub fields: Vec<String>,
}

fn changed(message: &str) -> ApiError {
    ApiError::conflict("location_evidence_changed", message, json!({}))
}

/// Called within the transaction that will publish the summary. All owner
/// writes acquire these fences; read committed ensures an earlier snapshot
/// cannot hide reports committed while this transaction waited for its locks.
/// The caller holds the fences through the actual publication commit.
pub async fn validate_candidate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    query: &EvidenceQuery,
    fingerprint: &str,
    canonical_sources: &[Source],
    raw_sources: &[RawCitation],
) -> ApiResult<Value> {
    Ok(validate_candidate_with_clock_evidence_in_tx(
        tx,
        auth,
        query,
        fingerprint,
        canonical_sources,
        raw_sources,
        None,
    )
    .await?
    .0)
}

/// Return the trusted scope and bounded evidence for content validation while
/// retaining the same publication locks. Raw evidence stays inside the API.
pub(crate) async fn validate_candidate_with_clock_evidence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    query: &EvidenceQuery,
    fingerprint: &str,
    canonical_sources: &[Source],
    raw_sources: &[RawCitation],
    context_sources: Option<&Value>,
) -> ApiResult<(Value, Value)> {
    auth.require(Capability::Save)?;
    query.validate(chrono::Utc::now())?;
    if canonical_sources.len() + raw_sources.len() == 0
        || canonical_sources.len() + raw_sources.len() > MAX_CITATIONS
    {
        return Err(ApiError::invalid(
            "location summaries require one to 64 exact supporting citations",
        ));
    }
    if fingerprint.len() != 71
        || !fingerprint.starts_with("sha256:")
        || !fingerprint[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(ApiError::invalid(
            "a canonical location evidence fingerprint is required",
        ));
    }
    let isolation: String = sqlx::query_scalar("SELECT current_setting('transaction_isolation')")
        .fetch_one(&mut **tx)
        .await?;
    if isolation != "read committed" {
        return Err(ApiError::invalid(
            "location publication requires a read-committed transaction before owner evidence locks",
        ));
    }
    crate::db::lock_workspace_commit(tx, auth.user_id.0).await?;
    lock_location_user(tx, auth.user_id.0).await?;
    let context_scope = json!({"context_sources":context_sources});
    if !context_sources_current_in_tx(tx, auth, &context_scope).await? {
        return Err(changed(
            "location context changed or is inaccessible; retain the day for fresh autonomous discovery",
        ));
    }
    let mut packet = evidence_in_tx(tx, auth, query).await?;
    if packet["completeness"]["complete"] != true || packet["fingerprint_complete"] != true {
        return Err(ApiError::conflict(
            "location_evidence_incomplete",
            "location evidence is incomplete; retain the candidate for a new bounded compilation",
            json!({"reasons":packet["completeness"]["reasons"]}),
        ));
    }
    if packet["evidence_fingerprint"] != fingerprint {
        return Err(changed(
            "location evidence changed; recompile before publication",
        ));
    }
    if packet["reports"].as_array().is_some_and(|r| !r.is_empty()) && raw_sources.is_empty() {
        return Err(ApiError::invalid(
            "location summaries must cite retained raw observations when the packet contains reports",
        ));
    }
    let canonical_months = packet["canonical_months"].as_array();
    if canonical_months.is_some_and(|months| {
        months.iter().any(|month| {
            month["selectors"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        }) && !canonical_sources
            .iter()
            .any(|source| months.iter().any(|month| month["ref"] == source.entry_ref))
    }) {
        return Err(ApiError::invalid(
            "location summaries must reconcile the relevant canonical visit rows with raw observations",
        ));
    }
    for source in canonical_sources {
        if !is_context_source(source, &context_scope) {
            validate_canonical(tx, auth, source, &packet).await?;
        }
    }
    for citation in raw_sources {
        validate_raw(citation, &packet)?;
    }

    // The readable timeline may merge or omit misleading canonical intervals,
    // but its evidence inventory must retain every relevant row for review.
    // Check before narrowing the packet to exact historical clock selectors.
    if packet["canonical_months"].as_array().is_some_and(|months| {
        months.iter().any(|month| {
            month["selectors"].as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    let text = row["text"].as_str().unwrap_or("");
                    !canonical_sources.iter().any(|source| {
                        month["ref"] == source.entry_ref
                            && source.excerpt.lines().any(|line| line == text)
                    })
                })
            })
        })
    }) {
        return Err(ApiError::invalid(
            "location evidence inventory must retain every relevant canonical row",
        ));
    }

    // Exact historical source versions remain valid when their selected rows
    // are unchanged after unrelated month growth. Above we checked every row
    // against the fresh packet. Preserve those typed boundaries at the cited
    // version/line coordinates for the clock checker; never parse model prose
    // or trust caller-supplied timestamps as canonical evidence.
    let mut clock_documents = Vec::new();
    for source in canonical_sources {
        let Some(document) = packet["canonical_months"]
            .as_array()
            .and_then(|months| months.iter().find(|month| month["ref"] == source.entry_ref))
        else {
            continue; // Places has no canonical visit boundaries.
        };
        let mut selectors = Vec::new();
        for (offset, line) in source.excerpt.lines().enumerate() {
            let mut selector = document["selectors"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["text"] == line))
                .cloned()
                .ok_or_else(|| changed("validated canonical row is missing from clock evidence"))?;
            selector["start_line"] = json!(source.start_line + offset);
            selector["end_line"] = json!(source.start_line + offset);
            selectors.push(selector);
        }
        clock_documents
            .push(json!({"ref":source.entry_ref,"version":source.version,"selectors":selectors}));
    }
    packet["canonical_months"] = json!(clock_documents);
    let mut scope = json!({"from":query.from.to_rfc3339(),"to":query.to.to_rfc3339(),"timezone":query.timezone,"fingerprint":fingerprint,"sources_validated":true});
    if let Some(context) = context_sources {
        scope["context_sources"] = context.clone();
    }
    Ok((scope, packet))
}

fn source_matches_context(source: &Source, context: &Value) -> bool {
    context["entry_ref"] == source.entry_ref
        && context["version"] == source.version
        && context["start_line"] == source.start_line
        && context["end_line"] == source.end_line
        && context["path"] == source.path
        && context["excerpt"] == source.excerpt
}

pub(crate) fn is_context_source(source: &Source, scope: &Value) -> bool {
    scope["context_sources"].as_array().is_some_and(|sources| {
        sources
            .iter()
            .any(|context| source_matches_context(source, context))
    })
}

/// Autonomously discovered context is independent of the raw packet hash.
/// Current visibility and exact versions are mandatory, including on reads.
pub(crate) async fn context_sources_current_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope: &Value,
) -> ApiResult<bool> {
    let Some(context) = scope
        .get("context_sources")
        .filter(|value| !value.is_null())
    else {
        return Ok(true);
    };
    let Some(sources) = context.as_array().filter(|sources| sources.len() <= 12) else {
        return Ok(false);
    };
    let mut seen = BTreeSet::new();
    let mut bytes = 0;
    for source in sources {
        let Some(id) = source["entry_ref"]
            .as_str()
            .and_then(|reference| reference.strip_prefix("entry:"))
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            return Ok(false);
        };
        if !seen.insert(id) {
            return Ok(false);
        }
        let Some(version) = source["version"].as_i64().filter(|version| *version > 0) else {
            return Ok(false);
        };
        let row = sqlx::query("SELECT e.path,e.current_version,v.content,v.content_sha256,v.metadata,head.metadata AS current_metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3 LEFT JOIN brunn.entry_versions head ON head.user_id=e.user_id AND head.entry_id=e.id AND head.version=e.current_version WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL AND e.kind='markdown'")
            .bind(auth.user_id.0).bind(id).bind(version).fetch_optional(&mut **tx).await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let path: String = row.try_get("path")?;
        if row.get::<i64, _>("current_version")
            != source["current_version"].as_i64().unwrap_or(version)
            || source["path"] != path
            || [
                "dreams/",
                "derived/",
                ".brunn/",
                "agent-memory/",
                "Location/",
            ]
            .iter()
            .any(|prefix| path.starts_with(prefix))
            || path == "private/dreamer.md"
            || crate::dreamer_summary::protected_metadata(&row.get::<Value, _>("metadata"))
            || crate::dreamer_summary::generated_briefing_metadata(&row.get::<Value, _>("metadata"))
            || row
                .get::<Option<Value>, _>("current_metadata")
                .as_ref()
                .is_some_and(crate::dreamer_summary::generated_briefing_metadata)
            || source["content_hash"]
                != format!("sha256:{}", row.get::<String, _>("content_sha256"))
        {
            return Ok(false);
        }
        if let Some(expires) = source["expires_at"].as_str()
            && chrono::DateTime::parse_from_rfc3339(expires)
                .map_or(true, |at| at <= chrono::Utc::now())
        {
            return Ok(false);
        }
        let Some(content) = row.get::<Option<String>, _>("content") else {
            return Ok(false);
        };
        let lines: Vec<_> = content.lines().collect();
        let Some(start) = source["start_line"]
            .as_u64()
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line > 0)
        else {
            return Ok(false);
        };
        let Some(end) = source["end_line"]
            .as_u64()
            .and_then(|line| usize::try_from(line).ok())
            .filter(|line| *line >= start && *line <= lines.len() && *line - start <= 400)
        else {
            return Ok(false);
        };
        let excerpt = lines[start - 1..end].join("\n");
        bytes += excerpt.len();
        if excerpt.is_empty() || bytes > 12_000 || source["excerpt"] != excerpt {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn validate_canonical(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    source: &Source,
    packet: &Value,
) -> ApiResult<()> {
    let id = source
        .entry_ref
        .strip_prefix("entry:")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            ApiError::invalid("location citation requires an exact canonical entry reference")
        })?;
    if source.version < 1
        || source.start_line < 1
        || source.end_line < source.start_line
        || source.end_line - source.start_line > 400
    {
        return Err(ApiError::invalid(
            "location canonical citation line range is invalid",
        ));
    }
    let row=sqlx::query("SELECT e.path,v.content FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3 WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL AND e.kind='markdown'")
        .bind(auth.user_id.0).bind(id).bind(source.version).fetch_optional(&mut **tx).await?.ok_or_else(||changed("a location source is missing or inaccessible"))?;
    let path: String = row.try_get("path")?;
    let content: Option<String> = row.try_get("content")?;
    let content = content.ok_or_else(|| changed("location source content is unavailable"))?;
    let lines: Vec<_> = content.lines().collect();
    if source.end_line > lines.len() {
        return Err(ApiError::invalid(
            "location citation lies outside its exact source version",
        ));
    }
    let selected = &lines[source.start_line - 1..source.end_line];
    let excerpt = selected.join("\n");
    if excerpt.is_empty()
        || excerpt.len() > 12_000
        || source.path != path
        || source.excerpt != excerpt
    {
        return Err(ApiError::invalid(
            "location citation path and excerpt must match their exact server-validated source version",
        ));
    }
    if let Some(document) = packet["canonical_months"].as_array().and_then(|docs| {
        docs.iter()
            .find(|d| d["ref"] == source.entry_ref && d["path"] == path)
    }) {
        if source.version > document["version"].as_i64().unwrap_or(0)
            || !selected.iter().all(|line| {
                document["selectors"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| row["text"] == *line))
            })
        {
            return Err(changed(
                "cited canonical rows are not exact relevant rows of this historical interval",
            ));
        }
        return Ok(());
    }
    let places = &packet["places"];
    if places["ref"] == source.entry_ref
        && places["path"] == path
        && places["version"] == source.version
        && places["text"] == content
    {
        return Ok(());
    }
    Err(changed(
        "canonical citation is outside this historical location evidence packet",
    ))
}

pub(crate) fn validate_raw<'a>(citation: &RawCitation, packet: &'a Value) -> ApiResult<&'a Value> {
    let key = citation
        .natural_key
        .as_object()
        .filter(|o| {
            o.len() == 2
                && o.get("at").is_some_and(Value::is_string)
                && o.get("type").is_some_and(Value::is_string)
        })
        .ok_or_else(|| {
            ApiError::invalid("raw location natural_key must contain exactly at and type")
        })?;
    if serde_json::to_vec(key)?.len() > 200
        || citation.fields.is_empty()
        || citation.fields.len() > MAX_FIELDS
        || citation.fields.iter().any(|field| field.len() > 64)
        || citation.fields.iter().collect::<BTreeSet<_>>().len() != citation.fields.len()
    {
        return Err(ApiError::invalid(
            "raw location citations require bounded distinct field names",
        ));
    }
    let record = packet["reports"]
        .as_array()
        .into_iter()
        .flatten()
        .chain([
            &packet["boundary_observations"]["before"],
            &packet["boundary_observations"]["after"],
        ])
        .find(|record| record.is_object() && record["natural_key"] == citation.natural_key)
        .ok_or_else(|| changed("raw citation is not in this owner historical evidence packet"))?;
    for field in &citation.fields {
        if selected_field_value(record, field).is_none() {
            return Err(ApiError::invalid(
                "raw citation selects an unavailable or unsupported field",
            ));
        }
    }
    Ok(record)
}

/// Shared whitelist and lookup for publication and transient owner previews.
/// A present JSON null is evidence of an unrecorded value, never inferred data.
pub fn selected_field_value<'a>(record: &'a Value, field: &str) -> Option<&'a Value> {
    if REPORT_FIELDS.contains(&field) {
        return record.get(field);
    }
    let components = field.split('.').collect::<Vec<_>>();
    if components.len() != 3 || components[0] != "poi" || !POI_FIELDS.contains(&components[2]) {
        return None;
    }
    let index = components[1]
        .parse::<usize>()
        .ok()
        .filter(|index| index.to_string() == components[1])?;
    record["poi"].as_array()?.get(index)?.get(components[2])
}

/// Read-only presentation within the caller's existing coherent review
/// snapshot. Values are regenerated, never stored in run/state/audit metadata.
/// Changed, expired or over-budget evidence returns no preview values.
pub async fn citation_previews_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope: &Value,
    raw_sources: &[RawCitation],
) -> ApiResult<Vec<Value>> {
    auth.require(Capability::Save)?;
    if raw_sources.is_empty() {
        return Ok(vec![]);
    }
    if raw_sources.len() > MAX_CITATIONS {
        return Err(ApiError::invalid(
            "raw citation preview exceeds the citation limit",
        ));
    }
    let query: EvidenceQuery = serde_json::from_value(
        json!({"from":scope["from"],"to":scope["to"],"timezone":scope["timezone"]}),
    )?;
    let packet = evidence_in_tx(tx, auth, &query).await?;
    if scope["sources_validated"] != true
        || packet["completeness"]["complete"] != true
        || packet["fingerprint_complete"] != true
        || packet["evidence_fingerprint"] != scope["fingerprint"]
    {
        return Ok(vec![]);
    }
    let mut previews = Vec::new();
    let mut preview_bytes = 0;
    for citation in raw_sources {
        let record = validate_raw(citation, &packet)?;
        let mut excerpt = format!("Exact report key: {}\n", citation.natural_key);
        if ["before", "after"].iter().any(|position| {
            packet["boundary_observations"][*position]["natural_key"] == citation.natural_key
        }) {
            excerpt.push_str(
                "Boundary observation outside the requested interval; not day coverage.\n",
            );
        }
        for field in &citation.fields {
            let value =
                selected_field_value(record, field).expect("field validated against same packet");
            excerpt.push_str(&format!("{field}: {value}\n"));
        }
        excerpt.push_str("Null means unrecorded. POIs are nearby candidates, not confirmed venues. Sample or callback times do not establish continuous presence.");
        let key = hex::encode(Sha256::digest(serde_json::to_vec(&citation.natural_key)?));
        let preview = json!({"entry_ref":format!("location-report:{key}"),"label":format!("Raw location report {} ({})",record["at"].as_str().unwrap_or(""),record["type"].as_str().unwrap_or("")),"excerpt":excerpt});
        preview_bytes += serde_json::to_vec(&preview)?.len();
        if preview_bytes > super::evidence::MAX_PACKET_BYTES {
            return Ok(vec![]);
        }
        previews.push(preview);
    }
    Ok(previews)
}

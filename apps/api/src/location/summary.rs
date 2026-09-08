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
    let packet = evidence_in_tx(tx, auth, query).await?;
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
    for source in canonical_sources {
        validate_canonical(tx, auth, source, &packet).await?;
    }
    for citation in raw_sources {
        validate_raw(citation, &packet)?;
    }
    Ok(
        json!({"from":query.from.to_rfc3339(),"to":query.to.to_rfc3339(),"timezone":query.timezone,"fingerprint":fingerprint,"sources_validated":true}),
    )
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

fn validate_raw<'a>(citation: &RawCitation, packet: &'a Value) -> ApiResult<&'a Value> {
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

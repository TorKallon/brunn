//! Bounded historical source evidence. This never updates presence, visits,
//! Places, raw retention, or source ownership and does no enrichment/reasoning.
use std::collections::BTreeSet;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Postgres, Row, Transaction};

use super::rules;
use crate::{
    AppState,
    auth::AuthContext,
    db::set_context,
    error::{ApiError, ApiResult},
    models::Capability,
};

pub const MAX_REPORTS: usize = 2_000;
pub const MAX_PACKET_BYTES: usize = 250_000;
const MAX_CANONICAL_DOCUMENTS: usize = 64;
const MAX_CANONICAL_SOURCE_BYTES: i64 = 1_000_000;
const MAX_PLACES_BYTES: i64 = 64_000;
const RETENTION_DAYS: i64 = 30;
const GAP_MINUTES: i64 = 15;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceQuery {
    pub from: DateTime<FixedOffset>,
    pub to: DateTime<FixedOffset>,
    pub timezone: String,
}

impl EvidenceQuery {
    pub fn validate(&self, now: DateTime<Utc>) -> ApiResult<Tz> {
        let timezone = self
            .timezone
            .parse::<Tz>()
            .map_err(|_| ApiError::invalid("timezone must be an IANA timezone"))?;
        let span = self.to - self.from;
        if span <= Duration::zero() || span > Duration::hours(26) || self.to.to_utc() > now {
            return Err(ApiError::invalid(
                "location evidence requires a closed historical interval of more than zero and no more than 26 hours",
            ));
        }
        Ok(timezone)
    }
}

/// Use the write-role connection for the existing Save-only raw SELECT policy,
/// while PostgreSQL itself prevents writes and pins every source to one snapshot.
pub async fn read_evidence(
    state: &AppState,
    auth: &AuthContext,
    query: &EvidenceQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Save)?;
    query.validate(Utc::now())?;
    let mut tx = state.rw_pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    set_context(&mut tx, auth).await?;
    sqlx::query("SELECT set_config('statement_timeout',$1,true)")
        .bind(format!("{}ms", state.config.request_timeout.as_millis()))
        .execute(&mut *tx)
        .await?;
    let packet = evidence_in_tx(&mut tx, auth, query).await?;
    tx.commit().await?;
    Ok(packet)
}

/// Shared with server-authorized summary validation. The caller must establish
/// a consistent transaction snapshot. Publication must acquire the existing
/// owner location lock before this read and hold it through the summary commit,
/// using the shared workspace->location lock order. A packet is publishable only
/// when completeness.complete and fingerprint_complete are both true.
pub async fn evidence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    query: &EvidenceQuery,
) -> ApiResult<Value> {
    auth.require(Capability::Save)?;
    let timezone = query.validate(Utc::now())?;
    let from = query.from.to_utc();
    let to = query.to.to_utc();
    let snapshot = sqlx::query("SELECT transaction_timestamp() AS at,pg_current_snapshot()::text AS snapshot,COALESCE((SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1),0)::bigint AS generation")
        .bind(auth.user_id.0).fetch_one(&mut **tx).await?;
    let snapshot_at: DateTime<Utc> = snapshot.try_get("at")?;
    let mut reasons = BTreeSet::new();
    if from < snapshot_at - Duration::days(RETENTION_DAYS) {
        reasons.insert("raw_retention_expired");
    }

    // Callback time is not visit time. Late callbacks are admitted whenever
    // their estimated interval overlaps; an unknown departure stays unknown.
    // Both assembled fragments are constants; all request values are bound.
    let selected_sql = format!(
        r#"{REPORT_SELECT}
        WHERE report.user_id=$1 AND (
          (report.at >= $2 AND report.at < $3)
          OR (report.type IN ('visit_arrival','visit_departure')
              AND report.arrived_at < $3
              AND (report.departed_at IS NULL OR report.departed_at > $2))
        ) ORDER BY report.at,report.type LIMIT $4"#
    );
    let selected = sqlx::query(AssertSqlSafe(selected_sql.as_str()))
        .bind(auth.user_id.0)
        .bind(from)
        .bind(to)
        .bind((MAX_REPORTS + 1) as i64)
        .fetch_all(&mut **tx)
        .await?;
    if selected.len() > MAX_REPORTS {
        reasons.insert("report_limit");
    }
    let mut reports = selected
        .into_iter()
        .take(MAX_REPORTS)
        .map(|row| row.try_get::<Value, _>("report"))
        .collect::<Result<Vec<_>, _>>()?;
    for report in &mut reports {
        annotate_report(report);
    }
    let before = boundary(tx, auth, from, true).await?;
    let after = boundary(tx, auth, to, false).await?;
    let gaps = sample_gaps(&reports, from, to);

    // Month files are keyed by arrival month. Scan the bounded canonical
    // location namespace up to the interval's final month to preserve old
    // cross-month visits, rather than assuming arrival was in the requested day.
    let final_month = (to - Duration::nanoseconds(1))
        .with_timezone(&timezone)
        .format("%Y-%m")
        .to_string();
    let final_path = format!("Location/Visits/{final_month}.md");
    let documents = sqlx::query(
        r#"
        SELECT entry.id,entry.path,entry.current_version,version.content_sha256,
               CASE WHEN sum(version.size_bytes) OVER (ORDER BY entry.path DESC)
                    <= $3 THEN version.content END AS content
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version ON version.user_id=entry.user_id
          AND version.entry_id=entry.id AND version.version=entry.current_version
        WHERE entry.user_id=$1 AND entry.deleted_at IS NULL
          AND entry.kind='markdown'
          AND entry.path ~ '^Location/Visits/[0-9]{4}-[0-9]{2}\.md$'
          AND entry.path <= $2
        ORDER BY entry.path DESC LIMIT $4
    "#,
    )
    .bind(auth.user_id.0)
    .bind(final_path)
    .bind(MAX_CANONICAL_SOURCE_BYTES)
    .bind((MAX_CANONICAL_DOCUMENTS + 1) as i64)
    .fetch_all(&mut **tx)
    .await?;
    if documents.len() > MAX_CANONICAL_DOCUMENTS {
        reasons.insert("canonical_document_limit");
    }
    let mut canonical = Vec::new();
    for document in documents.into_iter().take(MAX_CANONICAL_DOCUMENTS) {
        let content: Option<String> = document.try_get("content")?;
        let Some(content) = content else {
            reasons.insert("canonical_source_byte_limit");
            continue;
        };
        let lines = content.lines().collect::<Vec<_>>();
        let mut selectors = Vec::new();
        let parsed = rules::parse_history_rows_with_lines(&content);
        let table_line_count = lines
            .iter()
            .filter(|line| line.trim_start().starts_with('|'))
            .count();
        if table_line_count != parsed.len() + 2 {
            reasons.insert("canonical_parse_error");
        }
        for (line, row) in parsed {
            let overlaps = if row.kind == "transit" {
                row.arrived_at >= from && row.arrived_at < to
            } else {
                row.arrived_at < to && row.departed_at.is_none_or(|end| end > from)
            };
            if overlaps {
                selectors.push(json!({"start_line":line,"end_line":line,"text":lines[line-1],
                    "origin":"canonical_visit","arrived_at":row.arrived_at,"departed_at":row.departed_at,
                    "precision":"canonical minute and rounded coordinate values; use raw sources for greater precision"}));
            }
        }
        let path: String = document.try_get("path")?;
        // Keep the requested months even when no canonical row matches.
        if !selectors.is_empty()
            || path
                >= format!(
                    "Location/Visits/{}.md",
                    from.with_timezone(&timezone).format("%Y-%m")
                )
        {
            let id: uuid::Uuid = document.try_get("id")?;
            canonical.push(json!({"ref":format!("entry:{id}"),"path":path,
                "version":document.try_get::<i64,_>("current_version")?,
                "content_hash":format!("sha256:{}",document.try_get::<String,_>("content_sha256")?),
                "selectors":selectors}));
        }
    }
    let places_row = sqlx::query(r#"
        SELECT entry.id,entry.current_version,version.content_sha256,version.size_bytes,
               CASE WHEN version.size_bytes <= $2 THEN version.content END AS content
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version ON version.user_id=entry.user_id
          AND version.entry_id=entry.id AND version.version=entry.current_version
        WHERE entry.user_id=$1 AND entry.path='Location/Places.md' AND entry.deleted_at IS NULL AND entry.kind='markdown'
    "#).bind(auth.user_id.0).bind(MAX_PLACES_BYTES).fetch_optional(&mut **tx).await?;
    let places = if let Some(row) = places_row {
        let id: uuid::Uuid = row.try_get("id")?;
        let content: Option<String> = row.try_get("content")?;
        if content.is_none() {
            reasons.insert("places_byte_limit");
        }
        json!({"ref":format!("entry:{id}"),"path":"Location/Places.md","version":row.try_get::<i64,_>("current_version")?,
            "content_hash":format!("sha256:{}",row.try_get::<String,_>("content_sha256")?),
            "origin":"known_place_definitions","start_line":1,"end_line":content.as_ref().map(|s|s.lines().count()),"text":content})
    } else {
        Value::Null
    };
    let complete = reasons.is_empty();
    let mut packet = json!({
        "schema":"location.evidence.v1",
        "interval":{"from":from,"to":to,"timezone":query.timezone,"semantics":"inclusive from, exclusive to"},
        "snapshot":{"at":snapshot_at,"isolation":"repeatable_read","database_snapshot":snapshot.try_get::<String,_>("snapshot")?,
            "workspace_generation":snapshot.try_get::<i64,_>("generation")?,"reusable_snapshot_token":false},
        "completeness":{"complete":complete,"reasons":reasons,"report_limit":MAX_REPORTS,
            "byte_limit":MAX_PACKET_BYTES,"raw_retention_days":RETENTION_DAYS,"raw_retention_before":snapshot_at-Duration::days(RETENTION_DAYS),
            "physical_stop_coverage":"unverified","meaning":"Completeness covers bounded retained sources in this snapshot, not all physical stops or historical server availability."},
        "time_semantics":{
            "raw_key":"authenticated owner plus exact at and type; no invented raw version identity",
            "ping_at":"device location sample time",
            "visit_at":"Apple callback report time; not location sample or visit arrival time",
            "arrived_at_departed_at":"Apple visit estimates; null departure is unknown, not the requested interval end",
            "first_received_at":"server ingest instant of the first committed insert; null for legacy unknown receipt time",
            "delays":"sample/callback-to-receipt differences include clock uncertainty; negative differences are not clamped",
            "geocode":"address and area hints, not confirmed venues",
            "poi":"ranked nearby candidates, not evidence of entering a business"
        },
        "reports":reports,
        "boundary_observations":{"before":before,"after":after,"meaning":"Nearest retained usable ping outside the interval; not part of day coverage."},
        "canonical_months":canonical,"places":places,
        "sample_gaps":{"minimum_minutes":GAP_MINUTES,"intervals":gaps,
            "meaning":"No retained usable sampled position within these intervals. This does not establish driving, a missed stop, upload failure, or continuous presence."},
        "fingerprint_complete":false,"evidence_fingerprint":null
    });
    enforce_packet_budget(&mut packet)?;
    if packet["completeness"]["complete"] == true {
        packet["evidence_fingerprint"] = json!(evidence_fingerprint(&packet)?);
        packet["fingerprint_complete"] = json!(true);
    }
    Ok(packet)
}

const REPORT_SELECT: &str = r#"
    SELECT (to_jsonb(report)-'user_id') || jsonb_build_object('poi',COALESCE(pois.items,'[]'::jsonb)) AS report
    FROM brunn.location_reports AS report
    LEFT JOIN LATERAL (
      SELECT jsonb_agg(jsonb_build_object('rank',poi.rank,'name',poi.name,'category',poi.category,'distance_m',poi.distance_m) ORDER BY poi.rank) AS items
      FROM brunn.location_report_poi AS poi
      WHERE poi.user_id=report.user_id AND poi.at=report.at AND poi.type=report.type
    ) AS pois ON true
"#;

fn annotate_report(report: &mut Value) {
    report["natural_key"] = json!({"at":report["at"],"type":report["type"]});
    report["origin"] = json!(if report["type"] == "ping" {
        "sample"
    } else {
        "apple_visit_estimate"
    });
}

async fn boundary(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    at: DateTime<Utc>,
    before: bool,
) -> ApiResult<Value> {
    let filter = if before {
        "report.at < $2 ORDER BY report.at DESC"
    } else {
        "report.at >= $2 ORDER BY report.at ASC"
    };
    // Filter is selected from two fixed SQL fragments, never request text.
    let sql = format!(
        "{REPORT_SELECT} WHERE report.user_id=$1 AND report.type='ping' AND report.accuracy_m BETWEEN 0 AND 1000 AND {filter} LIMIT 1"
    );
    let row = sqlx::query(AssertSqlSafe(sql.as_str()))
        .bind(auth.user_id.0)
        .bind(at)
        .fetch_optional(&mut **tx)
        .await?;
    match row {
        Some(row) => {
            let mut value: Value = row.try_get("report")?;
            annotate_report(&mut value);
            Ok(value)
        }
        None => Ok(Value::Null),
    }
}

fn sample_gaps(reports: &[Value], from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Value> {
    let mut times = reports
        .iter()
        .filter(|report| {
            report["type"] == "ping"
                && report["accuracy_m"]
                    .as_f64()
                    .is_some_and(|m| (0.0..=1000.0).contains(&m))
        })
        .filter_map(|report| {
            report["at"]
                .as_str()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.to_utc())
        })
        .filter(|at| *at >= from && *at < to)
        .collect::<Vec<_>>();
    times.extend([from, to]);
    times.sort_unstable();
    times.dedup();
    times.windows(2).filter(|pair|pair[1]-pair[0]>=Duration::minutes(GAP_MINUTES))
        .map(|pair|json!({"from":pair[0],"to":pair[1],"duration_seconds":(pair[1]-pair[0]).num_seconds(),"label":"sample_gap"})).collect()
}

/// Compare scope-relevant evidence, not the changing whole-month version or
/// snapshot clock. Unrelated later-day appends do not change this fingerprint;
/// late raw-only rows, receipt/source expiry, selected rows and Places changes do.
pub fn evidence_fingerprint(packet: &Value) -> ApiResult<String> {
    let canonical=packet["canonical_months"].as_array().into_iter().flatten().map(|doc|json!({
        "ref":doc["ref"],"rows":doc["selectors"].as_array().into_iter().flatten().map(|row|row["text"].clone()).collect::<Vec<_>>()
    })).collect::<Vec<_>>();
    let inputs = json!({"schema":"location.evidence.v1","interval":packet["interval"],"reports":packet["reports"],
        "boundary_observations":packet["boundary_observations"],"canonical":canonical,
        "places":packet["places"]});
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&inputs)?))
    ))
}

fn enforce_packet_budget(packet: &mut Value) -> ApiResult<()> {
    // Leave room for the final fingerprint. Drop whole records only and mark
    // the result incomplete; no consumer may publish from a partial packet.
    let budget = MAX_PACKET_BYTES - 256;
    if serde_json::to_vec(packet)?.len() <= budget {
        return Ok(());
    }
    packet["completeness"]["complete"] = json!(false);
    packet["completeness"]["reasons"]
        .as_array_mut()
        .expect("reasons")
        .push(json!("packet_byte_limit"));
    let all = packet["reports"].as_array().expect("reports").clone();
    let mut lower = 0;
    let mut upper = all.len();
    while lower < upper {
        let count = lower + (upper - lower).div_ceil(2);
        packet["reports"] = json!(&all[..count]);
        if serde_json::to_vec(packet)?.len() <= budget {
            lower = count;
        } else {
            upper = count - 1;
        }
    }
    packet["reports"] = json!(&all[..lower]);
    if serde_json::to_vec(packet)?.len() > budget {
        packet["canonical_months"] = json!([]);
    }
    if serde_json::to_vec(packet)?.len() > budget {
        packet["places"] = Value::Null;
    }
    if serde_json::to_vec(packet)?.len() > budget {
        return Err(ApiError::invalid(
            "location evidence metadata exceeds its bounded response budget",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gaps_are_sample_absence_and_open_ended_estimates_stay_unknown() {
        let from = DateTime::parse_from_rfc3339("2026-09-06T00:00:00-07:00")
            .unwrap()
            .to_utc();
        let to = from + Duration::hours(1);
        let reports = vec![
            json!({"type":"visit_arrival","at":from,"arrived_at":from,"departed_at":null,"accuracy_m":5}),
        ];
        let gaps = sample_gaps(&reports, from, to);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["label"], "sample_gap");
        assert!(reports[0]["departed_at"].is_null());
    }
}

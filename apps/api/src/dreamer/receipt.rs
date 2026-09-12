//! Strict compatibility projection consumed by the morning briefing.
//! New lifecycle fields belong in the immutable attempt, never this v2 shape.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate};
use serde_json::Value;

pub const SCHEMA: &str = "dream.latest-receipt.v2";
pub const PATH: &str = "dreams/latest-receipt.md";
const FIELDS: &[&str] = &[
    "schema",
    "run_id",
    "receipt_ref",
    "receipt_version",
    "receipt_path",
    "status",
    "completed_at",
    "mode",
    "runner",
    "mode_flip",
    "probe_monitoring",
    "applied_writes",
    "entering_veto_window_today",
    "pending_owner",
    "pending_review_surfaces",
    "next_run_at",
];

fn fields(value: &Value, names: &[&str]) -> Result<(), String> {
    let object = value.as_object().ok_or("receipt object required")?;
    if object.len() != names.len() || names.iter().any(|key| !object.contains_key(*key)) {
        return Err("receipt fields do not match the v2 contract".into());
    }
    Ok(())
}

fn text<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str, String> {
    let value = value[key]
        .as_str()
        .ok_or_else(|| format!("{key} must be text"))?;
    if value.trim().is_empty()
        || value.chars().count() > max
        || value.chars().any(|c| c == '\r' || c == '\0')
    {
        return Err(format!("invalid {key}"));
    }
    Ok(value)
}

fn positive(value: &Value, key: &str) -> Result<i64, String> {
    value[key]
        .as_i64()
        .filter(|v| *v > 0)
        .ok_or_else(|| format!("invalid {key}"))
}

fn timestamp(value: &Value, key: &str) -> Result<DateTime<chrono::FixedOffset>, String> {
    DateTime::parse_from_rfc3339(text(value, key, 80)?).map_err(|_| format!("invalid {key}"))
}

fn portable(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.contains('`')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn reference(value: &Value, key: &str) -> Result<(), String> {
    let raw = text(value, key, 80)?;
    let id = raw
        .strip_prefix("entry:")
        .ok_or("invalid entry reference")?;
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| "invalid entry reference")?;
    if parsed.to_string() != id
        || parsed.get_version_num() != 7
        || parsed.get_variant() != uuid::Variant::RFC4122
    {
        return Err("noncanonical entry reference".into());
    }
    Ok(())
}

pub fn format_timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// v2 transport identity. Durable proposal/decision IDs retain their original
/// date/item form; this projection's historical consumer forbids slashes.
pub fn recommendation_id(original: &str) -> String {
    format!("dream-{}", original.replace('/', "-"))
}

fn valid_recommendation_id(id: &str) -> bool {
    (3..=120).contains(&id.len())
        && id
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

fn canonical_payload(payload: &Value) -> Result<Value, String> {
    let mut value = payload.clone();
    for key in ["completed_at", "next_run_at"] {
        value[key] = Value::String(
            timestamp(payload, key)?.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
    }
    for (category, key) in [
        ("pending_owner", "published_at"),
        ("pending_review_surfaces", "published_at"),
        ("entering_veto_window_today", "apply_at"),
    ] {
        for row in value[category]
            .as_array_mut()
            .ok_or("invalid timestamp category")?
        {
            row[key] = Value::String(
                timestamp(row, key)?.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            );
        }
    }
    Ok(value)
}

/// The nightly project status signal: `{"assigned":n,"transitions":[...]}`
/// or `{"failure":"..."}`. Optional; receipts before it remain valid.
pub fn project_status(value: &Value) -> Result<(), String> {
    if value.get("failure").is_some() {
        fields(value, &["failure"])?;
        text(value, "failure", 1000)?;
        return Ok(());
    }
    fields(value, &["assigned", "transitions"])?;
    value["assigned"].as_u64().ok_or("invalid assigned")?;
    let transitions = value["transitions"]
        .as_array()
        .ok_or("invalid transitions")?;
    if transitions.len() > 300 {
        return Err("transitions exceed bounds".into());
    }
    for row in transitions {
        fields(row, &["slug", "from", "to"])?;
        text(row, "slug", 100)?;
        for key in ["from", "to"] {
            if !super::project_status::STATUSES.contains(&text(row, key, 8)?) {
                return Err(format!("invalid transition {key}"));
            }
        }
    }
    Ok(())
}

/// One line for the run document's "Project status" heading.
pub fn project_status_line(value: &Value) -> String {
    if let Some(failure) = value["failure"].as_str() {
        return format!("Not assigned: {failure}");
    }
    let transitions = value["transitions"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    format!(
                        "{}: {} → {}",
                        row["slug"].as_str().unwrap_or(""),
                        row["from"].as_str().unwrap_or(""),
                        row["to"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    format!(
        "Assigned {}; transitions: {}.",
        value["assigned"],
        if transitions.is_empty() {
            "none".to_owned()
        } else {
            transitions.join(", ")
        }
    )
}

/// Validate every row; never silently drop old pending recommendations to fit v2.
pub fn validate(payload: &Value) -> Result<(), String> {
    let mut names = FIELDS.to_vec();
    if let Some(status) = payload.get("project_status") {
        project_status(status)?;
        names.push("project_status");
    }
    fields(payload, &names)?;
    if payload["schema"] != SCHEMA {
        return Err("unsupported receipt schema".into());
    }
    let run_id = text(payload, "run_id", 10)?;
    let date = NaiveDate::parse_from_str(run_id, "%Y-%m-%d").map_err(|_| "invalid run_id")?;
    if date.format("%Y-%m-%d").to_string() != run_id {
        return Err("invalid run_id".into());
    }
    if payload["receipt_path"] != format!("dreams/runs/{run_id}.md") {
        return Err("receipt path mismatch".into());
    }
    reference(payload, "receipt_ref")?;
    positive(payload, "receipt_version")?;
    if !matches!(
        text(payload, "status", 16)?,
        "completed" | "partial" | "skipped" | "failed"
    ) {
        return Err("invalid receipt status".into());
    }
    let completed = timestamp(payload, "completed_at")?;
    if timestamp(payload, "next_run_at")? <= completed {
        return Err("next run must follow completion".into());
    }
    text(payload, "runner", 120)?;
    if !payload["mode"].is_null()
        && !matches!(payload["mode"].as_str(), Some("report-only" | "full"))
    {
        return Err("invalid mode".into());
    }
    // No compatible evidence of the retired monitoring system is manufactured.
    if payload["mode_flip"] != false || !payload["probe_monitoring"].is_null() {
        return Err("mode transition requires a separately qualified monitoring contract".into());
    }
    let mut recommendation_ids = BTreeSet::new();
    for (category, keys, limit) in [
        ("applied_writes", &["path", "version", "summary"][..], 300),
        (
            "entering_veto_window_today",
            &["recommendation_id", "summary", "apply_at"][..],
            100,
        ),
        (
            "pending_owner",
            &[
                "recommendation_id",
                "summary",
                "reason",
                "published_at",
                "age_days",
            ][..],
            100,
        ),
        (
            "pending_review_surfaces",
            &[
                "review_id",
                "summary",
                "reason",
                "path",
                "ref",
                "version",
                "published_at",
            ][..],
            8,
        ),
    ] {
        let rows = payload[category]
            .as_array()
            .ok_or_else(|| format!("invalid {category}"))?;
        if rows.len() > limit {
            return Err(format!("{category} exceeds v2 bounds; backlog retained"));
        }
        let mut previous = None;
        let mut ids = BTreeSet::new();
        for row in rows {
            fields(row, keys)?;
            text(row, "summary", 1000)?;
            let sort_key = match category {
                "applied_writes" => {
                    let path = text(row, "path", 1024)?;
                    if !portable(path) || path.to_ascii_lowercase().starts_with("dreams/") {
                        return Err("invalid applied path".into());
                    }
                    let version = positive(row, "version")?;
                    (path.to_owned(), format!("{version:020}"))
                }
                "entering_veto_window_today" => {
                    let id = text(row, "recommendation_id", 120)?;
                    if !valid_recommendation_id(id) {
                        return Err("incompatible recommendation id".into());
                    }
                    if !recommendation_ids.insert(id.to_owned()) {
                        return Err("duplicate recommendation state".into());
                    }
                    timestamp(row, "apply_at")?;
                    (text(row, "apply_at", 80)?.to_owned(), id.to_owned())
                }
                "pending_owner" => {
                    let id = text(row, "recommendation_id", 120)?;
                    if !valid_recommendation_id(id) {
                        return Err("incompatible recommendation id".into());
                    }
                    if !recommendation_ids.insert(id.to_owned()) {
                        return Err("duplicate recommendation state".into());
                    }
                    text(row, "reason", 1000)?;
                    timestamp(row, "published_at")?;
                    row["age_days"].as_u64().ok_or("invalid age_days")?;
                    (id.to_owned(), String::new())
                }
                _ => {
                    text(row, "reason", 1000)?;
                    if !portable(text(row, "path", 1024)?) {
                        return Err("invalid review path".into());
                    }
                    reference(row, "ref")?;
                    positive(row, "version")?;
                    timestamp(row, "published_at")?;
                    (text(row, "review_id", 160)?.to_owned(), String::new())
                }
            };
            if !ids.insert(sort_key.clone()) || previous.as_ref().is_some_and(|v| v > &sort_key) {
                return Err(format!("{category} must be sorted and unique"));
            }
            previous = Some(sort_key);
        }
    }
    Ok(())
}

/// serde_json's default map uses lexical keys, matching the consumer's
/// sort_keys=True, ensure_ascii=False, separators=(',', ':') representation.
pub fn render_latest(payload: &Value) -> Result<String, String> {
    validate(payload)?;
    let canonical = canonical_payload(payload)?;
    let json = serde_json::to_string(&canonical).map_err(|e| e.to_string())?;
    if json.len() > 256 * 1024 {
        return Err("receipt exceeds compatible projection size; backlog retained".into());
    }
    Ok(format!(
        "---\nschema: {SCHEMA}\nrun_id: {}\nstatus: {}\n---\n# Latest Dreaming receipt\n\nThis is a stable briefing projection. The immutable receipt is authoritative.\n\n```json\n{json}\n```\n",
        payload["run_id"].as_str().unwrap(),
        payload["status"].as_str().unwrap(),
    ))
}

pub fn parse_latest(content: &str) -> Result<Value, String> {
    if content.len() > 512 * 1024 {
        return Err("receipt exceeds bounds".into());
    }
    let raw = content
        .split_once("```json\n")
        .ok_or("missing receipt payload")?
        .1
        .strip_suffix("\n```\n")
        .ok_or("invalid receipt document")?;
    let payload: Value = serde_json::from_str(raw).map_err(|_| "invalid receipt JSON")?;
    if render_latest(&payload)? != content {
        return Err("noncanonical receipt document".into());
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn receipt() -> Value {
        json!({"schema":SCHEMA,"run_id":"2026-09-07","receipt_ref":"entry:01a07b52-2bf0-7d62-8969-9ea0b3c49399","receipt_version":2,"receipt_path":"dreams/runs/2026-09-07.md","status":"completed","completed_at":"2026-09-07T10:10:00Z","mode":"report-only","runner":"brunn-rust-dreamer","mode_flip":false,"probe_monitoring":null,"applied_writes":[],"entering_veto_window_today":[],"pending_owner":[],"pending_review_surfaces":[],"next_run_at":"2026-09-08T10:00:00Z"})
    }
    #[test]
    fn exact_v2_round_trip_and_immutable_version() {
        let value = receipt();
        assert_eq!(value.as_object().unwrap().len(), 16);
        let rendered = render_latest(&value).unwrap();
        assert_eq!(parse_latest(&rendered).unwrap(), value);
        assert_eq!(
            rendered,
            include_str!("../../tests/fixtures/dreamer/latest_receipt_v2.md")
        );
        assert!(rendered.starts_with("---\nschema: dream.latest-receipt.v2\nrun_id: 2026-09-07\nstatus: completed\n---\n# Latest Dreaming receipt\n\n"));
    }
    #[test]
    fn rejects_unknown_duplicate_noncanonical_and_fake_mode_flip() {
        let mut value = receipt();
        value["processed"] = json!(1);
        assert!(render_latest(&value).is_err());
        let valid = render_latest(&receipt()).unwrap();
        assert!(
            parse_latest(&valid.replace(
                "\"schema\":",
                "\"schema\":\"dream.latest-receipt.v2\",\"schema\":"
            ))
            .is_err()
        );
        assert!(parse_latest(&valid.replace("\"runner\":", "\"runner\" : ")).is_err());
        let mut value = receipt();
        value["mode_flip"] = json!(true);
        assert!(render_latest(&value).is_err());
    }
    #[test]
    fn historical_transport_ids_and_seconds_are_required() {
        assert_eq!(recommendation_id("2026-09-07/1"), "dream-2026-09-07-1");
        let mut value = receipt();
        value["completed_at"] = json!("2026-09-07T10:10:00.123456789+00:00");
        let rendered = render_latest(&value).unwrap();
        assert!(rendered.contains("\"completed_at\":\"2026-09-07T10:10:00Z\""));
        let mut value = receipt();
        value["pending_owner"] = json!([{"recommendation_id":"2026-09-07/1","summary":"Review","reason":"Owner decision required","published_at":"2026-09-07T10:10:00Z","age_days":0}]);
        assert!(render_latest(&value).is_err());
    }

    #[test]
    fn project_status_is_optional_bounded_and_rendered() {
        let mut value = receipt();
        assert_eq!(
            parse_latest(&render_latest(&value).unwrap()).unwrap(),
            value
        );
        value["project_status"] =
            json!({"assigned":3,"transitions":[{"slug":"orchid","from":"grey","to":"yellow"}]});
        let rendered = render_latest(&value).unwrap();
        assert_eq!(parse_latest(&rendered).unwrap(), value);
        assert!(rendered.contains("\"project_status\":{\"assigned\":3"));
        assert_eq!(
            project_status_line(&value["project_status"]),
            "Assigned 3; transitions: orchid: grey → yellow."
        );
        assert_eq!(
            project_status_line(&json!({"assigned":2,"transitions":[]})),
            "Assigned 2; transitions: none."
        );
        assert_eq!(
            project_status_line(&json!({"failure":"packet unavailable"})),
            "Not assigned: packet unavailable"
        );
        for invalid in [
            json!({"assigned":1}),
            json!({"assigned":1,"transitions":[{"slug":"orchid","from":"grey","to":"amber"}]}),
            json!({"assigned":1,"transitions":[{"slug":"orchid","from":"grey"}]}),
            json!({"failure":""}),
            json!(null),
        ] {
            let mut value = receipt();
            value["project_status"] = invalid.clone();
            assert!(render_latest(&value).is_err(), "{invalid}");
        }
    }

    #[test]
    fn never_truncates_backlog_or_counts_report_writes() {
        let mut value = receipt();
        value["pending_owner"] = json!((0..101).map(|n| json!({"recommendation_id":format!("dream-2026-09-07-{n:03}"),"summary":"Review","reason":"explicit approval required","published_at":"2026-09-07T10:10:00Z","age_days":0})).collect::<Vec<_>>());
        assert!(render_latest(&value).is_err());
        let mut value = receipt();
        value["applied_writes"] =
            json!([{"path":"dreams/runs/2026-09-07.md","version":1,"summary":"report"}]);
        assert!(render_latest(&value).is_err());
    }
}

use std::collections::HashMap;

use axum::{Extension, Json, extract::Query, extract::State};
use chrono::{DateTime, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
    object_store::PhysicalUsageStatus,
    simple_core::WorkspaceEnvelope,
};

const ACTIVITY_DAYS: i64 = 7;
const DEFAULT_TIMEZONE: &str = "UTC";
const ACTIVITY_COVERAGE: &str = "tracked_operations_only";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardQuery {
    pub timezone: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DashboardData {
    pub generated_at: DateTime<Utc>,
    pub timezone: String,
    pub workspace_generation: i64,
    pub activity_tracking_started_at: Option<DateTime<Utc>>,
    pub tracking: DashboardTracking,
    pub storage: DashboardStorage,
    pub today: ActivityTotals,
    pub activity: Vec<ActivityDay>,
    pub access: Vec<AccessItem>,
    pub coverage: DashboardCoverage,
}

#[derive(Clone, Debug, Serialize)]
pub struct DashboardStorage {
    pub text: StorageTotal,
    pub binary: BinaryStorageTotal,
}

#[derive(Clone, Debug, Serialize)]
pub struct StorageTotal {
    pub count: i64,
    pub size_bytes: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BinaryStorageTotal {
    pub count: Option<u64>,
    pub size_bytes: Option<u64>,
    pub semantics: &'static str,
    pub status: PhysicalUsageStatus,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ActivityTotals {
    pub read_operations: i64,
    pub read_bytes: i64,
    pub write_operations: i64,
    pub write_bytes: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivityDay {
    pub date: NaiveDate,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    #[serde(flatten)]
    pub totals: ActivityTotals,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccessItem {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub manageable: bool,
    pub access: &'static str,
    pub status: &'static str,
    pub scope_ids: Vec<String>,
    pub capabilities: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_operation: Option<String>,
    pub read_operations_today: i64,
    pub write_operations_today: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DashboardCoverage {
    pub days: i64,
    pub activity: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct DashboardTracking {
    pub status: &'static str,
    pub tracking_started_at: Option<DateTime<Utc>>,
    pub data_through: Option<DateTime<Utc>>,
    pub last_flush_at: Option<DateTime<Utc>>,
    pub dropped_events: u64,
    pub flush_failures: u64,
}

#[derive(Clone, Debug)]
struct ActivityRow {
    credential_id: Uuid,
    bucket_start: DateTime<Utc>,
    operation: String,
    operation_count: i64,
    byte_count: i64,
}

pub async fn dashboard(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<DashboardQuery>,
) -> ApiResult<Json<WorkspaceEnvelope<DashboardData>>> {
    auth.require(Capability::Read)?;
    auth.require(Capability::Status)?;
    let timezone_name = query.timezone.as_deref().unwrap_or(DEFAULT_TIMEZONE);
    if timezone_name.len() > 100 || timezone_name.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            "timezone must be an IANA timezone name of at most 100 characters",
        ));
    }
    let timezone = timezone_name.parse::<Tz>().map_err(|_| {
        ApiError::invalid(format!(
            "timezone must be a recognized IANA timezone name: {timezone_name}"
        ))
    })?;
    let generated_at = Utc::now();
    let tracking_health = state.usage_tracker.product_activity_health();
    let local_today = generated_at.with_timezone(&timezone).date_naive();
    let first_date = local_today - Duration::days(ACTIVITY_DAYS - 1);
    let range_start = local_day_boundary(first_date, timezone)?;
    let range_end = local_day_boundary(local_today + Duration::days(1), timezone)?;

    let mut tx = state.begin_read(&auth).await?;
    let storage = sqlx::query(
        r#"
        WITH current_entries AS MATERIALIZED (
          SELECT entry.kind,version.size_bytes
          FROM brunn.entries AS entry
          JOIN brunn.entry_versions AS version
            ON version.user_id=entry.user_id
           AND version.entry_id=entry.id
           AND version.version=entry.current_version
          WHERE entry.user_id=$1 AND entry.deleted_at IS NULL
        )
        SELECT
          count(*) FILTER (WHERE kind='markdown')::bigint AS text_count,
          coalesce(sum(size_bytes) FILTER (WHERE kind='markdown'),0)::bigint
            AS text_size_bytes,
          brunn_auth.workspace_generation($1)::bigint AS workspace_generation,
          (
            SELECT min(first_recorded_at)
            FROM brunn.product_activity_minutely
            WHERE user_id=$1
          ) AS activity_tracking_started_at
        FROM current_entries
        "#,
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await?;
    let activity_rows = sqlx::query(
        r#"
        SELECT credential_id,bucket_start,operation,operation_count,byte_count
        FROM brunn.product_activity_minutely
        WHERE user_id=$1 AND bucket_start >= $2 AND bucket_start < $3
        ORDER BY bucket_start,credential_id,operation
        "#,
    )
    .bind(auth.user_id.0)
    .bind(range_start)
    .bind(range_end)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|row| -> ApiResult<ActivityRow> {
        Ok(ActivityRow {
            credential_id: row.try_get("credential_id")?,
            bucket_start: row.try_get("bucket_start")?,
            operation: row.try_get("operation")?,
            operation_count: row.try_get("operation_count")?,
            byte_count: row.try_get("byte_count")?,
        })
    })
    .collect::<ApiResult<Vec<_>>>()?;
    let credential_rows = sqlx::query("SELECT * FROM brunn_auth.dashboard_credentials($1)")
        .bind(auth.user_id.0)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let physical_usage = state
        .object_store
        .physical_usage(&auth.user_id.0.to_string())
        .await?;

    let (activity, per_credential_today) =
        aggregate_activity(first_date, local_today, timezone, &activity_rows)?;
    let today = activity.last().map(|day| day.totals).unwrap_or_default();
    let access = credential_rows
        .into_iter()
        .map(|row| -> ApiResult<AccessItem> {
            let credential_id: Uuid = row.try_get("id")?;
            let capabilities: Vec<String> = row.try_get("capabilities")?;
            let revoked_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;
            let credential_today = per_credential_today
                .get(&credential_id)
                .copied()
                .unwrap_or_default();
            Ok(AccessItem {
                id: format!("credential:{credential_id}"),
                name: row.try_get("label")?,
                kind: row.try_get("kind")?,
                manageable: row.try_get("manageable")?,
                access: credential_access_label(&capabilities),
                status: if revoked_at.is_some() {
                    "revoked"
                } else {
                    "active"
                },
                scope_ids: row.try_get("scope_refs")?,
                capabilities,
                created_at: row.try_get("created_at")?,
                revoked_at,
                last_used_at: row.try_get("last_used_at")?,
                last_operation: row.try_get("last_operation")?,
                read_operations_today: credential_today.read_operations,
                write_operations_today: credential_today.write_operations,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;

    let activity_tracking_started_at = storage.try_get("activity_tracking_started_at")?;
    Ok(Json(WorkspaceEnvelope::complete(DashboardData {
        generated_at,
        timezone: timezone.name().to_owned(),
        workspace_generation: storage.try_get("workspace_generation")?,
        activity_tracking_started_at,
        tracking: DashboardTracking {
            status: tracking_health.status.as_str(),
            tracking_started_at: activity_tracking_started_at,
            data_through: tracking_health.data_through,
            last_flush_at: tracking_health.last_successful_flush_at,
            dropped_events: tracking_health.dropped_events,
            flush_failures: tracking_health.failed_flushes,
        },
        storage: DashboardStorage {
            text: StorageTotal {
                count: storage.try_get("text_count")?,
                size_bytes: storage.try_get("text_size_bytes")?,
            },
            binary: BinaryStorageTotal {
                count: physical_usage.physical_object_versions,
                size_bytes: physical_usage.physical_size_bytes,
                semantics: physical_usage.object_count_semantics,
                status: physical_usage.status,
                observed_at: physical_usage.observed_at,
            },
        },
        today,
        activity,
        access,
        coverage: DashboardCoverage {
            days: ACTIVITY_DAYS,
            activity: ACTIVITY_COVERAGE,
        },
    })))
}

fn aggregate_activity(
    first_date: NaiveDate,
    local_today: NaiveDate,
    timezone: Tz,
    rows: &[ActivityRow],
) -> ApiResult<(Vec<ActivityDay>, HashMap<Uuid, ActivityTotals>)> {
    let mut totals_by_date = HashMap::<NaiveDate, ActivityTotals>::new();
    let mut per_credential_today = HashMap::<Uuid, ActivityTotals>::new();
    for row in rows {
        let date = row.bucket_start.with_timezone(&timezone).date_naive();
        if date < first_date || date > local_today {
            continue;
        }
        add_operation(
            totals_by_date.entry(date).or_default(),
            &row.operation,
            row.operation_count,
            row.byte_count,
        );
        if date == local_today {
            add_operation(
                per_credential_today.entry(row.credential_id).or_default(),
                &row.operation,
                row.operation_count,
                row.byte_count,
            );
        }
    }
    let mut activity = Vec::with_capacity(ACTIVITY_DAYS as usize);
    for offset in 0..ACTIVITY_DAYS {
        let date = first_date + Duration::days(offset);
        activity.push(ActivityDay {
            date,
            period_start: local_day_boundary(date, timezone)?,
            period_end: local_day_boundary(date + Duration::days(1), timezone)?,
            totals: totals_by_date.remove(&date).unwrap_or_default(),
        });
    }
    Ok((activity, per_credential_today))
}

fn add_operation(totals: &mut ActivityTotals, operation: &str, count: i64, bytes: i64) {
    match operation {
        "open" | "search" | "read" | "binary_fetch" | "briefing_list" | "briefing_read"
        | "briefing_topics" => {
            totals.read_operations = totals.read_operations.saturating_add(count);
            totals.read_bytes = totals.read_bytes.saturating_add(bytes);
        }
        "write" | "capture" | "checkpoint" | "binary_upload" | "delete" | "briefing_publish"
        | "briefing_action" => {
            totals.write_operations = totals.write_operations.saturating_add(count);
            totals.write_bytes = totals.write_bytes.saturating_add(bytes);
        }
        unknown => tracing::warn!(operation = unknown, "unknown product activity ignored"),
    }
}

fn local_day_boundary(date: NaiveDate, timezone: Tz) -> ApiResult<DateTime<Utc>> {
    let mut candidate = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::Internal("could not construct local day boundary".to_owned()))?;
    for _ in 0..=(26 * 60) {
        match timezone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                return Ok(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => candidate += Duration::minutes(1),
        }
    }
    Err(ApiError::Internal(format!(
        "could not resolve a local day boundary for {date} in {}",
        timezone.name()
    )))
}

fn credential_access_label(capabilities: &[String]) -> &'static str {
    if capabilities
        .iter()
        .any(|value| value == "credential:manage" || value == "admin")
    {
        "owner"
    } else if capabilities.iter().any(|value| {
        matches!(
            value.as_str(),
            "checkpoint" | "save" | "stage" | "correct" | "delete" | "dream"
        )
    }) {
        "read_write"
    } else {
        "read_only"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_zero_filled_and_regrouped_by_local_date() {
        let timezone: Tz = "America/Los_Angeles".parse().unwrap();
        let credential_id = Uuid::now_v7();
        let rows = vec![
            ActivityRow {
                credential_id,
                bucket_start: "2026-08-02T06:00:00Z".parse().unwrap(),
                operation: "read".to_owned(),
                operation_count: 2,
                byte_count: 40,
            },
            ActivityRow {
                credential_id,
                bucket_start: "2026-08-02T07:00:00Z".parse().unwrap(),
                operation: "write".to_owned(),
                operation_count: 1,
                byte_count: 10,
            },
        ];
        let first_date = NaiveDate::from_ymd_opt(2026, 7, 27).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let (days, per_credential) =
            aggregate_activity(first_date, today, timezone, &rows).unwrap();

        assert_eq!(days.len(), 7);
        assert_eq!(days[5].date, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap());
        assert_eq!(days[5].totals.read_operations, 2);
        assert_eq!(days[5].totals.read_bytes, 40);
        assert_eq!(days[6].date, today);
        assert_eq!(days[6].totals.write_operations, 1);
        assert_eq!(days[6].totals.write_bytes, 10);
        assert_eq!(per_credential[&credential_id].write_operations, 1);
        assert_eq!(per_credential[&credential_id].read_operations, 0);
    }

    #[test]
    fn local_period_boundaries_follow_daylight_saving_time() {
        let timezone: Tz = "America/Los_Angeles".parse().unwrap();
        let spring = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let start = local_day_boundary(spring, timezone).unwrap();
        let end = local_day_boundary(spring + Duration::days(1), timezone).unwrap();
        assert_eq!(end - start, Duration::hours(23));

        let fall = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let start = local_day_boundary(fall, timezone).unwrap();
        let end = local_day_boundary(fall + Duration::days(1), timezone).unwrap();
        assert_eq!(end - start, Duration::hours(25));
    }

    #[test]
    fn minute_buckets_respect_fractional_offset_midnight() {
        let timezone: Tz = "Asia/Kathmandu".parse().unwrap();
        let credential_id = Uuid::now_v7();
        let rows = vec![
            ActivityRow {
                credential_id,
                bucket_start: "2026-08-01T18:14:00Z".parse().unwrap(),
                operation: "briefing_read".to_owned(),
                operation_count: 2,
                byte_count: 40,
            },
            ActivityRow {
                credential_id,
                bucket_start: "2026-08-01T18:15:00Z".parse().unwrap(),
                operation: "briefing_publish".to_owned(),
                operation_count: 1,
                byte_count: 10,
            },
        ];
        let first_date = NaiveDate::from_ymd_opt(2026, 7, 27).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let (days, per_credential) =
            aggregate_activity(first_date, today, timezone, &rows).unwrap();

        assert_eq!(days[5].totals.read_operations, 2);
        assert_eq!(days[6].totals.write_operations, 1);
        assert_eq!(per_credential[&credential_id].write_bytes, 10);
    }

    #[test]
    fn access_labels_match_credential_templates() {
        assert_eq!(credential_access_label(&["read".to_owned()]), "read_only");
        assert_eq!(
            credential_access_label(&["read".to_owned(), "save".to_owned()]),
            "read_write"
        );
        assert_eq!(
            credential_access_label(&["credential:manage".to_owned()]),
            "owner"
        );
        assert_eq!(credential_access_label(&["admin".to_owned()]), "owner");
        for capability in ["checkpoint", "save", "stage", "correct", "delete", "dream"] {
            assert_eq!(
                credential_access_label(&[capability.to_owned()]),
                "read_write",
                "{capability} must be classified as write-capable"
            );
        }
    }
}

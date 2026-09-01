use std::time::Duration;

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    db::AppState,
    error::{ApiError, ApiResult},
};

pub const TASK_GUARD_SCHEDULER_INTERVAL: Duration = Duration::from_secs(60);
const NOTIFICATION_TTL_DAYS: i64 = 7;

#[derive(Clone, Debug)]
struct GuardSettings {
    timezone: Tz,
    hard_lead_days: i32,
    hard_second_lead_hours: i32,
    due_day_local_time: NaiveTime,
    quiet_hours_start: NaiveTime,
    quiet_hours_end: NaiveTime,
    quiet_override_enabled: bool,
    quiet_override_within_hours: i32,
}

#[derive(Clone, Debug)]
struct GuardCandidate {
    user_id: Uuid,
    task_id: Uuid,
    title: String,
    hard_due: Option<DateTime<Utc>>,
    hard_due_lead_days: Option<i32>,
    cost_of_delay: bool,
    provenance: Value,
    source_timestamps: Value,
    task: Value,
    created_at: DateTime<Utc>,
    settings: GuardSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EventKind {
    HardDeadline { inferred: bool },
    CostSet,
    CostWeekly,
}

#[derive(Clone, Debug)]
struct GuardEvent {
    event_key: String,
    scheduled_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    title: String,
    body: String,
    delivery_available_at: DateTime<Utc>,
    kind: EventKind,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskGuardEventReport {
    pub event_key: String,
    pub task_id: Uuid,
    pub notification_ref: Option<String>,
    pub route: String,
    pub inserted: bool,
    pub delivery_count: i64,
    pub delivery_available_at: DateTime<Utc>,
    pub quiet_delayed: bool,
    pub inferred: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TaskGuardRunReport {
    pub as_of: DateTime<Utc>,
    pub evaluated_tasks: usize,
    pub events: Vec<TaskGuardEventReport>,
}

pub fn validate_as_of_override(
    deployment_environment: &str,
    as_of: Option<DateTime<Utc>>,
) -> ApiResult<DateTime<Utc>> {
    if deployment_environment == "production" && as_of.is_some() {
        return Err(ApiError::invalid(
            "task guard as_of overrides are forbidden in production",
        ));
    }
    Ok(as_of.unwrap_or_else(Utc::now))
}

pub async fn run_once(
    state: &AppState,
    as_of_override: Option<DateTime<Utc>>,
) -> ApiResult<TaskGuardRunReport> {
    let as_of = validate_as_of_override(&state.config.deployment_environment, as_of_override)?;
    let pool = state.admin_pool.as_ref().ok_or_else(|| {
        ApiError::configuration("DATABASE_URL_ADMIN is required by the task guard")
    })?;
    run_on_pool(pool, as_of, state.config.apns_delivery_enabled).await
}

pub async fn run_on_pool(
    pool: &PgPool,
    as_of: DateTime<Utc>,
    delivery_enabled: bool,
) -> ApiResult<TaskGuardRunReport> {
    let result = run_on_pool_inner(pool, as_of, delivery_enabled).await;
    let recorded_at = Utc::now();
    match result {
        Ok(report) => {
            record_guard_outcome(pool, recorded_at, "success", None).await?;
            Ok(report)
        }
        Err(error) => {
            let error_code = guard_error_code(&error);
            let _ = record_guard_outcome(pool, recorded_at, "failed", Some(error_code)).await;
            Err(error)
        }
    }
}

async fn run_on_pool_inner(
    pool: &PgPool,
    as_of: DateTime<Utc>,
    delivery_enabled: bool,
) -> ApiResult<TaskGuardRunReport> {
    let candidates = load_candidates(pool, as_of).await?;
    let mut report = TaskGuardRunReport {
        as_of,
        evaluated_tasks: candidates.len(),
        events: Vec::new(),
    };

    for candidate in candidates {
        for event in events_for_candidate(&candidate, as_of)? {
            let row = sqlx::query(
                r#"
                SELECT notification_id,inserted,delivery_count
                FROM brunn.enqueue_task_guard_notification(
                  $1,$2,$3,$4,$5,$6,$7,$8,$9
                )
                "#,
            )
            .bind(candidate.user_id)
            .bind(candidate.task_id)
            .bind(&event.event_key)
            .bind(&event.title)
            .bind(&event.body)
            .bind(event.scheduled_at)
            .bind(event.expires_at)
            .bind(event.delivery_available_at)
            .bind(delivery_enabled)
            .fetch_one(pool)
            .await?;
            let notification_id: Option<Uuid> = row.try_get("notification_id")?;
            let inserted: bool = row.try_get("inserted")?;
            let delivery_count: i64 = row.try_get("delivery_count")?;
            let route = format!("brunn://task/{}", candidate.task_id);
            report.events.push(TaskGuardEventReport {
                event_key: event.event_key,
                task_id: candidate.task_id,
                notification_ref: notification_id.map(|id| format!("notification:{}", id.simple())),
                route,
                inserted,
                delivery_count,
                delivery_available_at: event.delivery_available_at,
                quiet_delayed: event.delivery_available_at > as_of,
                inferred: matches!(event.kind, EventKind::HardDeadline { inferred: true }),
            });
        }
    }

    metrics::counter!(
        "task_guard.runs",
        "result" => "success"
    )
    .increment(1);
    metrics::counter!(
        "task_guard.events",
        "result" => "created"
    )
    .increment(report.events.iter().filter(|event| event.inserted).count() as u64);
    Ok(report)
}

fn guard_error_code(error: &ApiError) -> &'static str {
    match error {
        ApiError::Public { code, .. } => code,
        ApiError::Database(_) => "task_guard_database",
        ApiError::Migration(_) => "task_guard_migration",
        ApiError::Json(_) => "task_guard_json",
        ApiError::Internal(_) => "task_guard_internal",
    }
}

async fn record_guard_outcome(
    pool: &PgPool,
    recorded_at: DateTime<Utc>,
    outcome: &str,
    error_code: Option<&str>,
) -> ApiResult<()> {
    let next_run_at = recorded_at
        + chrono::Duration::from_std(TASK_GUARD_SCHEDULER_INTERVAL)
            .map_err(|_| ApiError::Internal("task guard interval is invalid".to_owned()))?;
    sqlx::query(
        r#"
        UPDATE brunn.task_guard_state AS state
        SET last_run_at=$1,last_outcome=$2,last_error_code=$3,
            next_run_at=$4,updated_at=clock_timestamp()
        FROM brunn.users AS account
        WHERE account.id=state.user_id AND account.account_status='active'
        "#,
    )
    .bind(recorded_at)
    .bind(outcome)
    .bind(error_code)
    .bind(next_run_at)
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_candidates(pool: &PgPool, as_of: DateTime<Utc>) -> ApiResult<Vec<GuardCandidate>> {
    let rows = sqlx::query(
        r#"
        SELECT task.user_id,task.task_id,task.title,task.hard_due,
               task.hard_due_lead_days,
               (task.cost_amount_cents IS NOT NULL OR task.cost_flag) AS cost_of_delay,
               task.provenance,task.source_timestamps,task.task,task.created_at,
               settings.timezone,settings.hard_lead_days,
               settings.hard_second_lead_hours,settings.due_day_local_time,
               settings.quiet_hours_start,settings.quiet_hours_end,
               settings.quiet_override_enabled,
               settings.quiet_override_within_hours
        FROM brunn.task_index AS task
        JOIN brunn.users AS account ON account.id=task.user_id
        JOIN brunn.task_settings AS settings ON settings.user_id=task.user_id
        WHERE account.account_status='active'
          AND task.status IN ('open','waiting')
          AND task.created_at <= $1
          AND (task.hard_due IS NOT NULL
               OR task.cost_amount_cents IS NOT NULL
               OR task.cost_flag)
        ORDER BY task.user_id,task.task_id
        "#,
    )
    .bind(as_of)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let timezone_name: String = row.try_get("timezone")?;
            let timezone = timezone_name.parse::<Tz>().map_err(|_| {
                ApiError::Internal(format!(
                    "task settings contain invalid timezone {timezone_name}"
                ))
            })?;
            Ok(GuardCandidate {
                user_id: row.try_get("user_id")?,
                task_id: row.try_get("task_id")?,
                title: row.try_get("title")?,
                hard_due: row.try_get("hard_due")?,
                hard_due_lead_days: row.try_get("hard_due_lead_days")?,
                cost_of_delay: row.try_get("cost_of_delay")?,
                provenance: row.try_get("provenance")?,
                source_timestamps: row.try_get("source_timestamps")?,
                task: row.try_get("task")?,
                created_at: row.try_get("created_at")?,
                settings: GuardSettings {
                    timezone,
                    hard_lead_days: row.try_get("hard_lead_days")?,
                    hard_second_lead_hours: row.try_get("hard_second_lead_hours")?,
                    due_day_local_time: row.try_get("due_day_local_time")?,
                    quiet_hours_start: row.try_get("quiet_hours_start")?,
                    quiet_hours_end: row.try_get("quiet_hours_end")?,
                    quiet_override_enabled: row.try_get("quiet_override_enabled")?,
                    quiet_override_within_hours: row.try_get("quiet_override_within_hours")?,
                },
            })
        })
        .collect()
}

fn events_for_candidate(
    candidate: &GuardCandidate,
    as_of: DateTime<Utc>,
) -> ApiResult<Vec<GuardEvent>> {
    let mut events = Vec::new();
    if let Some(due) = candidate.hard_due {
        let source = field_source(candidate, "hard_due").unwrap_or("derived");
        let field_set_at = field_set_at(candidate, "hard_due").unwrap_or(candidate.created_at);
        if field_set_at <= as_of {
            let actual_todoist_deadline = source == "todoist"
                && field_note(candidate, "hard_due") == Some("todoist_deadline");
            let inferred = source == "derived"
                || source.starts_with("agent:")
                || (source == "todoist" && !actual_todoist_deadline);
            let can_break_quiet = candidate.settings.quiet_override_enabled
                && (source == "owner" || actual_todoist_deadline)
                && due >= as_of
                && due
                    <= as_of
                        + chrono::Duration::hours(i64::from(
                            candidate.settings.quiet_override_within_hours,
                        ));
            let lead_days = candidate
                .hard_due_lead_days
                .unwrap_or(candidate.settings.hard_lead_days);
            let due_day = due_day_instant(due, &candidate.settings)?;
            let schedules = [
                (
                    format!("{lead_days}d"),
                    due - chrono::Duration::days(i64::from(lead_days)),
                ),
                (
                    format!("{}h", candidate.settings.hard_second_lead_hours),
                    due - chrono::Duration::hours(i64::from(
                        candidate.settings.hard_second_lead_hours,
                    )),
                ),
                ("due-day".to_owned(), due_day),
            ];
            for (lead, scheduled_at) in schedules {
                if scheduled_at > as_of {
                    continue;
                }
                let delivery_available_at =
                    delivery_available_at(as_of, &candidate.settings, can_break_quiet)?;
                let body = if inferred {
                    format!(
                        "inferred — confirm? Confirm or downgrade: {} has a hard deadline at {}.",
                        candidate.title,
                        due.to_rfc3339()
                    )
                } else {
                    format!(
                        "{} has a hard deadline at {}.",
                        candidate.title,
                        due.to_rfc3339()
                    )
                };
                events.push(GuardEvent {
                    event_key: format!("task-deadline:{}:{lead}", candidate.task_id),
                    scheduled_at,
                    expires_at: due + chrono::Duration::days(NOTIFICATION_TTL_DAYS),
                    title: "Task deadline".to_owned(),
                    body,
                    delivery_available_at,
                    kind: EventKind::HardDeadline { inferred },
                });
            }
        }
    }

    if candidate.cost_of_delay {
        let set_at = field_set_at(candidate, "cost_of_delay").unwrap_or(candidate.created_at);
        if set_at <= as_of {
            let delivery_available_at = delivery_available_at(as_of, &candidate.settings, false)?;
            events.push(GuardEvent {
                event_key: format!("task-cost:{}:set", candidate.task_id),
                scheduled_at: set_at,
                expires_at: set_at + chrono::Duration::days(NOTIFICATION_TTL_DAYS),
                title: "Task is accruing cost".to_owned(),
                body: format!("{} is accruing cost while unresolved.", candidate.title),
                delivery_available_at,
                kind: EventKind::CostSet,
            });
            let elapsed_days = as_of.signed_duration_since(set_at).num_days();
            if elapsed_days >= 7 {
                let local_week = as_of.with_timezone(&candidate.settings.timezone).iso_week();
                let week_key = format!("{}-W{:02}", local_week.year(), local_week.week());
                let week_start = resolve_local(
                    candidate.settings.timezone,
                    NaiveDate::from_isoywd_opt(local_week.year(), local_week.week(), Weekday::Mon)
                        .expect("an ISO week returned by chrono is valid")
                        .and_time(NaiveTime::MIN),
                )?;
                events.push(GuardEvent {
                    event_key: format!("task-cost:{}:week:{week_key}", candidate.task_id),
                    scheduled_at: week_start.max(set_at + chrono::Duration::weeks(1)),
                    expires_at: week_start.max(set_at + chrono::Duration::weeks(1))
                        + chrono::Duration::days(NOTIFICATION_TTL_DAYS),
                    title: "Task is still accruing cost".to_owned(),
                    body: format!("{} is still accruing cost.", candidate.title),
                    delivery_available_at,
                    kind: EventKind::CostWeekly,
                });
            }
        }
    }
    Ok(events)
}

fn field_source<'a>(candidate: &'a GuardCandidate, field: &str) -> Option<&'a str> {
    candidate
        .provenance
        .get(field)
        .and_then(Value::as_str)
        .or_else(|| field_cell(candidate, field)?.get("source")?.as_str())
}

fn field_set_at(candidate: &GuardCandidate, field: &str) -> Option<DateTime<Utc>> {
    candidate
        .source_timestamps
        .get(field)
        .and_then(Value::as_str)
        .or_else(|| field_cell(candidate, field)?.get("set_at")?.as_str())
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn field_note<'a>(candidate: &'a GuardCandidate, field: &str) -> Option<&'a str> {
    field_cell(candidate, field)?
        .get("note")?
        .as_str()
        .map(str::trim)
}

fn field_cell<'a>(candidate: &'a GuardCandidate, field: &str) -> Option<&'a Value> {
    candidate
        .task
        .get(field)?
        .as_object()
        .map(|_| &candidate.task[field])
}

fn due_day_instant(due: DateTime<Utc>, settings: &GuardSettings) -> ApiResult<DateTime<Utc>> {
    let local_due = due.with_timezone(&settings.timezone);
    resolve_local(
        settings.timezone,
        NaiveDate::from_ymd_opt(local_due.year(), local_due.month(), local_due.day())
            .expect("a chrono date is valid")
            .and_time(settings.due_day_local_time),
    )
}

fn delivery_available_at(
    as_of: DateTime<Utc>,
    settings: &GuardSettings,
    can_break_quiet: bool,
) -> ApiResult<DateTime<Utc>> {
    if can_break_quiet {
        return Ok(as_of);
    }
    delivery_available_at_without_override(
        as_of,
        settings.timezone,
        settings.quiet_hours_start,
        settings.quiet_hours_end,
    )
}

pub(crate) fn delivery_available_at_without_override(
    as_of: DateTime<Utc>,
    timezone: Tz,
    quiet_hours_start: NaiveTime,
    quiet_hours_end: NaiveTime,
) -> ApiResult<DateTime<Utc>> {
    let local = as_of.with_timezone(&timezone);
    let time = local.time();
    let start = quiet_hours_start;
    let end = quiet_hours_end;
    let quiet = if start == end {
        false
    } else if start < end {
        time >= start && time < end
    } else {
        time >= start || time < end
    };
    if !quiet {
        return Ok(as_of);
    }
    let end_date = if start > end && time >= start {
        local.date_naive() + chrono::Duration::days(1)
    } else {
        local.date_naive()
    };
    resolve_local(timezone, end_date.and_time(end))
}

fn resolve_local(timezone: Tz, local: chrono::NaiveDateTime) -> ApiResult<DateTime<Utc>> {
    let resolved = match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => {
            let shifted = local + chrono::Duration::hours(1);
            match timezone.from_local_datetime(&shifted) {
                LocalResult::Single(value) => value,
                LocalResult::Ambiguous(first, second) => first.min(second),
                LocalResult::None => {
                    return Err(ApiError::Internal(
                        "could not resolve task guard local time".to_owned(),
                    ));
                }
            }
        }
    };
    Ok(resolved.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeZone, Utc};
    use chrono_tz::UTC;
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        EventKind, GuardCandidate, GuardSettings, delivery_available_at, events_for_candidate,
        validate_as_of_override,
    };

    fn instant(day: u32, hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn candidate(source: &str) -> GuardCandidate {
        let set_at = instant(20, 12);
        GuardCandidate {
            user_id: Uuid::now_v7(),
            task_id: Uuid::now_v7(),
            title: "Renew service".to_owned(),
            hard_due: Some(instant(27, 12)),
            hard_due_lead_days: None,
            cost_of_delay: false,
            provenance: json!({"hard_due":source}),
            source_timestamps: json!({"hard_due":set_at}),
            task: json!({"hard_due":{"value":instant(27,12),"source":source,"set_at":set_at}}),
            created_at: set_at,
            settings: GuardSettings {
                timezone: UTC,
                hard_lead_days: 7,
                hard_second_lead_hours: 48,
                due_day_local_time: NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
                quiet_hours_start: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
                quiet_hours_end: NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
                quiet_override_enabled: true,
                quiet_override_within_hours: 24,
            },
        }
    }

    #[test]
    fn hard_bands_are_deterministic_and_inferred_is_marked() {
        let task = candidate("agent:codex");
        let at_seven_days = events_for_candidate(&task, instant(20, 12)).unwrap();
        assert_eq!(at_seven_days.len(), 1);
        assert!(at_seven_days[0].event_key.ends_with(":7d"));
        assert!(at_seven_days[0].body.contains("inferred — confirm?"));
        assert_eq!(
            at_seven_days[0].kind,
            EventKind::HardDeadline { inferred: true }
        );

        let at_two_days = events_for_candidate(&task, instant(25, 12)).unwrap();
        assert_eq!(at_two_days.len(), 2);
        assert!(
            at_two_days
                .iter()
                .any(|event| event.event_key.ends_with(":48h"))
        );

        let due_day = events_for_candidate(&task, instant(27, 7)).unwrap();
        assert_eq!(due_day.len(), 3);
        assert!(
            due_day
                .iter()
                .any(|event| event.event_key.ends_with(":due-day"))
        );
    }

    #[test]
    fn quiet_hours_delay_inferred_but_owner_inside_window_breaks() {
        let as_of = instant(26, 23);
        let inferred = candidate("agent:codex");
        let delayed = delivery_available_at(as_of, &inferred.settings, false).unwrap();
        assert_eq!(delayed, instant(27, 7));
        let inferred_events = events_for_candidate(&inferred, as_of).unwrap();
        assert!(
            inferred_events
                .iter()
                .all(|event| event.delivery_available_at == instant(27, 7))
        );

        let owner = candidate("owner");
        let owner_events = events_for_candidate(&owner, as_of).unwrap();
        assert!(
            owner_events
                .iter()
                .all(|event| event.delivery_available_at == as_of)
        );
    }

    #[test]
    fn only_explicit_todoist_deadline_note_can_break_quiet() {
        let as_of = instant(26, 23);
        let mut due_date_only = candidate("todoist");
        let events = events_for_candidate(&due_date_only, as_of).unwrap();
        assert!(
            events
                .iter()
                .all(|event| event.delivery_available_at > as_of)
        );
        assert!(
            events
                .iter()
                .all(|event| matches!(event.kind, EventKind::HardDeadline { inferred: true }))
        );

        due_date_only.task["hard_due"]["note"] = json!("todoist_deadline");
        let events = events_for_candidate(&due_date_only, as_of).unwrap();
        assert!(
            events
                .iter()
                .all(|event| event.delivery_available_at == as_of)
        );
    }

    #[test]
    fn cost_events_publish_once_when_set_and_one_key_per_current_week() {
        let mut task = candidate("owner");
        task.hard_due = None;
        task.cost_of_delay = true;
        task.provenance = json!({"cost_of_delay":"agent:codex"});
        task.source_timestamps = json!({"cost_of_delay":instant(20,12)});
        task.task = json!({
            "cost_of_delay": {
                "value":{"flag":true,"since":"2026-08-20"},
                "source":"agent:codex",
                "set_at":instant(20,12)
            }
        });
        let events = events_for_candidate(&task, instant(27, 12)).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|event| event.event_key.ends_with(":set")));
        assert!(
            events
                .iter()
                .any(|event| event.event_key.ends_with(":week:2026-W35"))
        );
    }

    #[test]
    fn cost_week_keys_follow_local_iso_week_across_calendar_years() {
        let mut task = candidate("owner");
        task.hard_due = None;
        task.cost_of_delay = true;
        task.source_timestamps = json!({"cost_of_delay":"2026-12-20T12:00:00Z"});
        task.created_at = Utc
            .with_ymd_and_hms(2026, 12, 20, 12, 0, 0)
            .single()
            .unwrap();
        let jan_first = Utc.with_ymd_and_hms(2027, 1, 1, 12, 0, 0).single().unwrap();
        let events = events_for_candidate(&task, jan_first).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_key.ends_with(":week:2026-W53"))
        );
        let jan_fourth = Utc.with_ymd_and_hms(2027, 1, 4, 12, 0, 0).single().unwrap();
        let events = events_for_candidate(&task, jan_fourth).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_key.ends_with(":week:2027-W01"))
        );
    }

    #[test]
    fn production_rejects_as_of_override() {
        assert!(validate_as_of_override("production", Some(instant(20, 12))).is_err());
        assert_eq!(
            validate_as_of_override("development", Some(instant(20, 12))).unwrap(),
            instant(20, 12)
        );
    }
}

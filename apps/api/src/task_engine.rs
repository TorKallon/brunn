use std::cmp::Ordering;
use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: String,
    pub set_at: DateTime<Utc>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostPeriod {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CostOfDelay {
    Rate {
        amount_cents: i64,
        per: CostPeriod,
        since: NaiveDate,
        note: Option<String>,
    },
    Flag {
        since: NaiveDate,
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectInterest {
    Hot,
    Normal,
    Parked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Open,
    Waiting,
    Done,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskView {
    Urgent,
    Next,
    Triage,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub id: Uuid,
    pub title: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub ready_at: Option<Sourced<DateTime<Utc>>>,
    pub soft_due: Option<Sourced<NaiveDate>>,
    pub hard_due: Option<Sourced<DateTime<Utc>>>,
    pub hard_due_lead_days: Option<Sourced<i64>>,
    pub cost_of_delay: Option<Sourced<CostOfDelay>>,
    pub required_contexts: Option<Sourced<Vec<String>>>,
    pub project: Option<Sourced<String>>,
    pub project_interest: ProjectInterest,
    pub project_last_activity: Option<DateTime<Utc>>,
    pub parked: bool,
    pub waiting: bool,
    pub today_pin: Option<Sourced<NaiveDate>>,
    pub triaged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRequest {
    pub view: TaskView,
    pub limit: usize,
    pub contexts_available: BTreeSet<String>,
    pub include_waiting: bool,
    pub include_parked: bool,
    pub as_of: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineSettings {
    pub hard_due_lead_days: i64,
    pub soft_due_window_days: i64,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            hard_due_lead_days: 7,
            soft_due_window_days: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedTask {
    pub id: Uuid,
    pub title: String,
    pub tier: u8,
    pub reason: String,
    pub provenance_markers: Vec<String>,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedTasks {
    pub items: Vec<RankedTask>,
    pub urgent_total: usize,
    pub next_remaining: usize,
    pub backlog_total: usize,
}

const MAX_NEXT_LIMIT: usize = 25;
const MAX_TRIAGE_LIMIT: usize = 10;

pub fn rank_tasks(
    tasks: &[TaskSnapshot],
    request: &CandidateRequest,
    settings: &EngineSettings,
) -> RankedTasks {
    let backlog_total = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Open | TaskStatus::Waiting))
        .count();

    let mut evaluated = tasks
        .iter()
        .filter(|task| is_visible(task, request))
        .map(|task| evaluate(task, request.as_of, settings))
        .collect::<Vec<_>>();

    evaluated.sort_by(compare_ranked);

    let urgent_total = evaluated.iter().filter(|item| item.tier <= 2).count();
    let next_limit = request.limit.min(MAX_NEXT_LIMIT);
    let next_remaining = evaluated.len().saturating_sub(next_limit);

    let selected: Vec<EvaluatedTask<'_>> = match request.view {
        TaskView::Urgent => evaluated
            .into_iter()
            .filter(|item| item.tier <= 2)
            .collect(),
        TaskView::Next => evaluated.into_iter().take(next_limit).collect(),
        TaskView::Triage => {
            let mut triage = evaluated
                .into_iter()
                .filter(|item| item.task.triaged_at.is_none())
                .collect::<Vec<_>>();
            triage.sort_by(|left, right| stable_task_order(left.task, right.task));
            triage
                .into_iter()
                .take(request.limit.min(MAX_TRIAGE_LIMIT))
                .collect()
        }
        TaskView::All => evaluated.into_iter().take(next_limit).collect(),
    };

    RankedTasks {
        items: selected
            .into_iter()
            .map(EvaluatedTask::into_public)
            .collect(),
        urgent_total,
        next_remaining,
        backlog_total,
    }
}

/// Return the complete deterministic order for explicit pagination. This is
/// intentionally separate from bounded surfaces: callers must slice the
/// result before rendering, while sharing the exact visibility, pressure, and
/// comparator implementation used by [`rank_tasks`].
pub fn rank_all_tasks(
    tasks: &[TaskSnapshot],
    request: &CandidateRequest,
    settings: &EngineSettings,
) -> RankedTasks {
    let backlog_total = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Open | TaskStatus::Waiting))
        .count();
    let mut evaluated = tasks
        .iter()
        .filter(|task| is_visible(task, request))
        .map(|task| evaluate(task, request.as_of, settings))
        .collect::<Vec<_>>();
    evaluated.sort_by(compare_ranked);
    let urgent_total = evaluated.iter().filter(|item| item.tier <= 2).count();
    let next_remaining = evaluated
        .len()
        .saturating_sub(request.limit.min(MAX_NEXT_LIMIT));
    RankedTasks {
        items: evaluated
            .into_iter()
            .map(EvaluatedTask::into_public)
            .collect(),
        urgent_total,
        next_remaining,
        backlog_total,
    }
}

pub fn snooze_transition(current_count: u32) -> (u32, bool) {
    let next_count = current_count.saturating_add(1);
    (next_count, next_count >= 3)
}

fn is_visible(task: &TaskSnapshot, request: &CandidateRequest) -> bool {
    if !matches!(task.status, TaskStatus::Open | TaskStatus::Waiting) {
        return false;
    }

    let waiting = task.waiting || task.status == TaskStatus::Waiting;
    if waiting && !request.include_waiting {
        return false;
    }
    if task.parked && !request.include_parked {
        return false;
    }
    if task
        .ready_at
        .as_ref()
        .is_some_and(|ready_at| ready_at.value > request.as_of)
    {
        return false;
    }

    task.required_contexts.as_ref().is_none_or(|required| {
        required
            .value
            .iter()
            .all(|context| request.contexts_available.contains(context))
    })
}

struct EvaluatedTask<'a> {
    task: &'a TaskSnapshot,
    tier: u8,
    reason: String,
    provenance_markers: Vec<String>,
    pinned: bool,
    order: TierOrder,
}

impl EvaluatedTask<'_> {
    fn into_public(self) -> RankedTask {
        RankedTask {
            id: self.task.id,
            title: self.task.title.clone(),
            tier: self.tier,
            reason: self.reason,
            provenance_markers: self.provenance_markers,
            pinned: self.pinned,
        }
    }
}

enum TierOrder {
    Hard {
        due: DateTime<Utc>,
    },
    NumericCost {
        daily_numerator: i128,
        daily_denominator: i128,
        accrued_numerator: i128,
    },
    FlagCost {
        since: NaiveDate,
    },
    Soft {
        due: NaiveDate,
    },
    Hot {
        last_activity: Option<DateTime<Utc>>,
    },
    Fallback,
}

fn evaluate<'a>(
    task: &'a TaskSnapshot,
    as_of: DateTime<Utc>,
    settings: &EngineSettings,
) -> EvaluatedTask<'a> {
    let pinned = task
        .today_pin
        .as_ref()
        .is_some_and(|pin| pin.value == as_of.date_naive());

    let (tier, reason, mut provenance_markers, order) = pressure(task, as_of, settings);

    if pinned && let Some(pin) = task.today_pin.as_ref() {
        add_marker(&mut provenance_markers, &pin.source);
    }

    EvaluatedTask {
        task,
        tier,
        reason,
        provenance_markers,
        pinned,
        order,
    }
}

fn pressure(
    task: &TaskSnapshot,
    as_of: DateTime<Utc>,
    settings: &EngineSettings,
) -> (u8, String, Vec<String>, TierOrder) {
    if let Some(hard_due) = task.hard_due.as_ref() {
        let lead_days = task
            .hard_due_lead_days
            .as_ref()
            .map_or(settings.hard_due_lead_days, |lead| lead.value)
            .max(0);
        let seconds_remaining = hard_due.value.signed_duration_since(as_of).num_seconds();
        if i128::from(seconds_remaining) <= i128::from(lead_days) * 86_400 {
            let mut markers = Vec::new();
            add_marker(&mut markers, &hard_due.source);
            if let Some(lead) = task.hard_due_lead_days.as_ref() {
                add_marker(&mut markers, &lead.source);
            }
            let reason = hard_reason(hard_due.value, as_of, !markers.is_empty());
            return (
                1,
                reason,
                markers,
                TierOrder::Hard {
                    due: hard_due.value,
                },
            );
        }
    }

    if let Some(cost) = task.cost_of_delay.as_ref() {
        match &cost.value {
            CostOfDelay::Rate {
                amount_cents,
                per,
                since,
                ..
            } if *since <= as_of.date_naive() => {
                let denominator = period_days(*per);
                let daily_numerator = i128::from(*amount_cents);
                let elapsed_days = i128::from(
                    as_of
                        .date_naive()
                        .signed_duration_since(*since)
                        .num_days()
                        .max(0),
                );
                let accrued_numerator = daily_numerator * elapsed_days;
                let inferred = cost.source != "owner";
                let marker = if inferred { " (est.)" } else { "" };
                let reason = format!(
                    "~{}/day{} since {}, ~{} so far",
                    format_dollars(daily_numerator, denominator),
                    marker,
                    format_date(*since),
                    format_dollars(accrued_numerator, denominator),
                );
                let mut markers = Vec::new();
                add_marker(&mut markers, &cost.source);
                return (
                    2,
                    reason,
                    markers,
                    TierOrder::NumericCost {
                        daily_numerator,
                        daily_denominator: denominator,
                        accrued_numerator,
                    },
                );
            }
            CostOfDelay::Flag { since, .. } if *since <= as_of.date_naive() => {
                let inferred = cost.source != "owner";
                let marker = if inferred { " (est.)" } else { "" };
                let reason = format!("costing money{} since {}", marker, format_date(*since));
                let mut markers = Vec::new();
                add_marker(&mut markers, &cost.source);
                return (2, reason, markers, TierOrder::FlagCost { since: *since });
            }
            CostOfDelay::Rate { .. } | CostOfDelay::Flag { .. } => {}
        }
    }

    if let Some(soft_due) = task.soft_due.as_ref() {
        let days_remaining = soft_due
            .value
            .signed_duration_since(as_of.date_naive())
            .num_days();
        if days_remaining <= settings.soft_due_window_days.max(0) {
            let mut markers = Vec::new();
            add_marker(&mut markers, &soft_due.source);
            let marker = if markers.is_empty() { "" } else { " (est.)" };
            let reason = if days_remaining < 0 {
                format!(
                    "should do by {}{} (overdue by {})",
                    soft_due.value.format("%a"),
                    marker,
                    day_count(-days_remaining),
                )
            } else {
                format!("should do by {}{}", soft_due.value.format("%a"), marker)
            };
            return (
                3,
                reason,
                markers,
                TierOrder::Soft {
                    due: soft_due.value,
                },
            );
        }
    }

    if task.project_interest == ProjectInterest::Hot {
        let mut markers = Vec::new();
        if let Some(project) = task.project.as_ref() {
            add_marker(&mut markers, &project.source);
        }
        let marker = if markers.is_empty() { "" } else { " (est.)" };
        return (
            4,
            format!("active project{marker}"),
            markers,
            TierOrder::Hot {
                last_activity: task.project_last_activity,
            },
        );
    }

    (
        5,
        format!("ready since {}", format_date(task.created_at.date_naive())),
        Vec::new(),
        TierOrder::Fallback,
    )
}

fn compare_ranked(left: &EvaluatedTask<'_>, right: &EvaluatedTask<'_>) -> Ordering {
    right
        .pinned
        .cmp(&left.pinned)
        .then_with(|| left.tier.cmp(&right.tier))
        .then_with(|| compare_tier_order(&left.order, &right.order))
        .then_with(|| stable_task_order(left.task, right.task))
}

fn compare_tier_order(left: &TierOrder, right: &TierOrder) -> Ordering {
    match (left, right) {
        (TierOrder::Hard { due: left }, TierOrder::Hard { due: right }) => left.cmp(right),
        (
            TierOrder::NumericCost {
                daily_numerator: left_daily,
                daily_denominator: left_denominator,
                accrued_numerator: left_accrued,
            },
            TierOrder::NumericCost {
                daily_numerator: right_daily,
                daily_denominator: right_denominator,
                accrued_numerator: right_accrued,
            },
        ) => (right_daily * left_denominator)
            .cmp(&(left_daily * right_denominator))
            .then_with(|| {
                (right_accrued * left_denominator).cmp(&(left_accrued * right_denominator))
            }),
        (TierOrder::NumericCost { .. }, TierOrder::FlagCost { .. }) => Ordering::Less,
        (TierOrder::FlagCost { .. }, TierOrder::NumericCost { .. }) => Ordering::Greater,
        (TierOrder::FlagCost { since: left }, TierOrder::FlagCost { since: right }) => {
            left.cmp(right)
        }
        (TierOrder::Soft { due: left }, TierOrder::Soft { due: right }) => left.cmp(right),
        (
            TierOrder::Hot {
                last_activity: left,
            },
            TierOrder::Hot {
                last_activity: right,
            },
        ) => compare_optional_recency(*left, *right),
        (TierOrder::Fallback, TierOrder::Fallback) => Ordering::Equal,
        _ => Ordering::Equal,
    }
}

fn compare_optional_recency(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn stable_task_order(left: &TaskSnapshot, right: &TaskSnapshot) -> Ordering {
    left.created_at
        .cmp(&right.created_at)
        .then_with(|| left.id.cmp(&right.id))
}

fn hard_reason(due: DateTime<Utc>, as_of: DateTime<Utc>, inferred: bool) -> String {
    let marker = if inferred { " (est.)" } else { "" };
    let seconds_remaining = due.signed_duration_since(as_of).num_seconds();
    if seconds_remaining < 0 {
        let overdue_days = (i128::from(-seconds_remaining) + 86_399) / 86_400;
        format!(
            "hard deadline overdue by {}{marker}",
            day_count(overdue_days)
        )
    } else if seconds_remaining == 0 {
        format!("hard deadline due now{marker}")
    } else {
        let days_remaining = (i128::from(seconds_remaining) + 86_399) / 86_400;
        format!("hard deadline in {}{marker}", day_count(days_remaining))
    }
}

fn day_count(days: impl Into<i128>) -> String {
    let days = days.into();
    if days == 1 {
        "1 day".to_owned()
    } else {
        format!("{days} days")
    }
}

fn period_days(period: CostPeriod) -> i128 {
    match period {
        CostPeriod::Day => 1,
        CostPeriod::Week => 7,
        CostPeriod::Month => 30,
    }
}

fn format_dollars(numerator_cents: i128, denominator: i128) -> String {
    let negative = numerator_cents < 0;
    let absolute = numerator_cents.abs();
    let rounded_cents = (absolute + denominator / 2) / denominator;
    let dollars = rounded_cents / 100;
    let cents = rounded_cents % 100;
    let sign = if negative { "-" } else { "" };
    if cents == 0 {
        format!("${sign}{dollars}")
    } else {
        format!("${sign}{dollars}.{cents:02}")
    }
}

fn format_date(date: NaiveDate) -> String {
    date.format("%b %-d").to_string()
}

fn add_marker(markers: &mut Vec<String>, source: &str) {
    if source != "owner" && !markers.iter().any(|marker| marker == source) {
        markers.push(source.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    fn instant(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap()
    }

    fn source<T>(value: T, source: &str) -> Sourced<T> {
        Sourced {
            value,
            source: source.to_owned(),
            set_at: instant(1, 0),
            note: None,
        }
    }

    fn task(id: u128, title: &str, created_day: u32) -> TaskSnapshot {
        TaskSnapshot {
            id: Uuid::from_u128(id),
            title: title.to_owned(),
            status: TaskStatus::Open,
            created_at: instant(created_day, 0),
            ready_at: None,
            soft_due: None,
            hard_due: None,
            hard_due_lead_days: None,
            cost_of_delay: None,
            required_contexts: None,
            project: None,
            project_interest: ProjectInterest::Normal,
            project_last_activity: None,
            parked: false,
            waiting: false,
            today_pin: None,
            triaged_at: Some(instant(1, 0)),
        }
    }

    fn request(view: TaskView, as_of: DateTime<Utc>, limit: usize) -> CandidateRequest {
        CandidateRequest {
            view,
            limit,
            contexts_available: BTreeSet::new(),
            include_waiting: false,
            include_parked: false,
            as_of,
        }
    }

    fn ranked_one(
        task: TaskSnapshot,
        as_of: DateTime<Utc>,
        settings: &EngineSettings,
    ) -> RankedTask {
        rank_tasks(&[task], &request(TaskView::Next, as_of, 5), settings)
            .items
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn cost_periods_normalize_and_equal_rates_use_accrued_total() {
        let as_of = instant(27, 12);
        let mut daily = task(1, "daily", 1);
        daily.cost_of_delay = Some(source(
            CostOfDelay::Rate {
                amount_cents: 1_000,
                per: CostPeriod::Day,
                since: date(26),
                note: None,
            },
            "owner",
        ));
        let mut weekly = task(2, "weekly", 1);
        weekly.cost_of_delay = Some(source(
            CostOfDelay::Rate {
                amount_cents: 7_000,
                per: CostPeriod::Week,
                since: date(20),
                note: None,
            },
            "owner",
        ));
        let mut monthly = task(3, "monthly", 1);
        monthly.cost_of_delay = Some(source(
            CostOfDelay::Rate {
                amount_cents: 30_000,
                per: CostPeriod::Month,
                since: date(1),
                note: None,
            },
            "owner",
        ));

        let ranked = rank_tasks(
            &[daily, weekly, monthly],
            &request(TaskView::Next, as_of, 5),
            &EngineSettings::default(),
        );
        assert_eq!(
            ranked
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["monthly", "weekly", "daily"]
        );
        assert!(
            ranked
                .items
                .iter()
                .all(|item| item.reason.contains("~$10/day"))
        );
        assert!(ranked.items[0].reason.contains("~$260 so far"));
    }

    #[test]
    fn full_order_is_single_sort_authority_for_deliberate_pagination() {
        let as_of = instant(27, 12);
        let tasks = (1..=60)
            .map(|id| task(id, &format!("task-{id}"), 1))
            .collect::<Vec<_>>();
        let request = request(TaskView::All, as_of, 25);
        let bounded = rank_tasks(&tasks, &request, &EngineSettings::default());
        let full = rank_all_tasks(&tasks, &request, &EngineSettings::default());
        assert_eq!(bounded.items.len(), 25);
        assert_eq!(full.items.len(), 60);
        assert_eq!(bounded.items, full.items[..25]);
    }

    #[test]
    fn numeric_costs_precede_flags_and_flags_order_by_age() {
        let as_of = instant(27, 12);
        let mut numeric = task(1, "numeric", 3);
        numeric.cost_of_delay = Some(source(
            CostOfDelay::Rate {
                amount_cents: 1,
                per: CostPeriod::Month,
                since: date(27),
                note: None,
            },
            "owner",
        ));
        let mut old_flag = task(2, "old-flag", 2);
        old_flag.cost_of_delay = Some(source(
            CostOfDelay::Flag {
                since: date(1),
                note: None,
            },
            "owner",
        ));
        let mut new_flag = task(3, "new-flag", 1);
        new_flag.cost_of_delay = Some(source(
            CostOfDelay::Flag {
                since: date(20),
                note: None,
            },
            "owner",
        ));

        let ranked = rank_tasks(
            &[new_flag, old_flag, numeric],
            &request(TaskView::Next, as_of, 5),
            &EngineSettings::default(),
        );
        assert_eq!(
            ranked
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["numeric", "old-flag", "new-flag"]
        );
    }

    #[test]
    fn governing_non_owner_sources_mark_reasons_and_provenance() {
        let as_of = instant(27, 12);
        let mut hard = task(1, "hard", 1);
        hard.hard_due = Some(source(instant(29, 12), "agent:codex"));
        let hard = ranked_one(hard, as_of, &EngineSettings::default());
        assert_eq!(hard.reason, "hard deadline in 2 days (est.)");
        assert_eq!(hard.provenance_markers, ["agent:codex"]);

        let mut cost = task(2, "cost", 1);
        cost.cost_of_delay = Some(source(
            CostOfDelay::Rate {
                amount_cents: 1_200,
                per: CostPeriod::Day,
                since: date(20),
                note: None,
            },
            "derived",
        ));
        let cost = ranked_one(cost, as_of, &EngineSettings::default());
        assert!(cost.reason.contains("~$12/day (est.)"));
        assert_eq!(cost.provenance_markers, ["derived"]);

        let mut soft = task(3, "soft", 1);
        soft.soft_due = Some(source(date(28), "todoist"));
        let soft = ranked_one(soft, as_of, &EngineSettings::default());
        assert_eq!(soft.reason, "should do by Fri (est.)");
        assert_eq!(soft.provenance_markers, ["todoist"]);

        let mut owner_hard_with_inferred_lead = task(4, "lead", 1);
        owner_hard_with_inferred_lead.hard_due = Some(source(instant(29, 12), "owner"));
        owner_hard_with_inferred_lead.hard_due_lead_days = Some(source(3, "agent:codex"));
        let lead = ranked_one(
            owner_hard_with_inferred_lead,
            as_of,
            &EngineSettings::default(),
        );
        assert!(lead.reason.contains("est."));
        assert_eq!(lead.provenance_markers, ["agent:codex"]);

        let mut owner = task(5, "owner", 1);
        owner.hard_due = Some(source(instant(29, 12), "owner"));
        let owner = ranked_one(owner, as_of, &EngineSettings::default());
        assert!(!owner.reason.contains("est."));
        assert!(owner.provenance_markers.is_empty());
    }

    #[test]
    fn hard_and_soft_windows_are_inclusive_and_configurable() {
        let as_of = instant(20, 12);
        let defaults = EngineSettings::default();

        let mut hard_boundary = task(1, "hard-boundary", 1);
        hard_boundary.hard_due = Some(source(as_of + Duration::days(7), "owner"));
        assert_eq!(ranked_one(hard_boundary, as_of, &defaults).tier, 1);

        let mut hard_outside = task(2, "hard-outside", 1);
        hard_outside.hard_due = Some(source(
            as_of + Duration::days(7) + Duration::seconds(1),
            "owner",
        ));
        assert_eq!(ranked_one(hard_outside, as_of, &defaults).tier, 5);

        let mut hard_override = task(3, "hard-override", 1);
        hard_override.hard_due = Some(source(as_of + Duration::days(8), "owner"));
        hard_override.hard_due_lead_days = Some(source(8, "owner"));
        assert_eq!(ranked_one(hard_override, as_of, &defaults).tier, 1);

        let mut soft_boundary = task(4, "soft-boundary", 1);
        soft_boundary.soft_due = Some(source(as_of.date_naive() + Duration::days(3), "owner"));
        assert_eq!(ranked_one(soft_boundary, as_of, &defaults).tier, 3);

        let mut soft_outside = task(5, "soft-outside", 1);
        soft_outside.soft_due = Some(source(as_of.date_naive() + Duration::days(4), "owner"));
        assert_eq!(ranked_one(soft_outside, as_of, &defaults).tier, 5);

        let custom = EngineSettings {
            hard_due_lead_days: 2,
            soft_due_window_days: 1,
        };
        let mut custom_hard = task(6, "custom-hard", 1);
        custom_hard.hard_due = Some(source(as_of + Duration::days(3), "owner"));
        assert_eq!(ranked_one(custom_hard, as_of, &custom).tier, 5);
        let mut custom_soft = task(7, "custom-soft", 1);
        custom_soft.soft_due = Some(source(as_of.date_naive() + Duration::days(2), "owner"));
        assert_eq!(ranked_one(custom_soft, as_of, &custom).tier, 5);
    }

    #[test]
    fn overdue_tasks_stay_in_pressure_tiers_and_order_oldest_due_first() {
        let as_of = instant(27, 12);
        let mut older_hard = task(1, "older-hard", 5);
        older_hard.hard_due = Some(source(instant(25, 12), "owner"));
        let mut newer_hard = task(2, "newer-hard", 1);
        newer_hard.hard_due = Some(source(instant(26, 12), "owner"));
        let mut soft = task(3, "soft", 1);
        soft.soft_due = Some(source(date(25), "owner"));

        let ranked = rank_tasks(
            &[newer_hard, soft, older_hard],
            &request(TaskView::Next, as_of, 5),
            &EngineSettings::default(),
        );
        assert_eq!(
            ranked
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["older-hard", "newer-hard", "soft"]
        );
        assert_eq!(ranked.items[0].reason, "hard deadline overdue by 2 days");
        assert!(ranked.items[2].reason.contains("overdue by 2 days"));
    }

    #[test]
    fn contexts_are_and_constraints_and_empty_requirements_match() {
        let as_of = instant(27, 12);
        let anywhere = task(1, "anywhere", 1);
        let mut explicit_empty = task(2, "empty", 2);
        explicit_empty.required_contexts = Some(source(Vec::new(), "owner"));
        let mut both = task(3, "both", 3);
        both.required_contexts = Some(source(
            vec!["phone".to_owned(), "online".to_owned()],
            "owner",
        ));

        let mut phone_only = request(TaskView::Next, as_of, 5);
        phone_only.contexts_available.insert("phone".to_owned());
        let blocked = rank_tasks(
            &[anywhere.clone(), explicit_empty.clone(), both.clone()],
            &phone_only,
            &EngineSettings::default(),
        );
        assert_eq!(
            blocked
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["anywhere", "empty"]
        );

        phone_only.contexts_available.insert("online".to_owned());
        let matched = rank_tasks(
            &[anywhere, explicit_empty, both],
            &phone_only,
            &EngineSettings::default(),
        );
        assert_eq!(matched.items.len(), 3);
    }

    #[test]
    fn waiting_and_parked_flags_require_their_independent_includes() {
        let as_of = instant(27, 12);
        let open = task(1, "open", 1);
        let mut waiting_status = task(2, "waiting-status", 2);
        waiting_status.status = TaskStatus::Waiting;
        let mut waiting_flag = task(3, "waiting-flag", 3);
        waiting_flag.waiting = true;
        let mut parked = task(4, "parked", 4);
        parked.parked = true;
        let mut both = task(5, "both", 5);
        both.status = TaskStatus::Waiting;
        both.parked = true;
        let tasks = [open, waiting_status, waiting_flag, parked, both];

        let base = rank_tasks(
            &tasks,
            &request(TaskView::Next, as_of, 10),
            &EngineSettings::default(),
        );
        assert_eq!(base.items.len(), 1);
        assert_eq!(base.backlog_total, 5);

        let mut waiting = request(TaskView::Next, as_of, 10);
        waiting.include_waiting = true;
        assert_eq!(
            rank_tasks(&tasks, &waiting, &EngineSettings::default())
                .items
                .len(),
            3
        );

        let mut parked_request = request(TaskView::Next, as_of, 10);
        parked_request.include_parked = true;
        assert_eq!(
            rank_tasks(&tasks, &parked_request, &EngineSettings::default())
                .items
                .len(),
            2
        );

        waiting.include_parked = true;
        assert_eq!(
            rank_tasks(&tasks, &waiting, &EngineSettings::default())
                .items
                .len(),
            5
        );
    }

    #[test]
    fn only_the_pin_for_as_of_precedes_pressure_tiers() {
        let as_of = instant(27, 12);
        let mut current_pin = task(1, "current", 3);
        current_pin.today_pin = Some(source(date(27), "owner"));
        let mut next_pin = task(2, "next", 2);
        next_pin.today_pin = Some(source(date(28), "owner"));
        let mut urgent = task(3, "urgent", 1);
        urgent.hard_due = Some(source(instant(28, 12), "owner"));

        let today = rank_tasks(
            &[urgent.clone(), next_pin.clone(), current_pin],
            &request(TaskView::Next, as_of, 5),
            &EngineSettings::default(),
        );
        assert_eq!(today.items[0].title, "current");
        assert!(today.items[0].pinned);
        assert!(!today.items[2].pinned);

        let tomorrow = rank_tasks(
            &[urgent, next_pin],
            &request(TaskView::Next, instant(28, 12), 5),
            &EngineSettings::default(),
        );
        assert_eq!(tomorrow.items[0].title, "next");
        assert!(tomorrow.items[0].pinned);
    }

    #[test]
    fn done_and_dropped_are_never_candidates_or_backlog() {
        let as_of = instant(27, 12);
        let open = task(1, "open", 1);
        let mut waiting = task(2, "waiting", 2);
        waiting.status = TaskStatus::Waiting;
        let mut done = task(3, "done", 3);
        done.status = TaskStatus::Done;
        let mut dropped = task(4, "dropped", 4);
        dropped.status = TaskStatus::Dropped;

        let mut candidate_request = request(TaskView::Next, as_of, 10);
        candidate_request.include_waiting = true;
        candidate_request.include_parked = true;
        let ranked = rank_tasks(
            &[dropped, done, waiting, open],
            &candidate_request,
            &EngineSettings::default(),
        );
        assert_eq!(
            ranked
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["open", "waiting"]
        );
        assert_eq!(ranked.backlog_total, 2);
    }

    #[test]
    fn ties_break_by_creation_then_uuid_independent_of_input_order() {
        let as_of = instant(27, 12);
        let low_id = task(1, "low-id", 1);
        let high_id = task(2, "high-id", 1);
        let later = task(0, "later", 2);

        for tasks in [
            vec![later.clone(), high_id.clone(), low_id.clone()],
            vec![low_id.clone(), later.clone(), high_id.clone()],
        ] {
            let ranked = rank_tasks(
                &tasks,
                &request(TaskView::Next, as_of, 5),
                &EngineSettings::default(),
            );
            assert_eq!(
                ranked
                    .items
                    .iter()
                    .map(|item| item.title.as_str())
                    .collect::<Vec<_>>(),
                ["low-id", "high-id", "later"]
            );
        }
    }

    #[test]
    fn hot_projects_use_most_recent_activity_then_task_age() {
        let as_of = instant(27, 12);
        let mut recent = task(1, "recent", 3);
        recent.project_interest = ProjectInterest::Hot;
        recent.project_last_activity = Some(instant(27, 11));
        let mut older = task(2, "older", 1);
        older.project_interest = ProjectInterest::Hot;
        older.project_last_activity = Some(instant(26, 11));
        let mut unknown = task(3, "unknown", 1);
        unknown.project_interest = ProjectInterest::Hot;

        let ranked = rank_tasks(
            &[unknown, older, recent],
            &request(TaskView::Next, as_of, 5),
            &EngineSettings::default(),
        );
        assert_eq!(
            ranked
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["recent", "older", "unknown"]
        );
    }

    #[test]
    fn view_limits_are_defensive_but_urgent_remains_complete() {
        let as_of = instant(27, 12);
        let ready = (1..=30)
            .map(|id| task(id, &format!("ready-{id}"), 1))
            .collect::<Vec<_>>();
        let next = rank_tasks(
            &ready,
            &request(TaskView::Next, as_of, usize::MAX),
            &EngineSettings::default(),
        );
        assert_eq!(next.items.len(), 25);
        assert_eq!(next.next_remaining, 5);
        assert_eq!(next.backlog_total, 30);

        let all = rank_tasks(
            &ready,
            &request(TaskView::All, as_of, usize::MAX),
            &EngineSettings::default(),
        );
        assert_eq!(all.items.len(), 25);

        let mut untriaged = ready.clone();
        for task in &mut untriaged {
            task.triaged_at = None;
        }
        let triage = rank_tasks(
            &untriaged,
            &request(TaskView::Triage, as_of, usize::MAX),
            &EngineSettings::default(),
        );
        assert_eq!(triage.items.len(), 10);

        let urgent_tasks = (1..=30)
            .map(|id| {
                let mut task = task(id, &format!("urgent-{id}"), 1);
                task.cost_of_delay = Some(source(
                    CostOfDelay::Flag {
                        since: date(1),
                        note: None,
                    },
                    "owner",
                ));
                task
            })
            .collect::<Vec<_>>();
        let urgent = rank_tasks(
            &urgent_tasks,
            &request(TaskView::Urgent, as_of, 1),
            &EngineSettings::default(),
        );
        assert_eq!(urgent.items.len(), 30);
        assert_eq!(urgent.urgent_total, 30);
        assert_eq!(urgent.next_remaining, 29);
    }

    #[test]
    fn snooze_count_saturates_and_never_unparks_after_three() {
        assert_eq!(snooze_transition(2), (3, true));
        assert_eq!(snooze_transition(u32::MAX), (u32::MAX, true));
    }
}

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use uuid::Uuid;

use brunn::task_engine::{
    CandidateRequest, CostOfDelay, CostPeriod, EngineSettings, ProjectInterest, Sourced,
    TaskSnapshot, TaskStatus, TaskView, rank_tasks, snooze_transition,
};

fn instant(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
        .single()
        .unwrap()
}

fn source<T>(value: T, source: &str) -> Sourced<T> {
    Sourced {
        value,
        source: source.to_owned(),
        set_at: instant(1, 0),
        note: None,
    }
}

fn task(title: &str, created_day: u32) -> TaskSnapshot {
    TaskSnapshot {
        id: Uuid::now_v7(),
        title: title.to_owned(),
        status: TaskStatus::Open,
        status_source: "owner".to_owned(),
        created_at: instant(created_day, 0),
        done_at: None,
        dropped_at: None,
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

fn request(as_of: DateTime<Utc>) -> CandidateRequest {
    CandidateRequest {
        view: TaskView::Next,
        limit: 5,
        contexts_available: BTreeSet::new(),
        include_waiting: false,
        include_parked: false,
        as_of,
    }
}

#[test]
fn visibility_enforces_ready_context_and_parked_waiting_rules() {
    let as_of = instant(27, 12);
    let mut visible = task("visible", 1);
    visible.required_contexts = Some(source(
        vec!["phone".to_owned(), "online".to_owned()],
        "owner",
    ));

    let mut future = task("future", 2);
    future.ready_at = Some(source(instant(28, 0), "owner"));
    let mut parked = task("parked", 3);
    parked.parked = true;
    let mut waiting = task("waiting", 4);
    waiting.status = TaskStatus::Waiting;
    waiting.waiting = true;

    let mut req = request(as_of);
    req.contexts_available = ["phone".to_owned(), "online".to_owned()]
        .into_iter()
        .collect();
    let ranked = rank_tasks(
        &[future, parked, waiting, visible],
        &req,
        &EngineSettings::default(),
    );
    assert_eq!(ranked.items.len(), 1);
    assert_eq!(ranked.items[0].title, "visible");
}

#[test]
fn tiers_and_cost_order_are_deterministic() {
    let as_of = instant(27, 12);
    let mut overdue = task("overdue", 10);
    overdue.hard_due = Some(source(instant(26, 12), "owner"));
    let mut hard = task("hard", 9);
    hard.hard_due = Some(source(instant(29, 12), "owner"));
    let mut expensive = task("expensive", 8);
    expensive.cost_of_delay = Some(source(
        CostOfDelay::Rate {
            amount_cents: 1200,
            per: CostPeriod::Day,
            since: NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
            note: None,
        },
        "agent:codex",
    ));
    let mut cheaper = task("cheaper", 7);
    cheaper.cost_of_delay = Some(source(
        CostOfDelay::Rate {
            amount_cents: 2000,
            per: CostPeriod::Week,
            since: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            note: None,
        },
        "owner",
    ));
    let mut flag = task("flag", 6);
    flag.cost_of_delay = Some(source(
        CostOfDelay::Flag {
            since: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            note: None,
        },
        "owner",
    ));
    let mut soft = task("soft", 5);
    soft.soft_due = Some(source(
        NaiveDate::from_ymd_opt(2026, 8, 28).unwrap(),
        "owner",
    ));
    let mut hot = task("hot", 4);
    hot.project_interest = ProjectInterest::Hot;
    hot.project_last_activity = Some(instant(27, 11));
    let fallback = task("fallback", 1);

    let ranked = rank_tasks(
        &[fallback, hot, soft, flag, cheaper, expensive, hard, overdue],
        &CandidateRequest {
            limit: 25,
            ..request(as_of)
        },
        &EngineSettings::default(),
    );
    let titles = ranked
        .items
        .iter()
        .map(|item| item.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        titles,
        [
            "overdue",
            "hard",
            "expensive",
            "cheaper",
            "flag",
            "soft",
            "hot",
            "fallback"
        ]
    );
    assert_eq!(ranked.items[0].tier, 1);
    assert!(ranked.items[0].reason.contains("overdue"));
    assert_eq!(ranked.items[2].tier, 2);
    assert!(ranked.items[2].reason.contains("~$12/day"));
    assert!(ranked.items[2].reason.contains("est."));
}

#[test]
fn urgent_is_tiers_one_and_two_and_empty_is_first_class() {
    let as_of = instant(27, 12);
    let ordinary = task("ordinary", 1);
    let empty = rank_tasks(
        std::slice::from_ref(&ordinary),
        &CandidateRequest {
            view: TaskView::Urgent,
            ..request(as_of)
        },
        &EngineSettings::default(),
    );
    assert!(empty.items.is_empty());
    assert_eq!(empty.urgent_total, 0);
    assert_eq!(empty.backlog_total, 1);

    let mut cost = ordinary;
    cost.cost_of_delay = Some(source(
        CostOfDelay::Flag {
            since: NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
            note: None,
        },
        "owner",
    ));
    let urgent = rank_tasks(
        &[cost],
        &CandidateRequest {
            view: TaskView::Urgent,
            ..request(as_of)
        },
        &EngineSettings::default(),
    );
    assert_eq!(urgent.items.len(), 1);
    assert_eq!(urgent.items[0].tier, 2);
}

#[test]
fn pins_precede_tiers_and_limits_remain_bounded() {
    let as_of = instant(27, 12);
    let mut pinned = task("pinned", 9);
    pinned.today_pin = Some(source(
        NaiveDate::from_ymd_opt(2026, 8, 27).unwrap(),
        "owner",
    ));
    let mut tasks = vec![pinned];
    for day in 1..=9 {
        tasks.push(task(&format!("task-{day}"), day));
    }
    let ranked = rank_tasks(&tasks, &request(as_of), &EngineSettings::default());
    assert_eq!(ranked.items.len(), 5);
    assert_eq!(ranked.items[0].title, "pinned");
    assert!(ranked.items[0].pinned);
    assert_eq!(ranked.next_remaining, 5);
    assert_eq!(ranked.backlog_total, 10);
}

#[test]
fn triage_and_time_travel_are_stable() {
    let mut imported = task("imported", 1);
    imported.triaged_at = None;
    let mut later = task("later", 2);
    later.triaged_at = None;
    later.ready_at = Some(source(instant(28, 0), "todoist"));

    let before = rank_tasks(
        &[imported.clone(), later.clone()],
        &CandidateRequest {
            view: TaskView::Triage,
            limit: 10,
            ..request(instant(27, 12))
        },
        &EngineSettings::default(),
    );
    assert_eq!(
        before
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["imported"]
    );

    let after = rank_tasks(
        &[imported, later],
        &CandidateRequest {
            view: TaskView::Triage,
            limit: 10,
            ..request(instant(29, 12))
        },
        &EngineSettings::default(),
    );
    assert_eq!(
        after
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["imported", "later"]
    );
}

#[test]
fn third_snooze_parks_without_losing_the_task() {
    assert_eq!(snooze_transition(0), (1, false));
    assert_eq!(snooze_transition(1), (2, false));
    assert_eq!(snooze_transition(2), (3, true));
    assert_eq!(snooze_transition(3), (4, true));
}

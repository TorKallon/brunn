//! Incremental custody through real subprocesses and the runner HTTP boundary.
use super::*;

const PROTOCOL: &str = "dream.research.checkpoint.v1";
const NOTEBOOK: &str = "entry:019fba27-687b-7582-8b99-e9371dbe2ce8";
const SECOND: &str = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
const SAVED_NOTE: &str = "A supported checkpoint survives the next invocation.";

fn origin(version: i64) -> Value {
    origin_at(version, 17)
}

fn origin_at(version: i64, snapshot: i64) -> Value {
    json!({"entry_ref":NOTEBOOK,"version":version,"snapshot_generation":snapshot})
}

fn enabled_job(subject: &str) -> Value {
    let mut value = job(subject);
    value["version"] = json!(10);
    value["checkpoint_protocol"] = json!(PROTOCOL);
    value["checkpoint_context_status"] = json!("available");
    value["checkpoint_contexts"] = json!([]);
    value["current_checkpoint"] = Value::Null;
    value
}

fn selector(subject: &str, start: usize, end: usize) -> Value {
    json!({"entry_ref":subject,"version":2,"start_line":start,"end_line":end})
}

fn checkpoint_step(notes: &str, reviewed: Vec<Value>) -> Value {
    json!({"schema":"dream.research.step.v1","action":"checkpoint",
        "notes":notes,"reviewed_sources":reviewed,
        "reconciled_checkpoints":[],"findings":["Saved the supported source review; more work remains."]})
}

fn candidate_reconciling(version: i64) -> Value {
    let mut step = candidate_step();
    step["reconciled_checkpoints"] = json!([origin(version)]);
    step["findings"] = json!([
        "The current working checkpoint and its unfinished leads are incorporated in this supported proposal."
    ]);
    step
}

fn done_step(subject: &str) -> Value {
    json!({"schema":"dream.research.step.v1","action":"done",
        "reviewed_sources":[selector(subject,1,2)],
        "findings":["The exact source review found no useful change."]})
}

fn program(steps: &[(&str, Value)]) -> String {
    let mut script = String::from(
        "if [ \"$OUTPUT_NAME\" = 'probe-answer.md' ]; then echo READY; exit 0; fi\ncase \"$OUTPUT_NAME\" in\n",
    );
    for (name, step) in steps {
        script.push_str(&format!(
            " {name}) cat > \"$OUTPUT_PATH\" <<'CHECKPOINT_JSON'\n{step}\nCHECKPOINT_JSON\n ;;\n"
        ));
    }
    script.push_str(" *) exit 99;;\nesac\n");
    script
}

fn configure(shared: &Shared, jobs: Vec<Value>) {
    enable(shared);
    let mut state = shared.lock().unwrap();
    state.research_enabled = true;
    state.research_jobs = jobs;
}

/// This fixture emulates the acknowledgment boundary, not database validation.
/// An override can model transport custody ambiguity or an interleaved safe view.
pub(super) fn acknowledge(state: &mut Mock, body: &Value, current: &mut Value) {
    current
        .as_object_mut()
        .unwrap()
        .remove("checkpoint_receipt");
    let progress = body.get("research_progress").unwrap_or(body);
    if progress["checkpoint_protocol"] != PROTOCOL {
        return;
    }
    assert!(progress["reconciled_checkpoints"].is_array());
    let reconciled = progress["reconciled_checkpoints"].as_array().unwrap();
    let prior_head = current["research"]["current_checkpoint"].clone();
    let contexts = current["research"]["checkpoint_contexts"]
        .as_array_mut()
        .expect("offered checkpoint contexts");
    let before = contexts.len() + usize::from(prior_head.is_object());
    contexts.retain(|context| !reconciled.contains(&context["origin"]));
    if prior_head.is_object()
        && !reconciled.contains(&prior_head)
        && !contexts
            .iter()
            .any(|context| context["origin"] == prior_head)
    {
        contexts.push(json!({"status":"historical_revalidation_only",
            "origin":prior_head,"notes":"Earlier working checkpoint remains unresolved."}));
    }
    let unresolved = !contexts.is_empty();
    let terminal = body["candidates"].is_array() || progress["status"] == "no_change";
    if body["candidates"].is_array() {
        current["research"]["version"] =
            json!(current["research"]["version"].as_i64().unwrap() + 1);
    }
    for key in [
        "notes",
        "reviewed_sources",
        "pending_queries",
        "pending_targets",
        "status",
    ] {
        if let Some(value) = progress.get(key) {
            current["research"][key] = value.clone();
        }
    }
    if current["research"]["notes"]
        .as_str()
        .is_some_and(|notes| !notes.is_empty())
    {
        current["research"]["current_checkpoint"] = origin_at(
            current["research"]["version"].as_i64().unwrap(),
            current["research"]["snapshot_generation"].as_i64().unwrap(),
        );
    } else {
        current["research"]["current_checkpoint"] = Value::Null;
    }
    let after = current["research"]["checkpoint_contexts"]
        .as_array()
        .unwrap()
        .len()
        + usize::from(current["research"]["current_checkpoint"].is_object());
    let mut ack = json!({"operation_id":body["operation_id"],"protocol":PROTOCOL,
        "recorded":true,"new_source_coverage":true,"reconciled":after<before,
        "subject_complete":terminal&&!unresolved});
    if let Some(reply) = state.research_checkpoint_replies.pop_front() {
        if let Some(fields) = reply["view"].as_object() {
            current["research"]
                .as_object_mut()
                .unwrap()
                .extend(fields.clone());
        }
        if reply.get("ack").is_some_and(Value::is_null) {
            return;
        }
        if let Some(fields) = reply["ack"].as_object() {
            ack.as_object_mut().unwrap().extend(fields.clone());
        }
        if let Some(fields) = reply["omit"].as_array() {
            for field in fields {
                ack.as_object_mut().unwrap().remove(field.as_str().unwrap());
            }
        }
    }
    current["checkpoint_receipt"] = ack;
}

#[tokio::test]
async fn checkpoint_of_admitted_evidence_continues_to_submission_without_discovery() {
    let checkpoint = checkpoint_step(SAVED_NOTE, vec![selector(SOURCE, 1, 2)]);
    let behavior = program(&[
        ("research-1-1-answer.md", checkpoint.clone()),
        ("research-1-2-answer.md", candidate_reconciling(11)),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![enabled_job(SOURCE)]);
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(report.research["new_review_items"], 1);
    assert_eq!(report.research["processed_inputs"], 1);
    let next = std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
    assert!(next.contains(SAVED_NOTE));
    let state = shared.lock().unwrap();
    assert!(state.narrative_discoveries.is_empty());
    assert_eq!(state.research_progress.len(), 1);
    let saved = &state.research_progress[0];
    assert_eq!(saved["checkpoint_protocol"], PROTOCOL);
    assert_eq!(saved["status"], "researching");
    assert_eq!(saved["notes"], checkpoint["notes"]);
    assert_eq!(saved["processed_inputs"], json!([]));
    assert_eq!(saved["reconciled_checkpoints"], json!([]));
    for forbidden in [
        "queries",
        "targets",
        "candidates",
        "covers_existing",
        "follow_up",
    ] {
        assert!(saved.get(forbidden).is_none(), "{forbidden}: {saved}");
    }
    assert_eq!(
        state.submitted[0]["research_progress"]["checkpoint_protocol"],
        PROTOCOL
    );
    assert_eq!(
        state.submitted[0]["research_progress"]["reconciled_checkpoints"],
        json!([origin(11)])
    );
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
}

#[tokio::test]
async fn accepted_checkpoint_survives_a_child_failure_and_a_new_attempt() {
    let checkpoint = checkpoint_step(SAVED_NOTE, vec![selector(SOURCE, 1, 2)]);
    let candidate = candidate_reconciling(11);
    let behavior = format!(
        r#"
if [ "$OUTPUT_NAME" = 'probe-answer.md' ]; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md)
 if [ -f "$DIR/resume-checkpoint" ]; then
  cat > "$OUTPUT_PATH" <<'CHECKPOINT_JSON'
{candidate}
CHECKPOINT_JSON
 else
  cat > "$OUTPUT_PATH" <<'CHECKPOINT_JSON'
{checkpoint}
CHECKPOINT_JSON
 fi;;
 research-1-2-answer.md)
 cat <<'CHECKPOINT_EVENTS'
{{"type":"turn.failed","error":{{"message":"You've hit your usage limit."}}}}
CHECKPOINT_EVENTS
 exit 1;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![enabled_job(SOURCE)]);
    let first = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(first.outcome, RunOutcome::SkippedLimits, "{first:?}");
    assert_eq!(first.research["new_review_items"], 0);
    {
        let mut state = shared.lock().unwrap();
        assert_eq!(state.research_jobs[0]["notes"], SAVED_NOTE);
        assert_eq!(
            state.research_jobs[0]["reviewed_sources"],
            checkpoint["reviewed_sources"]
        );
        assert!(state.pending.is_empty());
        state.research_next_count = 0;
    }
    std::fs::write(dir.path().join("resume-checkpoint"), "resume").unwrap();
    let second = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(second.outcome, RunOutcome::Completed, "{second:?}");
    assert_eq!(second.research["new_review_items"], 1);
    let resumed =
        std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    assert!(resumed.contains(SAVED_NOTE));
    let state = shared.lock().unwrap();
    assert!(state.narrative_discoveries.is_empty());
    assert_eq!(state.research_progress.len(), 1);
    assert_eq!(state.submitted.len(), 1);
    assert_eq!(first.auth_persistence, "verified");
    assert_eq!(second.auth_persistence, "verified");
}

#[tokio::test]
async fn checkpoint_custody_requires_a_complete_exact_typed_ack_before_continuing() {
    let checkpoint = checkpoint_step(SAVED_NOTE, vec![selector(SOURCE, 1, 2)]);
    for reply in [
        json!({"ack":null}),
        json!({"ack":{"recorded":false}}),
        json!({"ack":{"operation_id":uuid::Uuid::now_v7()}}),
        json!({"ack":{"protocol":"dream.research.checkpoint.v0"}}),
        json!({"omit":["new_source_coverage"]}),
        json!({"omit":["reconciled"]}),
        json!({"omit":["subject_complete"]}),
        json!({"ack":{"new_source_coverage":"true"}}),
        json!({"ack":{"replayed":"false"}}),
    ] {
        let (shared, dreamer, dir) = build_with_budget(
            &program(&[("research-1-1-answer.md", checkpoint.clone())]),
            Duration::from_secs(8),
        )
        .await;
        configure(&shared, vec![enabled_job(SOURCE)]);
        shared
            .lock()
            .unwrap()
            .research_checkpoint_replies
            .push_back(reply.clone());
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert!(
            matches!(report.outcome, RunOutcome::Partial { .. }),
            "{reply}: {report:?}"
        );
        assert_eq!(
            report.research["stop_reason"], "operation_failed",
            "{reply}: {report:?}"
        );
        assert_eq!(report.research["processed_inputs"], 0);
        assert_eq!(report.research["new_review_items"], 0);
        assert!(!dir.path().join("prompt-research-1-2-answer.md").exists());
        let state = shared.lock().unwrap();
        assert_eq!(state.research_progress.len(), 1, "{reply}");
        assert!(state.submitted.is_empty());
        assert!(state.narrative_discoveries.is_empty());
        assert_eq!(report.auth_persistence, "verified");
    }
}

#[tokio::test]
async fn replay_does_not_credit_interleaved_coverage_and_next_subject_runs() {
    let first = checkpoint_step("First attempted checkpoint.", vec![selector(SOURCE, 1, 2)]);
    let second = checkpoint_step("Second attempted checkpoint.", vec![selector(SOURCE, 3, 4)]);
    let behavior = program(&[
        ("research-1-1-answer.md", first),
        ("research-1-2-answer.md", second),
        ("research-2-1-answer.md", done_step(SECOND)),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![enabled_job(SOURCE), enabled_job(SECOND)]);
    {
        let mut state = shared.lock().unwrap();
        for end in [100, 200] {
            state.research_checkpoint_replies.push_back(json!({
                "ack":{"replayed":true,"new_source_coverage":false,"reconciled":false},
                "view":{"notes":"Work saved by an interleaved request.",
                    "reviewed_sources":[selector(SOURCE,1,end)]}
            }));
        }
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["rounds"], 3, "{report:?}");
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert!(!dir.path().join("prompt-research-1-3-answer.md").exists());
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let state = shared.lock().unwrap();
    assert!(state.narrative_discoveries.is_empty());
    assert!(state.submitted.is_empty());
    assert!(
        state
            .research_progress
            .iter()
            .any(|request| request["subject_ref"] == SOURCE
                && request["status"] == "waiting"
                && request.get("checkpoint_protocol").is_none())
    );
}

#[tokio::test]
async fn cosmetic_reordered_and_alternating_checkpoint_coverage_cannot_keep_a_subject_running() {
    let a = selector(SOURCE, 1, 2);
    let b = selector(SOURCE, 3, 4);
    for reviewed in [
        vec![
            vec![a.clone(), b.clone()],
            vec![b.clone(), a.clone()],
            vec![a.clone(), b.clone()],
        ],
        vec![
            vec![a.clone()],
            vec![b.clone()],
            vec![a.clone()],
            vec![b.clone()],
        ],
    ] {
        let names = [
            "research-1-1-answer.md",
            "research-1-2-answer.md",
            "research-1-3-answer.md",
            "research-1-4-answer.md",
        ];
        let mut steps = reviewed
            .iter()
            .enumerate()
            .map(|(index, selectors)| {
                let mut step = checkpoint_step(
                    &format!("Cosmetic wording revision {index}."),
                    selectors.clone(),
                );
                if index > 0 {
                    step["reconciled_checkpoints"] = json!([origin(10 + index as i64)]);
                }
                (names[index], step)
            })
            .collect::<Vec<_>>();
        steps.push(("research-2-1-answer.md", done_step(SECOND)));
        let (shared, dreamer, dir) =
            build_with_budget(&program(&steps), Duration::from_secs(8)).await;
        configure(&shared, vec![enabled_job(SOURCE), enabled_job(SECOND)]);
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_eq!(report.research["rounds"], reviewed.len() + 1, "{report:?}");
        assert_eq!(report.research["subjects_yielded"], 1);
        assert_eq!(report.research["subjects_completed"], 1);
        assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
        assert!(
            !dir.path()
                .join(format!(
                    "prompt-research-1-{}-answer.md",
                    reviewed.len() + 1
                ))
                .exists()
        );
        let state = shared.lock().unwrap();
        assert!(state.narrative_discoveries.is_empty());
        assert!(state.submitted.is_empty());
    }
}

#[tokio::test]
async fn explicit_reconciliation_can_make_progress_without_new_source_coverage() {
    let selectors = vec![selector(SOURCE, 1, 2)];
    let mut first = checkpoint_step("First old unit incorporated.", selectors.clone());
    first["reconciled_checkpoints"] = json!([origin(1), origin(10)]);
    let mut second = checkpoint_step("Both old units incorporated.", selectors.clone());
    second["reconciled_checkpoints"] = json!([origin(2), origin(11)]);
    let behavior = program(&[
        ("research-1-1-answer.md", first),
        ("research-1-2-answer.md", second),
        ("research-1-3-answer.md", candidate_reconciling(12)),
    ]);
    let mut research = enabled_job(SOURCE);
    research["notes"] = json!("Current source review is already saved.");
    research["reviewed_sources"] = json!(selectors);
    research["current_checkpoint"] = origin(10);
    research["checkpoint_contexts"] = json!([
        {"status":"historical_revalidation_only","origin":origin(1),"notes":"Older unfinished unit one."},
        {"status":"historical_revalidation_only","origin":origin(2),"notes":"Older unfinished unit two."}
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![research]);
    {
        let mut state = shared.lock().unwrap();
        state.research_checkpoint_replies.extend([
            json!({"ack":{"new_source_coverage":false}}),
            json!({"ack":{"new_source_coverage":false}}),
        ]);
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(report.research["rounds"], 3);
    assert!(dir.path().join("prompt-research-1-3-answer.md").exists());
    let state = shared.lock().unwrap();
    assert_eq!(
        state.research_progress[0]["reconciled_checkpoints"],
        json!([origin(1), origin(10)])
    );
    assert_eq!(
        state.research_progress[1]["reconciled_checkpoints"],
        json!([origin(2), origin(11)])
    );
    assert!(state.research_progress.iter().all(|request| {
        request["findings"]
            .as_array()
            .is_some_and(|findings| !findings.is_empty())
    }));
    assert!(state.narrative_discoveries.is_empty());
}

#[tokio::test]
async fn accepted_partial_candidate_keeps_unresolved_work_and_counts_a_continuation() {
    let mut research = enabled_job(SOURCE);
    research["checkpoint_contexts"] = json!([{
        "status":"historical_revalidation_only","origin":origin(1),
        "notes":"A separate useful contribution remains unresolved."
    }]);
    let retained = research["checkpoint_contexts"].clone();
    let mut candidate = candidate_step();
    candidate["processed_inputs"] = json!([]);
    let (shared, dreamer, _dir) = build_with_budget(
        &program(&[("research-1-1-answer.md", candidate)]),
        Duration::from_secs(8),
    )
    .await;
    configure(&shared, vec![research]);
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.research["subjects_completed"], 0);
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["new_review_items"], 1);
    assert_eq!(report.research["processed_inputs"], 0);
    let state = shared.lock().unwrap();
    assert_eq!(state.pending.len(), 1);
    assert_eq!(
        state.submitted[0]["research_progress"]["checkpoint_protocol"],
        PROTOCOL
    );
    assert_eq!(state.research_jobs[0]["checkpoint_contexts"], retained);
    assert!(state.narrative_discoveries.is_empty());
}

#[tokio::test]
async fn candidate_custody_without_checkpoint_ack_is_not_reported_as_completed() {
    let (shared, dreamer, _dir) = build_with_budget(
        &program(&[("research-1-1-answer.md", candidate_step())]),
        Duration::from_secs(8),
    )
    .await;
    configure(&shared, vec![enabled_job(SOURCE)]);
    shared
        .lock()
        .unwrap()
        .research_checkpoint_replies
        .push_back(json!({"ack":null}));
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(
        report.research["stop_reason"], "operation_failed",
        "{report:?}"
    );
    assert_eq!(report.research["subjects_completed"], 0);
    assert_eq!(report.research["new_review_items"], 0);
    assert_eq!(report.research["processed_inputs"], 0);
    let state = shared.lock().unwrap();
    assert_eq!(state.submitted.len(), 1);
    // The server may have committed the candidate; uncertain acknowledgment
    // must not trigger another semantic request or a claimed completed pass.
    assert_eq!(state.pending.len(), 1);
    assert!(state.research_progress.is_empty());
}

#[tokio::test]
async fn old_api_rejects_checkpoint_action_and_still_accepts_legacy_submission() {
    let checkpoint = checkpoint_step(SAVED_NOTE, vec![selector(SOURCE, 1, 2)]);
    let behavior = program(&[
        ("research-1-1-answer.md", checkpoint.clone()),
        ("research-1-2-answer.md", checkpoint),
    ]);
    let (shared, dreamer, _dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![job(SOURCE)]);
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["subjects_yielded"], 1, "{report:?}");
    {
        let state = shared.lock().unwrap();
        assert!(state.submitted.is_empty());
        assert!(state.narrative_discoveries.is_empty());
        assert!(
            state
                .research_progress
                .iter()
                .all(|request| request.get("checkpoint_protocol").is_none()
                    && request.get("notes").is_none())
        );
        assert_eq!(state.research_jobs[0]["notes"], "");
    }
    let (shared, dreamer, _dir) = build_with_budget(
        &program(&[("research-1-1-answer.md", candidate_step())]),
        Duration::from_secs(8),
    )
    .await;
    configure(&shared, vec![job(SOURCE)]);
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["subjects_completed"], 1);
    let state = shared.lock().unwrap();
    assert!(
        state.submitted[0]["research_progress"]
            .get("checkpoint_protocol")
            .is_none()
    );
}

#[tokio::test]
async fn rejected_checkpoint_capacity_yields_locally_and_the_next_subject_completes() {
    let checkpoint = checkpoint_step(SAVED_NOTE, vec![selector(SOURCE, 1, 2)]);
    let behavior = program(&[
        ("research-1-1-answer.md", checkpoint.clone()),
        ("research-1-2-answer.md", checkpoint),
        ("research-2-1-answer.md", done_step(SECOND)),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![enabled_job(SOURCE), enabled_job(SECOND)]);
    {
        let mut state = shared.lock().unwrap();
        let response = json!({"error":{"code":"invalid_request",
            "message":"Research checkpoint capacity requires explicit reconciliation.",
            "details":{"dreamer_repair_phase":"checkpoint_validation"}}})
        .to_string();
        state.research_progress_replies.extend([
            Some((StatusCode::BAD_REQUEST, response.clone())),
            Some((StatusCode::BAD_REQUEST, response)),
        ]);
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["subjects_yielded"], 1, "{report:?}");
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(report.research["new_review_items"], 0);
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let state = shared.lock().unwrap();
    assert_eq!(state.research_jobs[0]["notes"], "");
    assert!(
        state
            .research_progress
            .iter()
            .any(|request| request["subject_ref"] == SOURCE
                && request["status"] == "waiting"
                && request.get("checkpoint_protocol").is_none())
    );
    assert!(state.submitted.is_empty());
}

fn freshness_rejection() -> Option<(StatusCode, String)> {
    Some((StatusCode::BAD_REQUEST, json!({"error":{
        "code":"research_refresh_required",
        "message":"Research evidence changed; refresh the admitted source versions before saving conclusions."
    }}).to_string()))
}

fn assert_header_only_refresh(request: &Value) {
    let allowed = [
        "attempt_id",
        "fence",
        "expected_state_version",
        "operation_id",
        "subject_ref",
        "research_version",
        "queries",
        "targets",
    ];
    assert_eq!(
        request.as_object().unwrap().len(),
        allowed.len(),
        "{request}"
    );
    assert!(
        request
            .as_object()
            .unwrap()
            .keys()
            .all(|field| allowed.contains(&field.as_str()))
    );
    assert_eq!(request["queries"], json!([]));
    assert_eq!(request["targets"], json!([]));
}

#[tokio::test]
async fn typed_checkpoint_freshness_rejection_refreshes_headers_before_the_next_child() {
    let stale = checkpoint_step("Rejected old-version work.", vec![selector(SOURCE, 1, 2)]);
    let mut current_selector = selector(SOURCE, 1, 2);
    current_selector["version"] = json!(3);
    let fresh = checkpoint_step(
        "Accepted current-version work.",
        vec![current_selector.clone()],
    );
    let mut done = done_step(SOURCE);
    done["reviewed_sources"] = json!([current_selector]);
    done["reconciled_checkpoints"] = json!([origin_at(13, 18)]);
    done["findings"] = json!([
        "The saved current-version work and remaining leads have been reviewed; no useful further change remains."
    ]);
    let behavior = program(&[
        ("research-1-1-answer.md", stale),
        ("research-1-2-answer.md", fresh),
        ("research-1-3-answer.md", done),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![enabled_job(SOURCE)]);
    {
        let mut state = shared.lock().unwrap();
        state
            .research_progress_replies
            .push_back(freshness_rejection());
        state.research_discovery_sources.push_back(vec![json!({
            "entry_ref":SOURCE,"path":"sources/Project-current.md","version":3,"generation":18
        })]);
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["rounds"], 3);
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(report.research["new_review_items"], 0);
    let prompt = std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
    assert!(prompt.contains("sources/Project-current.md"));
    let state = shared.lock().unwrap();
    assert_eq!(state.narrative_discoveries.len(), 1);
    assert_header_only_refresh(&state.narrative_discoveries[0]);
    assert_ne!(
        state.narrative_discoveries[0]["operation_id"],
        state.research_progress[0]["operation_id"]
    );
    assert_eq!(
        state.research_progress[1]["repair_feedback"]["phase"],
        "checkpoint_validation"
    );
    assert_eq!(state.research_progress[1]["status"], "researching");
    assert!(
        state.research_progress[1]
            .get("checkpoint_protocol")
            .is_none()
    );
    assert_eq!(
        state.research_progress[2]["notes"],
        "Accepted current-version work."
    );
    assert_eq!(
        state.research_progress[2]["reviewed_sources"][0]["version"],
        3
    );
    assert!(state.submitted.is_empty());
}

#[tokio::test]
async fn repeated_freshness_rejections_still_yield_after_successful_header_refreshes() {
    let checkpoint = checkpoint_step("Still rejected current work.", vec![selector(SOURCE, 1, 2)]);
    let behavior = program(&[
        ("research-1-1-answer.md", checkpoint.clone()),
        ("research-1-2-answer.md", checkpoint),
        ("research-2-1-answer.md", done_step(SECOND)),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    configure(&shared, vec![enabled_job(SOURCE), enabled_job(SECOND)]);
    {
        let mut state = shared.lock().unwrap();
        state
            .research_progress_replies
            .extend([freshness_rejection(), freshness_rejection()]);
        for _ in 0..2 {
            state
                .research_discovery_sources
                .push_back(job(SOURCE)["sources"].as_array().unwrap().clone());
        }
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["rounds"], 3, "{report:?}");
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert!(!dir.path().join("prompt-research-1-3-answer.md").exists());
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let state = shared.lock().unwrap();
    assert_eq!(state.narrative_discoveries.len(), 2);
    for refresh in &state.narrative_discoveries {
        assert_header_only_refresh(refresh);
    }
    assert_eq!(state.research_jobs[0]["notes"], "");
    assert!(
        state
            .research_progress
            .iter()
            .any(|request| request["subject_ref"] == SOURCE
                && request["status"] == "waiting"
                && request["repair_feedback"]["phase"] == "checkpoint_validation"
                && request.get("checkpoint_protocol").is_none())
    );
    assert!(state.submitted.is_empty());
}

//! Runner research: real subprocess/HTTP lifecycle, deterministic model fixture.
use super::*;

pub(super) async fn next(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    assert_operation(&body);
    let mut s = shared.lock().unwrap();
    s.state_version += 1;
    let mut current = s.current_admission.clone().unwrap();
    current["state_version"] = json!(s.state_version);
    current["research"] = s
        .research_jobs
        .get(s.research_next_count)
        .cloned()
        .unwrap_or(Value::Null);
    s.research_next_count += 1;
    s.current_admission = Some(current.clone());
    Json(json!({"data":current}))
}

pub(super) async fn progress(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    assert_operation(&body);
    let mut s = shared.lock().unwrap();
    s.state_version += 1;
    s.research_progress.push(body.clone());
    let mut current = s.current_admission.clone().unwrap();
    current["state_version"] = json!(s.state_version);
    current["research"]["version"] = json!(current["research"]["version"].as_i64().unwrap() + 1);
    for key in [
        "notes",
        "reviewed_sources",
        "pending_queries",
        "pending_targets",
        "status",
    ] {
        if let Some(value) = body.get(key) {
            current["research"][key] = value.clone();
        }
    }
    s.current_admission = Some(current.clone());
    Json(json!({"data":current}))
}

pub(super) fn assert_operation(body: &Value) {
    uuid::Uuid::parse_str(body["operation_id"].as_str().expect("operation identity"))
        .expect("server requires UUID operation identity");
}

fn job(subject: &str) -> Value {
    json!({"subject_ref":subject,"subject_path":"sources/Project.md","title":"Project",
        "output_path":"derived/entities/project.md","output_version":0,"version":1,"snapshot_generation":17,
        "sources":[{"entry_ref":subject,"path":"sources/Project.md","version":2,"generation":17}],
        "reviewed_sources":[],"notes":"","pending_queries":[],"pending_targets":[],"round":0,"status":"researching"})
}

fn candidate_step() -> Value {
    json!({"schema":"dream.research.step.v1","action":"submit",
        "reviewed_sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
        "notes":"The supported project state is ready for review.",
        "candidates":[{"kind":"summary","subject_ref":SOURCE,"title":"Project overview",
            "summary":"Current project state","reason":"Consolidates the supported project facts",
            "path":"derived/entities/project.md","expected_version":0,
            "content":"The project is active.[^s1]","sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],"uncertainty":""}],
        "processed_inputs":[{"entry_ref":SOURCE,"version":2,"generation":17}],"findings":[]})
}

#[tokio::test]
async fn source_research_follows_missing_primary_reference_then_submits_in_same_run() {
    let discovery = json!({"schema":"dream.research.step.v1","action":"discover","targets":["sources/Project/Outcome.md"]});
    let submit = candidate_step();
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{discovery}
JSON
 ;;
 research-1-2-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{submit}
JSON
 ;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build(&behavior).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE)];
        s.narrative_context = vec![
            job(SOURCE)["sources"][0].clone(),
            json!({"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce6","path":"sources/Project/Outcome.md","version":3,"generation":19}),
        ];
    }
    let report = dreamer
        .run_once_with_subjects(today(), RunKind::Manual, &[SOURCE.into()])
        .await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["rounds"], 2);
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(report.auth_persistence, "verified");
    let s = shared.lock().unwrap();
    assert_eq!(
        s.admission_requests[0]["requested_subject_refs"],
        json!([SOURCE])
    );
    assert_eq!(s.narrative_discoveries[0]["targets"], discovery["targets"]);
    assert_eq!(s.submitted.len(), 1);
    assert_eq!(s.submitted[0]["subject_ref"], SOURCE);
    assert_eq!(s.notifications.len(), 1);
    let prompt = std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
    assert!(prompt.contains("sources/Project/Outcome.md"));
    assert!(
        !std::fs::read_to_string(dir.path().join("calls"))
            .unwrap()
            .lines()
            .any(|name| name == "answer.md")
    );
}

#[tokio::test]
async fn malformed_candidate_is_repaired_locally_without_losing_the_subject() {
    let mut bad = candidate_step();
    bad["candidates"][0]["uncertainty"] = json!("Unsupported separate caveat");
    let good = candidate_step();
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{bad}
JSON
 ;;
 research-1-2-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{good}
JSON
 ;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build(&behavior).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE)];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(shared.lock().unwrap().submitted.len(), 1);
    assert!(
        std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md"))
            .unwrap()
            .contains("summary uncertainty must be a verbatim excerpt of content")
    );
}

#[tokio::test]
async fn repeatedly_invalid_subject_yields_and_next_subject_progresses() {
    let second = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let done = json!({"schema":"dream.research.step.v1","action":"done",
        "reviewed_sources":[{"entry_ref":second,"version":2,"start_line":1,"end_line":2}],
        "findings":["Existing material needs no new summary."]});
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-*-answer.md) echo 'invalid JSON' > "$OUTPUT_PATH";;
 research-2-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{done}
JSON
 ;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, _dir) = build(&behavior).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE), job(second)];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(shared.lock().unwrap().research_progress.len(), 2);
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn slow_subject_yields_saved_progress_and_leaves_time_for_another() {
    let second = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let done = json!({"schema":"dream.research.step.v1","action":"done",
        "reviewed_sources":[{"entry_ref":second,"version":2,"start_line":1,"end_line":2}],
        "findings":["The second subject is already covered."]});
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-*-answer.md) exec sleep 30;;
 research-2-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{done}
JSON
 ;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(6)).await;
    enable(&shared);
    {
        let mut first = job(SOURCE);
        first["notes"] = json!("Previously verified progress must survive a slow round.");
        first["reviewed_sources"] =
            json!([{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}]);
        first["pending_targets"] = json!(["sources/Project/Outcome.md"]);
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![first, job(second)];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let s = shared.lock().unwrap();
    assert_eq!(s.research_progress[0]["status"], "waiting");
    for key in ["notes", "reviewed_sources", "pending_targets"] {
        assert!(
            s.research_progress[0].get(key).is_none(),
            "forced yield must preserve {key}"
        );
    }
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn location_and_subject_research_remain_separate_and_notify_once() {
    let mut submit = candidate_step();
    submit["processed_inputs"] = json!([]);
    let location = location_output("Audited location observation.");
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 location-discovery-answer.md)
  echo '{{"schema":"dream.location.discovery.v1","context_queries":[],"lookups":[],"findings":[]}}' > "$OUTPUT_PATH";;
 location-answer.md|location-audit-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{location}
JSON
 ;;
 research-1-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{submit}
JSON
 ;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(6)).await;
    enable_location(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        let mut research = job(SOURCE);
        research["notes"] = json!("ORDINARY_RESEARCH_CANARY");
        s.research_jobs = vec![research.clone()];
        let admission = s.location_admission.as_mut().unwrap();
        admission["research"] = research;
        admission["location_evidence"]["private_canary"] = json!("LOCATION_PACKET_CANARY");
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["new_review_items"], 2);
    let s = shared.lock().unwrap();
    assert_eq!(s.notifications.len(), 1);
    assert_eq!(s.submitted.len(), 2);
    assert_eq!(
        s.submitted[0]["candidates"][0]["revises_item_id"],
        LOCATION_ITEM
    );
    assert_eq!(s.submitted[1]["subject_ref"], SOURCE);
    for name in [
        "prompt-location-answer.md",
        "prompt-location-audit-answer.md",
    ] {
        assert!(
            !std::fs::read_to_string(dir.path().join(name))
                .unwrap()
                .contains("ORDINARY_RESEARCH_CANARY")
        );
    }
    let research =
        std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    assert!(research.contains("ORDINARY_RESEARCH_CANARY"));
    assert!(!research.contains("LOCATION_PACKET_CANARY"));
    assert!(!research.contains("Audited location observation."));
}

#[tokio::test]
async fn source_change_pages_are_drained_before_model_research() {
    let done = json!({"schema":"dream.research.step.v1","action":"done",
        "reviewed_sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
        "processed_inputs":[{"entry_ref":SOURCE,"version":2,"generation":17}],
        "findings":["Reviewed the source; no useful change remains."]});
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{done}
JSON
 ;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build(&behavior).await;
    enable(&shared);
    {
        let mut subject = job(SOURCE);
        subject["coverage"] = json!({"change_status":"unchecked","change_reason":"subject_change_check_limit",
            "change_cursor":1000,"change_upper":3000});
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.narrative_context = subject["sources"].as_array().unwrap().clone();
        s.research_jobs = vec![subject];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["rounds"], 1);
    assert_eq!(report.research["change_pages"], 2);
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 2);
    assert!(
        s.narrative_discoveries
            .iter()
            .all(|v| v["queries"] == json!([]) && v["targets"] == json!([]))
    );
    let prompt = std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    assert!(prompt.contains("\"change_status\":\"complete\""));
}

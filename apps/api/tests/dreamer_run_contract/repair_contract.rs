//! Public validation errors survive a real runner process/restart boundary.
use super::*;

const CORRECTION: &str = "summary line 2 needs a declared citation or a Markdown heading";

fn rejection() -> (StatusCode, String) {
    (
        StatusCode::BAD_REQUEST,
        json!({"error":{"code":"invalid_request",
        "message":CORRECTION,"details":{"dreamer_repair_phase":"candidate_validation"}}})
        .to_string(),
    )
}

fn rejected_step() -> Value {
    let mut step = candidate_step();
    step["candidates"][0]["content"] = json!("The project is active.[^s1]\nThe remaining work:");
    step
}

fn assert_operational(body: &Value) {
    let allowed = [
        "attempt_id",
        "fence",
        "expected_state_version",
        "operation_id",
        "subject_ref",
        "research_version",
        "status",
        "repair_feedback",
        "processed_inputs",
    ];
    assert_eq!(body.as_object().unwrap().len(), allowed.len());
    assert!(
        body.as_object()
            .unwrap()
            .keys()
            .all(|key| allowed.contains(&key.as_str()))
    );
    assert_eq!(body["processed_inputs"], json!([]));
    assert_eq!(body["repair_feedback"]["message"], CORRECTION);
    assert_eq!(body["repair_feedback"]["phase"], "candidate_validation");
}

#[tokio::test]
async fn repair_feedback_survives_timed_out_correction_and_a_new_attempt() {
    let bad = rejected_step();
    let good = candidate_step();
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md)
 if [ ! -f "$DIR/first-rejected-attempt" ]; then
  touch "$DIR/first-rejected-attempt"
  cat > "$OUTPUT_PATH" <<'JSON'
{bad}
JSON
 else
  cat > "$OUTPUT_PATH" <<'JSON'
{good}
JSON
 fi;;
 research-1-2-answer.md) exec sleep 30;;
 *) exit 99;;
esac
"#
    );
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(6)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        let mut saved = job(SOURCE);
        saved["notes"] = json!("Previously accepted source-backed work.");
        saved["reviewed_sources"] =
            json!([{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}]);
        s.research_jobs = vec![saved];
        s.research_candidate_replies.push_back(rejection());
    }
    let first = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(first.outcome, RunOutcome::Partial { .. }),
        "{first:?}"
    );
    assert_eq!(first.research["processed_inputs"], 0);
    assert_eq!(first.auth_persistence, "verified");
    {
        let mut s = shared.lock().unwrap();
        assert_operational(&s.research_progress[0]);
        assert_eq!(s.research_progress[0]["status"], "researching");
        assert_eq!(
            s.research_jobs[0]["notes"],
            "Previously accepted source-backed work."
        );
        assert_eq!(s.research_jobs[0]["repair_feedback"]["message"], CORRECTION);
        assert_eq!(s.research_jobs[0]["status"], "waiting");
        assert!(s.pending.is_empty());
        s.research_next_count = 0;
    }
    let retry_prompt =
        std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
    assert!(retry_prompt.contains(CORRECTION));
    let second = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(second.outcome, RunOutcome::Completed, "{second:?}");
    assert_eq!(second.research["new_review_items"], 1);
    assert_eq!(second.auth_persistence, "verified");
    let resumed =
        std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    assert!(resumed.contains(CORRECTION));
    assert!(resumed.contains("Previously accepted source-backed work."));
    let s = shared.lock().unwrap();
    assert_eq!(s.submitted.len(), 2);
    assert!(
        s.submitted[1]["research_progress"]
            .get("repair_feedback")
            .is_none()
    );
}

#[tokio::test]
async fn repair_tail_yields_without_a_tiny_child_and_next_subject_completes() {
    let bad = rejected_step();
    let second = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let done = json!({"schema":"dream.research.step.v1","action":"done",
        "reviewed_sources":[{"entry_ref":second,"version":2,"start_line":1,"end_line":2}],
        "findings":["The second subject is already covered."]});
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) sleep 2.3; cat > "$OUTPUT_PATH" <<'JSON'
{bad}
JSON
 ;;
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
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE), job(second)];
        s.research_candidate_replies.push_back(rejection());
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["rounds"], 2, "{report:?}");
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert!(!dir.path().join("prompt-research-1-2-answer.md").exists());
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let s = shared.lock().unwrap();
    assert_eq!(s.research_progress.len(), 2);
    assert_operational(&s.research_progress[0]);
    assert_eq!(s.research_progress[0]["status"], "waiting");
    assert_eq!(s.research_jobs[0]["repair_feedback"]["message"], CORRECTION);
}

#[tokio::test]
async fn repair_custody_requires_the_exact_ack_before_any_correction_child() {
    for wrong_ack in [false, true] {
        let bad = rejected_step();
        let (shared, dreamer, dir) = build(&one_step_behavior(&bad)).await;
        enable(&shared);
        {
            let mut s = shared.lock().unwrap();
            s.research_enabled = true;
            s.research_jobs = vec![job(SOURCE)];
            s.research_candidate_replies.push_back(rejection());
            if wrong_ack {
                s.research_repair_replies.push_back((StatusCode::OK,
                    json!({"data":{"repair_feedback_receipt":{"operation_id":uuid::Uuid::now_v7(),"recorded":true}}}).to_string()));
            } else {
                s.omit_repair_ack = true;
            }
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_eq!(
            report.research["stop_reason"], "operation_failed",
            "{report:?}"
        );
        assert_eq!(report.research["processed_inputs"], 0);
        assert_eq!(report.auth_persistence, "verified");
        assert!(!dir.path().join("prompt-research-1-2-answer.md").exists());
        assert!(shared.lock().unwrap().pending.is_empty());
    }
}

#[tokio::test]
async fn repair_lifecycle_keeps_candidate_error_through_discovery_but_clears_parse_error() {
    for candidate_error in [false, true] {
        let mut bad = rejected_step();
        if !candidate_error {
            bad["candidates"][0]["uncertainty"] =
                json!("This separate caveat is not in the content.");
        }
        let discover = json!({"schema":"dream.research.step.v1","action":"discover",
            "notes":"Reviewed source after the failed response.",
            "reviewed_sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
            "queries":["project current status"]});
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
{discover}
JSON
 ;;
 research-1-3-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
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
            s.narrative_context = job(SOURCE)["sources"].as_array().unwrap().clone();
            if candidate_error {
                s.research_candidate_replies.push_back(rejection());
            }
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
        let s = shared.lock().unwrap();
        let hint = s.research_progress[0]["repair_feedback"]["message"]
            .as_str()
            .unwrap();
        let correction =
            std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
        let after =
            std::fs::read_to_string(dir.path().join("prompt-research-1-3-answer.md")).unwrap();
        assert!(correction.contains(hint));
        assert_eq!(after.contains(hint), candidate_error, "{after}");
        assert_eq!(
            s.research_progress[1]["notes"],
            "Reviewed source after the failed response."
        );
    }
}

#[tokio::test]
async fn search_only_discovery_does_not_reset_candidate_repair_limit() {
    let second = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let behavior = scripted_steps(&[
        ("research-1-1-answer.md", rejected_step()),
        (
            "research-1-2-answer.md",
            json!({"schema":"dream.research.step.v1",
            "action":"discover","queries":["project current status"]}),
        ),
        ("research-1-3-answer.md", rejected_step()),
        (
            "research-2-1-answer.md",
            json!({"schema":"dream.research.step.v1",
            "action":"done","reviewed_sources":[{"entry_ref":second,"version":2,"start_line":1,"end_line":2}],
            "findings":["The second subject already has a useful overview."]}),
        ),
    ]);
    let (shared, dreamer, dir) = build(&behavior).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE), job(second)];
        s.research_candidate_replies = VecDeque::from([rejection(), rejection()]);
        s.narrative_context = job(SOURCE)["sources"].as_array().unwrap().clone();
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["rounds"], 4, "{report:?}");
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert!(!dir.path().join("prompt-research-1-4-answer.md").exists());
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 1);
    assert_eq!(s.research_progress.len(), 3);
    assert_operational(&s.research_progress[0]);
    assert_operational(&s.research_progress[1]);
    assert_eq!(s.research_progress[1]["status"], "waiting");
    assert!(s.pending.is_empty());
}

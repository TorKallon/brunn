//! Wrapper lifecycle over real child processes; server transactions have separate DB gates.
use super::*;
use brunn::dreamer::research::{DRAFT_PROTOCOL, draft_hash};

fn pointer(candidate: &Value, version: i64) -> Value {
    json!({"entry_ref":RUN,"version":version,"candidate_hash":draft_hash(candidate)})
}

fn receipt(body: &Value, pointer: &Value, retired: bool) -> Value {
    json!({"protocol":DRAFT_PROTOCOL,"operation_id":body["operation_id"],"recorded":true,
        "replayed":false,"pointer":pointer,"retired":retired})
}

fn corrupt(current: &mut Value, fault: Option<&str>) {
    match fault {
        Some("missing") => {
            current.as_object_mut().unwrap().remove("draft_receipt");
        }
        Some("operation") => current["draft_receipt"]["operation_id"] = json!(uuid::Uuid::now_v7()),
        Some("hash") => {
            current["draft_receipt"]["pointer"]["candidate_hash"] = json!("0".repeat(64))
        }
        Some("version") => current["draft_receipt"]["pointer"]["version"] = json!(0),
        Some("retired") => {
            current["draft_receipt"]["retired"] =
                json!(!current["draft_receipt"]["retired"].as_bool().unwrap())
        }
        Some("replayed") => current["draft_receipt"]["replayed"] = Value::Null,
        None => {}
        _ => panic!("unknown test fault"),
    }
}

pub(super) fn custody(s: &mut Mock, body: &Value) -> Response {
    assert_eq!(body["draft_protocol"], DRAFT_PROTOCOL);
    assert_eq!(body["processed_inputs"], json!([]));
    for forbidden in [
        "notes",
        "status",
        "reviewed_sources",
        "reconciled_checkpoints",
        "covers_existing",
        "supersedes_existing",
        "follow_up",
        "resolved_follow_ups",
    ] {
        assert!(
            body.get(forbidden).is_none(),
            "custody cannot carry {forbidden}"
        );
    }
    let mut current = s.current_admission.clone().unwrap();
    let old = &current["research"]["unaccepted_draft"];
    let candidate = &body["draft_candidate"];
    let pointer = if old["status"] == "unaccepted_revalidation_only" {
        if old["pointer"]["candidate_hash"] == draft_hash(candidate) {
            old["pointer"].clone()
        } else {
            assert_eq!(body["replaces_draft"], old["pointer"]);
            assert!(!body["findings"].as_array().unwrap().is_empty());
            pointer(candidate, old["pointer"]["version"].as_i64().unwrap() + 1)
        }
    } else {
        pointer(candidate, 1)
    };
    current["research"]["unaccepted_draft"] = json!({"status":"unaccepted_revalidation_only",
        "pointer":pointer,"candidate":candidate,"origin":{"entry_ref":RUN,"version":1,"snapshot_generation":17},
        "source_delta":{"changed":[],"new":[],"missing":[],"coverage_complete":true,"truncated":false}});
    current["draft_receipt"] = receipt(body, &pointer, false);
    let saved = s
        .research_jobs
        .iter_mut()
        .find(|job| job["subject_ref"] == current["research"]["subject_ref"])
        .unwrap();
    *saved = current["research"].clone();
    s.current_admission = Some(current.clone());
    corrupt(&mut current, s.draft_custody_fault.as_deref());
    Json(json!({"data":current})).into_response()
}

pub(super) fn accepted(s: &mut Mock, body: &Value, current: &mut Value, accepted: bool) {
    current.as_object_mut().unwrap().remove("draft_receipt");
    if body.get("draft_pointer").is_none() {
        return;
    }
    assert_eq!(
        body["draft_pointer"],
        current["research"]["unaccepted_draft"]["pointer"]
    );
    current["draft_receipt"] = receipt(body, &body["draft_pointer"], accepted);
    if accepted {
        current["research"]["unaccepted_draft"] = Value::Null;
    }
    *s.research_jobs
        .iter_mut()
        .find(|job| job["subject_ref"] == current["research"]["subject_ref"])
        .unwrap() = current["research"].clone();
    corrupt(current, s.draft_submit_fault.as_deref());
}

fn enabled_job() -> Value {
    let mut value = job(SOURCE);
    value["draft_protocol"] = json!(DRAFT_PROTOCOL);
    value["unaccepted_draft"] = Value::Null;
    value
}

#[tokio::test]
async fn stale_draft_survives_timeout_and_restarts_as_exact_replacement() {
    let mut initial = candidate_step();
    initial["candidates"][0]["content"] =
        json!("UNACCEPTED_ORIGINAL_DRAFT remains supported.[^s1]");
    let mut revised = candidate_step();
    revised["candidates"][0]["content"] =
        json!("The project is active after checking the added scope.[^s1]");
    revised["replaces_draft"] = pointer(&initial["candidates"][0], 1);
    revised["findings"] =
        json!(["The initial draft was incorporated after reviewing newly admitted scope."]);
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md)
 if [ ! -f "$DIR/first-attempt" ]; then
  touch "$DIR/first-attempt"
  cat > "$OUTPUT_PATH" <<'JSON'
{initial}
JSON
 else
  cat > "$OUTPUT_PATH" <<'JSON'
{revised}
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
        s.research_jobs = vec![enabled_job()];
        s.research_discovery_sources
            .push_back(enabled_job()["sources"].as_array().unwrap().clone());
        s.research_candidate_replies.push_back((StatusCode::BAD_REQUEST,
            json!({"error":{"code":"research_refresh_required","message":"Relevant subject scope changed."}}).to_string()));
    }
    let first = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(first.outcome, RunOutcome::Partial { .. }),
        "{first:?}"
    );
    assert_eq!(first.research["processed_inputs"], 0);
    assert_eq!(first.research["new_review_items"], 0);
    assert_eq!(first.auth_persistence, "verified");
    {
        let mut s = shared.lock().unwrap();
        assert_eq!(
            s.research_jobs[0]["unaccepted_draft"]["candidate"],
            initial["candidates"][0]
        );
        assert!(s.pending.is_empty());
        assert_eq!(s.narrative_discoveries.len(), 1);
        assert_eq!(s.narrative_discoveries[0]["queries"], json!([]));
        assert_eq!(s.narrative_discoveries[0]["targets"], json!([]));
        assert_eq!(
            s.submitted[0]["research_version"],
            s.research_progress[0]["research_version"]
        );
        assert_eq!(
            s.submitted[0]["expected_state_version"],
            s.research_progress[0]["expected_state_version"]
        );
        s.research_next_count = 0;
    }
    let retry = std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
    assert!(retry.contains("UNACCEPTED_ORIGINAL_DRAFT"));
    assert!(retry.contains("Relevant subject scope changed"));
    let second = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(second.outcome, RunOutcome::Completed, "{second:?}");
    assert_eq!(second.research["new_review_items"], 1);
    let resumed =
        std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    assert!(resumed.contains("UNACCEPTED_ORIGINAL_DRAFT"));
    let s = shared.lock().unwrap();
    assert_eq!(s.submitted.len(), 2);
    assert_ne!(
        s.submitted[0]["operation_id"],
        s.submitted[1]["operation_id"]
    );
    assert_eq!(s.submitted[1]["candidates"][0], revised["candidates"][0]);
    assert!(s.research_jobs[0]["unaccepted_draft"].is_null());
}

#[tokio::test]
async fn custody_requires_a_complete_exact_ack_before_submission() {
    for fault in [
        "missing",
        "operation",
        "hash",
        "version",
        "retired",
        "replayed",
    ] {
        let (shared, dreamer, _dir) = build(&one_step_behavior(&candidate_step())).await;
        enable(&shared);
        {
            let mut s = shared.lock().unwrap();
            s.research_enabled = true;
            s.research_jobs = vec![enabled_job()];
            s.draft_custody_fault = Some(fault.into());
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_ne!(report.outcome, RunOutcome::Completed, "{fault}: {report:?}");
        assert_eq!(report.research["new_review_items"], 0);
        assert_eq!(report.research["processed_inputs"], 0);
        assert_eq!(report.auth_persistence, "verified");
        let s = shared.lock().unwrap();
        assert!(s.submitted.is_empty(), "{fault}");
        assert!(s.research_jobs[0]["unaccepted_draft"].is_object());
    }
}

#[tokio::test]
async fn unavailable_and_legacy_drafts_do_not_block_independent_ordinary_submission() {
    for unavailable in [false, true] {
        let (shared, dreamer, _dir) = build(&one_step_behavior(&candidate_step())).await;
        enable(&shared);
        {
            let mut s = shared.lock().unwrap();
            s.research_enabled = true;
            let mut job = job(SOURCE);
            if unavailable {
                job["draft_protocol"] = json!(DRAFT_PROTOCOL);
                job["unaccepted_draft"] = json!({"status":"unavailable"});
            }
            s.research_jobs = vec![job];
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
        let s = shared.lock().unwrap();
        assert!(
            !s.research_progress
                .iter()
                .any(|b| b.get("draft_candidate").is_some())
        );
        assert!(s.submitted[0].get("draft_pointer").is_none());
        if unavailable {
            assert_eq!(
                s.research_jobs[0]["unaccepted_draft"],
                json!({"status":"unavailable"})
            );
        }
    }
}

#[tokio::test]
async fn accepted_candidate_requires_matching_retirement_ack_before_credit() {
    let (shared, dreamer, _dir) = build(&one_step_behavior(&candidate_step())).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![enabled_job()];
        s.draft_submit_fault = Some("retired".into());
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_ne!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["new_review_items"], 0);
    assert_eq!(report.research["processed_inputs"], 0);
    assert_eq!(shared.lock().unwrap().submitted.len(), 1);
}

#[tokio::test]
async fn zero_id_response_retains_draft_without_research_credit() {
    let mut step = candidate_step();
    step["processed_inputs"] = json!([]);
    let (shared, dreamer, _dir) = build(&one_step_behavior(&step)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![enabled_job()];
        s.draft_zero_ids = true;
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["new_review_items"], 0);
    assert_eq!(report.research["processed_inputs"], 0);
    assert!(shared.lock().unwrap().research_jobs[0]["unaccepted_draft"].is_object());
}

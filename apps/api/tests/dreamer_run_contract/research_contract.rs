//! Runner research: real subprocess/HTTP lifecycle, deterministic model fixture.
use super::*;

#[path = "repair_contract.rs"]
mod repair_contract;

pub(super) async fn next(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    assert_operation(&body);
    let (current, delay) = {
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
        (current, s.research_next_delay)
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    Json(json!({"data":current}))
}

pub(super) async fn progress(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    assert_operation(&body);
    let (current, delay) = {
        let mut s = shared.lock().unwrap();
        s.research_progress.push(body.clone());
        let repair_only = body.get("repair_feedback").is_some();
        if repair_only && let Some((status, response)) = s.research_repair_replies.pop_front() {
            return (status, response).into_response();
        }
        if !repair_only
            && body["status"] == "researching"
            && let Some(Some((status, response))) = s.research_progress_replies.pop_front()
        {
            return (status, response).into_response();
        }
        s.state_version += 1;
        let mut current = s.current_admission.clone().unwrap();
        current
            .as_object_mut()
            .unwrap()
            .remove("repair_feedback_receipt");
        current["state_version"] = json!(s.state_version);
        current["research"]["version"] =
            json!(current["research"]["version"].as_i64().unwrap() + 1);
        for key in [
            "notes",
            "reviewed_sources",
            "pending_queries",
            "pending_targets",
            "status",
            "repair_feedback",
        ] {
            if let Some(value) = body.get(key) {
                current["research"][key] = value.clone();
            }
        }
        if repair_only && !s.omit_repair_ack {
            current["repair_feedback_receipt"] =
                json!({"operation_id":body["operation_id"],"recorded":true});
        } else if !repair_only
            && (body["status"] == "no_change"
                || (body["reviewed_sources"]
                    .as_array()
                    .is_some_and(|s| !s.is_empty())
                    && current["research"]["repair_feedback"]["phase"] != "candidate_validation"))
        {
            current["research"]["repair_feedback"] = Value::Null;
        }
        if let Some(job) = s
            .research_jobs
            .iter_mut()
            .find(|job| job["subject_ref"] == current["research"]["subject_ref"])
        {
            *job = current["research"].clone();
        }
        s.current_admission = Some(current.clone());
        let delay = if body["status"] == "waiting" {
            s.research_waiting_delay
        } else {
            Duration::ZERO
        };
        (current, delay)
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    Json(json!({"data":current})).into_response()
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

fn comparison() -> Value {
    json!({"pointer":{"item_id":"2026-09-07/1","candidate_hash":"a".repeat(64),
        "run_entry_ref":RUN,"run_version":1},"subject_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce7",
        "path":"derived/entities/overview.md","title":"Existing overview","content":"Comparison body only.",
        "sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}]})
}

fn one_step_behavior(step: &Value) -> String {
    format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{step}
JSON
 ;;
 *) exit 99;;
esac
"#
    )
}

#[tokio::test]
async fn successful_probe_ignores_old_limit_events_and_source_text() {
    let behavior = format!(
        r#"
if [ "$OUTPUT_NAME" = 'probe-answer.md' ]; then
 cat <<'EVENTS'
{{"type":"error","message":"You've hit your usage limit."}}
{{"type":"item.completed","item":{{"type":"agent_message","text":"READY; quota 429 appears only in source content"}}}}
{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":1}}}}
EVENTS
 exit 0
fi
{HAPPY}
"#
    );
    let (shared, dreamer, _dir) = build(&behavior).await;
    enable(&shared);
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert!(report.model_failures.is_empty());
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn failed_probe_classifies_terminal_error_and_persists_safe_diagnostic() {
    let behavior = r#"
cat <<'EVENTS'
{"type":"item.completed","item":{"type":"mcp_tool_call","result":{"content":[{"text":"PRIVATE SOURCE quota 429 Bearer SECRET"}]}}}
{"type":"turn.failed","error":{"message":"unexpected status 502 Bad Gateway: Bearer SECRET url: https://private.invalid/?token=SECRET"}}
EVENTS
exit 1
"#;
    let (shared, dreamer, _dir) = build(behavior).await;
    enable(&shared);
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
    assert_eq!(report.model_failures.len(), 1);
    let failure = &report.model_failures[0];
    assert_eq!(failure.stage, "probe");
    assert_eq!(failure.failure.http_status, Some(502));
    assert_eq!(failure.failure.exit_code, Some(1));
    assert_ne!(
        failure.failure.kind,
        brunn::dreamer::codex::FailureKind::UsageLimit
    );
    let state = shared.lock().unwrap();
    assert_eq!(state.runs.len(), 1);
    let detail = state.runs[0]["detail"].as_str().unwrap();
    assert!(detail.contains("probe"));
    assert!(detail.contains("502"));
    for private in [
        "PRIVATE SOURCE",
        "Bearer",
        "SECRET",
        "private.invalid",
        "429",
    ] {
        assert!(!detail.contains(private), "leaked {private}");
        assert!(!serde_json::to_string(&report).unwrap().contains(private));
    }
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
}

#[tokio::test]
async fn research_timeout_then_throttle_retains_both_diagnostics_and_retries() {
    use brunn::dreamer::codex::FailureKind;
    let behavior = r#"
if [ "$OUTPUT_NAME" = 'probe-answer.md' ]; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) sleep 10;;
 research-2-1-answer.md)
 cat <<'EVENTS'
{"type":"item.completed","item":{"type":"reasoning","text":"PRIVATE REASONING quota 429"}}
{"type":"error","message":"You've hit your usage limit."}
{"type":"turn.failed","error":{"message":"unexpected status 429 Too Many Requests: private request payload"}}
EVENTS
 exit 1;;
 research-2-2-answer.md)
 cat > "$OUTPUT_PATH" <<'JSON'
{"schema":"dream.research.step.v1","action":"yield","findings":["Required source is temporarily unavailable; retain the work."]}
JSON
 ;;
 *) exit 99;;
esac
"#;
    let (shared, dreamer, dir) = build_with_budget(behavior, Duration::from_secs(6)).await;
    enable(&shared);
    {
        let mut state = shared.lock().unwrap();
        state.research_enabled = true;
        state.research_jobs = vec![job(SOURCE), job("entry:other")];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.model_failures.len(), 2, "{report:?}");
    assert_eq!(report.model_failures[0].stage, "research-1-1-answer.md");
    assert_eq!(report.model_failures[0].failure.kind, FailureKind::Timeout);
    assert!(report.model_failures[0].elapsed_ms >= 2_000);
    assert_eq!(report.model_failures[1].stage, "research-2-1-answer.md");
    assert_eq!(
        report.model_failures[1].failure.kind,
        FailureKind::RateLimited
    );
    assert_eq!(report.model_failures[1].failure.http_status, Some(429));
    assert_eq!(report.model_failures[1].failure.exit_code, Some(1));
    assert_ne!(report.research["stop_reason"], "account_limits");
    assert_eq!(report.research["new_review_items"], 0);
    assert_eq!(report.research["processed_inputs"], 0);
    assert!(
        std::fs::read_to_string(dir.path().join("calls"))
            .unwrap()
            .contains("research-2-2-answer.md")
    );
    let state = shared.lock().unwrap();
    let detail = state.runs[0]["detail"].as_str().unwrap();
    for expected in ["research-1-1-answer.md", "research-2-1-answer.md", "429"] {
        assert!(detail.contains(expected), "missing {expected}: {detail}");
    }
    for private in ["PRIVATE REASONING", "private request payload", "You've hit"] {
        assert!(!detail.contains(private));
    }
    assert!(state.submitted.is_empty());
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
}

#[tokio::test]
async fn source_research_forwards_exact_coverage_and_duplicate_retirement_pointers() {
    let covering = comparison();
    let mut duplicate = covering.clone();
    duplicate["pointer"]["item_id"] = json!("2026-09-07/2");
    duplicate["pointer"]["candidate_hash"] = json!("b".repeat(64));
    duplicate["subject_ref"] = json!(SOURCE);
    duplicate["retirable"] = json!(true);
    let step = json!({"schema":"dream.research.step.v1","action":"done",
        "covers_existing":covering["pointer"],"supersedes_existing":duplicate["pointer"],
        "reviewed_sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
        "processed_inputs":[{"entry_ref":SOURCE,"version":2,"generation":17}],
        "findings":["The entire duplicate draft is supported and covered by the existing overview."]});
    let (shared, dreamer, _dir) = build(&one_step_behavior(&step)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE)];
        s.research_comparisons = vec![covering.clone(), duplicate.clone()];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    let s = shared.lock().unwrap();
    assert!(s.submitted.is_empty());
    assert_eq!(s.research_progress.len(), 1);
    assert_eq!(
        s.research_progress[0]["covers_existing"],
        covering["pointer"]
    );
    assert_eq!(
        s.research_progress[0]["supersedes_existing"],
        duplicate["pointer"]
    );
    assert_eq!(
        s.research_progress[0]["processed_inputs"],
        step["processed_inputs"]
    );
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn source_research_yields_enrichment_without_consuming_its_origin() {
    let covering = comparison();
    let follow_up = json!({"comparison":covering["pointer"],
        "origin_input":{"entry_ref":SOURCE,"version":2,"generation":17},"targets":[SOURCE]});
    let step = json!({"schema":"dream.research.step.v1","action":"yield","follow_up":follow_up,
        "reviewed_sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
        "findings":["Retain this contribution for the existing overview."]});
    let (shared, dreamer, _dir) = build(&one_step_behavior(&step)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE)];
        s.research_comparisons = vec![covering];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    let s = shared.lock().unwrap();
    assert!(s.submitted.is_empty());
    assert_eq!(s.research_progress.len(), 1);
    assert_eq!(s.research_progress[0]["status"], "waiting");
    assert_eq!(s.research_progress[0]["follow_up"], follow_up);
    assert_eq!(s.research_progress[0]["processed_inputs"], json!([]));
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn source_research_admits_routed_references_before_the_first_model_turn() {
    let target = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let step = candidate_step();
    let (shared, dreamer, dir) = build(&one_step_behavior(&step)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        let mut research = job(SOURCE);
        research["routed_targets"] = json!([target]);
        research["routed_work"] = json!([{"origin_input":{"entry_ref":SOURCE,"version":2,"generation":17},
            "targets":[target],"status":"pending"}]);
        s.research_jobs = vec![research];
        s.narrative_context = vec![
            job(SOURCE)["sources"][0].clone(),
            json!({"entry_ref":target,"version":1,"generation":16,"path":"sources/Project/Routed.md"}),
        ];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.research["routed_discoveries"], 1);
    assert_eq!(report.research["rounds"], 1);
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 1);
    assert_eq!(s.narrative_discoveries[0]["queries"], json!([]));
    assert_eq!(s.narrative_discoveries[0]["targets"], json!([target]));
    let prompt = std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    assert!(prompt.contains("sources/Project/Routed.md"));
    assert!(prompt.contains("routed_work"));
}

#[tokio::test]
async fn source_research_does_not_spin_on_an_unavailable_routed_reference() {
    let target = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let step = json!({"schema":"dream.research.step.v1","action":"yield",
        "findings":["Routed evidence is still unavailable; preserve this input."]});
    let (shared, dreamer, dir) = build(&one_step_behavior(&step)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        let mut research = job(SOURCE);
        research["routed_targets"] = json!([target]);
        research["routed_work"] = json!([{"origin_input":{"entry_ref":SOURCE,"version":2,"generation":17},
            "targets":[target],"status":"pending"}]);
        s.research_jobs = vec![research];
        // Discovery deliberately does not admit the unavailable target and
        // leaves routed_targets present in the returned server snapshot.
        s.narrative_context = vec![job(SOURCE)["sources"][0].clone()];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.research["routed_discoveries"], 1);
    assert_eq!(report.research["rounds"], 1);
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 1);
    assert!(s.submitted.is_empty());
    assert_eq!(s.research_progress[0]["processed_inputs"], json!([]));
    let prompt = std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    let input: Value = serde_json::from_str(prompt.split("INPUT:\n").nth(1).unwrap()).unwrap();
    assert_eq!(input["research"]["routed_targets"], json!([target]));
    assert_eq!(
        input["research"]["routed_work"][0]["origin_input"]["entry_ref"],
        SOURCE
    );
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn source_research_follows_missing_primary_reference_then_submits_in_same_run() {
    let discovery = json!({"schema":"dream.research.step.v1","action":"discover","targets":["sources/Project/Outcome.md"]});
    let submit = candidate_step();
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) sleep 2; cat > "$OUTPUT_PATH" <<'JSON'
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
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(12)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE)];
        s.research_jobs[0]["notes"] =
            json!("CORPUS_TIME_HINT: 999999 seconds remain according to an old note.");
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
    let remaining_seconds = |prompt: &str| -> u64 {
        let (instructions, input) = prompt.split_once("\nINPUT:\n").unwrap();
        assert!(!instructions.contains("CORPUS_TIME_HINT"));
        assert!(input.contains("CORPUS_TIME_HINT"));
        instructions
            .lines()
            .find_map(|line| line.strip_prefix("At invocation start, approximately "))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let first_prompt =
        std::fs::read_to_string(dir.path().join("prompt-research-1-1-answer.md")).unwrap();
    let first_remaining = remaining_seconds(&first_prompt);
    let second_remaining = remaining_seconds(&prompt);
    assert!((1..=6).contains(&first_remaining));
    assert!(second_remaining < first_remaining);
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
    let s = shared.lock().unwrap();
    assert_eq!(s.research_progress.len(), 3);
    assert!(
        s.research_progress[..2]
            .iter()
            .all(
                |body| body["repair_feedback"]["phase"] == "response_validation"
                    && body["processed_inputs"] == json!([])
            )
    );
    assert_eq!(s.research_progress[1]["status"], "waiting");
    assert_eq!(s.research_progress[2]["subject_ref"], second);
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
async fn selection_margin_stops_the_geometric_subject_tail_before_another_rpc() {
    let behavior = r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
exec sleep 30
"#;
    let (shared, dreamer, dir) = build_with_budget(behavior, Duration::from_secs(6)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = (0..16)
            .map(|_| job(&format!("entry:{}", uuid::Uuid::now_v7())))
            .collect();
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.research["stop_reason"], "time_exhausted");
    assert_eq!(report.research["subjects_completed"], 0);
    let rounds = report.research["rounds"].as_u64().unwrap() as usize;
    // Halving the remaining time crosses a fixed 10% entry margin after four
    // subjects. Recomputing that margin each turn would keep selecting jobs.
    assert!((2..=4).contains(&rounds), "{report:?}");
    assert_eq!(report.research["subjects_yielded"], rounds);
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|name| name.starts_with("research-"))
            .count(),
        rounds
    );
    let s = shared.lock().unwrap();
    assert_eq!(
        s.research_next_count, rounds,
        "no extra selection in the remaining tail"
    );
    assert_eq!(s.research_progress.len(), rounds);
    assert!(
        s.research_progress
            .iter()
            .all(|body| body["status"] == "waiting")
    );
    assert_eq!(s.runs[0]["expected_state_version"], s.state_version);
    assert_eq!(s.runs[0]["research"]["stop_reason"], "time_exhausted");
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
}

#[tokio::test]
async fn selection_margin_merges_slow_selection_without_starting_or_yielding_a_subject() {
    // The usable budget is 5.4s and its fixed margin is about 0.54s. A 5s
    // acknowledged selection crosses the margin but remains before timeout.
    for exhausted in [false, true] {
        let behavior = r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
exit 99
"#;
        let (shared, dreamer, dir) = build_with_budget(behavior, Duration::from_secs(6)).await;
        enable(&shared);
        let mut selected = job(SOURCE);
        selected["notes"] =
            json!("Previously checked evidence stays available for the next attempt.");
        {
            let mut s = shared.lock().unwrap();
            s.research_enabled = true;
            s.research_next_delay = Duration::from_secs(5);
            if !exhausted {
                s.research_jobs = vec![selected.clone()];
            }
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        if exhausted {
            assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
            assert_eq!(report.research["stop_reason"], "selected_work_complete");
        } else {
            assert!(
                matches!(report.outcome, RunOutcome::Partial { .. }),
                "{report:?}"
            );
            assert_eq!(report.research["stop_reason"], "time_exhausted");
        }
        for counter in [
            "rounds",
            "subjects_completed",
            "subjects_yielded",
            "processed_inputs",
            "new_review_items",
        ] {
            assert_eq!(report.research[counter], 0, "{counter}: {report:?}");
        }
        let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
        assert!(!calls.lines().any(|name| name.starts_with("research-")));
        let s = shared.lock().unwrap();
        assert_eq!(s.research_next_count, 1);
        assert!(
            s.research_progress.is_empty(),
            "no waiting mutation for a subject never started"
        );
        assert!(s.submitted.is_empty());
        assert!(s.narrative_discoveries.is_empty());
        assert_eq!(
            s.runs[0]["expected_state_version"], s.state_version,
            "finish uses acknowledged selection version"
        );
        if !exhausted {
            assert_eq!(s.current_admission.as_ref().unwrap()["research"], selected);
        }
        assert_eq!(report.auth_persistence, "verified");
        assert_eq!(report.receipt_persistence, "accepted");
    }
}

#[tokio::test]
async fn selection_margin_does_not_relabel_an_uncertain_checkpoint_as_exhaustion() {
    let behavior = r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
exec sleep 30
"#;
    let (shared, dreamer, dir) = build_with_budget(behavior, Duration::from_secs(6)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![
            job(SOURCE),
            job("entry:019fba27-687b-7582-8b99-e9371dbe2ce6"),
        ];
        // Persist the forced waiting operation, then delay its acknowledgement
        // past the deadline so the client cannot know that it committed.
        s.research_waiting_delay = Duration::from_secs(30);
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    let RunOutcome::Partial { detail } = &report.outcome else {
        panic!("{report:?}");
    };
    assert!(
        detail.contains("research continuation could not be checkpointed"),
        "{detail}"
    );
    assert!(detail.contains("research-progress timed out"), "{detail}");
    assert!(detail.contains("retained for reconciliation"), "{detail}");
    assert_eq!(report.research["stop_reason"], "operation_failed");
    assert_eq!(report.research["rounds"], 1);
    assert_eq!(
        report.research["subjects_yielded"], 0,
        "the checkpoint was not acknowledged"
    );
    assert_eq!(report.research["subjects_completed"], 0);
    assert!(dir.path().join("prompt-research-1-1-answer.md").exists());
    assert!(!dir.path().join("prompt-research-2-1-answer.md").exists());
    let s = shared.lock().unwrap();
    assert_eq!(s.research_next_count, 1);
    assert_eq!(s.research_progress.len(), 1);
    assert_eq!(s.research_progress[0]["status"], "waiting");
    assert_eq!(
        s.current_admission.as_ref().unwrap()["research"]["status"],
        "waiting"
    );
    assert_eq!(s.runs[0]["research"]["stop_reason"], "operation_failed");
    assert_eq!(
        s.runs[0]["expected_state_version"],
        s.research_progress[0]["expected_state_version"]
    );
    assert_ne!(
        s.runs[0]["expected_state_version"], s.state_version,
        "an uncertain acknowledgement must not advance local state"
    );
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

fn evidence_discovery() -> Value {
    json!({"schema":"dream.research.step.v1","action":"discover",
        "queries":["Project current outcome"],"targets":["sources/Project/Outcome.md"],
        "notes":"REJECTED_CHECKPOINT_NOTES",
        "reviewed_sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
        "pending_queries":["Unfinished project lead"],
        "pending_targets":["sources/Project/Unfinished.md"],
        "processed_inputs":[],"findings":["REJECTED_CHECKPOINT_FINDING"]})
}

fn refresh_error(status: StatusCode, code: &str) -> Option<(StatusCode, String)> {
    Some((
        status,
        json!({"error":{"code":code,"message":"Refresh the exact admitted evidence."}}).to_string(),
    ))
}

fn refreshed_sources(version: i64) -> Vec<Value> {
    vec![
        job(SOURCE)["sources"][0].clone(),
        json!({"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce7",
            "path":"sources/Project/Refreshed.md","version":version,"generation":17+version}),
    ]
}

fn scripted_steps(steps: &[(&str, Value)]) -> String {
    let mut behavior = String::from(
        "if grep -q 'single word READY' \"$DIR/prompt\"; then echo READY; exit 0; fi\ncase \"$OUTPUT_NAME\" in\n",
    );
    for (name, step) in steps {
        behavior.push_str(&format!(
            " {name}) cat > \"$OUTPUT_PATH\" <<'JSON'\n{step}\nJSON\n ;;\n"
        ));
    }
    behavior.push_str(" *) exit 99;;\nesac\n");
    behavior
}

fn assert_discovery_only(discovery: &Value, rejected: &Value, step: &Value) {
    let expected = json!({
        "attempt_id":rejected["attempt_id"],"fence":rejected["fence"],
        "expected_state_version":rejected["expected_state_version"],
        "operation_id":discovery["operation_id"],"subject_ref":rejected["subject_ref"],
        "research_version":rejected["research_version"],
        "queries":step["queries"],"targets":step["targets"]
    });
    assert_eq!(
        discovery, &expected,
        "refresh sends only the fenced discovery request"
    );
    assert_operation(discovery);
    assert_ne!(discovery["operation_id"], rejected["operation_id"]);
}

#[tokio::test]
async fn typed_checkpoint_rejections_refresh_without_saving_or_consuming_rejected_work() {
    for (status, code) in [
        (StatusCode::BAD_REQUEST, "research_refresh_required"),
        (StatusCode::CONFLICT, "dreamer_source_changed"),
    ] {
        let discover = evidence_discovery();
        let behavior = scripted_steps(&[
            ("research-1-1-answer.md", discover.clone()),
            (
                "research-1-2-answer.md",
                json!({"schema":"dream.research.step.v1","action":"yield"}),
            ),
        ]);
        let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
        enable(&shared);
        {
            let mut s = shared.lock().unwrap();
            s.research_enabled = true;
            s.research_jobs = vec![job(SOURCE)];
            s.research_progress_replies
                .push_back(refresh_error(status, code));
            s.narrative_context = refreshed_sources(3);
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert!(
            matches!(report.outcome, RunOutcome::Partial { .. }),
            "{code}: {report:?}"
        );
        assert_eq!(report.research["rounds"], 2, "{code}");
        assert_eq!(report.research["processed_inputs"], 0);
        assert_eq!(report.research["new_review_items"], 0);
        let s = shared.lock().unwrap();
        assert_eq!(s.narrative_discoveries.len(), 1);
        assert_eq!(s.research_progress.len(), 3);
        assert_eq!(
            s.research_progress[1]["repair_feedback"]["phase"],
            "checkpoint_validation"
        );
        assert!(s.research_progress[1].get("notes").is_none());
        assert_eq!(s.research_progress[0]["notes"], "REJECTED_CHECKPOINT_NOTES");
        assert_eq!(s.research_progress[0]["findings"], discover["findings"]);
        assert_discovery_only(
            &s.narrative_discoveries[0],
            &s.research_progress[0],
            &discover,
        );
        assert!(s.submitted.is_empty());
        assert!(s.notifications.is_empty());
        assert!(
            s.research_progress
                .iter()
                .all(|request| request["processed_inputs"] == json!([]))
        );
        let prompt =
            std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
        assert!(prompt.contains("previous research checkpoint was not saved"));
        assert!(prompt.contains("Reread the refreshed sources"));
        assert!(!prompt.contains("REJECTED_CHECKPOINT_NOTES"));
        assert!(!prompt.contains("REJECTED_CHECKPOINT_FINDING"));
        let input: Value =
            serde_json::from_str(prompt.split("\nINPUT:\n").nth(1).unwrap().trim()).unwrap();
        assert_eq!(input["research"]["sources"], json!(refreshed_sources(3)));
        assert_eq!(input["research"]["version"], 3);
        assert_eq!(input["research"]["notes"], "");
        assert_eq!(input["research"]["reviewed_sources"], json!([]));
        assert_eq!(input["inputs"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn unrelated_checkpoint_errors_never_execute_requested_discovery() {
    let mut cases = vec![
        (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid selectors",
        ),
        (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "a candidate source is missing or inaccessible",
        ),
        (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "reviewed selectors must resolve in admitted research evidence",
        ),
        (
            StatusCode::UNAUTHORIZED,
            "research_refresh_required",
            "not authenticated",
        ),
        (
            StatusCode::FORBIDDEN,
            "research_refresh_required",
            "not authorized",
        ),
        (
            StatusCode::CONFLICT,
            "dreamer_attempt_conflict",
            "attempt fence changed",
        ),
        (
            StatusCode::CONFLICT,
            "dreamer_research_conflict",
            "research fence changed",
        ),
        (
            StatusCode::CONFLICT,
            "dreamer_state_conflict",
            "state fence changed",
        ),
        (
            StatusCode::CONFLICT,
            "research_refresh_required",
            "wrong status for code",
        ),
        (
            StatusCode::BAD_REQUEST,
            "dreamer_source_changed",
            "wrong status for code",
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "research_refresh_required",
            "server failure",
        ),
    ]
    .into_iter()
    .map(|(status, code, message)| {
        (
            status,
            json!({"error":{"code":code,"message":message}}).to_string(),
        )
    })
    .collect::<Vec<_>>();
    cases.push((StatusCode::BAD_REQUEST, "{malformed response".into()));
    let behavior = scripted_steps(&[("research-1-*-answer.md", evidence_discovery())]);
    for (status, body) in cases {
        let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
        enable(&shared);
        {
            let mut s = shared.lock().unwrap();
            s.research_enabled = true;
            s.research_jobs = vec![job(SOURCE)];
            s.research_progress_replies =
                VecDeque::from([Some((status, body.clone())), Some((status, body.clone()))]);
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_eq!(report.research["rounds"], 2, "{status} {body}: {report:?}");
        assert_eq!(report.research["subjects_yielded"], 1);
        let s = shared.lock().unwrap();
        assert!(s.narrative_discoveries.is_empty(), "{status} {body}");
        assert!(s.submitted.is_empty());
        assert_eq!(s.research_progress.len(), 3);
        assert_eq!(s.research_progress[2]["status"], "waiting");
        assert!(
            s.research_progress
                .iter()
                .all(|v| v["processed_inputs"] == json!([]))
        );
        let prompt =
            std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
        assert!(!prompt.contains("Discovery has refreshed the admitted evidence"));
    }
}

#[tokio::test]
async fn repeated_refresh_rejections_yield_even_when_discovery_succeeds_and_next_subject_runs() {
    let second = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let discover = evidence_discovery();
    let behavior = scripted_steps(&[
        ("research-1-*-answer.md", discover.clone()),
        (
            "research-2-1-answer.md",
            json!({"schema":"dream.research.step.v1","action":"done",
            "reviewed_sources":[{"entry_ref":second,"version":2,"start_line":1,"end_line":2}],
            "findings":["The second subject needs no new overview."]}),
        ),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE), job(second)];
        s.research_progress_replies = VecDeque::from([
            refresh_error(StatusCode::BAD_REQUEST, "research_refresh_required"),
            refresh_error(StatusCode::CONFLICT, "dreamer_source_changed"),
        ]);
        s.research_discovery_sources = VecDeque::from([refreshed_sources(3), refreshed_sources(4)]);
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["rounds"], 3, "{report:?}");
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert!(!dir.path().join("prompt-research-1-3-answer.md").exists());
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 2);
    for index in 0..2 {
        assert_discovery_only(
            &s.narrative_discoveries[index],
            &s.research_progress[index * 2],
            &discover,
        );
    }
    assert_eq!(s.research_progress.len(), 5);
    assert_eq!(s.research_progress[3]["status"], "waiting");
    assert_eq!(
        s.research_progress[3]["repair_feedback"]["phase"],
        "checkpoint_validation"
    );
    assert_eq!(s.research_progress[4]["subject_ref"], second);
    assert!(s.submitted.is_empty());
}

#[tokio::test]
async fn accepted_evidence_checkpoint_resets_refresh_rejection_budget() {
    let discover = evidence_discovery();
    let behavior = scripted_steps(&[
        (
            "research-1-5-answer.md",
            json!({"schema":"dream.research.step.v1","action":"yield"}),
        ),
        ("research-1-*-answer.md", discover),
    ]);
    let (shared, dreamer, dir) = build_with_budget(&behavior, Duration::from_secs(8)).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        s.research_jobs = vec![job(SOURCE)];
        s.research_progress_replies = VecDeque::from([
            refresh_error(StatusCode::BAD_REQUEST, "research_refresh_required"),
            None,
            refresh_error(StatusCode::CONFLICT, "dreamer_source_changed"),
            None,
        ]);
        s.research_discovery_sources = (3..=6).map(refreshed_sources).collect();
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["rounds"], 5, "{report:?}");
    assert!(dir.path().join("prompt-research-1-5-answer.md").exists());
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 4);
    assert_eq!(s.research_progress.len(), 7);
    assert_eq!(
        s.research_progress
            .iter()
            .filter(|body| body.get("repair_feedback").is_some())
            .count(),
        2
    );
    assert!(s.submitted.is_empty());
}

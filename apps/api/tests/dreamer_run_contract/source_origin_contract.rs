//! Real subprocess/HTTP runner contracts; primary and comparison text is synthetic.
use super::*;

const PROTOCOL: &str = "dream.research.follow_up.v1";
const DESTINATION: &str = "entry:019fba27-687b-7582-8b99-e9371dbe2ce7";

fn route() -> Value {
    json!({"route_id":"019fba27-687b-7582-8b99-e9371dbe2ce8","revision":2,
        "comparison":comparison()["pointer"]})
}

fn source(reference: &str) -> Value {
    json!({"entry_ref":reference,"version":2,"start_line":1,"end_line":2})
}

fn destination_job() -> Value {
    let mut value = job(DESTINATION);
    value["follow_up_protocol"] = json!(PROTOCOL);
    value["sources"]
        .as_array_mut()
        .unwrap()
        .push(job(SOURCE)["sources"][0].clone());
    value["routed_work"] = json!([{"route":route(),"comparison":comparison()["pointer"],
        "origin_source":{"entry_ref":SOURCE,"version":2},"current_source":{"entry_ref":SOURCE,"version":2},
        "targets":[SOURCE],"status":"pending"}]);
    value
}

fn submit() -> Value {
    let mut step = candidate_step();
    step["candidates"][0]["subject_ref"] = json!(DESTINATION);
    step["candidates"][0]["sources"] = json!([source(DESTINATION), source(SOURCE)]);
    step["candidates"][0]["content"] =
        json!("The current project includes the reviewed equipment detail.[^s1][^s2]");
    step["reviewed_sources"] = json!([source(DESTINATION), source(SOURCE)]);
    step["processed_inputs"] = json!([]);
    step["findings"] =
        json!(["The exact source detail is incorporated in this existing overview."]);
    step["resolved_follow_ups"] = json!([route()]);
    step
}

fn program(first: &Value, second: Option<&Value>) -> String {
    let mut script = format!(
        "if [ \"$OUTPUT_NAME\" = 'probe-answer.md' ]; then echo READY; exit 0; fi\ncase \"$OUTPUT_NAME\" in\n research-1-1-answer.md) cat > \"$OUTPUT_PATH\" <<'SOURCE_ROUTE_JSON'\n{first}\nSOURCE_ROUTE_JSON\n ;;\n"
    );
    if let Some(second) = second {
        script.push_str(&format!(" research-2-1-answer.md) cat > \"$OUTPUT_PATH\" <<'SOURCE_ROUTE_JSON'\n{second}\nSOURCE_ROUTE_JSON\n ;;\n"));
    }
    script.push_str(" *) exit 99;;\nesac\n");
    script
}

pub(super) fn acknowledge(state: &mut Mock, body: &Value, current: &mut Value) {
    current.as_object_mut().unwrap().remove("follow_up_receipt");
    let progress = body.get("research_progress").unwrap_or(body);
    if progress["follow_up_protocol"] != PROTOCOL {
        return;
    }
    let accepted = body["candidates"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
        || progress["status"] == "no_change";
    let mut ack = json!({"protocol":PROTOCOL,"operation_id":body["operation_id"],"recorded":true,
        "resolved_follow_ups":if accepted {progress.get("resolved_follow_ups").cloned().unwrap_or(json!([]))} else {json!([])},
        "replayed":false});
    if let Some(patch) = state.research_follow_up_replies.pop_front() {
        let Some(fields) = patch.as_object() else {
            return;
        };
        ack.as_object_mut().unwrap().extend(fields.clone());
    }
    current["follow_up_receipt"] = ack;
}

#[tokio::test]
async fn source_origin_yield_and_destination_submit_cross_http_without_input_events() {
    let first = json!({"schema":"dream.research.step.v1","action":"yield",
        "notes":"A reviewed equipment detail belongs in the other overview.","reviewed_sources":[source(SOURCE)],
        "findings":["The reviewed primary record adds equipment detail to the existing overview."],
        "follow_up":{"comparison":comparison()["pointer"],"origin_source":{"entry_ref":SOURCE,"version":2},"targets":[SOURCE]}});
    let (shared, dreamer, _dir) =
        build_with_budget(&program(&first, Some(&submit())), Duration::from_secs(8)).await;
    enable(&shared);
    {
        let mut state = shared.lock().unwrap();
        state.research_enabled = true;
        state.research_inputs = Some(vec![]);
        let mut origin = job(SOURCE);
        origin["follow_up_protocol"] = json!(PROTOCOL);
        state.research_jobs = vec![origin, destination_job()];
        state.research_comparisons = vec![comparison()];
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.research["new_review_items"], 1, "{report:?}");
    assert_eq!(report.research["processed_inputs"], 0);
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    let state = shared.lock().unwrap();
    let request = state
        .research_progress
        .iter()
        .find(|request| request["follow_up"].is_object())
        .unwrap();
    assert_eq!(request["follow_up_protocol"], PROTOCOL);
    assert!(request["follow_up"].get("origin_input").is_none());
    assert_eq!(request["processed_inputs"], json!([]));
    assert_eq!(state.submitted.len(), 1);
    let submission = &state.submitted[0];
    assert_eq!(submission["processed_inputs"], json!([]));
    assert_eq!(
        submission["research_progress"]["follow_up_protocol"],
        PROTOCOL
    );
    assert_eq!(
        submission["research_progress"]["resolved_follow_ups"],
        json!([route()])
    );
    assert!(submission.get("resolved_follow_ups").is_none());
    assert!(state.narrative_discoveries.is_empty());
    assert_eq!(
        state.current_admission.as_ref().unwrap()["inputs"],
        json!([])
    );
}

#[tokio::test]
async fn source_route_completion_requires_matching_typed_operation_acknowledgement() {
    for patch in [
        Value::Null,
        json!({"operation_id":"different-operation"}),
        json!({"protocol":"other"}),
        json!({"recorded":false}),
        json!({"resolved_follow_ups":[]}),
        json!({"replayed":"false"}),
    ] {
        let (shared, dreamer, dir) =
            build_with_budget(&program(&submit(), None), Duration::from_secs(5)).await;
        enable(&shared);
        {
            let mut state = shared.lock().unwrap();
            state.research_enabled = true;
            state.research_inputs = Some(vec![]);
            state.research_jobs = vec![destination_job()];
            state.research_follow_up_replies.push_back(patch.clone());
        }
        let report = dreamer.run_once(today(), RunKind::Manual).await;
        assert_eq!(
            report.research["stop_reason"], "operation_failed",
            "{patch}: {report:?}"
        );
        assert_eq!(report.research["processed_inputs"], 0);
        assert_eq!(report.research["subjects_completed"], 0);
        assert_eq!(shared.lock().unwrap().submitted.len(), 1);
        assert!(!dir.path().join("prompt-research-1-2-answer.md").exists());
    }
}

//! A new search receipt must not keep a stalled subject running forever.
use super::*;

#[tokio::test]
async fn audit_only_discovery_yields_without_consuming_work_and_next_subject_runs() {
    let second = "entry:019fba27-687b-7582-8b99-e9371dbe2ce6";
    let first_query = "Cedar observatory detector outcome";
    let second_query = "Cedar observatory calibration outcome";
    let discover = |query| {
        json!({"schema":"dream.research.step.v1","action":"discover",
        "queries":[query],"findings":["Check a material later outcome; no input is dispositioned."]})
    };
    let done = json!({"schema":"dream.research.step.v1","action":"done",
        "reviewed_sources":[{"entry_ref":second,"version":2,"start_line":1,"end_line":2}],
        "findings":["The second subject is already covered by its primary source."]});
    let behavior = format!(
        r#"
if [ "$OUTPUT_NAME" = 'probe-answer.md' ]; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 research-1-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{}
JSON
 ;;
 research-1-2-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{}
JSON
 ;;
 research-2-1-answer.md) cat > "$OUTPUT_PATH" <<'JSON'
{done}
JSON
 ;;
 *) exit 99;;
esac
"#,
        discover(first_query),
        discover(second_query)
    );
    let (shared, dreamer, dir) = build(&behavior).await;
    enable(&shared);
    {
        let mut s = shared.lock().unwrap();
        s.research_enabled = true;
        let mut first = job(SOURCE);
        first["notes"] = json!("Previous supported work remains retained.");
        first["reviewed_sources"] =
            json!([{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}]);
        first["discovery_audit"] = json!({"validity":"legacy_or_unknown","last_search":null});
        let headers = first["sources"].as_array().unwrap().clone();
        s.research_jobs = vec![first, job(second)];
        s.research_discovery_sources = vec![headers.clone(), headers].into();
        s.research_discovery_audits = [first_query, second_query]
            .iter()
            .enumerate()
            .map(|(index, query)| {
                json!({"validity":"current","last_search":{
                "schema":"dream.research.discovery.v1","retrieval_policy":2,
                "searched_generation":17 + index,"queries":[query],
                "groups":[
                    {"query_index":0,"sort":"best_match","returned":0,"limit":8,
                     "execution_status":"bounded","output_limit_reached":false},
                    {"query_index":0,"sort":"last_modified","returned":0,"limit":8,
                     "execution_status":"bounded","output_limit_reached":false}]}})
            })
            .collect();
    }
    let report = dreamer.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.research["subjects_yielded"], 1);
    assert_eq!(report.research["subjects_completed"], 1);
    assert_eq!(report.research["processed_inputs"], 0);
    assert!(!dir.path().join("prompt-research-1-3-answer.md").exists());
    assert!(dir.path().join("prompt-research-2-1-answer.md").exists());
    let prompt = std::fs::read_to_string(dir.path().join("prompt-research-1-2-answer.md")).unwrap();
    let input: Value = serde_json::from_str(prompt.split("\nINPUT:\n").nth(1).unwrap()).unwrap();
    assert_eq!(input["research"]["discovery_audit"]["validity"], "current");
    assert_eq!(
        input["research"]["discovery_audit"]["last_search"]["queries"],
        json!([first_query])
    );
    let s = shared.lock().unwrap();
    assert_eq!(s.narrative_discoveries.len(), 2);
    assert_eq!(s.research_progress[0]["status"], "waiting");
    assert!(s.research_progress[0].get("notes").is_none());
    assert!(s.research_progress[0].get("discovery_audit").is_none());
    assert!(s.submitted.is_empty());
    assert_eq!(report.auth_persistence, "verified");
}

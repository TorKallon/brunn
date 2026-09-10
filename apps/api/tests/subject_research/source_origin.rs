//! Synthetic HTTP/database contracts for routing primary evidence without an
//! intake event. These use the real admission, proposal and owner-review paths.
use super::*;

const PROTOCOL: &str = "dream.research.follow_up.v1";
const PROGRESS: &str = "/v1/workspace/dreamer/research-progress";
const CANDIDATES: &str = "/v1/workspace/dreamer/candidates";

struct Scenario {
    destination: Value,
    origin: Value,
    extra: Value,
    later: Value,
    selected: Value,
    destination_item: Value,
    origin_item: Value,
    original_receipt: Value,
}

fn job_path(source: &Value) -> String {
    format!(
        "dreams/research/{}.md",
        source["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    )
}

fn source_identity(source: &Value) -> Value {
    json!({"entry_ref":source["entry_ref"],"version":source["version"]})
}

fn checkpoint_origins(admission: &Value) -> Vec<Value> {
    let mut origins: Vec<_> = admission["research"]["checkpoint_contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|context| context["origin"].clone())
        .collect();
    if admission["research"]["current_checkpoint"].is_object() {
        origins.push(admission["research"]["current_checkpoint"].clone());
    }
    origins
}

fn semantic_progress(admission: &Value, sources: &[&Value], status: &str) -> Value {
    let mut body = progress_body(
        admission,
        sources.iter().map(|source| reviewed(source)).collect(),
        status,
        "The exact primary observations and the retained unfinished detail were reviewed.",
    );
    body["processed_inputs"] = json!([]);
    body["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
    body["reconciled_checkpoints"] = json!(checkpoint_origins(admission));
    body["findings"] = json!([
        "The offered checkpoints were reconciled against these current primary observations; the destination should assess the additional equipment detail."
    ]);
    body
}

fn overview(admission: &Value, sources: &[&Value], revision: Option<&Value>) -> Value {
    let content = format!(
        "# Survey overview\n\n{}",
        sources
            .iter()
            .enumerate()
            .map(|(index, _)| {
                format!(
                    "A checked primary observation is retained.[^s{}]\n",
                    index + 1
                )
            })
            .collect::<String>()
    );
    let mut candidate = json!({
        "kind":"summary","title":"Survey overview",
        "summary":"A bounded survey account with independently checked observations.",
        "reason":"Retain the observations together for owner review.",
        "subject_ref":admission["research"]["subject_ref"],
        "path":admission["research"]["output_path"],
        "expected_version":admission["research"]["output_version"],
        "content":content,"sources":sources.iter().map(|source|reviewed(source)).collect::<Vec<_>>()
    });
    if let Some(item) = revision {
        candidate["revises_item_id"] = item["id"].clone();
    }
    candidate
}

fn submission(
    admission: &Value,
    sources: &[&Value],
    revision: Option<&Value>,
    resolve: Option<Value>,
) -> Value {
    let mut body = research_request(admission);
    body["candidates"] = json!([overview(admission, sources, revision)]);
    body["processed_inputs"] = json!([]);
    body["findings"] = json!(["The current overview includes the reviewed primary observations."]);
    body["research_progress"] = semantic_progress(admission, sources, "waiting");
    if let Some(route) = resolve {
        body["research_progress"]["follow_up_protocol"] = json!(PROTOCOL);
        body["research_progress"]["resolved_follow_ups"] = json!([route]);
    }
    body
}

fn comparison(admission: &Value, item: &Value) -> Value {
    admission["comparison_proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|proposal| proposal["pointer"]["item_id"] == item["id"])
        .expect("the exact overlapping pending proposal is offered")["pointer"]
        .clone()
}

fn offered_route(admission: &Value) -> Value {
    let work = admission["research"]["routed_work"].as_array().unwrap();
    assert_eq!(work.len(), 1, "{admission}");
    let route = work[0]["route"].clone();
    assert!(
        route.is_object(),
        "current actionable source route is explicitly offered: {work:?}"
    );
    Uuid::parse_str(route["route_id"].as_str().unwrap()).unwrap();
    assert!(route["revision"].as_i64().unwrap() > 0);
    assert!(route["comparison"].is_object());
    route
}

async fn state(f: &Fixture) -> Value {
    current(f, "dreams/state.md").await.unwrap().2["dreamer_state"].clone()
}

fn resolution_receipt(response: &Value, request: &Value, resolved: &Value, replayed: bool) {
    let receipt = &response["follow_up_receipt"];
    assert_eq!(receipt["operation_id"], request["operation_id"]);
    assert_eq!(receipt["protocol"], PROTOCOL);
    assert_eq!(receipt["recorded"], true);
    assert_eq!(&receipt["resolved_follow_ups"], resolved);
    assert_eq!(receipt["replayed"], replayed);
}

async fn routes(f: &Fixture) -> Value {
    state(f).await["research"]["follow_ups"].clone()
}

async fn admit_requested(f: &Fixture, references: Vec<Value>) -> Value {
    ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({
            "attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual",
            "lease_seconds":60,"requested_subject_refs":references
        }),
    )
    .await)
}

async fn scenario(f: &Fixture) -> Scenario {
    scenario_with_options(f, false, false).await
}

async fn scenario_with_options(
    f: &Fixture,
    prior_exclusion: bool,
    legacy_destination: bool,
) -> Scenario {
    control(f, "report-only", 0).await;
    let destination = write(
        f,
        "sources/Notes/Survey.md",
        "# Survey\n\nThe survey records a completed observation.\n",
        0,
    )
    .await;
    let mut origin = write(
        f,
        "sources/Notes/Witness.md",
        "# Witness\n\nThe independent witness records the survey outcome.\n",
        0,
    )
    .await;
    let extra = write(
        f,
        "sources/Notes/Instrument.md",
        "# Instrument\n\nThe instrument record retains an equipment qualification.\n",
        0,
    )
    .await;
    let later = write(
        f,
        "sources/Notes/Calibration.md",
        "# Calibration\n\nThe calibration records a separate exposure interval.\n",
        0,
    )
    .await;
    if prior_exclusion {
        // Policy retirement records an excluded retained input. A source that
        // was generated before its first admission is simply never admitted.
        let ordinary = admit(f).await;
        assert!(ordinary["inputs"].as_array().unwrap().iter().any(|input| {
            input["entry_ref"] == origin["entry_ref"] && input["version"] == origin["version"]
        }));
        finish(
            f,
            &ordinary,
            ordinary["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        ok(post(
            f,
            &f.owner,
            "/v1/workspace/write",
            json!({
                "path":"sources/Notes/Witness.md","expected_version":1,
                "content":"# Witness\n\nA generated edition temporarily occupies this identity.\n",
                "metadata":{"kind":"briefing_edition"}
            }),
        )
        .await);
        let initial = admit(f).await;
        assert!(
            !initial["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|input| input["entry_ref"] == origin["entry_ref"])
        );
        assert_eq!(state(f).await["processed_count"], 0);
        let (_, consumed) = submit(
            f,
            &initial,
            initial["state_version"].as_i64().unwrap(),
            vec![],
        )
        .await;
        finish(
            f,
            &initial,
            consumed["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        origin = write(
            f,
            "sources/Notes/Witness.md",
            "# Witness\n\nThe restored ordinary primary records the survey outcome.\n",
            2,
        )
        .await;
        assert!(
            state(f).await["source_dispositions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|disposition| {
                    disposition["entry_ref"] == origin["entry_ref"]
                        && disposition["disposition"] == "excluded_generated_briefing"
                })
        );
    }
    let mut admitted = admit_requested(
        f,
        vec![
            destination["entry_ref"].clone(),
            origin["entry_ref"].clone(),
        ],
    )
    .await;
    // A real completed intake disposition leaves no event to borrow. Research
    // below is selected explicitly from ordinary source headers afterward.
    let (_, consumed) = submit(
        f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        vec![],
    )
    .await;
    assert_eq!(consumed["accepted_candidate_ids"], json!([]));
    admitted["state_version"] = consumed["state_version"].clone();
    let selected = next_subject(f, &admitted).await;
    assert_eq!(selected["inputs"], json!([]));
    assert_eq!(
        selected["research"]["subject_ref"],
        destination["entry_ref"]
    );
    assert_eq!(selected["research"]["follow_up_protocol"], PROTOCOL);
    let (_, selected) = discover_subject(f, &selected, vec![origin["entry_ref"].clone()]).await;
    let mut first_submission = submission(&selected, &[&destination, &origin], None, None);
    if legacy_destination {
        let progress = first_submission["research_progress"]
            .as_object_mut()
            .unwrap();
        progress.remove("checkpoint_protocol");
        progress.remove("reconciled_checkpoints");
    }
    let accepted = ok(post(f, &f.runner, CANDIDATES, first_submission).await);
    let destination_item = review(f).await["items"][0].clone();
    let selected = next_subject(f, &accepted).await;
    assert_eq!(selected["research"]["subject_ref"], origin["entry_ref"]);
    let (_, selected) = discover_subject(f, &selected, vec![extra["entry_ref"].clone()]).await;
    let original_receipt = ok(post(
        f,
        &f.runner,
        CANDIDATES,
        submission(&selected, &[&origin, &extra], None, None),
    )
    .await);
    let origin_item = review(f).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == original_receipt["accepted_candidate_ids"][0])
        .unwrap()
        .clone();
    assert_eq!(original_receipt["inputs"], json!([]));
    Scenario {
        destination,
        origin,
        extra,
        later,
        selected: original_receipt.clone(),
        destination_item,
        origin_item,
        original_receipt,
    }
}

fn routing(s: &Scenario, admission: &Value, targets: &[&Value]) -> Value {
    let mut sources = vec![&s.origin];
    sources.extend_from_slice(targets);
    let mut body = semantic_progress(admission, &sources, "waiting");
    body["follow_up_protocol"] = json!(PROTOCOL);
    body["follow_up"] = json!({
        "comparison":comparison(admission, &s.destination_item),
        "origin_source":source_identity(&s.origin),
        "targets":targets.iter().map(|source|source["entry_ref"].clone()).collect::<Vec<_>>()
    });
    body
}

async fn queue(f: &Fixture, s: &Scenario) -> Value {
    let before = state(f).await;
    let routed =
        ok(post(f, &f.runner, PROGRESS, routing(s, &s.selected, &[&s.extra])).await)["data"]
            .clone();
    assert_eq!(routed["inputs"], json!([]));
    let after = state(f).await;
    assert_eq!(after["processed_count"], before["processed_count"]);
    let durable = &after["research"]["follow_ups"][0];
    assert_eq!(durable["origin_source"], source_identity(&s.origin));
    assert!(
        durable.get("origin_input").is_none(),
        "old readers must fail closed on a new durable origin"
    );
    routed
}

async fn resume_destination(f: &Fixture, s: &Scenario, admission: &Value) -> Value {
    finish(
        f,
        admission,
        admission["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let resumed = next_subject(f, &admit(f).await).await;
    assert_eq!(
        resumed["research"]["subject_ref"],
        s.destination["entry_ref"]
    );
    let targets = resumed["research"]["routed_targets"]
        .as_array()
        .unwrap()
        .clone();
    discover_subject(f, &resumed, targets).await.1
}

fn done(admission: &Value, sources: &[&Value], route: Value) -> Value {
    let mut body = semantic_progress(admission, sources, "no_change");
    body["notes"] = json!("");
    body["follow_up_protocol"] = json!(PROTOCOL);
    body["resolved_follow_ups"] = json!([route]);
    body
}

async fn rejected_unchanged(f: &Fixture, source: &Value, endpoint: &str, body: Value) {
    let before_state = current(f, "dreams/state.md").await.unwrap();
    let before_job = current(f, &job_path(source)).await.unwrap();
    let response = post(f, &f.runner, endpoint, body).await;
    assert!(response.status.is_client_error(), "{}", response.body);
    assert_eq!(current(f, "dreams/state.md").await.unwrap(), before_state);
    assert_eq!(current(f, &job_path(source)).await.unwrap(), before_job);
}

#[tokio::test]
async fn source_origin_empty_intake_enriches_then_retires_only_fully_covered_draft() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let immutable = exact_run_record(&f, &s.original_receipt).await;
    let body = routing(&s, &s.selected, &[&s.extra]);
    let routed = ok(post(&f, &f.runner, PROGRESS, body.clone()).await)["data"].clone();
    let retained = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, PROGRESS, body).await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), retained);
    let other = next_subject(&f, &routed).await;
    for source in [&s.destination, &s.origin] {
        assert_ne!(
            other["research"]["subject_ref"], source["entry_ref"],
            "already served subjects keep the attempt fence"
        );
    }
    let destination = resume_destination(&f, &s, &other).await;
    let route = offered_route(&destination);
    let before = state(&f).await;
    let origin_job = current(&f, &job_path(&s.origin)).await.unwrap().2["dreamer_research"].clone();
    let body = submission(
        &destination,
        &[&s.destination, &s.origin, &s.extra],
        Some(&s.destination_item),
        Some(route),
    );
    let accepted = ok(post(&f, &f.runner, CANDIDATES, body.clone()).await);
    resolution_receipt(
        &accepted,
        &body,
        &body["research_progress"]["resolved_follow_ups"],
        false,
    );
    assert_eq!(
        accepted["accepted_candidate_ids"],
        json!([s.destination_item["id"]])
    );
    assert_eq!(accepted["inputs"], json!([]));
    assert_eq!(routes(&f).await, json!([]));
    assert_eq!(
        state(&f).await["processed_count"],
        before["processed_count"]
    );
    let resumed_job =
        current(&f, &job_path(&s.origin)).await.unwrap().2["dreamer_research"].clone();
    assert_eq!(resumed_job["last_attempt"], origin_job["last_attempt"]);
    assert_eq!(resumed_job["notes"], origin_job["notes"]);
    let due =
        chrono::DateTime::parse_from_rfc3339(resumed_job["retry_at"].as_str().unwrap()).unwrap();
    assert!(due <= Utc::now());
    let accepted_state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, CANDIDATES, body.clone()).await);
    resolution_receipt(
        &replay,
        &body,
        &body["research_progress"]["resolved_follow_ups"],
        true,
    );
    assert_eq!(
        replay["accepted_candidate_ids"],
        accepted["accepted_candidate_ids"]
    );
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        accepted_state
    );
    assert_eq!(
        review(&f).await["items"].as_array().unwrap().len(),
        2,
        "enrichment itself cannot retire either draft"
    );
    // This is ordinary due-job selection, not a synthetic input or another
    // priority queue. A resume cursor at the end may wrap on the next attempt.
    let mut selected = next_subject(&f, &accepted).await;
    if selected["research"].is_null() {
        finish(
            &f,
            &selected,
            selected["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        selected = next_subject(&f, &admit(&f).await).await;
    }
    assert_eq!(selected["research"]["subject_ref"], s.origin["entry_ref"]);
    let covering = comparison(&selected, &s.destination_item);
    let duplicate = comparison(&selected, &s.origin_item);
    let mut retire = semantic_progress(&selected, &[&s.origin, &s.extra], "no_change");
    retire["notes"] = json!("");
    retire["covers_existing"] = covering;
    retire["supersedes_existing"] = duplicate;
    retire["findings"] = json!([
        "Both complete drafts were compared against the primary records. Every useful observation, qualification and boundary in the selected draft is present in the covering overview."
    ]);
    let retired = ok(post(&f, &f.runner, PROGRESS, retire).await)["data"].clone();
    assert_eq!(retired["inputs"], json!([]));
    assert_eq!(
        state(&f).await["processed_count"],
        before["processed_count"]
    );
    let view = review(&f).await;
    assert_eq!(view["items"].as_array().unwrap().len(), 1);
    assert_eq!(view["items"][0]["id"], s.destination_item["id"]);
    assert_eq!(exact_run_record(&f, &s.original_receipt).await, immutable);
}

#[tokio::test]
async fn source_origin_requires_exact_reviewed_primary_and_unambiguous_capability() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    for invalid in [
        "legacy_input",
        "both",
        "neither",
        "wrong_version",
        "unknown_source",
        "unreviewed",
        "missing_protocol",
        "unknown_protocol",
    ] {
        let mut body = routing(&s, &s.selected, &[&s.extra]);
        match invalid {
            "legacy_input" | "both" => {
                body["follow_up"]["origin_input"] = json!({"entry_ref":s.origin["entry_ref"],"version":s.origin["version"],"generation":s.origin["workspace_generation"]});
                if invalid == "legacy_input" {
                    body["follow_up"]
                        .as_object_mut()
                        .unwrap()
                        .remove("origin_source");
                }
            }
            "neither" => {
                body["follow_up"]
                    .as_object_mut()
                    .unwrap()
                    .remove("origin_source");
            }
            "wrong_version" => body["follow_up"]["origin_source"]["version"] = json!(99),
            "unknown_source" => {
                body["follow_up"]["origin_source"]["entry_ref"] = s.later["entry_ref"].clone()
            }
            "unreviewed" => body["reviewed_sources"] = json!([reviewed(&s.extra)]),
            "missing_protocol" => {
                body.as_object_mut().unwrap().remove("follow_up_protocol");
            }
            "unknown_protocol" => {
                body["follow_up_protocol"] = json!("dream.research.follow_up.unknown")
            }
            _ => unreachable!(),
        }
        rejected_unchanged(&f, &s.origin, PROGRESS, body).await;
    }
    let routed = queue(&f, &s).await;
    assert_eq!(routed["inputs"], json!([]));
    assert_eq!(routes(&f).await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn source_origin_ack_is_exact_and_requires_current_primary_review_and_citation() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let destination = resume_destination(&f, &s, &queue(&f, &s).await).await;
    let route = offered_route(&destination);
    for invalid in [
        "revision",
        "route_id",
        "comparison",
        "missing_protocol",
        "unchecked_target",
        "uncited_origin",
        "waiting",
    ] {
        let mut body = submission(
            &destination,
            &[&s.destination, &s.origin, &s.extra],
            Some(&s.destination_item),
            Some(route.clone()),
        );
        let mut endpoint = CANDIDATES;
        match invalid {
            "revision" => {
                body["research_progress"]["resolved_follow_ups"][0]["revision"] =
                    json!(route["revision"].as_i64().unwrap() + 1)
            }
            "route_id" => {
                body["research_progress"]["resolved_follow_ups"][0]["route_id"] =
                    json!(Uuid::now_v7())
            }
            "comparison" => {
                body["research_progress"]["resolved_follow_ups"][0]["comparison"]["candidate_hash"] =
                    json!("0".repeat(64))
            }
            "missing_protocol" => {
                body["research_progress"]
                    .as_object_mut()
                    .unwrap()
                    .remove("follow_up_protocol");
            }
            "unchecked_target" => {
                body["research_progress"]["reviewed_sources"] =
                    json!([reviewed(&s.destination), reviewed(&s.origin)])
            }
            "uncited_origin" => {
                body["candidates"] = json!([overview(
                    &destination,
                    &[&s.destination, &s.extra],
                    Some(&s.destination_item)
                )])
            }
            "waiting" => {
                endpoint = PROGRESS;
                body = done(
                    &destination,
                    &[&s.destination, &s.origin, &s.extra],
                    route.clone(),
                );
                body["status"] = json!("waiting");
            }
            _ => unreachable!(),
        }
        rejected_unchanged(&f, &s.destination, endpoint, body).await;
    }
}

#[tokio::test]
async fn source_origin_revision_without_ack_retains_route_then_current_no_change_resolves() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let destination = resume_destination(&f, &s, &queue(&f, &s).await).await;
    let original_route = offered_route(&destination);
    let before_revision = state(&f).await;
    let accepted = ok(post(
        &f,
        &f.runner,
        CANDIDATES,
        submission(
            &destination,
            &[&s.destination, &s.origin, &s.extra],
            Some(&s.destination_item),
            None,
        ),
    )
    .await);
    assert_eq!(routes(&f).await.as_array().unwrap().len(), 1);
    let current_route = offered_route(&accepted);
    assert_eq!(accepted["checkpoint_receipt"]["subject_complete"], false);
    assert_eq!(
        state(&f).await["research"]["completed"],
        before_revision["research"]["completed"]
    );
    assert_eq!(current_route["route_id"], original_route["route_id"]);
    assert_eq!(current_route["revision"], original_route["revision"]);
    assert_ne!(current_route["comparison"], original_route["comparison"]);
    rejected_unchanged(
        &f,
        &s.destination,
        PROGRESS,
        done(
            &accepted,
            &[&s.destination, &s.origin, &s.extra],
            original_route,
        ),
    )
    .await;
    let before = state(&f).await;
    let body = done(
        &accepted,
        &[&s.destination, &s.origin, &s.extra],
        current_route,
    );
    let completed = ok(post(&f, &f.runner, PROGRESS, body.clone()).await)["data"].clone();
    resolution_receipt(&completed, &body, &body["resolved_follow_ups"], false);
    assert_eq!(completed["research"]["routed_work"], json!([]));
    assert_eq!(completed["inputs"], json!([]));
    assert_eq!(
        state(&f).await["processed_count"],
        before["processed_count"]
    );
    let complete_state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, PROGRESS, body.clone()).await);
    assert_eq!(replay["no_op"], true);
    resolution_receipt(&replay["data"], &body, &body["resolved_follow_ups"], true);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        complete_state
    );
}

#[tokio::test]
async fn source_origin_zero_accepted_ids_never_dispose_the_offered_route() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let destination = resume_destination(&f, &s, &queue(&f, &s).await).await;
    let before = state(&f).await;
    let mut body = submission(
        &destination,
        &[&s.destination, &s.origin, &s.extra],
        Some(&s.destination_item),
        Some(offered_route(&destination)),
    );
    body["candidates"] = json!([]);
    let response = post(&f, &f.runner, CANDIDATES, body.clone()).await;
    assert!(
        response.status == StatusCode::OK || response.status.is_client_error(),
        "{}",
        response.body
    );
    if response.status == StatusCode::OK {
        assert_eq!(response.body["accepted_candidate_ids"], json!([]));
        resolution_receipt(&response.body, &body, &json!([]), false);
        assert_eq!(
            response.body["checkpoint_receipt"]["subject_complete"],
            false
        );
    }
    let after = state(&f).await;
    assert_eq!(
        after["research"]["follow_ups"],
        before["research"]["follow_ups"]
    );
    assert_eq!(after["processed_count"], before["processed_count"]);
    assert_eq!(after["inputs"], before["inputs"]);
    assert_eq!(
        after["research"]["completed"],
        before["research"]["completed"]
    );
}

#[tokio::test]
async fn source_origin_v1_destination_omitted_ack_cannot_advance_completion_counter() {
    let Some(f) = fixture().await else { return };
    let s = scenario_with_options(&f, false, true).await;
    let destination = resume_destination(&f, &s, &queue(&f, &s).await).await;
    let path = job_path(&s.destination);
    assert_eq!(
        current(&f, &path).await.unwrap().2["dreamer_research"]["schema"],
        "dream.research.v1"
    );
    let before = state(&f).await;
    let retained = routes(&f).await;
    let mut body = submission(
        &destination,
        &[&s.destination, &s.origin, &s.extra],
        Some(&s.destination_item),
        None,
    );
    let progress = body["research_progress"].as_object_mut().unwrap();
    progress.remove("checkpoint_protocol");
    progress.remove("reconciled_checkpoints");
    progress.insert("follow_up_protocol".into(), json!(PROTOCOL));
    assert!(!progress.contains_key("resolved_follow_ups"));
    let accepted = ok(post(&f, &f.runner, CANDIDATES, body.clone()).await);
    assert_eq!(
        accepted["accepted_candidate_ids"],
        json!([s.destination_item["id"]])
    );
    resolution_receipt(&accepted, &body, &json!([]), false);
    assert_eq!(routes(&f).await, retained);
    let after = state(&f).await;
    assert_eq!(
        after["research"]["completed"],
        before["research"]["completed"]
    );
    assert_eq!(after["processed_count"], before["processed_count"]);
    assert_eq!(after["inputs"], before["inputs"]);
    let job = current(&f, &path).await.unwrap().2["dreamer_research"].clone();
    assert_eq!(job["schema"], "dream.research.v1");
    assert_eq!(job["status"], "researching");
    assert_eq!(accepted["research"]["status"], "researching");
    assert!(offered_route(&accepted).is_object());
}

#[tokio::test]
async fn source_origin_target_merge_advances_only_the_route_revision_and_retains_targets() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let routed = queue(&f, &s).await;
    let original = routes(&f).await[0].clone();
    let (_, selected) = discover_subject(&f, &routed, vec![s.later["entry_ref"].clone()]).await;
    let body = routing(&s, &selected, &[&s.extra, &s.later]);
    let merged = ok(post(&f, &f.runner, PROGRESS, body.clone()).await)["data"].clone();
    let retained = routes(&f).await;
    assert_eq!(retained.as_array().unwrap().len(), 1);
    assert_eq!(
        retained[0]["route"]["route_id"],
        original["route"]["route_id"]
    );
    assert!(
        retained[0]["route"]["revision"].as_i64().unwrap()
            > original["route"]["revision"].as_i64().unwrap()
    );
    for source in [&s.origin, &s.extra, &s.later] {
        assert!(
            retained[0]["targets"]
                .as_array()
                .unwrap()
                .contains(&source["entry_ref"])
        );
    }
    let snapshot = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(ok(post(&f, &f.runner, PROGRESS, body).await)["no_op"], true);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), snapshot);
    let destination = resume_destination(&f, &s, &merged).await;
    let mut old = offered_route(&destination);
    old["revision"] = original["route"]["revision"].clone();
    rejected_unchanged(
        &f,
        &s.destination,
        PROGRESS,
        done(
            &destination,
            &[&s.destination, &s.origin, &s.extra, &s.later],
            old,
        ),
    )
    .await;
}

#[tokio::test]
async fn source_origin_changed_primary_requires_current_cited_revision_not_stale_no_change() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let routed = queue(&f, &s).await;
    let replacement = write(&f, "sources/Notes/Witness.md", "# Witness\n\nThe independent witness corrects the survey result and retains its uncertainty.\n", 1).await;
    let destination = resume_destination(&f, &s, &routed).await;
    let work = &destination["research"]["routed_work"][0];
    assert_eq!(work["origin_source"], source_identity(&s.origin));
    assert_eq!(work["current_source"], source_identity(&replacement));
    let route = offered_route(&destination);
    rejected_unchanged(
        &f,
        &s.destination,
        PROGRESS,
        done(
            &destination,
            &[&s.destination, &replacement, &s.extra],
            route.clone(),
        ),
    )
    .await;
    let before = state(&f).await;
    let body = submission(
        &destination,
        &[&s.destination, &replacement, &s.extra],
        Some(&s.destination_item),
        Some(route.clone()),
    );
    let accepted = ok(post(&f, &f.runner, CANDIDATES, body.clone()).await);
    assert_eq!(routes(&f).await, json!([]));
    assert_eq!(
        accepted["accepted_candidate_ids"],
        json!([s.destination_item["id"]])
    );
    assert_eq!(
        state(&f).await["processed_count"],
        before["processed_count"]
    );
    assert_eq!(
        accepted["inputs"], destination["inputs"],
        "routing a newer source never consumes its independent newly admitted event"
    );
    let audit_path = format!(
        "dreams/reviews/source-route-{}-{}.md",
        body["operation_id"].as_str().unwrap(),
        route["route_id"].as_str().unwrap()
    );
    let audit = current(&f, &audit_path).await.unwrap();
    let disposition = &audit.2["dreamer_review"]["disposition"];
    assert_eq!(
        disposition["route"]["origin_source"],
        source_identity(&s.origin)
    );
    assert_eq!(disposition["current_source"], source_identity(&replacement));
    assert_eq!(
        disposition["destination"]["item_id"],
        s.destination_item["id"]
    );
    assert_eq!(
        disposition["destination"]["run_entry_ref"],
        accepted["run_entry_ref"]
    );
    assert_eq!(
        disposition["destination"]["run_version"],
        accepted["run_version"]
    );
    assert_eq!(disposition["model_processed"], false);
    ok(post(&f, &f.runner, CANDIDATES, body).await);
    assert_eq!(
        current(&f, &audit_path).await.unwrap(),
        audit,
        "exact replay preserves immutable original-to-current evidence"
    );
}

#[tokio::test]
async fn source_origin_owner_race_rejects_ack_even_with_current_state_version() {
    for choice in ["reject", "defer", "approve", "correct"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let destination = resume_destination(&f, &s, &queue(&f, &s).await).await;
        let mut body = done(
            &destination,
            &[&s.destination, &s.origin, &s.extra],
            offered_route(&destination),
        );
        let view = review(&f).await;
        let decided = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, &s.destination_item, choice),
        )
        .await);
        body["expected_state_version"] = decided["data"]["state_version"].clone();
        rejected_unchanged(&f, &s.destination, PROGRESS, body).await;
        assert_eq!(routes(&f).await.as_array().unwrap().len(), 1, "{choice}");
    }
}

#[tokio::test]
async fn source_origin_withheld_primary_hides_actionable_token_without_discarding_route() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let destination = resume_destination(&f, &s, &queue(&f, &s).await).await;
    let old_route = offered_route(&destination);
    let retained = routes(&f).await;
    let id = Uuid::parse_str(
        s.origin["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap();
    // Fixture-only RLS move: no protected record is read through the runner.
    sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(id)
        .bind(format!(".brunn/tasks/{}.md", Uuid::now_v7()))
        .execute(&f.pool)
        .await
        .unwrap();
    rejected_unchanged(
        &f,
        &s.destination,
        PROGRESS,
        done(
            &destination,
            &[&s.destination, &s.origin, &s.extra],
            old_route,
        ),
    )
    .await;
    let (_, refreshed) = discover_subject(&f, &destination, vec![]).await;
    assert_eq!(routes(&f).await, retained);
    let work = &refreshed["research"]["routed_work"][0];
    assert!(work["route"].is_null());
    assert!(work["origin_source"].is_null());
    assert!(work["current_source"].is_null());
    assert!(
        !work
            .to_string()
            .contains(s.origin["entry_ref"].as_str().unwrap())
    );
    assert!(!work.to_string().contains("Witness"));
}

#[tokio::test]
async fn source_origin_proven_exclusion_retires_without_model_or_input_credit() {
    for loss in ["generated", "deleted"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let routed = queue(&f, &s).await;
        let before = state(&f).await;
        finish(
            &f,
            &routed,
            routed["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        if loss == "generated" {
            ok(post(
                &f,
                &f.owner,
                "/v1/workspace/write",
                json!({
                    "path":"sources/Notes/Witness.md","expected_version":1,
                    "content":"# Witness\n\nA generated edition now occupies this identity.\n",
                    "metadata":{"kind":"briefing_edition"}
                }),
            )
            .await);
        } else {
            ok(request(
                &f.app,
                &f.owner,
                Method::DELETE,
                &format!(
                    "/v1/workspace/entries/{}?expected_version=1",
                    s.origin["entry_ref"].as_str().unwrap()
                ),
                None,
            )
            .await);
        }
        let admitted = admit(&f).await;
        assert_eq!(admitted["inputs"], json!([]));
        let after = state(&f).await;
        assert_eq!(after["research"]["follow_ups"], json!([]));
        // Admission resets this attempt-local counter. Policy exclusion must not
        // create model processing credit in the newly admitted attempt.
        assert_eq!(after["processed_count"], 0);
        assert_eq!(
            after["research"]["completed"],
            before["research"]["completed"]
        );
        assert!(
            after["source_dispositions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|disposition| {
                    disposition["disposition"] == "excluded_by_source_policy"
                        && disposition["route"]["origin_source"] == source_identity(&s.origin)
                        && disposition["model_processed"] == false
                })
        );
    }
}

#[tokio::test]
async fn source_origin_old_exclusion_cannot_retire_restored_current_primary() {
    let Some(f) = fixture().await else { return };
    let s = scenario_with_options(&f, true, false).await;
    assert_eq!(s.origin["version"], 3);
    let routed = queue(&f, &s).await;
    let retained = routes(&f).await;
    let destination = resume_destination(&f, &s, &routed).await;
    assert_eq!(destination["inputs"], json!([]));
    assert_eq!(
        routes(&f).await,
        retained,
        "a historical generated exclusion cannot retire the currently ordinary source route"
    );
    assert!(offered_route(&destination).is_object());
}

//! Real HTTP/database regressions for comparing proposals without treating
//! generated text as evidence or transferring canonical proposal identities.
use super::*;

struct Scenario {
    first: Value,
    origin: Value,
    extra: Value,
    selected: Value,
    accepted: Value,
    item: Value,
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

fn input_identity(admission: &Value, source: &Value) -> Value {
    let input = admission["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|input| input["entry_ref"] == source["entry_ref"])
        .expect("fixture source is still retained");
    json!({"entry_ref":input["entry_ref"],"version":input["version"],"generation":input["generation"]})
}

fn has_input(admission: &Value, source: &Value) -> bool {
    admission["inputs"].as_array().unwrap().iter().any(|input| {
        input["entry_ref"] == source["entry_ref"] && input["version"] == source["version"]
    })
}

fn comparison(admission: &Value, item: &Value) -> Value {
    admission["comparison_proposals"]
        .as_array()
        .expect("comparison context is an explicit collection")
        .iter()
        .find(|proposal| proposal["pointer"]["item_id"] == item["id"])
        .expect("exact canonical source overlap offers the pending proposal")
        .clone()
}

fn overview(admission: &Value, sources: &[&Value], revision: Option<&Value>) -> Value {
    let content = format!(
        "# Source overview\n\n{}",
        sources
            .iter()
            .enumerate()
            .map(|(index, _)| format!("An independently checked observation.[^s{}]\n", index + 1))
            .collect::<String>()
    );
    let mut candidate = json!({
        "kind":"summary","title":"Source overview",
        "summary":"A bounded account supported by the selected primary records.",
        "reason":"Keep the useful observations together for owner review.",
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

async fn submit_overview(
    f: &Fixture,
    admission: &Value,
    sources: &[&Value],
    revision: Option<&Value>,
    processed: Vec<Value>,
) -> Value {
    let mut body = research_request(admission);
    body["candidates"] = json!([overview(admission, sources, revision)]);
    body["processed_inputs"] = json!(processed);
    body["research_progress"] = json!({
        "status":"waiting","notes":"Checked the exact primary records for this revision.",
        "reviewed_sources":sources.iter().map(|source|reviewed(source)).collect::<Vec<_>>(),
        "pending_queries":[],"pending_targets":[]
    });
    ok(post(f, &f.runner, "/v1/workspace/dreamer/candidates", body).await)
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
    control(f, "report-only", 0).await;
    // Deliberately different titles and no name matches: retrieval here must
    // come from exact cited primary evidence, never similar proposal titles.
    let first = write(
        f,
        "sources/Notes/Atlas.md",
        "# Atlas\n\nThe original record describes a completed survey.\n",
        0,
    )
    .await;
    let origin = write(
        f,
        "sources/Notes/Boreal.md",
        "# Boreal\n\nThe independent record documents the survey result.\n",
        0,
    )
    .await;
    let extra = write(
        f,
        "sources/Notes/Cedar.md",
        "# Cedar\n\nThe specification records an additional equipment detail.\n",
        0,
    )
    .await;
    let admitted = admit_requested(
        f,
        vec![first["entry_ref"].clone(), origin["entry_ref"].clone()],
    )
    .await;
    let selected = next_subject(f, &admitted).await;
    assert_eq!(selected["research"]["subject_ref"], first["entry_ref"]);
    let (_, selected) = discover_subject(f, &selected, vec![origin["entry_ref"].clone()]).await;
    let accepted = submit_overview(f, &selected, &[&first, &origin], None, vec![]).await;
    let item = review(f).await["items"][0].clone();
    assert_eq!(item["stale"], false);
    let selected = next_subject(f, &accepted).await;
    assert_eq!(selected["research"]["subject_ref"], origin["entry_ref"]);
    Scenario {
        first,
        origin,
        extra,
        selected,
        accepted,
        item,
    }
}

fn coverage_done(admission: &Value, origin: &Value, pointer: Value) -> Value {
    let mut body = progress_body(
        admission,
        vec![reviewed(origin)],
        "no_change",
        "Checked the primary input against the existing proposal; its useful claim is already covered.",
    );
    body["covers_existing"] = pointer;
    body["processed_inputs"] = json!([input_identity(admission, origin)]);
    body
}

fn follow_up(admission: &Value, origin: &Value, extra: &Value, pointer: Value) -> Value {
    let mut body = progress_body(
        admission,
        vec![reviewed(origin), reviewed(extra)],
        "waiting",
        "The primary specification adds a detail for the existing overview to assess.",
    );
    body["processed_inputs"] = json!([]);
    body["follow_up"] = json!({
        "comparison":pointer,"origin_input":input_identity(admission, origin),
        "targets":[origin["entry_ref"],extra["entry_ref"]]
    });
    body
}

async fn queued(f: &Fixture, source: &Value) -> bool {
    current(f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]
        ["requested_subject_refs"]
        .as_array()
        .unwrap()
        .contains(&source["entry_ref"])
}

#[tokio::test]
async fn comparison_exact_coverage_completes_only_reviewed_input_without_duplicate_or_mutation() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let offered = comparison(&s.selected, &s.item);
    assert_eq!(offered["subject_ref"], s.first["entry_ref"]);
    assert_eq!(
        offered["pointer"]["candidate_hash"],
        s.item["candidate_hash"]
    );
    assert_eq!(offered["pointer"]["run_entry_ref"], s.item["run_entry_ref"]);
    assert_eq!(offered["pointer"]["run_version"], s.item["run_version"]);
    assert!(
        offered["content"]
            .as_str()
            .is_some_and(|content| !content.is_empty())
    );
    assert!(offered["sources"].as_array().unwrap().iter().any(|source| {
        source["entry_ref"] == s.origin["entry_ref"] && source["version"] == s.origin["version"]
    }));
    assert!(
        s.selected["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|source| { source["entry_ref"] != offered["pointer"]["run_entry_ref"] }),
        "comparison prose cannot become a factual dependency"
    );
    let immutable = exact_run_record(&f, &s.accepted).await;
    let request = coverage_done(&s.selected, &s.origin, offered["pointer"].clone());
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request.clone(),
    )
    .await)["data"]
        .clone();
    assert!(!has_input(&saved, &s.origin));
    assert!(has_input(&saved, &s.first));
    assert!(has_input(&saved, &s.extra));
    assert!(!queued(&f, &s.origin).await);
    let view = review(&f).await;
    assert_eq!(view["items"].as_array().unwrap().len(), 1);
    assert_eq!(view["items"][0], s.item);
    assert_eq!(exact_run_record(&f, &s.accepted).await, immutable);
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(replay["data"]["inputs"], saved["inputs"]);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn comparison_overlap_does_not_prevent_distinct_scope_and_irrelevant_proposals_are_absent() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    comparison(&s.selected, &s.item);
    let accepted = submit_overview(&f, &s.selected, &[&s.origin], None, vec![]).await;
    let view = review(&f).await;
    assert_eq!(view["items"].as_array().unwrap().len(), 2);
    assert!(
        view["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == &s.item)
    );
    let destinations = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["candidate"]["target_path"].clone())
        .collect::<Vec<_>>();
    assert_ne!(
        destinations[0], destinations[1],
        "different useful scope keeps its canonical destination"
    );
    let unrelated = next_subject(&f, &accepted).await;
    assert_eq!(unrelated["research"]["subject_ref"], s.extra["entry_ref"]);
    assert_eq!(unrelated["comparison_proposals"], json!([]));
}

#[tokio::test]
async fn comparison_changed_pointer_or_uncovered_processed_source_rejects_atomically() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let (_, selected) = discover_subject(&f, &s.selected, vec![s.extra["entry_ref"].clone()]).await;
    let pointer = comparison(&selected, &s.item)["pointer"].clone();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&s.origin)).await.unwrap();
    for (field, value) in [
        ("item_id", json!("2099-01-01/999")),
        ("candidate_hash", json!("0".repeat(64))),
        ("run_entry_ref", json!(format!("entry:{}", Uuid::now_v7()))),
        (
            "run_version",
            json!(pointer["run_version"].as_i64().unwrap() + 1),
        ),
    ] {
        let mut changed = pointer.clone();
        changed[field] = value;
        let response = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            coverage_done(&selected, &s.origin, changed),
        )
        .await;
        assert!(
            response.status.is_client_error(),
            "{field}: {}",
            response.body
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), job);
    }
    let mut uncovered = coverage_done(&selected, &s.origin, pointer);
    uncovered["reviewed_sources"] = json!([reviewed(&s.origin), reviewed(&s.extra)]);
    uncovered["processed_inputs"] = json!([
        input_identity(&selected, &s.origin),
        input_identity(&selected, &s.extra)
    ]);
    let response = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        uncovered,
    )
    .await;
    assert!(response.status.is_client_error(), "{}", response.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), job);
}

#[tokio::test]
async fn comparison_owner_or_source_race_cannot_consume_retained_input() {
    for change in ["reject", "correct", "defer", "approve", "source"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let mut body = coverage_done(
            &s.selected,
            &s.origin,
            comparison(&s.selected, &s.item)["pointer"].clone(),
        );
        if change == "source" {
            write(
                &f,
                "sources/Notes/Atlas.md",
                "# Atlas\n\nThe original record now corrects the survey outcome.\n",
                1,
            )
            .await;
        } else {
            let view = review(&f).await;
            let decided = ok(post(
                &f,
                &f.owner,
                "/v1/dreamer/review/decisions",
                decision(&view, &s.item, change),
            )
            .await);
            // Supply the current state version so this exercises comparison
            // revalidation even after the client refreshes its state CAS.
            body["expected_state_version"] = decided["data"]["state_version"].clone();
        }
        let state = current(&f, "dreams/state.md").await.unwrap();
        let job = current(&f, &job_path(&s.origin)).await.unwrap();
        let response = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            body,
        )
        .await;
        if change == "source" {
            // A cited source moving after the pass cutoff is refresh work; the
            // exact admitted input remains covered by the pinned proposal.
            assert!(response.status.is_success(), "{change}: {}", response.body);
            continue;
        }
        assert!(
            response.status.is_client_error(),
            "{change}: {}",
            response.body
        );
        assert_eq!(
            current(&f, "dreams/state.md").await.unwrap(),
            state,
            "{change}"
        );
        assert_eq!(
            current(&f, &job_path(&s.origin)).await.unwrap(),
            job,
            "{change}"
        );
        let mut discover = s.selected.clone();
        discover["state_version"] = json!(state.0);
        let (_, current_view) = discover_subject(&f, &discover, vec![]).await;
        assert!(has_input(&current_view, &s.origin));
        assert_eq!(current_view["comparison_proposals"], json!([]), "{change}");
    }
}

#[tokio::test]
async fn comparison_enrichment_survives_fence_restart_and_omitted_origin_until_exact_disposition() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let (_, origin) = discover_subject(&f, &s.selected, vec![s.extra["entry_ref"].clone()]).await;
    let request = follow_up(
        &origin,
        &s.origin,
        &s.extra,
        comparison(&origin, &s.item)["pointer"].clone(),
    );
    let routed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request.clone(),
    )
    .await)["data"]
        .clone();
    assert_eq!(
        routed["inputs"], origin["inputs"],
        "routing never acknowledges source input"
    );
    assert!(queued(&f, &s.first).await);
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    // A new acknowledged operation for the same origin/destination must merge
    // the existing route just as replaying one operation must not duplicate it.
    let routed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        follow_up(
            &routed,
            &s.origin,
            &s.extra,
            comparison(&origin, &s.item)["pointer"].clone(),
        ),
    )
    .await)["data"]
        .clone();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let origin_job = current(&f, &job_path(&s.origin)).await.unwrap();
    let mut legacy = attempt(&routed, routed["state_version"].as_i64().unwrap());
    legacy["candidates"] = json!([]);
    legacy["processed_inputs"] = json!([input_identity(&routed, &s.origin)]);
    legacy["findings"] =
        json!(["An ordinary submission attempts to acknowledge the routed input."]);
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", legacy).await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), origin_job);
    let other = next_subject(&f, &routed).await;
    assert_ne!(
        other["research"]["subject_ref"], s.first["entry_ref"],
        "an already served destination waits for the next attempt"
    );
    assert!(queued(&f, &s.first).await);
    finish(
        &f,
        &other,
        other["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let resumed = next_subject(&f, &admit(&f).await).await;
    // The originating subject remains independently queued; its waiting state
    // must not prevent the destination from receiving a turn in this attempt.
    let resumed = if resumed["research"]["subject_ref"] != s.first["entry_ref"] {
        let mut wait = research_request(&resumed);
        wait["status"] = json!("waiting");
        let yielded = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            wait,
        )
        .await)["data"]
            .clone();
        next_subject(&f, &yielded).await
    } else {
        resumed
    };
    assert_eq!(resumed["research"]["subject_ref"], s.first["entry_ref"]);
    let routes = resumed["research"]["routed_work"].as_array().unwrap();
    assert_eq!(routes.len(), 1, "replay retains one durable route");
    assert_eq!(
        routes[0]["origin_input"],
        input_identity(&origin, &s.origin)
    );
    assert_eq!(
        routes[0]["comparison"],
        comparison(&origin, &s.item)["pointer"]
    );
    assert_eq!(
        resumed["research"]["routed_targets"],
        json!([s.extra["entry_ref"]])
    );
    let clear = progress_body(
        &resumed,
        vec![reviewed(&s.first), reviewed(&s.origin)],
        "researching",
        "The existing sources were checked; the routed specification still needs inspection.",
    );
    let cleared = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        clear,
    )
    .await)["data"]
        .clone();
    assert_eq!(cleared["research"]["pending_targets"], json!([]));
    assert_eq!(
        cleared["research"]["routed_work"],
        resumed["research"]["routed_work"]
    );
    assert_eq!(
        cleared["research"]["routed_targets"],
        resumed["research"]["routed_targets"]
    );
    let (_, expanded) = discover_subject(
        &f,
        &cleared,
        cleared["research"]["routed_targets"]
            .as_array()
            .unwrap()
            .clone(),
    )
    .await;
    assert_eq!(expanded["research"]["routed_targets"], json!([]));
    assert!(
        expanded["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == s.extra["entry_ref"])
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&s.first)).await.unwrap();
    // Reviewing the origin in progress cannot conceal its omission from the
    // accepted summary. A failed proposal must not advance either input state
    // or the immutable proposal identity.
    let mut omitted_citation = research_request(&expanded);
    omitted_citation["candidates"] =
        json!([overview(&expanded, &[&s.first, &s.extra], Some(&s.item),)]);
    omitted_citation["processed_inputs"] = json!([input_identity(&expanded, &s.origin)]);
    omitted_citation["research_progress"] = json!({
        "status":"waiting","notes":"All headers were checked but the summary omits the origin.",
        "reviewed_sources":[reviewed(&s.first),reviewed(&s.origin),reviewed(&s.extra)],
        "pending_queries":[],"pending_targets":[]
    });
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        omitted_citation,
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.first)).await.unwrap(), job);
    let mut unchecked_target = progress_body(
        &expanded,
        vec![reviewed(&s.first), reviewed(&s.origin)],
        "no_change",
        "The origin was reviewed, but its routed specification was not.",
    );
    unchecked_target["processed_inputs"] = json!([input_identity(&expanded, &s.origin)]);
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        unchecked_target,
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.first)).await.unwrap(), job);
    let immutable = exact_run_record(&f, &s.accepted).await;
    let omitted = submit_overview(
        &f,
        &expanded,
        &[&s.first, &s.origin, &s.extra],
        Some(&s.item),
        vec![],
    )
    .await;
    assert!(has_input(&omitted, &s.origin));
    assert!(
        queued(&f, &s.first).await,
        "a revision without explicit origin disposition leaves its priority"
    );
    assert_eq!(
        omitted["research"]["routed_work"].as_array().unwrap().len(),
        1
    );
    assert_eq!(omitted["accepted_candidate_ids"], json!([s.item["id"]]));
    assert_eq!(review(&f).await["items"].as_array().unwrap().len(), 1);
    assert_eq!(exact_run_record(&f, &s.accepted).await, immutable);
    let mut done = progress_body(
        &omitted,
        vec![reviewed(&s.first), reviewed(&s.origin), reviewed(&s.extra)],
        "no_change",
        "All routed primary evidence was reviewed and the updated overview covers the input.",
    );
    done["processed_inputs"] = json!([input_identity(&omitted, &s.origin)]);
    let completed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        done,
    )
    .await)["data"]
        .clone();
    assert!(!has_input(&completed, &s.origin));
    assert!(!queued(&f, &s.first).await);
    assert!(
        has_input(&completed, &s.extra),
        "reviewing a target is distinct from disposing its own retained input"
    );
    assert_eq!(completed["research"]["routed_work"], json!([]));
}

#[tokio::test]
async fn comparison_follow_up_queue_overflow_rolls_back_origin_progress_and_route() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    finish(
        &f,
        &s.selected,
        s.selected["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let mut references = vec![s.origin["entry_ref"].clone(), s.extra["entry_ref"].clone()];
    for index in 0..14 {
        let source = write(
            &f,
            &format!("sources/Queue/Item{index:02}.md"),
            &format!("# Queue item {index:02}\n\nAn independent retained task.\n"),
            0,
        )
        .await;
        references.push(source["entry_ref"].clone());
    }
    let admitted = admit_requested(&f, references).await;
    let selected = next_subject(&f, &admitted).await;
    assert_eq!(selected["research"]["subject_ref"], s.origin["entry_ref"]);
    let (_, selected) = discover_subject(&f, &selected, vec![s.extra["entry_ref"].clone()]).await;
    let body = follow_up(
        &selected,
        &s.origin,
        &s.extra,
        comparison(&selected, &s.item)["pointer"].clone(),
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    let origin_job = current(&f, &job_path(&s.origin)).await.unwrap();
    let target_job = current(&f, &job_path(&s.first)).await.unwrap();
    let response = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await;
    assert!(response.status.is_client_error(), "{}", response.body);
    assert!(
        response.body.to_string().contains("queue"),
        "{}",
        response.body
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), origin_job);
    assert_eq!(current(&f, &job_path(&s.first)).await.unwrap(), target_job);
    assert!(!queued(&f, &s.first).await);
}

#[tokio::test]
async fn comparison_retires_only_exact_unreviewed_duplicate_and_preserves_immutable_history() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let duplicate_receipt = submit_overview(&f, &s.selected, &[&s.origin], None, vec![]).await;
    let duplicate = review(&f).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == duplicate_receipt["accepted_candidate_ids"][0])
        .unwrap()
        .clone();
    let first_record = exact_run_record(&f, &s.accepted).await;
    let duplicate_record = exact_run_record(&f, &duplicate_receipt).await;
    finish(
        &f,
        &duplicate_receipt,
        duplicate_receipt["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let selected = next_subject(
        &f,
        &admit_requested(&f, vec![s.origin["entry_ref"].clone()]).await,
    )
    .await;
    let covering = comparison(&selected, &s.item);
    let retired = comparison(&selected, &duplicate);
    assert_eq!(retired["retirable"], true);
    assert_eq!(
        retired["subject_ref"], selected["research"]["subject_ref"],
        "the selected subject's own draft is available for full comparison"
    );
    let mut request = coverage_done(&selected, &s.origin, covering["pointer"].clone());
    request["supersedes_existing"] = retired["pointer"].clone();
    request["findings"] = json!([
        "I reviewed both complete drafts against their primary sources. Every useful claim, qualification, date and scope in this draft is covered by the retained overview."
    ]);
    let audit_path = format!(
        "dreams/reviews/comparison-{}.md",
        request["operation_id"].as_str().unwrap()
    );
    let response = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request.clone(),
    )
    .await);
    assert!(!has_input(&response["data"], &s.origin));
    let view = review(&f).await;
    assert!(
        view["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != duplicate["id"]),
        "retired duplicates leave the live Review inbox"
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    let superseded = state.2["dreamer_state"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == duplicate["id"])
        .unwrap();
    assert_eq!(superseded["status"], "superseded");
    for field in ["id", "candidate_hash", "run_entry_ref", "run_version"] {
        assert_eq!(
            superseded[field], duplicate[field],
            "retirement preserves {field}"
        );
    }
    let original = duplicate_record.1["dreamer_run"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == duplicate["id"])
        .unwrap();
    let audit = current(&f, &audit_path).await.unwrap();
    let audited = &audit.2["dreamer_review"]["item"];
    assert_eq!(audited["status"], "superseded");
    assert_eq!(
        audited["candidate"], original["candidate"],
        "all exact candidate bytes and dependencies survive retirement"
    );
    assert_eq!(
        audit.2["dreamer_review"]["disposition"]["duplicate"],
        retired["pointer"]
    );
    assert_eq!(
        audit.2["dreamer_review"]["disposition"]["covering"],
        covering["pointer"]
    );
    assert_eq!(view["counts"]["proposals"], 1);
    assert_eq!(exact_run_record(&f, &s.accepted).await, first_record);
    assert_eq!(
        exact_run_record(&f, &duplicate_receipt).await,
        duplicate_record
    );
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &audit_path).await.unwrap(), audit);
    let rejected = post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &duplicate, "approve"),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::CONFLICT, "{}", rejected.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn comparison_cannot_retire_a_regenerated_item_with_prior_owner_review() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let mut accepted = submit_overview(&f, &s.selected, &[&s.origin], None, vec![]).await;
    let view = review(&f).await;
    let duplicate = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == accepted["accepted_candidate_ids"][0])
        .unwrap()
        .clone();
    let correction = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &duplicate, "correct"),
    )
    .await);
    accepted["state_version"] = correction["data"]["state_version"].clone();
    let mut revision = research_request(&accepted);
    let mut candidate = overview(&accepted, &[&s.origin], Some(&duplicate));
    candidate["content"] = json!(
        "# Qualified survey record\n\nThe independent record documents the survey result, with its original qualification preserved.[^s1]\n"
    );
    revision["candidates"] = json!([candidate]);
    revision["processed_inputs"] = json!([]);
    let updated = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", revision).await);
    assert_eq!(updated["accepted_candidate_ids"], json!([duplicate["id"]]));
    let view = review(&f).await;
    let regenerated = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == duplicate["id"])
        .unwrap()
        .clone();
    assert_eq!(regenerated["status"], "pending");
    assert_ne!(regenerated["candidate_hash"], duplicate["candidate_hash"]);
    // Model RunState's bounded history rolling past this decision. The actual
    // immutable HTTP decision audit remains available and must still prohibit
    // automatic retirement of the regenerated identity.
    let mut compacted = current(&f, "dreams/state.md").await.unwrap().2;
    assert!(
        !compacted["dreamer_state"]["history"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    compacted["dreamer_state"]["history"] = json!([]);
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$2 FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(compacted).execute(&f.pool).await.unwrap();
    // Historical manifest lookup must include deleted audit entries and every
    // equivalent path spelling. This models restoration/deletion history; the
    // normal workspace API itself cannot delete a managed review artifact.
    let changed = sqlx::query("UPDATE brunn.entries e SET path=upper(e.path),deleted_at=clock_timestamp() FROM brunn.entry_versions v WHERE e.user_id=$1 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.metadata->'dreamer_review'->'decision'->>'item_id'=$2")
        .bind(f.owner.user).bind(duplicate["id"].as_str().unwrap()).execute(&f.pool).await.unwrap();
    assert_eq!(changed.rows_affected(), 1);
    finish(
        &f,
        &updated,
        updated["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let selected = next_subject(
        &f,
        &admit_requested(&f, vec![s.origin["entry_ref"].clone()]).await,
    )
    .await;
    let covering = comparison(&selected, &s.item);
    let protected = comparison(&selected, &regenerated);
    assert_eq!(
        protected["retirable"], false,
        "a new pending revision does not erase owner-review history"
    );
    let mut request = coverage_done(&selected, &s.origin, covering["pointer"].clone());
    request["supersedes_existing"] = protected["pointer"].clone();
    request["findings"] = json!(["I compared the complete drafts and their primary sources."]);
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&s.origin)).await.unwrap();
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request,
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), job);
    assert_eq!(
        review(&f).await["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == regenerated["id"])
            .unwrap(),
        &regenerated
    );
}

#[tokio::test]
async fn comparison_retirement_waits_for_incoming_enrichment_even_without_processed_inputs() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let first = write(
        &f,
        "sources/Notes/Atlas.md",
        "# Atlas\n\nA source survey.\n",
        0,
    )
    .await;
    let duplicate_source = write(
        &f,
        "sources/Notes/Boreal.md",
        "# Boreal\n\nA survey account.\n",
        0,
    )
    .await;
    let origin = write(
        &f,
        "sources/Notes/Cedar.md",
        "# Cedar\n\nAn additional survey qualification.\n",
        0,
    )
    .await;
    let admitted = admit_requested(
        &f,
        vec![
            first["entry_ref"].clone(),
            duplicate_source["entry_ref"].clone(),
            origin["entry_ref"].clone(),
        ],
    )
    .await;
    let selected = next_subject(&f, &admitted).await;
    assert_eq!(selected["research"]["subject_ref"], first["entry_ref"]);
    let (_, selected) = discover_subject(
        &f,
        &selected,
        vec![
            duplicate_source["entry_ref"].clone(),
            origin["entry_ref"].clone(),
        ],
    )
    .await;
    let covering_receipt = submit_overview(
        &f,
        &selected,
        &[&first, &duplicate_source, &origin],
        None,
        vec![],
    )
    .await;
    let covering_item = review(&f).await["items"][0].clone();
    let selected = next_subject(&f, &covering_receipt).await;
    assert_eq!(
        selected["research"]["subject_ref"],
        duplicate_source["entry_ref"]
    );
    let (_, selected) = discover_subject(&f, &selected, vec![origin["entry_ref"].clone()]).await;
    let duplicate_receipt =
        submit_overview(&f, &selected, &[&duplicate_source, &origin], None, vec![]).await;
    let duplicate = review(&f).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == duplicate_receipt["accepted_candidate_ids"][0])
        .unwrap()
        .clone();
    let immutable = exact_run_record(&f, &duplicate_receipt).await;
    let selected = next_subject(&f, &duplicate_receipt).await;
    assert_eq!(selected["research"]["subject_ref"], origin["entry_ref"]);
    let (_, selected) =
        discover_subject(&f, &selected, vec![duplicate_source["entry_ref"].clone()]).await;
    let routed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        follow_up(
            &selected,
            &origin,
            &duplicate_source,
            comparison(&selected, &duplicate)["pointer"].clone(),
        ),
    )
    .await)["data"]
        .clone();
    finish(
        &f,
        &routed,
        routed["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let selected = next_subject(
        &f,
        &admit_requested(&f, vec![duplicate_source["entry_ref"].clone()]).await,
    )
    .await;
    let selected = if selected["research"]["subject_ref"] == origin["entry_ref"] {
        let mut wait = research_request(&selected);
        wait["status"] = json!("waiting");
        let waiting = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            wait,
        )
        .await)["data"]
            .clone();
        next_subject(&f, &waiting).await
    } else {
        selected
    };
    assert_eq!(
        selected["research"]["subject_ref"],
        duplicate_source["entry_ref"]
    );
    let mut retirement = progress_body(
        &selected,
        vec![reviewed(&duplicate_source), reviewed(&origin)],
        "no_change",
        "Both complete drafts were compared against their primary records.",
    );
    retirement["processed_inputs"] = json!([]);
    retirement["covers_existing"] = comparison(&selected, &covering_item)["pointer"].clone();
    retirement["supersedes_existing"] = comparison(&selected, &duplicate)["pointer"].clone();
    retirement["findings"] = json!([
        "Every useful claim, qualification and scope in the complete duplicate draft is covered by the complete retained overview."
    ]);
    let audit_path = format!(
        "dreams/reviews/comparison-{}.md",
        retirement["operation_id"].as_str().unwrap()
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&duplicate_source)).await.unwrap();
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        retirement.clone(),
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert!(
        rejected.body.to_string().contains("incoming enrichment"),
        "{}",
        rejected.body
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(
        current(&f, &job_path(&duplicate_source)).await.unwrap(),
        job
    );
    assert!(current(&f, &audit_path).await.is_none());
    assert!(has_input(&selected, &origin));
    assert!(queued(&f, &duplicate_source).await);
    assert_eq!(exact_run_record(&f, &duplicate_receipt).await, immutable);

    // Explicitly supported no_change resolves the primary-source obligation
    // first. A later whole-draft disposition may then retire the duplicate.
    let mut done = progress_body(
        &selected,
        vec![reviewed(&duplicate_source), reviewed(&origin)],
        "no_change",
        "The routed qualification is already represented by the current fresh destination overview.",
    );
    done["processed_inputs"] = json!([input_identity(&selected, &origin)]);
    let completed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        done,
    )
    .await)["data"]
        .clone();
    assert_eq!(completed["research"]["routed_work"], json!([]));
    assert!(!has_input(&completed, &origin));
    retirement["expected_state_version"] = completed["state_version"].clone();
    retirement["research_version"] = completed["research"]["version"].clone();
    let retired = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        retirement,
    )
    .await);
    assert_eq!(retired["data"]["research"]["routed_work"], json!([]));
    assert_eq!(
        current(&f, &audit_path).await.unwrap().2["dreamer_review"]["item"]["status"],
        "superseded"
    );
    assert_eq!(exact_run_record(&f, &duplicate_receipt).await, immutable);
}

#[tokio::test]
async fn comparison_follow_up_rejects_self_routing_and_retained_cycles_atomically() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let (_, selected) = discover_subject(&f, &s.selected, vec![s.first["entry_ref"].clone()]).await;
    let accepted = submit_overview(&f, &selected, &[&s.origin, &s.first], None, vec![]).await;
    let view = review(&f).await;
    let second = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == accepted["accepted_candidate_ids"][0])
        .unwrap()
        .clone();
    let self_request = follow_up(
        &accepted,
        &s.origin,
        &s.first,
        comparison(&accepted, &second)["pointer"].clone(),
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&s.origin)).await.unwrap();
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        self_request,
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), job);

    let route = follow_up(
        &accepted,
        &s.origin,
        &s.first,
        comparison(&accepted, &s.item)["pointer"].clone(),
    );
    let routed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        route,
    )
    .await)["data"]
        .clone();
    finish(
        &f,
        &routed,
        routed["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let destination = next_subject(&f, &admit(&f).await).await;
    assert_eq!(destination["research"]["subject_ref"], s.first["entry_ref"]);
    let cycle = follow_up(
        &destination,
        &s.first,
        &s.origin,
        comparison(&destination, &second)["pointer"].clone(),
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    let first_job = current(&f, &job_path(&s.first)).await.unwrap();
    let second_job = current(&f, &job_path(&s.origin)).await.unwrap();
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        cycle,
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert!(
        rejected.body.to_string().contains("cycle"),
        "{}",
        rejected.body
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.first)).await.unwrap(), first_job);
    assert_eq!(current(&f, &job_path(&s.origin)).await.unwrap(), second_job);
}

#[tokio::test]
async fn comparison_follow_up_reconciles_changed_origin_only_after_cited_current_disposition() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let (_, selected) = discover_subject(&f, &s.selected, vec![s.extra["entry_ref"].clone()]).await;
    let old_origin = input_identity(&selected, &s.origin);
    let request = follow_up(
        &selected,
        &s.origin,
        &s.extra,
        comparison(&selected, &s.item)["pointer"].clone(),
    );
    let routed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request,
    )
    .await)["data"]
        .clone();
    finish(
        &f,
        &routed,
        routed["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let replacement = write(&f, "sources/Notes/Boreal.md", "# Boreal\n\nThe independent record corrects the survey result and retains its qualification.\n", 1).await;
    assert_eq!(replacement["entry_ref"], s.origin["entry_ref"]);
    let resumed = next_subject(&f, &admit(&f).await).await;
    let destination = if resumed["research"]["subject_ref"] == s.first["entry_ref"] {
        resumed
    } else {
        assert_eq!(resumed["research"]["subject_ref"], s.origin["entry_ref"]);
        let mut wait = research_request(&resumed);
        wait["status"] = json!("waiting");
        let waiting = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            wait,
        )
        .await)["data"]
            .clone();
        next_subject(&f, &waiting).await
    };
    assert_eq!(destination["research"]["subject_ref"], s.first["entry_ref"]);
    let routes = destination["research"]["routed_work"].as_array().unwrap();
    assert_eq!(
        routes.len(),
        1,
        "upgrading retained input alone cannot erase the old route"
    );
    assert_eq!(routes[0]["origin_input"], old_origin);
    assert_eq!(
        routes[0]["current_input"],
        input_identity(&destination, &replacement)
    );
    assert_eq!(routes[0]["status"], "source_changed");
    assert!(queued(&f, &s.first).await);
    let (_, expanded) = discover_subject(
        &f,
        &destination,
        destination["research"]["routed_targets"]
            .as_array()
            .unwrap()
            .clone(),
    )
    .await;
    for source in [&s.first, &replacement, &s.extra] {
        assert!(
            expanded["research"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|head| head["entry_ref"] == source["entry_ref"]
                    && head["version"] == source["version"])
        );
    }
    let current_origin = input_identity(&expanded, &replacement);
    // Research refresh makes the job current, but it cannot update the old
    // pending proposal. A source-based no_change must not strand that stale
    // view by consuming its routed correction and removing its priority.
    let view = review(&f).await;
    assert_eq!(view["items"][0]["status"], "pending");
    assert_eq!(view["items"][0]["stale"], false);
    assert_eq!(view["items"][0]["refresh_pending"], true);
    let mut premature = progress_body(
        &expanded,
        vec![
            reviewed(&s.first),
            reviewed(&replacement),
            reviewed(&s.extra),
        ],
        "no_change",
        "The replacement primary record was reviewed in the refreshed job.",
    );
    premature["processed_inputs"] = json!([current_origin.clone()]);
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&s.first)).await.unwrap();
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        premature,
    )
    .await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert!(
        rejected.body.to_string().contains("fresh destination"),
        "{}",
        rejected.body
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&s.first)).await.unwrap(), job);
    assert!(queued(&f, &s.first).await);
    let accepted = submit_overview(
        &f,
        &expanded,
        &[&s.first, &replacement, &s.extra],
        Some(&s.item),
        vec![current_origin.clone()],
    )
    .await;
    assert_eq!(accepted["accepted_candidate_ids"], json!([s.item["id"]]));
    assert!(!has_input(&accepted, &replacement));
    assert_eq!(accepted["research"]["routed_work"], json!([]));
    assert!(!queued(&f, &s.first).await);
    let record = exact_run_record(&f, &accepted).await;
    let disposition = record.1["dreamer_run"]["source_dispositions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["disposition"] == "research_follow_up_completed")
        .expect("immutable accepted run retains original-to-current disposition");
    assert_eq!(disposition["origin_input"], old_origin);
    assert_eq!(disposition["processed_input"], current_origin);
    assert_eq!(disposition["subject_ref"], s.first["entry_ref"]);
}

#[tokio::test]
async fn comparison_cap_preserves_complete_bodies_and_reserves_selected_subjects_own_draft() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let origin = write(
        &f,
        "sources/Notes/Boreal.md",
        "# Boreal\n\nAn independently retained survey result.\n",
        0,
    )
    .await;
    let mut peers = Vec::new();
    for index in 0..5 {
        peers.push(
            write(
                &f,
                &format!("sources/Notes/Peer{index:02}.md"),
                &format!("# Peer{index:02}\n\nA separate primary observation for this peer.\n"),
                0,
            )
            .await,
        );
    }
    let mut references = peers
        .iter()
        .map(|source| source["entry_ref"].clone())
        .collect::<Vec<_>>();
    references.push(origin["entry_ref"].clone());
    let mut admission = admit_requested(&f, references).await;
    let mut complete_bodies = Vec::new();
    for peer in &peers {
        let selected = next_subject(&f, &admission).await;
        assert_eq!(selected["research"]["subject_ref"], peer["entry_ref"]);
        let (_, selected) =
            discover_subject(&f, &selected, vec![origin["entry_ref"].clone()]).await;
        admission = submit_overview(&f, &selected, &[peer, &origin], None, vec![]).await;
        complete_bodies.push((
            admission["accepted_candidate_ids"][0].clone(),
            overview(&selected, &[peer, &origin], None)["content"].clone(),
        ));
    }
    let selected = next_subject(&f, &admission).await;
    assert_eq!(selected["research"]["subject_ref"], origin["entry_ref"]);
    let accepted = submit_overview(&f, &selected, &[&origin], None, vec![]).await;
    let own_id = accepted["accepted_candidate_ids"][0].clone();
    complete_bodies.push((
        own_id.clone(),
        overview(&selected, &[&origin], None)["content"].clone(),
    ));
    finish(
        &f,
        &accepted,
        accepted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let selected = next_subject(
        &f,
        &admit_requested(&f, vec![origin["entry_ref"].clone()]).await,
    )
    .await;
    let comparisons = selected["comparison_proposals"].as_array().unwrap();
    assert_eq!(
        comparisons.len(),
        4,
        "comparison retrieval is bounded before model admission"
    );
    assert!(
        comparisons
            .iter()
            .any(|proposal| proposal["pointer"]["item_id"] == own_id),
        "the selected subject's later own draft remains available for possible retirement under rank pressure"
    );
    for proposal in comparisons {
        let original = complete_bodies
            .iter()
            .find(|(id, _)| id == &proposal["pointer"]["item_id"])
            .unwrap();
        assert_eq!(
            proposal["content"], original.1,
            "bounded comparison context never supplies a truncated draft as complete coverage"
        );
        assert!(
            proposal["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|source| source["entry_ref"] == origin["entry_ref"]
                    && source["version"] == origin["version"])
        );
    }
}

#[tokio::test]
async fn comparison_follow_up_source_policy_exclusion_retires_route_without_model_completion() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let (_, selected) = discover_subject(&f, &s.selected, vec![s.extra["entry_ref"].clone()]).await;
    let request = follow_up(
        &selected,
        &s.origin,
        &s.extra,
        comparison(&selected, &s.item)["pointer"].clone(),
    );
    let routed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request,
    )
    .await)["data"]
        .clone();
    let before = current(&f, "dreams/state.md").await.unwrap().2;
    assert_eq!(
        before["dreamer_state"]["research"]["follow_ups"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let original_route = before["dreamer_state"]["research"]["follow_ups"][0].clone();
    assert_eq!(
        original_route["origin_input"],
        input_identity(&selected, &s.origin)
    );
    assert!(queued(&f, &s.first).await);
    finish(
        &f,
        &routed,
        routed["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    // A normal owner write supplies the classification. Admission's existing
    // deterministic source policy, rather than model judgment, excludes it.
    ok(post(
        &f,
        &f.owner,
        "/v1/workspace/write",
        json!({
            "path":"sources/Notes/Boreal.md","expected_version":1,
            "content":"# Boreal\n\nA generated edition now occupies this source identity.\n",
            "metadata":{"kind":"briefing_edition"}
        }),
    )
    .await);
    let admitted = admit(&f).await;
    assert!(
        !admitted["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input["entry_ref"] == s.origin["entry_ref"])
    );
    let after = current(&f, "dreams/state.md").await.unwrap().2;
    assert_eq!(after["dreamer_state"]["research"]["follow_ups"], json!([]));
    assert_eq!(
        after["dreamer_state"]["processed_count"],
        before["dreamer_state"]["processed_count"]
    );
    assert_eq!(
        after["dreamer_state"]["research"]["completed"],
        before["dreamer_state"]["research"]["completed"]
    );
    assert!(
        after["dreamer_state"]["source_dispositions"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |disposition| disposition["entry_ref"] == s.origin["entry_ref"]
                    && disposition["disposition"] == "excluded_generated_briefing"
            )
    );
    assert!(
        !queued(&f, &s.first).await,
        "the final policy-excluded route releases its destination priority"
    );
    let audits: Vec<Value> = sqlx::query_scalar("SELECT v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id WHERE e.user_id=$1 AND starts_with(e.path,'dreams/reviews/research-exclusion-')")
        .bind(f.owner.user).fetch_all(&f.pool).await.unwrap();
    assert_eq!(
        audits.len(),
        1,
        "one immutable audit records this route retirement"
    );
    let disposition = &audits[0]["dreamer_review"]["disposition"];
    assert_eq!(disposition["disposition"], "excluded_by_source_policy");
    assert_eq!(disposition["route"], original_route);
    assert_eq!(
        disposition["source_disposition"]["entry_ref"],
        s.origin["entry_ref"]
    );
    assert_eq!(
        disposition["source_disposition"]["disposition"],
        "excluded_generated_briefing"
    );
    assert_eq!(disposition["model_processed"], false);
    assert!(
        after["dreamer_state"]["source_dispositions"]
            .as_array()
            .unwrap()
            .contains(disposition)
    );
}

#[tokio::test]
async fn comparison_routed_no_change_rechecks_owner_decision_after_destination_selection() {
    for change in ["reject", "defer", "approve", "correct"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let (_, origin) =
            discover_subject(&f, &s.selected, vec![s.extra["entry_ref"].clone()]).await;
        let routed = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            follow_up(
                &origin,
                &s.origin,
                &s.extra,
                comparison(&origin, &s.item)["pointer"].clone(),
            ),
        )
        .await)["data"]
            .clone();
        finish(
            &f,
            &routed,
            routed["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        let mut current_run = admit(&f).await;
        for _ in 0..4 {
            current_run = next_subject(&f, &current_run).await;
            if current_run["research"]["subject_ref"] == s.first["entry_ref"] {
                break;
            }
            let mut wait = research_request(&current_run);
            wait["status"] = json!("waiting");
            current_run = ok(post(
                &f,
                &f.runner,
                "/v1/workspace/dreamer/research-progress",
                wait,
            )
            .await)["data"]
                .clone();
        }
        assert_eq!(current_run["research"]["subject_ref"], s.first["entry_ref"]);
        let (_, destination) = discover_subject(
            &f,
            &current_run,
            current_run["research"]["routed_targets"]
                .as_array()
                .unwrap()
                .clone(),
        )
        .await;
        let mut done = progress_body(
            &destination,
            vec![reviewed(&s.first), reviewed(&s.origin), reviewed(&s.extra)],
            "no_change",
            "Exact primary sources were reviewed; no additional source change was found.",
        );
        done["processed_inputs"] = json!([input_identity(&destination, &s.origin)]);
        done["findings"] = json!(["The routed primary records have been reviewed."]);
        assert!(done.get("covers_existing").is_none());
        let view = review(&f).await;
        let decided = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, &s.item, change),
        )
        .await);
        done["expected_state_version"] = decided["data"]["state_version"].clone();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let job = current(&f, &job_path(&s.first)).await.unwrap();
        let rejected = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            done,
        )
        .await;
        assert!(
            rejected.status.is_client_error(),
            "{change}: {}",
            rejected.body
        );
        assert_eq!(
            current(&f, "dreams/state.md").await.unwrap(),
            state,
            "{change}"
        );
        assert_eq!(
            current(&f, &job_path(&s.first)).await.unwrap(),
            job,
            "{change}"
        );
        assert_eq!(
            state.2["dreamer_state"]["research"]["follow_ups"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            state.2["dreamer_state"]["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|input| input["entry_ref"] == s.origin["entry_ref"])
        );
    }
}

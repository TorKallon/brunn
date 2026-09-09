//! Included by the real HTTP/database review fixture, not a standalone model.
use super::*;

fn research_request(admission: &Value) -> Value {
    let mut body = attempt(admission, admission["state_version"].as_i64().unwrap());
    body["subject_ref"] = admission["research"]["subject_ref"].clone();
    body["research_version"] = admission["research"]["version"].clone();
    body["operation_id"] = json!(Uuid::now_v7());
    body
}
async fn next_subject(f: &Fixture, admission: &Value) -> Value {
    let mut body = attempt(admission, admission["state_version"].as_i64().unwrap());
    body["operation_id"] = json!(Uuid::now_v7());
    ok(post(f, &f.runner, "/v1/workspace/dreamer/research-next", body).await)["data"].clone()
}
async fn discover_subject(f: &Fixture, admission: &Value, targets: Vec<Value>) -> (Value, Value) {
    let mut body = research_request(admission);
    body["queries"] = json!([]);
    body["targets"] = json!(targets);
    let response = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        body.clone(),
    )
    .await)["data"]
        .clone();
    (body, response)
}
fn reviewed(source: &Value) -> Value {
    json!({"entry_ref":source["entry_ref"],"version":source["version"],"start_line":3,"end_line":3})
}
fn progress_body(admission: &Value, selectors: Vec<Value>, status: &str, notes: &str) -> Value {
    let mut body = research_request(admission);
    body["notes"] = json!(notes);
    body["reviewed_sources"] = json!(selectors);
    body["pending_queries"] = json!([]);
    body["pending_targets"] = json!([]);
    body["status"] = json!(status);
    body
}
fn subject_candidate(admission: &Value, selectors: Vec<Value>) -> Value {
    json!({"kind":"summary","title":"Radley overview","summary":"A current person overview with one unresolved detail.","reason":"Retain independently supported facts despite one missing specification.",
        "subject_ref":admission["research"]["subject_ref"],"path":admission["research"]["output_path"],"expected_version":admission["research"]["output_version"],
        "content":"# Radley\n\nRadley is the canonical person.[^s1]\nThe current outcome is complete; one equipment detail remains unresolved.[^s2]\n", "sources":selectors})
}

#[tokio::test]
async fn linked_primary_second_round_replay_after_interleaving_and_exact_candidate_publication() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley has a project trail at [[sources/Notes/Trail.md]].\n",
        0,
    )
    .await;
    let trail = write(
        &f,
        "sources/Notes/Trail.md",
        "# Trail\n\nThe primary result is in [[sources/Outcomes/Final.md]].\n",
        0,
    )
    .await;
    let primary = write(
        &f,
        "sources/Outcomes/Final.md",
        "# Final\n\nAn old plan is awaiting execution.\n",
        0,
    )
    .await;
    let admission = admit(&f).await;
    assert_eq!(admission["research_protocol"], 1);
    let first = next_subject(&f, &admission).await;
    assert_eq!(first["research"]["subject_ref"], person["entry_ref"]);
    assert_eq!(first["research"]["sources"].as_array().unwrap().len(), 1);
    let frozen = first["frozen_generation"].clone();
    let (request_a, second) = discover_subject(&f, &first, vec![trail["path"].clone()]).await;
    assert_eq!(second["research"]["sources"].as_array().unwrap().len(), 2);
    let current_primary = write(
        &f,
        "sources/Outcomes/Final.md",
        "# Final\n\nThe current outcome is complete; one equipment detail remains unresolved.\n",
        1,
    )
    .await;
    let (_, third) = discover_subject(&f, &second, vec![primary["entry_ref"].clone()]).await;
    assert_eq!(
        third["frozen_generation"], frozen,
        "location/attempt snapshot must stay frozen"
    );
    assert!(third["research"]["snapshot_generation"].as_i64().unwrap() > frozen.as_i64().unwrap());
    assert!(
        third["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == primary["entry_ref"] && s["version"] == 2)
    );
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        request_a.clone(),
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(
        replay["data"]["research"]["version"],
        third["research"]["version"]
    );
    assert_eq!(
        replay["data"]["research"]["sources"],
        third["research"]["sources"]
    );
    let mut mismatched = request_a;
    mismatched["targets"] = json!(["sources/People/Radley.md"]);
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            mismatched
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        third["inputs"], admission["inputs"],
        "research discovery never consumes input"
    );

    let selectors = vec![reviewed(&person), reviewed(&current_primary)];
    let candidate = subject_candidate(&third, selectors.clone());
    let mut submit = research_request(&third);
    submit["candidates"] = json!([candidate]);
    submit["research_progress"] = json!({"notes":"The completed outcome supersedes the old plan. The equipment detail remains unresolved.","reviewed_sources":selectors,"pending_queries":[],"pending_targets":[],"status":"waiting"});
    submit["processed_inputs"] = json!([]);
    submit["findings"] = json!(["Useful overview despite an isolated unknown."]);
    let accepted = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        submit.clone(),
    )
    .await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        accepted["research"]["accepted_candidate_ids"],
        accepted["accepted_candidate_ids"]
    );
    let mut progress = progress_body(
        &accepted,
        vec![reviewed(&person), reviewed(&current_primary)],
        "waiting",
        "The completed outcome supersedes the old plan.",
    );
    progress["pending_targets"] = json!(["sources/Equipment/Specification.md"]);
    let later = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    let replay = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", submit).await);
    assert_eq!(
        replay["accepted_candidate_ids"],
        accepted["accepted_candidate_ids"]
    );
    assert_eq!(replay["state_version"], later["state_version"]);
    let view = review(&f).await;
    let item = &view["items"][0];
    assert_eq!(
        accepted["pending"][0]["candidate"]["subject_ref"],
        person["entry_ref"]
    );
    let metadata = current(
        &f,
        &format!(
            "dreams/research/{}.md",
            person["entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:")
        ),
    )
    .await
    .unwrap()
    .2;
    assert_eq!(
        metadata["dreamer_research"]["subject_ref"],
        person["entry_ref"]
    );
    // Publication still needs the owner decision; report-only retains approval.
    let approved = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, item, "approve"),
    )
    .await);
    assert!(approved.to_string().contains("approved_held"));
    assert!(
        current(&f, first["research"]["output_path"].as_str().unwrap())
            .await
            .is_none()
    );
    let original_identity = item["candidate_hash"].clone();
    let held_view = review(&f).await;
    assert_eq!(held_view["items"][0]["candidate_hash"], original_identity);
    let mut forbidden = research_request(&later);
    forbidden["expected_state_version"] = held_view["decision_version"].clone();
    forbidden["candidates"] = json!([subject_candidate(
        &later,
        vec![reviewed(&person), reviewed(&current_primary)]
    )]);
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/candidates", forbidden)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    finish(
        &f,
        &later,
        later["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    control(&f, "full", 1).await;
    admit(&f).await;
    let published = current(&f, first["research"]["output_path"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        published.2["dreamer_summary"]["subject_ref"],
        person["entry_ref"]
    );
    assert_eq!(
        published.2["dreamer_summary"]["subject_scope"]["subject_ref"],
        person["entry_ref"]
    );
    assert!(published.1.contains("current outcome is complete"));
    assert!(published.1.contains("equipment detail remains unresolved"));
    assert!(!published.1.contains("old plan is awaiting"));
}

#[tokio::test]
async fn research_restart_restores_checked_notes_and_waiting_subject_yields_to_ungrouped_input() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley has an unresolved specification.\n",
        0,
    )
    .await;
    let input = write(
        &f,
        "sources/Notes/Other.md",
        "# Other\n\nAn independent current fact.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let body = progress_body(
        &a,
        vec![reviewed(&person)],
        "researching",
        "The source records an unresolved specification.",
    );
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await)["data"]
        .clone();
    finish(
        &f,
        &saved,
        saved["state_version"].as_i64().unwrap(),
        "failed",
    )
    .await;
    let restarted = admit(&f).await;
    assert_eq!(
        restarted["research"]["notes"],
        "The source records an unresolved specification."
    );
    let continued = next_subject(&f, &restarted).await;
    // Fair input order may resume this person or advance the independent note.
    assert!(!continued["research"].is_null());
    let current_source = if continued["research"]["subject_ref"] == person["entry_ref"] {
        &person
    } else {
        &input
    };
    let waiting = progress_body(
        &continued,
        vec![reviewed(current_source)],
        "waiting",
        "The exact source supports a bounded unresolved lead.",
    );
    let after = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        waiting,
    )
    .await)["data"]
        .clone();
    let other = next_subject(&f, &after).await;
    assert_ne!(
        other["research"]["subject_ref"],
        continued["research"]["subject_ref"]
    );
    assert!(
        !other["research"].is_null(),
        "a waiting subject cannot monopolize the run"
    );
    assert_eq!(
        other["inputs"], restarted["inputs"],
        "routing and waiting do not acknowledge work"
    );
}

#[tokio::test]
async fn research_new_relevant_source_blocks_acceptance_and_discovery_reconciles_it() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley has a known plan.\n",
        0,
    )
    .await;
    let prior = write(
        &f,
        "sources/Plans/Past.md",
        "# Past\n\nRadley planned an outcome; one equipment detail remains unresolved.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let (_, a) = discover_subject(&f, &a, vec![prior["entry_ref"].clone()]).await;
    let newer = write(
        &f,
        "UnrelatedDirectory/News.md",
        "# News\n\nRadley completed the outcome today.\n",
        0,
    )
    .await;
    let mut submit = research_request(&a);
    submit["candidates"] = json!([subject_candidate(
        &a,
        vec![reviewed(&person), reviewed(&prior)]
    )]);
    submit["processed_inputs"] = json!([]);
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", submit).await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert!(rejected.body.to_string().contains("scope changed"));
    let (_, renewed) = discover_subject(&f, &a, vec![]).await;
    assert!(
        renewed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == newer["entry_ref"]),
        "all checked newly relevant changes must enter the research packet even outside search rank and source directories"
    );
    assert_eq!(renewed["research"]["notes"], "");
}

#[tokio::test]
async fn research_done_consumes_only_exact_reviewed_input_and_does_not_leak_after_source_change() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley source marker before correction.\n",
        0,
    )
    .await;
    let unrelated = write(
        &f,
        "sources/Notes/Unrelated.md",
        "# Unrelated\n\nUnreviewed work must remain.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let mut done = progress_body(
        &a,
        vec![reviewed(&person)],
        "no_change",
        "Checked source supports no useful change.",
    );
    done["processed_inputs"] = a["inputs"].clone();
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            done.clone()
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    done["processed_inputs"] = json!(
        a["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["entry_ref"] == person["entry_ref"])
            .collect::<Vec<_>>()
    );
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        done.clone(),
    )
    .await)["data"]
        .clone();
    assert!(
        saved["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == unrelated["entry_ref"])
    );
    assert!(
        !saved["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == person["entry_ref"])
    );
    write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nA corrected fact replaces the old source marker.\n",
        1,
    )
    .await;
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        done,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert!(
        !replay
            .to_string()
            .contains("Checked source supports no useful change"),
        "replay cannot restore stale cached conclusions"
    );
}

#[tokio::test]
async fn research_change_cap_preserves_scope_boundary_and_never_accepts_unchecked_candidate() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley has a stable source fact.\n",
        0,
    )
    .await;
    let unrelated = write(
        &f,
        "sources/Noise.md",
        "# Noise\n\nAn unrelated ordinary record.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let research_path = format!(
        "dreams/research/{}.md",
        person["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let baseline=current(&f,&research_path).await.unwrap().2["dreamer_research"]["scope"]["checked_generation"].clone();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT e.user_id,e.id,e.current_version,'update',e.path,v.content_sha256 FROM generate_series(1,2001) n CROSS JOIN brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.id=$2 ORDER BY n")
        .bind(f.owner.user).bind(Uuid::parse_str(unrelated["entry_ref"].as_str().unwrap().trim_start_matches("entry:")).unwrap()).execute(&f.pool).await.unwrap();
    let (_, refreshed) = discover_subject(&f, &a, vec![]).await;
    assert_eq!(
        refreshed["research"]["coverage"]["change_status"],
        "unchecked"
    );
    assert_eq!(
        current(&f, &research_path).await.unwrap().2["dreamer_research"]["scope"]["checked_generation"],
        baseline
    );
    let mut candidate = subject_candidate(&refreshed, vec![reviewed(&person)]);
    candidate["content"] = json!("# Radley\n\nA supported fact.[^s1]\n");
    let mut submit = research_request(&refreshed);
    submit["candidates"] = json!([candidate]);
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/candidates", submit)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert!(
        refreshed["research"]["coverage"]["change_cursor"]
            .as_i64()
            .unwrap()
            > baseline.as_i64().unwrap()
    );
    let (_, completed) = discover_subject(&f, &refreshed, vec![]).await;
    assert_eq!(
        completed["research"]["coverage"]["change_status"],
        "complete"
    );
    assert!(current(&f,&research_path).await.unwrap().2["dreamer_research"]["scope"]["checked_generation"].as_i64().unwrap()>baseline.as_i64().unwrap());
    let mut waiting = research_request(&completed);
    waiting["status"] = json!("waiting");
    let waiting = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        waiting,
    )
    .await)["data"]
        .clone();
    let independent = next_subject(&f, &waiting).await;
    assert_eq!(
        independent["research"]["subject_ref"],
        unrelated["entry_ref"]
    );
}

#[tokio::test]
async fn research_protected_record_withholds_notes_after_dependency_access_loss() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley is the canonical source.\n",
        0,
    )
    .await;
    let support = write(
        &f,
        "sources/Scoped/Support.md",
        "# Support\n\nPRIVATE_RESEARCH_CONCLUSION is an exact support fact.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let (_, a) = discover_subject(&f, &a, vec![support["entry_ref"].clone()]).await;
    let body = progress_body(
        &a,
        vec![reviewed(&person), reviewed(&support)],
        "researching",
        "PRIVATE_RESEARCH_CONCLUSION was supported by the exact source.",
    );
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body.clone(),
    )
    .await)["data"]
        .clone();
    let research_path = format!(
        "dreams/research/{}.md",
        person["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let forged=post(&f,&f.owner,"/v1/workspace/write",json!({"path":research_path,"content":"Forged conclusion","expected_version":saved["research"]["version"],"metadata":{}})).await;
    assert_eq!(forged.status, StatusCode::BAD_REQUEST);
    // Moving a source into the managed task namespace changes actual RLS access
    // for the Dreamer credential, including access to its historical versions.
    sqlx::query(
        "UPDATE brunn.entries SET path='.brunn/tasks/'||id::text||'.md' WHERE user_id=$1 AND id=$2",
    )
    .bind(f.owner.user)
    .bind(
        Uuid::parse_str(
            support["entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:"),
        )
        .unwrap(),
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await);
    assert!(!replay.to_string().contains("PRIVATE_RESEARCH_CONCLUSION"));
    assert_eq!(replay["data"]["research"]["notes"], "");
    let read = ok(post(
        &f,
        &f.model,
        "/v1/workspace/read",
        json!({"requests":[{"path":research_path}]}),
    )
    .await);
    assert!(!read.to_string().contains("PRIVATE_RESEARCH_CONCLUSION"));
}

#[tokio::test]
async fn research_project_registry_and_people_cursor_survive_restart_without_starving_input() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/First.md",
        "# First\n\nA canonical person source.\n",
        0,
    )
    .await;
    let project = write(
        &f,
        "sources/Hubs/Orchid.md",
        "# Orchid\n\nThe registered project hub.\n",
        0,
    )
    .await;
    let input = write(
        &f,
        "sources/Notes/Independent.md",
        "# Independent\n\nAn ungrouped source requiring attention.\n",
        0,
    )
    .await;
    sqlx::query("INSERT INTO brunn.task_projects(user_id,slug,title,hub_path) VALUES($1,'orchid','Orchid',$2)")
        .bind(f.owner.user).bind(project["path"].as_str().unwrap()).execute(&f.pool).await.unwrap();
    let a = next_subject(&f, &admit(&f).await).await;
    assert_eq!(a["research"]["subject_ref"], person["entry_ref"]);
    // Omitted notes on a scheduling yield preserve the last validated notebook.
    let progress = progress_body(
        &a,
        vec![reviewed(&person)],
        "researching",
        "A preserved source-backed checkpoint.",
    );
    let a = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    let mut waiting = research_request(&a);
    waiting["status"] = json!("waiting");
    let a = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        waiting,
    )
    .await)["data"]
        .clone();
    assert_eq!(
        a["research"]["notes"],
        "A preserved source-backed checkpoint."
    );
    finish(&f, &a, a["state_version"].as_i64().unwrap(), "partial").await;
    let mut a = admit(&f).await;
    let mut reached = std::collections::BTreeSet::new();
    for _ in 0..4 {
        a = next_subject(&f, &a).await;
        if a["research"].is_null() {
            continue;
        }
        let reference = a["research"]["subject_ref"].as_str().unwrap().to_owned();
        reached.insert(reference.clone());
        let source = if a["research"]["subject_ref"] == project["entry_ref"] {
            &project
        } else {
            &input
        };
        let progress = progress_body(
            &a,
            vec![reviewed(source)],
            "waiting",
            "The exact source supports a waiting checkpoint.",
        );
        a = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress,
        )
        .await)["data"]
            .clone();
    }
    assert!(reached.contains(project["entry_ref"].as_str().unwrap()));
    assert!(reached.contains(input["entry_ref"].as_str().unwrap()));
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert!(
        !state
            .2
            .to_string()
            .contains("preserved source-backed checkpoint"),
        "global scheduling state must not copy research conclusions"
    );
}

#[tokio::test]
async fn subject_revision_cannot_replace_another_subject_question_or_location_item() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let first = write(
        &f,
        "sources/People/First.md",
        "# First\n\nThe first canonical identity.\n",
        0,
    )
    .await;
    let second = write(
        &f,
        "sources/People/Second.md",
        "# Second\n\nA different canonical identity.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    assert_eq!(a["research"]["subject_ref"], first["entry_ref"]);
    let mut question = research_request(&a);
    question["candidates"] = json!([{"kind":"question","title":"First question","question":"A narrow unresolved need.","subject_ref":first["entry_ref"],"sources":[reviewed(&first)]}]);
    let accepted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", question).await);
    let b = next_subject(&f, &accepted).await;
    assert_eq!(b["research"]["subject_ref"], second["entry_ref"]);
    let mut unrelated = research_request(&b);
    unrelated["candidates"] = json!([{"kind":"question","title":"Other subject","question":"Different unresolved need.","subject_ref":second["entry_ref"],"sources":[reviewed(&second)],"revises_item_id":accepted["accepted_candidate_ids"][0]}]);
    let denied = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", unrelated).await;
    assert_eq!(denied.status, StatusCode::BAD_REQUEST, "{}", denied.body);
    assert!(denied.body.to_string().contains("canonical identity"));
    assert_eq!(
        review(&f).await["items"][0]["id"],
        accepted["accepted_candidate_ids"][0]
    );

    let Some(location) = fixture().await else {
        return;
    };
    control(&location, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&location).await;
    queue_pilot(&location, from).await;
    let person = write(
        &location,
        "sources/People/Radley.md",
        "# Radley\n\nA canonical person unrelated to location.\n",
        0,
    )
    .await;
    let mut a = admit(&location).await;
    let (_, submitted) = submit(
        &location,
        &a,
        a["state_version"].as_i64().unwrap(),
        vec![pilot_candidate(&a)],
    )
    .await;
    a["state_version"] = submitted["state_version"].clone();
    let a = next_subject(&location, &a).await;
    let mut revision = research_request(&a);
    let mut candidate = subject_candidate(&a, vec![reviewed(&person)]);
    candidate["content"] = json!("# Radley\n\nA supported person fact.[^s1]\n");
    candidate["revises_item_id"] = submitted["accepted_candidate_ids"][0].clone();
    revision["candidates"] = json!([candidate]);
    assert_eq!(
        post(
            &location,
            &location.runner,
            "/v1/workspace/dreamer/candidates",
            revision
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        review(&location).await["items"][0]["id"],
        submitted["accepted_candidate_ids"][0]
    );
}

#[tokio::test]
async fn review_withholds_subject_candidate_after_non_cited_dependency_is_deleted() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley is the canonical source.\n",
        0,
    )
    .await;
    let claim = write(
        &f,
        "sources/Claims/Outcome.md",
        "# Outcome\n\nThe current outcome is complete.\n",
        0,
    )
    .await;
    let dependency = write(
        &f,
        "sources/Chain/Bridge.md",
        "# Bridge\n\nAn authority used to research the outcome.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let (_, a) = discover_subject(
        &f,
        &a,
        vec![claim["entry_ref"].clone(), dependency["entry_ref"].clone()],
    )
    .await;
    let mut submit = research_request(&a);
    let mut candidate = subject_candidate(&a, vec![reviewed(&person), reviewed(&claim)]);
    candidate["title"] = json!("UNCITED_DEPENDENCY_PRIVATE_TITLE");
    submit["candidates"] = json!([candidate]);
    let accepted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", submit).await);
    assert_eq!(
        accepted["pending"][0]["candidate"]["subject_scope"]["dependencies"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    ok(request(
        &f.app,
        &f.owner,
        Method::DELETE,
        &format!(
            "/v1/workspace/entries/{}?expected_version=1",
            dependency["entry_ref"].as_str().unwrap()
        ),
        None,
    )
    .await);
    let view = review(&f).await;
    assert_eq!(view["items"][0]["title"], "Evidence unavailable");
    assert!(
        !view
            .to_string()
            .contains("UNCITED_DEPENDENCY_PRIVATE_TITLE")
    );
    assert!(!view.to_string().contains("current outcome is complete"));
    assert_eq!(view["items"][0]["reviewable"], false);
}

#[tokio::test]
async fn requested_subject_priority_survives_selection_and_yield_until_supported_completion() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/First.md",
        "# First\n\nA routinely seeded person.\n",
        0,
    )
    .await;
    let requested = write(
        &f,
        "sources/Notes/Requested.md",
        "# Requested\n\nThe owner requested this source's subject.\n",
        0,
    )
    .await;
    let a=ok(post(&f,&f.runner,"/v1/workspace/dreamer/admit",json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60,"requested_subject_refs":[format!("entry:{}", requested["entry_ref"].as_str().unwrap().trim_start_matches("entry:").to_uppercase()),requested["entry_ref"]]})).await);
    let a = next_subject(&f, &a).await;
    assert_eq!(a["research"]["subject_ref"], requested["entry_ref"]);
    let state = current(&f, "dreams/state.md").await.unwrap().2;
    assert_eq!(
        state["dreamer_state"]["research"]["requested_subject_refs"],
        json!([requested["entry_ref"]])
    );
    let mut waiting = research_request(&a);
    waiting["status"] = json!("waiting");
    let waiting = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        waiting,
    )
    .await)["data"]
        .clone();
    let other = next_subject(&f, &waiting).await;
    assert_eq!(other["research"]["subject_ref"], person["entry_ref"]);
    // A yielded subject stays queued, but cannot be repeatedly selected in one attempt.
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["requested_subject_refs"],
        json!([requested["entry_ref"]])
    );
    finish(
        &f,
        &other,
        other["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let resumed = next_subject(&f, &admit(&f).await).await;
    assert_eq!(resumed["research"]["subject_ref"], requested["entry_ref"]);
    let done = progress_body(
        &resumed,
        vec![reviewed(&requested)],
        "no_change",
        "The exact requested subject was checked.",
    );
    ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        done,
    )
    .await);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["requested_subject_refs"],
        json!([])
    );
}

#[tokio::test]
async fn stale_subject_held_approval_requires_new_review_in_report_only_mode() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nThe prior canonical fact.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let mut request = research_request(&a);
    let mut candidate = subject_candidate(&a, vec![reviewed(&person)]);
    candidate["content"] = json!("# Radley\n\nThe prior canonical fact.[^s1]\n");
    request["candidates"] = json!([candidate]);
    let accepted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", request).await);
    let before = review(&f).await;
    let identity = before["items"][0].clone();
    ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&before, &identity, "approve"),
    )
    .await);
    let newer = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nThe current corrected fact.\n",
        1,
    )
    .await;
    finish(
        &f,
        &accepted,
        accepted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let admitted = admit(&f).await;
    assert_eq!(admitted["pending"][0]["status"], "needs_changes");
    assert_eq!(
        admitted["pending"][0]["candidate_hash"],
        identity["candidate_hash"]
    );
    assert_eq!(admitted["pending"][0]["id"], identity["id"]);
    assert!(
        current(&f, a["research"]["output_path"].as_str().unwrap())
            .await
            .is_none()
    );
    let a = next_subject(&f, &admitted).await;
    let mut request = research_request(&a);
    let mut revision = subject_candidate(&a, vec![reviewed(&newer)]);
    revision["content"] = json!("# Radley\n\nThe corrected source fact.[^s1]\n");
    revision["revises_item_id"] = identity["id"].clone();
    request["candidates"] = json!([revision]);
    let revised = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", request).await);
    assert_eq!(revised["accepted_candidate_ids"], json!([identity["id"]]));
    assert_eq!(revised["pending"][0]["status"], "pending");
    assert_ne!(
        revised["pending"][0]["candidate_hash"],
        identity["candidate_hash"]
    );
}

#[tokio::test]
async fn oversized_relevant_source_keeps_coverage_unchecked_and_safe_yield_does_not_starve_other_subjects()
 {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nA canonical source fact.\n",
        0,
    )
    .await;
    let other = write(
        &f,
        "sources/Notes/Independent.md",
        "# Independent\n\nAn unrelated source fact.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let large = write(
        &f,
        "sources/Large/Primary.md",
        "# Primary\n\nRadley has supporting primary evidence.\n",
        0,
    )
    .await;
    // Materialize an authentic large inline version in this isolated fixture;
    // the ordinary intake header deliberately excludes records over 1 MiB.
    let content = format!(
        "# Primary\n\nRadley has supporting primary evidence.\n{}",
        "Large source detail.\n".repeat(60000)
    );
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    sqlx::query("UPDATE brunn.entry_versions SET content=$3,size_bytes=$4,content_sha256=$5 WHERE user_id=$1 AND entry_id=$2 AND version=1")
        .bind(f.owner.user).bind(Uuid::parse_str(large["entry_ref"].as_str().unwrap().trim_start_matches("entry:")).unwrap()).bind(&content).bind(content.len() as i64).bind(hash).execute(&f.pool).await.unwrap();
    let (_, refreshed) = discover_subject(&f, &a, vec![]).await;
    assert_eq!(
        refreshed["research"]["coverage"]["change_status"],
        "unchecked"
    );
    assert!(
        refreshed["research"]["coverage"]["pending_source_refs"]
            .as_array()
            .unwrap()
            .contains(&large["entry_ref"])
    );
    let mut waiting = research_request(&refreshed);
    waiting["status"] = json!("waiting");
    let yielded = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        waiting,
    )
    .await)["data"]
        .clone();
    assert_eq!(yielded["research"]["notes"], "");
    let next = next_subject(&f, &yielded).await;
    assert_ne!(next["research"]["subject_ref"], person["entry_ref"]);
    assert_eq!(next["research"]["subject_ref"], other["entry_ref"]);
}

#[tokio::test]
async fn automatic_invalid_subject_is_skipped_without_rolling_back_the_selection_cursor() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let invalid = write(
        &f,
        "sources/People/First.md",
        &format!(
            "# {}\n\nAn ordinary source with an overlong canonical title.\n",
            "x".repeat(161)
        ),
        0,
    )
    .await;
    let valid = write(
        &f,
        "sources/People/Second.md",
        "# Second\n\nA valid canonical source.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    assert_eq!(a["research"]["subject_ref"], valid["entry_ref"]);
    let state = current(&f, "dreams/state.md").await.unwrap().2;
    assert_eq!(
        state["dreamer_state"]["research"]["people_after"],
        valid["path"]
    );
    assert_eq!(
        state["dreamer_state"]["research"]["skipped_subjects"][0]["entry_ref"],
        invalid["entry_ref"]
    );
    finish(&f, &a, a["state_version"].as_i64().unwrap(), "partial").await;
    let explicit=post(&f,&f.runner,"/v1/workspace/dreamer/admit",json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","requested_subject_refs":[invalid["entry_ref"]]})).await;
    assert_eq!(
        explicit.status,
        StatusCode::BAD_REQUEST,
        "explicit invalid requested identities remain a useful validation error"
    );
}

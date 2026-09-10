//! Included by the real HTTP/database review fixture, not a standalone model.
use super::*;
mod checkpoint;
mod comparison;
mod repair;
mod revalidation;

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
fn assert_same_source_headers(actual: &Value, expected: &Value) {
    // SQL may reorder supporting sources on reload. Compare every complete
    // header while preserving the explicit canonical-first contract.
    let mut actual = actual.as_array().unwrap().iter().collect::<Vec<_>>();
    let mut expected = expected.as_array().unwrap().iter().collect::<Vec<_>>();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual.first(), expected.first());
    for sources in [&mut actual, &mut expected] {
        sources.sort_by(|left, right| left["entry_ref"].as_str().cmp(&right["entry_ref"].as_str()));
    }
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert_eq!(actual, expected, "retain each source's exact header");
    }
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
async fn discovery_policy_upgrade_retries_legacy_query_without_replaying_accepted_operations() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Cedar.md",
        "# Cedar\n\nCedar has a checked primary observation.\n",
        0,
    )
    .await;
    let primary = write(
        &f,
        "sources/Optics/Measurement.md",
        "# Measurement\n\nThe detector schedule records the exposure interval.\n",
        0,
    )
    .await;
    let query = "Please inspect photometry calibration observatory detector schedule and explain what the current request should report.";
    let primary_id = Uuid::parse_str(
        primary["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap();
    let (indexed, strict_match): (i64, Option<bool>) = sqlx::query_as(
        "SELECT count(*),bool_or(search_vector @@ websearch_to_tsquery('english',$3)) FROM brunn.search_chunks WHERE user_id=$1 AND entry_id=$2",
    )
    .bind(f.owner.user)
    .bind(primary_id)
    .bind(query)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(indexed > 0, "the primary is already searchable");
    assert_eq!(strict_match, Some(false), "the old strict AND missed it");

    let selected = next_subject(&f, &admit(&f).await).await;
    assert_eq!(selected["research"]["subject_ref"], canonical["entry_ref"]);
    assert_eq!(selected["research"]["sources"].as_array().unwrap().len(), 1);
    assert!(
        primary["workspace_generation"].as_i64().unwrap()
            <= selected["research"]["snapshot_generation"]
                .as_i64()
                .unwrap()
    );
    let notes = "The canonical observation is checked; further source discovery remains open.";
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(&selected, vec![reviewed(&canonical)], "researching", notes),
    )
    .await)["data"]
        .clone();
    let research_path = format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let mut legacy_request = research_request(&saved);
    legacy_request["queries"] = json!([query]);
    legacy_request["targets"] = json!([]);
    let legacy_query_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&json!({"queries":[query.to_lowercase()],"targets":[]})).unwrap(),
    ));
    let mut legacy_payload = legacy_request.clone();
    legacy_payload
        .as_object_mut()
        .unwrap()
        .remove("expected_state_version");
    let legacy_request_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&json!({"kind":"narrative-discover","payload":legacy_payload})).unwrap(),
    ));
    let mut legacy = current(&f, &research_path).await.unwrap().2;
    legacy["dreamer_research"]["round"] = json!(1);
    legacy["dreamer_research"]["discoveries"] = json!([{
        "query_hash":legacy_query_hash,"generation":saved["research"]["snapshot_generation"]
    }]);
    legacy["dreamer_research"]["receipts"]
        .as_array_mut()
        .unwrap()
        .push(
            json!({"operation_id":legacy_request["operation_id"],"request_hash":legacy_request_hash,
            "producer":f.runner.id,"result":{"round":1,"no_op":false}}),
        );
    // Model only this disposable fixture's pre-upgrade record. No source,
    // generation or exact operation identity changes during the policy upgrade.
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$3 FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(&research_path).bind(legacy).execute(&f.pool).await.unwrap();
    let legacy_job = current(&f, &research_path).await.unwrap();
    let legacy_state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        legacy_request.clone(),
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(replay["data"]["research"]["notes"], notes);
    assert_ne!(replay["data"]["research"]["needs_refresh"], true);
    assert_eq!(
        replay["data"]["research"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(current(&f, &research_path).await.unwrap(), legacy_job);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), legacy_state);

    let mut discovery = research_request(&replay["data"]);
    discovery["queries"] = json!([query]);
    discovery["targets"] = json!([]);
    let discovered = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        discovery.clone(),
    )
    .await);
    assert_eq!(discovered["no_op"], false);
    let discovered = discovered["data"].clone();
    assert_eq!(discovered["research"]["round"], 2);
    assert_eq!(
        discovered["research"]["sources"].as_array().unwrap().len(),
        2
    );
    assert!(
        discovered["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == primary["entry_ref"] && s["version"] == primary["version"])
    );
    assert_eq!(discovered["research"]["notes"], notes);
    assert_eq!(
        discovered["research"]["reviewed_sources"],
        saved["research"]["reviewed_sources"]
    );
    assert_eq!(discovered["inputs"], saved["inputs"]);
    assert_eq!(
        discovered["processed_generation"],
        saved["processed_generation"]
    );
    let discovered_job = current(&f, &research_path).await.unwrap();
    let discoveries = discovered_job.2["dreamer_research"]["discoveries"]
        .as_array()
        .unwrap();
    assert_eq!(discoveries.len(), 2);
    assert_eq!(discoveries[0]["query_hash"], legacy_query_hash);
    assert_ne!(discoveries[1]["query_hash"], legacy_query_hash);

    let mut repeated = research_request(&discovered);
    repeated["queries"] = json!([format!(
        "  {}  ",
        query.to_uppercase().replacen(' ', "   ", 1)
    )]);
    repeated["targets"] = json!([]);
    let repeated = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        repeated,
    )
    .await);
    assert_eq!(repeated["no_op"], true);
    let repeated = repeated["data"].clone();
    assert_eq!(
        repeated["research"]["round"],
        discovered["research"]["round"]
    );
    assert_eq!(repeated["research"]["coverage"]["query_results"], json!([]));
    assert_same_source_headers(
        &repeated["research"]["sources"],
        &discovered["research"]["sources"],
    );
    assert_eq!(repeated["research"]["notes"], notes);
    assert_eq!(
        repeated["research"]["reviewed_sources"],
        saved["research"]["reviewed_sources"]
    );

    let later_notes =
        "Both the canonical observation and the discovered measurement are now checked.";
    let later = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &repeated,
            vec![reviewed(&canonical), reviewed(&primary)],
            "researching",
            later_notes,
        ),
    )
    .await)["data"]
        .clone();
    let durable_job = current(&f, &research_path).await.unwrap();
    let durable_state = current(&f, "dreams/state.md").await.unwrap();
    for original in [legacy_request, discovery] {
        let replay = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            original,
        )
        .await);
        assert_eq!(replay["no_op"], true);
        assert_eq!(replay["data"]["research"]["notes"], later_notes);
        assert_eq!(
            replay["data"]["research"]["version"],
            later["research"]["version"]
        );
        assert_eq!(replay["data"]["state_version"], later["state_version"]);
        assert_eq!(
            replay["data"]["research"]["reviewed_sources"],
            later["research"]["reviewed_sources"]
        );
        assert_same_source_headers(
            &replay["data"]["research"]["sources"],
            &later["research"]["sources"],
        );
        assert_eq!(
            current(&f, &research_path).await.unwrap(),
            durable_job,
            "old-policy and current-policy operation replay must not rerun or rewrite history"
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), durable_state);
    }
    f.pool.close().await;
}

async fn briefing_fixture_source(f: &Fixture, path: &str, version: i64, metadata: Value) -> Value {
    ok(post(f, &f.owner, "/v1/workspace/write", json!({
        "path":path,"content":format!("# Edition context\n\nOrchid has an established source observation.\n\nFixture revision {version}.\n"),
        "expected_version":version,"metadata":metadata
    })).await)["data"].clone()
}

async fn mark_legacy_briefing_version(f: &Fixture, source: &Value) {
    // Model a record already classified under the former admission policy,
    // without changing its exact evidence version or the change cursor.
    sqlx::query("UPDATE brunn.entry_versions SET metadata=metadata||'{\"kind\":\"briefing_edition\"}'::jsonb WHERE user_id=$1 AND entry_id=$2 AND version=$3")
        .bind(f.owner.user)
        .bind(Uuid::parse_str(source["entry_ref"].as_str().unwrap().trim_start_matches("entry:")).unwrap())
        .bind(source["version"].as_i64().unwrap())
        .execute(&f.pool).await.unwrap();
}

async fn exact_run_record(f: &Fixture, receipt: &Value) -> (String, Value) {
    sqlx::query_as("SELECT content,metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
        .bind(f.owner.user)
        .bind(Uuid::parse_str(receipt["run_entry_ref"].as_str().unwrap().trim_start_matches("entry:")).unwrap())
        .bind(receipt["run_version"].as_i64().unwrap())
        .fetch_one(&f.pool).await.unwrap()
}

#[tokio::test]
async fn generated_briefing_legacy_held_candidate_does_not_abort_full_admission() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let source = briefing_fixture_source(&f, "Briefings/Legacy.md", 0, json!({})).await;
    let first = admit(&f).await;
    let (_, accepted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "legacy-held")],
    )
    .await;
    finish(
        &f,
        &first,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(&f).await;
    let original = view["items"][0].clone();
    assert_eq!(original["stale"], false);
    let held = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &original, "approve"),
    )
    .await);
    assert_eq!(held["data"]["application_status"], "approved_held");
    let immutable = exact_run_record(&f, &accepted).await;
    mark_legacy_briefing_version(&f, &source).await;
    let stale = review(&f).await;
    assert_eq!(stale["items"][0]["stale"], true);
    assert_eq!(
        stale["items"][0]["candidate_hash"],
        original["candidate_hash"]
    );
    control(&f, "full", 1).await;
    let next = admit(&f).await;
    assert_eq!(next["admitted"], true);
    assert_eq!(next["pending"][0]["status"], "needs_changes");
    assert_eq!(
        next["pending"][0]["candidate_hash"],
        original["candidate_hash"]
    );
    assert!(
        current(&f, "derived/entities/legacy-held.md")
            .await
            .is_none()
    );
    assert_eq!(exact_run_record(&f, &accepted).await, immutable);
}

#[tokio::test]
async fn generated_briefing_churn_keeps_an_ordinary_legacy_prefix_candidate_fresh() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let source = briefing_fixture_source(&f, "Briefings/Primary.md", 0, json!({})).await;
    let first = admit(&f).await;
    let (_, accepted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "legacy-prefix")],
    )
    .await;
    let original = review(&f).await["items"][0].clone();
    assert_eq!(original["stale"], false);
    let immutable = exact_run_record(&f, &accepted).await;
    let generated = briefing_fixture_source(
        &f,
        "Briefings/Edition.md",
        0,
        json!({"kind":"briefing_edition"}),
    )
    .await;
    briefing_fixture_source(
        &f,
        generated["path"].as_str().unwrap(),
        1,
        json!({"kind":"briefing_edition"}),
    )
    .await;
    let fresh = review(&f).await;
    assert_eq!(fresh["items"][0]["stale"], false);
    assert_eq!(
        fresh["items"][0]["candidate_hash"],
        original["candidate_hash"]
    );
    // The same prefix still detects the edition becoming primary evidence.
    briefing_fixture_source(&f, generated["path"].as_str().unwrap(), 2, json!({})).await;
    assert_eq!(review(&f).await["items"][0]["stale"], true);
    assert_eq!(exact_run_record(&f, &accepted).await, immutable);
}

#[tokio::test]
async fn generated_briefing_location_candidates_recheck_cited_and_uncited_context() {
    // A canonical citation exercises the evidence_scope early return; an
    // uncited retained context exercises exact and current metadata separately.
    for excluded in ["canonical", "exact_context", "current_context"] {
        let Some(f) = fixture().await else { return };
        control(&f, "report-only", 0).await;
        let (from, _, _) = seed_location_pilot(&f).await;
        let mut context = None;
        let mut current_context = None;
        if excluded != "canonical" {
            context = Some(
                historical_context(
                    &f,
                    from,
                    "sources/Context/Owner.md",
                    "# Owner\n\nI identify the stop as Example Garden.\n",
                )
                .await,
            );
            current_context = Some(
                write(
                    &f,
                    "sources/Context/Owner.md",
                    "# Current owner note\n\nExample Garden remains a known venue.\n",
                    1,
                )
                .await,
            );
        }
        queue_pilot(&f, from).await;
        let mut admitted = admit(&f).await;
        if excluded != "canonical" {
            admitted = discover_context(&f, &admitted, "Example Garden").await;
            assert_eq!(admitted["location_context"].as_array().unwrap().len(), 1);
            assert_eq!(admitted["location_context"][0]["version"], 1);
            assert_eq!(admitted["location_context"][0]["current_version"], 2);
        }
        let mut proposal = pilot_candidate(&admitted);
        if excluded != "canonical" {
            proposal["evidence_scope"]["context_sources"] =
                admitted["location_work"]["context_sources"].clone();
        }
        assert_eq!(
            proposal["sources"].as_array().unwrap().len(),
            1,
            "retained context remains uncited"
        );
        let (_, accepted) = submit(
            &f,
            &admitted,
            admitted["state_version"].as_i64().unwrap(),
            vec![proposal],
        )
        .await;
        finish(
            &f,
            &admitted,
            accepted["state_version"].as_i64().unwrap(),
            "completed",
        )
        .await;
        let view = review(&f).await;
        let original = view["items"][0].clone();
        assert_eq!(original["stale"], false, "{excluded}");
        let held = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, &original, "approve"),
        )
        .await);
        assert_eq!(held["data"]["application_status"], "approved_held");
        let immutable = exact_run_record(&f, &accepted).await;
        let source = match excluded {
            "canonical" => {
                json!({"entry_ref":admitted["location_evidence"]["canonical_months"][0]["ref"],"version":admitted["location_evidence"]["canonical_months"][0]["version"]})
            }
            "exact_context" => context.unwrap(),
            _ => current_context.unwrap(),
        };
        mark_legacy_briefing_version(&f, &source).await;
        let stale = review(&f).await;
        assert_eq!(stale["items"][0]["stale"], true, "{excluded}");
        assert_eq!(
            stale["items"][0]["candidate_hash"],
            original["candidate_hash"]
        );
        let denied = post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&stale, &stale["items"][0], "approve"),
        )
        .await;
        assert_eq!(denied.status, StatusCode::CONFLICT, "{}", denied.body);
        assert_eq!(exact_run_record(&f, &accepted).await, immutable);
        assert!(
            current(&f, &format!("derived/location/{}.md", from.date_naive()))
                .await
                .is_none()
        );
    }
}

#[tokio::test]
async fn generated_briefing_editions_are_excluded_without_excluding_ordinary_briefing_notes() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let legacy = briefing_fixture_source(
        &f,
        "Briefings/2026/Morning.md",
        0,
        json!({"kind":"briefing_edition"}),
    )
    .await;
    let edition = briefing_fixture_source(
        &f,
        "sources/GeneratedEdition.md",
        0,
        json!({"kind":"briefing_edition","briefing":{"schema":"briefing.edition.v1"}}),
    )
    .await;
    let ordinary = briefing_fixture_source(&f, "Briefings/Ordinary.md", 0, json!({})).await;
    let annotated = briefing_fixture_source(
        &f,
        "sources/Annotated.md",
        0,
        json!({"briefing":{"note":"an ordinary source annotation"}}),
    )
    .await;
    let admitted = admit(&f).await;
    let inputs = admitted["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 3);
    for source in [&canonical, &ordinary, &annotated] {
        assert!(
            inputs
                .iter()
                .any(|input| input["entry_ref"] == source["entry_ref"])
        );
    }
    let selected = next_subject(&f, &admitted).await;
    let (_, discovered) = discover_subject(
        &f,
        &selected,
        vec![
            legacy["path"].clone(),
            edition["entry_ref"].clone(),
            ordinary["path"].clone(),
            annotated["entry_ref"].clone(),
        ],
    )
    .await;
    assert_eq!(
        discovered["research"]["coverage"]["unresolved_targets"],
        json!([legacy["path"], edition["entry_ref"]])
    );
    let sources = discovered["research"]["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 3);
    for source in [&canonical, &ordinary, &annotated] {
        assert!(
            sources
                .iter()
                .any(|input| input["entry_ref"] == source["entry_ref"])
        );
    }
    // Evidence policy does not make briefing storage or exact reads protected.
    let updated = briefing_fixture_source(
        &f,
        legacy["path"].as_str().unwrap(),
        1,
        json!({"kind":"briefing_edition"}),
    )
    .await;
    assert_eq!(updated["version"], 2);
    let read = ok(post(
        &f,
        &f.owner,
        "/v1/workspace/read",
        json!({"requests":[{"path":legacy["path"],"view":"full"}]}),
    )
    .await);
    assert!(
        read.to_string()
            .contains("Orchid has an established source observation.")
    );
    finish(
        &f,
        &discovered,
        discovered["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let denied = post(&f, &f.runner, "/v1/workspace/dreamer/admit", json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60,"requested_subject_refs":[legacy["entry_ref"]]})).await;
    assert_eq!(denied.status, StatusCode::BAD_REQUEST, "{}", denied.body);
}

#[tokio::test]
async fn generated_briefing_retained_inputs_receive_exclusions_without_model_processing() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let legacy = briefing_fixture_source(&f, "Briefings/Legacy.md", 0, json!({})).await;
    let transition = briefing_fixture_source(&f, "Briefings/Transition.md", 0, json!({})).await;
    let first = admit(&f).await;
    assert_eq!(first["inputs"].as_array().unwrap().len(), 3);
    finish(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    mark_legacy_briefing_version(&f, &legacy).await;
    briefing_fixture_source(
        &f,
        transition["path"].as_str().unwrap(),
        1,
        json!({"kind":"briefing_edition"}),
    )
    .await;
    let second = admit(&f).await;
    assert_eq!(second["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(second["inputs"][0]["entry_ref"], canonical["entry_ref"]);
    let stored = current(&f, "dreams/state.md").await.unwrap().2;
    assert_eq!(stored["dreamer_state"]["processed_count"], 0);
    let dispositions = stored["dreamer_state"]["source_dispositions"]
        .as_array()
        .unwrap();
    assert_eq!(dispositions.len(), 2);
    for source in [&legacy, &transition] {
        let input = first["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|input| input["entry_ref"] == source["entry_ref"])
            .unwrap();
        let disposition = dispositions
            .iter()
            .find(|item| item["entry_ref"] == source["entry_ref"])
            .unwrap();
        assert_eq!(disposition["disposition"], "excluded_generated_briefing");
        assert_eq!(disposition["version"], input["version"]);
        assert_eq!(disposition["generation"], input["generation"]);
        assert!(
            disposition["detail"]
                .as_str()
                .unwrap()
                .contains("not model processing")
        );
    }
    let (_, terminal) = finish(
        &f,
        &second,
        second["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    assert_eq!(terminal["counts"]["processed"], 0);
    let run = current(&f, &format!("dreams/runs/{}.md", date()))
        .await
        .unwrap();
    assert_eq!(
        run.2["dreamer_run"]["source_dispositions"],
        json!(dispositions)
    );
    let third = admit(&f).await;
    assert_eq!(third["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(third["inputs"][0]["entry_ref"], canonical["entry_ref"]);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["processed_count"],
        0
    );
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["source_dispositions"],
        json!([]),
        "an excluded retained identity is not repeatedly dispositioned"
    );
}

#[tokio::test]
async fn generated_briefing_legacy_research_and_candidates_invalidate_without_rewriting_dependencies()
 {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let legacy = briefing_fixture_source(&f, "Briefings/Legacy.md", 0, json!({})).await;
    let selected = next_subject(&f, &admit(&f).await).await;
    let (_, selected) = discover_subject(&f, &selected, vec![legacy["entry_ref"].clone()]).await;
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &selected,
            vec![reviewed(&canonical), reviewed(&legacy)],
            "researching",
            "Legacy conclusions used the briefing dependency.",
        ),
    )
    .await)["data"]
        .clone();
    let mut submission = research_request(&saved);
    submission["candidates"] = json!([subject_candidate(
        &saved,
        vec![reviewed(&canonical), reviewed(&legacy)]
    )]);
    submission["processed_inputs"] = json!([]);
    let accepted = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        submission,
    )
    .await);
    let before = review(&f).await;
    let immutable_run_path = format!("dreams/runs/{}.md", date());
    let immutable_run = current(&f, &immutable_run_path).await.unwrap();
    let research_path = format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    mark_legacy_briefing_version(&f, &legacy).await;
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &research_path).await.unwrap();
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &accepted,
            vec![reviewed(&canonical), reviewed(&legacy)],
            "researching",
            "Do not retain new conclusions from generated evidence.",
        ),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    assert!(
        rejected
            .body
            .to_string()
            .contains("generated briefing editions cannot be source evidence"),
        "{}",
        rejected.body
    );
    // A retained ordinary candidate path must obey the same exact-source gate.
    let mut request = attempt(&accepted, accepted["state_version"].as_i64().unwrap());
    request["candidates"] = json!([candidate(&legacy, "briefing-forbidden")]);
    request["processed_inputs"] = json!([]);
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", request).await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    assert!(
        rejected
            .body
            .to_string()
            .contains("generated briefing editions cannot be source evidence"),
        "{}",
        rejected.body
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &research_path).await.unwrap(), job);
    let view = review(&f).await;
    assert_eq!(
        view["items"][0]["candidate_hash"],
        before["items"][0]["candidate_hash"]
    );
    let denied = post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &view["items"][0], "approve"),
    )
    .await;
    assert_eq!(denied.status, StatusCode::CONFLICT, "{}", denied.body);
    assert_eq!(
        current(&f, &immutable_run_path).await.unwrap(),
        immutable_run
    );
    let (_, refreshed) = discover_subject(&f, &accepted, vec![]).await;
    assert_eq!(
        refreshed["research"]["sources"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        refreshed["research"]["sources"][0]["entry_ref"],
        canonical["entry_ref"]
    );
    assert_eq!(refreshed["research"]["notes"], "");
    assert_eq!(refreshed["research"]["reviewed_sources"], json!([]));
    ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &refreshed,
            vec![reviewed(&canonical)],
            "researching",
            "Reconsidered using only the canonical primary source.",
        ),
    )
    .await);
    assert_eq!(
        current(&f, &immutable_run_path).await.unwrap(),
        immutable_run,
        "refresh must not prune an old proposal's immutable dependency manifest"
    );
}

#[tokio::test]
async fn compact_research_checkpoint_retains_64_large_ranges_across_additions_replay_and_reload() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let mut sources = Vec::new();
    let mut hydrated_bytes = 0;
    for index in 0..64 {
        let path = if index == 0 {
            "sources/People/Orchid.md".to_owned()
        } else {
            format!("sources/Checkpoint/Source-{index:02}.md")
        };
        let title = if index == 0 {
            "Orchid".to_owned()
        } else {
            format!("Evidence {index}")
        };
        let content = format!(
            "# {title}\n\nSynthetic observation {index}.\nRecord-{index:02}: {}\n",
            "Retained source detail. ".repeat(190)
        );
        let excerpt = content.lines().skip(2).collect::<Vec<_>>().join("\n");
        assert!(excerpt.len() < 32 * 1024);
        hydrated_bytes += excerpt.len();
        sources.push(write(&f, &path, &content, 0).await);
    }
    assert!(
        hydrated_bytes > 192 * 1024,
        "the previous excerpt-bearing checkpoint must exceed the existing job cap"
    );
    let extra = imported_link_source(&f, "sources/Checkpoint/Additional.md").await;
    let mut selected = next_subject(&f, &admit(&f).await).await;
    assert_eq!(selected["research"]["subject_ref"], sources[0]["entry_ref"]);
    for page in sources[1..].chunks(32) {
        (_, selected) = discover_subject(
            &f,
            &selected,
            page.iter().map(|s| s["entry_ref"].clone()).collect(),
        )
        .await;
    }
    let selectors = sources.iter().map(|s|json!({"entry_ref":s["entry_ref"],"version":s["version"],"start_line":3,"end_line":4})).collect::<Vec<_>>();
    let expected = sources.iter().map(|s|json!({"entry_ref":s["entry_ref"],"version":s["version"],"start_line":3,"end_line":4,"path":s["path"]})).collect::<Vec<_>>();
    let notes = format!("All 64 exact source ranges were checked.\n\n{}", "Keep the checked observations and their unresolved follow-up leads available for later research.\n".repeat(80));
    assert!(notes.len() > 6 * 1024 && notes.len() < 12 * 1024);
    let original = progress_body(&selected, selectors.clone(), "researching", &notes);
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        original.clone(),
    )
    .await)["data"]
        .clone();
    assert_eq!(saved["research"]["notes"], notes);
    assert_eq!(saved["research"]["reviewed_sources"], json!(expected));
    assert_eq!(saved["inputs"], selected["inputs"]);
    let research_path = format!(
        "dreams/research/{}.md",
        sources[0]["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let durable = current(&f, &research_path).await.unwrap();
    assert_eq!(
        durable.2["dreamer_research"]["reviewed_sources"],
        json!(expected)
    );
    assert_eq!(durable.2["dreamer_research"]["notes"], notes);
    assert!(serde_json::to_vec(&durable.2).unwrap().len() < 192 * 1024);
    let (_, expanded) = discover_subject(&f, &saved, vec![extra["entry_ref"].clone()]).await;
    assert_eq!(
        expanded["research"]["sources"].as_array().unwrap().len(),
        65
    );
    assert_eq!(expanded["research"]["reviewed_sources"], json!(expected));
    assert_eq!(expanded["research"]["notes"], notes);
    let later_notes = format!("{notes}\nThe additional source remains an unreviewed lead.");
    let later = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(&expanded, selectors.clone(), "researching", &later_notes),
    )
    .await)["data"]
        .clone();
    let durable = current(&f, &research_path).await.unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        original,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(replay["data"]["research"]["notes"], later_notes);
    assert_eq!(
        replay["data"]["research"]["reviewed_sources"],
        json!(expected)
    );
    assert_eq!(
        replay["data"]["research"]["version"],
        later["research"]["version"]
    );
    assert_eq!(
        current(&f, &research_path).await.unwrap(),
        durable,
        "replay must not rewrite or duplicate stored progress"
    );
    finish(
        &f,
        &later,
        later["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let restarted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/admit", json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60,"requested_subject_refs":[sources[0]["entry_ref"]]})).await);
    assert_eq!(restarted["research"]["notes"], later_notes);
    let resumed = next_subject(&f, &restarted).await;
    assert_eq!(resumed["research"]["subject_ref"], sources[0]["entry_ref"]);
    assert_eq!(resumed["research"]["reviewed_sources"], json!(expected));
    assert_eq!(resumed["research"]["notes"], later_notes);

    let mut oversized = research_request(&resumed);
    oversized["candidates"] = json!([subject_candidate(&resumed, selectors)]);
    oversized["processed_inputs"] = json!([]);
    let durable = current(&f, &research_path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", oversized).await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    assert!(
        rejected
            .body
            .to_string()
            .contains("candidate exceeds 32 KiB"),
        "{}",
        rejected.body
    );
    assert_eq!(current(&f, &research_path).await.unwrap(), durable);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    let mut candidate =
        subject_candidate(&resumed, vec![reviewed(&sources[0]), reviewed(&sources[1])]);
    candidate["title"] = json!("Orchid source observations");
    candidate["content"] = json!(
        "# Orchid\n\nThe canonical source records an observation.[^s1]\nA supporting source records another observation.[^s2]\n"
    );
    let mut request = research_request(&resumed);
    request["candidates"] = json!([candidate]);
    request["processed_inputs"] = json!([]);
    let accepted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", request).await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    assert_eq!(accepted["research"]["reviewed_sources"], json!(expected));
    let run: Value = sqlx::query_scalar(
        "SELECT metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3",
    )
    .bind(f.owner.user)
    .bind(
        Uuid::parse_str(
            accepted["run_entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:"),
        )
        .unwrap(),
    )
    .bind(accepted["run_version"].as_i64().unwrap())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let item = run["dreamer_run"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == accepted["accepted_candidate_ids"][0])
        .unwrap();
    assert_eq!(
        item["candidate"]["sources"][0]["excerpt"],
        "Synthetic observation 0."
    );
    assert_eq!(
        item["candidate"]["sources"][1]["excerpt"],
        "Synthetic observation 1."
    );
    assert_eq!(item["candidate"]["sources"][0]["path"], sources[0]["path"]);
}

#[tokio::test]
async fn compact_research_checkpoint_loads_legacy_excerpts_and_preserves_validation() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nThe original canonical observation.\n",
        0,
    )
    .await;
    let support = imported_link_source(&f, "sources/Notes/Support.md").await;
    let selected = next_subject(&f, &admit(&f).await).await;
    let (_, admitted) = discover_subject(&f, &selected, vec![support["entry_ref"].clone()]).await;
    let notes = "LEGACY_RESEARCH_NOTES: both exact sources were checked.";
    let original = progress_body(
        &admitted,
        vec![reviewed(&canonical), reviewed(&support)],
        "researching",
        notes,
    );
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        original.clone(),
    )
    .await)["data"]
        .clone();
    let research_path = format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let mut legacy = current(&f, &research_path).await.unwrap().2;
    legacy["dreamer_research"]["reviewed_sources"][0]["excerpt"] =
        json!("The original canonical observation.");
    legacy["dreamer_research"]["reviewed_sources"][1]["excerpt"] =
        json!("A synthetic source observation.");
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$3 FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(&research_path).bind(&legacy).execute(&f.pool).await.unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        original,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(replay["data"]["research"]["notes"], notes);
    assert_eq!(
        replay["data"]["research"]["reviewed_sources"],
        saved["research"]["reviewed_sources"]
    );
    assert!(
        replay["data"]["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s.get("excerpt").is_none())
    );
    assert_eq!(
        current(&f, &research_path).await.unwrap().2,
        legacy,
        "reading an old record must not rewrite its stored version"
    );
    let read = ok(post(
        &f,
        &f.model,
        "/v1/workspace/read",
        json!({"requests":[{"path":research_path,"view":"full"}]}),
    )
    .await);
    assert!(read.to_string().contains(notes));
    let mut checkpoint = research_request(&replay["data"]);
    checkpoint["status"] = json!("researching");
    let compact = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        checkpoint.clone(),
    )
    .await)["data"]
        .clone();
    let stored = current(&f, &research_path).await.unwrap();
    assert!(stored.0 > saved["research"]["version"].as_i64().unwrap());
    assert_eq!(stored.2["dreamer_research"]["notes"], notes);
    assert_eq!(
        stored.2["dreamer_research"]["reviewed_sources"],
        saved["research"]["reviewed_sources"]
    );
    let corrected = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nThe corrected canonical observation.\n",
        1,
    )
    .await;
    let stale = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        checkpoint,
    )
    .await);
    assert!(
        stale["data"]["research"].is_null(),
        "a changed canonical source withholds the whole saved research view"
    );
    assert!(!stale.to_string().contains(notes));
    let (_, refreshed) = discover_subject(&f, &compact, vec![]).await;
    let revoked_notes = "REVOKED_LEGACY_RESEARCH_NOTES: the current evidence was checked.";
    let checkpoint = progress_body(
        &refreshed,
        vec![reviewed(&corrected), reviewed(&support)],
        "researching",
        revoked_notes,
    );
    let checked = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        checkpoint.clone(),
    )
    .await)["data"]
        .clone();
    let mut legacy = current(&f, &research_path).await.unwrap().2;
    legacy["dreamer_research"]["reviewed_sources"][0]["excerpt"] =
        json!("The corrected canonical observation.");
    legacy["dreamer_research"]["reviewed_sources"][1]["excerpt"] =
        json!("A synthetic source observation.");
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$3 FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(&research_path).bind(legacy).execute(&f.pool).await.unwrap();
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
    let withheld = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        checkpoint,
    )
    .await);
    assert_eq!(
        withheld["data"]["research"]["version"],
        checked["research"]["version"]
    );
    assert_eq!(withheld["data"]["research"]["notes"], "");
    assert_eq!(withheld["data"]["research"]["reviewed_sources"], json!([]));
    assert!(!withheld.to_string().contains(revoked_notes));
    let read = ok(post(
        &f,
        &f.model,
        "/v1/workspace/read",
        json!({"requests":[{"path":research_path,"view":"full"}]}),
    )
    .await);
    assert!(!read.to_string().contains(revoked_notes));
}

async fn imported_link_source(f: &Fixture, path: &str) -> Value {
    write(
        f,
        path,
        "# Evidence\n\nA synthetic source observation.\n",
        0,
    )
    .await
}

#[tokio::test]
async fn imported_link_targets_preserve_directories_exact_precedence_receipts_and_progress() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let outline = imported_link_source(&f, "sources/Projects/Orchid/Outline.md").await;
    let decision = imported_link_source(&f, "sources/Projects/Orchid/Decision.markdown").await;
    let exact = imported_link_source(&f, "Projects/Exact.md").await;
    let shadow = imported_link_source(&f, "sources/Projects/Exact.md").await;
    let collision = imported_link_source(&f, "Projects/Collision.md").await;
    let imported_collision = imported_link_source(&f, "sources/Projects/Collision.markdown").await;
    let wrong_folder = imported_link_source(&f, "sources/Elsewhere/Only.md").await;
    let selected = next_subject(&f, &admit(&f).await).await;
    let notes = "The canonical source has been checked; the imported project links remain leads.";
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(&selected, vec![reviewed(&canonical)], "researching", notes),
    )
    .await)["data"]
        .clone();
    let unresolved = [
        "Projects/Collision",
        "Projects/Wrong/Only",
        "Only",
        "/sources/Projects/Orchid/Outline.md",
        "https://example.org/Projects/Orchid/Outline.md",
        "C:\\Projects\\Orchid\\Outline.md",
        "Projects/Orchid/Outline#Details",
    ];
    let mut targets = [
        "Projects/Orchid/Outline",
        "Projects/Orchid/Outline.md",
        "Projects/Orchid/Outline.markdown",
        "sources/Projects/Orchid/Outline",
        "sources/Projects/Orchid/Outline.markdown",
        "Projects/Orchid/Decision",
        "Projects/Orchid/Decision.md",
        "Projects/Orchid/Decision.markdown",
        "Projects/Exact.md",
        "Projects/Orchid/Outline",
    ]
    .map(|t| json!(t))
    .to_vec();
    targets.extend(unresolved.iter().map(|t| json!(t)));
    let (original, expanded) = discover_subject(&f, &saved, targets.clone()).await;
    assert_eq!(
        expanded["research"]["coverage"]["unresolved_targets"],
        json!(unresolved)
    );
    let sources = expanded["research"]["sources"].as_array().unwrap();
    assert_eq!(
        sources.len(),
        4,
        "different imported spellings admit each identity only once"
    );
    for source in [&canonical, &outline, &decision, &exact] {
        assert!(
            sources
                .iter()
                .any(|s| s["entry_ref"] == source["entry_ref"] && s["path"] == source["path"])
        );
    }
    for source in [&shadow, &collision, &imported_collision, &wrong_folder] {
        assert!(
            !sources
                .iter()
                .any(|s| s["entry_ref"] == source["entry_ref"])
        );
    }
    assert_eq!(expanded["research"]["notes"], notes);
    assert_eq!(
        expanded["research"]["reviewed_sources"],
        saved["research"]["reviewed_sources"]
    );
    assert_eq!(expanded["inputs"], saved["inputs"]);
    let mut repeated = research_request(&expanded);
    repeated["queries"] = json!([]);
    targets.reverse();
    repeated["targets"] = json!(targets);
    let repeated = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        repeated,
    )
    .await);
    assert_eq!(
        repeated["no_op"], true,
        "variant order and duplicate spellings cannot manufacture discovery progress"
    );
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        original,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_same_source_headers(
        &replay["data"]["research"]["sources"],
        &expanded["research"]["sources"],
    );
    assert_eq!(replay["data"]["research"]["notes"], notes);
    assert_eq!(
        replay["data"]["research"]["version"],
        repeated["data"]["research"]["version"]
    );
    let checked = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &replay["data"],
            vec![reviewed(&canonical), reviewed(&outline)],
            "researching",
            "The canonical and imported outline have both been checked.",
        ),
    )
    .await)["data"]
        .clone();
    let corrected = write(
        &f,
        "sources/Projects/Orchid/Outline.md",
        "# Evidence\n\nA corrected source observation.\n",
        1,
    )
    .await;
    let (_, refreshed) =
        discover_subject(&f, &checked, vec![json!("Projects/Orchid/Outline")]).await;
    assert_eq!(refreshed["research"]["notes"], "");
    assert_eq!(refreshed["research"]["reviewed_sources"], json!([]));
    assert_eq!(
        refreshed["research"]["coverage"]["unresolved_targets"],
        json!([])
    );
    assert!(
        refreshed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == corrected["entry_ref"] && s["version"] == 2)
    );
}

#[tokio::test]
async fn imported_link_aliases_cannot_bypass_source_exclusions_or_exact_identity() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let foreign_actor = actor(&f.pool, None, OWNER_CAPS).await;
    let foreign = ok(post(&f, &foreign_actor, "/v1/workspace/write", json!({"path":"sources/Imported/Foreign.md","content":"# Foreign\n\nAnother user's source.\n","expected_version":0,"metadata":{}})).await)["data"].clone();
    let mut excluded = Vec::new();
    for name in ["Deleted", "Generated", "Large"] {
        let exact = imported_link_source(&f, &format!("Imported/{name}.md")).await;
        imported_link_source(&f, &format!("sources/Imported/{name}.md")).await;
        let fallback = imported_link_source(&f, &format!("sources/Imported/{name}Only.md")).await;
        for source in [exact, fallback] {
            let id = Uuid::parse_str(
                source["entry_ref"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("entry:"),
            )
            .unwrap();
            match name {
                "Deleted" => {
                    sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE user_id=$1 AND id=$2").bind(f.owner.user).bind(id).execute(&f.pool).await.unwrap();
                }
                "Generated" => {
                    sqlx::query("UPDATE brunn.entry_versions SET metadata=jsonb_build_object('dreamer_run',jsonb_build_object('schema','dream.run.v1')) WHERE user_id=$1 AND entry_id=$2 AND version=1").bind(f.owner.user).bind(id).execute(&f.pool).await.unwrap();
                }
                "Large" => {
                    let content = "Synthetic large evidence.\n".repeat(50000);
                    let hash = hex::encode(Sha256::digest(content.as_bytes()));
                    sqlx::query("UPDATE brunn.entry_versions SET content=$3,size_bytes=$4,content_sha256=$5 WHERE user_id=$1 AND entry_id=$2 AND version=1").bind(f.owner.user).bind(id).bind(&content).bind(content.len() as i64).bind(hash).execute(&f.pool).await.unwrap();
                }
                _ => unreachable!(),
            }
            excluded.push(source);
        }
    }
    let selected = next_subject(&f, &admit(&f).await).await;
    let mut targets = [
        "Imported/Deleted.md",
        "Imported/Generated.md",
        "Imported/Large.md",
        "Imported/DeletedOnly",
        "Imported/GeneratedOnly",
        "Imported/LargeOnly",
        "Imported/Foreign",
    ]
    .map(|t| json!(t))
    .to_vec();
    targets.push(foreign["entry_ref"].clone());
    targets.extend(excluded.iter().map(|s| s["entry_ref"].clone()));
    let (_, expanded) = discover_subject(&f, &selected, targets.clone()).await;
    assert_eq!(
        expanded["research"]["coverage"]["unresolved_targets"],
        json!(targets)
    );
    assert_eq!(expanded["research"]["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        expanded["research"]["sources"][0]["entry_ref"],
        canonical["entry_ref"]
    );
    assert_eq!(expanded["inputs"], selected["inputs"]);
}

#[tokio::test]
async fn imported_link_target_stays_unresolved_when_the_unique_source_exceeds_the_job_cap() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let content = "# Evidence\n\nA synthetic source observation.\n";
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO brunn.entries(user_id,path,title,kind,media_type,current_version) SELECT $1,'sources/Bulk/'||lpad(n::text,6,'0')||'.md','Evidence','markdown','text/markdown',1 FROM generate_series(1,256) n")
        .bind(f.owner.user).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions(user_id,entry_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) SELECT user_id,id,1,$2,$3,$4,'{}'::jsonb,$5 FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Bulk/')")
        .bind(f.owner.user).bind(&hash).bind(content).bind(content.len() as i64).bind(f.owner.id).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT user_id,id,1,'create',path,$2 FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Bulk/') ORDER BY path")
        .bind(f.owner.user).bind(hash).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut selected = next_subject(&f, &admit(&f).await).await;
    let targets = (1..=255)
        .map(|n| json!(format!("Bulk/{n:06}")))
        .collect::<Vec<_>>();
    for page in targets.chunks(32) {
        (_, selected) = discover_subject(&f, &selected, page.to_vec()).await;
        assert_eq!(
            selected["research"]["coverage"]["unresolved_targets"],
            json!([])
        );
    }
    assert_eq!(
        selected["research"]["sources"].as_array().unwrap().len(),
        256
    );
    let (_, capped) = discover_subject(
        &f,
        &selected,
        vec![json!("Bulk/000256"), json!("People/Orchid")],
    )
    .await;
    assert_eq!(capped["research"]["coverage"]["source_cap_reached"], true);
    assert_eq!(
        capped["research"]["coverage"]["unresolved_targets"],
        json!(["Bulk/000256"])
    );
    assert_same_source_headers(
        &capped["research"]["sources"],
        &selected["research"]["sources"],
    );
    assert_eq!(
        capped["research"]["sources"][0]["entry_ref"],
        canonical["entry_ref"]
    );
}

#[tokio::test]
async fn checkpoint_source_and_scope_changes_return_typed_recovery_errors_without_partial_writes() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let supporting = imported_link_source(&f, "sources/Notes/Support.md").await;
    let selected = next_subject(&f, &admit(&f).await).await;
    let (_, admitted) =
        discover_subject(&f, &selected, vec![supporting["entry_ref"].clone()]).await;
    let mut saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &admitted,
            vec![reviewed(&canonical), reviewed(&supporting)],
            "researching",
            "The original exact sources have been checked.",
        ),
    )
    .await)["data"]
        .clone();
    let research_path = format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    for (cited_change, status, code) in [
        (true, StatusCode::CONFLICT, "dreamer_source_changed"),
        (false, StatusCode::BAD_REQUEST, "research_refresh_required"),
    ] {
        let changed = if cited_change {
            write(
                &f,
                "sources/Notes/Support.md",
                "# Evidence\n\nA corrected supporting observation.\n",
                1,
            )
            .await
        } else {
            write(
                &f,
                "UnrelatedDirectory/Outcome.md",
                "# Outcome\n\nOrchid has a newly relevant outcome.\n",
                0,
            )
            .await
        };
        let persisted = current(&f, &research_path).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let selectors = saved["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .clone();
        let rejected = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress_body(
                &saved,
                selectors,
                "researching",
                "The rejected checkpoint must not replace the prior notes.",
            ),
        )
        .await;
        assert_eq!(rejected.status, status, "{}", rejected.body);
        assert_eq!(rejected.body["error"]["code"], code, "{}", rejected.body);
        assert_eq!(
            current(&f, &research_path).await.unwrap(),
            persisted,
            "rejection must preserve notes, job version and operation receipts"
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let (_, refreshed) = discover_subject(&f, &saved, vec![]).await;
        assert_eq!(refreshed["research"]["notes"], "");
        assert_eq!(refreshed["research"]["reviewed_sources"], json!([]));
        assert!(
            refreshed["research"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["entry_ref"] == changed["entry_ref"]
                    && s["version"] == changed["version"])
        );
        let selectors = refreshed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(reviewed)
            .collect();
        saved = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress_body(
                &refreshed,
                selectors,
                "researching",
                "Current exact source versions have been checked after reconciliation.",
            ),
        )
        .await)["data"]
            .clone();
        assert_eq!(
            saved["research"]["notes"],
            "Current exact source versions have been checked after reconciliation."
        );
    }
}

#[tokio::test]
async fn checkpoint_eof_normalization_preserves_all_progress_and_replay_but_candidates_stay_strict()
{
    for (newline, trailing) in [("\n", true), ("\r\n", true), ("\n", false)] {
        let Some(f) = fixture().await else { return };
        control(&f, "report-only", 0).await;
        write(
            &f,
            "sources/People/Orchid.md",
            "# Orchid\n\nEarlier source.\n",
            0,
        )
        .await;
        let mut content = (1..=25)
            .map(|line| match line {
                1 => "# Orchid".to_owned(),
                2 => String::new(),
                _ => format!("Synthetic canonical observation {line}."),
            })
            .collect::<Vec<_>>()
            .join(newline);
        if trailing {
            content.push_str(newline);
        }
        assert_eq!(content.lines().count(), 25);
        let canonical = write(&f, "sources/People/Orchid.md", &content, 1).await;
        let supporting = write(
            &f,
            "sources/Notes/Support.md",
            "# Support\n\nA supporting source observation.\n",
            0,
        )
        .await;
        let selected = next_subject(&f, &admit(&f).await).await;
        let (_, admitted) =
            discover_subject(&f, &selected, vec![supporting["entry_ref"].clone()]).await;
        let selectors = vec![
            json!({"entry_ref":canonical["entry_ref"],"version":2,"start_line":1,"end_line":26,"path":"untrusted-path","excerpt":"untrusted-excerpt"}),
            json!({"entry_ref":supporting["entry_ref"],"version":1,"start_line":1,"end_line":3}),
        ];
        let notes = format!(
            "Checked observations remain provisional.\n\n{}",
            "Both exact sources have been read; the remaining lead needs further investigation.\n"
                .repeat(80)
        );
        let original = progress_body(&admitted, selectors.clone(), "researching", &notes);
        let saved = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            original.clone(),
        )
        .await)["data"]
            .clone();
        assert_eq!(saved["research"]["notes"], notes);
        let normalized = &saved["research"]["reviewed_sources"];
        assert_eq!(normalized.as_array().unwrap().len(), selectors.len());
        assert_eq!(normalized[0]["entry_ref"], canonical["entry_ref"]);
        assert_eq!(normalized[0]["version"], 2);
        assert_eq!(normalized[0]["start_line"], 1);
        assert_eq!(normalized[0]["end_line"], 25);
        assert_eq!(normalized[0]["path"], canonical["path"]);
        assert!(normalized[0].get("excerpt").is_none());
        assert_eq!(normalized[1]["entry_ref"], supporting["entry_ref"]);
        assert_eq!(normalized[1]["start_line"], 1);
        assert_eq!(normalized[1]["end_line"], 3);
        assert_eq!(normalized[1]["path"], supporting["path"]);
        assert!(normalized[1].get("excerpt").is_none());
        assert_eq!(
            saved["inputs"], admitted["inputs"],
            "a checkpoint does not consume input"
        );
        let research_path = format!(
            "dreams/research/{}.md",
            canonical["entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:")
        );
        let persisted = current(&f, &research_path).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        assert_eq!(persisted.2["dreamer_research"]["notes"], notes);
        assert_eq!(
            persisted.2["dreamer_research"]["reviewed_sources"],
            *normalized
        );

        let replay = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            original.clone(),
        )
        .await);
        assert_eq!(replay["no_op"], true);
        assert_eq!(replay["data"]["research"]["notes"], notes);
        assert_eq!(replay["data"]["research"]["reviewed_sources"], *normalized);
        assert_eq!(current(&f, &research_path).await.unwrap(), persisted);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let mut different = original;
        different["reviewed_sources"][0]["end_line"] = json!(25);
        let changed_payload = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            different,
        )
        .await;
        assert_eq!(changed_payload.status, StatusCode::BAD_REQUEST);
        assert!(
            changed_payload
                .body
                .to_string()
                .contains("operation_id was already accepted with a different payload")
        );

        let mut candidate = subject_candidate(&saved, selectors);
        candidate["title"] = json!("Orchid overview");
        candidate["content"] = json!(
            "# Orchid\n\nThe canonical source records observations.[^s1]\nA supporting source records another observation.[^s2]\n"
        );
        let mut request = research_request(&saved);
        request["candidates"] = json!([candidate]);
        request["processed_inputs"] = json!([]);
        let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", request).await;
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
        assert!(
            rejected
                .body
                .to_string()
                .contains("source selector is outside its exact source version"),
            "{}",
            rejected.body
        );
        assert_eq!(current(&f, &research_path).await.unwrap(), persisted);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        assert!(
            current(&f, saved["research"]["output_path"].as_str().unwrap())
                .await
                .is_none()
        );
    }
}

#[tokio::test]
async fn checkpoint_eof_normalization_rejects_invalid_ranges_and_sources_atomically() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nA canonical source observation.\n",
        0,
    )
    .await;
    let support = write(
        &f,
        "sources/Notes/Support.md",
        "# Support\n\nA supporting source observation.\n",
        0,
    )
    .await;
    let empty = write(&f, "sources/Notes/Empty.md", "", 0).await;
    let deleted = write(
        &f,
        "sources/Notes/Deleted.md",
        "# Deleted\n\nAn initially accessible source.\n",
        0,
    )
    .await;
    let generated = write(
        &f,
        "sources/Notes/Generated.md",
        "# Generated\n\nAn initially ordinary source.\n",
        0,
    )
    .await;
    let foreign_actor = actor(&f.pool, None, OWNER_CAPS).await;
    let foreign = ok(post(&f, &foreign_actor, "/v1/workspace/write", json!({"path":"sources/Foreign.md","content":"# Foreign\n\nAnother user's source.\n","expected_version":0,"metadata":{}})).await)["data"].clone();
    let selected = next_subject(&f, &admit(&f).await).await;
    let (_, admitted) = discover_subject(
        &f,
        &selected,
        [&support, &empty, &deleted, &generated]
            .map(|s| s["entry_ref"].clone())
            .to_vec(),
    )
    .await;
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &admitted,
            vec![reviewed(&canonical), reviewed(&support)],
            "researching",
            "Previously checked notes must survive rejected updates.",
        ),
    )
    .await)["data"]
        .clone();
    let research_path = format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let persisted = current(&f, &research_path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let first = json!({"entry_ref":canonical["entry_ref"],"version":1,"start_line":1,"end_line":4});
    let invalid = [
        (
            json!({"entry_ref":support["entry_ref"],"version":1,"start_line":0,"end_line":3}),
            "positive and ordered",
        ),
        (
            json!({"entry_ref":support["entry_ref"],"version":1,"start_line":2,"end_line":1}),
            "positive and ordered",
        ),
        (
            json!({"entry_ref":support["entry_ref"],"version":1,"start_line":4,"end_line":5}),
            "outside its exact source version",
        ),
        (
            json!({"entry_ref":support["entry_ref"],"version":1,"start_line":1,"end_line":402}),
            "outside its exact source version",
        ),
        (
            json!({"entry_ref":empty["entry_ref"],"version":1,"start_line":1,"end_line":1}),
            "outside its exact source version",
        ),
        (
            json!({"entry_ref":foreign["entry_ref"],"version":1,"start_line":1,"end_line":4}),
            "admitted research evidence",
        ),
    ];
    for (selector, error) in invalid {
        let response = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress_body(
                &saved,
                vec![first.clone(), selector],
                "researching",
                "This rejected checkpoint must not replace the saved notes.",
            ),
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert!(
            response.body.to_string().contains(error),
            "{}",
            response.body
        );
        assert_eq!(
            current(&f, &research_path).await.unwrap(),
            persisted,
            "an earlier valid selector cannot partially persist"
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    }
    write(
        &f,
        "sources/Notes/Support.md",
        "# Support\n\nA corrected supporting observation.\n",
        1,
    )
    .await;
    sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(
            Uuid::parse_str(
                deleted["entry_ref"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("entry:"),
            )
            .unwrap(),
        )
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE brunn.entries SET path='derived/Generated.md' WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(
            Uuid::parse_str(
                generated["entry_ref"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("entry:"),
            )
            .unwrap(),
        )
        .execute(&f.pool)
        .await
        .unwrap();
    for (source, status, error) in [
        (&support, StatusCode::CONFLICT, "candidate evidence changed"),
        (&deleted, StatusCode::BAD_REQUEST, "missing or inaccessible"),
        (
            &generated,
            StatusCode::BAD_REQUEST,
            "generated Dreamer output cannot be its own source evidence",
        ),
    ] {
        let selector =
            json!({"entry_ref":source["entry_ref"],"version":1,"start_line":1,"end_line":4});
        let response = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress_body(
                &saved,
                vec![first.clone(), selector],
                "researching",
                "This invalid evidence must not replace the saved notes.",
            ),
        )
        .await;
        assert_eq!(response.status, status);
        assert!(
            response.body.to_string().contains(error),
            "{}",
            response.body
        );
        assert_eq!(current(&f, &research_path).await.unwrap(), persisted);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    }
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
    assert_same_source_headers(
        &replay["data"]["research"]["sources"],
        &third["research"]["sources"],
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
    let invalid = ok(post(
        &f,
        &f.owner,
        "/v1/workspace/write",
        json!({"path":"sources/People/First.md","content":"# First\n\nAn ordinary source with an overlong explicit title.\n","expected_version":0,"metadata":{"title":"x".repeat(161)}}),
    )
    .await)["data"].clone();
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

#[tokio::test]
async fn additive_discovery_preserves_checked_progress_but_source_or_scope_changes_invalidate_it() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/People/Orchid.md",
        "# Orchid\n\nOrchid has an established source fact.\n",
        0,
    )
    .await;
    let supporting = write(
        &f,
        "sources/Notes/Supporting.md",
        "# Supporting\n\nA supporting observation refers to [[sources/Notes/Primary.md]].\n",
        0,
    )
    .await;
    let primary = write(
        &f,
        "sources/Notes/Primary.md",
        "# Primary\n\nAn independently retained primary observation.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let note =
        "The canonical source records an established fact; supporting sources still need review.";
    let progress = progress_body(&a, vec![reviewed(&canonical)], "researching", note);
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    let (first_discovery, expanded) =
        discover_subject(&f, &saved, vec![supporting["entry_ref"].clone()]).await;
    assert_eq!(
        expanded["research"]["notes"], note,
        "adding evidence must not erase already checked work"
    );
    assert_eq!(
        expanded["research"]["reviewed_sources"],
        saved["research"]["reviewed_sources"]
    );
    assert_eq!(expanded["research"]["sources"].as_array().unwrap().len(), 2);
    assert!(
        !expanded["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == supporting["entry_ref"]),
        "new source headers remain unreviewed"
    );
    assert_eq!(
        expanded["inputs"], saved["inputs"],
        "discovery does not acknowledge historical input"
    );

    let reconciled_note = "The canonical and supporting observations have been reviewed; the linked primary remains an unfinished lead.";
    let progress = progress_body(
        &expanded,
        vec![reviewed(&canonical), reviewed(&supporting)],
        "researching",
        reconciled_note,
    );
    let checkpoint = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    let (_, expanded) = discover_subject(&f, &checkpoint, vec![primary["entry_ref"].clone()]).await;
    assert_eq!(expanded["research"]["notes"], reconciled_note);
    assert_eq!(
        expanded["research"]["reviewed_sources"],
        checkpoint["research"]["reviewed_sources"]
    );
    assert_eq!(expanded["research"]["sources"].as_array().unwrap().len(), 3);
    assert!(
        !expanded["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == primary["entry_ref"])
    );
    let research_path = format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    );
    let stored = current(&f, &research_path).await.unwrap().2;
    assert_eq!(
        stored["dreamer_research"]["notes"], reconciled_note,
        "checked work must survive a process restart in the durable job"
    );
    assert_eq!(
        stored["dreamer_research"]["reviewed_sources"],
        checkpoint["research"]["reviewed_sources"]
    );
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        first_discovery,
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(
        replay["data"]["research"]["notes"], reconciled_note,
        "old operation replay returns current checked progress"
    );

    let corrected = write(
        &f,
        "sources/Notes/Supporting.md",
        "# Supporting\n\nA corrected supporting observation replaces the prior one.\n",
        1,
    )
    .await;
    let (_, changed) = discover_subject(&f, &expanded, vec![]).await;
    assert_eq!(changed["research"]["notes"], "");
    assert_eq!(changed["research"]["reviewed_sources"], json!([]));
    assert!(
        changed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == corrected["entry_ref"] && source["version"] == 2)
    );

    let progress = progress_body(
        &changed,
        vec![reviewed(&canonical), reviewed(&corrected)],
        "researching",
        "The corrected source has now been checked.",
    );
    let checked = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    let relevant = write(
        &f,
        "UnrelatedDirectory/Current.md",
        "# Current\n\nOrchid has a newly relevant outcome.\n",
        0,
    )
    .await;
    let (_, refreshed) = discover_subject(&f, &checked, vec![]).await;
    assert_eq!(
        refreshed["research"]["notes"], "",
        "newly relevant corpus changes still invalidate old conclusions"
    );
    assert_eq!(refreshed["research"]["reviewed_sources"], json!([]));
    assert!(
        refreshed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == relevant["entry_ref"])
    );

    let progress = progress_body(
        &refreshed,
        vec![
            reviewed(&canonical),
            reviewed(&corrected),
            reviewed(&relevant),
        ],
        "researching",
        "The newly relevant outcome has been reconciled with the checked sources.",
    );
    let checked = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    sqlx::query(
        "UPDATE brunn.entries SET path='.brunn/tasks/'||id::text||'.md' WHERE user_id=$1 AND id=$2",
    )
    .bind(f.owner.user)
    .bind(
        Uuid::parse_str(
            primary["entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:"),
        )
        .unwrap(),
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, withheld) = discover_subject(&f, &checked, vec![]).await;
    assert_eq!(
        withheld["research"]["notes"], "",
        "loss of access to prior research evidence still clears cached conclusions"
    );
    assert_eq!(withheld["research"]["reviewed_sources"], json!([]));
    assert_eq!(
        current(&f, &research_path).await.unwrap().2["dreamer_research"]["notes"],
        ""
    );
}

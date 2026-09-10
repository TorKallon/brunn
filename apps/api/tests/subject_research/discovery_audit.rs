//! Current-only, server-owned query provenance; never historical evidence.
use super::*;

const QUERY: &str = "detector schedule";
const MISS: &str = "nonexistentquasarfixture";
const DISCOVER: &str = "/v1/workspace/dreamer/narrative-discover";

struct AuditFixture {
    canonical: Value,
    primary: Value,
    selected: Value,
}

fn job_path(canonical: &Value) -> String {
    format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    )
}

fn audit(admission: &Value) -> &Value {
    &admission["research"]["discovery_audit"]
}

async fn setup(f: &Fixture) -> AuditFixture {
    control(f, "report-only", 0).await;
    let canonical = write(
        f,
        "sources/People/Aster.md",
        "# Aster\n\nAster coordinates an optical survey.\n",
        0,
    )
    .await;
    let primary = write(
        f,
        "sources/Optics/Measurement.md",
        "# Measurement\n\nThe detector schedule records an exposure interval.\n",
        0,
    )
    .await;
    let selected = next_subject(f, &admit(f).await).await;
    assert_eq!(selected["research"]["subject_ref"], canonical["entry_ref"]);
    assert_eq!(
        audit(&selected),
        &json!({"validity":"legacy_or_unknown","last_search":null})
    );
    AuditFixture {
        canonical,
        primary,
        selected,
    }
}

fn query_body(admission: &Value, queries: Vec<&str>) -> Value {
    let mut body = research_request(admission);
    body["queries"] = json!(queries);
    body["targets"] = json!([]);
    body
}

async fn search(f: &Fixture, admission: &Value, queries: Vec<&str>) -> (Value, Value) {
    let body = query_body(admission, queries);
    let response = ok(post(f, &f.runner, DISCOVER, body.clone()).await);
    (body, response)
}

fn current_audit(admission: &Value) -> &Value {
    assert_eq!(audit(admission)["validity"], "current", "{admission}");
    let search = &audit(admission)["last_search"];
    assert_eq!(search["schema"], "dream.research.discovery.v1");
    assert_eq!(search["retrieval_policy"], 3);
    assert!(search["searched_generation"].as_i64().unwrap() > 0);
    assert!(search["queries"].as_array().unwrap().len() <= 6);
    assert_eq!(
        search["groups"].as_array().unwrap().len(),
        search["queries"].as_array().unwrap().len() * 2
    );
    for (n, group) in search["groups"].as_array().unwrap().iter().enumerate() {
        assert_eq!(group["query_index"], n / 2);
        assert_eq!(
            group["sort"],
            if n % 2 == 0 {
                "best_match"
            } else {
                "last_modified"
            }
        );
        assert_eq!(group["limit"], 8);
        assert_eq!(group["execution_status"], "bounded");
        assert_eq!(
            group["output_limit_reached"],
            group["returned"].as_u64().unwrap() == 8
        );
        assert!(group.get("candidates").is_none());
    }
    assert!(
        admission["research"]["coverage"]
            .get("discovery_audit")
            .is_none(),
        "internal scope and history authority never enter the model view"
    );
    assert!(serde_json::to_vec(search).unwrap().len() <= 32 * 1024);
    search
}

async fn replace_current_metadata(f: &Fixture, canonical: &Value, metadata: Value) {
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$3 FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(job_path(canonical)).bind(metadata).execute(&f.pool).await.unwrap();
}

#[tokio::test]
async fn discovery_audit_static_ineligible_hit_does_not_invalidate_successful_search() {
    let Some(f) = fixture().await else { return };
    // The exclusion is present before selection and searching. No source race,
    // unavailable historical checkpoint, or scope reconciliation is involved.
    let excluded = ok(post(
        &f,
        &f.owner,
        "/v1/workspace/write",
        json!({"path":"sources/Optics/Evaluation.md","expected_version":0,
            "content":"# Evaluation\n\nThe detector schedule is synthetic evaluation output.\n",
            "metadata":{"evaluation_output":true}}),
    )
    .await)["data"]
        .clone();
    let s = setup(&f).await;
    let excluded_id = Uuid::parse_str(
        excluded["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap();
    let (indexed, matches): (i64, Option<bool>) = sqlx::query_as(
        "SELECT count(*),bool_or(search_vector @@ websearch_to_tsquery('english',$3)) FROM brunn.search_chunks WHERE user_id=$1 AND entry_id=$2",
    )
    .bind(f.owner.user)
    .bind(excluded_id)
    .bind(QUERY)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(indexed > 0);
    assert_eq!(
        matches,
        Some(true),
        "the excluded record is already indexed"
    );
    let excluded_before = current(&f, excluded["path"].as_str().unwrap())
        .await
        .unwrap();
    let primary_before = current(&f, s.primary["path"].as_str().unwrap())
        .await
        .unwrap();
    let (original, first) = search(&f, &s.selected, vec![QUERY]).await;
    assert_eq!(first["no_op"], false);
    for group in first["data"]["research"]["coverage"]["query_results"]
        .as_array()
        .unwrap()
    {
        assert_eq!(group["returned"], 1, "only the eligible primary is emitted");
    }
    let (_, repeated) = search(&f, &first["data"], vec![QUERY]).await;
    for response in [&first, &repeated] {
        let research = &response["data"]["research"];
        assert_eq!(research["coverage"]["change_status"], "complete");
        assert_eq!(research["checkpoint_context_status"], "available");
        assert!(
            research["checkpoint_contexts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let sources = research["sources"].as_array().unwrap();
        assert!(
            sources
                .iter()
                .any(|source| source["entry_ref"] == s.primary["entry_ref"])
        );
        assert!(
            sources
                .iter()
                .all(|source| source["entry_ref"] != excluded["entry_ref"])
        );
        assert_eq!(response["data"]["inputs"], s.selected["inputs"]);
    }
    assert_eq!(
        current(&f, excluded["path"].as_str().unwrap())
            .await
            .unwrap(),
        excluded_before
    );
    assert_eq!(
        current(&f, s.primary["path"].as_str().unwrap())
            .await
            .unwrap(),
        primary_before
    );

    // Accepted operation replay keeps the later exact state; it must not be
    // mistaken for a fresh execution that can repair an invalidated audit.
    let notebook = current(&f, &job_path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, DISCOVER, original).await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(audit(&replay["data"]), audit(&repeated["data"]));
    assert_eq!(
        current(&f, &job_path(&s.canonical)).await.unwrap(),
        notebook
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    eprintln!(
        "static eligibility audit: first={}, repeated={}, first_groups={}, repeated_no_op={}",
        audit(&first["data"]),
        audit(&repeated["data"]),
        first["data"]["research"]["coverage"]["query_results"],
        repeated["no_op"]
    );
    assert_eq!(
        json!([
            audit(&first["data"])["validity"],
            audit(&repeated["data"])["validity"]
        ]),
        json!(["current", "current"]),
        "a static policy-excluded search hit is not a source change during discovery"
    );
    assert_eq!(
        repeated["no_op"], true,
        "the current same-policy batch is reusable"
    );
}

#[tokio::test]
async fn discovery_audit_maps_actual_normalized_queries_and_survives_nonsearch_operations() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let (original, response) =
        search(&f, &s.selected, vec![MISS, " DETECTOR\tSCHEDULE ", QUERY]).await;
    assert_eq!(response["no_op"], false);
    let first = response["data"].clone();
    let receipt = current_audit(&first).clone();
    assert_eq!(receipt["queries"], json!([QUERY, MISS]));
    assert!(receipt["groups"][0]["returned"].as_u64().unwrap() > 0);
    assert_eq!(receipt["groups"][2]["returned"], 0);
    assert!(
        first["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input["entry_ref"] == s.primary["entry_ref"])
    );
    assert_eq!(
        first["inputs"], s.selected["inputs"],
        "searching headers never consumes inputs"
    );
    let (_, response) = search(&f, &first, vec![QUERY, " NONEXISTENTQUASARFIXTURE "]).await;
    assert_eq!(response["no_op"], true);
    let repeated = response["data"].clone();
    assert_eq!(current_audit(&repeated), &receipt);
    assert_eq!(repeated["research"]["round"], first["research"]["round"]);
    let (_, target_only) =
        discover_subject(&f, &repeated, vec![s.primary["entry_ref"].clone()]).await;
    assert_eq!(current_audit(&target_only), &receipt);
    let (_, empty) = discover_subject(&f, &target_only, vec![]).await;
    assert_eq!(current_audit(&empty), &receipt);
    let progress = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &empty,
            vec![reviewed(&s.canonical)],
            "researching",
            "The canonical observation is checked; a detector detail remains open.",
        ),
    )
    .await)["data"]
        .clone();
    assert_eq!(current_audit(&progress), &receipt);
    let before = current(&f, &job_path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, DISCOVER, original).await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(current_audit(&replay["data"]), &receipt);
    assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn discovery_audit_legacy_hash_and_replaced_latest_batch_never_suppress_real_search() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let (original, response) = search(&f, &s.selected, vec![QUERY]).await;
    let first = response["data"].clone();
    for variant in ["missing", "old_policy", "malformed"] {
        let mut metadata = current(&f, &job_path(&s.canonical)).await.unwrap().2;
        let stored = &mut metadata["dreamer_research"]["coverage"];
        match variant {
            "missing" => {
                stored.as_object_mut().unwrap().remove("discovery_audit");
            }
            "old_policy" => stored["discovery_audit"]["search"]["retrieval_policy"] = json!(2),
            "malformed" => {
                stored["discovery_audit"]["search"]["groups"][0]["query_index"] = json!(99)
            }
            _ => unreachable!(),
        }
        replace_current_metadata(&f, &s.canonical, metadata).await;
        let before = current(&f, &job_path(&s.canonical)).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let replay = ok(post(&f, &f.runner, DISCOVER, original.clone()).await);
        assert_eq!(replay["no_op"], true);
        assert_eq!(
            audit(&replay["data"])["validity"],
            if variant == "old_policy" {
                "outdated"
            } else {
                "legacy_or_unknown"
            }
        );
        assert!(audit(&replay["data"])["last_search"].is_null());
        assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), before);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let (_, response) = search(&f, &replay["data"], vec![QUERY]).await;
        assert_eq!(
            response["no_op"], false,
            "{variant}: an old hash cannot prove audited search"
        );
        current_audit(&response["data"]);
    }
    let replay = ok(post(&f, &f.runner, DISCOVER, original.clone()).await);
    let (_, b) = search(&f, &replay["data"], vec![MISS]).await;
    assert_eq!(b["no_op"], false);
    assert_eq!(current_audit(&b["data"])["queries"], json!([MISS]));
    let (_, a) = search(&f, &b["data"], vec![QUERY]).await;
    assert_eq!(
        a["no_op"], false,
        "A→B→A must run A after its sole audit was replaced"
    );
    assert_eq!(current_audit(&a["data"])["queries"], json!([QUERY]));
    assert!(
        a["data"]["research"]["round"].as_u64().unwrap()
            > first["research"]["round"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn discovery_audit_relevant_scope_invalidates_but_unrelated_and_generated_churn_preserve() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let (original, response) = search(&f, &s.selected, vec![QUERY]).await;
    let first = response["data"].clone();
    let receipt = current_audit(&first).clone();
    write(
        &f,
        "sources/Unrelated/Weather.md",
        "# Weather\n\nA distant observatory recorded rainfall.\n",
        0,
    )
    .await;
    ok(post(&f, &f.owner, "/v1/workspace/write", json!({"path":"Briefings/Generated.md","expected_version":0,"content":"# Briefing\n\nAster detector schedule appears in generated commentary.\n","metadata":{"kind":"briefing_edition"}})).await);
    let replay = ok(post(&f, &f.runner, DISCOVER, original.clone()).await);
    assert_eq!(current_audit(&replay["data"]), &receipt);
    let (_, unchanged) = discover_subject(&f, &first, vec![]).await;
    assert_eq!(current_audit(&unchanged), &receipt);
    let relevant = write(
        &f,
        "sources/Optics/Later.md",
        "# Later\n\nAster has a later detector outcome to review.\n",
        0,
    )
    .await;
    let replay = ok(post(&f, &f.runner, DISCOVER, original.clone()).await);
    assert_eq!(
        audit(&replay["data"]),
        &json!({"validity":"outdated","last_search":null})
    );
    let (_, refreshed) = discover_subject(&f, &unchanged, vec![]).await;
    assert_eq!(audit(&refreshed)["validity"], "outdated");
    assert!(
        refreshed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == relevant["entry_ref"])
    );
    let (_, searched) = search(&f, &refreshed, vec![QUERY]).await;
    assert_eq!(searched["no_op"], false);
    let audited = current_audit(&searched["data"]);
    assert!(
        audited["searched_generation"].as_i64().unwrap()
            >= relevant["workspace_generation"].as_i64().unwrap()
    );
}

#[tokio::test]
async fn discovery_audit_checkpoint_addition_preserves_search_but_access_loss_withholds_it() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let (_, result) = search(&f, &s.selected, vec![QUERY]).await;
    let mut progress = progress_body(
        &result["data"],
        vec![reviewed(&s.canonical)],
        "researching",
        "HISTORICAL_A: canonical work and an independent exposure lead remain open.",
    );
    progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
    progress["reconciled_checkpoints"] = json!([]);
    let first = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    current_audit(&first);
    let mut progress = progress_body(
        &first,
        vec![reviewed(&s.primary)],
        "researching",
        "CURRENT_B: detector timing is checked independently of the older open lead.",
    );
    progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
    progress["reconciled_checkpoints"] = json!([]);
    let second = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    assert_eq!(current_audit(&second), current_audit(&first));
    assert_eq!(
        second["research"]["checkpoint_contexts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let historical = &second["research"]["checkpoint_contexts"][0];
    assert!(historical.get("discovery_audit").is_none());
    assert!(!historical.to_string().contains(QUERY));
    let (search_operation, searched) = search(&f, &second, vec![QUERY]).await;
    current_audit(&searched["data"]);
    ok(post(&f, &f.owner, "/v1/workspace/write", json!({"path":s.primary["path"],"expected_version":1,"content":"# Measurement\n\nA generated source now occupies this identity.\n","metadata":{"kind":"briefing_edition"}})).await);
    let replay = ok(post(&f, &f.runner, DISCOVER, search_operation).await);
    assert_eq!(audit(&replay["data"])["validity"], "outdated");
    assert!(audit(&replay["data"])["last_search"].is_null());
    let (_, pruned) = discover_subject(&f, &searched["data"], vec![]).await;
    assert_eq!(
        pruned["research"]["checkpoint_context_status"],
        "unavailable"
    );
    assert_eq!(audit(&pruned)["validity"], "outdated");
    let (_, attempted) = search(&f, &pruned, vec![QUERY]).await;
    assert_eq!(
        audit(&attempted["data"])["validity"],
        "outdated",
        "a smaller active manifest cannot launder query provenance from denied historical work"
    );
    assert!(audit(&attempted["data"])["last_search"].is_null());
}

#[tokio::test]
async fn discovery_audit_reconciled_history_keeps_original_authority_until_a_new_search() {
    for exclude_by_policy in [false, true] {
        let Some(f) = fixture().await else { return };
        let s = setup(&f).await;
        let (_, searched) = search(&f, &s.selected, vec![QUERY]).await;
        let mut admission = searched["data"].clone();
        for notes in [
            "Earlier survey work retains an independent exposure lead.",
            "The canonical survey identity is checked for the current overview.",
        ] {
            let mut progress = progress_body(
                &admission,
                vec![reviewed(&s.canonical)],
                "researching",
                notes,
            );
            progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
            progress["reconciled_checkpoints"] = json!([]);
            admission = ok(post(
                &f,
                &f.runner,
                "/v1/workspace/dreamer/research-progress",
                progress,
            )
            .await)["data"]
                .clone();
        }
        assert_eq!(
            admission["research"]["checkpoint_contexts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // Isolate an uncited historical dependency from the active manifest.
        // The real historical version retains its complete original sources.
        let mut metadata = current(&f, &job_path(&s.canonical)).await.unwrap().2;
        metadata["dreamer_research"]["sources"]
            .as_array_mut()
            .unwrap()
            .retain(|source| source["entry_ref"] == s.canonical["entry_ref"]);
        replace_current_metadata(&f, &s.canonical, metadata).await;
        let (operation, searched) = search(&f, &admission, vec!["historicalinsightprobe"]).await;
        admission = searched["data"].clone();
        let original_audit = current_audit(&admission).clone();
        let mut origins = admission["research"]["checkpoint_contexts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|context| context["origin"].clone())
            .collect::<Vec<_>>();
        origins.push(admission["research"]["current_checkpoint"].clone());
        let mut progress = progress_body(
            &admission,
            vec![reviewed(&s.canonical)],
            "researching",
            "The checked canonical identity supplies the bounded survey overview.",
        );
        progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
        progress["reconciled_checkpoints"] = json!(origins);
        progress["findings"] = json!([
            "Both offered checkpoints are reconciled: retain the checked survey identity; the exposure lead is deliberately outside this bounded overview."
        ]);
        let reconciled = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress,
        )
        .await)["data"]
            .clone();
        assert!(
            reconciled["research"]["checkpoint_contexts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(current_audit(&reconciled), &original_audit);
        let (_, repeated) = search(&f, &reconciled, vec!["historicalinsightprobe"]).await;
        assert_eq!(
            repeated["no_op"], true,
            "retirement does not repeat a completed search"
        );
        assert_eq!(current_audit(&repeated["data"]), &original_audit);

        if exclude_by_policy {
            ok(post(&f, &f.owner, "/v1/workspace/write", json!({"path":s.primary["path"],"expected_version":1,"content":"# Measurement\n\nGenerated presentation now occupies this identity.\n","metadata":{"kind":"briefing_edition"}})).await);
        } else {
            ok(request_delete(&f, &s.primary).await);
        }
        let replay = ok(post(&f, &f.runner, DISCOVER, operation).await);
        assert_eq!(
            replay["no_op"], true,
            "exact replay still executes no search"
        );
        assert_eq!(audit(&replay["data"])["validity"], "outdated");
        assert!(audit(&replay["data"])["last_search"].is_null());
        assert_eq!(
            replay["data"]["research"]["sources"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let (_, fresh) = search(&f, &replay["data"], vec!["historicalinsightprobe"]).await;
        assert_eq!(
            fresh["no_op"], false,
            "retired-origin access loss prevents audit reuse"
        );
        current_audit(&fresh["data"]);
    }
}

#[tokio::test]
async fn discovery_audit_rejects_forged_authority_and_preserves_record_byte_atomicity() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let (_, result) = search(&f, &s.selected, vec![QUERY]).await;
    let admission = result["data"].clone();
    let receipt = current_audit(&admission).clone();
    let before = current(&f, &job_path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let mut forged = progress_body(
        &admission,
        vec![reviewed(&s.canonical)],
        "researching",
        "A forged audit is never accepted as source-backed search proof.",
    );
    forged["discovery_audit"] =
        json!({"validity":"current","last_search":{"queries":["invented private query"]}});
    assert!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            forged
        )
        .await
        .status
        .is_client_error()
    );
    assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    let mut ignored = research_request(&admission);
    ignored["candidates"] = json!([]);
    ignored["processed_inputs"] = json!([]);
    ignored["discovery_audit"] =
        json!({"validity":"current","last_search":{"queries":["forged candidate audit"]}});
    let ignored = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", ignored).await);
    assert_eq!(current_audit(&ignored), &receipt);
    let mut metadata = current(&f, &job_path(&s.canonical)).await.unwrap().2;
    metadata["dreamer_research"]["coverage"]
        .as_object_mut()
        .unwrap()
        .remove("discovery_audit");
    metadata["dreamer_research"]["receipts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"fixture_padding":""}));
    let padding = 192 * 1024 - 512 - serde_json::to_vec(&metadata).unwrap().len();
    metadata["dreamer_research"]["receipts"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["fixture_padding"] = json!("x".repeat(padding));
    assert_eq!(
        serde_json::to_vec(&metadata).unwrap().len(),
        192 * 1024 - 512
    );
    replace_current_metadata(&f, &s.canonical, metadata).await;
    let before = current(&f, &job_path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let too_large = post(
        &f,
        &f.runner,
        DISCOVER,
        query_body(&ignored, vec![QUERY, MISS]),
    )
    .await;
    assert_eq!(
        too_large.status,
        StatusCode::BAD_REQUEST,
        "{}",
        too_large.body
    );
    assert!(
        too_large.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Research record is full")
    );
    assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn discovery_audit_relevant_write_during_search_cannot_be_stamped_as_current_at_commit() {
    let Some(mut f) = fixture().await else { return };
    let s = setup(&f).await;
    let tag = format!("discovery-audit-{}", f.owner.user);
    let url = std::env::var("BRUNN_TEST_DATABASE_URL").unwrap();
    let mut config = Config::from_env().unwrap();
    let mut rw = Url::parse(&url).unwrap();
    rw.query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    let mut ro = Url::parse(&url).unwrap();
    ro.query_pairs_mut()
        .append_pair("options", "-c role=app_ro")
        .append_pair("application_name", &tag);
    config.database_url_rw = rw.to_string();
    config.database_url_ro = ro.to_string();
    config.database_url_admin = None;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    let mut state = AppState::connect(config).await.unwrap();
    // AppState's normal pool constructor deliberately overrides URL names
    // with "brunn-ro". Give only this fixture's actual search transaction a
    // dedicated connection, then observe its exact verified backend PID.
    state.ro_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(ro.as_str())
        .await
        .unwrap();
    let (search_pid, application_name): (i32, String) =
        sqlx::query_as("SELECT pg_backend_pid(),current_setting('application_name')")
            .fetch_one(&state.ro_pool)
            .await
            .unwrap();
    assert_eq!(application_name, tag);
    f.app = router(state);
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE brunn.search_chunks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let app = f.app.clone();
    let runner = Actor {
        user: f.runner.user,
        id: f.runner.id,
        token: f.runner.token.clone(),
    };
    let operation = query_body(&s.selected, vec!["aster"]);
    let task = tokio::spawn(async move {
        request(&app, &runner, Method::POST, DISCOVER, Some(operation)).await
    });
    let mut waiting = false;
    for _ in 0..150 {
        waiting = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='brunn.search_chunks'::regclass AND NOT granted)")
            .bind(search_pid).fetch_one(&f.pool).await.unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    if !waiting {
        let diagnostic = if task.is_finished() {
            let response = task.await.unwrap();
            format!(
                "request already returned HTTP {}: {}",
                response.status, response.body
            )
        } else {
            task.abort();
            let _ = task.await;
            "request was still pending before the expected search barrier".into()
        };
        blocker.rollback().await.unwrap();
        panic!(
            "the actual query must pause after its initial transaction and pre-search generation: {diagnostic}"
        );
    }
    let canonical_id = Uuid::parse_str(
        s.canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap();
    let content = "# Aster\n\nAster records a revised optical survey outcome.\n";
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    // Append a real fixture source version and change while its query is
    // blocked, without any production hook or mutation of immutable old data.
    sqlx::query("INSERT INTO brunn.entry_versions(user_id,entry_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) VALUES($1,$2,2,$3,$4,$5,'{}'::jsonb,$6)")
        .bind(f.owner.user).bind(canonical_id).bind(&hash).bind(content).bind(content.len() as i64).bind(f.owner.id).execute(&mut *blocker).await.unwrap();
    sqlx::query("UPDATE brunn.entries SET current_version=2 WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(canonical_id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let changed:i64 = sqlx::query_scalar("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) VALUES($1,$2,2,'update',$3,$4) RETURNING generation")
        .bind(f.owner.user).bind(canonical_id).bind(s.canonical["path"].as_str().unwrap()).bind(&hash).fetch_one(&mut *blocker).await.unwrap();
    blocker.commit().await.unwrap();
    let response = ok(task.await.unwrap());
    assert!(
        response["data"]["research"]["snapshot_generation"]
            .as_i64()
            .unwrap()
            >= changed
    );
    assert_eq!(response["data"]["research"]["sources"][0]["version"], 2);
    assert_eq!(
        audit(&response["data"]),
        &json!({"validity":"outdated","last_search":null}),
        "commit-time freshness cannot retroactively prove the earlier query interval"
    );
    let (_, rerun) = search(&f, &response["data"], vec!["aster"]).await;
    assert_eq!(rerun["no_op"], false);
    assert!(
        current_audit(&rerun["data"])["searched_generation"]
            .as_i64()
            .unwrap()
            >= changed
    );
}

#[tokio::test]
async fn discovery_audit_source_excluded_after_search_still_invalidates_at_commit() {
    let Some(mut f) = fixture().await else { return };
    let s = setup(&f).await;
    let url = std::env::var("BRUNN_TEST_DATABASE_URL").unwrap();
    let mut config = Config::from_env().unwrap();
    let mut rw = Url::parse(&url).unwrap();
    rw.query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    let mut ro = Url::parse(&url).unwrap();
    let tag = format!("audit-exclusion-race-{}", f.owner.user);
    ro.query_pairs_mut()
        .append_pair("options", "-c role=app_ro")
        .append_pair("application_name", &tag);
    config.database_url_rw = rw.to_string();
    config.database_url_ro = ro.to_string();
    config.database_url_admin = None;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    let mut state = AppState::connect(config).await.unwrap();
    state.ro_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(ro.as_str())
        .await
        .unwrap();
    state.rw_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(rw.as_str())
        .await
        .unwrap();
    let (search_pid, application_name): (i32, String) =
        sqlx::query_as("SELECT pg_backend_pid(),current_setting('application_name')")
            .fetch_one(&state.ro_pool)
            .await
            .unwrap();
    assert_eq!(application_name, tag);
    let write_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&state.rw_pool)
        .await
        .unwrap();
    f.app = router(state);

    // First stop the actual read after the initial owner transaction. Once it
    // is there, take the owner fence so its later commit transaction must wait.
    let mut search_blocker = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE brunn.search_chunks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *search_blocker)
        .await
        .unwrap();
    let app = f.app.clone();
    let runner = Actor {
        user: f.runner.user,
        id: f.runner.id,
        token: f.runner.token.clone(),
    };
    let operation = query_body(&s.selected, vec![QUERY]);
    let original = operation.clone();
    let task = tokio::spawn(async move {
        request(&app, &runner, Method::POST, DISCOVER, Some(operation)).await
    });
    let mut searching = false;
    for _ in 0..150 {
        searching = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='brunn.search_chunks'::regclass AND NOT granted)")
            .bind(search_pid).fetch_one(&f.pool).await.unwrap();
        if searching {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    if !searching {
        task.abort();
        let _ = task.await;
        search_blocker.rollback().await.unwrap();
        panic!("verified search PID never reached the lexical-read barrier");
    }
    let mut commit_blocker = f.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("brunn-workspace-commit:{}", f.owner.user))
        .execute(&mut *commit_blocker)
        .await
        .unwrap();
    search_blocker.commit().await.unwrap();
    let mut committing = false;
    for _ in 0..150 {
        committing = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND NOT granted)")
            .bind(write_pid).fetch_one(&f.pool).await.unwrap();
        if committing {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    if !committing {
        task.abort();
        let _ = task.await;
        commit_blocker.rollback().await.unwrap();
        panic!("verified writer PID never reached the post-search owner fence");
    }
    let read_finished: bool = sqlx::query_scalar(
        "SELECT state='idle' AND xact_start IS NULL FROM pg_stat_activity WHERE pid=$1",
    )
    .bind(search_pid)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(
        read_finished,
        "the search transaction must finish before the policy change"
    );

    // This was an eligible emitted hit, not a previously admitted dependency.
    // Appending its exclusion only now must not be laundered by a later filter.
    let primary_id = Uuid::parse_str(
        s.primary["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions(user_id,entry_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) SELECT user_id,entry_id,2,content_sha256,content,size_bytes,'{\"evaluation_output\":true}'::jsonb,$3 FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=1")
        .bind(f.owner.user).bind(primary_id).bind(f.owner.id).execute(&mut *commit_blocker).await.unwrap();
    sqlx::query("UPDATE brunn.entries SET current_version=2 WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(primary_id)
        .execute(&mut *commit_blocker)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT e.user_id,e.id,2,'update',e.path,v.content_sha256 FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=2 WHERE e.user_id=$1 AND e.id=$2")
        .bind(f.owner.user).bind(primary_id).execute(&mut *commit_blocker).await.unwrap();
    commit_blocker.commit().await.unwrap();

    let response = ok(task.await.unwrap());
    assert_eq!(response["no_op"], false);
    for group in response["data"]["research"]["coverage"]["query_results"]
        .as_array()
        .unwrap()
    {
        assert_eq!(
            group["returned"], 1,
            "the source was emitted while eligible"
        );
    }
    assert_eq!(
        response["data"]["research"]["coverage"]["change_status"],
        "complete"
    );
    assert!(
        response["data"]["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|source| source["entry_ref"] != s.primary["entry_ref"])
    );
    assert_eq!(
        audit(&response["data"]),
        &json!({"validity":"outdated","last_search":null})
    );
    assert_eq!(response["data"]["inputs"], s.selected["inputs"]);
    let (_, rerun) = search(&f, &response["data"], vec![QUERY]).await;
    assert_eq!(rerun["no_op"], false);
    for group in current_audit(&rerun["data"])["groups"].as_array().unwrap() {
        assert_eq!(
            group["returned"], 0,
            "the next stable search filters the exclusion"
        );
    }
    let notebook = current(&f, &job_path(&s.canonical)).await.unwrap();
    let retained_state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, DISCOVER, original).await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(
        current_audit(&replay["data"]),
        current_audit(&rerun["data"])
    );
    assert_eq!(
        current(&f, &job_path(&s.canonical)).await.unwrap(),
        notebook
    );
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        retained_state
    );
}

#[tokio::test]
async fn discovery_audit_ordinary_snapshots_strip_private_queries_from_historical_only_authority() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let (_, result) = search(&f, &s.selected, vec![QUERY]).await;
    let mut progress = progress_body(
        &result["data"],
        vec![reviewed(&s.primary)],
        "researching",
        "HISTORICAL_SOURCE_MARKER: detector source informs an unfinished check.",
    );
    progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
    progress["reconciled_checkpoints"] = json!([]);
    let first = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    let mut progress = progress_body(
        &first,
        vec![reviewed(&s.canonical)],
        "researching",
        "CURRENT_NOTE_VISIBLE_MARKER: the canonical optical survey observation is independently checked.",
    );
    progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
    progress["reconciled_checkpoints"] = json!([]);
    let second = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress,
    )
    .await)["data"]
        .clone();
    // Model an eligible current notebook that no longer includes one still-
    // accessible historical dependency. This isolates historical-only query
    // provenance from the active manifest audited by ordinary snapshot reads.
    let mut metadata = current(&f, &job_path(&s.canonical)).await.unwrap().2;
    metadata["dreamer_research"]["sources"]
        .as_array_mut()
        .unwrap()
        .retain(|source| source["entry_ref"] == s.canonical["entry_ref"]);
    replace_current_metadata(&f, &s.canonical, metadata).await;
    let (operation, searched) = search(&f, &second, vec!["historicalinsightprobe"]).await;
    let selected = &searched["data"];
    current_audit(selected);
    assert_eq!(selected["research"]["sources"].as_array().unwrap().len(), 1);
    let notebook = current(&f, &job_path(&s.canonical)).await.unwrap();
    assert!(
        notebook.2.to_string().contains("historicalinsightprobe"),
        "the fixture contains the real private stored audit"
    );
    let request =
        json!({"requests":[{"path":job_path(&s.canonical),"version":notebook.0,"view":"full"}]});
    let visible = ok(post(&f, &f.model, "/v1/workspace/read", request.clone()).await);
    assert!(visible.to_string().contains("CURRENT_NOTE_VISIBLE_MARKER"));
    assert!(!visible.to_string().contains("historicalinsightprobe"));
    assert!(!visible.to_string().contains("discovery_audit"));
    ok(request_delete(&f, &s.primary).await);
    let replay = ok(post(&f, &f.runner, DISCOVER, operation).await);
    assert_eq!(audit(&replay["data"])["validity"], "outdated");
    assert!(audit(&replay["data"])["last_search"].is_null());
    let historical = ok(post(&f, &f.model, "/v1/workspace/read", request).await);
    assert!(
        historical
            .to_string()
            .contains("CURRENT_NOTE_VISIBLE_MARKER"),
        "ordinary notes backed by accessible current-manifest sources remain readable"
    );
    assert!(!historical.to_string().contains("historicalinsightprobe"));
    assert!(!historical.to_string().contains("discovery_audit"));
}

async fn request_delete(f: &Fixture, source: &Value) -> Response {
    request(
        &f.app,
        &f.owner,
        Method::DELETE,
        &format!(
            "/v1/workspace/entries/{}?expected_version={}",
            source["entry_ref"].as_str().unwrap(),
            source["version"].as_i64().unwrap()
        ),
        None,
    )
    .await
}

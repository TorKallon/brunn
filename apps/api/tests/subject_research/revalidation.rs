//! Synthetic HTTP/database regressions for historical notebook revalidation.
//! An old notebook is planning context; its selectors never become current evidence.
use super::*;

const NOTES: &str = "REVALIDATION_NOTE_MARKER: the canonical observation and detector schedule were checked; an independent measurement remains unresolved.";

struct Notebook {
    canonical: Value,
    support: Value,
    lead: Value,
    saved: Value,
    operation: Value,
}

fn notebook_path(source: &Value) -> String {
    format!(
        "dreams/research/{}.md",
        source["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    )
}

fn entry_uuid(source: &Value) -> Uuid {
    Uuid::parse_str(
        source["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap()
}

async fn save_progress(f: &Fixture, body: Value) -> Value {
    ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await)["data"]
        .clone()
}

async fn notebook(f: &Fixture) -> Notebook {
    control(f, "report-only", 0).await;
    let canonical = write(
        f,
        "sources/People/Cedar.md",
        "# Cedar\n\nCedar has a checked primary observation.\n",
        0,
    )
    .await;
    let support = write(
        f,
        "sources/Optics/Detector.md",
        "# Detector\n\nThe detector schedule records an exposure interval.\n",
        0,
    )
    .await;
    let lead = write(
        f,
        "sources/Optics/Measurement.md",
        "# Measurement\n\nAn independent measurement remains available for inspection.\n",
        0,
    )
    .await;
    let admitted = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({
            "attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60,
            "requested_subject_refs":[canonical["entry_ref"]]
        }),
    )
    .await);
    let selected = next_subject(f, &admitted).await;
    assert_eq!(selected["research"]["subject_ref"], canonical["entry_ref"]);
    let (_, selected) = discover_subject(
        f,
        &selected,
        vec![support["entry_ref"].clone(), lead["entry_ref"].clone()],
    )
    .await;
    let mut operation = progress_body(
        &selected,
        vec![reviewed(&canonical), reviewed(&support)],
        "researching",
        NOTES,
    );
    operation["pending_queries"] = json!(["independent detector measurement"]);
    operation["pending_targets"] = json!([lead["entry_ref"]]);
    let saved = save_progress(f, operation.clone()).await;
    assert!(saved["research"].get("revalidation_checkpoint").is_none());
    assert!(saved["research"]["revalidation_context"].is_null());
    Notebook {
        canonical,
        support,
        lead,
        saved,
        operation,
    }
}

fn context(admission: &Value) -> &Value {
    &admission["research"]["revalidation_context"]
}

fn compact(source: &Value) -> Value {
    json!({"entry_ref":source["entry_ref"],"version":source["version"],"start_line":source["start_line"],"end_line":source["end_line"]})
}

fn assert_context(admission: &Value, prior: &Value) {
    let historical = context(admission);
    assert_eq!(
        historical["status"], "historical_revalidation_only",
        "{admission}"
    );
    assert_eq!(
        historical["origin"]["version"],
        prior["research"]["version"]
    );
    assert_eq!(
        historical["origin"]["snapshot_generation"],
        prior["research"]["snapshot_generation"]
    );
    assert_eq!(historical["notes"], prior["research"]["notes"]);
    assert_eq!(
        historical["prior_reviewed_sources"],
        json!(
            prior["research"]["reviewed_sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(compact)
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        historical["prior_pending_queries"],
        prior["research"]["pending_queries"]
    );
    assert_eq!(
        historical["prior_pending_targets"],
        prior["research"]["pending_targets"]
    );
    assert_eq!(
        historical["prior_progress"]["round"],
        prior["research"]["round"]
    );
    assert_eq!(
        historical["prior_progress"]["admitted_source_count"],
        prior["research"]["sources"].as_array().unwrap().len()
    );
    assert_eq!(
        historical["prior_progress"]["reviewed_selector_count"],
        prior["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .len()
    );
    assert_eq!(
        historical["prior_progress"]["source_cap_reached"],
        prior["research"]["coverage"]["source_cap_reached"]
    );
    assert!(historical.get("receipts").is_none());
    assert!(historical.get("discoveries").is_none());
    for selector in historical["prior_reviewed_sources"].as_array().unwrap() {
        assert!(selector.get("path").is_none());
        assert!(selector.get("excerpt").is_none());
    }
    assert!(
        admission["research"]
            .get("revalidation_checkpoint")
            .is_none()
    );
    assert!(serde_json::to_vec(historical).unwrap().len() <= 96 * 1024);
}

async fn checkpoint(f: &Fixture, canonical: &Value) -> Value {
    current(f, &notebook_path(canonical)).await.unwrap().2["dreamer_research"]["revalidation_checkpoint"].clone()
}

async fn replay_progress(f: &Fixture, operation: &Value) -> Value {
    let response = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        operation.clone(),
    )
    .await);
    assert_eq!(response["no_op"], true);
    response["data"].clone()
}

async fn exact_metadata(f: &Fixture, path: &str, version: i64) -> Value {
    sqlx::query_scalar("SELECT v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id WHERE e.user_id=$1 AND e.path=$2 AND v.version=$3")
        .bind(f.owner.user).bind(path).bind(version).fetch_one(&f.pool).await.unwrap()
}

async fn reload_router(f: &mut Fixture) {
    let url = std::env::var("BRUNN_TEST_DATABASE_URL").unwrap();
    let mut config = Config::from_env().unwrap();
    let mut rw = Url::parse(&url).unwrap();
    rw.query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    let mut ro = Url::parse(&url).unwrap();
    ro.query_pairs_mut()
        .append_pair("options", "-c role=app_ro");
    config.database_url_rw = rw.to_string();
    config.database_url_ro = ro.to_string();
    config.database_url_admin = None;
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    f.app = router(AppState::connect(config).await.unwrap());
}

async fn replace_fixture_metadata(f: &Fixture, path: &str, version: i64, metadata: Value) {
    // Isolated fixture corruption/legacy simulation only; production versions stay immutable.
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$4 FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3")
        .bind(f.owner.user).bind(path).bind(version).bind(metadata).execute(&f.pool).await.unwrap();
}

/// Yield the pass and start the next one so post-cutoff versions are pinned.
async fn repin(f: &Fixture, admission: &Value, canonical: &Value) -> Value {
    yield_and_finish(f, admission).await;
    let next = next_subject(
        f,
        &admit_requested(f, vec![canonical["entry_ref"].clone()]).await,
    )
    .await;
    assert_eq!(next["research"]["subject_ref"], canonical["entry_ref"]);
    next
}

async fn drift(f: &Fixture, source: &Value) -> Value {
    let version = source["version"].as_i64().unwrap();
    write(f, source["path"].as_str().unwrap(), &format!("# Detector\n\nThe detector schedule now records a revised exposure interval.\n\nRevision {}.\n", version + 1), version).await
}

#[tokio::test]
async fn revalidation_retains_latest_discovery_context_across_scope_refresh_and_selection() {
    for selection in [false, true] {
        let Some(f) = fixture().await else { return };
        let n = notebook(&f).await;
        let mut discovery = research_request(&n.saved);
        discovery["queries"] = json!(["detector schedule"]);
        discovery["targets"] = json!([]);
        let prior = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            discovery,
        )
        .await)["data"]
            .clone();
        assert_eq!(prior["research"]["notes"], NOTES);
        assert!(
            context(&prior).is_null(),
            "additive unchanged discovery does not create history"
        );
        assert!(
            !prior["research"]["coverage"]["query_results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let path = notebook_path(&n.canonical);
        let original = current(&f, &path).await.unwrap();
        let added = write(
            &f,
            "sources/Optics/NewOutcome.md",
            "# New outcome\n\nCedar has a new relevant optical observation.\n",
            0,
        )
        .await;
        // A relevant source written after the cutoff is queued for the next
        // pass; the checked notes stay current instead of becoming history.
        let ephemeral = replay_progress(&f, &n.operation).await;
        assert!(context(&ephemeral).is_null());
        assert_eq!(ephemeral["research"]["notes"], NOTES);
        assert_eq!(
            ephemeral["research"]["reviewed_sources"],
            prior["research"]["reviewed_sources"]
        );
        assert_ne!(ephemeral["research"]["needs_refresh"], true);
        assert_eq!(
            current(&f, &path).await.unwrap(),
            original,
            "a read-only replay cannot persist a pointer"
        );
        let refreshed = if selection {
            repin(&f, &prior, &n.canonical).await
        } else {
            discover_subject(&f, &prior, vec![]).await.1
        };
        assert_eq!(
            refreshed["research"]["subject_ref"],
            n.canonical["entry_ref"]
        );
        assert!(context(&refreshed).is_null());
        assert_eq!(refreshed["research"]["notes"], NOTES);
        assert_eq!(
            refreshed["research"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["entry_ref"] == added["entry_ref"] && s["version"] == added["version"]),
            selection,
            "the new relevant source joins the next pass, not the pinned one"
        );
        if selection {
            let queued = refreshed["research"]["pass"]["changed"].as_array().unwrap();
            assert!(
                queued
                    .iter()
                    .any(|c| c["entry_ref"] == added["entry_ref"] && c["kind"] == "new_relevant"),
                "{queued:?}"
            );
        }
        assert_eq!(exact_metadata(&f, &path, original.0).await, original.2);
        let mut selectors = vec![reviewed(&n.canonical), reviewed(&n.support)];
        if selection {
            selectors.push(reviewed(&added));
        }
        let replacement = save_progress(&f, progress_body(&refreshed, selectors, "researching", "The old observations and the new relevant outcome have been reconciled; the independent measurement remains open.")).await;
        assert!(context(&replacement).is_null());
        assert_eq!(checkpoint(&f, &n.canonical).await, json!({"state":"none"}));
        assert_eq!(
            replacement["inputs"], refreshed["inputs"],
            "revalidation alone never consumes inputs"
        );
    }
}

#[tokio::test]
async fn revalidation_version_drift_rejects_old_evidence_and_zero_id_progress_preserves_origin() {
    let Some(f) = fixture().await else { return };
    let n = notebook(&f).await;
    let newer = drift(&f, &n.support).await;
    let refreshed = repin(&f, &n.saved, &n.canonical).await;
    assert!(context(&refreshed).is_null());
    assert_eq!(refreshed["research"]["notes"], NOTES, "the prose is kept");
    assert_eq!(
        refreshed["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "the moved selector is dropped until reread"
    );
    assert!(
        refreshed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == newer["entry_ref"] && s["version"] == 2)
    );
    let path = notebook_path(&n.canonical);
    let before = current(&f, &path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let old = progress_body(
        &refreshed,
        vec![reviewed(&n.canonical), reviewed(&n.support)],
        "researching",
        "An old selector cannot be promoted to current evidence.",
    );
    let denied = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        old,
    )
    .await;
    assert!(denied.status.is_client_error(), "{}", denied.body);
    assert_eq!(current(&f, &path).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    let selectors = vec![reviewed(&n.canonical), reviewed(&newer)];
    let mut zero = research_request(&refreshed);
    zero["candidates"] = json!([]);
    zero["processed_inputs"] = json!([]);
    zero["research_progress"] = json!({"status":"researching","notes":"ZERO_ID_NOTE_MARKER: current source versions were checked, with the independent measurement still open.","reviewed_sources":selectors,"pending_queries":[],"pending_targets":[n.lead["entry_ref"]]});
    let partial = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", zero).await);
    assert_eq!(partial["accepted_candidate_ids"], json!([]));
    assert_eq!(
        partial["research"]["notes"],
        "ZERO_ID_NOTE_MARKER: current source versions were checked, with the independent measurement still open."
    );
    assert!(context(&partial).is_null());
    let newest = drift(&f, &newer).await;
    let reset = repin(&f, &partial, &n.canonical).await;
    assert!(context(&reset).is_null());
    assert_eq!(
        reset["research"]["notes"],
        "ZERO_ID_NOTE_MARKER: current source versions were checked, with the independent measurement still open."
    );
    let mut proposal = candidate(&n.canonical, "cedar-revalidation");
    proposal["subject_ref"] = n.canonical["entry_ref"].clone();
    proposal["path"] = reset["research"]["output_path"].clone();
    proposal["expected_version"] = reset["research"]["output_version"].clone();
    proposal["content"] = json!(
        "# Cedar\n\nCedar has a checked primary observation.[^s1]\nThe detector schedule records a revised exposure interval.[^s2]\n"
    );
    proposal["sources"] = json!([reviewed(&n.canonical), reviewed(&newest)]);
    let mut submission = research_request(&reset);
    submission["candidates"] = json!([proposal]);
    submission["processed_inputs"] = json!([]);
    let accepted = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        submission,
    )
    .await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    assert!(context(&accepted).is_null());
    assert_eq!(checkpoint(&f, &n.canonical).await, json!({"state":"none"}));
}

#[tokio::test]
async fn revalidation_stale_waiting_and_replays_preserve_context_until_supported_completion() {
    let Some(mut f) = fixture().await else { return };
    let n = notebook(&f).await;
    let newer = drift(&f, &n.support).await;
    let mut waiting = research_request(&n.saved);
    waiting["status"] = json!("waiting");
    waiting["processed_inputs"] = json!([]);
    let mut current_admission = save_progress(&f, waiting.clone()).await;
    assert!(context(&current_admission).is_null());
    assert_eq!(current_admission["research"]["notes"], NOTES);
    assert_eq!(
        current_admission["research"]["pending_queries"],
        json!(["independent detector measurement"])
    );
    assert_eq!(
        current_admission["research"]["pending_targets"],
        json!([n.lead["entry_ref"]])
    );
    assert_eq!(current_admission["research"]["pass"]["yielded"], true);
    let (_, refreshed) = discover_subject(&f, &current_admission, vec![]).await;
    current_admission = refreshed;
    for kind in ["empty", "omitted", "repair"] {
        let mut body = research_request(&current_admission);
        body["status"] = json!("waiting");
        body["processed_inputs"] = json!([]);
        if kind == "empty" {
            body["notes"] = json!("");
            body["reviewed_sources"] = json!([]);
        } else if kind == "repair" {
            body["repair_feedback"] = json!({"phase":"response_validation","message":"Return one valid structured checkpoint."});
        }
        current_admission = save_progress(&f, body).await;
        assert!(context(&current_admission).is_null());
        if kind == "empty" {
            assert_eq!(current_admission["research"]["notes"], "");
        }
    }
    // Restore reviewed work so the moved support source is reliance again.
    current_admission = save_progress(
        &f,
        progress_body(
            &current_admission,
            vec![reviewed(&n.canonical), reviewed(&n.support)],
            "waiting",
            NOTES,
        ),
    )
    .await;
    let path = notebook_path(&n.canonical);
    let before = current(&f, &path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = replay_progress(&f, &waiting).await;
    assert!(context(&replay).is_null());
    assert_eq!(current(&f, &path).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    let mut changed = waiting;
    changed["notes"] = json!("Rejected same-operation notes must not become historical context.");
    assert!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            changed
        )
        .await
        .status
        .is_client_error()
    );
    assert_eq!(current(&f, &path).await.unwrap(), before);
    finish(
        &f,
        &current_admission,
        current_admission["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    reload_router(&mut f).await;
    // The explicit yield lets the next selection re-pin at a newer cutoff;
    // completion must reread the moved reliance source.
    let restarted = next_subject(
        &f,
        &admit_requested(&f, vec![n.canonical["entry_ref"].clone()]).await,
    )
    .await;
    assert!(context(&restarted).is_null());
    let queued = restarted["research"]["pass"]["changed"].as_array().unwrap();
    assert!(
        queued
            .iter()
            .any(|c| c["entry_ref"] == newer["entry_ref"] && c["required"] == true),
        "{queued:?}"
    );
    let done = save_progress(&f, progress_body(&restarted, vec![reviewed(&n.canonical), reviewed(&newer)], "no_change", "The current canonical source and revised schedule were inspected; no proposal is required.")).await;
    assert!(context(&done).is_null());
    assert_eq!(
        done["research"]["reviewed_through"]["disposition"],
        "no_change"
    );
    assert_eq!(checkpoint(&f, &n.canonical).await, json!({"state":"none"}));
}

#[tokio::test]
async fn revalidation_audits_cited_and_uncited_original_dependencies_before_and_after_pruning() {
    for loss in [
        "uncited_deleted",
        "cited_generated",
        "uncited_sensitive",
        "uncited_access",
        "original_generated",
        "canonical_deleted",
    ] {
        let Some(f) = fixture().await else { return };
        let n = notebook(&f).await;
        let newer = drift(&f, &n.support).await;
        let (discovery, retained) = discover_subject(&f, &n.saved, vec![]).await;
        assert!(context(&retained).is_null());
        assert_eq!(retained["research"]["notes"], NOTES);
        let path = notebook_path(&n.canonical);
        let origin_version = n.saved["research"]["version"].as_i64().unwrap();
        let original = exact_metadata(&f, &path, origin_version).await;
        let read = json!({"requests":[{"path":path,"version":origin_version,"view":"full"}]});
        assert!(
            ok(post(&f, &f.model, "/v1/workspace/read", read.clone()).await)
                .to_string()
                .contains("REVALIDATION_NOTE_MARKER")
        );
        match loss {
            "uncited_deleted" | "canonical_deleted" => {
                let source = if loss == "canonical_deleted" {
                    &n.canonical
                } else {
                    &n.lead
                };
                ok(request(
                    &f.app,
                    &f.owner,
                    Method::DELETE,
                    &format!(
                        "/v1/workspace/entries/{}?expected_version=1",
                        source["entry_ref"].as_str().unwrap()
                    ),
                    None,
                )
                .await);
            }
            "cited_generated" => {
                ok(post(&f, &f.owner, "/v1/workspace/write", json!({"path":newer["path"],"expected_version":2,"content":"# Detector\n\nA generated edition now occupies the identity.\n","metadata":{"kind":"briefing_edition"}})).await);
            }
            "uncited_sensitive" | "uncited_access" => {
                let target = if loss == "uncited_sensitive" {
                    "sources/Credentials/Measurement.md".to_owned()
                } else {
                    format!(".brunn/tasks/{}.md", Uuid::now_v7())
                };
                sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
                    .bind(f.owner.user)
                    .bind(entry_uuid(&n.lead))
                    .bind(target)
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            "original_generated" => {
                // A current eligible replacement cannot launder an excluded exact old version.
                sqlx::query("UPDATE brunn.entry_versions SET metadata=metadata||'{\"kind\":\"briefing_edition\"}'::jsonb WHERE user_id=$1 AND entry_id=$2 AND version=1")
                    .bind(f.owner.user).bind(entry_uuid(&n.support)).execute(&f.pool).await.unwrap();
            }
            _ => unreachable!(),
        }
        let before = current(&f, &path).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let replay = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            discovery.clone(),
        )
        .await);
        assert_eq!(replay["no_op"], true);
        assert!(context(&replay["data"]).is_null(), "{loss}: {replay}");
        // Losing reviewed evidence withholds the cached conclusions; losing an
        // admitted but unreviewed lead does not.
        let relied = matches!(
            loss,
            "cited_generated" | "original_generated" | "canonical_deleted"
        );
        assert_eq!(
            !replay["data"]["research"]["notes"]
                .as_str()
                .unwrap_or_default()
                .contains("REVALIDATION_NOTE_MARKER"),
            relied,
            "{loss}: {replay}"
        );
        assert_eq!(current(&f, &path).await.unwrap(), before);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let historical = ok(post(&f, &f.model, "/v1/workspace/read", read.clone()).await);
        assert!(
            !historical.to_string().contains("REVALIDATION_NOTE_MARKER"),
            "{loss}"
        );
        if loss == "canonical_deleted" {
            assert!(replay["data"]["research"].is_null());
        } else {
            let (_, refreshed) = discover_subject(&f, &retained, vec![]).await;
            assert!(
                context(&refreshed).is_null(),
                "pruning must not make old prose accessible: {loss}"
            );
            let replay = ok(post(
                &f,
                &f.runner,
                "/v1/workspace/dreamer/narrative-discover",
                discovery,
            )
            .await);
            assert!(context(&replay["data"]).is_null(), "{loss}");
            assert!(
                !ok(post(&f, &f.model, "/v1/workspace/read", read).await)
                    .to_string()
                    .contains("REVALIDATION_NOTE_MARKER"),
                "historical notebook reads keep their immutable manifest authority: {loss}"
            );
        }
        assert_eq!(
            exact_metadata(&f, &path, origin_version).await,
            original,
            "projection never rewrites the old notebook: {loss}"
        );
    }
}

#[tokio::test]
async fn revalidation_legacy_recovery_is_once_only_limited_to_latest_sixteen_versions() {
    for mode in [
        "recent",
        "initialized_none",
        "outside_window",
        "malformed_newest",
        "inaccessible_newest",
    ] {
        let Some(f) = fixture().await else { return };
        let n = notebook(&f).await;
        let path = notebook_path(&n.canonical);
        let mut saved = n.saved.clone();
        let mut expected_origin = n.saved["research"]["version"].as_i64().unwrap();
        let mut malformed_version = None;
        if matches!(mode, "inaccessible_newest" | "malformed_newest") {
            saved = save_progress(
                &f,
                progress_body(
                    &saved,
                    vec![reviewed(&n.canonical), reviewed(&n.support)],
                    "researching",
                    NOTES,
                ),
            )
            .await;
            let latest = saved["research"]["version"].as_i64().unwrap();
            if mode == "inaccessible_newest" {
                // Mutate only the older version after the newer full manifest
                // has been persisted, so falling back would bypass the loss.
                let mut older = exact_metadata(&f, &path, expected_origin).await;
                older["dreamer_research"]["sources"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|s| s["entry_ref"] != n.lead["entry_ref"]);
                older["dreamer_research"]["pending_targets"] = json!([]);
                older["dreamer_research"]["notes"] =
                    json!("OLDER_ACCESSIBLE_NOTE_MARKER: a narrower earlier notebook.");
                replace_fixture_metadata(&f, &path, expected_origin, older).await;
                expected_origin = latest;
            } else {
                malformed_version = Some(latest);
            }
        }
        let mut last_empty = Value::Null;
        for _ in 0..if mode == "outside_window" { 17 } else { 1 } {
            last_empty = progress_body(&saved, vec![], "researching", "");
            saved = save_progress(&f, last_empty.clone()).await;
        }
        if let Some(version) = malformed_version {
            let mut malformed = exact_metadata(&f, &path, version).await;
            malformed["dreamer_research"]["schema"] = json!("dream.research.malformed");
            replace_fixture_metadata(&f, &path, version, malformed).await;
        }
        if mode != "initialized_none" {
            let mut legacy = current(&f, &path).await.unwrap();
            legacy.2["dreamer_research"]
                .as_object_mut()
                .unwrap()
                .remove("revalidation_checkpoint");
            replace_fixture_metadata(&f, &path, legacy.0, legacy.2).await;
        }
        if mode == "inaccessible_newest" {
            sqlx::query(
                "UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
            )
            .bind(f.owner.user)
            .bind(entry_uuid(&n.lead))
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let before = current(&f, &path).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let replay = replay_progress(&f, &last_empty).await;
        assert!(
            context(&replay).is_null(),
            "replay cannot perform lazy legacy recovery"
        );
        assert_eq!(current(&f, &path).await.unwrap(), before);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let started = std::time::Instant::now();
        let (_, recovered) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            discover_subject(&f, &saved, vec![]),
        )
        .await
        .expect("bounded legacy recovery fits the existing client deadline");
        if mode == "outside_window" {
            eprintln!(
                "Legacy recovery beyond sixteen empty versions: {:?}",
                started.elapsed()
            );
        }
        if matches!(mode, "recent" | "malformed_newest") {
            assert_context(&recovered, &n.saved);
            assert_eq!(
                checkpoint(&f, &n.canonical).await,
                json!({"state":"retained","version":expected_origin})
            );
        } else {
            assert!(context(&recovered).is_null(), "{mode}: {recovered}");
            assert!(
                !recovered
                    .to_string()
                    .contains("OLDER_ACCESSIBLE_NOTE_MARKER")
            );
            if mode != "inaccessible_newest" {
                assert_eq!(checkpoint(&f, &n.canonical).await, json!({"state":"none"}));
            } else {
                assert_eq!(
                    checkpoint(&f, &n.canonical).await,
                    json!({"state":"retained","version":expected_origin})
                );
            }
        }
        let selected_pointer = checkpoint(&f, &n.canonical).await;
        let (_, again) = discover_subject(&f, &recovered, vec![]).await;
        assert_eq!(checkpoint(&f, &n.canonical).await, selected_pointer);
        assert_eq!(
            context(&again),
            context(&recovered),
            "initialized recovery never scans around a selected failure or deliberate empty state"
        );
    }
}

#[tokio::test]
async fn revalidation_rejects_malformed_origins_and_server_field_injection_without_following_embedded_pointers()
 {
    let Some(f) = fixture().await else { return };
    let n = notebook(&f).await;
    drift(&f, &n.support).await;
    let (discovery, retained) = discover_subject(&f, &n.saved, vec![]).await;
    let path = notebook_path(&n.canonical);
    let origin_version = n.saved["research"]["version"].as_i64().unwrap();
    let original = exact_metadata(&f, &path, origin_version).await;
    let head = current(&f, &path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    for field in ["revalidation_checkpoint", "revalidation_context"] {
        let mut injection = research_request(&retained);
        injection["status"] = json!("waiting");
        injection[field] = json!({"state":"retained","version":origin_version});
        let denied = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            injection.clone(),
        )
        .await;
        assert!(denied.status.is_client_error(), "{field}: {}", denied.body);
        let mut candidate = research_request(&retained);
        candidate["candidates"] = json!([]);
        candidate["research_progress"] = injection;
        assert!(
            post(&f, &f.runner, "/v1/workspace/dreamer/candidates", candidate)
                .await
                .status
                .is_client_error()
        );
        assert_eq!(current(&f, &path).await.unwrap(), head);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    }
    for bad_version in [0, -1, head.0, head.0 + 100] {
        let mut bad = head.2.clone();
        bad["dreamer_research"]["revalidation_checkpoint"] =
            json!({"state":"retained","version":bad_version});
        replace_fixture_metadata(&f, &path, head.0, bad).await;
        let response = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            discovery.clone(),
        )
        .await;
        assert!(!response.status.is_server_error(), "{}", response.body);
        assert!(
            context(&response.body["data"]).is_null(),
            "{bad_version}: {}",
            response.body
        );
    }
    replace_fixture_metadata(&f, &path, head.0, head.2.clone()).await;
    let foreign = actor(&f.pool, None, OWNER_CAPS).await;
    let foreign_source = ok(post(&f, &foreign, "/v1/workspace/write", json!({"path":"sources/Foreign/Observation.md","content":"# Foreign\n\nA different owner's source.\n","expected_version":0,"metadata":{}})).await)["data"].clone();
    for defect in [
        "subject",
        "schema",
        "notes",
        "selectors",
        "span",
        "snapshot",
        "membership",
        "foreign",
        "queries",
        "targets",
    ] {
        let mut bad = original.clone();
        let job = &mut bad["dreamer_research"];
        match defect {
            "subject" => job["subject_ref"] = n.support["entry_ref"].clone(),
            "schema" => job["schema"] = json!("dream.research.invalid"),
            "notes" => job["notes"] = json!("x".repeat(12 * 1024 + 1)),
            "selectors" => job["reviewed_sources"] = json!(vec![reviewed(&n.canonical); 65]),
            "span" => job["reviewed_sources"][0]["end_line"] = json!(500),
            "snapshot" => job["snapshot_generation"] = json!(0),
            "membership" => job["sources"]
                .as_array_mut()
                .unwrap()
                .retain(|s| s["entry_ref"] != n.support["entry_ref"]),
            "foreign" => {
                // Keep canonical identity and reviewed membership valid: the
                // uncited foreign dependency must fail the complete access audit.
                let lead = job["sources"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|s| s["entry_ref"] == n.lead["entry_ref"])
                    .unwrap();
                lead["entry_ref"] = foreign_source["entry_ref"].clone();
            }
            "queries" => job["pending_queries"] = json!(vec!["unresolved measurement"; 13]),
            "targets" => job["pending_targets"] = json!(["x".repeat(1025)]),
            _ => unreachable!(),
        }
        replace_fixture_metadata(&f, &path, origin_version, bad).await;
        let response = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            discovery.clone(),
        )
        .await);
        assert!(context(&response["data"]).is_null(), "{defect}: {response}");
        assert_eq!(
            current(&f, &path).await.unwrap(),
            head,
            "read-only validation failure leaves the retained pointer intact"
        );
    }
    // Origins from legitimate partial progress may carry an older pointer. Its
    // shape or destination is irrelevant because only this origin is expanded.
    for embedded in [
        json!({"state":"retained","version":origin_version}),
        json!({"state":"retained","version":999999}),
        json!({"state":"malformed","entry_ref":foreign_source["entry_ref"]}),
    ] {
        let mut origin = original.clone();
        origin["dreamer_research"]["revalidation_checkpoint"] = embedded;
        replace_fixture_metadata(&f, &path, origin_version, origin).await;
        let response = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            discovery.clone(),
        )
        .await);
        assert!(
            context(&response["data"]).is_null(),
            "an embedded pointer in an origin is never followed"
        );
        assert_eq!(response["data"]["research"]["notes"], NOTES);
    }
    replace_fixture_metadata(&f, &path, origin_version, original).await;
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn revalidation_maximum_notes_selectors_and_leads_remain_bounded_under_client_deadline() {
    let Some(f) = fixture_with_deadline(Some(std::time::Duration::from_secs(30))).await else {
        return;
    };
    let n = notebook(&f).await;
    let source_text = format!(
        "# Detector\n\n{}",
        (0..64)
            .map(|i| format!("Measurement {i}: {}\n", "synthetic detail ".repeat(256)))
            .collect::<String>()
    );
    let expanded = write(&f, n.support["path"].as_str().unwrap(), &source_text, 1).await;
    let selected = repin(&f, &n.saved, &n.canonical).await;
    let mut selectors = vec![reviewed(&n.canonical)];
    selectors.extend((3..66).map(|line| json!({"entry_ref":expanded["entry_ref"],"version":expanded["version"],"start_line":line,"end_line":line})));
    assert_eq!(selectors.len(), 64);
    let mut body = progress_body(&selected, selectors, "researching", &"n".repeat(12 * 1024));
    body["pending_queries"] = json!(
        (0..12)
            .map(|i| format!("{i:02}{}", "q".repeat(158)))
            .collect::<Vec<_>>()
    );
    body["pending_targets"] = json!(
        (0..32)
            .map(|i| format!("sources/Followups/{i:02}{}.md", "t".repeat(1001)))
            .collect::<Vec<_>>()
    );
    assert!(
        body["pending_targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p.as_str().unwrap().len() == 1024)
    );
    let maximum = save_progress(&f, body.clone()).await;
    drift(&f, &expanded).await;
    let before = current(&f, &notebook_path(&n.canonical)).await.unwrap();
    let started = std::time::Instant::now();
    let historical = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        replay_progress(&f, &body),
    )
    .await
    .expect("bounded historical projection must fit the existing 30-second client deadline");
    let elapsed = started.elapsed();
    // A post-cutoff version keeps the maximal notebook current and served.
    assert!(context(&historical).is_null());
    assert_eq!(
        historical["research"]["notes"],
        maximum["research"]["notes"]
    );
    assert_eq!(
        historical["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        historical["research"]["pending_queries"]
            .as_array()
            .unwrap()
            .len(),
        12
    );
    assert_eq!(
        historical["research"]["pending_targets"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    let bytes = serde_json::to_vec(&historical["research"]).unwrap().len();
    eprintln!(
        "Maximum historical research projection: {bytes} bytes, {elapsed:?}; 64 exact selectors, 12 queries, 32 targets"
    );
    assert!(elapsed < std::time::Duration::from_secs(30));
    assert_eq!(
        current(&f, &notebook_path(&n.canonical)).await.unwrap(),
        before
    );
}

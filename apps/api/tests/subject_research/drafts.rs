//! Synthetic HTTP/database gates for custody of an unaccepted research draft.
//! Custody preserves a proposal for revalidation; it grants no evidence or Review authority.
use super::*;

const PROTOCOL: &str = "dream.research.draft.v1";
const PROGRESS: &str = "/v1/workspace/dreamer/research-progress";
const CANDIDATES: &str = "/v1/workspace/dreamer/candidates";
const MARKER: &str = "UNACCEPTED_DRAFT_PRIVATE_MARKER";

struct Scenario {
    canonical: Value,
    support: Value,
    unadmitted: Value,
    admission: Value,
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

fn job_path(source: &Value) -> String {
    format!("dreams/research/{}.md", entry_uuid(source))
}

fn draft_path(source: &Value) -> String {
    format!("dreams/reviews/research-draft-{}.md", entry_uuid(source))
}

async fn scenario(f: &Fixture) -> Scenario {
    control(f, "report-only", 0).await;
    let canonical = write(
        f,
        "sources/Projects/Prism/Prism.md",
        "# Prism\n\nPrism is an optical survey project.\n",
        0,
    )
    .await;
    let support = write(
        f,
        "sources/Optics/Detector.md",
        "# Detector\n\nThe optical survey uses a calibrated detector.\n",
        0,
    )
    .await;
    let unadmitted = write(
        f,
        "sources/Other/Unrelated.md",
        "# Other\n\nAn independent project has a separate observation.\n",
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
    assert_eq!(selected["research"]["draft_protocol"], PROTOCOL);
    assert!(selected["research"]["unaccepted_draft"].is_null());
    let (_, admission) = discover_subject(f, &selected, vec![support["entry_ref"].clone()]).await;
    assert!(
        admission["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == support["entry_ref"])
    );
    assert!(
        !admission["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == unadmitted["entry_ref"])
    );
    Scenario {
        canonical,
        support,
        unadmitted,
        admission,
    }
}

fn proposal(admission: &Value, canonical: &Value, revision: &str) -> Value {
    let mut value = candidate(canonical, "Prism");
    value["subject_ref"] = canonical["entry_ref"].clone();
    value["path"] = admission["research"]["output_path"].clone();
    value["expected_version"] = admission["research"]["output_version"].clone();
    value["title"] = json!(format!("{MARKER} {revision}"));
    value["content"] = json!(format!(
        "# Prism {revision}\n\nPrism is an optical survey project.[^s1]\n"
    ));
    value
}

fn custody(admission: &Value, candidate: Value, replacement: Option<Value>) -> Value {
    let mut body = research_request(admission);
    body["draft_protocol"] = json!(PROTOCOL);
    body["draft_candidate"] = candidate;
    body["processed_inputs"] = json!([]);
    if let Some(pointer) = replacement {
        body["replaces_draft"] = pointer;
        body["findings"] = json!([
            "The replacement incorporates the earlier survey identity and deliberately reconsiders its remaining scope against the current primary sources."
        ]);
    }
    body
}

async fn save(f: &Fixture, operation: Value) -> Value {
    ok(post(f, &f.runner, PROGRESS, operation).await)["data"].clone()
}

fn draft(admission: &Value) -> &Value {
    &admission["research"]["unaccepted_draft"]
}

fn pointer(admission: &Value) -> Value {
    let projected = draft(admission);
    assert_eq!(
        projected["status"], "unaccepted_revalidation_only",
        "{admission}"
    );
    let pointer = projected["pointer"].clone();
    assert!(pointer["entry_ref"].as_str().unwrap().starts_with("entry:"));
    assert!(pointer["version"].as_i64().unwrap() > 0);
    let hash = pointer["candidate_hash"].as_str().unwrap();
    assert_eq!(hash.len(), 64);
    assert!(hash.bytes().all(|c| c.is_ascii_hexdigit()));
    pointer
}

fn assert_draft(admission: &Value, candidate: &Value, expected_pointer: &Value) {
    assert_eq!(pointer(admission), *expected_pointer);
    for field in [
        "content",
        "title",
        "subject_ref",
        "path",
        "expected_version",
    ] {
        assert_eq!(
            draft(admission)["candidate"][field],
            candidate[field],
            "exact draft {field}"
        );
    }
    let actual = draft(admission)["candidate"]["sources"].as_array().unwrap();
    let expected = candidate["sources"].as_array().unwrap();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        for field in ["entry_ref", "version", "start_line", "end_line"] {
            assert_eq!(actual[field], expected[field]);
        }
        assert!(actual.get("excerpt").is_none_or(|v| v == ""));
    }
    assert!(serde_json::to_vec(draft(admission)).unwrap().len() <= 64 * 1024);
}

fn assert_receipt(
    admission: &Value,
    operation: &Value,
    expected_pointer: &Value,
    replayed: bool,
    retired: bool,
) {
    assert_eq!(
        admission["draft_receipt"],
        json!({
            "protocol":PROTOCOL,"operation_id":operation["operation_id"],"recorded":true,
            "replayed":replayed,"pointer":expected_pointer,"retired":retired
        })
    );
}

fn assert_no_work_credit(before: &Value, after: &Value) {
    for field in [
        "inputs",
        "processed_count",
        "processed_generation",
        "items",
        "source_dispositions",
        "pending_notifications",
        "research",
    ] {
        assert_eq!(
            after["dreamer_state"][field], before["dreamer_state"][field],
            "draft custody must not change {field}"
        );
    }
}

async fn rejected_unchanged(
    f: &Fixture,
    canonical: &Value,
    endpoint: &str,
    operation: Value,
) -> Value {
    let job = current(f, &job_path(canonical)).await;
    let draft = current(f, &draft_path(canonical)).await;
    let state = current(f, "dreams/state.md").await;
    let run = current(f, &format!("dreams/runs/{}.md", date())).await;
    let rejected = post(f, &f.runner, endpoint, operation).await;
    assert!(rejected.status.is_client_error(), "{}", rejected.body);
    assert_eq!(
        current(f, &job_path(canonical)).await,
        job,
        "failed transaction cannot mutate the notebook"
    );
    assert_eq!(
        current(f, &draft_path(canonical)).await,
        draft,
        "failed transaction cannot replace or retire the draft"
    );
    assert_eq!(
        current(f, "dreams/state.md").await,
        state,
        "failed transaction cannot dispose inputs, routes, or Review work"
    );
    assert_eq!(current(f, &format!("dreams/runs/{}.md", date())).await, run);
    rejected.body
}

fn submission(admission: &Value, candidate: Value, draft_pointer: Option<Value>) -> Value {
    let mut body = research_request(admission);
    body["research_progress"] = json!({
        "notes":"The canonical project identity was checked; the uncited detector lead remains unresolved.",
        "reviewed_sources":candidate["sources"],"pending_queries":[],"pending_targets":[],"status":"waiting"
    });
    body["candidates"] = json!([candidate]);
    body["processed_inputs"] = json!([]);
    body["findings"] = json!(["The bounded overview preserves the checked project identity."]);
    if let Some(pointer) = draft_pointer {
        body["draft_protocol"] = json!(PROTOCOL);
        body["draft_pointer"] = pointer;
    }
    body
}

async fn reload(f: &mut Fixture) {
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

async fn assert_raw_reads_withheld(f: &Fixture, canonical: &Value, pointers: &[Value]) {
    let mut requests = vec![json!({"path":draft_path(canonical),"view":"full"})];
    for pointer in pointers {
        requests
            .push(json!({"ref":pointer["entry_ref"],"version":pointer["version"],"view":"full"}));
        requests
            .push(json!({"path":draft_path(canonical),"version":pointer["version"],"view":"full"}));
    }
    for actor in [&f.model, &f.owner] {
        let read = ok(post(f, actor, "/v1/workspace/read", json!({"requests":requests})).await);
        assert!(
            !read.to_string().contains(MARKER),
            "raw current/historical draft text must stay protected"
        );
        assert!(!read.to_string().contains("draft_candidate"));
    }
}

#[tokio::test]
async fn draft_one_pass_custody_survives_stale_submit_restart_and_exact_replay() {
    for change in ["changed", "new"] {
        let Some(mut f) = fixture().await else { return };
        let s = scenario(&f).await;
        assert_eq!(s.admission["research"]["notes"], "");
        assert!(
            s.admission["research"]["current_checkpoint"].is_null(),
            "one-pass custody cannot depend on accepted notebook prose"
        );
        let candidate = proposal(&s.admission, &s.canonical, "original");
        let operation = custody(&s.admission, candidate.clone(), None);
        let before = current(&f, "dreams/state.md").await.unwrap();
        let before_job = current(&f, &job_path(&s.canonical)).await.unwrap();
        let saved = save(&f, operation.clone()).await;
        let ptr = pointer(&saved);
        assert_draft(&saved, &candidate, &ptr);
        assert_receipt(&saved, &operation, &ptr, false, false);
        assert_eq!(
            draft(&saved)["origin"]["version"],
            s.admission["research"]["version"]
        );
        assert_eq!(
            draft(&saved)["origin"]["snapshot_generation"],
            s.admission["research"]["snapshot_generation"]
        );
        assert_eq!(saved["research"]["notes"], "");
        assert_eq!(saved["research"]["round"], s.admission["research"]["round"]);
        assert_eq!(
            saved["research"]["version"],
            s.admission["research"]["version"]
        );
        assert_eq!(saved["state_version"], s.admission["state_version"]);
        assert_eq!(
            current(&f, &job_path(&s.canonical)).await.unwrap(),
            before_job
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
        assert_no_work_credit(&before.2, &current(&f, "dreams/state.md").await.unwrap().2);
        assert!(review(&f).await["items"].as_array().unwrap().is_empty());

        let changed = if change == "changed" {
            write(
                &f,
                s.support["path"].as_str().unwrap(),
                "# Detector\n\nThe calibrated detector now has an updated exposure interval.\n",
                1,
            )
            .await
        } else {
            write(
                &f,
                "sources/Projects/Prism/New-observation.md",
                "# Prism observation\n\nPrism has a newly recorded calibration observation.\n",
                0,
            )
            .await
        };
        // Post-cutoff changes are invisible to this pass; an unrelated stale
        // notebook version still rejects the submission without retiring custody.
        let mut stale = submission(&saved, candidate.clone(), Some(ptr.clone()));
        stale["research_version"] = json!(0);
        rejected_unchanged(&f, &s.canonical, CANDIDATES, stale).await;
        let durable = current(&f, &draft_path(&s.canonical)).await.unwrap();
        reload(&mut f).await;
        let job = current(&f, &job_path(&s.canonical)).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let replay = ok(post(&f, &f.runner, PROGRESS, operation.clone()).await);
        assert_eq!(replay["no_op"], true);
        assert_receipt(&replay["data"], &operation, &ptr, true, false);
        assert_draft(&replay["data"], &candidate, &ptr);
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            durable
        );
        assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), job);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);

        let (_, refreshed) = discover_subject(&f, &replay["data"], vec![]).await;
        assert_draft(&refreshed, &candidate, &ptr);
        let delta = &draft(&refreshed)["source_delta"];
        assert_eq!(
            delta["coverage_complete"], true,
            "bounded fixture refresh completes: {refreshed}"
        );
        assert!(
            !delta[change]
                .to_string()
                .contains(changed["entry_ref"].as_str().unwrap()),
            "post-cutoff {change} waits for the next pass: {delta}"
        );
        assert_eq!(refreshed["inputs"], saved["inputs"]);
        assert_eq!(
            refreshed["processed_generation"],
            saved["processed_generation"]
        );
        assert!(review(&f).await["items"].as_array().unwrap().is_empty());
        let revised = proposal(&refreshed, &s.canonical, "revalidated scope");
        let retained = save(&f, custody(&refreshed, revised.clone(), Some(ptr.clone()))).await;
        let revised_pointer = pointer(&retained);
        let accepted = ok(post(
            &f,
            &f.runner,
            CANDIDATES,
            submission(&retained, revised, Some(revised_pointer.clone())),
        )
        .await);
        assert_eq!(
            accepted["accepted_candidate_ids"].as_array().unwrap().len(),
            1
        );
        assert!(draft(&accepted).is_null());
        assert_eq!(accepted["draft_receipt"]["retired"], true);
        assert_raw_reads_withheld(&f, &s.canonical, &[ptr, revised_pointer]).await;
        f.pool.close().await;
    }
}

#[tokio::test]
async fn draft_custody_and_submission_use_pinned_evidence_while_newer_versions_wait() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let candidate = proposal(&s.admission, &s.canonical, "already stale");
    let newer = write(
        &f,
        s.support["path"].as_str().unwrap(),
        "# Detector\n\nThe detector observation changed before custody could be recorded.\n",
        1,
    )
    .await;
    let job = current(&f, &job_path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let saved = save(&f, custody(&s.admission, candidate.clone(), None)).await;
    let ptr = pointer(&saved);
    assert_draft(&saved, &candidate, &ptr);
    assert_eq!(draft(&saved)["source_delta"]["coverage_complete"], true);
    assert_eq!(draft(&saved)["source_delta"]["changed"], json!([]));
    assert_eq!(
        newer["version"], 2,
        "the newer version waits for the next pass"
    );
    assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), job);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    let accepted = ok(post(
        &f,
        &f.runner,
        CANDIDATES,
        submission(&saved, candidate.clone(), Some(ptr.clone())),
    )
    .await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    assert_eq!(accepted["draft_receipt"]["retired"], true);
    assert!(draft(&accepted).is_null());
    f.pool.close().await;
}

#[tokio::test]
async fn draft_source_delta_is_bounded_and_never_claims_complete_truncated_coverage() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let candidate = proposal(&s.admission, &s.canonical, "bounded delta");
    let saved = save(&f, custody(&s.admission, candidate.clone(), None)).await;
    let ptr = pointer(&saved);
    for index in 0..18 {
        write(
            &f,
            &format!("sources/Projects/Prism/Observation-{index}.md"),
            &format!(
                "# Prism observation {index}\n\nPrism records calibration observation {index}.\n"
            ),
            0,
        )
        .await;
    }
    // Leads written after the cutoff join the next pass, where the retained
    // draft's delta is computed against the re-pinned evidence.
    yield_and_finish(&f, &saved).await;
    let refreshed = next_subject(
        &f,
        &admit_requested(&f, vec![s.canonical["entry_ref"].clone()]).await,
    )
    .await;
    assert_draft(&refreshed, &candidate, &ptr);
    assert_ne!(
        refreshed["research"]["needs_refresh"], true,
        "this bounded fixture completes source discovery"
    );
    let delta = &draft(&refreshed)["source_delta"];
    let count = ["changed", "new", "missing"]
        .iter()
        .map(|field| delta[*field].as_array().unwrap().len())
        .sum::<usize>();
    assert_eq!(count, 16);
    assert_eq!(delta["truncated"], true);
    assert_eq!(delta["coverage_complete"], false);
    assert!(review(&f).await["items"].as_array().unwrap().is_empty());
    f.pool.close().await;
}

#[tokio::test]
async fn draft_replacement_requires_exact_offered_identity_and_preserves_immutable_versions() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let first = proposal(&s.admission, &s.canonical, "first");
    let first_op = custody(&s.admission, first.clone(), None);
    let a = save(&f, first_op.clone()).await;
    let a_ptr = pointer(&a);
    let original = current(&f, &draft_path(&s.canonical)).await.unwrap();
    let replacement = proposal(&a, &s.canonical, "reconsidered");

    let job = current(&f, &job_path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let identical_op = custody(&a, first.clone(), None);
    let identical = save(&f, identical_op.clone()).await;
    assert_draft(&identical, &first, &a_ptr);
    assert_eq!(
        draft(&identical)["origin"],
        draft(&a)["origin"],
        "a new receipt cannot rebase old draft authority"
    );
    assert_receipt(&identical, &identical_op, &a_ptr, false, false);
    assert_eq!(current(&f, &job_path(&s.canonical)).await.unwrap(), job);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);

    rejected_unchanged(
        &f,
        &s.canonical,
        PROGRESS,
        custody(&a, replacement.clone(), None),
    )
    .await;
    for field in ["entry_ref", "version", "candidate_hash"] {
        let mut wrong = a_ptr.clone();
        wrong[field] = match field {
            "entry_ref" => json!(format!("entry:{}", Uuid::now_v7())),
            "version" => json!(a_ptr["version"].as_i64().unwrap() + 1),
            _ => json!("0".repeat(64)),
        };
        rejected_unchanged(
            &f,
            &s.canonical,
            PROGRESS,
            custody(&a, replacement.clone(), Some(wrong)),
        )
        .await;
    }
    let before = current(&f, "dreams/state.md").await.unwrap();
    let mut second_op = custody(&a, replacement.clone(), Some(a_ptr.clone()));
    // A boilerplate sentence is not a retention check. Exact wrapper-owned
    // identity is still mandatory, and the earlier draft version remains.
    second_op["findings"] = json!([]);
    let b = save(&f, second_op.clone()).await;
    let b_ptr = pointer(&b);
    assert_eq!(
        b_ptr["entry_ref"], a_ptr["entry_ref"],
        "one fixed draft entry per subject"
    );
    assert!(b_ptr["version"].as_i64().unwrap() > a_ptr["version"].as_i64().unwrap());
    assert_ne!(b_ptr["candidate_hash"], a_ptr["candidate_hash"]);
    assert_draft(&b, &replacement, &b_ptr);
    assert_receipt(&b, &second_op, &b_ptr, false, false);
    assert_no_work_credit(&before.2, &current(&f, "dreams/state.md").await.unwrap().2);
    let historical: (String, Value) = sqlx::query_as("SELECT content,metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
        .bind(f.owner.user).bind(entry_uuid(&a_ptr)).bind(a_ptr["version"].as_i64().unwrap()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(historical, (original.1, original.2));
    assert_eq!(
        historical.1["dreamer_review"]["schema"], PROTOCOL,
        "older raw readers fail closed on the protected unknown audit schema"
    );

    let accepted_draft = current(&f, &draft_path(&s.canonical)).await.unwrap();
    let accepted_state = current(&f, "dreams/state.md").await.unwrap();
    for (operation, ptr) in [
        (&first_op, &a_ptr),
        (&identical_op, &a_ptr),
        (&second_op, &b_ptr),
    ] {
        let replay = ok(post(&f, &f.runner, PROGRESS, operation.clone()).await);
        assert_eq!(replay["no_op"], true);
        assert_receipt(&replay["data"], operation, ptr, true, false);
        assert_draft(&replay["data"], &replacement, &b_ptr);
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            accepted_draft
        );
        assert_eq!(
            current(&f, "dreams/state.md").await.unwrap(),
            accepted_state
        );
    }
    let mut conflicting_replay = first_op;
    conflicting_replay["draft_candidate"] = replacement.clone();
    rejected_unchanged(&f, &s.canonical, PROGRESS, conflicting_replay).await;
    rejected_unchanged(
        &f,
        &s.canonical,
        PROGRESS,
        custody(&b, first, Some(a_ptr.clone())),
    )
    .await;
    assert_raw_reads_withheld(&f, &s.canonical, &[a_ptr, b_ptr]).await;
    f.pool.close().await;
}

#[tokio::test]
async fn draft_bounds_unadmitted_authority_and_operational_fields_reject_atomically() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let candidate = proposal(&s.admission, &s.canonical, "retained");
    let saved = save(&f, custody(&s.admission, candidate.clone(), None)).await;
    let ptr = pointer(&saved);
    let base = custody(&saved, candidate.clone(), Some(ptr.clone()));
    for fault in [
        "bytes",
        "selectors",
        "foreign_source",
        "wrong_version",
        "forged_excerpt",
        "subject",
        "path",
        "snapshot",
        "origin",
        "manifest",
        "findings",
        "missing_inputs",
    ] {
        let mut invalid = base.clone();
        match fault {
            "bytes" => invalid["draft_candidate"]["content"] = json!("x".repeat(32 * 1024 + 1)),
            "selectors" => {
                invalid["draft_candidate"]["sources"] = json!(vec![reviewed(&s.canonical); 65])
            }
            "foreign_source" => {
                invalid["draft_candidate"]["sources"] = json!([reviewed(&s.unadmitted)])
            }
            "wrong_version" => invalid["draft_candidate"]["sources"][0]["version"] = json!(99999),
            "forged_excerpt" => {
                invalid["draft_candidate"]["sources"][0]["excerpt"] =
                    json!("CALLER_PRIMARY_TEXT_MUST_NOT_BE_STORED")
            }
            "subject" => {
                invalid["draft_candidate"]["subject_ref"] = s.unadmitted["entry_ref"].clone()
            }
            "path" => {
                invalid["draft_candidate"]["path"] = json!("derived/entities/other-subject.md")
            }
            "snapshot" => invalid["snapshot_generation"] = json!(99999),
            "origin" => {
                invalid["origin"] = json!({"entry_ref":s.unadmitted["entry_ref"],"version":1,"snapshot_generation":1})
            }
            "manifest" => invalid["sources"] = json!([reviewed(&s.canonical)]),
            "findings" => invalid["findings"] = json!(["x".repeat(16_001)]),
            "missing_inputs" => {
                invalid.as_object_mut().unwrap().remove("processed_inputs");
            }
            _ => unreachable!(),
        }
        rejected_unchanged(&f, &s.canonical, PROGRESS, invalid).await;
    }
    for (field, value) in [
        (
            "notes",
            json!("Draft custody must not accept factual notes."),
        ),
        ("reviewed_sources", json!([reviewed(&s.canonical)])),
        ("status", json!("no_change")),
        ("processed_inputs", saved["inputs"].clone()),
        ("reconciled_checkpoints", json!([])),
        ("resolved_follow_ups", json!([])),
        ("candidates", json!([candidate])),
        (
            "repair_feedback",
            json!({"phase":"candidate_validation","message":"Not draft custody."}),
        ),
    ] {
        let mut invalid = base.clone();
        invalid[field] = value;
        rejected_unchanged(&f, &s.canonical, PROGRESS, invalid).await;
    }
    for field in [
        "expected_state_version",
        "research_version",
        "fence",
        "attempt_id",
    ] {
        let mut invalid = base.clone();
        invalid[field] = if field.ends_with("version") {
            json!(-1)
        } else {
            json!(Uuid::now_v7())
        };
        rejected_unchanged(&f, &s.canonical, PROGRESS, invalid).await;
    }
    let model_write = post(&f, &f.model, PROGRESS, base.clone()).await;
    assert!(model_write.status.is_client_error());
    let forged_raw = post(&f, &f.owner, "/v1/workspace/write", json!({"path":draft_path(&s.canonical),"expected_version":ptr["version"],"content":"FORGED_DRAFT","metadata":{}})).await;
    assert!(forged_raw.status.is_client_error());
    expire_fixture_lease(&f).await;
    rejected_unchanged(&f, &s.canonical, PROGRESS, base).await;
    f.pool.close().await;
}

#[tokio::test]
async fn draft_only_matching_accepted_candidate_retires_and_legacy_omission_preserves_it() {
    for attach in [false, true] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let mut candidate = proposal(&s.admission, &s.canonical, "ready");
        if !attach {
            // Questions legitimately reach exact hash dedup without requiring
            // a new summary revision identity for the same destination.
            candidate["kind"] = json!("question");
            candidate["question"] = json!("Which optical observation should be reviewed next?");
        }
        let custody_op = custody(&s.admission, candidate.clone(), None);
        let saved = save(&f, custody_op.clone()).await;
        let ptr = pointer(&saved);
        let durable = current(&f, &draft_path(&s.canonical)).await.unwrap();
        let mut zero = research_request(&saved);
        zero["draft_protocol"] = json!(PROTOCOL);
        zero["draft_pointer"] = ptr.clone();
        zero["candidates"] = json!([]);
        zero["processed_inputs"] = json!([]);
        rejected_unchanged(&f, &s.canonical, CANDIDATES, zero).await;

        let mut wrong = submission(
            &saved,
            proposal(&saved, &s.canonical, "different body"),
            Some(ptr.clone()),
        );
        rejected_unchanged(&f, &s.canonical, CANDIDATES, wrong.clone()).await;
        wrong["candidates"] = json!([candidate.clone()]);
        wrong["draft_pointer"]["candidate_hash"] = json!("0".repeat(64));
        rejected_unchanged(&f, &s.canonical, CANDIDATES, wrong).await;

        let submit = submission(&saved, candidate.clone(), attach.then(|| ptr.clone()));
        let accepted = ok(post(&f, &f.runner, CANDIDATES, submit.clone()).await);
        assert_eq!(
            accepted["accepted_candidate_ids"].as_array().unwrap().len(),
            1
        );
        assert_eq!(accepted["inputs"], saved["inputs"]);
        if attach {
            assert!(draft(&accepted).is_null());
            assert_receipt(&accepted, &submit, &ptr, false, true);
            assert!(current(&f, &draft_path(&s.canonical)).await.unwrap().0 > durable.0);
        } else {
            assert_draft(&accepted, &candidate, &ptr);
            assert!(accepted.get("draft_receipt").is_none_or(Value::is_null));
            assert_eq!(
                current(&f, &draft_path(&s.canonical)).await.unwrap(),
                durable
            );
            // Normal candidate deduplication can return zero accepted IDs even
            // with an exact available draft; it must not count as retirement.
            let duplicate_op = submission(&accepted, candidate.clone(), Some(ptr.clone()));
            let duplicate = ok(post(&f, &f.runner, CANDIDATES, duplicate_op.clone()).await);
            assert_eq!(duplicate["accepted_candidate_ids"], json!([]));
            assert_receipt(&duplicate, &duplicate_op, &ptr, false, false);
            assert_draft(&duplicate, &candidate, &ptr);
            assert_eq!(
                current(&f, &draft_path(&s.canonical)).await.unwrap(),
                durable
            );
        }
        let retired = current(&f, &draft_path(&s.canonical)).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let replay = ok(post(&f, &f.runner, CANDIDATES, submit.clone()).await);
        assert_eq!(
            replay["accepted_candidate_ids"],
            accepted["accepted_candidate_ids"]
        );
        if attach {
            assert_receipt(&replay, &submit, &ptr, true, true);
            assert!(draft(&replay).is_null());
        } else {
            assert_draft(&replay, &candidate, &ptr);
        }
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            retired
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let old_save = ok(post(&f, &f.runner, PROGRESS, custody_op).await);
        if attach {
            assert!(
                draft(&old_save["data"]).is_null(),
                "old custody replay cannot resurrect an accepted draft"
            );
        }
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            retired
        );
        assert_raw_reads_withheld(&f, &s.canonical, &[ptr]).await;
        f.pool.close().await;
    }
}

async fn exclude_source(f: &Fixture, source: &Value, loss: &str) {
    match loss {
        "deleted" => {
            ok(request(
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
            .await);
        }
        "generated" => {
            ok(post(f, &f.owner, "/v1/workspace/write", json!({"path":source["path"],"expected_version":source["version"],"content":"# Detector\n\nA generated edition occupies this identity.\n","metadata":{"kind":"briefing_edition"}})).await);
        }
        "protected" | "sensitive" => {
            // Isolated access-loss fixture only; no production metadata bypass.
            let path = if loss == "protected" {
                format!(".brunn/tasks/{}.md", Uuid::now_v7())
            } else {
                "sources/Credentials/Detector.md".to_owned()
            };
            sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
                .bind(f.owner.user)
                .bind(entry_uuid(source))
                .bind(path)
                .execute(&f.pool)
                .await
                .unwrap();
        }
        "original_generated" => {
            write(
                f,
                source["path"].as_str().unwrap(),
                "# Detector\n\nA currently eligible detector observation.\n",
                source["version"].as_i64().unwrap(),
            )
            .await;
            // The exact old authority is disallowed even though its current head is eligible.
            sqlx::query("UPDATE brunn.entry_versions SET metadata=metadata||'{\"kind\":\"briefing_edition\"}'::jsonb WHERE user_id=$1 AND entry_id=$2 AND version=$3")
                .bind(f.owner.user).bind(entry_uuid(source)).bind(source["version"].as_i64().unwrap()).execute(&f.pool).await.unwrap();
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn draft_uncited_original_authority_is_audited_on_replay_and_after_live_pruning() {
    for loss in [
        "deleted",
        "generated",
        "protected",
        "sensitive",
        "original_generated",
    ] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let candidate = proposal(&s.admission, &s.canonical, "uncited authority");
        assert!(
            !candidate["sources"]
                .to_string()
                .contains(s.support["entry_ref"].as_str().unwrap())
        );
        let operation = custody(&s.admission, candidate.clone(), None);
        let saved = save(&f, operation.clone()).await;
        let ptr = pointer(&saved);
        let original = current(&f, &draft_path(&s.canonical)).await.unwrap();
        exclude_source(&f, &s.support, loss).await;
        let before = current(&f, "dreams/state.md").await.unwrap();
        let replay = ok(post(&f, &f.runner, PROGRESS, operation.clone()).await);
        assert_eq!(replay["no_op"], true);
        assert_eq!(
            draft(&replay["data"]),
            &json!({"status":"unavailable"}),
            "{loss}: {replay}"
        );
        assert!(!replay.to_string().contains(MARKER), "{loss}");
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            original
        );
        let (_, refreshed) = discover_subject(&f, &replay["data"], vec![]).await;
        assert_eq!(
            draft(&refreshed),
            &json!({"status":"unavailable"}),
            "{loss}: {refreshed}"
        );
        if loss != "original_generated" {
            assert!(
                !refreshed["research"]["sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|source| source["entry_ref"] == s.support["entry_ref"])
            );
        }
        let replay = ok(post(&f, &f.runner, PROGRESS, operation).await);
        assert_eq!(draft(&replay["data"]), &json!({"status":"unavailable"}));
        assert!(!replay.to_string().contains(MARKER));
        let guessed = custody(
            &refreshed,
            proposal(&refreshed, &s.canonical, "guessed replacement"),
            Some(ptr.clone()),
        );
        rejected_unchanged(&f, &s.canonical, PROGRESS, guessed).await;
        if matches!(loss, "deleted" | "generated") {
            let mut independent =
                proposal(&refreshed, &s.canonical, "current independent overview");
            independent["title"] = json!("Independently checked current survey identity");
            let accepted = ok(post(
                &f,
                &f.runner,
                CANDIDATES,
                submission(&refreshed, independent, None),
            )
            .await);
            assert_eq!(
                accepted["accepted_candidate_ids"].as_array().unwrap().len(),
                1
            );
            assert_eq!(
                draft(&accepted),
                &json!({"status":"unavailable"}),
                "an inaccessible historical draft does not block an ordinary current candidate"
            );
        }
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            original
        );
        assert_raw_reads_withheld(&f, &s.canonical, &[ptr]).await;
        f.pool.close().await;
    }
}

#[tokio::test]
async fn draft_freezes_full_historical_authority_even_after_notebook_reconciliation() {
    for loss in ["deleted", "generated"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let mut admission = s.admission.clone();
        for notes in [
            "Earlier survey work retains the uncited detector lead.",
            "Current survey identity is independently checked.",
        ] {
            let mut progress = progress_body(
                &admission,
                vec![reviewed(&s.canonical)],
                "researching",
                notes,
            );
            progress["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
            progress["reconciled_checkpoints"] = json!([]);
            admission = save(&f, progress).await;
        }
        assert_eq!(
            admission["research"]["checkpoint_contexts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // Isolate a historical-only dependency. Its immutable original notebook
        // keeps the complete source manifest, while the active notebook omits it.
        let mut metadata = current(&f, &job_path(&s.canonical)).await.unwrap().2;
        metadata["dreamer_research"]["sources"]
            .as_array_mut()
            .unwrap()
            .retain(|source| source["entry_ref"] == s.canonical["entry_ref"]);
        sqlx::query("UPDATE brunn.entry_versions v SET metadata=$3 FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
            .bind(f.owner.user).bind(job_path(&s.canonical)).bind(metadata).execute(&f.pool).await.unwrap();
        let candidate = proposal(&admission, &s.canonical, "historical authority");
        let operation = custody(&admission, candidate.clone(), None);
        let saved = save(&f, operation.clone()).await;
        let ptr = pointer(&saved);
        assert_draft(&saved, &candidate, &ptr);
        assert_eq!(saved["research"]["sources"].as_array().unwrap().len(), 1);

        let mut origins = saved["research"]["checkpoint_contexts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["origin"].clone())
            .collect::<Vec<_>>();
        if saved["research"]["current_checkpoint"].is_object() {
            origins.push(saved["research"]["current_checkpoint"].clone());
        }
        let mut reconcile = progress_body(
            &saved,
            vec![reviewed(&s.canonical)],
            "researching",
            "The canonical survey identity is retained; the detector lead is explicitly outside this bounded overview.",
        );
        reconcile["checkpoint_protocol"] = json!("dream.research.checkpoint.v1");
        reconcile["reconciled_checkpoints"] = json!(origins);
        reconcile["findings"] = json!([
            "All offered historical observations were reconsidered; the current draft retains the canonical survey identity and excludes the unresolved detector lead."
        ]);
        let reconciled = save(&f, reconcile).await;
        assert!(
            reconciled["research"]["checkpoint_contexts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_draft(&reconciled, &candidate, &ptr);
        let original = current(&f, &draft_path(&s.canonical)).await.unwrap();
        exclude_source(&f, &s.support, loss).await;
        let replay = ok(post(&f, &f.runner, PROGRESS, operation).await);
        assert_eq!(
            draft(&replay["data"]),
            &json!({"status":"unavailable"}),
            "retiring notebook history cannot prune a draft's original authority: {loss}"
        );
        assert!(!replay.to_string().contains(MARKER));
        assert_eq!(
            current(&f, &draft_path(&s.canonical)).await.unwrap(),
            original
        );
        assert_raw_reads_withheld(&f, &s.canonical, &[ptr]).await;
        f.pool.close().await;
    }
}

#[tokio::test]
async fn draft_custody_cannot_bypass_destination_cas_or_owner_holds() {
    for owner_choice in ["destination_changed", "reject", "defer", "approve"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let candidate = proposal(&s.admission, &s.canonical, "owner guarded");
        let mut saved = save(&f, custody(&s.admission, candidate.clone(), None)).await;
        let mut ptr = pointer(&saved);
        if owner_choice == "destination_changed" {
            // Fixture-only destination race: ordinary writes correctly cannot
            // create Dreamer artifacts. Move an existing, uncited fixture row
            // to simulate an output appearing after the version-0 draft froze.
            let seeded = sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
                .bind(f.owner.user)
                .bind(entry_uuid(&s.unadmitted))
                .bind(candidate["path"].as_str().unwrap())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(seeded.rows_affected(), 1);
            let rejected = rejected_unchanged(
                &f,
                &s.canonical,
                CANDIDATES,
                submission(&saved, candidate, Some(ptr.clone())),
            )
            .await;
            assert_eq!(rejected["error"]["code"], "dreamer_output_changed");
        } else {
            // Legacy acceptance deliberately leaves draft custody untouched.
            let accepted = ok(post(
                &f,
                &f.runner,
                CANDIDATES,
                submission(&saved, candidate.clone(), None),
            )
            .await);
            let mut revision = candidate;
            revision["revises_item_id"] = accepted["accepted_candidate_ids"][0].clone();
            saved = save(&f, custody(&accepted, revision.clone(), Some(ptr))).await;
            ptr = pointer(&saved);
            let view = review(&f).await;
            let item = view["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == accepted["accepted_candidate_ids"][0])
                .unwrap();
            ok(post(
                &f,
                &f.owner,
                "/v1/dreamer/review/decisions",
                decision(&view, item, owner_choice),
            )
            .await);
            let mut operation = submission(&saved, revision, Some(ptr.clone()));
            // Use the live state CAS to ensure this gate is the owner decision,
            // not merely a stale state version left by that decision.
            operation["expected_state_version"] =
                json!(current(&f, "dreams/state.md").await.unwrap().0);
            rejected_unchanged(&f, &s.canonical, CANDIDATES, operation).await;
        }
        let retained = current(&f, &draft_path(&s.canonical)).await.unwrap();
        assert_eq!(retained.0, ptr["version"].as_i64().unwrap());
        f.pool.close().await;
    }
}

#[tokio::test]
async fn draft_contract_failures_are_repairable_only_after_valid_fences() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let proposed = proposal(&s.admission, &s.canonical, "bounded repair");
    let corrected = custody(&s.admission, proposed.clone(), None);
    let job = current(&f, &job_path(&s.canonical)).await;
    let state = current(&f, "dreams/state.md").await;
    for fault in ["findings", "candidate_bytes"] {
        let mut invalid = corrected.clone();
        invalid["operation_id"] = json!(Uuid::now_v7());
        if fault == "findings" {
            invalid["findings"] = json!(vec!["f".repeat(2_000); 9]);
        } else {
            invalid["draft_candidate"]["content"] = json!("c".repeat(32 * 1024));
        }
        let response = post(&f, &f.runner, PROGRESS, invalid.clone()).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{}",
            response.body
        );
        assert_eq!(response.body["error"]["code"], "invalid_request");
        assert_eq!(
            response.body["error"]["details"]["dreamer_repair_phase"],
            "checkpoint_validation"
        );
        assert_eq!(current(&f, &job_path(&s.canonical)).await, job);
        assert_eq!(current(&f, "dreams/state.md").await, state);
        assert!(current(&f, &draft_path(&s.canonical)).await.is_none());

        // The same bad model payload cannot relabel a stale fence as repairable.
        invalid["fence"] = json!(Uuid::now_v7());
        let fenced = post(&f, &f.runner, PROGRESS, invalid).await;
        assert!(fenced.status.is_client_error());
        assert!(
            fenced.body["error"]["details"]
                .get("dreamer_repair_phase")
                .is_none()
        );
    }
    let saved = save(&f, corrected.clone()).await;
    let ptr = pointer(&saved);
    assert_receipt(&saved, &corrected, &ptr, false, false);
    assert_draft(&saved, &proposed, &ptr);
    assert_eq!(current(&f, &job_path(&s.canonical)).await, job);
    assert_eq!(current(&f, "dreams/state.md").await, state);
    f.pool.close().await;
}

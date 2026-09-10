//! Synthetic HTTP/database gates for bounded immutable checkpoint custody.
use super::*;

const PROTOCOL: &str = "dream.research.checkpoint.v1";
const PROGRESS: &str = "/v1/workspace/dreamer/research-progress";
const CANDIDATES: &str = "/v1/workspace/dreamer/candidates";

struct Scenario {
    canonical: Value,
    support: Value,
    secondary: Value,
    admission: Value,
}

fn path(source: &Value) -> String {
    format!(
        "dreams/research/{}.md",
        source["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    )
}

async fn scenario(f: &Fixture) -> Scenario {
    control(f, "report-only", 0).await;
    let canonical = write(
        f,
        "sources/People/Juniper.md",
        "# Juniper\n\nJuniper coordinates an optical survey.\n",
        0,
    )
    .await;
    let support = write(f, "sources/Optics/Calibration.md", "# Calibration\n\nThe optical survey uses a calibrated detector.\n\nAn independent exposure check remains open.\n", 0).await;
    let secondary = write(
        f,
        "sources/People/Willow.md",
        "# Willow\n\nWillow coordinates an independent survey.\n",
        0,
    )
    .await;
    let admission = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({
            "attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60,
            "requested_subject_refs":[canonical["entry_ref"],secondary["entry_ref"]]
        }),
    )
    .await);
    let selected = next_subject(f, &admission).await;
    assert_eq!(selected["research"]["subject_ref"], canonical["entry_ref"]);
    assert_eq!(selected["research"]["checkpoint_protocol"], PROTOCOL);
    let (_, admission) = discover_subject(f, &selected, vec![support["entry_ref"].clone()]).await;
    Scenario {
        canonical,
        support,
        secondary,
        admission,
    }
}

fn body(admission: &Value, selectors: Vec<Value>, notes: &str, reconciled: Vec<Value>) -> Value {
    let mut body = progress_body(admission, selectors, "researching", notes);
    body["checkpoint_protocol"] = json!(PROTOCOL);
    body["reconciled_checkpoints"] = json!(reconciled);
    body["findings"] = json!([
        "The named checkpoints' useful conclusions and unfinished leads were incorporated or explicitly reconsidered against the reviewed primary evidence."
    ]);
    body["processed_inputs"] = json!([]);
    body
}

async fn save(f: &Fixture, operation: Value) -> Value {
    ok(post(f, &f.runner, PROGRESS, operation).await)["data"].clone()
}

async fn save_access_phase(
    f: &Fixture,
    operation: Value,
    loss: &str,
    phase: &str,
    prior: &Value,
) -> Value {
    let response = post(f, &f.runner, PROGRESS, operation).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "loss={loss}, phase={phase}, prior_needs_refresh={}, prior_coverage={}, error={}",
        prior["research"]["needs_refresh"],
        prior["research"]["coverage"],
        response.body
    );
    response.body["data"].clone()
}

fn contexts(admission: &Value) -> &Vec<Value> {
    admission["research"]["checkpoint_contexts"]
        .as_array()
        .expect("audited checkpoint array")
}

fn origin(admission: &Value) -> Value {
    let value = admission["research"]["current_checkpoint"].clone();
    assert!(
        value.is_object(),
        "accepted current notebook has an exact origin: {admission}"
    );
    assert_eq!(value["version"], admission["research"]["version"]);
    assert_eq!(
        value["snapshot_generation"],
        admission["research"]["snapshot_generation"]
    );
    assert!(value["entry_ref"].as_str().unwrap().starts_with("entry:"));
    value
}

fn offered(admission: &Value) -> Vec<Value> {
    let mut offered: Vec<Value> = contexts(admission)
        .iter()
        .map(|context| context["origin"].clone())
        .collect();
    if admission["research"]["current_checkpoint"].is_object() {
        offered.push(admission["research"]["current_checkpoint"].clone());
    }
    offered
}

fn assert_contexts(admission: &Value, expected: &[(&Value, &str)]) {
    assert_eq!(
        admission["research"]["checkpoint_context_status"], "available",
        "{admission}"
    );
    assert_eq!(contexts(admission).len(), expected.len());
    for (origin, notes) in expected {
        let found = contexts(admission)
            .iter()
            .find(|context| &context["origin"] == *origin)
            .expect("every immutable origin retained exactly once");
        assert_eq!(found["status"], "historical_revalidation_only");
        assert_eq!(found["notes"], *notes);
        for source in found["prior_reviewed_sources"].as_array().unwrap() {
            assert!(source.get("excerpt").is_none());
            assert!(source.get("path").is_none());
        }
    }
    assert!(
        serde_json::to_vec(&admission["research"]["checkpoint_contexts"])
            .unwrap()
            .len()
            <= 96 * 1024
    );
}

fn assert_receipt(
    admission: &Value,
    operation: &Value,
    replayed: bool,
    coverage: bool,
    reconciled: bool,
    complete: bool,
) {
    assert_eq!(
        admission["checkpoint_receipt"],
        json!({
            "operation_id":operation["operation_id"],"protocol":PROTOCOL,"recorded":true,
            "replayed":replayed,"new_source_coverage":coverage,"reconciled":reconciled,
            "subject_complete":complete
        })
    );
}

async fn rejected_unchanged(f: &Fixture, canonical: &Value, endpoint: &str, operation: Value) {
    let before = current(f, &path(canonical)).await.unwrap();
    let state = current(f, "dreams/state.md").await.unwrap();
    let response = post(f, &f.runner, endpoint, operation).await;
    assert!(response.status.is_client_error(), "{}", response.body);
    assert_eq!(
        current(f, &path(canonical)).await.unwrap(),
        before,
        "failed operation must not save notes, custody, or receipt"
    );
    assert_eq!(
        current(f, "dreams/state.md").await.unwrap(),
        state,
        "failed operation must not change input, cursor, candidate, or scheduler state"
    );
}

async fn drift(f: &Fixture, source: &Value) -> Value {
    let version = source["version"].as_i64().unwrap();
    write(f, source["path"].as_str().unwrap(), &format!("# Calibration\n\nThe optical survey uses detector calibration revision {}.\n\nThe independent exposure check remains open.\n", version + 1), version).await
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

async fn queued(f: &Fixture, canonical: &Value) -> bool {
    current(f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["requested_subject_refs"].as_array().unwrap().contains(&canonical["entry_ref"])
}

#[tokio::test]
async fn checkpoint_preserves_independent_units_across_repeated_refresh_reload_and_replay() {
    let Some(mut f) = fixture().await else { return };
    let s = scenario(&f).await;
    let a_body = body(
        &s.admission,
        vec![reviewed(&s.canonical)],
        "UNIT_A: canonical identity checked; detector calibration remains open.",
        vec![],
    );
    let a = save(&f, a_body.clone()).await;
    let a_origin = origin(&a);
    assert_receipt(&a, &a_body, false, true, false, false);
    let inputs = a["inputs"].clone();
    let b_body = body(
        &a,
        vec![reviewed(&s.support)],
        "UNIT_B: detector calibration checked; exposure interval remains open.",
        vec![],
    );
    let b = save(&f, b_body.clone()).await;
    let b_origin = origin(&b);
    assert_contexts(&b, &[(&a_origin, a_body["notes"].as_str().unwrap())]);
    assert_eq!(
        b["inputs"], inputs,
        "checkpoint is never an input disposition"
    );
    let newer = drift(&f, &s.support).await;
    let (_, refreshed) = discover_subject(&f, &b, vec![]).await;
    assert_contexts(
        &refreshed,
        &[
            (&a_origin, a_body["notes"].as_str().unwrap()),
            (&b_origin, b_body["notes"].as_str().unwrap()),
        ],
    );
    assert_eq!(refreshed["research"]["notes"], "");
    assert_eq!(refreshed["research"]["reviewed_sources"], json!([]));
    assert!(refreshed["research"]["current_checkpoint"].is_null());
    let c_body = body(
        &refreshed,
        vec![reviewed(&newer)],
        "UNIT_C: revised calibration checked; earlier identity and exposure conclusions still need consolidation.",
        vec![],
    );
    let c = save(&f, c_body.clone()).await;
    let c_origin = origin(&c);
    let newest = drift(&f, &newer).await;
    let mut waiting = research_request(&c);
    waiting["status"] = json!("waiting");
    waiting["processed_inputs"] = json!([]);
    let retained = save(&f, waiting).await;
    let expected = [
        (&a_origin, a_body["notes"].as_str().unwrap()),
        (&b_origin, b_body["notes"].as_str().unwrap()),
        (&c_origin, c_body["notes"].as_str().unwrap()),
    ];
    assert_contexts(&retained, &expected);
    let (_, refreshed) = discover_subject(&f, &retained, vec![]).await;
    reload(&mut f).await;
    let job = current(&f, &path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    for operation in [&a_body, &b_body, &c_body] {
        let response = ok(post(&f, &f.runner, PROGRESS, operation.clone()).await);
        assert_eq!(response["no_op"], true);
        assert_contexts(&response["data"], &expected);
        assert_receipt(&response["data"], operation, true, false, false, false);
        assert_eq!(current(&f, &path(&s.canonical)).await.unwrap(), job);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    }
    let consolidate = body(
        &refreshed,
        vec![reviewed(&s.canonical), reviewed(&newest)],
        "UNIFIED_AC: canonical identity and current calibration reconciled; the separate exposure conclusion remains open.",
        vec![a_origin.clone(), c_origin.clone()],
    );
    let merged = save(&f, consolidate.clone()).await;
    assert_contexts(&merged, &[(&b_origin, b_body["notes"].as_str().unwrap())]);
    assert_eq!(merged["checkpoint_receipt"]["reconciled"], true);
    assert_eq!(merged["inputs"], inputs);
    let exact = ok(post(&f, &f.model, "/v1/workspace/read", json!({"requests":[{"path":path(&s.canonical),"version":a_origin["version"],"view":"full"}]})).await);
    assert!(
        exact.to_string().contains("UNIT_A"),
        "v2 history remains audited and readable after its custody reference retires"
    );
}

#[tokio::test]
async fn checkpoint_exact_reconciliation_and_count_capacity_are_atomic_and_yieldable() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let mut a = s.admission.clone();
    let mut units = Vec::new();
    for n in 0..4 {
        let notes = format!(
            "COUNT_UNIT_{n}: a distinct checked observation retains its independent follow-up."
        );
        a = save(&f, body(&a, vec![reviewed(&s.canonical)], &notes, vec![])).await;
        units.push((origin(&a), notes));
    }
    assert_eq!(
        contexts(&a).len(),
        3,
        "four total slots include the current working head"
    );
    let overflow = body(
        &a,
        vec![reviewed(&s.canonical)],
        "FIFTH_UNIT_MUST_NOT_PERSIST",
        vec![],
    );
    rejected_unchanged(&f, &s.canonical, PROGRESS, overflow.clone()).await;
    for wrong in [
        json!({"entry_ref":format!("entry:{}",Uuid::now_v7()),"version":units[0].0["version"],"snapshot_generation":units[0].0["snapshot_generation"]}),
        json!({"entry_ref":units[0].0["entry_ref"],"version":99999,"snapshot_generation":units[0].0["snapshot_generation"]}),
        json!({"entry_ref":units[0].0["entry_ref"],"version":units[0].0["version"],"snapshot_generation":0}),
    ] {
        rejected_unchanged(
            &f,
            &s.canonical,
            PROGRESS,
            body(
                &a,
                vec![reviewed(&s.canonical)],
                "INVALID_IDENTITY_MUST_NOT_PERSIST",
                vec![wrong],
            ),
        )
        .await;
    }
    let mut no_finding = body(
        &a,
        vec![reviewed(&s.canonical)],
        "MISSING_FINDING_MUST_NOT_PERSIST",
        vec![units[0].0.clone()],
    );
    no_finding["findings"] = json!([]);
    rejected_unchanged(&f, &s.canonical, PROGRESS, no_finding).await;
    let mut duplicate = body(
        &a,
        vec![reviewed(&s.canonical)],
        "DUPLICATE_IDENTITY_MUST_NOT_PERSIST",
        vec![units[0].0.clone(), units[0].0.clone()],
    );
    duplicate["reconciled_checkpoints"] = json!(vec![units[0].0.clone(); 5]);
    rejected_unchanged(&f, &s.canonical, PROGRESS, duplicate).await;
    for field in ["fence", "research_version", "expected_state_version"] {
        let mut stale = body(
            &a,
            vec![reviewed(&s.canonical)],
            "STALE_FENCE_MUST_NOT_PERSIST",
            vec![units[0].0.clone()],
        );
        stale[field] = if field == "fence" {
            json!(Uuid::now_v7().to_string())
        } else {
            json!(stale[field].as_i64().unwrap() + 1)
        };
        rejected_unchanged(&f, &s.canonical, PROGRESS, stale).await;
    }
    let mut waiting = research_request(&a);
    waiting["status"] = json!("waiting");
    waiting["processed_inputs"] = json!([]);
    let waited = save(&f, waiting).await;
    assert_eq!(contexts(&waited).len(), 3);
    assert_eq!(waited["research"]["notes"], units[3].1);
    let mut repair = research_request(&waited);
    repair["status"] = json!("waiting");
    repair["processed_inputs"] = json!([]);
    repair["repair_feedback"] = json!({"phase":"checkpoint_validation","message":"Consolidate offered checkpoints before saving a fifth unit."});
    let repaired = save(&f, repair).await;
    assert_eq!(contexts(&repaired).len(), 3);
    let newer = drift(&f, &s.support).await;
    let (_, refreshed) = discover_subject(&f, &repaired, vec![]).await;
    assert_eq!(
        contexts(&refreshed).len(),
        4,
        "reserved current unit fits when refresh invalidates it"
    );
    assert!(refreshed["research"]["current_checkpoint"].is_null());
    let combined = save(&f, body(&refreshed, vec![reviewed(&s.canonical), reviewed(&newer)], "All four units were reconsidered into a current aggregate, with independent follow-up retained.", offered(&refreshed))).await;
    assert!(contexts(&combined).is_empty());
    assert_eq!(combined["checkpoint_receipt"]["reconciled"], true);
    let mut waiting = research_request(&combined);
    waiting["status"] = json!("waiting");
    let waited = save(&f, waiting).await;
    let next = next_subject(&f, &waited).await;
    assert_eq!(
        next["research"]["subject_ref"], s.secondary["entry_ref"],
        "capacity work cannot starve an independent requested subject"
    );
}

#[tokio::test]
async fn checkpoint_aggregate_byte_reservation_rejects_whole_write_and_survives_discovery() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let mut admission = s.admission.clone();
    for n in 0..2 {
        let mut operation = body(
            &admission,
            vec![reviewed(&s.canonical)],
            &format!("BYTE_UNIT_{n}:{}", "n".repeat(12 * 1024 - 12)),
            vec![],
        );
        operation["pending_targets"] = json!(
            (0..24)
                .map(|i| format!("sources/Open/{i:02}-{}.md", "t".repeat(994)))
                .collect::<Vec<_>>()
        );
        assert!(
            operation["pending_targets"]
                .as_array()
                .unwrap()
                .iter()
                .all(|target| target.as_str().unwrap().len() <= 1024)
        );
        if n == 1 {
            let mut escaped_growth_overflow = operation.clone();
            escaped_growth_overflow["operation_id"] = json!(Uuid::now_v7());
            escaped_growth_overflow["pending_targets"] = json!(
                (0..32)
                    .map(|i| format!("sources/Open/{i:02}-{}.md", "t".repeat(994)))
                    .collect::<Vec<_>>()
            );
            // Plain ASCII future-group accounting incorrectly admits this;
            // maximum encoded id/status strings require the extra reservation.
            rejected_unchanged(&f, &s.canonical, PROGRESS, escaped_growth_overflow).await;
        }
        admission = save(&f, operation).await;
    }
    let mut too_large = body(
        &admission,
        vec![reviewed(&s.canonical)],
        &"x".repeat(12 * 1024),
        vec![],
    );
    too_large["pending_targets"] = json!(
        (0..32)
            .map(|i| format!("sources/Open/{i:02}-{}.md", "t".repeat(994)))
            .collect::<Vec<_>>()
    );
    rejected_unchanged(&f, &s.canonical, PROGRESS, too_large).await;
    let (_, discovered) =
        discover_subject(&f, &admission, vec![s.support["entry_ref"].clone()]).await;
    assert_eq!(
        discovered["research"]["notes"],
        admission["research"]["notes"]
    );
    let groups = json!((0..12).map(|_| json!({"id":"\u{0001}".repeat(64),"returned":8,"query_status":"\u{0001}".repeat(64)})).collect::<Vec<_>>());
    // A disposable fixture models the maximum structurally permitted query
    // group representation, including JSON escaping. Never change source or
    // notebook identities; all historical versions retain their own manifests.
    for origin in offered(&discovered) {
        sqlx::query("UPDATE brunn.entry_versions v SET metadata=jsonb_set(v.metadata,'{dreamer_research,coverage,query_results}',$4) FROM brunn.entries e WHERE e.user_id=$1 AND e.path=$2 AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=$3")
            .bind(f.owner.user).bind(path(&s.canonical)).bind(origin["version"].as_i64().unwrap()).bind(&groups).execute(&f.pool).await.unwrap();
    }
    let _newer = drift(&f, &s.support).await;
    let (_, refreshed) = discover_subject(&f, &discovered, vec![]).await;
    assert_eq!(contexts(&refreshed).len(), 2);
    assert!(refreshed["research"]["current_checkpoint"].is_null());
    let total = serde_json::to_vec(&refreshed["research"]["checkpoint_contexts"])
        .unwrap()
        .len();
    assert!(
        total > 80 * 1024 && total <= 96 * 1024,
        "near-limit legitimate custody is retained, bytes={total}"
    );
}

#[tokio::test]
async fn checkpoint_original_dependency_loss_withholds_entire_frontier_and_cannot_be_guessed_away()
{
    for loss in ["deleted", "generated", "protected"] {
        let Some(f) = fixture().await else { return };
        let s = scenario(&f).await;
        let a_body = body(
            &s.admission,
            vec![reviewed(&s.canonical)],
            "ACCESS_UNIT_A: canonical observation with an uncited detector lead.",
            vec![],
        );
        let a = save(&f, a_body.clone()).await;
        let a_origin = origin(&a);
        let b = save(
            &f,
            body(
                &a,
                vec![reviewed(&s.canonical)],
                "ACCESS_UNIT_B: another independent canonical observation.",
                vec![],
            ),
        )
        .await;
        let original = offered(&b);
        match loss {
            "deleted" => {
                ok(request(
                    &f.app,
                    &f.owner,
                    Method::DELETE,
                    &format!(
                        "/v1/workspace/entries/{}?expected_version=1",
                        s.support["entry_ref"].as_str().unwrap()
                    ),
                    None,
                )
                .await);
            }
            "generated" => {
                ok(post(&f, &f.owner, "/v1/workspace/write", json!({"path":s.support["path"],"expected_version":1,"content":"# Calibration\n\nGenerated edition.\n","metadata":{"kind":"briefing_edition"}})).await);
            }
            "protected" => {
                let id = Uuid::parse_str(
                    s.support["entry_ref"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("entry:"),
                )
                .unwrap();
                sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
                    .bind(f.owner.user)
                    .bind(id)
                    .bind(format!(".brunn/tasks/{}.md", Uuid::now_v7()))
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let replay = ok(post(&f, &f.runner, PROGRESS, a_body).await)["data"].clone();
        assert_eq!(
            replay["research"]["checkpoint_context_status"], "unavailable",
            "{loss}: {replay}"
        );
        assert!(contexts(&replay).is_empty());
        assert!(!replay.to_string().contains("ACCESS_UNIT_A"));
        assert!(!replay.to_string().contains("ACCESS_UNIT_B"));
        let (_, refreshed) = discover_subject(&f, &b, vec![]).await;
        assert_eq!(
            refreshed["research"]["checkpoint_context_status"],
            "unavailable"
        );
        assert!(contexts(&refreshed).is_empty());
        assert!(
            !refreshed["research"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|source| source["entry_ref"] == s.support["entry_ref"])
        );
        let mut zero = research_request(&refreshed);
        zero["candidates"] = json!([]);
        zero["processed_inputs"] = json!([]);
        zero["research_progress"] = body(
            &refreshed,
            vec![reviewed(&s.canonical)],
            "ZERO_ID_WITHHELD_RECONCILIATION_MUST_NOT_PERSIST",
            original.clone(),
        );
        rejected_unchanged(&f, &s.canonical, CANDIDATES, zero).await;
        rejected_unchanged(
            &f,
            &s.canonical,
            PROGRESS,
            body(
                &refreshed,
                vec![reviewed(&s.canonical)],
                "UNAVAILABLE_HISTORY_MUST_NOT_BE_DISCARDED",
                original,
            ),
        )
        .await;
        let read = ok(post(&f, &f.model, "/v1/workspace/read", json!({"requests":[{"path":path(&s.canonical),"version":a_origin["version"],"view":"full"}]})).await);
        assert!(
            !read.to_string().contains("ACCESS_UNIT_A"),
            "historical v2 reads retain original manifest authority"
        );
        let retained = current(&f, &path(&s.canonical)).await.unwrap().2["dreamer_research"]["checkpoint_versions"].clone();
        assert_eq!(retained.as_array().unwrap().len(), 2);
        if loss == "protected" {
            // RLS hides this head entirely. Unlike a visible tombstone or
            // generated edition, its disappearance cannot prove a terminal
            // source-policy disposition, so current scope remains unchecked.
            assert_eq!(refreshed["research"]["needs_refresh"], true);
            assert_eq!(
                refreshed["research"]["coverage"]["change_status"],
                "unchecked"
            );
            assert_eq!(
                refreshed["research"]["coverage"]["change_reason"],
                "research_sources_unresolved"
            );
            let before = current(&f, &path(&s.canonical)).await.unwrap();
            let state = current(&f, "dreams/state.md").await.unwrap();
            let refused = post(
                &f,
                &f.runner,
                PROGRESS,
                body(
                    &refreshed,
                    vec![reviewed(&s.canonical)],
                    "UNCHECKED_CURRENT_MUST_NOT_PERSIST",
                    vec![],
                ),
            )
            .await;
            assert_eq!(
                refused.status,
                StatusCode::BAD_REQUEST,
                "loss={loss}, phase=unchecked current checkpoint, error={}",
                refused.body
            );
            assert_eq!(refused.body["error"]["code"], "research_refresh_required");
            assert_eq!(current(&f, &path(&s.canonical)).await.unwrap(), before);
            assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
            let mut waiting = research_request(&refreshed);
            waiting["status"] = json!("waiting");
            let waiting =
                save_access_phase(&f, waiting, loss, "unchecked operational yield", &refreshed)
                    .await;
            assert_eq!(
                waiting["research"]["checkpoint_context_status"],
                "unavailable"
            );
            assert!(contexts(&waiting).is_empty());
            assert_eq!(
                current(&f, &path(&s.canonical)).await.unwrap().2["dreamer_research"]["checkpoint_versions"],
                retained
            );
            let next = next_subject(&f, &waiting).await;
            assert_eq!(next["research"]["subject_ref"], s.secondary["entry_ref"]);
            continue;
        }
        assert_ne!(
            refreshed["research"]["needs_refresh"], true,
            "loss={loss}, phase=explicit policy disposition permits fresh source scope, coverage={}",
            refreshed["research"]["coverage"]
        );
        let mut fresh = save_access_phase(&f, body(&refreshed, vec![reviewed(&s.canonical)], "FRESH_CURRENT: the remaining canonical record is independently checked; unavailable historical conclusions remain unresolved.", vec![]), loss, "initial fresh checkpoint", &refreshed).await;
        for step in 0..5 {
            assert_eq!(
                fresh["research"]["checkpoint_context_status"],
                "unavailable"
            );
            assert!(contexts(&fresh).is_empty());
            assert!(!fresh.to_string().contains("ACCESS_UNIT_A"));
            assert!(!fresh.to_string().contains("ACCESS_UNIT_B"));
            let current_origin = origin(&fresh);
            fresh = save_access_phase(&f, body(&fresh, vec![reviewed(&s.canonical)], &format!("FRESH_CUMULATIVE_{step}: the current canonical observation and open question are carried forward without disposing inaccessible historical work."), vec![current_origin]), loss, &format!("cumulative checkpoint {step}"), &fresh).await;
            assert_eq!(
                current(&f, &path(&s.canonical)).await.unwrap().2["dreamer_research"]["checkpoint_versions"],
                retained,
                "current-only reconciliation must not retire hidden origins or grow the frontier"
            );
        }
        assert!(fresh["research"]["current_checkpoint"].is_object());
        rejected_unchanged(
            &f,
            &s.canonical,
            PROGRESS,
            body(
                &fresh,
                vec![reviewed(&s.canonical)],
                "FRESH_CURRENT_CANNOT_AUTHORIZE_HIDDEN_RETIREMENT",
                vec![a_origin.clone()],
            ),
        )
        .await;
        let mut waiting = research_request(&fresh);
        waiting["status"] = json!("waiting");
        let waiting = save(&f, waiting).await;
        assert_eq!(
            waiting["research"]["checkpoint_context_status"],
            "unavailable"
        );
        let next = next_subject(&f, &waiting).await;
        assert_eq!(next["research"]["subject_ref"], s.secondary["entry_ref"]);
    }
}

fn proposal(admission: &Value, canonical: &Value) -> Value {
    let mut proposal = candidate(canonical, "Optical survey");
    proposal["subject_ref"] = canonical["entry_ref"].clone();
    proposal["path"] = admission["research"]["output_path"].clone();
    proposal["expected_version"] = admission["research"]["output_version"].clone();
    proposal["content"] =
        json!("# Optical survey\n\nThe coordinator maintains an optical survey.[^s1]\n");
    proposal
}

#[tokio::test]
async fn checkpoint_partial_candidate_and_zero_ids_retain_custody_priority_until_explicit_done() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let a = save(
        &f,
        body(
            &s.admission,
            vec![reviewed(&s.canonical)],
            "CANDIDATE_UNIT_A: detector follow-up remains unresolved.",
            vec![],
        ),
    )
    .await;
    let a_origin = origin(&a);
    let b = save(
        &f,
        body(
            &a,
            vec![reviewed(&s.canonical)],
            "CANDIDATE_UNIT_B: canonical observation is ready for a partial overview.",
            vec![],
        ),
    )
    .await;
    let b_origin = origin(&b);
    let completed =
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["completed"]
            .clone();
    let mut invalid = research_request(&b);
    invalid["candidates"] = json!([proposal(&b, &s.canonical)]);
    invalid["processed_inputs"] = json!([]);
    invalid["research_progress"] = body(
        &b,
        vec![reviewed(&s.canonical)],
        "INVALID_CANDIDATE_RECONCILIATION",
        vec![
            json!({"entry_ref":a_origin["entry_ref"],"version":99999,"snapshot_generation":a_origin["snapshot_generation"]}),
        ],
    );
    let mut invalid_zero = invalid.clone();
    invalid_zero["operation_id"] = json!(Uuid::now_v7());
    invalid_zero["candidates"] = json!([]);
    rejected_unchanged(&f, &s.canonical, CANDIDATES, invalid_zero).await;
    rejected_unchanged(&f, &s.canonical, CANDIDATES, invalid).await;
    assert!(review(&f).await["items"].as_array().unwrap().is_empty());
    let mut submission = research_request(&b);
    submission["candidates"] = json!([proposal(&b, &s.canonical)]);
    submission["processed_inputs"] = json!([]);
    submission["research_progress"] = body(
        &b,
        vec![reviewed(&s.canonical)],
        "The partial overview includes the canonical observation, while the independent detector follow-up remains open.",
        vec![b_origin],
    );
    let accepted = ok(post(&f, &f.runner, CANDIDATES, submission.clone()).await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    assert_contexts(
        &accepted,
        &[(
            &a_origin,
            "CANDIDATE_UNIT_A: detector follow-up remains unresolved.",
        )],
    );
    assert_eq!(accepted["checkpoint_receipt"]["subject_complete"], false);
    assert!(queued(&f, &s.canonical).await);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["completed"],
        completed
    );
    let mut zero = research_request(&accepted);
    zero["candidates"] = json!([]);
    zero["processed_inputs"] = json!([]);
    zero["research_progress"] = body(
        &accepted,
        vec![reviewed(&s.canonical)],
        "ZERO_ID_CANNOT_RETIRE: no candidate was accepted for the original detector work.",
        vec![a_origin.clone()],
    );
    let zero_result = ok(post(&f, &f.runner, CANDIDATES, zero).await);
    assert_eq!(zero_result["accepted_candidate_ids"], json!([]));
    assert!(
        contexts(&zero_result)
            .iter()
            .any(|context| context["origin"] == a_origin)
    );
    assert_eq!(zero_result["checkpoint_receipt"]["subject_complete"], false);
    assert_eq!(zero_result["checkpoint_receipt"]["reconciled"], false);
    let mut incomplete = body(
        &zero_result,
        vec![reviewed(&s.canonical), reviewed(&s.support)],
        "The primary sources were checked, but no historical work was explicitly resolved.",
        vec![],
    );
    incomplete["status"] = json!("no_change");
    rejected_unchanged(&f, &s.canonical, PROGRESS, incomplete).await;
    let mut done = body(
        &zero_result,
        vec![reviewed(&s.canonical), reviewed(&s.support)],
        "All offered detector conclusions and unfinished leads were reconsidered against the primary sources; the existing overview needs no further change.",
        offered(&zero_result),
    );
    done["status"] = json!("no_change");
    done["notes"] = json!("");
    done["findings"] = json!([
        "All offered detector conclusions and unfinished leads were reconsidered against the current primary records; the existing overview needs no further change and no working notes need retention."
    ]);
    let done_response = save(&f, done.clone()).await;
    assert!(contexts(&done_response).is_empty());
    assert!(done_response["research"]["current_checkpoint"].is_null());
    assert_eq!(done_response["research"]["notes"], "");
    assert_eq!(
        done_response["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        done_response["checkpoint_receipt"]["subject_complete"],
        true
    );
    assert!(!queued(&f, &s.canonical).await);
    let before = current(&f, &path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, CANDIDATES, submission).await);
    assert_eq!(replay["checkpoint_receipt"]["replayed"], true);
    assert!(contexts(&replay).is_empty());
    assert_eq!(current(&f, &path(&s.canonical)).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn checkpoint_fully_reconciled_candidate_completes_with_researching_nested_progress() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let first = save(
        &f,
        body(
            &s.admission,
            vec![reviewed(&s.canonical)],
            "COMPLETE_A: the canonical survey observation is checked.",
            vec![],
        ),
    )
    .await;
    let second = save(
        &f,
        body(
            &first,
            vec![reviewed(&s.support)],
            "COMPLETE_B: the detector context has been considered and adds no separate claim.",
            vec![],
        ),
    )
    .await;
    assert_eq!(offered(&second).len(), 2);
    let completed =
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["completed"]
            .as_u64()
            .unwrap();
    let mut submission = research_request(&second);
    submission["candidates"] = json!([proposal(&second, &s.canonical)]);
    submission["processed_inputs"] = json!([]);
    submission["research_progress"] = body(
        &second,
        vec![reviewed(&s.canonical), reviewed(&s.support)],
        "Both prior units and their unfinished leads were incorporated or explicitly reconsidered in this complete source-backed overview.",
        offered(&second),
    );
    assert_eq!(submission["research_progress"]["status"], "researching");
    let accepted = ok(post(&f, &f.runner, CANDIDATES, submission.clone()).await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    assert_eq!(accepted["checkpoint_receipt"]["subject_complete"], true);
    assert_eq!(accepted["research"]["status"], "waiting");
    assert!(contexts(&accepted).is_empty());
    assert!(!queued(&f, &s.canonical).await);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["completed"],
        completed + 1
    );
    let before = current(&f, &path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(&f, &f.runner, CANDIDATES, submission).await);
    assert_eq!(replay["checkpoint_receipt"]["replayed"], true);
    assert_eq!(replay["checkpoint_receipt"]["subject_complete"], true);
    assert_eq!(current(&f, &path(&s.canonical)).await.unwrap(), before);
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
}

#[tokio::test]
async fn checkpoint_mechanical_receipts_distinguish_new_coverage_cosmetics_consolidation_and_replay()
 {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let a_body = body(
        &s.admission,
        vec![reviewed(&s.canonical), reviewed(&s.support)],
        "MECHANICAL_A: primary records checked with a bounded open question.",
        vec![],
    );
    let a = save(&f, a_body.clone()).await;
    assert_receipt(&a, &a_body, false, true, false, false);
    let a_origin = origin(&a);
    let b_body = body(
        &a,
        vec![reviewed(&s.support), reviewed(&s.canonical)],
        "  MECHANICAL_A: primary records checked with a bounded open question.  ",
        vec![],
    );
    let b = save(&f, b_body.clone()).await;
    assert_receipt(&b, &b_body, false, false, false, false);
    let c_body = body(
        &b,
        vec![reviewed(&s.canonical), reviewed(&s.support)],
        "MECHANICAL_C: both prior notes and their open questions were incorporated into this current aggregate.",
        vec![a_origin, origin(&b)],
    );
    let c = save(&f, c_body.clone()).await;
    assert_receipt(&c, &c_body, false, false, true, false);
    let before = current(&f, &path(&s.canonical)).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    for operation in [&a_body, &b_body, &c_body] {
        let replay = ok(post(&f, &f.runner, PROGRESS, operation.clone()).await);
        assert_eq!(replay["no_op"], true);
        assert_receipt(&replay["data"], operation, true, false, false, false);
        assert_eq!(replay["data"]["research"], c["research"]);
        assert_eq!(current(&f, &path(&s.canonical)).await.unwrap(), before);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    }
    let mut changed = b_body;
    changed["notes"] = json!("CHANGED_REPLAY_PAYLOAD_MUST_NOT_PERSIST");
    rejected_unchanged(&f, &s.canonical, PROGRESS, changed).await;
}

#[tokio::test]
async fn checkpoint_legacy_migration_and_old_client_omission_cannot_erase_v2_custody() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let legacy = save(
        &f,
        progress_body(
            &s.admission,
            vec![reviewed(&s.canonical)],
            "researching",
            "LEGACY_UNIT: prior accepted notes retain an unfinished detector question.",
        ),
    )
    .await;
    assert_eq!(
        current(&f, &path(&s.canonical)).await.unwrap().2["dreamer_research"]["schema"],
        "dream.research.v1"
    );
    let newer = drift(&f, &s.support).await;
    let (_, refreshed) = discover_subject(&f, &legacy, vec![]).await;
    assert_eq!(contexts(&refreshed).len(), 1);
    let legacy_origin = contexts(&refreshed)[0]["origin"].clone();
    assert_eq!(legacy_origin["version"], legacy["research"]["version"]);
    let upgraded = save(
        &f,
        body(
            &refreshed,
            vec![reviewed(&newer)],
            "UPGRADED_UNIT: current detector note is independently checked.",
            vec![],
        ),
    )
    .await;
    assert_eq!(
        current(&f, &path(&s.canonical)).await.unwrap().2["dreamer_research"]["schema"],
        "dream.research.v2",
        "rollback API rejects this durable schema before saving unknown custody fields"
    );
    assert!(upgraded["research"]["revalidation_context"].is_null());
    let old = save(
        &f,
        progress_body(
            &upgraded,
            vec![reviewed(&s.canonical)],
            "researching",
            "OLD_CLIENT_UNIT: omitted protocol cannot imply reconciliation.",
        ),
    )
    .await;
    assert_eq!(contexts(&old).len(), 2);
    assert!(
        contexts(&old)
            .iter()
            .any(|context| context["origin"] == legacy_origin)
    );
    let no_change = progress_body(
        &old,
        vec![reviewed(&s.canonical), reviewed(&newer)],
        "no_change",
        "Old client has not explicitly accounted for the retained checkpoints.",
    );
    rejected_unchanged(&f, &s.canonical, PROGRESS, no_change).await;
    let mut unknown = body(
        &old,
        vec![reviewed(&s.canonical)],
        "UNKNOWN_PROTOCOL_MUST_NOT_PERSIST",
        vec![],
    );
    unknown["checkpoint_protocol"] = json!("dream.research.checkpoint.v999");
    rejected_unchanged(&f, &s.canonical, PROGRESS, unknown).await;
    let mut injected = body(
        &old,
        vec![reviewed(&s.canonical)],
        "SERVER_FRONTIER_INJECTION_MUST_NOT_PERSIST",
        vec![],
    );
    injected["checkpoint_contexts"] = json!([]);
    rejected_unchanged(&f, &s.canonical, PROGRESS, injected).await;
}

#[tokio::test]
async fn checkpoint_four_unit_maximum_authority_projection_remains_bounded_and_exact() {
    let Some(f) = fixture().await else { return };
    let s = scenario(&f).await;
    let mut sources = vec![s.canonical.clone(), s.support.clone()];
    for n in 0..254 {
        sources.push(write(&f, &format!("sources/Optics/Batch/Measurement-{n:03}.md"), &format!("# Measurement {n:03}\n\nAn independently recorded optical measurement has sample index {n:03}.\n"), 0).await);
    }
    let mut admission = s.admission.clone();
    for batch in sources[2..].chunks(32) {
        (_, admission) = discover_subject(
            &f,
            &admission,
            batch
                .iter()
                .map(|source| source["entry_ref"].clone())
                .collect(),
        )
        .await;
    }
    assert_eq!(
        admission["research"]["sources"].as_array().unwrap().len(),
        256
    );
    let mut units = Vec::new();
    for n in 0..4 {
        let notes = format!("MAX_UNIT_{n}:{}", "m".repeat(2 * 1024));
        let operation = body(
            &admission,
            sources[n * 64..(n + 1) * 64].iter().map(reviewed).collect(),
            &notes,
            vec![],
        );
        admission = save(&f, operation).await;
        units.push((origin(&admission), notes));
    }
    let _newer = drift(&f, &s.support).await;
    let started = std::time::Instant::now();
    let (_, refreshed) = discover_subject(&f, &admission, vec![]).await;
    let elapsed = started.elapsed();
    let expected = units
        .iter()
        .map(|(origin, notes)| (origin, notes.as_str()))
        .collect::<Vec<_>>();
    assert_contexts(&refreshed, &expected);
    for context in contexts(&refreshed) {
        assert_eq!(context["prior_progress"]["admitted_source_count"], 256);
        assert_eq!(
            context["prior_reviewed_sources"].as_array().unwrap().len(),
            64
        );
    }
    let bytes = serde_json::to_vec(&refreshed["research"]["checkpoint_contexts"])
        .unwrap()
        .len();
    eprintln!(
        "checkpoint maximum authority: four exact manifests x256 dependencies, x64 selectors; bytes={bytes}, elapsed={elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "projection must fit the existing client deadline: {elapsed:?}"
    );
}

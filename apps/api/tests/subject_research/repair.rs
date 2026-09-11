//! Real HTTP/database checks for operational repair context. A saved diagnostic
//! neither accepts rejected evidence nor replaces the last checked notebook.
use super::*;

struct Research {
    canonical: Value,
    support: Value,
    extra: Value,
    saved: Value,
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

fn feedback(phase: &str) -> Value {
    json!({"phase":phase,"message":"REPAIR_CONTEXT_MARKER: make the unsupported lead-in a heading or cite its source."})
}

fn hint_request(admission: &Value, phase: &str, status: &str) -> Value {
    let mut body = research_request(admission);
    body["status"] = json!(status);
    body["repair_feedback"] = feedback(phase);
    body["processed_inputs"] = json!([]);
    body
}

async fn progress(f: &Fixture, body: Value) -> Value {
    ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await)["data"]
        .clone()
}

async fn research(f: &Fixture) -> Research {
    control(f, "report-only", 0).await;
    let canonical = write(
        f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley is the canonical person.\n",
        0,
    )
    .await;
    let support = write(
        f,
        "sources/Notes/Outcome.md",
        "# Outcome\n\nThe current outcome is complete; one equipment detail remains unresolved.\n",
        0,
    )
    .await;
    let extra = write(
        f,
        "sources/Notes/Specification.md",
        "# Specification\n\nA separately retained equipment specification.\n",
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
    let (_, selected) = discover_subject(f, &selected, vec![support["entry_ref"].clone()]).await;
    let mut checked = progress_body(
        &selected,
        vec![reviewed(&canonical), reviewed(&support)],
        "researching",
        "CHECKED_NOTE_MARKER: the exact sources were inspected; the equipment detail remains unresolved.",
    );
    checked["pending_queries"] = json!(["equipment specification"]);
    checked["pending_targets"] = json!([extra["entry_ref"]]);
    let saved = progress(f, checked).await;
    Research {
        canonical,
        support,
        extra,
        saved,
    }
}

fn input_identity(admission: &Value, source: &Value) -> Value {
    let input = admission["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|input| input["entry_ref"] == source["entry_ref"])
        .unwrap();
    json!({"entry_ref":input["entry_ref"],"version":input["version"],"generation":input["generation"]})
}

fn unchanged_research(before: &Value, after: &Value) {
    let mut before = before.clone();
    let mut after = after.clone();
    for object in [&mut before, &mut after] {
        for field in ["repair_feedback", "status", "retry_at", "receipts"] {
            object.as_object_mut().unwrap().remove(field);
        }
    }
    assert_eq!(
        after, before,
        "an operational hint preserves every evidence and notebook field"
    );
}

fn unchanged_work(before: &Value, after: &Value) {
    for field in [
        "inputs",
        "processed_count",
        "processed_generation",
        "research",
        "items",
        "source_dispositions",
    ] {
        assert_eq!(
            after["dreamer_state"][field], before["dreamer_state"][field],
            "an operational hint preserves {field}"
        );
    }
}

fn assert_no_repair_phase(response: &Response) {
    assert!(
        response
            .body
            .pointer("/error/details/dreamer_repair_phase")
            .is_none(),
        "{}",
        response.body
    );
}

#[tokio::test]
async fn repair_hint_preserves_real_routed_work_and_notebook_across_replay_and_restart() {
    let Some(f) = fixture().await else { return };
    let r = research(&f).await;
    let mut submission = research_request(&r.saved);
    submission["candidates"] = json!([subject_candidate(
        &r.saved,
        vec![reviewed(&r.canonical), reviewed(&r.support)]
    )]);
    submission["processed_inputs"] = json!([]);
    let accepted = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        submission,
    )
    .await);
    let selected = next_subject(&f, &accepted).await;
    assert_eq!(selected["research"]["subject_ref"], r.support["entry_ref"]);
    let pointer = selected["comparison_proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["subject_ref"] == r.canonical["entry_ref"])
        .unwrap()["pointer"]
        .clone();
    let mut route = progress_body(
        &selected,
        vec![reviewed(&r.support)],
        "waiting",
        "CHECKED_NOTE_MARKER: the checked source needs attention in the existing overview.",
    );
    route["pending_queries"] = json!(["equipment specification"]);
    route["pending_targets"] = json!([r.extra["entry_ref"]]);
    route["processed_inputs"] = json!([]);
    route["follow_up"] = json!({"comparison":pointer,"origin_input":input_identity(&selected,&r.support),"targets":[r.support["entry_ref"]]});
    let routed = progress(&f, route).await;
    let path = job_path(&r.support);
    let before = current(&f, &path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(
        state.2["dreamer_state"]["research"]["follow_ups"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let request = hint_request(&routed, "candidate_validation", "researching");
    let saved = progress(&f, request.clone()).await;
    assert_eq!(
        saved["research"]["repair_feedback"],
        feedback("candidate_validation")
    );
    assert_eq!(
        saved["repair_feedback_receipt"],
        json!({"operation_id":request["operation_id"],"recorded":true})
    );
    let after = current(&f, &path).await.unwrap();
    unchanged_research(&before.2["dreamer_research"], &after.2["dreamer_research"]);
    unchanged_work(&state.2, &current(&f, "dreams/state.md").await.unwrap().2);
    assert!(after.0 > before.0);
    let accepted_state = current(&f, "dreams/state.md").await.unwrap();
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        request.clone(),
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(
        replay["data"]["repair_feedback_receipt"],
        saved["repair_feedback_receipt"]
    );
    assert_eq!(current(&f, &path).await.unwrap(), after);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        accepted_state
    );
    let mut changed = request;
    changed["repair_feedback"]["message"] =
        json!("A different correction cannot reuse this operation.");
    let denied = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        changed,
    )
    .await;
    assert!(denied.status.is_client_error(), "{}", denied.body);
    assert_no_repair_phase(&denied);
    assert_eq!(current(&f, &path).await.unwrap(), after);
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        accepted_state
    );
    finish(
        &f,
        &saved,
        saved["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let restarted = admit(&f).await;
    assert_eq!(restarted["research"]["subject_ref"], r.support["entry_ref"]);
    assert_eq!(
        restarted["research"]["repair_feedback"],
        feedback("candidate_validation")
    );
    assert_eq!(restarted["research"]["notes"], saved["research"]["notes"]);
    assert_eq!(
        restarted["research"]["pending_targets"],
        saved["research"]["pending_targets"]
    );
    assert!(
        restarted.get("repair_feedback_receipt").is_none(),
        "an ordinary admission is not an acknowledgement of a hint operation"
    );
}

#[tokio::test]
async fn repair_hint_rejects_mixed_payloads_bad_bounds_and_fences_without_partial_writes() {
    let Some(f) = fixture().await else { return };
    let r = research(&f).await;
    let path = job_path(&r.canonical);
    let before = current(&f, &path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let template = hint_request(&r.saved, "candidate_validation", "waiting");
    let mut invalid = Vec::new();
    for field in [
        "notes",
        "reviewed_sources",
        "pending_queries",
        "pending_targets",
        "findings",
        "candidates",
        "covers_existing",
        "supersedes_existing",
        "follow_up",
    ] {
        let mut body = template.clone();
        body[field] = if field == "notes" {
            json!("")
        } else {
            json!([])
        };
        invalid.push((field.to_owned(), body));
    }
    for (label, value) in [
        ("null", Value::Null),
        (
            "invalid phase",
            json!({"phase":"transport_failure","message":"Try again."}),
        ),
        (
            "oversize UTF-8",
            json!({"phase":"candidate_validation","message":"é".repeat(2049)}),
        ),
        (
            "empty",
            json!({"phase":"candidate_validation","message":"  "}),
        ),
        (
            "unknown field",
            json!({"phase":"candidate_validation","message":"Fix the citation.","raw_response":"Rejected body"}),
        ),
    ] {
        let mut body = template.clone();
        body["repair_feedback"] = value;
        invalid.push((label.into(), body));
    }
    let mut processed = template.clone();
    processed["processed_inputs"] = json!([input_identity(&r.saved, &r.canonical)]);
    invalid.push(("processed input".into(), processed));
    let mut terminal = template.clone();
    terminal["status"] = json!("no_change");
    invalid.push(("terminal hint".into(), terminal));
    for field in ["expected_state_version", "research_version"] {
        let mut body = template.clone();
        body[field] = json!(body[field].as_i64().unwrap() + 1);
        invalid.push((field.to_owned(), body));
    }
    // Attempt fences are opaque strings; change the identity without turning
    // this case into a malformed-type rejection before the fence check.
    assert!(template["fence"].as_str().is_some());
    let mut wrong_fence = template.clone();
    wrong_fence["fence"] = json!(Uuid::now_v7().to_string());
    invalid.push(("foreign fence".into(), wrong_fence));
    let mut wrong_attempt = template.clone();
    wrong_attempt["attempt_id"] = json!(Uuid::now_v7());
    invalid.push(("foreign attempt".into(), wrong_attempt));
    for (label, body) in invalid {
        let denied = post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            body,
        )
        .await;
        assert!(denied.status.is_client_error(), "{label}: {}", denied.body);
        assert_no_repair_phase(&denied);
        assert_eq!(current(&f, &path).await.unwrap(), before, "{label}");
        assert_eq!(
            current(&f, "dreams/state.md").await.unwrap(),
            state,
            "{label}"
        );
    }
    let foreign_runner = actor(&f.pool, Some(f.owner.user), &["dreamer:run"]).await;
    for caller in [&foreign_runner, &f.model] {
        let denied = post(
            &f,
            caller,
            "/v1/workspace/dreamer/research-progress",
            template.clone(),
        )
        .await;
        assert!(denied.status.is_client_error(), "{}", denied.body);
        assert_no_repair_phase(&denied);
        assert_eq!(current(&f, &path).await.unwrap(), before);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    }
}

#[tokio::test]
async fn repair_hint_phase_lifetime_distinguishes_evidence_checkpoints_from_dispositions() {
    for phase in [
        "response_validation",
        "checkpoint_validation",
        "candidate_validation",
    ] {
        let Some(f) = fixture().await else { return };
        let r = research(&f).await;
        let hinted = progress(&f, hint_request(&r.saved, phase, "researching")).await;
        let mut forced_wait = research_request(&hinted);
        forced_wait["status"] = json!("waiting");
        let waited = progress(&f, forced_wait).await;
        assert_eq!(waited["research"]["repair_feedback"], feedback(phase));
        assert!(waited.get("repair_feedback_receipt").is_none());
        let (_, expanded) = discover_subject(&f, &waited, vec![r.extra["entry_ref"].clone()]).await;
        assert_eq!(
            expanded["research"]["repair_feedback"],
            feedback(phase),
            "discovery does not repair a rejected response"
        );
        let checked = progress(
            &f,
            progress_body(
                &expanded,
                vec![
                    reviewed(&r.canonical),
                    reviewed(&r.support),
                    reviewed(&r.extra),
                ],
                "researching",
                "The admitted primary sources were explicitly reviewed.",
            ),
        )
        .await;
        if phase == "candidate_validation" {
            assert_eq!(
                checked["research"]["repair_feedback"],
                feedback(phase),
                "valid notebook progress does not validate candidate formatting"
            );
        } else {
            assert!(checked["research"]["repair_feedback"].is_null(), "{phase}");
        }
        let hinted = progress(&f, hint_request(&checked, phase, "researching")).await;
        let completed = if phase == "response_validation" {
            progress(
                &f,
                progress_body(
                    &hinted,
                    vec![
                        reviewed(&r.canonical),
                        reviewed(&r.support),
                        reviewed(&r.extra),
                    ],
                    "no_change",
                    "The exact sources support no additional change.",
                ),
            )
            .await
        } else {
            let mut body = research_request(&hinted);
            body["candidates"] = json!([subject_candidate(
                &hinted,
                vec![reviewed(&r.canonical), reviewed(&r.support)]
            )]);
            body["processed_inputs"] = json!([]);
            ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await)
        };
        assert!(
            completed["research"]["repair_feedback"].is_null(),
            "accepted disposition clears {phase}"
        );
        assert!(current(&f, &job_path(&r.canonical)).await.unwrap().2["dreamer_research"]["repair_feedback"].is_null());
    }
}

#[tokio::test]
async fn repair_hint_survives_pinned_progress_while_newer_versions_wait() {
    let Some(f) = fixture().await else { return };
    let r = research(&f).await;
    let replacement = write(
        &f,
        "sources/Notes/Outcome.md",
        "# Outcome\n\nThe current source corrects the earlier outcome.\n",
        1,
    )
    .await;
    let path = job_path(&r.canonical);
    let before = current(&f, &path).await.unwrap();
    let state = current(&f, "dreams/state.md").await.unwrap();
    let saved = progress(
        &f,
        hint_request(&r.saved, "candidate_validation", "waiting"),
    )
    .await;
    assert_eq!(saved["repair_feedback_receipt"]["recorded"], true);
    assert_eq!(
        saved["research"]["repair_feedback"],
        feedback("candidate_validation"),
        "a post-cutoff source version does not withhold the retained correction"
    );
    assert_eq!(saved["research"]["notes"], r.saved["research"]["notes"]);
    assert_eq!(
        saved["research"]["reviewed_sources"],
        r.saved["research"]["reviewed_sources"]
    );
    let after = current(&f, &path).await.unwrap();
    unchanged_research(&before.2["dreamer_research"], &after.2["dreamer_research"]);
    assert_eq!(
        after.2["dreamer_research"]["repair_feedback"],
        feedback("candidate_validation")
    );
    unchanged_work(&state.2, &current(&f, "dreams/state.md").await.unwrap().2);
    let pinned = progress(
        &f,
        progress_body(
            &saved,
            vec![reviewed(&r.canonical), reviewed(&r.support)],
            "researching",
            "Pinned selectors remain valid evidence within the pass.",
        ),
    )
    .await;
    assert_eq!(
        pinned["research"]["notes"],
        "Pinned selectors remain valid evidence within the pass."
    );
    assert_eq!(
        pinned["research"]["repair_feedback"],
        feedback("candidate_validation"),
        "notebook progress does not discharge a candidate defect"
    );
    let (_, refreshed) =
        discover_subject(&f, &pinned, vec![replacement["entry_ref"].clone()]).await;
    assert_eq!(
        refreshed["research"]["repair_feedback"],
        feedback("candidate_validation")
    );
    assert!(
        refreshed["research"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["entry_ref"] == replacement["entry_ref"]
                && source["version"] == 1),
        "the replacement waits for the next pass"
    );
    assert_eq!(replacement["version"], 2);
}

#[tokio::test]
async fn repair_hint_mixed_phases_keep_both_corrections_until_candidate_or_done_acceptance() {
    for phase in ["response_validation", "checkpoint_validation"] {
        let Some(f) = fixture().await else { return };
        let r = research(&f).await;
        let mut candidate_hint = hint_request(&r.saved, "candidate_validation", "researching");
        let prior_message = format!("CANDIDATE_REPAIR_MARKER: {}", "é".repeat(1400));
        candidate_hint["repair_feedback"]["message"] = json!(prior_message);
        let hinted = progress(&f, candidate_hint).await;
        let support = r.support.clone();
        if phase == "checkpoint_validation" {
            // A post-cutoff revision does not withhold the pinned notebook.
            write(&f, "sources/Notes/Outcome.md", "# Outcome\n\nThe current outcome is complete; one equipment detail remains unresolved.\n\nAn eligible source revision.\n", 1).await;
        }
        let mut lower = hint_request(&hinted, phase, "researching");
        lower["repair_feedback"]["message"] =
            json!(format!("LOWER_REPAIR_MARKER: {}", "界".repeat(1200)));
        let saved = progress(&f, lower.clone()).await;
        assert_eq!(
            saved["repair_feedback_receipt"],
            json!({"operation_id":lower["operation_id"],"recorded":true})
        );
        let path = job_path(&r.canonical);
        let stored = current(&f, &path).await.unwrap();
        let combined = stored.2["dreamer_research"]["repair_feedback"].clone();
        assert_eq!(combined["phase"], "candidate_validation");
        let message = combined["message"].as_str().unwrap();
        let mut prefix_end = 2048;
        while !prior_message.is_char_boundary(prefix_end) {
            prefix_end -= 1;
        }
        assert!(message.starts_with(&prior_message[..prefix_end]));
        assert!(message.contains(&format!(
            "Additional {phase} correction: LOWER_REPAIR_MARKER:"
        )));
        assert!(message.len() <= 4096);
        assert!(
            message.len() > 4000,
            "the combined UTF-8 bound is exercised"
        );
        assert_eq!(saved["research"]["repair_feedback"], combined);
        let (_, refreshed) = discover_subject(&f, &saved, vec![support["entry_ref"].clone()]).await;
        assert_eq!(refreshed["research"]["repair_feedback"], combined);
        let checked = progress(
            &f,
            progress_body(
                &refreshed,
                vec![reviewed(&r.canonical), reviewed(&support)],
                "researching",
                "The current primary sources were explicitly reviewed.",
            ),
        )
        .await;
        assert_eq!(
            checked["research"]["repair_feedback"], combined,
            "notebook acceptance cannot clear the outstanding candidate correction"
        );
        let job = current(&f, &path).await.unwrap();
        let state = current(&f, "dreams/state.md").await.unwrap();
        let replay = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            lower,
        )
        .await);
        assert_eq!(replay["no_op"], true);
        assert_eq!(replay["data"]["research"]["repair_feedback"], combined);
        assert_eq!(current(&f, &path).await.unwrap(), job);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        let accepted = if phase == "response_validation" {
            let mut body = research_request(&checked);
            body["candidates"] = json!([subject_candidate(
                &checked,
                vec![reviewed(&r.canonical), reviewed(&support)]
            )]);
            body["processed_inputs"] = json!([]);
            ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await)
        } else {
            progress(
                &f,
                progress_body(
                    &checked,
                    vec![reviewed(&r.canonical), reviewed(&support)],
                    "no_change",
                    "The current sources support no additional change.",
                ),
            )
            .await
        };
        assert!(accepted["research"]["repair_feedback"].is_null());
        assert!(
            current(&f, &path).await.unwrap().2["dreamer_research"]["repair_feedback"].is_null()
        );
    }
}

#[tokio::test]
async fn repair_hint_is_withheld_and_cleared_before_dependency_policy_or_access_pruning() {
    for loss in ["generated", "deleted", "credential", "access"] {
        let Some(f) = fixture().await else { return };
        let r = research(&f).await;
        let hint = hint_request(&r.saved, "candidate_validation", "researching");
        let saved = progress(&f, hint.clone()).await;
        let path = job_path(&r.canonical);
        let historical_version = saved["research"]["version"].as_i64().unwrap();
        let original = current(&f, &path).await.unwrap();
        let read_request =
            json!({"requests":[{"path":path,"version":historical_version,"view":"full"}]});
        let visible = ok(post(&f, &f.model, "/v1/workspace/read", read_request.clone()).await);
        assert!(
            visible.to_string().contains("REPAIR_CONTEXT_MARKER"),
            "the readable historical record actually contains the diagnostic"
        );
        match loss {
            "generated" => {
                ok(post(&f, &f.owner, "/v1/workspace/write", json!({"path":"sources/Notes/Outcome.md","expected_version":1,"content":"# Outcome\n\nA generated edition occupies this identity.\n","metadata":{"kind":"briefing_edition"}})).await);
            }
            "deleted" => {
                ok(request(
                    &f.app,
                    &f.owner,
                    Method::DELETE,
                    &format!(
                        "/v1/workspace/entries/{}?expected_version=1",
                        r.support["entry_ref"].as_str().unwrap()
                    ),
                    None,
                )
                .await);
            }
            "credential" | "access" => {
                let next_path = if loss == "credential" {
                    "sources/Credentials/Outcome.md".to_owned()
                } else {
                    format!(".brunn/tasks/{}.md", Uuid::now_v7())
                };
                // Fixture-only path movement exercises the real path policy/RLS
                // boundaries for current and exact historical source reads.
                sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
                    .bind(f.owner.user)
                    .bind(
                        Uuid::parse_str(
                            r.support["entry_ref"]
                                .as_str()
                                .unwrap()
                                .trim_start_matches("entry:"),
                        )
                        .unwrap(),
                    )
                    .bind(next_path)
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let replay = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            hint,
        )
        .await);
        assert_eq!(replay["no_op"], true);
        assert!(
            !replay.to_string().contains("REPAIR_CONTEXT_MARKER"),
            "{loss}"
        );
        assert!(
            !replay.to_string().contains("CHECKED_NOTE_MARKER"),
            "{loss}"
        );
        let historical = ok(post(&f, &f.model, "/v1/workspace/read", read_request.clone()).await);
        assert!(
            !historical.to_string().contains("REPAIR_CONTEXT_MARKER"),
            "{loss}"
        );
        let (_, refreshed) = discover_subject(&f, &saved, vec![]).await;
        assert!(refreshed["research"]["repair_feedback"].is_null(), "{loss}");
        assert!(
            current(&f, &path).await.unwrap().2["dreamer_research"]["repair_feedback"].is_null(),
            "a future fresh projection cannot resurrect revoked diagnostic context: {loss}"
        );
        let historical = ok(post(&f, &f.model, "/v1/workspace/read", read_request).await);
        assert!(
            !historical.to_string().contains("REPAIR_CONTEXT_MARKER"),
            "{loss}"
        );
        let retained: Value = sqlx::query_scalar("SELECT v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id WHERE e.user_id=$1 AND e.path=$2 AND v.version=$3")
            .bind(f.owner.user).bind(&path).bind(historical_version).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            retained, original.2,
            "safe projection never rewrites an immutable historical version"
        );
    }
}

#[tokio::test]
async fn repair_phase_marker_identifies_only_actual_candidate_and_checkpoint_contract_failures() {
    let Some(f) = fixture().await else { return };
    let r = research(&f).await;
    let hinted = progress(
        &f,
        hint_request(&r.saved, "candidate_validation", "researching"),
    )
    .await;
    let state = current(&f, "dreams/state.md").await.unwrap();
    let job = current(&f, &job_path(&r.canonical)).await.unwrap();
    let mut invalid =
        subject_candidate(&hinted, vec![reviewed(&r.canonical), reviewed(&r.support)]);
    invalid["content"] = json!(
        "# Radley\n\nRadley is the canonical person.[^s1]\nA formatting lead-in introduces the next observation:\nThe equipment detail remains unresolved.[^s2]\n"
    );
    let mut candidate = research_request(&hinted);
    candidate["candidates"] = json!([invalid]);
    candidate["processed_inputs"] = json!([]);
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", candidate).await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert_eq!(rejected.body["error"]["code"], "invalid_request");
    assert_eq!(
        rejected.body["error"]["details"]["dreamer_repair_phase"],
        "candidate_validation"
    );
    let checkpoint = progress_body(
        &hinted,
        vec![reviewed(&r.canonical), reviewed(&r.support)],
        "researching",
        &"x".repeat(12 * 1024 + 1),
    );
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        checkpoint.clone(),
    )
    .await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert_eq!(
        rejected.body["error"]["details"]["dreamer_repair_phase"],
        "checkpoint_validation"
    );
    let mut submitted_checkpoint = research_request(&hinted);
    submitted_checkpoint["candidates"] = json!([subject_candidate(
        &hinted,
        vec![reviewed(&r.canonical), reviewed(&r.support)]
    )]);
    submitted_checkpoint["research_progress"] = checkpoint;
    submitted_checkpoint["processed_inputs"] = json!([]);
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        submitted_checkpoint,
    )
    .await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert_eq!(
        rejected.body["error"]["details"]["dreamer_repair_phase"],
        "checkpoint_validation"
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
    assert_eq!(current(&f, &job_path(&r.canonical)).await.unwrap(), job);
}

#[tokio::test]
async fn repair_hint_does_not_override_a_fresh_owner_held_or_deferred_proposal() {
    for choice in ["approve", "defer"] {
        let Some(f) = fixture().await else { return };
        let r = research(&f).await;
        let mut body = research_request(&r.saved);
        body["candidates"] = json!([subject_candidate(
            &r.saved,
            vec![reviewed(&r.canonical), reviewed(&r.support)]
        )]);
        body["processed_inputs"] = json!([]);
        let mut accepted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await);
        let immutable = exact_run_record(&f, &accepted).await;
        let view = review(&f).await;
        let item = view["items"][0].clone();
        let decided = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, &item, choice),
        )
        .await);
        accepted["state_version"] = decided["data"]["state_version"].clone();
        let held = review(&f).await["items"][0].clone();
        assert_eq!(held["candidate_hash"], item["candidate_hash"]);
        assert_eq!(
            held["status"],
            if choice == "approve" {
                "approved_held"
            } else {
                "deferred"
            }
        );
        let state = current(&f, "dreams/state.md").await.unwrap();
        let hinted = progress(
            &f,
            hint_request(&accepted, "candidate_validation", "waiting"),
        )
        .await;
        unchanged_work(&state.2, &current(&f, "dreams/state.md").await.unwrap().2);
        assert_eq!(review(&f).await["items"][0], held);
        let state = current(&f, "dreams/state.md").await.unwrap();
        let job = current(&f, &job_path(&r.canonical)).await.unwrap();
        let mut revision =
            subject_candidate(&hinted, vec![reviewed(&r.canonical), reviewed(&r.support)]);
        revision["revises_item_id"] = item["id"].clone();
        revision["reason"] =
            json!("An operational correction cannot override the owner's retained decision.");
        let mut body = research_request(&hinted);
        body["candidates"] = json!([revision]);
        body["processed_inputs"] = json!([]);
        let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
        assert!(
            rejected.status.is_client_error(),
            "{choice}: {}",
            rejected.body
        );
        assert_no_repair_phase(&rejected);
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), state);
        assert_eq!(current(&f, &job_path(&r.canonical)).await.unwrap(), job);
        assert_eq!(
            job.2["dreamer_research"]["repair_feedback"],
            feedback("candidate_validation")
        );
        assert_eq!(review(&f).await["items"][0], held);
        assert_eq!(exact_run_record(&f, &accepted).await, immutable);
    }
}

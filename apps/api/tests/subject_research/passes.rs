//! Bounded research passes: a fixed evidence cutoff per pass, reliance-only
//! dependencies, refresh of what matters, and honest fast reads. These cover
//! the revised Dreamer acceptance cases with real HTTP/database fixtures.
use super::*;
use std::time::Instant;

fn research_path(canonical: &Value) -> String {
    format!(
        "dreams/research/{}.md",
        canonical["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:")
    )
}

/// The published overview is read by its managed path; summary projection
/// applies to managed paths regardless of the canonical-read preference flag.
async fn read_current(f: &Fixture, output_path: &str) -> Value {
    ok(post(
        f,
        &f.owner,
        "/v1/workspace/read",
        json!({"requests":[{"path":output_path,"view":"current_state","max_chars":12000}]}),
    )
    .await)["data"]["items"][0]
        .clone()
}

async fn read_exact(f: &Fixture, reference: &Value, version: i64) -> Value {
    ok(post(
        f,
        &f.owner,
        "/v1/workspace/read",
        json!({"requests":[{"ref":reference["entry_ref"],"version":version,"view":"full","max_chars":12000}]}),
    )
    .await)["data"]["items"][0]
        .clone()
}

fn overview(admission: &Value, selectors: Vec<Value>, content: &str) -> Value {
    let mut candidate = subject_candidate(admission, selectors);
    candidate["content"] = json!(content);
    candidate
}

async fn submit_overview(f: &Fixture, admission: &Value, candidate: Value) -> Response {
    let mut submit = research_request(admission);
    submit["candidates"] = json!([candidate]);
    submit["processed_inputs"] = json!([]);
    submit["findings"] = json!(["Supported overview assembled from pinned evidence."]);
    post(f, &f.runner, "/v1/workspace/dreamer/candidates", submit).await
}

/// Cases 2, 4 and 5: an uncited reminder changes during drafting and again
/// after an interruption; the pass keeps its cutoff and retained work, accepts
/// the supported overview without a restart or duplicate proposal, and the
/// completed pass is distinguishable from later refresh work.
#[tokio::test]
async fn uncited_lead_churn_cannot_move_the_finish_line_and_resume_keeps_the_cutoff() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley is the canonical person with a current plan.\n",
        0,
    )
    .await;
    let plan = write(
        &f,
        "sources/Notes/Radley plan.md",
        "# Radley plan\n\nRadley's school plan is on track for the autumn term.\n",
        0,
    )
    .await;
    let reminder = write(
        &f,
        "shared/tasks/rental-cancellation-reminders.md",
        "# Rental reminders\n\nBooking confirmation pending; Radley's school run is unaffected.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    assert_eq!(a["research"]["subject_ref"], person["entry_ref"]);
    let cutoff = a["research"]["pass"]["cutoff"].as_i64().unwrap();
    assert_eq!(a["research"]["coverage"]["evidence_cutoff"], cutoff);
    assert_eq!(a["research"]["pass"]["rounds"], 0);
    let (_, a) = discover_subject(
        &f,
        &a,
        vec![plan["entry_ref"].clone(), reminder["entry_ref"].clone()],
    )
    .await;
    assert_eq!(a["research"]["sources"].as_array().unwrap().len(), 3);
    assert_eq!(
        a["research"]["pass"]["cutoff"], cutoff,
        "discovery keeps the cutoff"
    );
    let notes = "Radley's plan is on track for the autumn term; the reminder is unrelated context.";
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &a,
            vec![reviewed(&person), reviewed(&plan)],
            "researching",
            notes,
        ),
    )
    .await)["data"]
        .clone();
    assert_eq!(saved["research"]["notes"], notes);
    // The uncited, unreviewed reminder changes while the draft is composed.
    write(
        &f,
        "shared/tasks/rental-cancellation-reminders.md",
        "# Rental reminders\n\nBooking confirmed and refund closed; delivery workflow updated.\n",
        1,
    )
    .await;
    // Interruption: the attempt ends without a yield. The next selection resumes
    // the same pass with the same cutoff and pinned evidence.
    finish(
        &f,
        &saved,
        saved["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let resumed = next_subject(&f, &admit(&f).await).await;
    assert_eq!(resumed["research"]["subject_ref"], person["entry_ref"]);
    assert_eq!(
        resumed["research"]["pass"]["cutoff"], cutoff,
        "resume keeps the cutoff: {}",
        resumed["research"]
    );
    assert_eq!(
        resumed["research"]["notes"], notes,
        "useful draft work is preserved"
    );
    assert_ne!(resumed["research"]["needs_refresh"], true);
    let pinned = resumed["research"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["entry_ref"] == reminder["entry_ref"])
        .expect("reminder stays admitted");
    assert_eq!(
        pinned["version"], 1,
        "post-cutoff versions are queued, not mixed in"
    );
    assert_eq!(resumed["research"]["pass"]["changed"], json!([]));
    let content = "# Radley\n\nRadley is the canonical person.[^s1]\nRadley's school plan is on track for the autumn term.[^s2]\n";
    let accepted = ok(submit_overview(
        &f,
        &resumed,
        overview(&resumed, vec![reviewed(&person), reviewed(&plan)], content),
    )
    .await);
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    let scope = &accepted["pending"][0]["candidate"]["subject_scope"];
    assert_eq!(scope["checked_generation"], cutoff);
    let bound = scope["dependencies"].as_array().unwrap();
    assert_eq!(bound.len(), 2, "only reliance evidence is bound: {bound:?}");
    assert!(
        !bound
            .iter()
            .any(|d| d["entry_ref"] == reminder["entry_ref"])
    );
    assert!(accepted["research"]["pass"].is_null(), "the pass completed");
    assert_eq!(
        accepted["research"]["reviewed_through"]["generation"],
        cutoff
    );
    assert_eq!(
        accepted["research"]["reviewed_through"]["disposition"],
        "accepted"
    );
    let view = review(&f).await;
    assert_eq!(
        view["items"].as_array().unwrap().len(),
        1,
        "no duplicate proposals"
    );
    assert_eq!(view["items"][0]["stale"], false);
    assert_eq!(view["items"][0]["refresh_pending"], false);
    assert_eq!(view["items"][0]["evidence_cutoff"], cutoff);
    let item = view["items"][0].clone();

    // Case 5: continuing nonmaterial writes neither stale the proposal nor
    // make the completed subject due again.
    write(
        &f,
        "shared/tasks/rental-cancellation-reminders.md",
        "# Rental reminders\n\nDelivery rescheduled again; nothing about the plan.\n",
        2,
    )
    .await;
    write(
        &f,
        "sources/Notes/Unrelated.md",
        "# Unrelated\n\nWeather.\n",
        0,
    )
    .await;
    let view = review(&f).await;
    assert_eq!(view["items"][0]["stale"], false);
    assert_eq!(view["items"][0]["refresh_pending"], false);
    finish(
        &f,
        &accepted,
        accepted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let idle = next_subject(&f, &admit(&f).await).await;
    assert_ne!(
        idle["research"]["subject_ref"], person["entry_ref"],
        "a completed pass is not reselected for unreviewed lead churn: {}",
        idle["research"]
    );
    if idle["research"]["subject_ref"].is_string() {
        // Another admitted input became a subject; settle it so lane order
        // cannot interfere with the refresh selection below.
        let settle = progress_body(
            &idle,
            vec![reviewed(&idle["research"]["sources"][0])],
            "no_change",
            "The input's own record supports no overview yet.",
        );
        ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            settle,
        )
        .await);
    }
    finish(
        &f,
        &idle,
        idle["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;

    // Reliance drift is outstanding refresh work: the subject is due, the new
    // pass has a newer cutoff, the prose is kept and the moved source is a
    // required reread. Submitting without rereading it is rejected.
    let plan_v2 = write(
        &f,
        "sources/Notes/Radley plan.md",
        "# Radley plan\n\nRadley's school plan is on track; the autumn term start moved one week later.\n",
        1,
    )
    .await;
    let view = review(&f).await;
    assert_eq!(
        view["items"][0]["stale"], false,
        "drift is refresh, not invalidation: {}",
        view["items"][0]
    );
    assert_eq!(view["items"][0]["refresh_pending"], true);
    let refresh = next_subject(
        &f,
        &admit_requested(&f, vec![person["entry_ref"].clone()]).await,
    )
    .await;
    assert_eq!(refresh["research"]["subject_ref"], person["entry_ref"]);
    let next_cutoff = refresh["research"]["pass"]["cutoff"].as_i64().unwrap();
    assert!(next_cutoff > cutoff);
    assert_eq!(
        refresh["research"]["notes"], notes,
        "prose is kept across refresh"
    );
    let changed = refresh["research"]["pass"]["changed"].as_array().unwrap();
    let moved = changed
        .iter()
        .find(|c| c["entry_ref"] == plan["entry_ref"])
        .unwrap_or_else(|| panic!("{changed:?}"));
    assert_eq!(moved["kind"], "version");
    assert_eq!(moved["from_version"], 1);
    assert_eq!(moved["version"], 2);
    assert_eq!(moved["required"], true);
    assert!(
        changed
            .iter()
            .filter(|c| c["entry_ref"] != plan["entry_ref"])
            .all(|c| c["required"] == false),
        "unreviewed lead churn is offered, never required: {changed:?}"
    );
    assert!(
        !refresh["research"]["reviewed_sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["entry_ref"] == plan["entry_ref"]),
        "a moved selector is dropped until reread"
    );
    let mut stale_revision = overview(
        &refresh,
        vec![reviewed(&person)],
        "# Radley\n\nRadley is the canonical person.[^s1]\n",
    );
    stale_revision["revises_item_id"] = item["id"].clone();
    let refused = submit_overview(&f, &refresh, stale_revision).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    assert!(
        refused.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("changed reliance evidence must be reviewed"),
        "{}",
        refused.body
    );
    let mut revision = overview(
        &refresh,
        vec![reviewed(&person), reviewed(&plan_v2)],
        "# Radley\n\nRadley is the canonical person.[^s1]\nRadley's autumn term start moved one week later.[^s2]\n",
    );
    revision["revises_item_id"] = item["id"].clone();
    let revised = ok(submit_overview(&f, &refresh, revision).await);
    assert_eq!(revised["accepted_candidate_ids"], json!([item["id"]]));
    assert_eq!(
        revised["research"]["reviewed_through"]["generation"],
        next_cutoff
    );
    let view = review(&f).await;
    assert_eq!(view["items"].as_array().unwrap().len(), 1);
    assert_eq!(view["items"][0]["refresh_pending"], false);
    let record = current(&f, &research_path(&person)).await.unwrap().2;
    assert_eq!(
        record["dreamer_research"]["coverage"]["last_pass"]["outcome"],
        "accepted"
    );
    f.pool.close().await;
}

/// Cases 3, 6, 7 and 8: a published overview keeps serving as a dated
/// snapshot without model work or a change scan; a daytime correction in an
/// uncited source is retrievable immediately and forces reassessment in the
/// next pass; access loss still withholds; exact history stays exact.
#[tokio::test]
async fn contradiction_in_uncited_source_is_reassessed_and_reads_stay_honest_and_fast() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Radley.md",
        "# Radley\n\nRadley is the canonical person.\n",
        0,
    )
    .await;
    let booking = write(
        &f,
        "sources/Trips/Cabin booking.md",
        "# Cabin booking\n\nRadley booked the cabin for October.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let (_, a) = discover_subject(&f, &a, vec![booking["entry_ref"].clone()]).await;
    let notes = "Radley booked the cabin for October per the booking note.";
    let saved = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        progress_body(
            &a,
            vec![reviewed(&person), reviewed(&booking)],
            "researching",
            notes,
        ),
    )
    .await)["data"]
        .clone();
    // The overview relies on the booking note but cites only the canonical.
    let content =
        "# Radley\n\nRadley is the canonical person and is going to the cabin in October.[^s1]\n";
    let accepted = ok(submit_overview(
        &f,
        &saved,
        overview(&saved, vec![reviewed(&person)], content),
    )
    .await);
    let bound = accepted["pending"][0]["candidate"]["subject_scope"]["dependencies"]
        .as_array()
        .unwrap();
    assert!(
        bound.iter().any(|d| d["entry_ref"] == booking["entry_ref"]),
        "reviewed evidence is reliance even when uncited: {bound:?}"
    );
    let cutoff = accepted["research"]["reviewed_through"]["generation"]
        .as_i64()
        .unwrap();
    finish(
        &f,
        &accepted,
        accepted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let view = review(&f).await;
    let item = view["items"][0].clone();
    ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &item, "approve"),
    )
    .await);
    control(&f, "full", 1).await;
    let publishing = admit(&f).await;
    let output = a["research"]["output_path"].as_str().unwrap().to_owned();
    let published = current(&f, &output).await.expect("published overview");
    assert!(published.1.contains("going to the cabin in October"));
    finish(
        &f,
        &publishing,
        publishing["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    control(&f, "report-only", 2).await;

    // Case 7: repeated unchanged reads serve the saved overview with honest
    // status. Measure against the raw source read on the same fixture.
    let first = read_current(&f, &output).await;
    println!(
        "ACCEPTANCE_PUBLISHED_OVERVIEW path={output}\n{}\nACCEPTANCE_FRESHNESS_AFTER_PUBLICATION {}",
        first["text"].as_str().unwrap_or_default(),
        first["freshness"]
    );
    assert_eq!(first["representation"], "derived_summary", "{first}");
    assert_eq!(first["freshness"]["status"], "fresh");
    assert_eq!(first["freshness"]["refresh"]["status"], "current");
    assert_eq!(first["freshness"]["evidence_cutoff"], cutoff);
    assert!(first["freshness"]["reviewed_at"].is_string());
    assert!(first["freshness"]["published_at"].is_string());
    for index in 0..200 {
        write(
            &f,
            &format!("sources/Journal/Day-{index}.md"),
            &format!("# Day {index}\n\nUnrelated entry.\n"),
            0,
        )
        .await;
    }
    let mut summary_ms = Vec::new();
    let mut raw_ms = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let read = read_current(&f, &output).await;
        summary_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(read["representation"], "derived_summary");
        assert_eq!(read["freshness"]["status"], "fresh");
        assert_eq!(read["text"], published.1);
        let started = Instant::now();
        let raw = read_exact(&f, &person, 1).await;
        raw_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(raw["version"], 1);
    }
    summary_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    raw_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (summary_median, raw_median) = (summary_ms[10], raw_ms[10]);
    println!(
        "ACCEPTANCE_LATENCY summary_median_ms={summary_median:.2} raw_median_ms={raw_median:.2} summary_p90_ms={:.2} raw_p90_ms={:.2} churn_writes=200",
        summary_ms[18], raw_ms[18]
    );
    assert!(
        summary_median <= raw_median * 4.0 + 5.0,
        "summary reads must stay within a small factor of raw reads: {summary_median:.2}ms vs {raw_median:.2}ms"
    );

    // Case 8: a material correction lands in the uncited booking note during
    // the day. Source retrieval sees it immediately; the dated overview stays
    // available with refresh pending; nothing reruns Dreamer.
    let booking_v2 = write(
        &f,
        "sources/Trips/Cabin booking.md",
        "# Cabin booking\n\nCancelled: Radley's October cabin booking was cancelled and refunded.\n",
        1,
    )
    .await;
    let found = ok(post(
        &f,
        &f.owner,
        "/v1/workspace/search",
        json!({"queries":[{"query":"Radley cabin booking cancelled","modes":["lexical"]}]}),
    )
    .await);
    assert!(
        found.to_string().contains("cancelled"),
        "the correction is retrievable without Dreamer: {found}"
    );
    let exact_v1 = read_exact(&f, &booking, 1).await;
    assert!(
        exact_v1["text"]
            .as_str()
            .unwrap()
            .contains("booked the cabin for October")
    );
    assert!(!exact_v1["text"].as_str().unwrap().contains("Cancelled"));
    let exact_v2 = read_exact(&f, &booking, 2).await;
    assert!(exact_v2["text"].as_str().unwrap().contains("Cancelled"));
    let pending = read_current(&f, &output).await;
    println!(
        "ACCEPTANCE_DAYTIME_CORRECTION source_v2={}\nACCEPTANCE_FRESHNESS_AFTER_CORRECTION {}",
        exact_v2["text"].as_str().unwrap_or_default().trim(),
        pending["freshness"]
    );
    assert_eq!(pending["representation"], "derived_summary");
    assert_eq!(pending["freshness"]["status"], "fresh");
    assert_eq!(pending["freshness"]["refresh"]["status"], "pending");
    assert_eq!(pending["freshness"]["refresh"]["changed_source_count"], 1);
    assert_eq!(pending["freshness"]["evidence_cutoff"], cutoff);

    // Case 3: the next pass must reassess the contradiction. Unchanged
    // citations alone cannot certify the affected conclusion.
    let refresh = next_subject(
        &f,
        &admit_requested(&f, vec![person["entry_ref"].clone()]).await,
    )
    .await;
    assert_eq!(refresh["research"]["subject_ref"], person["entry_ref"]);
    let changed = refresh["research"]["pass"]["changed"].as_array().unwrap();
    assert!(
        changed
            .iter()
            .any(|c| c["entry_ref"] == booking["entry_ref"] && c["required"] == true),
        "{changed:?}"
    );
    let stale = overview(&refresh, vec![reviewed(&person)], content);
    let refused = submit_overview(&f, &refresh, stale).await;
    println!(
        "ACCEPTANCE_NEXT_PASS_CHANGED {}\nACCEPTANCE_UNREREAD_SUBMISSION_REFUSED {}",
        refresh["research"]["pass"]["changed"], refused.body["error"]["message"]
    );
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    assert_eq!(
        refused.body["error"]["details"]["dreamer_repair_phase"],
        "candidate_validation"
    );
    let mut corrected = overview(
        &refresh,
        vec![reviewed(&person), reviewed(&booking_v2)],
        "# Radley\n\nRadley is the canonical person.[^s1]\nThe October cabin booking was cancelled and refunded; no cabin trip is planned.[^s2]\n",
    );
    corrected["expected_version"] = json!(published.0);
    let revised = ok(submit_overview(&f, &refresh, corrected).await);
    assert_eq!(
        revised["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    let after = read_current(&f, &output).await;
    println!(
        "ACCEPTANCE_CORRECTED_REVISION_ACCEPTED {}\nACCEPTANCE_FRESHNESS_AFTER_REVISION {}",
        revised["pending"].as_array().map_or(0, Vec::len),
        after["freshness"]
    );
    assert_eq!(
        after["representation"], "derived_summary",
        "the dated overview remains available"
    );
    assert_eq!(after["freshness"]["status"], "revision_pending");
    assert_eq!(after["freshness"]["refresh"]["status"], "revision_pending");
    assert_eq!(after["text"], published.1);
    finish(
        &f,
        &revised,
        revised["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;

    // Case 6: access loss is not a refresh lag. Deleting reliance evidence
    // withholds the cached overview and stales the pending proposal.
    ok(request(
        &f.app,
        &f.owner,
        Method::DELETE,
        &format!(
            "/v1/workspace/entries/{}?expected_version=2",
            booking["entry_ref"].as_str().unwrap()
        ),
        None,
    )
    .await);
    let withheld = read_current(&f, &output).await;
    assert_ne!(withheld["representation"], "derived_summary");
    assert_ne!(withheld["freshness"]["status"], "fresh");
    assert!(
        !withheld["text"]
            .as_str()
            .unwrap_or_default()
            .contains("cabin")
    );
    let view = review(&f).await;
    assert_eq!(view["items"][0]["stale"], true);
    assert_eq!(view["items"][0]["reviewable"], false);
    let exact_v1 = read_exact(&f, &person, 1).await;
    assert!(
        exact_v1["text"]
            .as_str()
            .unwrap()
            .contains("canonical person")
    );
    f.pool.close().await;
}

/// A pass has one model-round allowance across resumes. When it is spent, the
/// next selection starts a new pass at a newer cutoff, keeping retained work.
#[tokio::test]
async fn exhausted_pass_allowance_starts_a_new_cutoff_without_losing_work() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let person = write(
        &f,
        "sources/People/Wren.md",
        "# Wren\n\nWren is the canonical person.\n",
        0,
    )
    .await;
    let a = next_subject(&f, &admit(&f).await).await;
    let cutoff = a["research"]["pass"]["cutoff"].as_i64().unwrap();
    let limit = a["research"]["pass"]["rounds_remaining"].as_u64().unwrap();
    assert!(limit > 0);
    let notes = "Wren's canonical observation is checked.";
    let mut latest = a.clone();
    for _ in 0..limit {
        latest = ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/research-progress",
            progress_body(&latest, vec![reviewed(&person)], "researching", notes),
        )
        .await)["data"]
            .clone();
    }
    assert_eq!(latest["research"]["pass"]["rounds_remaining"], 0);
    assert_eq!(latest["research"]["pass"]["cutoff"], cutoff);
    write(
        &f,
        "sources/Notes/Later.md",
        "# Later\n\nWren wrote later.\n",
        0,
    )
    .await;
    finish(
        &f,
        &latest,
        latest["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let renewed = next_subject(
        &f,
        &admit_requested(&f, vec![person["entry_ref"].clone()]).await,
    )
    .await;
    assert_eq!(renewed["research"]["subject_ref"], person["entry_ref"]);
    assert!(renewed["research"]["pass"]["cutoff"].as_i64().unwrap() > cutoff);
    assert_eq!(renewed["research"]["pass"]["rounds"], 0);
    assert_eq!(
        renewed["research"]["notes"], notes,
        "retained work survives the new pass"
    );
    let record = current(&f, &research_path(&person)).await.unwrap().2;
    assert_eq!(
        record["dreamer_research"]["coverage"]["last_pass"]["outcome"],
        "exhausted"
    );
    assert_eq!(
        record["dreamer_research"]["coverage"]["last_pass"]["cutoff"],
        cutoff
    );
    f.pool.close().await;
}

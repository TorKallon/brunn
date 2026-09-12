use super::*;
use chrono::{DateTime, Duration};

async fn enable(f: &Fixture, mode: &str, version: i64) {
    write(
        f,
        "dreams/CONTROL.md",
        &format!("enabled: true\nmode: {mode}\nauto_apply_after_hours: 24\n"),
        version,
    )
    .await;
}

// Simulate server time on the exact immutable fixture audit, never production data.
async fn proposed_at(f: &Fixture, item: &Value, at: DateTime<Utc>, legacy: bool) {
    let id = Uuid::parse_str(
        item["run_entry_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("entry:")
            .unwrap(),
    )
    .unwrap();
    let version = item["run_version"].as_i64().unwrap();
    let mut metadata: Value = sqlx::query_scalar(
        "SELECT metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3",
    )
    .bind(f.owner.user)
    .bind(id)
    .bind(version)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let audited = metadata["dreamer_run"]["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|a| a["id"] == item["id"])
        .unwrap();
    if legacy {
        audited.as_object_mut().unwrap().remove("proposed_at");
    } else {
        audited["proposed_at"] = json!(at);
    }
    sqlx::query("UPDATE brunn.entry_versions SET metadata=$4,created_at=$5 WHERE user_id=$1 AND entry_id=$2 AND version=$3")
        .bind(f.owner.user).bind(id).bind(version).bind(metadata).bind(at).execute(&f.pool).await.unwrap();
}

#[tokio::test]
async fn automatic_publication_applies_all_due_items_preserves_holds_and_replays_once() {
    let Some(f) = fixture().await else { return };
    enable(&f, "full", 0).await;
    let source = write(
        &f,
        "sources/Ready/Fixture.md",
        "# Ready\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let drift = write(
        &f,
        "sources/Drift/Fixture.md",
        "# Drift\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let first = admit(&f).await;
    let mut candidates: Vec<Value> = (0..9)
        .map(|i| candidate(&source, &format!("automatic-{i}")))
        .collect();
    candidates.extend(
        ["reject", "defer", "correct", "young", "future"].map(|name| candidate(&source, name)),
    );
    candidates.push(candidate(&drift, "stale"));
    candidates.push(json!({"kind":"question","title":"An owner question","question":"Which option do you prefer?","sources":[{"entry_ref":source["entry_ref"],"version":1,"start_line":3,"end_line":3}]}));
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        candidates,
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let initial = review(&f).await;
    for item in initial["items"].as_array().unwrap() {
        let age = match item["title"].as_str().unwrap() {
            "young summary" => Duration::hours(23),
            "future summary" => Duration::hours(-1),
            _ => Duration::hours(25),
        };
        proposed_at(&f, item, Utc::now() - age, false).await;
    }
    for choice in ["reject", "defer", "correct"] {
        let view = review(&f).await;
        let item = view["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["title"] == format!("{choice} summary"))
            .unwrap();
        let mut body = decision(&view, item, choice);
        if choice == "correct" {
            body["correction"] = json!("Retain the original observation.");
        }
        ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", body).await);
    }
    write(
        &f,
        "sources/Drift/Fixture.md",
        "# Drift\n\nA changed observation.\n",
        1,
    )
    .await;
    let view = review(&f).await;
    assert_eq!(view["auto_apply_after_hours"], 24);
    for item in view["items"].as_array().unwrap() {
        if item["status"] != "pending" || item["kind"] == "question" {
            assert!(item["auto_apply_at"].is_null());
        }
    }
    let request =
        json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60});
    let next = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        request.clone(),
    )
    .await);
    assert_eq!(
        ok(post(&f, &f.runner, "/v1/workspace/dreamer/admit", request).await),
        next
    );
    for i in 0..9 {
        assert_eq!(
            current(&f, &format!("derived/entities/automatic-{i}.md"))
                .await
                .unwrap()
                .0,
            1
        );
    }
    for name in ["reject", "defer", "correct", "young", "future", "stale"] {
        assert!(
            current(&f, &format!("derived/entities/{name}.md"))
                .await
                .is_none()
        );
    }
    let audits:i64=sqlx::query_scalar("SELECT count(*) FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.path LIKE 'dreams/reviews/auto-%' AND v.metadata#>>'{dreamer_review,decision,decision}'='auto_apply'").bind(f.owner.user).fetch_one(&f.pool).await.unwrap();
    assert_eq!(audits, 9);
    assert_eq!(
        review(&f).await["history"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["decision"] == "auto_apply")
            .count(),
        9
    );
    let (_, terminal) = finish(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    assert_eq!(
        terminal["latest_receipt"]["applied_writes"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
    let third = admit(&f).await;
    assert_eq!(
        current(&f, "derived/entities/automatic-0.md")
            .await
            .unwrap()
            .0,
        1
    );
    finish(
        &f,
        &third,
        third["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
}

#[tokio::test]
async fn automatic_publication_requires_policy_and_full_mode_and_survives_pause() {
    let Some(f) = fixture().await else { return };
    control(&f, "full", 0).await;
    let source = write(
        &f,
        "sources/Legacy/Fixture.md",
        "# Legacy\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let first = admit(&f).await;
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "legacy-policy")],
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let item = review(&f).await["items"][0].clone();
    proposed_at(&f, &item, Utc::now() - Duration::hours(25), true).await;
    let second = admit(&f).await;
    assert!(
        current(&f, "derived/entities/legacy-policy.md")
            .await
            .is_none()
    );
    finish(
        &f,
        &second,
        second["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
    enable(&f, "report-only", 1).await;
    let third = admit(&f).await;
    assert!(
        current(&f, "derived/entities/legacy-policy.md")
            .await
            .is_none()
    );
    finish(
        &f,
        &third,
        third["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
    enable(&f, "full", 2).await;
    ok(post(&f, &f.owner, "/v1/workspace/dreaming/pause", json!({})).await);
    assert_eq!(admit(&f).await["admitted"], false);
    ok(post(&f, &f.owner, "/v1/workspace/dreaming/resume", json!({})).await);
    assert!(
        current(&f, "dreams/CONTROL.md")
            .await
            .unwrap()
            .1
            .contains("auto_apply_after_hours: 24")
    );
    let fourth = admit(&f).await;
    assert_eq!(
        current(&f, "derived/entities/legacy-policy.md")
            .await
            .unwrap()
            .0,
        1
    );
    finish(
        &f,
        &fourth,
        fourth["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn automatic_publication_revision_restarts_clock_but_exact_replay_does_not() {
    let Some(f) = fixture().await else { return };
    enable(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Revisions/Fixture.md",
        "# Revision\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let first = admit(&f).await;
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "revision-policy")],
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let original = review(&f).await["items"][0].clone();
    proposed_at(&f, &original, Utc::now() - Duration::hours(25), false).await;
    let second = admit(&f).await;
    let mut revised = candidate(&source, "revision-policy");
    revised["title"] = json!("Revised title");
    revised["revises_item_id"] = original["id"].clone();
    let (body, submitted) = submit(
        &f,
        &second,
        second["state_version"].as_i64().unwrap(),
        vec![revised],
    )
    .await;
    let revised_item = review(&f).await["items"][0].clone();
    assert_ne!(revised_item["candidate_hash"], original["candidate_hash"]);
    assert_eq!(revised_item["id"], original["id"]);
    ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await);
    assert_eq!(
        review(&f).await["items"][0]["proposed_at"],
        revised_item["proposed_at"]
    );
    finish(
        &f,
        &second,
        submitted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    enable(&f, "full", 1).await;
    let third = admit(&f).await;
    assert!(
        current(&f, "derived/entities/revision-policy.md")
            .await
            .is_none()
    );
    finish(
        &f,
        &third,
        third["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
    proposed_at(&f, &revised_item, Utc::now() - Duration::hours(25), false).await;
    let fourth = admit(&f).await;
    assert_eq!(
        current(&f, "derived/entities/revision-policy.md")
            .await
            .unwrap()
            .0,
        1
    );
    finish(
        &f,
        &fourth,
        fourth["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

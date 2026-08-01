use chrono::NaiveDate;
use serde_json::json;
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

use straylight::briefing_service::{
    BriefingOmission, BriefingSection, DedupeCandidate, apply_edition_to_ledger,
    dedupe_candidate_in_tx,
};

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping briefing endpoint test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to disposable Postgres");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Straylight migrations");
    Some(pool)
}

async fn insert_test_user(pool: &PgPool) -> Uuid {
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("briefing-endpoint-test:{user_id}"))
        .bind("Briefing endpoint test")
        .execute(pool)
        .await
        .expect("insert test user");
    user_id
}

fn sections_fixture(value: serde_json::Value) -> Vec<BriefingSection> {
    serde_json::from_value(value).expect("sections fixture deserializes")
}

fn omissions_fixture(value: serde_json::Value) -> Vec<BriefingOmission> {
    serde_json::from_value(value).expect("omissions fixture deserializes")
}

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date parses")
}

fn candidate(value: serde_json::Value) -> DedupeCandidate {
    serde_json::from_value(value).expect("candidate fixture deserializes")
}

async fn seed_ledger(pool: &PgPool, user_id: Uuid) {
    let sections = sections_fixture(json!([
        {
            "topic": "ai",
            "title": "AI",
            "items": [
                {
                    "id": "alpha-item",
                    "kind": "news",
                    "headline_md": "**Alpha lands.**",
                    "delta": "new",
                    "story": {
                        "key": "story-alpha",
                        "urls": ["https://example.com/alpha"],
                        "title": "OpenAI evaluation incident",
                        "entities": ["OpenAI"],
                        "event_at": "2026-07-28"
                    }
                },
                {
                    "id": "beta-item",
                    "kind": "news",
                    "headline_md": "**Beta echoes.**",
                    "delta": "corroboration",
                    "story": {
                        "key": "story-beta",
                        "urls": ["https://example.com/beta"],
                        "title": "Kimi weights release"
                    }
                }
            ]
        }
    ]));
    let omitted = omissions_fixture(json!([
        {
            "story_key": "story-gamma",
            "urls": ["https://example.com/gamma"],
            "reason": "already delivered elsewhere"
        }
    ]));
    let mut tx = pool.begin().await.expect("begin seed");
    apply_edition_to_ledger(
        &mut tx,
        user_id,
        "entry:dedupe-seed-1",
        date("2026-07-28"),
        &sections,
        &omitted,
    )
    .await
    .expect("seed ledger");
    tx.commit().await.expect("commit seed");
}

#[tokio::test]
async fn dedupe_lookups_classify_url_story_key_and_unseen_candidates() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    seed_ledger(&pool, user_id).await;
    let mut tx = pool.begin().await.expect("begin dedupe lookups");

    // Exact URL hit on a delivered story, through canonicalization noise.
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({
            "urls": ["HTTPS://Example.COM/alpha?utm_source=x#frag"],
            "title": ""
        })),
    )
    .await
    .expect("url candidate report");
    assert_eq!(report.verdict_hint, "duplicate");
    assert_eq!(report.exact.len(), 1);
    assert_eq!(report.exact[0].story_key, "story-alpha");
    assert_eq!(report.exact[0].matched_by, ["url"]);
    assert_eq!(report.exact[0].delivery_count, 1);
    assert_eq!(
        report.exact[0].last_delivered_date,
        Some(date("2026-07-28")),
    );
    assert_eq!(
        report.exact[0].last_delivered_edition_ref.as_deref(),
        Some("entry:dedupe-seed-1"),
    );
    assert_eq!(
        report.exact[0].last_delivered_headline.as_deref(),
        Some("**Alpha lands.**"),
    );

    // Story-key hit with a newer candidate event date.
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({
            "story_key": "story-alpha",
            "event_at": "2026-07-30"
        })),
    )
    .await
    .expect("story-key candidate report");
    assert_eq!(report.verdict_hint, "possible_update");
    assert_eq!(report.exact.len(), 1);
    assert_eq!(report.exact[0].matched_by, ["story_key"]);

    // URL and story-key hits on the same story merge into one exact entry.
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({
            "urls": ["https://example.com/alpha"],
            "story_key": "story-alpha"
        })),
    )
    .await
    .expect("merged candidate report");
    assert_eq!(report.exact.len(), 1);
    assert_eq!(report.exact[0].matched_by, ["url", "story_key"]);

    // Seen-but-never-delivered stories (corroboration, suppression) hint update.
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({"story_key": "story-beta"})),
    )
    .await
    .expect("corroborated candidate report");
    assert_eq!(report.verdict_hint, "possible_update");
    assert_eq!(report.exact[0].delivery_count, 0);
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({"urls": ["https://example.com/gamma"]})),
    )
    .await
    .expect("suppressed candidate report");
    assert_eq!(report.verdict_hint, "possible_update");
    assert_eq!(report.exact[0].story_key, "story-gamma");
    assert_eq!(report.exact[0].suppression_count, 1);

    // The near lane finds ledger titles by FTS without an exact hit.
    // plainto_tsquery ANDs every non-stopword term, so each candidate term
    // must stem-match the stored title ("the" drops, "incidents" stems).
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({
            "urls": ["https://example.com/unrelated"],
            "title": "The OpenAI evaluation incidents"
        })),
    )
    .await
    .expect("near candidate report");
    assert_eq!(report.verdict_hint, "unseen");
    assert!(report.exact.is_empty());
    let ledger_titles: Vec<&str> = report
        .near
        .iter()
        .filter(|entry| entry["lane"] == "ledger_titles")
        .filter_map(|entry| entry["story_key"].as_str())
        .collect();
    assert_eq!(ledger_titles, ["story-alpha"]);

    // An exact hit is not repeated in the near lane for the same title.
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({
            "story_key": "story-alpha",
            "title": "OpenAI evaluation incident"
        })),
    )
    .await
    .expect("exact-and-near candidate report");
    assert_eq!(report.exact.len(), 1);
    assert!(
        report
            .near
            .iter()
            .all(|entry| entry["story_key"].as_str() != Some("story-alpha")),
        "the exact story must not repeat in the near lane",
    );

    // Nothing matches: unseen with empty lanes. (The workspace lexical lane
    // requires the request auth context and returns no rows under a plain
    // test connection; it is exercised by the live contract cycle.)
    let report = dedupe_candidate_in_tx(
        &mut tx,
        user_id,
        &candidate(json!({
            "urls": ["https://example.com/never-seen"],
            "title": "Entirely novel subject matter",
            "story_key": "story-omega"
        })),
    )
    .await
    .expect("unseen candidate report");
    assert_eq!(report.verdict_hint, "unseen");
    assert!(report.exact.is_empty());
    assert!(
        report
            .near
            .iter()
            .all(|entry| entry["lane"] != "ledger_titles"),
    );
    tx.commit().await.expect("commit dedupe lookups");
}

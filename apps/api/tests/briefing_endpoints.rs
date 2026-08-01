use chrono::{NaiveDate, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

use straylight::briefing_service::{
    BriefingOmission, BriefingSection, DedupeCandidate, apply_edition_to_ledger,
    dedupe_candidate_in_tx, feedback_log_path, get_edition_in_tx, list_editions_in_tx,
    render_request_document, topics_snapshot_in_tx,
};
use straylight::error::ApiError;

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

async fn insert_entry_version(
    pool: &PgPool,
    user_id: Uuid,
    entry_id: Uuid,
    path: &str,
    version: i64,
    content: &str,
    metadata: serde_json::Value,
) {
    let title = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md");
    let mut tx = pool.begin().await.expect("begin entry insert");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,$4,'markdown','text/markdown',$5)
        ON CONFLICT (user_id,(lower(normalize(path, NFC)))) DO UPDATE
        SET current_version=EXCLUDED.current_version
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(path)
    .bind(title)
    .bind(version)
    .execute(&mut *tx)
    .await
    .expect("insert entry");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(entry_id)
    .bind(version)
    .bind(hex::encode(Sha256::digest(content.as_bytes())))
    .bind(content)
    .bind(i64::try_from(content.len()).expect("content fits"))
    .bind(metadata)
    .execute(&mut *tx)
    .await
    .expect("insert entry version");
    tx.commit().await.expect("commit entry insert");
}

fn edition_metadata_fixture(date: &str, edition: &str, headline: &str) -> serde_json::Value {
    json!({
        "kind": "briefing_edition",
        "briefing": {
            "schema": "briefing.v1",
            "date": date,
            "edition": edition,
            "generated_at": format!("{date}T06:30:00-07:00"),
            "summary_md": ["First bullet.", "Second bullet."],
            "sections": [
                {
                    "topic": "ai",
                    "title": "AI",
                    "items": [
                        {"id": "item-one", "kind": "news", "headline_md": headline},
                        {"id": "item-two", "kind": "metric", "headline_md": "**Metric.**"}
                    ]
                },
                {
                    "topic": "markets",
                    "title": "Markets",
                    "items": [
                        {"id": "item-three", "kind": "metric", "headline_md": "**Open.**"}
                    ]
                }
            ],
            "omitted": [],
            "delta": {"added": [], "changed": [], "removed": []}
        }
    })
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

#[tokio::test]
async fn list_and_get_editions_project_metadata_and_paginate() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    let morning_aug1 = Uuid::now_v7();
    insert_entry_version(
        &pool,
        user_id,
        morning_aug1,
        "Briefings/2026/Morning briefing - 2026-08-01.md",
        1,
        "# Morning briefing - 2026-08-01 v1\n",
        edition_metadata_fixture("2026-08-01", "morning", "**First take.**"),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        morning_aug1,
        "Briefings/2026/Morning briefing - 2026-08-01.md",
        2,
        "# Morning briefing - 2026-08-01 v2\n",
        edition_metadata_fixture("2026-08-01", "morning", "**Updated take.**"),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/2026/Evening briefing - 2026-08-01.md",
        1,
        "# Evening briefing - 2026-08-01 v1\n",
        edition_metadata_fixture("2026-08-01", "evening", "**Evening take.**"),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/2026/Morning briefing - 2026-07-31.md",
        1,
        "# Morning briefing - 2026-07-31 v1\n",
        edition_metadata_fixture("2026-07-31", "morning", "**Yesterday.**"),
    )
    .await;
    // A topic entry under Briefings/ without edition metadata must not list.
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/Topics/ai.md",
        1,
        "---\nkind: briefing_topic\n---\nWatch labs.\n",
        json!({}),
    )
    .await;

    let mut tx = pool.begin().await.expect("begin list");
    let (page_one, truncated) = list_editions_in_tx(&mut tx, user_id, 2, None)
        .await
        .expect("first page lists");
    assert!(truncated);
    let paths: Vec<&str> = page_one
        .iter()
        .filter_map(|row| row["path"].as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "Briefings/2026/Morning briefing - 2026-08-01.md",
            "Briefings/2026/Evening briefing - 2026-08-01.md",
        ],
        "date-descending, then path-descending within a date",
    );
    assert_eq!(page_one[0]["date"], "2026-08-01");
    assert_eq!(page_one[0]["edition"], "morning");
    assert_eq!(page_one[0]["version"], 2);
    assert_eq!(page_one[0]["section_titles"], json!(["AI", "Markets"]));
    assert_eq!(page_one[0]["item_count"], 3);
    assert_eq!(
        page_one[0]["summary_md"],
        json!(["First bullet.", "Second bullet."]),
    );

    let (page_two, truncated) = list_editions_in_tx(
        &mut tx,
        user_id,
        2,
        Some("Briefings/2026/Evening briefing - 2026-08-01.md"),
    )
    .await
    .expect("second page lists");
    assert!(!truncated);
    assert_eq!(page_two.len(), 1);
    assert_eq!(
        page_two[0]["path"],
        "Briefings/2026/Morning briefing - 2026-07-31.md",
    );

    let error = list_editions_in_tx(&mut tx, user_id, 2, Some("Briefings/Topics/ai.md"))
        .await
        .expect_err("a non-edition cursor is rejected");
    assert!(matches!(
        error,
        ApiError::Public {
            code: "invalid_request",
            ..
        }
    ));

    let edition = get_edition_in_tx(&mut tx, user_id, "2026-08-01", "morning", None)
        .await
        .expect("current edition loads");
    assert_eq!(edition["version"], 2);
    assert_eq!(edition["current_version"], 2);
    assert_eq!(edition["markdown"], "# Morning briefing - 2026-08-01 v2\n");
    assert_eq!(edition["briefing"]["edition"], "morning");
    assert_eq!(
        edition["briefing"]["sections"][0]["items"][0]["headline_md"],
        "**Updated take.**",
    );
    assert_eq!(edition["versions"].as_array().map(Vec::len), Some(2));
    assert_eq!(edition["versions"][0]["version"], 1);
    assert_eq!(edition["versions"][1]["version"], 2);

    let historical = get_edition_in_tx(&mut tx, user_id, "2026-08-01", "morning", Some(1))
        .await
        .expect("historical version loads");
    assert_eq!(historical["version"], 1);
    assert_eq!(
        historical["markdown"],
        "# Morning briefing - 2026-08-01 v1\n",
    );
    assert_eq!(
        historical["briefing"]["sections"][0]["items"][0]["headline_md"],
        "**First take.**",
    );

    for (date, edition_slug, version) in [
        ("2026-08-02", "morning", None),
        ("2026-08-01", "overnight", None),
        ("2026-08-01", "morning", Some(9)),
    ] {
        let error = get_edition_in_tx(&mut tx, user_id, date, edition_slug, version)
            .await
            .expect_err("missing editions are 404s");
        assert!(matches!(
            error,
            ApiError::Public {
                code: "briefing_not_found",
                ..
            }
        ));
    }
    tx.commit().await.expect("commit list");
}

#[tokio::test]
async fn topics_snapshot_lists_topics_requests_and_feedback_tail() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    let now = Utc::now();
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/Topics/stocks.md",
        1,
        "---\nkind: briefing_topic\nname: Stock watchlist\nsection_order: 70\n\
         mode: every_briefing\nsymbols: [GOOGL, MSFT]\n---\n\nAbsolute dollar changes.\n",
        json!({}),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/Topics/ai.md",
        1,
        "---\nkind: briefing_topic\n---\n\nWatch labs.\n",
        json!({}),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/Topics/broken.md",
        1,
        "---\nkind: briefing_topic\nnever closed\n",
        json!({}),
    )
    .await;
    let edition_ref = format!("entry:{}", Uuid::now_v7());
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/Requests/2026-08-01 - item-one.md",
        1,
        &render_request_document(
            "2026-08-01",
            &edition_ref,
            "item-one",
            Some("ai"),
            Some("Compare vendors."),
        ),
        json!({"kind": "briefing_request"}),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        "Briefings/Requests/2026-07-30 - item-two.md",
        1,
        "---\nkind: briefing_request\nstatus: answered\nitem_id: item-two\n---\n\nDone.\n",
        json!({"kind": "briefing_request"}),
    )
    .await;
    let feedback_lines: Vec<String> = (1..=60).map(|index| format!("- line {index}")).collect();
    insert_entry_version(
        &pool,
        user_id,
        Uuid::now_v7(),
        &feedback_log_path(now),
        1,
        &format!("{}\n", feedback_lines.join("\n")),
        json!({"kind": "briefing_feedback_log"}),
    )
    .await;

    let mut tx = pool.begin().await.expect("begin snapshot");
    let snapshot = topics_snapshot_in_tx(&mut tx, user_id, now)
        .await
        .expect("snapshot loads");
    tx.commit().await.expect("commit snapshot");

    let topics = snapshot["topics"].as_array().expect("topics array");
    let slugs: Vec<&str> = topics
        .iter()
        .filter_map(|topic| topic["slug"].as_str())
        .collect();
    assert_eq!(
        slugs,
        ["stocks", "ai", "broken"],
        "section_order then slug ordering",
    );
    assert_eq!(topics[0]["name"], "Stock watchlist");
    assert_eq!(topics[0]["section_order"], 70);
    assert_eq!(topics[0]["symbols"], json!(["GOOGL", "MSFT"]));
    assert_eq!(topics[0]["body"], "Absolute dollar changes.\n");
    assert!(topics[0].get("parse_error").is_none());
    assert_eq!(topics[1]["mode"], "every_briefing");
    assert!(
        topics[2]["parse_error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "malformed topics surface a parse_error",
    );
    assert_eq!(
        topics[2]["body"], "---\nkind: briefing_topic\nnever closed\n",
        "malformed topics keep the raw body",
    );

    let pending = snapshot["pending_requests"]
        .as_array()
        .expect("pending requests array");
    assert_eq!(pending.len(), 1, "answered requests are excluded");
    assert_eq!(
        pending[0]["path"],
        "Briefings/Requests/2026-08-01 - item-one.md",
    );
    assert_eq!(pending[0]["date"], "2026-08-01");
    assert_eq!(pending[0]["item_id"], "item-one");
    assert_eq!(pending[0]["edition_ref"], json!(edition_ref));
    assert_eq!(pending[0]["topic"], "ai");
    assert_eq!(pending[0]["note"], "Compare vendors.");

    assert_eq!(snapshot["feedback_path"], json!(feedback_log_path(now)));
    let tail = snapshot["feedback_tail"].as_array().expect("tail array");
    assert_eq!(tail.len(), 50);
    assert_eq!(tail[0], "- line 11");
    assert_eq!(tail[49], "- line 60");
}

use chrono::NaiveDate;
use serde_json::json;
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

use sha2::{Digest, Sha256};
use straylight::briefing_service::{
    BriefingOmission, BriefingSection, apply_edition_to_ledger, canonicalize_url,
    rebuild_briefing_ledger, story_url_hash,
};

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping briefing ledger test");
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
        .bind(format!("briefing-ledger-test:{user_id}"))
        .bind("Briefing ledger test")
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

type StoryRow = (
    String,
    String,
    Vec<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
    i32,
    String,
);

async fn story_row(pool: &PgPool, user_id: Uuid, story_key: &str) -> StoryRow {
    sqlx::query_as(
        r#"
        SELECT title,topic,entities,event_at::text,last_delivered_date::text,
               last_delivered_edition_ref,last_delivered_headline,
               delivery_count,suppression_count,status
        FROM straylight.briefing_stories
        WHERE user_id=$1 AND story_key=$2
        "#,
    )
    .bind(user_id)
    .bind(story_key)
    .fetch_one(pool)
    .await
    .expect("story row exists")
}

async fn url_rows(pool: &PgPool, user_id: Uuid) -> Vec<(String, String, String)> {
    sqlx::query_as(
        r#"
        SELECT story_key,url_hash::text,url
        FROM straylight.briefing_story_urls
        WHERE user_id=$1
        ORDER BY story_key,url_hash
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .expect("url rows load")
}

type LedgerSnapshot = (
    Vec<(
        String,
        String,
        String,
        Vec<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i32,
        i32,
        String,
    )>,
    Vec<(String, String, String)>,
);

async fn ledger_snapshot(pool: &PgPool, user_id: Uuid) -> LedgerSnapshot {
    let stories = sqlx::query_as(
        r#"
        SELECT story_key,title,topic,entities,event_at::text,last_delivered_date::text,
               last_delivered_edition_ref,last_delivered_headline,
               delivery_count,suppression_count,status
        FROM straylight.briefing_stories
        WHERE user_id=$1
        ORDER BY story_key
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .expect("story snapshot loads");
    (stories, url_rows(pool, user_id).await)
}

struct EditionFixture {
    entry_id: Uuid,
    path: &'static str,
    version: i64,
    date: &'static str,
    sections: serde_json::Value,
    omitted: serde_json::Value,
}

async fn insert_edition_version(pool: &PgPool, user_id: Uuid, edition: &EditionFixture) {
    let mut tx = pool.begin().await.expect("begin edition insert");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,$4,'markdown','text/markdown',$5)
        ON CONFLICT (user_id,(lower(normalize(path, NFC)))) DO UPDATE
        SET current_version=EXCLUDED.current_version
        "#,
    )
    .bind(edition.entry_id)
    .bind(user_id)
    .bind(edition.path)
    .bind(format!("Morning briefing - {}", edition.date))
    .bind(edition.version)
    .execute(&mut *tx)
    .await
    .expect("insert edition entry");
    let content = format!(
        "# Morning briefing - {} v{}\n",
        edition.date, edition.version
    );
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(edition.entry_id)
    .bind(edition.version)
    .bind(hex::encode(Sha256::digest(content.as_bytes())))
    .bind(&content)
    .bind(i64::try_from(content.len()).expect("content fits"))
    .bind(json!({
        "kind": "briefing_edition",
        "briefing": {
            "schema": "briefing.v1",
            "date": edition.date,
            "edition": "morning",
            "sections": edition.sections,
            "omitted": edition.omitted,
            "delta": {"added": [], "changed": [], "removed": []}
        }
    }))
    .execute(&mut *tx)
    .await
    .expect("insert edition version");
    tx.commit().await.expect("commit edition insert");
}

#[tokio::test]
async fn rebuild_replay_matches_publish_order_application() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    let first_entry = Uuid::now_v7();
    let second_entry = Uuid::now_v7();
    let editions = [
        EditionFixture {
            entry_id: first_entry,
            path: "Briefings/2026/Morning briefing - 2026-08-01.md",
            version: 1,
            date: "2026-08-01",
            sections: json!([
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
                                "title": "Alpha story",
                                "entities": ["Alpha"],
                                "event_at": "2026-07-30"
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
                                "title": "Beta story"
                            }
                        }
                    ]
                }
            ]),
            omitted: json!([
                {"story_key": "story-gamma", "urls": ["https://example.com/gamma"], "reason": "seen"}
            ]),
        },
        EditionFixture {
            entry_id: first_entry,
            path: "Briefings/2026/Morning briefing - 2026-08-01.md",
            version: 2,
            date: "2026-08-01",
            sections: json!([
                {
                    "topic": "ai",
                    "title": "AI",
                    "items": [
                        {
                            "id": "alpha-item",
                            "kind": "news",
                            "headline_md": "**Alpha gains a timeline.**",
                            "delta": "update",
                            "story": {
                                "key": "story-alpha",
                                "urls": ["https://example.com/alpha", "https://example.com/alpha-postmortem"]
                            }
                        },
                        {
                            "id": "delta-item",
                            "kind": "news",
                            "headline_md": "**Delta emerges.**",
                            "delta": "new",
                            "story": {
                                "key": "story-delta",
                                "urls": ["https://example.com/delta"],
                                "title": "Delta story"
                            }
                        }
                    ]
                }
            ]),
            omitted: json!([
                {"story_key": "story-gamma", "reason": "still seen"}
            ]),
        },
        EditionFixture {
            entry_id: second_entry,
            path: "Briefings/2026/Morning briefing - 2026-08-02.md",
            version: 1,
            date: "2026-08-02",
            sections: json!([
                {
                    "topic": "markets",
                    "title": "Markets",
                    "items": [
                        {
                            "id": "beta-item",
                            "kind": "news",
                            "headline_md": "**Beta confirmed.**",
                            "delta": "new",
                            "story": {
                                "key": "story-beta",
                                "urls": ["https://example.com/beta-deep-dive"]
                            }
                        },
                        {
                            "id": "alpha-item",
                            "kind": "news",
                            "headline_md": "**Alpha echo.**",
                            "delta": "corroboration",
                            "story": {"key": "story-alpha"}
                        }
                    ]
                }
            ]),
            omitted: json!([
                {"story_key": "story-epsilon", "urls": ["https://example.com/epsilon"], "reason": "minor"}
            ]),
        },
    ];

    for edition in &editions {
        let mut tx = pool.begin().await.expect("begin publish-order apply");
        apply_edition_to_ledger(
            &mut tx,
            user_id,
            &format!("entry:{}", edition.entry_id),
            date(edition.date),
            &sections_fixture(edition.sections.clone()),
            &omissions_fixture(edition.omitted.clone()),
        )
        .await
        .expect("apply edition in publish order");
        tx.commit().await.expect("commit publish-order apply");
        insert_edition_version(&pool, user_id, edition).await;
    }
    let publish_order = ledger_snapshot(&pool, user_id).await;
    assert_eq!(publish_order.0.len(), 5, "five stories accumulate");

    sqlx::query(
        r#"
        INSERT INTO straylight.briefing_stories (user_id,story_key,title,delivery_count)
        VALUES ($1,'zzz-stale-row','A row rebuild must remove',7)
        "#,
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("seed stale ledger row");

    let mut tx = pool.begin().await.expect("begin rebuild");
    let rebuild = rebuild_briefing_ledger(&mut tx, user_id)
        .await
        .expect("rebuild ledger");
    tx.commit().await.expect("commit rebuild");
    assert_eq!(rebuild.replayed_versions, 3);
    assert_eq!(rebuild.skipped_versions, 0);
    assert_eq!(rebuild.skipped_invalid_urls, 0);

    let rebuilt = ledger_snapshot(&pool, user_id).await;
    assert_eq!(
        rebuilt, publish_order,
        "replaying every stored version reproduces the publish-order ledger",
    );
}

#[tokio::test]
async fn apply_edition_tracks_delivery_corroboration_and_suppression() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;

    let first_sections = sections_fixture(json!([
        {
            "topic": "ai",
            "title": "AI",
            "items": [
                {
                    "id": "alpha-item",
                    "kind": "news",
                    "headline_md": "**Alpha headline.**",
                    "delta": "new",
                    "story": {
                        "key": "story-alpha",
                        "urls": [
                            "HTTPS://Example.COM/alpha?utm_source=x&b=2&a=1",
                            "not a url"
                        ],
                        "title": "Alpha story",
                        "entities": ["Alpha"],
                        "event_at": "2026-07-30"
                    }
                },
                {
                    "id": "beta-item",
                    "kind": "news",
                    "headline_md": "**Beta headline.**",
                    "delta": "corroboration",
                    "story": {
                        "key": "story-beta",
                        "urls": ["https://example.com/beta"],
                        "title": "Beta story"
                    }
                }
            ]
        }
    ]));
    let first_omitted = omissions_fixture(json!([
        {
            "story_key": "story-gamma",
            "urls": ["https://example.com/gamma"],
            "reason": "already delivered 2026-07-28"
        }
    ]));

    let mut tx = pool.begin().await.expect("begin first edition");
    let skipped = apply_edition_to_ledger(
        &mut tx,
        user_id,
        "entry:briefing-test-1",
        date("2026-08-01"),
        &first_sections,
        &first_omitted,
    )
    .await
    .expect("apply first edition");
    tx.commit().await.expect("commit first edition");
    assert_eq!(skipped, 1, "the malformed URL is skipped, not fatal");

    let alpha = story_row(&pool, user_id, "story-alpha").await;
    assert_eq!(alpha.0, "Alpha story");
    assert_eq!(alpha.1, "ai");
    assert_eq!(alpha.2, vec!["Alpha".to_owned()]);
    assert_eq!(alpha.3.as_deref(), Some("2026-07-30"));
    assert_eq!(alpha.4.as_deref(), Some("2026-08-01"));
    assert_eq!(alpha.5.as_deref(), Some("entry:briefing-test-1"));
    assert_eq!(alpha.6.as_deref(), Some("**Alpha headline.**"));
    assert_eq!((alpha.7, alpha.8), (1, 0));
    assert_eq!(alpha.9, "active");

    let beta = story_row(&pool, user_id, "story-beta").await;
    assert_eq!(beta.0, "Beta story");
    assert_eq!(beta.1, "ai");
    assert_eq!(beta.4, None, "corroboration never sets delivery fields");
    assert_eq!((beta.7, beta.8), (0, 0));

    let gamma = story_row(&pool, user_id, "story-gamma").await;
    assert_eq!(gamma.0, "");
    assert_eq!(gamma.4, None, "suppression never sets delivery fields");
    assert_eq!((gamma.7, gamma.8), (0, 1));

    let alpha_canonical =
        canonicalize_url("HTTPS://Example.COM/alpha?utm_source=x&b=2&a=1").expect("canonical url");
    assert_eq!(alpha_canonical, "https://example.com/alpha?a=1&b=2");
    let urls = url_rows(&pool, user_id).await;
    assert_eq!(
        urls,
        vec![
            (
                "story-alpha".to_owned(),
                story_url_hash(&alpha_canonical),
                alpha_canonical.clone(),
            ),
            (
                "story-beta".to_owned(),
                story_url_hash("https://example.com/beta"),
                "https://example.com/beta".to_owned(),
            ),
            (
                "story-gamma".to_owned(),
                story_url_hash("https://example.com/gamma"),
                "https://example.com/gamma".to_owned(),
            ),
        ],
    );

    let second_sections = sections_fixture(json!([
        {
            "topic": "ai-follow-up",
            "title": "AI follow-up",
            "items": [
                {
                    "id": "alpha-item",
                    "kind": "news",
                    "headline_md": "**Alpha updated.**",
                    "delta": "update",
                    "story": {
                        "key": "story-alpha",
                        "urls": ["https://example.com/alpha?b=2&a=1"]
                    }
                }
            ]
        }
    ]));
    let second_omitted = omissions_fixture(json!([
        {"story_key": "story-gamma", "reason": "still no material delta"}
    ]));

    let mut tx = pool.begin().await.expect("begin second edition");
    let skipped = apply_edition_to_ledger(
        &mut tx,
        user_id,
        "entry:briefing-test-2",
        date("2026-08-02"),
        &second_sections,
        &second_omitted,
    )
    .await
    .expect("apply second edition");
    tx.commit().await.expect("commit second edition");
    assert_eq!(skipped, 0);

    let alpha = story_row(&pool, user_id, "story-alpha").await;
    assert_eq!(alpha.0, "Alpha story", "empty update preserves the title");
    assert_eq!(alpha.1, "ai-follow-up");
    assert_eq!(alpha.3.as_deref(), Some("2026-07-30"));
    assert_eq!(alpha.4.as_deref(), Some("2026-08-02"));
    assert_eq!(alpha.5.as_deref(), Some("entry:briefing-test-2"));
    assert_eq!(alpha.6.as_deref(), Some("**Alpha updated.**"));
    assert_eq!((alpha.7, alpha.8), (2, 0));

    let gamma = story_row(&pool, user_id, "story-gamma").await;
    assert_eq!((gamma.7, gamma.8), (0, 2));

    let urls = url_rows(&pool, user_id).await;
    assert_eq!(urls.len(), 3, "a re-seen URL hash is not duplicated");
}

#[tokio::test]
async fn rebuild_matches_live_order_and_backfill_never_regresses_delivery() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    let evening_entry = Uuid::now_v7();
    let story = |headline: &str, urls: serde_json::Value| {
        json!([
            {
                "topic": "ai",
                "title": "AI",
                "items": [
                    {
                        "id": "x-item",
                        "kind": "news",
                        "headline_md": headline,
                        "delta": "new",
                        "story": {"key": "story-x", "urls": urls, "title": "Story X"}
                    }
                ]
            }
        ])
    };
    let editions = [
        EditionFixture {
            entry_id: Uuid::now_v7(),
            path: "Briefings/2026/Morning briefing - 2026-08-01.md",
            version: 1,
            date: "2026-08-01",
            sections: story("**Morning.**", json!(["https://example.com/x-morning"])),
            omitted: json!([]),
        },
        // Same date, published second: with created_at replay order the
        // evening delivery must win in both live and rebuilt ledgers, even
        // though its path sorts before the morning edition's.
        EditionFixture {
            entry_id: evening_entry,
            path: "Briefings/2026/Evening briefing - 2026-08-01.md",
            version: 1,
            date: "2026-08-01",
            sections: story("**Evening.**", json!([])),
            omitted: json!([]),
        },
        // Backfilled older edition published last: delivery_count grows but
        // last_delivered_* must not regress to the older date.
        EditionFixture {
            entry_id: Uuid::now_v7(),
            path: "Briefings/2026/Morning briefing - 2026-07-20.md",
            version: 1,
            date: "2026-07-20",
            sections: story("**Old.**", json!(["https://example.com/x-old"])),
            omitted: json!([]),
        },
    ];
    for edition in &editions {
        let mut tx = pool.begin().await.expect("begin live apply");
        apply_edition_to_ledger(
            &mut tx,
            user_id,
            &format!("entry:{}", edition.entry_id),
            date(edition.date),
            &sections_fixture(edition.sections.clone()),
            &omissions_fixture(edition.omitted.clone()),
        )
        .await
        .expect("apply edition live");
        tx.commit().await.expect("commit live apply");
        insert_edition_version(&pool, user_id, edition).await;
    }

    let live_story = story_row(&pool, user_id, "story-x").await;
    assert_eq!(live_story.4.as_deref(), Some("2026-08-01"));
    assert_eq!(
        live_story.5.as_deref(),
        Some(format!("entry:{evening_entry}").as_str()),
        "the same-date evening publish wins the delivery pointer",
    );
    assert_eq!(live_story.6.as_deref(), Some("**Evening.**"));
    assert_eq!(
        live_story.7, 3,
        "the backfilled delivery still counts even though it never wins the pointer",
    );
    let live = ledger_snapshot(&pool, user_id).await;

    let mut tx = pool.begin().await.expect("begin rebuild");
    let rebuild = rebuild_briefing_ledger(&mut tx, user_id)
        .await
        .expect("rebuild ledger");
    tx.commit().await.expect("commit rebuild");
    assert_eq!(rebuild.replayed_versions, 3);

    let rebuilt = ledger_snapshot(&pool, user_id).await;
    assert_eq!(
        rebuilt, live,
        "created_at replay order reproduces the live ledger exactly",
    );
}

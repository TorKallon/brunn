use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

use straylight::{document_service::get_document_in_tx, error::ApiError};

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping document endpoint test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
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
        .bind(format!("document-endpoint-test:{user_id}"))
        .bind("Document endpoint test")
        .execute(pool)
        .await
        .expect("insert test user");
    user_id
}

async fn insert_entry_version(
    pool: &PgPool,
    user_id: Uuid,
    entry_id: Uuid,
    version: i64,
    content: &str,
    metadata: Value,
) {
    let mut tx = pool.begin().await.expect("begin entry insert");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,'Documents/trip-plan.md','Trip plan','markdown','text/markdown',$3)
        ON CONFLICT (user_id,(lower(normalize(path, NFC)))) DO UPDATE
        SET current_version=EXCLUDED.current_version
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(version)
    .execute(&mut *tx)
    .await
    .expect("insert or advance entry");
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

fn document_metadata(summary: &str) -> Value {
    json!({
        "kind": "human_document",
        "document": {
            "schema": "document.v1",
            "slug": "trip-plan",
            "title": "Trip plan",
            "summary": summary,
            "sources": []
        }
    })
}

#[tokio::test]
async fn reads_only_marked_history_and_unmarked_current_head_unpublishes() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    let other_user_id = insert_test_user(&pool).await;
    let entry_id = Uuid::now_v7();
    insert_entry_version(&pool, user_id, entry_id, 1, "raw predecessor\n", json!({})).await;
    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        2,
        "# Trip plan\n\nFirst body.\n",
        document_metadata("First summary."),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        3,
        "# Trip plan\n\nCurrent body.\n",
        document_metadata("Current summary."),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        4,
        "uncurated later workspace edit\n",
        json!({}),
    )
    .await;

    let mut tx = pool.begin().await.expect("begin hidden-head read");
    let error = get_document_in_tx(
        &mut tx,
        user_id,
        "https://straylight.example",
        "trip-plan",
        None,
    )
    .await
    .expect_err("an unmarked current head must unpublish the stable route");
    assert!(matches!(
        error,
        ApiError::Public {
            code: "document_not_found",
            ..
        }
    ));
    tx.rollback().await.expect("rollback hidden-head read");

    sqlx::query("UPDATE straylight.entries SET current_version=3 WHERE user_id=$1 AND id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&pool)
        .await
        .expect("restore marked current head");
    let mut tx = pool.begin().await.expect("begin document reads");
    let current = get_document_in_tx(
        &mut tx,
        user_id,
        "https://straylight.example",
        "trip-plan",
        None,
    )
    .await
    .expect("current document loads");
    assert_eq!(current["version"], 3);
    assert_eq!(current["current_version"], 3);
    assert_eq!(current["body_md"], "Current body.");
    assert_eq!(current["summary"], "Current summary.");
    assert_eq!(
        current["versions"]
            .as_array()
            .expect("versions array")
            .iter()
            .map(|item| item["version"].as_i64().expect("numeric version"))
            .collect::<Vec<_>>(),
        [2, 3],
        "the raw predecessor must not be promoted into document history",
    );
    assert_eq!(
        current["version_url"],
        "https://straylight.example/documents/trip-plan?version=3",
    );

    let historical = get_document_in_tx(
        &mut tx,
        user_id,
        "https://straylight.example",
        "trip-plan",
        Some(2),
    )
    .await
    .expect("marked historical version loads");
    assert_eq!(historical["body_md"], "First body.");

    for (reader, version) in [(user_id, Some(1)), (other_user_id, None)] {
        let error = get_document_in_tx(
            &mut tx,
            reader,
            "https://straylight.example",
            "trip-plan",
            version,
        )
        .await
        .expect_err("unmarked or cross-user document reads must be hidden");
        assert!(matches!(
            error,
            ApiError::Public {
                code: "document_not_found",
                ..
            }
        ));
    }
    tx.commit().await.expect("commit document reads");
}

use brunn::retrieval_sql::SIMPLE_ENTRY_LINK_CANDIDATES_SQL;
use chrono::{Duration, Utc};
use sqlx::{AssertSqlSafe, PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping search sort database test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");
    Some(pool)
}

async fn insert_link_entry(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    path: &str,
    title: &str,
    ordinal: u128,
) -> Uuid {
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let content = format!("link fixture {ordinal}");
    sqlx::query(
        "INSERT INTO brunn.entries \
         (id,user_id,path,title,kind,media_type,current_version) \
         VALUES ($1,$2,$3,$4,'markdown','text/markdown',0)",
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(path)
    .bind(title)
    .execute(&mut **tx)
    .await
    .expect("insert link fixture entry");
    sqlx::query(
        "INSERT INTO brunn.entry_versions \
         (id,user_id,entry_id,version,content_sha256,content,size_bytes) \
         VALUES ($1,$2,$3,1,$4,$5,$6)",
    )
    .bind(version_id)
    .bind(user_id)
    .bind(entry_id)
    .bind(format!("{ordinal:064x}"))
    .bind(&content)
    .bind(content.len() as i64)
    .execute(&mut **tx)
    .await
    .expect("insert link fixture version");
    sqlx::query("UPDATE brunn.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&mut **tx)
        .await
        .expect("activate link fixture version");
    entry_id
}

async fn explain_link_lookup(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    filename_keys: &[String],
) -> String {
    let statement = format!("EXPLAIN (COSTS OFF) {SIMPLE_ENTRY_LINK_CANDIDATES_SQL}");
    sqlx::query(AssertSqlSafe(statement))
        .bind(user_id)
        .bind(filename_keys)
        .fetch_all(&mut **tx)
        .await
        .expect("explain normalized-basename lookup")
        .into_iter()
        .map(|row| row.get::<String, _>("QUERY PLAN"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn non_relevance_sorts_select_before_the_bounded_candidate_cutoff() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let mut tx = pool.begin().await.expect("begin fixture transaction");
    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("search-sort-test:{user_id}"))
        .bind("Search sort integration test")
        .execute(&mut *tx)
        .await
        .expect("insert search sort test user");

    let base_time = Utc::now() - Duration::days(400);
    for ordinal in 1_u128..=300 {
        let entry_id = Uuid::from_u128(ordinal);
        let version_id = Uuid::from_u128(10_000 + ordinal);
        let path = format!("Notes/{ordinal:03}.md");
        let title = if ordinal == 1 {
            "Aardvark".to_owned()
        } else {
            format!("Note {ordinal:03}")
        };
        let content = format!("needle fixture {ordinal}");
        let content_hash = format!("{ordinal:064x}");
        let updated_at = base_time + Duration::days(ordinal as i64);
        sqlx::query(
            "INSERT INTO brunn.entries \
             (id,user_id,path,title,kind,media_type,current_version,updated_at) \
             VALUES ($1,$2,$3,$4,'markdown','text/markdown',0,$5)",
        )
        .bind(entry_id)
        .bind(user_id)
        .bind(&path)
        .bind(&title)
        .bind(updated_at)
        .execute(&mut *tx)
        .await
        .expect("insert fixture entry");
        sqlx::query(
            "INSERT INTO brunn.entry_versions \
             (id,user_id,entry_id,version,content_sha256,content,size_bytes) \
             VALUES ($1,$2,$3,1,$4,$5,$6)",
        )
        .bind(version_id)
        .bind(user_id)
        .bind(entry_id)
        .bind(&content_hash)
        .bind(&content)
        .bind(content.len() as i64)
        .execute(&mut *tx)
        .await
        .expect("insert fixture version");
        sqlx::query("UPDATE brunn.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
            .bind(user_id)
            .bind(entry_id)
            .execute(&mut *tx)
            .await
            .expect("activate fixture version");
        sqlx::query(
            "INSERT INTO brunn.search_chunks \
             (id,user_id,entry_id,entry_version_id,chunk_index,path,heading,content,token_estimate) \
             VALUES ($1,$2,$3,$4,0,$5,'',$6,3)",
        )
        .bind(Uuid::from_u128(20_000 + ordinal))
        .bind(user_id)
        .bind(entry_id)
        .bind(version_id)
        .bind(&path)
        .bind(&content)
        .execute(&mut *tx)
        .await
        .expect("insert fixture search chunk");
        sqlx::query(
            "INSERT INTO brunn.workspace_changes \
             (user_id,entry_id,entry_version,operation,path,content_sha256) \
             VALUES ($1,$2,1,'create',$3,$4)",
        )
        .bind(user_id)
        .bind(entry_id)
        .bind(&path)
        .bind(&content_hash)
        .execute(&mut *tx)
        .await
        .expect("insert fixture workspace change");
    }
    sqlx::query(
        "SELECT set_config('app.current_user_id',$1,true), \
                set_config('app.current_credential_id',$2,true), \
                set_config('app.context_valid','true',true)",
    )
    .bind(user_id.to_string())
    .bind(credential_id.to_string())
    .execute(&mut *tx)
    .await
    .expect("establish validated request context");

    let legacy_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.workspace_lexical_candidates('needle')",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("old function remains callable for rolling deployments");
    assert!(legacy_count > 0);

    let modified = sqlx::query(
        "SELECT path FROM brunn.workspace_lexical_candidates_v2('needle','last_modified')",
    )
    .fetch_all(&mut *tx)
    .await
    .expect("fetch modified-date candidates");
    assert_eq!(modified.len(), 64);
    assert_eq!(modified[0].get::<String, _>("path"), "Notes/300.md");

    let title =
        sqlx::query("SELECT path FROM brunn.workspace_lexical_candidates_v2('needle','title')")
            .fetch_all(&mut *tx)
            .await
            .expect("fetch title candidates");
    assert_eq!(title.len(), 64);
    assert_eq!(title[0].get::<String, _>("path"), "Notes/001.md");

    insert_link_entry(
        &mut tx,
        user_id,
        "sources/Knowledge/Roadmap.md",
        "Roadmap",
        30_001,
    )
    .await;
    insert_link_entry(&mut tx, user_id, "sources/Other/Plan.md", "Roadmap", 30_002).await;
    let filename_keys = vec![
        "roadmap".to_owned(),
        "roadmap.md".to_owned(),
        "roadmap.markdown".to_owned(),
    ];
    let unique_link = sqlx::query(SIMPLE_ENTRY_LINK_CANDIDATES_SQL)
        .bind(user_id)
        .bind(&filename_keys)
        .fetch_all(&mut *tx)
        .await
        .expect("resolve a globally unique basename despite a title collision");
    assert_eq!(unique_link.len(), 1);
    assert_eq!(
        unique_link[0].get::<String, _>("path"),
        "sources/Knowledge/Roadmap.md"
    );
    assert_eq!(
        unique_link[0].get::<Option<String>, _>("content"),
        Some("link fixture 30001".to_owned())
    );

    sqlx::query("ANALYZE brunn.entries")
        .execute(&mut *tx)
        .await
        .expect("refresh entry statistics for plan assertions");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .expect("make index eligibility deterministic");
    for (label, keys) in [
        ("one match", filename_keys.clone()),
        (
            "zero matches",
            vec![
                "missing-entry".to_owned(),
                "missing-entry.md".to_owned(),
                "missing-entry.markdown".to_owned(),
            ],
        ),
    ] {
        let plan = explain_link_lookup(&mut tx, user_id, &keys).await;
        assert!(
            plan.contains("entries_user_normalized_basename_idx"),
            "{label} lookup did not use the normalized-basename index:\n{plan}"
        );
    }

    insert_link_entry(
        &mut tx,
        user_id,
        "sources/Other/Roadmap.markdown",
        "Other roadmap",
        30_003,
    )
    .await;
    let ambiguous_link = sqlx::query(SIMPLE_ENTRY_LINK_CANDIDATES_SQL)
        .bind(user_id)
        .bind(&filename_keys)
        .fetch_all(&mut *tx)
        .await
        .expect("bound an ambiguous basename lookup");
    assert_eq!(ambiguous_link.len(), 2);
    tx.rollback()
        .await
        .expect("roll back fixture and request context");
}

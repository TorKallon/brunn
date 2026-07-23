use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn corpus_revision_allocator_is_collision_free_under_concurrency() {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping Postgres allocator test");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect(&database_url)
        .await
        .expect("connect to disposable Postgres");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Straylight migrations");

    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("revision-test:{user_id}"))
        .bind("Revision allocator test")
        .execute(&pool)
        .await
        .expect("insert test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(format!("scope:revision-test:{scope_id}"))
        .bind("Revision allocator test")
        .execute(&pool)
        .await
        .expect("insert test scope");

    let base_id = Uuid::now_v7();
    let base_number = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO straylight.corpus_revisions (
          id,user_id,scope_id,parent_revision_id,revision_number,manifest_hash
        ) VALUES ($1,$2,$3,NULL,999,$4)
        RETURNING revision_number
        "#,
    )
    .bind(base_id)
    .bind(user_id)
    .bind(scope_id)
    .bind("0".repeat(64))
    .fetch_one(&pool)
    .await
    .expect("insert base revision");
    assert_eq!(base_number, 1, "the trigger owns even the first allocation");

    let mut tasks = Vec::new();
    for index in 0..24_u8 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            sqlx::query_scalar::<_, i64>(
                r#"
                INSERT INTO straylight.corpus_revisions (
                  id,user_id,scope_id,parent_revision_id,revision_number,manifest_hash
                ) VALUES ($1,$2,$3,$4,2,$5)
                RETURNING revision_number
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(scope_id)
            .bind(base_id)
            .bind(format!("{index:064x}"))
            .fetch_one(&pool)
            .await
        }));
    }
    let mut allocated = Vec::new();
    for task in tasks {
        allocated.push(
            task.await
                .expect("allocator task joined")
                .expect("concurrent revision inserted"),
        );
    }
    allocated.sort_unstable();
    assert_eq!(allocated, (2_i64..=25_i64).collect::<Vec<_>>());
}

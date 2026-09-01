use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use brunn::{
    auth::AuthContext,
    db::set_context,
    models::{CredentialId, UserId},
};

struct Principal {
    auth: AuthContext,
    user_id: Uuid,
    credential_id: Uuid,
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping dashboard telemetry test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");
    Some(pool)
}

async fn insert_principal(pool: &PgPool, label: &str) -> Principal {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:telemetry-{scope_id}");
    let capabilities = vec![
        "open".to_owned(),
        "query".to_owned(),
        "read".to_owned(),
        "status".to_owned(),
    ];
    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("telemetry-test:{user_id}"))
        .bind(format!("Telemetry test {label}"))
        .execute(pool)
        .await
        .expect("insert telemetry test user");
    sqlx::query("INSERT INTO brunn.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Telemetry test {label}"))
        .execute(pool)
        .await
        .expect("insert telemetry test scope");
    sqlx::query(
        r#"
        INSERT INTO brunn.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Telemetry test {label}"))
    .bind(format!("telemetry-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert telemetry test credential");
    sqlx::query(
        r#"
        INSERT INTO brunn.credential_scope_grants (
          credential_id,user_id,scope_id
        ) VALUES ($1,$2,$3)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant telemetry test scope");
    Principal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: true,
        },
        user_id,
        credential_id,
    }
}

async fn insert_entry(pool: &PgPool, user_id: Uuid, label: &str) -> Uuid {
    let entry_id = Uuid::now_v7();
    let mut tx = pool.begin().await.expect("begin telemetry entry insert");
    sqlx::query(
        r#"
        INSERT INTO brunn.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,$4,'markdown','text/markdown',1)
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(format!("telemetry/{entry_id}.md"))
    .bind(label)
    .execute(&mut *tx)
    .await
    .expect("insert telemetry test entry");
    sqlx::query(
        r#"
        INSERT INTO brunn.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes
        ) VALUES ($1,$2,$3,1,$4,$5,$6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(entry_id)
    .bind("0".repeat(64))
    .bind(label)
    .bind(i64::try_from(label.len()).expect("label length fits"))
    .execute(&mut *tx)
    .await
    .expect("insert telemetry test entry version");
    tx.commit().await.expect("commit telemetry entry insert");
    entry_id
}

async fn begin_as_app_rw<'a>(
    pool: &'a PgPool,
    auth: &AuthContext,
) -> Transaction<'a, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("begin app_rw transaction");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .expect("assume app_rw role");
    set_context(&mut tx, auth)
        .await
        .expect("establish validated app_rw context");
    tx
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .map(|code| code.into_owned())
}

async fn write_product(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    principal: &Principal,
    bucket: DateTime<Utc>,
    operation: &str,
    operation_count: i64,
    byte_count: i64,
    first_recorded_at: DateTime<Utc>,
    last_recorded_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        SELECT brunn_auth.write_product_activity(
          $1,$2,$3,$4,$5,$6,$7,$8
        )
        "#,
    )
    .bind(principal.user_id)
    .bind(principal.credential_id)
    .bind(vec![bucket])
    .bind(vec![operation])
    .bind(vec![operation_count])
    .bind(vec![byte_count])
    .bind(vec![first_recorded_at])
    .bind(vec![last_recorded_at])
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[tokio::test]
async fn telemetry_writers_are_exact_principal_validated_and_additive() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let first = insert_principal(&pool, "first").await;
    let second = insert_principal(&pool, "second").await;
    let first_entry = insert_entry(&pool, first.user_id, "First telemetry entry").await;

    let privileges = sqlx::query(
        r#"
        SELECT
          count(*) AS writer_count,
          bool_and(has_function_privilege('app_rw', procedure.oid, 'EXECUTE'))
            AS rw_can_execute,
          bool_or(has_function_privilege('app_ro', procedure.oid, 'EXECUTE'))
            AS ro_can_execute
        FROM pg_proc AS procedure
        JOIN pg_namespace AS namespace ON namespace.oid=procedure.pronamespace
        WHERE namespace.nspname='brunn_auth'
          AND procedure.proname IN (
            'write_entry_usage',
            'write_product_activity',
            'write_credential_activity'
          )
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read telemetry writer privileges");
    assert_eq!(privileges.try_get::<i64, _>("writer_count").unwrap(), 3);
    assert!(privileges.try_get::<bool, _>("rw_can_execute").unwrap());
    assert!(!privileges.try_get::<bool, _>("ro_can_execute").unwrap());

    let bucket: DateTime<Utc> = "2026-08-03T03:10:00Z".parse().unwrap();
    let first_seen: DateTime<Utc> = "2026-08-03T03:10:10Z".parse().unwrap();
    let last_seen: DateTime<Utc> = "2026-08-03T03:10:20Z".parse().unwrap();
    let earlier_seen: DateTime<Utc> = "2026-08-03T03:10:05Z".parse().unwrap();
    let later_seen: DateTime<Utc> = "2026-08-03T03:10:25Z".parse().unwrap();

    let mut direct_tx = begin_as_app_rw(&pool, &first.auth).await;
    let direct_error = sqlx::query(
        r#"
        INSERT INTO brunn.product_activity_minutely (
          user_id,credential_id,bucket_start,operation,
          operation_count,byte_count,first_recorded_at,last_recorded_at
        ) VALUES ($1,$2,$3,'read',1,1,$4,$4)
        "#,
    )
    .bind(first.user_id)
    .bind(first.credential_id)
    .bind(bucket)
    .bind(first_seen)
    .execute(&mut *direct_tx)
    .await
    .expect_err("app_rw must not write activity tables directly");
    assert_eq!(database_code(&direct_error).as_deref(), Some("42501"));
    drop(direct_tx);

    let mut valid_tx = begin_as_app_rw(&pool, &first.auth).await;
    write_product(
        &mut valid_tx,
        &first,
        bucket,
        "read",
        2,
        10,
        first_seen,
        last_seen,
    )
    .await
    .expect("write first product activity batch");
    write_product(
        &mut valid_tx,
        &first,
        bucket,
        "read",
        3,
        4,
        earlier_seen,
        later_seen,
    )
    .await
    .expect("add second product activity batch");
    sqlx::query("SELECT brunn_auth.write_entry_usage($1,$2,$3,$4,$5)")
        .bind(first.user_id)
        .bind(first.credential_id)
        .bind(vec![first_entry])
        .bind(vec![2_i64])
        .bind(vec![1_i64])
        .execute(&mut *valid_tx)
        .await
        .expect("write entry usage");
    sqlx::query("SELECT brunn_auth.write_credential_activity($1,$2,$3,$4,$5)")
        .bind(first.user_id)
        .bind(first.credential_id)
        .bind("read")
        .bind(last_seen)
        .bind(2_i64)
        .execute(&mut *valid_tx)
        .await
        .expect("write credential activity");
    sqlx::query("SELECT brunn_auth.write_credential_activity($1,$2,$3,$4,$5)")
        .bind(first.user_id)
        .bind(first.credential_id)
        .bind("search")
        .bind(first_seen)
        .bind(3_i64)
        .execute(&mut *valid_tx)
        .await
        .expect("add older credential activity");
    valid_tx.commit().await.expect("commit valid telemetry");

    let product = sqlx::query(
        r#"
        SELECT operation_count,byte_count,first_recorded_at,last_recorded_at
        FROM brunn.product_activity_minutely
        WHERE user_id=$1 AND credential_id=$2
          AND bucket_start=$3 AND operation='read'
        "#,
    )
    .bind(first.user_id)
    .bind(first.credential_id)
    .bind(bucket)
    .fetch_one(&pool)
    .await
    .expect("read product activity result");
    assert_eq!(product.try_get::<i64, _>("operation_count").unwrap(), 5);
    assert_eq!(product.try_get::<i64, _>("byte_count").unwrap(), 14);
    assert_eq!(
        product
            .try_get::<DateTime<Utc>, _>("first_recorded_at")
            .unwrap(),
        earlier_seen
    );
    assert_eq!(
        product
            .try_get::<DateTime<Utc>, _>("last_recorded_at")
            .unwrap(),
        later_seen
    );
    let entry_usage = sqlx::query(
        "SELECT read_count,search_count FROM brunn.entry_usage WHERE user_id=$1 AND entry_id=$2",
    )
    .bind(first.user_id)
    .bind(first_entry)
    .fetch_one(&pool)
    .await
    .expect("read entry usage result");
    assert_eq!(entry_usage.try_get::<i64, _>("read_count").unwrap(), 2);
    assert_eq!(entry_usage.try_get::<i64, _>("search_count").unwrap(), 1);
    let credential = sqlx::query(
        r#"
        SELECT last_operation,last_used_at,request_count
        FROM brunn.credential_activity
        WHERE user_id=$1 AND credential_id=$2
        "#,
    )
    .bind(first.user_id)
    .bind(first.credential_id)
    .fetch_one(&pool)
    .await
    .expect("read credential activity result");
    assert_eq!(
        credential.try_get::<String, _>("last_operation").unwrap(),
        "read"
    );
    assert_eq!(
        credential
            .try_get::<DateTime<Utc>, _>("last_used_at")
            .unwrap(),
        last_seen
    );
    assert_eq!(credential.try_get::<i64, _>("request_count").unwrap(), 5);

    let mut cross_tx = begin_as_app_rw(&pool, &first.auth).await;
    let cross_error = write_product(
        &mut cross_tx,
        &second,
        bucket,
        "read",
        1,
        1,
        first_seen,
        first_seen,
    )
    .await
    .expect_err("one principal context must not write another principal's telemetry");
    assert_eq!(database_code(&cross_error).as_deref(), Some("42501"));
    drop(cross_tx);

    let mut invalid_tx = begin_as_app_rw(&pool, &first.auth).await;
    let invalid_error = write_product(
        &mut invalid_tx,
        &first,
        bucket,
        "not_allowed",
        1,
        1,
        first_seen,
        first_seen,
    )
    .await
    .expect_err("invalid telemetry operations must fail closed");
    assert_eq!(database_code(&invalid_error).as_deref(), Some("22023"));
    drop(invalid_tx);

    let mut missing_context_tx = pool
        .begin()
        .await
        .expect("begin missing-context transaction");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *missing_context_tx)
        .await
        .expect("assume app_rw without context");
    let missing_context_error = write_product(
        &mut missing_context_tx,
        &first,
        bucket,
        "read",
        1,
        1,
        first_seen,
        first_seen,
    )
    .await
    .expect_err("telemetry writes require validated context");
    assert_eq!(
        database_code(&missing_context_error).as_deref(),
        Some("42501")
    );
    drop(missing_context_tx);

    let mut stale_context_tx = begin_as_app_rw(&pool, &second.auth).await;
    sqlx::query("UPDATE brunn.api_credentials SET disabled_at=clock_timestamp() WHERE id=$1")
        .bind(second.credential_id)
        .execute(&pool)
        .await
        .expect("disable second telemetry credential");
    let stale_context_error = write_product(
        &mut stale_context_tx,
        &second,
        bucket,
        "read",
        1,
        1,
        first_seen,
        first_seen,
    )
    .await
    .expect_err("the writer must revalidate a credential disabled after context setup");
    assert_eq!(
        database_code(&stale_context_error).as_deref(),
        Some("42501")
    );
    drop(stale_context_tx);

    let mut revoked_tx = pool
        .begin()
        .await
        .expect("begin revoked-context transaction");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *revoked_tx)
        .await
        .expect("assume app_rw for revoked context");
    assert!(set_context(&mut revoked_tx, &second.auth).await.is_err());
}

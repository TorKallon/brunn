use std::collections::HashSet;

use brunn::{
    auth::AuthContext,
    db::set_context,
    models::{CredentialId, UserId},
};
use chrono::{Duration, Utc};
use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

struct Principal {
    auth: AuthContext,
    user_id: Uuid,
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping location RLS database test");
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

async fn insert_user(pool: &PgPool, label: &str) -> (Uuid, Uuid, String) {
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:location-rls-{scope_id}");
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("location-rls:{label}:{user_id}"))
        .bind(format!("Location RLS {label}"))
        .execute(pool)
        .await
        .expect("insert location RLS user");
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Location RLS {label}"))
        .execute(pool)
        .await
        .expect("insert location RLS scope");
    (user_id, scope_id, scope_ref)
}

async fn insert_credential(
    pool: &PgPool,
    user_id: Uuid,
    scope_id: Uuid,
    scope_ref: &str,
    label: &str,
    capabilities: &[&str],
) -> Principal {
    let credential_id = Uuid::now_v7();
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) \
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(label)
    .bind(format!("location-rls-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert location RLS credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) \
         VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant location RLS scope");
    let read_only = !capabilities
        .iter()
        .any(|capability| capability == "save" || capability == "checkpoint");
    Principal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref.to_owned()],
            read_only,
        },
        user_id,
    }
}

async fn begin_as<'a>(
    pool: &'a PgPool,
    auth: &AuthContext,
    role: &str,
) -> Transaction<'a, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("begin location RLS transaction");
    let set_role = match role {
        "app_rw" => "SET LOCAL ROLE app_rw",
        "app_ro" => "SET LOCAL ROLE app_ro",
        unexpected => panic!("unsupported test role {unexpected}"),
    };
    sqlx::query(set_role)
        .execute(&mut *tx)
        .await
        .expect("assume application role");
    set_context(&mut tx, auth)
        .await
        .expect("install validated location RLS context");
    tx
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|value| value.into_owned())
}

#[tokio::test]
async fn location_tables_enforce_capability_tenancy_and_role_boundaries() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let (user_id, scope_id, scope_ref) = insert_user(&pool, "owner").await;
    let location = insert_credential(
        &pool,
        user_id,
        scope_id,
        &scope_ref,
        "location device",
        &[
            "open",
            "query",
            "read",
            "compute",
            "verify",
            "status",
            "task.read",
            "location.write",
        ],
    )
    .await;
    let saver = insert_credential(
        &pool,
        user_id,
        scope_id,
        &scope_ref,
        "location rederive",
        &["read", "save"],
    )
    .await;
    let reader = insert_credential(
        &pool,
        user_id,
        scope_id,
        &scope_ref,
        "location reader",
        &["read"],
    )
    .await;
    let (neighbor_user_id, neighbor_scope_id, neighbor_scope_ref) =
        insert_user(&pool, "neighbor").await;
    let neighbor = insert_credential(
        &pool,
        neighbor_user_id,
        neighbor_scope_id,
        &neighbor_scope_ref,
        "neighbor reader",
        &["read", "save"],
    )
    .await;

    let security = sqlx::query(
        r#"
        SELECT relname,relrowsecurity,relforcerowsecurity
        FROM pg_class
        JOIN pg_namespace ON pg_namespace.oid=pg_class.relnamespace
        WHERE pg_namespace.nspname='brunn'
          AND relname=ANY($1)
        ORDER BY relname
        "#,
    )
    .bind(vec![
        "location_presence".to_owned(),
        "location_report_poi".to_owned(),
        "location_reports".to_owned(),
    ])
    .fetch_all(&pool)
    .await
    .expect("inspect location RLS state");
    assert_eq!(security.len(), 3);
    for row in security {
        assert!(row.get::<bool, _>("relrowsecurity"));
        assert!(row.get::<bool, _>("relforcerowsecurity"));
    }

    let at = Utc::now() - Duration::hours(1);
    let mut tx = begin_as(&pool, &location.auth, "app_rw").await;
    sqlx::query(
        r#"
        INSERT INTO brunn.location_reports(
          user_id,at,type,offset_min,lat,lon,accuracy_m,arrived_at,departed_at,
          city,region,country,name
        ) VALUES($1,$2,'visit_departure',-420,47.6,-122.2,20,$3,$4,
                 'canary-city','WA','US','canary-label')
        "#,
    )
    .bind(location.user_id)
    .bind(at)
    .bind(at - Duration::minutes(30))
    .bind(at - Duration::minutes(1))
    .execute(&mut *tx)
    .await
    .expect("location.write inserts raw evidence");
    sqlx::query(
        "INSERT INTO brunn.location_report_poi(user_id,at,type,rank,name,category,distance_m) \
         VALUES($1,$2,'visit_departure',1,'canary-poi','restaurant',12)",
    )
    .bind(location.user_id)
    .bind(at)
    .execute(&mut *tx)
    .await
    .expect("location.write inserts raw POI evidence");
    sqlx::query(
        r#"
        INSERT INTO brunn.location_presence(
          user_id,timezone,reported_at,last_lat,last_lon,last_accuracy_m,
          city,region,country
        ) VALUES($1,'America/Los_Angeles',$2,47.6,-122.2,20,
                 'canary-city','WA','US')
        "#,
    )
    .bind(location.user_id)
    .bind(at)
    .execute(&mut *tx)
    .await
    .expect("location.write inserts derived presence");
    tx.commit().await.expect("commit location evidence fixture");

    let mut tx = begin_as(&pool, &location.auth, "app_rw").await;
    let presence_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.location_presence WHERE user_id=$1",
    )
    .bind(location.user_id)
    .fetch_one(&mut *tx)
    .await
    .expect("location credential reads its presence");
    let hidden_reports =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.location_reports")
            .fetch_one(&mut *tx)
            .await
            .expect("location credential can query the raw table only through its denied policy");
    let hidden_poi = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.location_report_poi")
        .fetch_one(&mut *tx)
        .await
        .expect("location credential can query the POI table only through its denied policy");
    assert_eq!(presence_count, 1);
    assert_eq!(
        hidden_reports, 0,
        "device credentials cannot read raw evidence"
    );
    assert_eq!(
        hidden_poi, 0,
        "device credentials cannot read raw POI evidence"
    );
    tx.rollback().await.expect("finish location read");

    let mut tx = begin_as(&pool, &saver.auth, "app_rw").await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.location_reports")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.location_report_poi")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    tx.rollback().await.expect("finish rederive read");

    let mut tx = begin_as(&pool, &neighbor.auth, "app_rw").await;
    let neighbor_visible =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.location_presence")
            .fetch_one(&mut *tx)
            .await
            .expect("neighbor reads only its tenant");
    assert_eq!(neighbor_visible, 0);
    tx.rollback().await.expect("finish neighbor read");

    let mut tx = begin_as(&pool, &reader.auth, "app_rw").await;
    let denied = sqlx::query("DELETE FROM brunn.location_presence WHERE user_id=$1")
        .bind(reader.user_id)
        .execute(&mut *tx)
        .await
        .expect("a denied RLS delete affects no rows");
    assert_eq!(denied.rows_affected(), 0);
    tx.rollback().await.expect("finish denied RLS delete");

    let mut tx = begin_as(&pool, &reader.auth, "app_ro").await;
    let app_ro_visible = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.location_presence WHERE user_id=$1",
    )
    .bind(reader.user_id)
    .fetch_one(&mut *tx)
    .await
    .expect("app_ro can read owner presence");
    assert_eq!(app_ro_visible, 1);
    let write_error = sqlx::query("DELETE FROM brunn.location_presence WHERE user_id=$1")
        .bind(reader.user_id)
        .execute(&mut *tx)
        .await
        .expect_err("app_ro has no location mutation grant");
    assert_eq!(database_code(&write_error).as_deref(), Some("42501"));
    tx.rollback().await.expect("finish app_ro check");

    pool.close().await;
}

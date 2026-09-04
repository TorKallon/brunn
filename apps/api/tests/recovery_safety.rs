use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn recovery_remap_is_narrow_idempotent_and_identity_bound() {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping recovery safety test");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");

    let mut role_check = pool.begin().await.expect("begin role check");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *role_check)
        .await
        .expect("assume app_rw for trigger predicate check");
    let app_role_is_admin = sqlx::query_scalar::<_, bool>("SELECT brunn.database_administrator()")
        .fetch_one(&mut *role_check)
        .await
        .expect("app_rw can evaluate administrator predicate");
    assert!(!app_role_is_admin);
    let app_role_authorized = sqlx::query_scalar::<_, bool>(
        "SELECT brunn.asset_internal_operation_authorized(
           'restore_locator_remap',$1,$2,1
         )",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .fetch_one(&mut *role_check)
    .await
    .expect("app_rw can evaluate internal-operation predicate");
    assert!(!app_role_authorized);
    role_check.rollback().await.expect("rollback role check");

    let retired_legacy_objects = sqlx::query_as::<_, (bool, bool)>(
        "SELECT to_regclass('brunn.corpus_revisions') IS NULL,
                to_regprocedure('brunn.expire_unpromoted_stage(uuid)') IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect retired legacy recovery objects");
    assert_eq!(retired_legacy_objects, (true, true));

    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();

    let mut setup = pool.begin().await.expect("begin fixture transaction");
    sqlx::query(
        "INSERT INTO brunn.users (id,external_ref,display_name)
         VALUES ($1,$2,$3)",
    )
    .bind(user_id)
    .bind(format!("recovery-safety:{user_id}"))
    .bind("Recovery safety test")
    .execute(&mut *setup)
    .await
    .expect("insert test user");
    let policy_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id
         FROM brunn.policies
         WHERE user_id=$1 AND is_default",
    )
    .bind(user_id)
    .fetch_one(&mut *setup)
    .await
    .expect("read seeded default policy");
    sqlx::query(
        "INSERT INTO brunn.scopes (id,user_id,scope_ref,name)
         VALUES ($1,$2,$3,$4)",
    )
    .bind(scope_id)
    .bind(user_id)
    .bind(format!("scope:recovery-safety:{scope_id}"))
    .bind("Recovery safety test")
    .execute(&mut *setup)
    .await
    .expect("insert test scope");
    setup.commit().await.expect("commit fixture");

    let invalid_asset_id = Uuid::now_v7();
    let mut invalid_version = pool.begin().await.expect("begin invalid asset test");
    sqlx::query(
        "INSERT INTO brunn.assets (
           id,user_id,scope_id,current_version,policy_id,policy_version
         ) VALUES ($1,$2,$3,1,$4,1)",
    )
    .bind(invalid_asset_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(policy_id)
    .execute(&mut *invalid_version)
    .await
    .expect("insert invalid-version test asset");
    let missing_version_id = sqlx::query(
        "INSERT INTO brunn.asset_versions (
           user_id,asset_id,version,previous_version,bucket,object_key,
           content_hash,size_bytes,media_type,object_version_id
         ) VALUES ($1,$2,1,NULL,'recovery-test',$3,$4,1,'text/plain',NULL)",
    )
    .bind(user_id)
    .bind(invalid_asset_id)
    .bind(format!("{user_id}/blobs/{}", "9".repeat(64)))
    .bind("9".repeat(64))
    .execute(&mut *invalid_version)
    .await;
    assert!(
        missing_version_id
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|error| error.code())
            .is_some_and(|code| code == "23514"),
        "new immutable asset rows must require an exact provider version ID"
    );
    invalid_version
        .rollback()
        .await
        .expect("rollback invalid asset test");

    let persistent_asset_id = Uuid::now_v7();
    let persistent_key = format!("{user_id}/blobs/{}", "4".repeat(64));
    let persistent_hash = "4".repeat(64);
    let mut persistent = pool.begin().await.expect("begin persistent asset");
    sqlx::query(
        "INSERT INTO brunn.assets (
           id,user_id,scope_id,current_version,policy_id,policy_version
         ) VALUES ($1,$2,$3,1,$4,1)",
    )
    .bind(persistent_asset_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(policy_id)
    .execute(&mut *persistent)
    .await
    .expect("insert persistent asset");
    sqlx::query(
        "INSERT INTO brunn.asset_versions (
           user_id,asset_id,version,previous_version,bucket,object_key,
           content_hash,size_bytes,media_type,object_version_id
         ) VALUES ($1,$2,1,NULL,'recovery-test',$3,$4,4,'text/plain',$5)",
    )
    .bind(user_id)
    .bind(persistent_asset_id)
    .bind(&persistent_key)
    .bind(&persistent_hash)
    .bind("source-persistent-version")
    .execute(&mut *persistent)
    .await
    .expect("insert persistent asset version");
    persistent.commit().await.expect("commit persistent asset");

    let direct_update = sqlx::query(
        "UPDATE brunn.asset_versions
         SET object_version_id='forbidden-direct-update'
         WHERE user_id=$1 AND asset_id=$2 AND version=1",
    )
    .bind(user_id)
    .bind(persistent_asset_id)
    .execute(&pool)
    .await;
    assert!(
        direct_update.is_err(),
        "ordinary updates must retain asset-version immutability"
    );

    let mapping = json!([{
        "object_key": persistent_key,
        "source_version_id": "source-persistent-version",
        "restored_version_id": "restored-persistent-version",
        "source_bucket": "recovery-test",
        "restored_bucket": "recovery-test-restored",
        "content_hash": persistent_hash,
        "size_bytes": 4
    }]);
    let updated =
        sqlx::query_scalar::<_, i64>("SELECT brunn.remap_asset_object_versions($1::jsonb)")
            .bind(&mapping)
            .fetch_one(&pool)
            .await
            .expect("perform exact locator remap");
    assert_eq!(updated, 1);
    let repeated =
        sqlx::query_scalar::<_, i64>("SELECT brunn.remap_asset_object_versions($1::jsonb)")
            .bind(&mapping)
            .fetch_one(&pool)
            .await
            .expect("repeat exact locator remap");
    assert_eq!(repeated, 0, "recovery replay must be idempotent");

    let wrong_identity = json!([{
        "object_key": persistent_key,
        "source_version_id": "source-persistent-version",
        "restored_version_id": "restored-persistent-version",
        "source_bucket": "recovery-test",
        "restored_bucket": "recovery-test-restored",
        "content_hash": "5".repeat(64),
        "size_bytes": 4
    }]);
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT brunn.remap_asset_object_versions($1::jsonb)",)
            .bind(wrong_identity)
            .fetch_one(&pool)
            .await
            .is_err(),
        "recovery must reject a changed key/hash/size identity"
    );
    let locator = sqlx::query_as::<_, (String, String)>(
        "SELECT bucket::text,object_version_id
         FROM brunn.asset_versions
         WHERE user_id=$1 AND asset_id=$2 AND version=1",
    )
    .bind(user_id)
    .bind(persistent_asset_id)
    .fetch_one(&pool)
    .await
    .expect("read remapped locator");
    assert_eq!(
        locator,
        (
            "recovery-test-restored".to_owned(),
            "restored-persistent-version".to_owned()
        )
    );
}

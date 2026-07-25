use std::time::{Duration, Instant};

use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn recovery_remap_is_narrow_and_stage_reclamation_fences_child_inserts() {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping recovery safety test");
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
        .expect("apply Straylight migrations");

    let mut role_check = pool.begin().await.expect("begin role check");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *role_check)
        .await
        .expect("assume app_rw for trigger predicate check");
    let app_role_is_admin =
        sqlx::query_scalar::<_, bool>("SELECT straylight.database_administrator()")
            .fetch_one(&mut *role_check)
            .await
            .expect("app_rw can evaluate administrator predicate");
    assert!(!app_role_is_admin);
    let app_role_authorized = sqlx::query_scalar::<_, bool>(
        "SELECT straylight.asset_internal_operation_authorized(
           'stage_reclaim',$1,$2,1
         )",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .fetch_one(&mut *role_check)
    .await
    .expect("app_rw can evaluate internal-operation predicate");
    assert!(!app_role_authorized);
    role_check.rollback().await.expect("rollback role check");

    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let corpus_revision_id = Uuid::now_v7();
    let expiring_stage_id = Uuid::now_v7();
    let competing_stage_id = Uuid::now_v7();
    let staged_asset_id = Uuid::now_v7();
    let content_hash = "1".repeat(64);
    let staged_object_key = format!("{user_id}/blobs/{}", "1".repeat(64));

    let mut setup = pool.begin().await.expect("begin fixture transaction");
    sqlx::query(
        "INSERT INTO straylight.users (id,external_ref,display_name)
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
         FROM straylight.policies
         WHERE user_id=$1 AND is_default",
    )
    .bind(user_id)
    .fetch_one(&mut *setup)
    .await
    .expect("read seeded default policy");
    sqlx::query(
        "INSERT INTO straylight.scopes (id,user_id,scope_ref,name)
         VALUES ($1,$2,$3,$4)",
    )
    .bind(scope_id)
    .bind(user_id)
    .bind(format!("scope:recovery-safety:{scope_id}"))
    .bind("Recovery safety test")
    .execute(&mut *setup)
    .await
    .expect("insert test scope");
    sqlx::query(
        "INSERT INTO straylight.api_credentials (
           id,user_id,label,token_hash,capabilities
         ) VALUES ($1,$2,$3,$4,ARRAY['stage'])",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind("Recovery safety test")
    .bind(format!("recovery-safety-token:{credential_id}"))
    .execute(&mut *setup)
    .await
    .expect("insert test credential");
    sqlx::query(
        "INSERT INTO straylight.credential_scope_grants (
           credential_id,user_id,scope_id
         ) VALUES ($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(&mut *setup)
    .await
    .expect("grant test scope");
    sqlx::query(
        "INSERT INTO straylight.corpus_revisions (
           id,user_id,scope_id,parent_revision_id,revision_number,manifest_hash
         ) VALUES ($1,$2,$3,NULL,1,$4)",
    )
    .bind(corpus_revision_id)
    .bind(user_id)
    .bind(scope_id)
    .bind("2".repeat(64))
    .execute(&mut *setup)
    .await
    .expect("insert test corpus revision");
    for (stage_id, status, created_offset, expires_offset) in [
        (expiring_stage_id, "ready", "-2 hours", "-1 hour"),
        (competing_stage_id, "uploading", "-1 minute", "1 hour"),
    ] {
        sqlx::query(
            "INSERT INTO straylight.stages (
               id,user_id,scope_id,credential_id,base_corpus_revision_id,
               input_hash,status,policy_id,policy_version,created_at,expires_at
             ) VALUES (
               $1,$2,$3,$4,$5,$6,$7,$8,1,
               clock_timestamp() + $9::interval,
               clock_timestamp() + $10::interval
             )",
        )
        .bind(stage_id)
        .bind(user_id)
        .bind(scope_id)
        .bind(credential_id)
        .bind(corpus_revision_id)
        .bind("3".repeat(64))
        .bind(status)
        .bind(policy_id)
        .bind(created_offset)
        .bind(expires_offset)
        .execute(&mut *setup)
        .await
        .expect("insert test stage");
    }
    sqlx::query(
        "INSERT INTO straylight.assets (
           id,user_id,scope_id,current_version,policy_id,policy_version
         ) VALUES ($1,$2,$3,1,$4,1)",
    )
    .bind(staged_asset_id)
    .bind(user_id)
    .bind(scope_id)
    .bind(policy_id)
    .execute(&mut *setup)
    .await
    .expect("insert staged asset");
    sqlx::query(
        "INSERT INTO straylight.asset_versions (
           user_id,asset_id,version,previous_version,bucket,object_key,
           content_hash,size_bytes,media_type,object_version_id
         ) VALUES ($1,$2,1,NULL,'recovery-test',$3,$4,3,'text/plain',$5)",
    )
    .bind(user_id)
    .bind(staged_asset_id)
    .bind(&staged_object_key)
    .bind(&content_hash)
    .bind("source-stage-version")
    .execute(&mut *setup)
    .await
    .expect("insert staged asset version");
    sqlx::query(
        "INSERT INTO straylight.staged_entries (
           id,user_id,scope_id,stage_id,path,entry_kind,media_type,size_bytes,
           content_hash,asset_id,asset_version,readability
         ) VALUES ($1,$2,$3,$4,'staged.txt','file','text/plain',3,$5,$6,1,'readable')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(expiring_stage_id)
    .bind(&content_hash)
    .bind(staged_asset_id)
    .execute(&mut *setup)
    .await
    .expect("insert staged entry");
    setup.commit().await.expect("commit fixture");

    let invalid_asset_id = Uuid::now_v7();
    let mut invalid_version = pool.begin().await.expect("begin invalid asset test");
    sqlx::query(
        "INSERT INTO straylight.assets (
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
        "INSERT INTO straylight.asset_versions (
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

    let mut reclaim = pool.begin().await.expect("begin reclamation");
    let reclaimed = sqlx::query_scalar::<_, String>(
        "SELECT reclaimed_object_key
         FROM straylight.expire_unpromoted_stage($1)",
    )
    .bind(expiring_stage_id)
    .fetch_all(&mut *reclaim)
    .await
    .expect("reclaim expired stage");
    assert_eq!(reclaimed, vec![staged_object_key.clone()]);

    let mut competing = pool.begin().await.expect("begin competing insert");
    sqlx::query("SET LOCAL lock_timeout='750ms'")
        .execute(&mut *competing)
        .await
        .expect("set competing lock timeout");
    let blocked_at = Instant::now();
    let blocked_insert = sqlx::query(
        "INSERT INTO straylight.staged_entries (
           id,user_id,scope_id,stage_id,path,entry_kind,media_type,size_bytes,
           content_hash,asset_id,asset_version,readability
         ) VALUES ($1,$2,$3,$4,'competing.txt','file','text/plain',3,$5,$6,1,'readable')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(competing_stage_id)
    .bind(&content_hash)
    .bind(staged_asset_id)
    .execute(&mut *competing)
    .await;
    assert!(
        blocked_insert.is_err(),
        "a concurrent child insert must not pass an uncommitted parent deletion"
    );
    assert!(
        blocked_at.elapsed() >= Duration::from_millis(500),
        "the child insert did not wait on the locked parent row"
    );
    competing.rollback().await.expect("rollback blocked insert");
    reclaim.commit().await.expect("commit reclamation");

    let post_commit_insert = sqlx::query(
        "INSERT INTO straylight.staged_entries (
           id,user_id,scope_id,stage_id,path,entry_kind,media_type,size_bytes,
           content_hash,asset_id,asset_version,readability
         ) VALUES ($1,$2,$3,$4,'after.txt','file','text/plain',3,$5,$6,1,'readable')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(scope_id)
    .bind(competing_stage_id)
    .bind(&content_hash)
    .bind(staged_asset_id)
    .execute(&pool)
    .await;
    assert!(
        post_commit_insert
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|error| error.code())
            .is_some_and(|code| code == "23503"),
        "the child insert must fail its foreign key after reclamation commits"
    );

    let persistent_asset_id = Uuid::now_v7();
    let persistent_key = format!("{user_id}/blobs/{}", "4".repeat(64));
    let persistent_hash = "4".repeat(64);
    let mut persistent = pool.begin().await.expect("begin persistent asset");
    sqlx::query(
        "INSERT INTO straylight.assets (
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
        "INSERT INTO straylight.asset_versions (
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
        "UPDATE straylight.asset_versions
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
        sqlx::query_scalar::<_, i64>("SELECT straylight.remap_asset_object_versions($1::jsonb)")
            .bind(&mapping)
            .fetch_one(&pool)
            .await
            .expect("perform exact locator remap");
    assert_eq!(updated, 1);
    let repeated =
        sqlx::query_scalar::<_, i64>("SELECT straylight.remap_asset_object_versions($1::jsonb)")
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
        sqlx::query_scalar::<_, i64>("SELECT straylight.remap_asset_object_versions($1::jsonb)",)
            .bind(wrong_identity)
            .fetch_one(&pool)
            .await
            .is_err(),
        "recovery must reject a changed key/hash/size identity"
    );
    let locator = sqlx::query_as::<_, (String, String)>(
        "SELECT bucket::text,object_version_id
         FROM straylight.asset_versions
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

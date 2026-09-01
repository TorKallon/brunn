use std::collections::HashSet;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::{Value, json};
use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use brunn::{
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
    secret_service::{decrypt_secret_value, encrypt_secret_value, secret_value_aad},
    task_service::{
        apply_todoist_sync_in_tx, materialize_next_todoist_occurrence_in_tx,
        sync_managed_entry_in_tx,
    },
    todoist_sync::{
        TodoistClientError, TodoistCompletedOccurrence, TodoistSyncResponse, claim_next_sync,
        finish_sync_failure, finish_sync_success_in_tx,
    },
};

struct Owner {
    auth: AuthContext,
    user_id: Uuid,
    credential_id: Uuid,
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping Todoist database test");
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
        .expect("apply Todoist migration");
    Some(pool)
}

async fn insert_owner(pool: &PgPool) -> Owner {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:todoist-{scope_id}");
    let capabilities = vec![
        "open",
        "query",
        "read",
        "compute",
        "verify",
        "status",
        "checkpoint",
        "save",
        "stage",
        "correct",
        "delete",
        "dream",
        "credential:manage",
        "notification:publish",
        "notification:manage",
        "secret:read",
        "secret:write",
        "task.read",
        "task.write",
        "integration.manage",
        "admin",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("todoist-test:{user_id}"))
        .bind("Todoist test")
        .execute(pool)
        .await
        .expect("insert Todoist owner");
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind("Todoist test")
        .execute(pool)
        .await
        .expect("insert Todoist scope");
    sqlx::query(
        r#"
        INSERT INTO brunn.api_credentials(
          id,user_id,label,token_hash,capabilities
        ) VALUES($1,$2,'Todoist owner',$3,$4)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(hash_token(&format!("todoist-owner-{credential_id}")))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert Todoist owner credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant Todoist owner scope");
    Owner {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: false,
        },
        user_id,
        credential_id,
    }
}

async fn begin_as_app_rw<'a>(
    pool: &'a PgPool,
    auth: &AuthContext,
) -> Transaction<'a, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("begin Todoist RLS transaction");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .expect("assume app_rw");
    set_context(&mut tx, auth)
        .await
        .expect("install Todoist RLS context");
    tx
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|value| value.into_owned())
}

fn sync_response(value: Value) -> TodoistSyncResponse {
    serde_json::from_value(value).expect("valid recorded Todoist response")
}

fn recorded_full_sync() -> TodoistSyncResponse {
    serde_json::from_str(include_str!("fixtures/todoist/v1/full_sync.json"))
        .expect("recorded full-sync fixture")
}

fn recurring_sync(external_id: &str, due: &str, expression: &str) -> TodoistSyncResponse {
    sync_response(json!({
        "sync_token":format!("cursor-{due}"),
        "full_sync":false,
        "projects":[],
        "items":[{
            "id":external_id,
            "content":format!("Recurring {external_id}"),
            "description":"Recorded recurring fixture",
            "project_id":"fixture-project",
            "labels":[],
            "priority":1,
            "checked":false,
            "is_deleted":false,
            "completed_at":null,
            "due":{
                "date":due,
                "string":expression,
                "lang":"en",
                "is_recurring":true,
                "timezone":null
            },
            "deadline":null
        }]
    }))
}

fn fixed_recurring_sync(external_id: &str, due_utc: &str, expression: &str) -> TodoistSyncResponse {
    sync_response(json!({
        "sync_token":format!("cursor-{due_utc}"),
        "full_sync":false,
        "projects":[],
        "items":[{
            "id":external_id,
            "content":format!("Fixed recurring {external_id}"),
            "description":"Recorded fixed-zone recurring fixture",
            "project_id":"fixture-project",
            "labels":[],"priority":1,"checked":false,"is_deleted":false,"completed_at":null,
            "due":{"date":due_utc,"string":expression,"lang":"en","is_recurring":true,"timezone":"America/Los_Angeles"},
            "deadline":null
        }]
    }))
}

fn ordinary_sync(external_id: &str, title: &str) -> TodoistSyncResponse {
    sync_response(json!({
        "sync_token":format!("cursor-{external_id}"),
        "full_sync":false,
        "projects":[],
        "items":[{
            "id":external_id,
            "content":title,
            "description":"Recorded ordinary fixture",
            "project_id":"fixture-project",
            "labels":[],
            "priority":1,
            "checked":false,
            "is_deleted":false,
            "completed_at":null,
            "due":null,
            "deadline":null
        }]
    }))
}

async fn todoist_producer(pool: &PgPool, user_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>("SELECT brunn.ensure_task_todoist_producer($1)")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("ensure non-bearer Todoist producer")
}

async fn apply_sync(
    pool: &PgPool,
    user_id: Uuid,
    producer_credential_id: Uuid,
    responses: &[TodoistSyncResponse],
    completed: &[TodoistCompletedOccurrence],
) {
    let mut tx = pool.begin().await.expect("begin Todoist apply transaction");
    apply_todoist_sync_in_tx(
        &mut tx,
        user_id,
        producer_credential_id,
        Tz::America__Los_Angeles,
        responses,
        completed,
    )
    .await
    .expect("apply Todoist fixture");
    tx.commit().await.expect("commit Todoist fixture");
}

async fn external_task(pool: &PgPool, user_id: Uuid, external_id: &str) -> (Uuid, Value) {
    let row = sqlx::query(
        r#"
        SELECT reference.task_id,task.task
        FROM brunn.task_external_refs AS reference
        JOIN brunn.task_index AS task
          ON task.user_id=reference.user_id AND task.task_id=reference.task_id
        WHERE reference.user_id=$1 AND reference.system='todoist'
          AND reference.external_id=$2
        "#,
    )
    .bind(user_id)
    .bind(external_id)
    .fetch_one(pool)
    .await
    .expect("load canonical task by Todoist identity");
    (row.get("task_id"), row.get("task"))
}

async fn rewrite_task_metadata<F>(pool: &PgPool, owner: &Owner, task_id: Uuid, rewrite: F)
where
    F: FnOnce(&mut Value),
{
    let mut tx = pool.begin().await.expect("begin canonical task rewrite");
    let row = sqlx::query(
        r#"
        SELECT entry.id AS entry_id,entry.path,entry.current_version,
               version.content_sha256::text AS content_sha256,
               version.content,version.size_bytes,version.metadata
        FROM brunn.task_index AS task
        JOIN brunn.entries AS entry
          ON entry.user_id=task.user_id AND entry.id=task.entry_id
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE task.user_id=$1 AND task.task_id=$2
        FOR UPDATE OF entry
        "#,
    )
    .bind(owner.user_id)
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await
    .expect("lock canonical task entry");
    let entry_id: Uuid = row.get("entry_id");
    let path: String = row.get("path");
    let current_version: i64 = row.get("current_version");
    let content_sha256: String = row.get("content_sha256");
    let content: String = row
        .get::<Option<String>, _>("content")
        .expect("inline task body");
    let size_bytes: i64 = row.get("size_bytes");
    let mut metadata: Value = row.get("metadata");
    rewrite(&mut metadata);
    let next_version = current_version + 1;
    sqlx::query(
        r#"
        INSERT INTO brunn.entry_versions(
          user_id,entry_id,version,content_sha256,content,size_bytes,metadata,
          created_by_credential_id
        ) VALUES($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(owner.user_id)
    .bind(entry_id)
    .bind(next_version)
    .bind(content_sha256)
    .bind(content)
    .bind(size_bytes)
    .bind(&metadata)
    .bind(owner.credential_id)
    .execute(&mut *tx)
    .await
    .expect("append canonical task metadata version");
    sqlx::query(
        "UPDATE brunn.entries SET current_version=$3,updated_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
    )
    .bind(owner.user_id)
    .bind(entry_id)
    .bind(next_version)
    .execute(&mut *tx)
    .await
    .expect("advance canonical task entry");
    sync_managed_entry_in_tx(
        &mut tx,
        owner.user_id,
        entry_id,
        next_version,
        &path,
        &metadata,
    )
    .await
    .expect("rebuild task projections from canonical metadata");
    tx.commit().await.expect("commit canonical task rewrite");
}

fn task_object(metadata: &mut Value) -> &mut serde_json::Map<String, Value> {
    metadata
        .get_mut("task")
        .and_then(Value::as_object_mut)
        .expect("canonical task.v1 object")
}

#[tokio::test]
async fn todoist_worker_secret_read_is_exact_audited_hidden_and_non_bearer() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let key = [17_u8; 32];
    let secret_id = Uuid::now_v7();
    let canary = "todoist-secret-canary-7f12";
    let aad = secret_value_aad("development", owner.user_id, secret_id, 1);
    let (ciphertext, nonce) = encrypt_secret_value(&key, &aad, canary).unwrap();
    sqlx::query(
        r#"
        INSERT INTO brunn.secrets(
          id,user_id,name,description,value_ciphertext,value_nonce,version,
          created_by_credential_id,updated_by_credential_id
        ) VALUES($1,$2,'todoist-api-token','Todoist token',$3,$4,1,$5,$5)
        "#,
    )
    .bind(secret_id)
    .bind(owner.user_id)
    .bind(&ciphertext)
    .bind(&nonce)
    .bind(owner.credential_id)
    .execute(&pool)
    .await
    .expect("insert encrypted Todoist token");

    let row = sqlx::query("SELECT * FROM brunn.task_todoist_secret_for_worker($1)")
        .bind(owner.user_id)
        .fetch_one(&pool)
        .await
        .expect("read exact Todoist worker secret");
    let producer_id: Uuid = row.try_get("producer_credential_id").unwrap();
    let stored_ciphertext: Vec<u8> = row.try_get("value_ciphertext").unwrap();
    let stored_nonce: Vec<u8> = row.try_get("value_nonce").unwrap();
    assert_eq!(
        decrypt_secret_value(&key, &aad, &stored_ciphertext, &stored_nonce).unwrap(),
        canary
    );
    assert!(!String::from_utf8_lossy(&stored_ciphertext).contains(canary));

    let producer =
        sqlx::query("SELECT label,capabilities,disabled_at FROM brunn.api_credentials WHERE id=$1")
            .bind(producer_id)
            .fetch_one(&pool)
            .await
            .expect("load internal Todoist producer");
    assert_eq!(
        producer.try_get::<Vec<String>, _>("capabilities").unwrap(),
        ["task.read", "task.write"]
    );
    assert_eq!(
        producer.try_get::<String, _>("label").unwrap(),
        "__brunn_todoist_sync__"
    );
    assert!(
        producer
            .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("disabled_at")
            .unwrap()
            .is_none()
    );
    let access_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.secret_access_log WHERE user_id=$1 AND secret_id=$2 AND credential_id=$3 AND operation='get'",
    )
    .bind(owner.user_id)
    .bind(secret_id)
    .bind(producer_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(access_count, 1);

    let mut owner_tx = begin_as_app_rw(&pool, &owner.auth).await;
    assert!(
        sqlx::query("SELECT * FROM brunn.task_todoist_secret_for_worker($1)")
            .bind(owner.user_id)
            .fetch_one(&mut *owner_tx)
            .await
            .is_err(),
        "even an owner/admin application credential cannot invoke the worker read"
    );
    owner_tx.rollback().await.unwrap();

    let mut owner_tx = begin_as_app_rw(&pool, &owner.auth).await;
    let listed = sqlx::query_scalar::<_, Uuid>("SELECT id FROM brunn_auth.list_credentials($1)")
        .bind(owner.user_id)
        .fetch_all(&mut *owner_tx)
        .await
        .expect("list public credentials");
    assert!(!listed.contains(&producer_id));
    let revoke = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "SELECT brunn_auth.revoke_credential($1,$2)",
    )
    .bind(owner.user_id)
    .bind(producer_id)
    .fetch_one(&mut *owner_tx)
    .await
    .expect_err("ordinary credential control cannot revoke an internal producer");
    assert_eq!(database_code(&revoke).as_deref(), Some("P0002"));
    owner_tx.rollback().await.unwrap();
}

#[tokio::test]
async fn todoist_sync_state_is_seeded_with_a_paired_durable_lease() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let row = sqlx::query(
        "SELECT system,configuration_generation,lease_owner,lease_expires_at FROM brunn.task_sync_state WHERE user_id=$1",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .expect("load seeded Todoist sync state");
    assert_eq!(row.try_get::<String, _>("system").unwrap(), "todoist");
    assert_eq!(
        row.try_get::<i64, _>("configuration_generation").unwrap(),
        1
    );
    assert!(
        row.try_get::<Option<String>, _>("lease_owner")
            .unwrap()
            .is_none()
    );

    let error = sqlx::query(
        "UPDATE brunn.task_sync_state SET lease_owner='worker' WHERE user_id=$1 AND system='todoist'",
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .expect_err("half-populated scheduler lease must fail");
    assert_eq!(database_code(&error).as_deref(), Some("23514"));
}

#[tokio::test]
async fn stale_worker_cannot_finalize_after_a_boot_unique_reclaim() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let secret_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO brunn.secrets(
          id,user_id,name,value_ciphertext,value_nonce,version,
          created_by_credential_id,updated_by_credential_id
        ) VALUES($1,$2,'todoist-api-token',$3,$4,1,$5,$5)
        "#,
    )
    .bind(secret_id)
    .bind(owner.user_id)
    .bind(vec![1_u8; 32])
    .bind(vec![2_u8; 12])
    .bind(owner.credential_id)
    .execute(&pool)
    .await
    .expect("insert scheduler-eligibility secret metadata");
    sqlx::query(
        "UPDATE brunn.task_integration_config SET mode='pull' WHERE user_id=$1 AND system='todoist'",
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .unwrap();
    // claim_next_sync is intentionally global. Give this owner an explicit
    // oldest manual request so the test remains deterministic when the
    // disposable database contains rows left by an earlier contract run.
    sqlx::query(
        r#"
        UPDATE brunn.task_sync_state
        SET manual_requested_at='2000-01-01T00:00:00Z',
            next_run_at=NULL,lease_owner=NULL,lease_expires_at=NULL
        WHERE user_id=$1 AND system='todoist'
        "#,
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .unwrap();

    let claim_a = claim_next_sync(&pool, true, "worker:1:boot-a")
        .await
        .unwrap()
        .expect("first worker claims Todoist sync");
    sqlx::query(
        "UPDATE brunn.task_sync_state SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE user_id=$1 AND system='todoist'",
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .unwrap();
    let claim_b = claim_next_sync(&pool, true, "worker:1:boot-b")
        .await
        .unwrap()
        .expect("replacement worker reclaims expired Todoist sync");

    let completion_watermark: DateTime<Utc> = "2026-08-27T20:15:00Z".parse().unwrap();
    let mut stale_tx = pool.begin().await.unwrap();
    let stale = finish_sync_success_in_tx(
        &mut stale_tx,
        &claim_a,
        "cursor-from-stale-a",
        completion_watermark,
    )
    .await
    .expect_err("stale worker must lose the exact lease-owner fence");
    assert!(stale.to_string().contains("configuration changed"));
    stale_tx.rollback().await.unwrap();

    let mut winner_tx = pool.begin().await.unwrap();
    finish_sync_success_in_tx(
        &mut winner_tx,
        &claim_b,
        "cursor-from-winner-b",
        completion_watermark,
    )
    .await
    .expect("replacement worker owns finalization");
    winner_tx.commit().await.unwrap();
    let cursor = sqlx::query_scalar::<_, Option<String>>(
        "SELECT cursor FROM brunn.task_sync_state WHERE user_id=$1 AND system='todoist'",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cursor.as_deref(), Some("cursor-from-winner-b"));
    let last_run_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT last_run_at FROM brunn.task_sync_state WHERE user_id=$1 AND system='todoist'",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(last_run_at, Some(completion_watermark));

    for worker_id in ["worker:1:failed-c", "worker:1:failed-d"] {
        sqlx::query(
            r#"
            UPDATE brunn.task_sync_state
            SET manual_requested_at='2000-01-01T00:00:00Z',next_run_at=NULL
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(owner.user_id)
        .execute(&pool)
        .await
        .unwrap();
        let failed_claim = claim_next_sync(&pool, true, worker_id)
            .await
            .unwrap()
            .expect("retry worker claims Todoist sync");
        finish_sync_failure(
            &pool,
            &failed_claim,
            TodoistClientError::bounded_for_contract_test("todoist_unavailable"),
        )
        .await
        .unwrap();
        let state = sqlx::query(
            "SELECT cursor,last_run_at,last_outcome FROM brunn.task_sync_state WHERE user_id=$1 AND system='todoist'",
        )
        .bind(owner.user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            state.get::<Option<String>, _>("cursor").as_deref(),
            Some("cursor-from-winner-b")
        );
        assert_eq!(
            state.get::<Option<DateTime<Utc>>, _>("last_run_at"),
            Some(completion_watermark),
            "a failed pull must not advance the completed-history watermark"
        );
        assert_eq!(
            state.get::<Option<String>, _>("last_outcome").as_deref(),
            Some("error")
        );
    }

    sqlx::query(
        r#"
        UPDATE brunn.task_sync_state
        SET manual_requested_at='2000-01-01T00:00:00Z',next_run_at=NULL
        WHERE user_id=$1 AND system='todoist'
        "#,
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .unwrap();
    let retry_claim = claim_next_sync(&pool, true, "worker:1:recovered-e")
        .await
        .unwrap()
        .expect("recovery worker claims Todoist sync");
    let recovered_watermark: DateTime<Utc> = "2026-08-28T20:15:00Z".parse().unwrap();
    let mut retry_tx = pool.begin().await.unwrap();
    finish_sync_success_in_tx(
        &mut retry_tx,
        &retry_claim,
        "cursor-from-recovered-e",
        recovered_watermark,
    )
    .await
    .unwrap();
    retry_tx.commit().await.unwrap();
    let recovered = sqlx::query(
        "SELECT cursor,last_run_at FROM brunn.task_sync_state WHERE user_id=$1 AND system='todoist'",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        recovered.get::<Option<String>, _>("cursor").as_deref(),
        Some("cursor-from-recovered-e")
    );
    assert_eq!(
        recovered.get::<Option<DateTime<Utc>>, _>("last_run_at"),
        Some(recovered_watermark)
    );
}

#[tokio::test]
async fn recorded_fixture_is_idempotent_file_native_and_requires_near_match_confirmation() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    sqlx::query(
        "INSERT INTO brunn.task_projects(user_id,slug,title,created_by) VALUES($1,'brunn','Brunn','owner')",
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .expect("seed exact project match");

    apply_sync(&pool, owner.user_id, producer, &[recorded_full_sync()], &[]).await;
    let task_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.task_index WHERE user_id=$1")
            .bind(owner.user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(task_count, 3);

    let (_, deadline) = external_task(&pool, owner.user_id, "9QwErTyUiOpAsDfG").await;
    assert_eq!(deadline["project"]["value"], json!("brunn"));
    assert_eq!(
        deadline["required_contexts"]["value"],
        json!(["online", "release"])
    );
    assert_eq!(deadline["soft_due"]["value"], json!("2026-08-30"));
    assert_eq!(deadline["hard_due"]["value"], json!("2026-09-02T06:59:59Z"));
    assert_eq!(deadline["hard_due"]["note"], json!("todoist_deadline"));
    assert_eq!(
        deadline["external_refs"][0]["url"],
        json!("https://app.todoist.com/app/task/9QwErTyUiOpAsDfG")
    );

    let (recurring_task_id, recurring) =
        external_task(&pool, owner.user_id, "A1b2C3d4E5f6G7h8").await;
    assert_eq!(recurring["hard_due"]["note"], json!("todoist_priority_p1"));
    assert_eq!(
        recurring["recurrence"]["value"]["series_id"],
        json!("A1b2C3d4E5f6G7h8")
    );
    assert_eq!(
        recurring["external_refs"][0]["occurrence_key"],
        json!("2026-08-31T16:00:00Z")
    );

    let (_, hard_without_due) = external_task(&pool, owner.user_id, "H4rDNoDue0000001").await;
    assert!(hard_without_due["hard_due"]["value"].is_null());
    assert!(hard_without_due["triaged_at"]["value"].is_null());

    let version_sum_before = sqlx::query_scalar::<_, i64>(
        "SELECT coalesce(sum(entry.current_version),0)::bigint FROM brunn.entries AS entry JOIN brunn.task_index AS task ON task.user_id=entry.user_id AND task.entry_id=entry.id WHERE entry.user_id=$1",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    apply_sync(&pool, owner.user_id, producer, &[recorded_full_sync()], &[]).await;
    let version_sum_after = sqlx::query_scalar::<_, i64>(
        "SELECT coalesce(sum(entry.current_version),0)::bigint FROM brunn.entries AS entry JOIN brunn.task_index AS task ON task.user_id=entry.user_id AND task.entry_id=entry.id WHERE entry.user_id=$1",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(version_sum_after, version_sum_before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.task_index WHERE user_id=$1",)
            .bind(owner.user_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        3
    );

    // Relational identity is a rebuildable projection of canonical task.v1
    // metadata. Simulate an export/import rebuild by removing the projections
    // and syncing the unchanged canonical entry back through the generic path.
    let mut rebuild_tx = pool.begin().await.unwrap();
    let row = sqlx::query(
        r#"
        SELECT task.entry_id,task.entry_version,entry.path,version.metadata
        FROM brunn.task_index AS task
        JOIN brunn.entries AS entry
          ON entry.user_id=task.user_id AND entry.id=task.entry_id
        JOIN brunn.entry_versions AS version
          ON version.user_id=task.user_id AND version.entry_id=task.entry_id
         AND version.version=task.entry_version
        WHERE task.user_id=$1 AND task.task_id=$2
        "#,
    )
    .bind(owner.user_id)
    .bind(recurring_task_id)
    .fetch_one(&mut *rebuild_tx)
    .await
    .unwrap();
    let entry_id: Uuid = row.get("entry_id");
    let entry_version: i64 = row.get("entry_version");
    let path: String = row.get("path");
    let metadata: Value = row.get("metadata");
    sqlx::query("DELETE FROM brunn.task_external_refs WHERE user_id=$1 AND task_id=$2")
        .bind(owner.user_id)
        .bind(recurring_task_id)
        .execute(&mut *rebuild_tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND task_id=$2")
        .bind(owner.user_id)
        .bind(recurring_task_id)
        .execute(&mut *rebuild_tx)
        .await
        .unwrap();
    sync_managed_entry_in_tx(
        &mut rebuild_tx,
        owner.user_id,
        entry_id,
        entry_version,
        &path,
        &metadata,
    )
    .await
    .expect("rebuild Todoist identity from canonical task metadata");
    rebuild_tx.commit().await.unwrap();
    let (rebuilt_task_id, _) = external_task(&pool, owner.user_id, "A1b2C3d4E5f6G7h8").await;
    assert_eq!(rebuilt_task_id, recurring_task_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND task_id=$2",
        )
        .bind(owner.user_id)
        .bind(recurring_task_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    apply_sync(&pool, owner.user_id, producer, &[recorded_full_sync()], &[]).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.task_index WHERE user_id=$1",)
            .bind(owner.user_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        3
    );

    let near_match = sync_response(json!({
        "sync_token":"near-match-cursor",
        "full_sync":false,
        "projects":[],
        "items":[{
            "id":"NearMatchPhnoe0001",
            "content":"Confirm a near-match context",
            "description":"phnoe must suggest phone without minting",
            "project_id":"6X7r2pQm9AbCdEfG",
            "labels":["phnoe"],
            "priority":1,
            "checked":false,
            "is_deleted":false,
            "completed_at":null,
            "due":null,
            "deadline":null
        }]
    }));
    apply_sync(&pool, owner.user_id, producer, &[near_match], &[]).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_contexts WHERE user_id=$1 AND (slug='phnoe' OR lower(display_name)='phnoe')",
        )
        .bind(owner.user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_context_aliases WHERE user_id=$1 AND lower(alias)='phnoe'",
        )
        .bind(owner.user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    let (_, near_match_task) = external_task(&pool, owner.user_id, "NearMatchPhnoe0001").await;
    assert_eq!(near_match_task["required_contexts"]["value"], json!([]));
    assert_eq!(
        near_match_task["todoist_context_suggestions"][0]["requested"],
        json!("phnoe")
    );
    assert_eq!(
        near_match_task["todoist_context_suggestions"][0]["suggested_existing"][0]["slug"],
        json!("phone")
    );
}

#[tokio::test]
async fn recurring_history_materializes_every_missed_occurrence_once_and_replays_cleanly() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let series_id = "MissedWeeklySeries0001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            series_id,
            "2026-08-25T09:00:00",
            "every Tuesday at 9am",
        )],
        &[],
    )
    .await;
    let first_completed_at: DateTime<Utc> = "2026-08-25T16:05:00Z".parse().unwrap();
    let second_completed_at: DateTime<Utc> = "2026-09-01T16:06:00Z".parse().unwrap();
    let third_completed_at: DateTime<Utc> = "2026-09-08T16:07:00Z".parse().unwrap();
    // Deliberately reverse upstream order; apply must establish chronology.
    let completed = vec![
        TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: third_completed_at,
            occurrence_key: Some("2026-09-08T09:00:00".to_owned()),
        },
        TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: second_completed_at,
            occurrence_key: Some("2026-09-01T09:00:00".to_owned()),
        },
        TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: first_completed_at,
            occurrence_key: Some("2026-08-25T09:00:00".to_owned()),
        },
    ];
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            series_id,
            "2026-09-15T09:00:00",
            "every Tuesday at 9am",
        )],
        &completed,
    )
    .await;

    let rows = sqlx::query(
        r#"
        SELECT occurrence.occurrence_key,task.status,task.done_at,task.task_id
        FROM brunn.task_todoist_occurrences AS occurrence
        JOIN brunn.task_index AS task
          ON task.user_id=occurrence.user_id AND task.task_id=occurrence.task_id
        WHERE occurrence.user_id=$1 AND occurrence.series_id=$2
        ORDER BY occurrence.occurrence_key
        "#,
    )
    .bind(owner.user_id)
    .bind(series_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter()
            .map(|row| row.get::<String, _>("occurrence_key"))
            .collect::<Vec<_>>(),
        [
            "2026-08-25T09:00:00",
            "2026-09-01T09:00:00",
            "2026-09-08T09:00:00",
            "2026-09-15T09:00:00",
        ]
    );
    assert_eq!(rows[0].get::<String, _>("status"), "done");
    assert_eq!(
        rows[0].get::<Option<DateTime<Utc>>, _>("done_at"),
        Some(first_completed_at)
    );
    assert_eq!(rows[1].get::<String, _>("status"), "done");
    assert_eq!(
        rows[1].get::<Option<DateTime<Utc>>, _>("done_at"),
        Some(second_completed_at)
    );
    assert_eq!(rows[2].get::<String, _>("status"), "done");
    assert_eq!(
        rows[2].get::<Option<DateTime<Utc>>, _>("done_at"),
        Some(third_completed_at)
    );
    assert_eq!(rows[3].get::<String, _>("status"), "open");
    assert!(rows[3].get::<Option<DateTime<Utc>>, _>("done_at").is_none());
    let active_task_id = rows[3].get::<Uuid, _>("task_id");
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT task_id FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        active_task_id
    );

    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            series_id,
            "2026-09-15T09:00:00",
            "every Tuesday at 9am",
        )],
        &completed,
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        4
    );
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT task_id FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        active_task_id
    );
}

#[tokio::test]
async fn fixed_zone_recurring_history_keeps_utc_keys_across_dst() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let series_id = "FixedDstSeries00000001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[fixed_recurring_sync(
            series_id,
            "2026-03-02T17:00:00Z",
            "every Monday at 9am",
        )],
        &[],
    )
    .await;
    let completed = vec![
        TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: "2026-03-09T16:05:00Z".parse().unwrap(),
            occurrence_key: Some("2026-03-09T16:00:00Z".to_owned()),
        },
        TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: "2026-03-02T17:05:00Z".parse().unwrap(),
            occurrence_key: Some("2026-03-02T17:00:00Z".to_owned()),
        },
    ];
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[fixed_recurring_sync(
            series_id,
            "2026-03-16T16:00:00Z",
            "every Monday at 9am",
        )],
        &completed,
    )
    .await;
    let keys = sqlx::query_scalar::<_, String>(
        "SELECT occurrence_key FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2 ORDER BY occurrence_key",
    )
    .bind(owner.user_id)
    .bind(series_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        keys,
        [
            "2026-03-02T17:00:00Z",
            "2026-03-09T16:00:00Z",
            "2026-03-16T16:00:00Z",
        ]
    );
    let (_, active) = external_task(&pool, owner.user_id, series_id).await;
    assert_eq!(active["status"]["value"], json!("open"));
    assert_eq!(
        active["recurrence"]["value"]["due"],
        json!("2026-03-16T16:00:00Z")
    );
}

#[tokio::test]
async fn unparseable_remote_recurrence_keeps_one_review_while_recording_each_completion() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let series_id = "UnparseableRemote00001";
    let response = |due: &str| {
        sync_response(json!({
            "sync_token":format!("unparseable-{due}"),
            "full_sync":false,
            "projects":[],
            "items":[{
                "id":series_id,
                "content":"Cada semana revisión",
                "description":"Preserve raw non-English recurrence for review.",
                "project_id":"fixture-project",
                "labels":[],"priority":1,"checked":false,"is_deleted":false,"completed_at":null,
                "due":{"date":due,"string":"cada semana","lang":"es","is_recurring":true,"timezone":null},
                "deadline":null
            }]
        }))
    };
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[response("2026-08-25T09:00:00")],
        &[],
    )
    .await;
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[response("2026-09-01T09:00:00")],
        &[TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: "2026-08-25T16:05:00Z".parse().unwrap(),
            occurrence_key: Some("2026-08-25T09:00:00".to_owned()),
        }],
    )
    .await;
    let (review_task_id, review_task) = external_task(&pool, owner.user_id, series_id).await;
    assert_eq!(review_task["status"]["value"], json!("open"));
    assert_eq!(
        review_task["triaged_at"]["note"],
        json!("todoist_recurrence_review")
    );
    assert!(
        review_task["external_refs"][0]["occurrence_key"]
            .as_str()
            .unwrap()
            .starts_with("review:")
    );

    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[response("2026-09-08T09:00:00")],
        &[TodoistCompletedOccurrence {
            external_id: series_id.to_owned(),
            completed_at: "2026-09-01T16:06:00Z".parse().unwrap(),
            occurrence_key: Some("2026-09-01T09:00:00".to_owned()),
        }],
    )
    .await;
    let third_completion = TodoistCompletedOccurrence {
        external_id: series_id.to_owned(),
        completed_at: "2026-09-08T16:07:00Z".parse().unwrap(),
        occurrence_key: Some("2026-09-08T09:00:00".to_owned()),
    };
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[response("2026-09-15T09:00:00")],
        std::slice::from_ref(&third_completion),
    )
    .await;
    let occurrences = sqlx::query(
        r#"
        SELECT occurrence.occurrence_key,task.status,task.task_id
        FROM brunn.task_todoist_occurrences AS occurrence
        JOIN brunn.task_index AS task
          ON task.user_id=occurrence.user_id AND task.task_id=occurrence.task_id
        WHERE occurrence.user_id=$1 AND occurrence.series_id=$2
        ORDER BY occurrence.occurrence_key
        "#,
    )
    .bind(owner.user_id)
    .bind(series_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(occurrences.len(), 4);
    assert_eq!(
        occurrences
            .iter()
            .filter(|row| row.get::<String, _>("status") == "done")
            .count(),
        3
    );
    assert_eq!(
        occurrences
            .iter()
            .filter(|row| row.get::<String, _>("status") == "open")
            .count(),
        1
    );
    assert_eq!(
        occurrences
            .iter()
            .filter(|row| {
                row.get::<Uuid, _>("task_id") == review_task_id
                    && row
                        .get::<String, _>("occurrence_key")
                        .starts_with("review:")
            })
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT task_id FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        review_task_id
    );
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[response("2026-09-15T09:00:00")],
        std::slice::from_ref(&third_completion),
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        4
    );
}

#[tokio::test]
async fn stale_initial_full_plus_incremental_preserves_the_completed_baseline_occurrence() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let series_id = "InitialCatchupSeries0001";
    // Re-deserialize so the public fixture boundary, not private fields, sets
    // the documented full-sync marker.
    let stale_full = sync_response(json!({
        "sync_token":"initial-stale-full",
        "full_sync":true,
        "projects":[],
        "items":[{
            "id":series_id,
            "content":"Initial catch-up recurrence",
            "description":"The stale baseline occurrence must survive.",
            "project_id":"fixture-project",
            "labels":[],
            "priority":1,
            "checked":false,
            "is_deleted":false,
            "completed_at":null,
            "due":{
                "date":"2026-08-25T09:00:00",
                "string":"every Tuesday at 9am",
                "lang":"en",
                "is_recurring":true,
                "timezone":null
            },
            "deadline":null
        }]
    }));
    let active_incremental =
        recurring_sync(series_id, "2026-09-01T09:00:00", "every Tuesday at 9am");
    let completed_at: DateTime<Utc> = "2026-08-25T16:05:00Z".parse().unwrap();
    let completed = vec![TodoistCompletedOccurrence {
        external_id: series_id.to_owned(),
        completed_at,
        occurrence_key: Some("2026-08-25T09:00:00".to_owned()),
    }];
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[stale_full, active_incremental],
        &completed,
    )
    .await;
    let rows = sqlx::query(
        r#"
        SELECT occurrence.occurrence_key,task.status,task.done_at
        FROM brunn.task_todoist_occurrences AS occurrence
        JOIN brunn.task_index AS task
          ON task.user_id=occurrence.user_id AND task.task_id=occurrence.task_id
        WHERE occurrence.user_id=$1 AND occurrence.series_id=$2
        ORDER BY occurrence.occurrence_key
        "#,
    )
    .bind(owner.user_id)
    .bind(series_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get::<String, _>("occurrence_key"),
        "2026-08-25T09:00:00"
    );
    assert_eq!(rows[0].get::<String, _>("status"), "done");
    assert_eq!(
        rows[0].get::<Option<DateTime<Utc>>, _>("done_at"),
        Some(completed_at)
    );
    assert_eq!(
        rows[1].get::<String, _>("occurrence_key"),
        "2026-09-01T09:00:00"
    );
    assert_eq!(rows[1].get::<String, _>("status"), "open");

    let replay_responses = [
        sync_response(json!({
            "sync_token":"initial-stale-full",
            "full_sync":true,
            "projects":[],
            "items":[{
                "id":series_id,
                "content":"Initial catch-up recurrence",
                "description":"The stale baseline occurrence must survive.",
                "project_id":"fixture-project",
                "labels":[],"priority":1,"checked":false,"is_deleted":false,"completed_at":null,
                "due":{"date":"2026-08-25T09:00:00","string":"every Tuesday at 9am","lang":"en","is_recurring":true,"timezone":null},
                "deadline":null
            }]
        })),
        recurring_sync(series_id, "2026-09-01T09:00:00", "every Tuesday at 9am"),
    ];
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &replay_responses,
        &completed,
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
}

#[tokio::test]
async fn current_initial_full_ignores_unprovable_completion_history_before_the_import_baseline() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let series_id = "CurrentBaselineSeries01";
    let full_current = sync_response(json!({
        "sync_token":"current-baseline-full",
        "full_sync":true,
        "projects":[],
        "items":[{
            "id":series_id,"content":"Current import baseline","description":"",
            "project_id":"fixture-project","labels":[],"priority":1,
            "checked":false,"is_deleted":false,"completed_at":null,
            "due":{"date":"2026-09-01T09:00:00","string":"every Tuesday at 9am","lang":"en","is_recurring":true,"timezone":null},
            "deadline":null
        }]
    }));
    let catchup = sync_response(json!({
        "sync_token":"current-baseline-catchup",
        "full_sync":false,
        "projects":[],
        "items":[]
    }));
    let pre_import_history = vec![TodoistCompletedOccurrence {
        external_id: series_id.to_owned(),
        completed_at: "2026-08-25T16:05:00Z".parse().unwrap(),
        occurrence_key: Some("2026-08-25T09:00:00".to_owned()),
    }];
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[full_current, catchup],
        &pre_import_history,
    )
    .await;
    let rows = sqlx::query(
        r#"
        SELECT occurrence.occurrence_key,task.status
        FROM brunn.task_todoist_occurrences AS occurrence
        JOIN brunn.task_index AS task
          ON task.user_id=occurrence.user_id AND task.task_id=occurrence.task_id
        WHERE occurrence.user_id=$1 AND occurrence.series_id=$2
        "#,
    )
    .bind(owner.user_id)
    .bind(series_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get::<String, _>("occurrence_key"),
        "2026-09-01T09:00:00"
    );
    assert_eq!(rows[0].get::<String, _>("status"), "open");
}

#[tokio::test]
async fn recurring_due_reschedules_replace_the_current_ledger_key_without_completion() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let series_id = "RemoteRescheduleSeries0001";

    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            series_id,
            "2026-09-01T09:00:00",
            "every Tuesday at 9am",
        )],
        &[],
    )
    .await;
    let (task_id, _) = external_task(&pool, owner.user_id, series_id).await;

    for due in ["2026-09-08T09:00:00", "2026-09-03T09:00:00"] {
        apply_sync(
            &pool,
            owner.user_id,
            producer,
            &[recurring_sync(series_id, due, "every Tuesday at 9am")],
            &[],
        )
        .await;
        let (current_task_id, current_task) = external_task(&pool, owner.user_id, series_id).await;
        assert_eq!(
            current_task_id, task_id,
            "a reschedule must reuse the open task"
        );
        assert_eq!(
            current_task["status"]["value"],
            json!("open"),
            "a due-date edit must not synthesize a completion"
        );
        let ledger = sqlx::query(
            r#"
            SELECT occurrence_key,task_id
            FROM brunn.task_todoist_occurrences
            WHERE user_id=$1 AND series_id=$2
            "#,
        )
        .bind(owner.user_id)
        .bind(series_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].get::<String, _>("occurrence_key"), due);
        assert_eq!(ledger[0].get::<Uuid, _>("task_id"), task_id);
    }
}

#[tokio::test]
async fn field_authority_controls_terminals_and_recurrence_lifecycle_independently() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;

    let overridden_series = "OwnerRecurrenceOverride0001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            overridden_series,
            "2026-08-25T09:00:00",
            "every Tuesday at 9am",
        )],
        &[],
    )
    .await;
    let (overridden_task_id, _) = external_task(&pool, owner.user_id, overridden_series).await;
    rewrite_task_metadata(&pool, &owner, overridden_task_id, |metadata| {
        task_object(metadata).insert(
            "recurrence".to_owned(),
            json!({
                "value":null,
                "source":"owner",
                "set_at":"2026-08-26T18:00:00Z",
                "note":"owner removed recurrence"
            }),
        );
    })
    .await;
    let overridden_completed_at: DateTime<Utc> = "2026-08-27T16:05:00Z".parse().unwrap();
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            overridden_series,
            "2026-09-01T09:00:00",
            "every Tuesday at 9am",
        )],
        &[TodoistCompletedOccurrence {
            external_id: overridden_series.to_owned(),
            completed_at: overridden_completed_at,
            occurrence_key: Some("2026-08-25T09:00:00".to_owned()),
        }],
    )
    .await;
    let (still_same_task_id, overridden_task) =
        external_task(&pool, owner.user_id, overridden_series).await;
    assert_eq!(still_same_task_id, overridden_task_id);
    assert_eq!(overridden_task["status"]["value"], json!("done"));
    assert_eq!(overridden_task["status"]["source"], json!("todoist"));
    assert_eq!(overridden_task["done_at"], json!("2026-08-27T16:05:00Z"));
    assert_eq!(overridden_task["recurrence"]["source"], json!("owner"));
    assert!(overridden_task["recurrence"]["value"].is_null());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(overridden_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1,
        "recurrence override must suppress the remote next-occurrence lifecycle"
    );

    let owner_dropped_id = "OwnerDroppedAuthority0001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[ordinary_sync(owner_dropped_id, "Owner dropped authority")],
        &[],
    )
    .await;
    let (owner_dropped_task_id, _) = external_task(&pool, owner.user_id, owner_dropped_id).await;
    rewrite_task_metadata(&pool, &owner, owner_dropped_task_id, |metadata| {
        let task = task_object(metadata);
        task.insert(
            "status".to_owned(),
            json!({"value":"dropped","source":"owner","set_at":"2026-08-27T17:00:00Z"}),
        );
        task.insert(
            "dropped_reason".to_owned(),
            json!({"value":"owner_cancelled","source":"owner","set_at":"2026-08-27T17:00:00Z"}),
        );
        task.insert("dropped_at".to_owned(), json!("2026-08-27T17:00:00Z"));
    })
    .await;
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[],
        &[TodoistCompletedOccurrence {
            external_id: owner_dropped_id.to_owned(),
            completed_at: "2026-08-27T18:00:00Z".parse().unwrap(),
            occurrence_key: None,
        }],
    )
    .await;
    let (_, owner_dropped) = external_task(&pool, owner.user_id, owner_dropped_id).await;
    assert_eq!(owner_dropped["status"]["value"], json!("dropped"));
    assert_eq!(owner_dropped["status"]["source"], json!("owner"));
    assert_eq!(
        owner_dropped["dropped_reason"]["value"],
        json!("owner_cancelled")
    );
    assert_eq!(owner_dropped["dropped_reason"]["source"], json!("owner"));

    let owner_open_id = "OwnerOpenDeletion000001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[ordinary_sync(owner_open_id, "Owner open authority")],
        &[],
    )
    .await;
    let (owner_open_task_id, _) = external_task(&pool, owner.user_id, owner_open_id).await;
    rewrite_task_metadata(&pool, &owner, owner_open_task_id, |metadata| {
        task_object(metadata).insert(
            "status".to_owned(),
            json!({"value":"open","source":"owner","set_at":"2026-08-27T17:30:00Z"}),
        );
    })
    .await;
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[sync_response(json!({
            "sync_token":"owner-open-delete",
            "full_sync":false,
            "projects":[],
            "items":[{"id":owner_open_id,"is_deleted":true}]
        }))],
        &[],
    )
    .await;
    let (_, owner_open) = external_task(&pool, owner.user_id, owner_open_id).await;
    assert_eq!(owner_open["status"]["value"], json!("open"));
    assert_eq!(owner_open["status"]["source"], json!("owner"));

    let removed_series = "RemovedRecurrenceSeries1";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            removed_series,
            "2026-08-25T09:00:00",
            "every Tuesday at 9am",
        )],
        &[],
    )
    .await;
    let (removed_task_id, _) = external_task(&pool, owner.user_id, removed_series).await;
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[ordinary_sync(removed_series, "Recurrence removed remotely")],
        &[],
    )
    .await;
    let identity = sqlx::query(
        "SELECT series_id,occurrence_key FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
    )
    .bind(owner.user_id)
    .bind(removed_series)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(identity.get::<Option<String>, _>("series_id").is_none());
    assert!(
        identity
            .get::<Option<String>, _>("occurrence_key")
            .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND task_id=$2",
        )
        .bind(owner.user_id)
        .bind(removed_task_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    rewrite_task_metadata(&pool, &owner, removed_task_id, |metadata| {
        let task = task_object(metadata);
        task.insert(
            "status".to_owned(),
            json!({"value":"done","source":"owner","set_at":"2026-08-27T19:00:00Z"}),
        );
        task.insert(
            "completed_via".to_owned(),
            json!({"value":"owner","source":"owner","set_at":"2026-08-27T19:00:00Z"}),
        );
        task.insert("done_at".to_owned(), json!("2026-08-27T19:00:00Z"));
    })
    .await;
    let mut tx = pool.begin().await.unwrap();
    let next = materialize_next_todoist_occurrence_in_tx(
        &mut tx,
        owner.user_id,
        owner.credential_id,
        removed_task_id,
        "2026-08-27T19:00:00Z".parse().unwrap(),
    )
    .await
    .expect("recurrence removal must not make owner completion fail");
    tx.commit().await.unwrap();
    assert!(next.is_none());
}

#[tokio::test]
async fn owner_completion_materializes_one_next_or_one_review_and_remote_replay_dedupes() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    let completed_at: DateTime<Utc> = "2026-08-27T19:00:00Z".parse().unwrap();

    let parseable_series = "OwnerParseableSeries0001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            parseable_series,
            "2026-08-25T09:00:00",
            "every Tuesday at 9am",
        )],
        &[],
    )
    .await;
    let (parseable_completed_task, _) = external_task(&pool, owner.user_id, parseable_series).await;
    rewrite_task_metadata(&pool, &owner, parseable_completed_task, |metadata| {
        let task = task_object(metadata);
        task.insert(
            "status".to_owned(),
            json!({"value":"done","source":"owner","set_at":completed_at}),
        );
        task.insert(
            "completed_via".to_owned(),
            json!({"value":"owner","source":"owner","set_at":completed_at}),
        );
        task.insert("done_at".to_owned(), json!(completed_at));
    })
    .await;
    let mut first_tx = pool.begin().await.unwrap();
    let first_next = materialize_next_todoist_occurrence_in_tx(
        &mut first_tx,
        owner.user_id,
        owner.credential_id,
        parseable_completed_task,
        completed_at,
    )
    .await
    .unwrap()
    .expect("parseable owner completion creates a next occurrence");
    first_tx.commit().await.unwrap();
    let mut replay_tx = pool.begin().await.unwrap();
    let replay_next = materialize_next_todoist_occurrence_in_tx(
        &mut replay_tx,
        owner.user_id,
        owner.credential_id,
        parseable_completed_task,
        completed_at,
    )
    .await
    .unwrap()
    .expect("parseable owner completion replay reuses next occurrence");
    replay_tx.commit().await.unwrap();
    assert_eq!(replay_next, first_next);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(parseable_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT task_id FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
        )
        .bind(owner.user_id)
        .bind(parseable_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        first_next
    );
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            parseable_series,
            "2026-08-25T09:00:00",
            "every Tuesday at 9am",
        )],
        &[],
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(parseable_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );

    let review_series = "OwnerReviewSeries000001";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            review_series,
            "2026-08-26T09:00:00",
            "every blue moon",
        )],
        &[],
    )
    .await;
    let (review_completed_task, _) = external_task(&pool, owner.user_id, review_series).await;
    rewrite_task_metadata(&pool, &owner, review_completed_task, |metadata| {
        let task = task_object(metadata);
        task.insert(
            "status".to_owned(),
            json!({"value":"done","source":"owner","set_at":completed_at}),
        );
        task.insert(
            "completed_via".to_owned(),
            json!({"value":"owner","source":"owner","set_at":completed_at}),
        );
        task.insert("done_at".to_owned(), json!(completed_at));
    })
    .await;
    let mut review_tx = pool.begin().await.unwrap();
    let review_task = materialize_next_todoist_occurrence_in_tx(
        &mut review_tx,
        owner.user_id,
        owner.credential_id,
        review_completed_task,
        completed_at,
    )
    .await
    .unwrap()
    .expect("unparseable owner completion creates a triage review");
    review_tx.commit().await.unwrap();
    let mut review_replay_tx = pool.begin().await.unwrap();
    let review_replay = materialize_next_todoist_occurrence_in_tx(
        &mut review_replay_tx,
        owner.user_id,
        owner.credential_id,
        review_completed_task,
        completed_at,
    )
    .await
    .unwrap()
    .expect("unparseable owner completion replay reuses its review");
    review_replay_tx.commit().await.unwrap();
    assert_eq!(review_replay, review_task);
    let review_projection = sqlx::query_scalar::<_, Value>(
        "SELECT task FROM brunn.task_index WHERE user_id=$1 AND task_id=$2",
    )
    .bind(owner.user_id)
    .bind(review_task)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        review_projection["triaged_at"]["note"],
        json!("todoist_recurrence_review")
    );
    assert!(review_projection["soft_due"]["value"].is_null());
    let review_occurrence_key = format!("review:{review_completed_task}");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT occurrence_key FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
        )
        .bind(owner.user_id)
        .bind(review_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        review_occurrence_key
    );
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[recurring_sync(
            review_series,
            "2026-08-26T09:00:00",
            "every blue moon",
        )],
        &[],
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_todoist_occurrences WHERE user_id=$1 AND series_id=$2",
        )
        .bind(owner.user_id)
        .bind(review_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT task_id FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id=$2",
        )
        .bind(owner.user_id)
        .bind(review_series)
        .fetch_one(&pool)
        .await
        .unwrap(),
        review_task
    );
}

#[tokio::test]
async fn project_only_renames_remap_todoist_owned_fields_but_preserve_owner_overrides() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool).await;
    let producer = todoist_producer(&pool, owner.user_id).await;
    for (slug, title) in [("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")] {
        sqlx::query(
            "INSERT INTO brunn.task_projects(user_id,slug,title,created_by) VALUES($1,$2,$3,'owner')",
        )
        .bind(owner.user_id)
        .bind(slug)
        .bind(title)
        .execute(&pool)
        .await
        .unwrap();
    }
    let item = |external_id: &str, project_id: &str| {
        json!({
            "id":external_id,
            "content":format!("Project mapping {external_id}"),
            "description":"Recorded project rename fixture",
            "project_id":project_id,
            "labels":[],
            "priority":1,
            "checked":false,
            "is_deleted":false,
            "completed_at":null,
            "due":null,
            "deadline":null
        })
    };

    let window_project = "ProjectWindowExternal01";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[
            sync_response(json!({
                "sync_token":"window-full",
                "full_sync":true,
                "projects":[{"id":window_project,"name":"Alpha","is_deleted":false}],
                "items":[item("WindowRenameItem000001",window_project)]
            })),
            sync_response(json!({
                "sync_token":"window-catchup",
                "full_sync":false,
                "projects":[{"id":window_project,"name":"Beta","is_deleted":false}],
                "items":[]
            })),
        ],
        &[],
    )
    .await;
    let (_, window_task) = external_task(&pool, owner.user_id, "WindowRenameItem000001").await;
    assert_eq!(window_task["project"]["value"], json!("beta"));

    let existing_project = "ProjectExistingExtern1";
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[sync_response(json!({
            "sync_token":"existing-full",
            "full_sync":false,
            "projects":[{"id":existing_project,"name":"Alpha","is_deleted":false}],
            "items":[item("ExistingRenameItem0001",existing_project)]
        }))],
        &[],
    )
    .await;
    let (existing_task_id, existing_task) =
        external_task(&pool, owner.user_id, "ExistingRenameItem0001").await;
    assert_eq!(existing_task["project"]["value"], json!("alpha"));
    assert_eq!(
        existing_task["external_refs"][0]["project_id"],
        json!(existing_project)
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT metadata->>'project_id' FROM brunn.task_external_refs WHERE user_id=$1 AND system='todoist' AND external_id='ExistingRenameItem0001'",
        )
        .bind(owner.user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        existing_project
    );
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[sync_response(json!({
            "sync_token":"existing-rename-beta",
            "full_sync":false,
            "projects":[{"id":existing_project,"name":"Beta","is_deleted":false}],
            "items":[]
        }))],
        &[],
    )
    .await;
    let (_, remapped_task) = external_task(&pool, owner.user_id, "ExistingRenameItem0001").await;
    assert_eq!(remapped_task["project"]["value"], json!("beta"));
    assert_eq!(remapped_task["project"]["source"], json!("todoist"));

    rewrite_task_metadata(&pool, &owner, existing_task_id, |metadata| {
        task_object(metadata).insert(
            "project".to_owned(),
            json!({
                "value":"alpha",
                "source":"owner",
                "set_at":"2026-08-27T20:00:00Z",
                "note":"owner project override"
            }),
        );
    })
    .await;
    apply_sync(
        &pool,
        owner.user_id,
        producer,
        &[sync_response(json!({
            "sync_token":"existing-rename-gamma",
            "full_sync":false,
            "projects":[{"id":existing_project,"name":"Gamma","is_deleted":false}],
            "items":[]
        }))],
        &[],
    )
    .await;
    let (_, owner_project) = external_task(&pool, owner.user_id, "ExistingRenameItem0001").await;
    assert_eq!(owner_project["project"]["value"], json!("alpha"));
    assert_eq!(owner_project["project"]["source"], json!("owner"));
}

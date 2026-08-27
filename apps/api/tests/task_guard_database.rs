use std::collections::HashSet;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use straylight::{
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
    task_guard,
};

struct Principal {
    auth: AuthContext,
    user_id: Uuid,
    credential_id: Uuid,
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping task guard database test");
        return None;
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
    Some(pool)
}

async fn insert_owner(pool: &PgPool, label: &str) -> Principal {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-guard-{scope_id}");
    let capabilities = vec![
        "open".to_owned(),
        "query".to_owned(),
        "read".to_owned(),
        "compute".to_owned(),
        "verify".to_owned(),
        "status".to_owned(),
        "checkpoint".to_owned(),
        "save".to_owned(),
        "stage".to_owned(),
        "correct".to_owned(),
        "delete".to_owned(),
        "dream".to_owned(),
        "credential:manage".to_owned(),
        "notification:publish".to_owned(),
        "notification:manage".to_owned(),
        "secret:read".to_owned(),
        "secret:write".to_owned(),
        "task.read".to_owned(),
        "task.write".to_owned(),
        "integration.manage".to_owned(),
        "admin".to_owned(),
    ];
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-guard-test:{label}:{user_id}"))
        .bind(format!("Task guard test {label}"))
        .execute(pool)
        .await
        .expect("insert guard test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Task guard test {label}"))
        .execute(pool)
        .await
        .expect("insert guard test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Task guard owner {label}"))
    .bind(hash_token(&format!("task-guard-owner-{credential_id}")))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert guard owner credential");
    sqlx::query(
        r#"
        INSERT INTO straylight.credential_scope_grants (
          credential_id,user_id,scope_id
        ) VALUES ($1,$2,$3)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant guard owner scope");
    Principal {
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
    let mut tx = pool
        .begin()
        .await
        .expect("begin task guard RLS transaction");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .expect("assume app_rw");
    set_context(&mut tx, auth)
        .await
        .expect("install task guard RLS context");
    tx
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|value| value.into_owned())
}

async fn insert_installation(pool: &PgPool, principal: &Principal) {
    sqlx::query(
        r#"
        INSERT INTO straylight.notification_installations (
          user_id,client_installation_id,registered_by_credential_id,
          platform,environment,app_id,token_ciphertext,token_nonce,token_hash,
          preview,enabled
        ) VALUES (
          $1,$2,$3,'ios','development','com.example.Straylight',
          $4,$5,$6,'generic',true
        )
        "#,
    )
    .bind(principal.user_id)
    .bind(Uuid::now_v7())
    .bind(principal.credential_id)
    .bind(vec![7_u8; 32])
    .bind(vec![8_u8; 12])
    .bind(format!(
        "{}{}",
        principal.user_id.simple(),
        principal.user_id.simple()
    ))
    .execute(pool)
    .await
    .expect("insert guard delivery installation");
}

async fn insert_task(
    pool: &PgPool,
    principal: &Principal,
    title: &str,
    hard_due: DateTime<Utc>,
    hard_due_lead_days: Option<i32>,
    source: &str,
    note: Option<&str>,
    set_at: DateTime<Utc>,
) -> Uuid {
    let task_id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let path = format!(".straylight/tasks/{task_id}.md");
    let cell = json!({
        "value":hard_due,
        "source":source,
        "set_at":set_at,
        "note":note
    });
    let task = json!({
        "id":task_id,
        "title":title,
        "status":{"value":"open","source":"owner","set_at":set_at},
        "hard_due":cell
    });
    let metadata = json!({"kind":"task","schema":"task.v1","task":task});
    let mut tx = pool.begin().await.expect("begin guard task fixture");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version,created_at,updated_at
        ) VALUES ($1,$2,$3,$4,'markdown','text/markdown',0,$5,$5)
        "#,
    )
    .bind(entry_id)
    .bind(principal.user_id)
    .bind(path)
    .bind(title)
    .bind(set_at)
    .execute(&mut *tx)
    .await
    .expect("insert guard task entry");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata,
          created_by_credential_id,created_at
        ) VALUES ($1,$2,$3,1,$4,$5,$6,$7,$8,$9)
        "#,
    )
    .bind(version_id)
    .bind(principal.user_id)
    .bind(entry_id)
    .bind("b".repeat(64))
    .bind(format!("# {title}\n"))
    .bind(i64::try_from(title.len() + 3).unwrap())
    .bind(metadata)
    .bind(principal.credential_id)
    .bind(set_at)
    .execute(&mut *tx)
    .await
    .expect("insert guard task version");
    sqlx::query("UPDATE straylight.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(principal.user_id)
        .bind(entry_id)
        .execute(&mut *tx)
        .await
        .expect("advance guard task head");
    sqlx::query(
        r#"
        INSERT INTO straylight.task_index (
          user_id,task_id,entry_id,entry_version,title,status,hard_due,
          hard_due_lead_days,provenance,source_timestamps,task,
          created_at,updated_at
        ) VALUES (
          $1,$2,$3,1,$4,'open',$5,$6,$7,$8,$9,$10,$10
        )
        "#,
    )
    .bind(principal.user_id)
    .bind(task_id)
    .bind(entry_id)
    .bind(title)
    .bind(hard_due)
    .bind(hard_due_lead_days)
    .bind(json!({"hard_due":source}))
    .bind(json!({"hard_due":set_at}))
    .bind(task)
    .bind(set_at)
    .execute(&mut *tx)
    .await
    .expect("insert guard task projection");
    tx.commit().await.expect("commit guard task fixture");
    task_id
}

#[tokio::test]
async fn task_guard_time_travel_dedupes_routes_and_delays_inferred_quiet_delivery() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool, "flow").await;
    insert_installation(&pool, &owner).await;
    sqlx::query(
        r#"
        UPDATE straylight.task_settings
        SET timezone='UTC',quiet_hours_start='22:00',quiet_hours_end='07:00',
            quiet_override_enabled=true,quiet_override_within_hours=24
        WHERE user_id=$1
        "#,
    )
    .bind(owner.user_id)
    .execute(&pool)
    .await
    .expect("configure deterministic guard timezone");

    let seven_day = Utc
        .with_ymd_and_hms(2026, 8, 27, 12, 0, 0)
        .single()
        .unwrap();
    let due = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).single().unwrap();
    let task_id = insert_task(
        &pool,
        &owner,
        "Renew certificate",
        due,
        None,
        "owner",
        None,
        seven_day,
    )
    .await;
    sqlx::query(
        r#"
        UPDATE straylight.task_index
        SET cost_flag=true,cost_since=$3::date,
            provenance=provenance || jsonb_build_object('cost_of_delay','agent:codex'),
            source_timestamps=source_timestamps || jsonb_build_object('cost_of_delay',$3::text),
            task=jsonb_set(
              task,'{cost_of_delay}',
              jsonb_build_object(
                'value',jsonb_build_object('flag',true,'since',$3::date),
                'source','agent:codex','set_at',$3
              ),true
            )
        WHERE user_id=$1 AND task_id=$2
        "#,
    )
    .bind(owner.user_id)
    .bind(task_id)
    .bind(seven_day)
    .execute(&pool)
    .await
    .expect("add cost guard fixture");

    let first = task_guard::run_on_pool(&pool, seven_day, true)
        .await
        .expect("run seven-day guard band");
    let first_task = first
        .events
        .iter()
        .filter(|event| event.task_id == task_id && event.event_key.starts_with("task-deadline:"))
        .collect::<Vec<_>>();
    assert_eq!(first_task.len(), 1);
    assert_eq!(
        first_task[0].event_key,
        format!("task-deadline:{task_id}:7d")
    );
    assert!(first_task[0].inserted);
    assert_eq!(first_task[0].route, format!("straylight://task/{task_id}"));

    let replay = task_guard::run_on_pool(&pool, seven_day, true)
        .await
        .expect("replay seven-day guard band");
    assert_eq!(
        replay
            .events
            .iter()
            .filter(|event| event.task_id == task_id && event.inserted)
            .count(),
        0
    );
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.notifications WHERE user_id=$1 AND event_key=$2",
    )
    .bind(owner.user_id)
    .bind(format!("task-deadline:{task_id}:7d"))
    .fetch_one(&pool)
    .await
    .expect("count deduped seven-day notification");
    assert_eq!(count, 1);
    let cost_set_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.notifications WHERE user_id=$1 AND event_key=$2",
    )
    .bind(owner.user_id)
    .bind(format!("task-cost:{task_id}:set"))
    .fetch_one(&pool)
    .await
    .expect("count once-only cost-set notification");
    assert_eq!(cost_set_count, 1);

    let at_48h = due - chrono::Duration::hours(48);
    task_guard::run_on_pool(&pool, at_48h, true)
        .await
        .expect("run 48-hour guard band");
    let due_day = Utc.with_ymd_and_hms(2026, 9, 3, 7, 0, 0).single().unwrap();
    task_guard::run_on_pool(&pool, due_day, true)
        .await
        .expect("run due-day guard band");
    task_guard::run_on_pool(&pool, seven_day + chrono::Duration::weeks(1), true)
        .await
        .expect("run weekly cost guard band");
    let weekly_cost_keys = sqlx::query_scalar::<_, String>(
        "SELECT event_key FROM straylight.notifications WHERE user_id=$1 AND event_key LIKE $2 ORDER BY event_key",
    )
    .bind(owner.user_id)
    .bind(format!("task-cost:{task_id}:week:%"))
    .fetch_all(&pool)
    .await
    .expect("read weekly local-week cost key");
    assert_eq!(
        weekly_cost_keys,
        [format!("task-cost:{task_id}:week:2026-W36")]
    );
    let keys = sqlx::query_scalar::<_, String>(
        "SELECT event_key FROM straylight.notifications WHERE user_id=$1 AND event_key LIKE $2 ORDER BY event_key",
    )
    .bind(owner.user_id)
    .bind(format!("task-deadline:{task_id}:%"))
    .fetch_all(&pool)
    .await
    .expect("read deadline band keys");
    assert_eq!(
        keys,
        [
            format!("task-deadline:{task_id}:48h"),
            format!("task-deadline:{task_id}:7d"),
            format!("task-deadline:{task_id}:due-day"),
        ]
    );
    let target: Value = sqlx::query_scalar(
        "SELECT target FROM straylight.notifications WHERE user_id=$1 AND event_key=$2",
    )
    .bind(owner.user_id)
    .bind(format!("task-deadline:{task_id}:7d"))
    .fetch_one(&pool)
    .await
    .expect("read typed task target");
    assert_eq!(
        target,
        json!({"type":"task","task_ref":task_id.to_string()})
    );

    let quiet_as_of = Utc.with_ymd_and_hms(2026, 9, 4, 23, 0, 0).single().unwrap();
    let quiet_due = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).single().unwrap();
    let inferred_id = insert_task(
        &pool,
        &owner,
        "Confirm inferred renewal",
        quiet_due,
        Some(0),
        "agent:codex",
        None,
        quiet_as_of - chrono::Duration::hours(1),
    )
    .await;
    let quiet = task_guard::run_on_pool(&pool, quiet_as_of, true)
        .await
        .expect("run inferred quiet-hours guard");
    let quiet_events = quiet
        .events
        .iter()
        .filter(|event| event.task_id == inferred_id)
        .collect::<Vec<_>>();
    assert_eq!(quiet_events.len(), 1);
    assert!(quiet_events[0].inferred);
    assert!(quiet_events[0].quiet_delayed);
    assert_eq!(
        quiet_events[0].delivery_available_at,
        Utc.with_ymd_and_hms(2026, 9, 5, 7, 0, 0).single().unwrap()
    );
    let quiet_row = sqlx::query(
        r#"
        SELECT notification.body,delivery.state,delivery.available_at
        FROM straylight.notifications AS notification
        JOIN straylight.notification_deliveries AS delivery
          ON delivery.user_id=notification.user_id
         AND delivery.notification_id=notification.id
        WHERE notification.user_id=$1 AND notification.event_key=$2
        "#,
    )
    .bind(owner.user_id)
    .bind(format!("task-deadline:{inferred_id}:48h"))
    .fetch_one(&pool)
    .await
    .expect("read delayed inferred guard delivery");
    assert!(
        quiet_row
            .get::<String, _>("body")
            .contains("inferred — confirm?")
    );
    assert_eq!(quiet_row.get::<String, _>("state"), "queued");
    assert_eq!(
        quiet_row.get::<DateTime<Utc>, _>("available_at"),
        Utc.with_ymd_and_hms(2026, 9, 5, 7, 0, 0).single().unwrap()
    );

    let reserved_as_of = Utc
        .with_ymd_and_hms(2026, 9, 24, 12, 0, 0)
        .single()
        .unwrap();
    let reserved_due = reserved_as_of + chrono::Duration::days(7);
    let reserved_id = insert_task(
        &pool,
        &owner,
        "Reserved event namespace",
        reserved_due,
        None,
        "owner",
        None,
        reserved_as_of,
    )
    .await;
    let reserved_key = format!("task-deadline:{reserved_id}:7d");
    sqlx::query(
        r#"
        INSERT INTO straylight.notifications (
          user_id,producer_credential_id,event_key,request_hash,correlation_id,
          kind,importance,title,body,target,occurred_at,expires_at
        ) VALUES (
          $1,$2,$3,$4,'preempted','operational','important','Wrong producer',
          'Wrong content',jsonb_build_object('type','notification'),$5,$6
        )
        "#,
    )
    .bind(owner.user_id)
    .bind(owner.credential_id)
    .bind(&reserved_key)
    .bind("c".repeat(64))
    .bind(reserved_as_of)
    .bind(reserved_as_of + chrono::Duration::days(1))
    .execute(&pool)
    .await
    .expect("insert reserved-key preemption fixture");
    assert!(
        task_guard::run_on_pool(&pool, reserved_as_of, true)
            .await
            .is_err(),
        "the internal enqueue primitive must fail closed on reserved-key preemption"
    );
    let reserved_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.notifications WHERE user_id=$1 AND event_key=$2",
    )
    .bind(owner.user_id)
    .bind(reserved_key)
    .fetch_one(&pool)
    .await
    .expect("count fail-closed reserved event key");
    assert_eq!(reserved_count, 1);
    let failed_state = sqlx::query(
        "SELECT last_outcome,last_error_code FROM straylight.task_guard_state WHERE user_id=$1",
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .expect("read content-free failed guard state");
    assert_eq!(
        failed_state
            .get::<Option<String>, _>("last_outcome")
            .as_deref(),
        Some("failed")
    );
    assert_eq!(
        failed_state
            .get::<Option<String>, _>("last_error_code")
            .as_deref(),
        Some("task_guard_database")
    );
}

#[tokio::test]
async fn task_guard_state_is_seeded_content_free_and_rls_isolated() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner_a = insert_owner(&pool, "state-a").await;
    let owner_b = insert_owner(&pool, "state-b").await;
    let columns = sqlx::query_scalar::<_, String>(
        r#"
        SELECT column_name FROM information_schema.columns
        WHERE table_schema='straylight' AND table_name='task_guard_state'
        ORDER BY ordinal_position
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("list guard state columns");
    assert_eq!(
        columns,
        [
            "user_id",
            "last_run_at",
            "last_outcome",
            "last_error_code",
            "next_run_at",
            "updated_at",
        ]
    );
    let seeded = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.task_guard_state WHERE user_id=ANY($1)",
    )
    .bind(vec![owner_a.user_id, owner_b.user_id])
    .fetch_one(&pool)
    .await
    .expect("count seeded guard state");
    assert_eq!(seeded, 2);

    task_guard::run_on_pool(&pool, Utc::now(), false)
        .await
        .expect("record successful guard run");
    let state = sqlx::query(
        "SELECT last_run_at,last_outcome,last_error_code,next_run_at FROM straylight.task_guard_state WHERE user_id=$1",
    )
    .bind(owner_a.user_id)
    .fetch_one(&pool)
    .await
    .expect("read successful guard state");
    assert!(
        state
            .get::<Option<DateTime<Utc>>, _>("last_run_at")
            .is_some()
    );
    assert_eq!(
        state.get::<Option<String>, _>("last_outcome").as_deref(),
        Some("success")
    );
    assert_eq!(state.get::<Option<String>, _>("last_error_code"), None);
    assert!(
        state
            .get::<Option<DateTime<Utc>>, _>("next_run_at")
            .is_some()
    );

    let mut own_tx = begin_as_app_rw(&pool, &owner_a.auth).await;
    let visible = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM straylight.task_guard_state ORDER BY user_id",
    )
    .fetch_all(&mut *own_tx)
    .await
    .expect("task reader sees guard status");
    assert_eq!(visible, [owner_a.user_id]);
    own_tx.rollback().await.unwrap();

    let mut no_task_read = owner_a.auth.clone();
    no_task_read.capabilities.remove("task.read");
    no_task_read.capabilities.remove("admin");
    let mut denied_tx = begin_as_app_rw(&pool, &no_task_read).await;
    let hidden = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM straylight.task_guard_state WHERE user_id=$1",
    )
    .bind(owner_a.user_id)
    .fetch_optional(&mut *denied_tx)
    .await
    .expect("non-task principal sees no guard state");
    assert_eq!(hidden, None);
    let write = sqlx::query(
        "UPDATE straylight.task_guard_state SET last_outcome='failed' WHERE user_id=$1",
    )
    .bind(owner_a.user_id)
    .execute(&mut *denied_tx)
    .await
    .expect_err("application roles cannot write guard state");
    assert_eq!(database_code(&write).as_deref(), Some("42501"));
    denied_tx.rollback().await.unwrap();
}

#[tokio::test]
async fn task_guard_internal_producer_has_no_bearer_and_is_hidden_and_irrevocable() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner = insert_owner(&pool, "producer").await;
    let producer = sqlx::query(
        r#"
        SELECT credential.id,credential.token_hash,credential.capabilities
        FROM straylight.task_guard_producers AS guard
        JOIN straylight.api_credentials AS credential
          ON credential.user_id=guard.user_id
         AND credential.id=guard.credential_id
        WHERE guard.user_id=$1
        "#,
    )
    .bind(owner.user_id)
    .fetch_one(&pool)
    .await
    .expect("read hidden task guard producer as database owner");
    let producer_id: Uuid = producer.get("id");
    let stored_hash: String = producer.get("token_hash");
    let capabilities: Vec<String> = producer.get("capabilities");
    assert_eq!(capabilities, ["task.read", "notification:publish"]);
    assert_eq!(stored_hash.len(), 64);
    assert!(stored_hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let rls = sqlx::query(
        r#"
        SELECT relrowsecurity,relforcerowsecurity
        FROM pg_class AS class
        JOIN pg_namespace AS namespace ON namespace.oid=class.relnamespace
        WHERE namespace.nspname='straylight'
          AND class.relname='task_guard_producers'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read guard producer RLS metadata");
    assert!(rls.get::<bool, _>("relrowsecurity"));
    assert!(rls.get::<bool, _>("relforcerowsecurity"));
    let execute_grants: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
          has_function_privilege(
            'app_rw',
            'straylight.enqueue_task_guard_notification(uuid,uuid,text,text,text,timestamptz,timestamptz,timestamptz,boolean)',
            'EXECUTE'
          ),
          has_function_privilege(
            'app_ro',
            'straylight.enqueue_task_guard_notification(uuid,uuid,text,text,text,timestamptz,timestamptz,timestamptz,boolean)',
            'EXECUTE'
          ),
          has_function_privilege(
            'public',
            'straylight.enqueue_task_guard_notification(uuid,uuid,text,text,text,timestamptz,timestamptz,timestamptz,boolean)',
            'EXECUTE'
          )
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("read guard enqueue execution grants");
    assert_eq!(execute_grants, (false, false, false));

    let mut denied_tx = begin_as_app_rw(&pool, &owner.auth).await;
    let denied =
        sqlx::query("SELECT credential_id FROM straylight.task_guard_producers WHERE user_id=$1")
            .bind(owner.user_id)
            .fetch_optional(&mut *denied_tx)
            .await
            .expect_err("app_rw must not read hidden guard producers");
    assert_eq!(database_code(&denied).as_deref(), Some("42501"));
    denied_tx
        .rollback()
        .await
        .expect("rollback hidden producer denial");

    let old_forgeable_bearer = format!(
        "straylight.task-guard.internal.v1|{}|{}",
        owner.user_id, producer_id
    );
    assert_ne!(stored_hash, hash_token(&old_forgeable_bearer));
    for attempted_bearer in [old_forgeable_bearer, stored_hash.clone()] {
        let authenticated = sqlx::query_scalar::<_, Uuid>(
            "SELECT credential_id FROM straylight_auth.authenticate_credential($1)",
        )
        .bind(hash_token(&attempted_bearer))
        .fetch_optional(&pool)
        .await
        .expect("attempt hidden producer authentication");
        assert_eq!(
            authenticated, None,
            "hidden producer must have no usable bearer"
        );
    }

    let mut tx = begin_as_app_rw(&pool, &owner.auth).await;
    let listed =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM straylight_auth.list_credentials($1)")
            .bind(owner.user_id)
            .fetch_all(&mut *tx)
            .await
            .expect("list visible credentials");
    assert!(listed.contains(&owner.credential_id));
    assert!(!listed.contains(&producer_id));
    let revoke =
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT straylight_auth.revoke_credential($1,$2)")
            .bind(owner.user_id)
            .bind(producer_id)
            .fetch_one(&mut *tx)
            .await
            .expect_err("ordinary revoke must not disable the hidden producer");
    assert_eq!(database_code(&revoke).as_deref(), Some("P0002"));
    tx.rollback()
        .await
        .expect("rollback revoke denial transaction");

    let disabled_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT disabled_at FROM straylight.api_credentials WHERE user_id=$1 AND id=$2",
    )
    .bind(owner.user_id)
    .bind(producer_id)
    .fetch_one(&pool)
    .await
    .expect("verify guard producer remains active");
    assert_eq!(disabled_at, None);
}

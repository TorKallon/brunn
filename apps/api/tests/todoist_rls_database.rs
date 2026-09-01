use std::collections::HashSet;

use sqlx::{PgPool, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use brunn::{
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
};

struct Principal {
    auth: AuthContext,
    user_id: Uuid,
}

#[derive(Debug, PartialEq)]
struct TodoistTenantSnapshot {
    occurrence: (String, String, Uuid, Uuid),
    config: (String, i64),
    state: (Option<String>, i64, Option<String>),
    project: (String, String, bool),
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping Todoist RLS database test");
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
        .expect("apply Brunn migrations through frozen schema 0074");
    Some(pool)
}

async fn insert_principal(pool: &PgPool, label: &str) -> Principal {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:todoist-rls-{scope_id}");
    let capabilities = ["task.read", "task.write", "integration.manage", "admin"]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("todoist-rls-test:{label}:{user_id}"))
        .bind(format!("Todoist RLS {label}"))
        .execute(pool)
        .await
        .expect("insert Todoist RLS user");
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Todoist RLS {label}"))
        .execute(pool)
        .await
        .expect("insert Todoist RLS scope");
    sqlx::query(
        r#"
        INSERT INTO brunn.api_credentials(
          id,user_id,label,token_hash,capabilities
        ) VALUES($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Todoist RLS {label}"))
    .bind(hash_token(&format!("todoist-rls-{credential_id}")))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert Todoist RLS credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant Todoist RLS scope");

    Principal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: false,
        },
        user_id,
    }
}

async fn seed_todoist_tenant(pool: &PgPool, principal: &Principal, label: &str) {
    let task_id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let series_id = format!("{label}-series");
    let occurrence_key = format!("{label}-occurrence");
    let content = format!("# Todoist RLS {label} task\n");
    let mut tx = pool.begin().await.expect("begin Todoist tenant fixture");

    sqlx::query(
        r#"
        INSERT INTO brunn.entries(
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES($1,$2,$3,$4,'markdown','text/markdown',0)
        "#,
    )
    .bind(entry_id)
    .bind(principal.user_id)
    .bind(format!(".brunn/tasks/{task_id}.md"))
    .bind(format!("Todoist RLS {label} task"))
    .execute(&mut *tx)
    .await
    .expect("insert Todoist occurrence entry fixture");
    sqlx::query(
        r#"
        INSERT INTO brunn.entry_versions(
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES($1,$2,$3,1,$4,$5,$6,'{}'::jsonb)
        "#,
    )
    .bind(version_id)
    .bind(principal.user_id)
    .bind(entry_id)
    .bind(format!("{:064x}", task_id.as_u128()))
    .bind(&content)
    .bind(i64::try_from(content.len()).expect("fixture content length fits i64"))
    .execute(&mut *tx)
    .await
    .expect("insert Todoist occurrence entry version fixture");
    sqlx::query("UPDATE brunn.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(principal.user_id)
        .bind(entry_id)
        .execute(&mut *tx)
        .await
        .expect("advance Todoist occurrence entry fixture");
    sqlx::query(
        r#"
        INSERT INTO brunn.task_todoist_occurrences(
          user_id,series_id,occurrence_key,task_id,entry_id
        ) VALUES($1,$2,$3,$4,$5)
        "#,
    )
    .bind(principal.user_id)
    .bind(&series_id)
    .bind(&occurrence_key)
    .bind(task_id)
    .bind(entry_id)
    .execute(&mut *tx)
    .await
    .expect("insert Todoist occurrence fixture");
    tx.commit().await.expect("commit Todoist tenant fixture");
    sqlx::query(
        r#"
        UPDATE brunn.task_integration_config
        SET mode='pull',configuration_generation=$2
        WHERE user_id=$1 AND system='todoist'
        "#,
    )
    .bind(principal.user_id)
    .bind(if label == "a" { 7_i64 } else { 17_i64 })
    .execute(pool)
    .await
    .expect("customize Todoist config fixture");
    sqlx::query(
        r#"
        UPDATE brunn.task_sync_state
        SET cursor=$2,configuration_generation=$3,last_outcome=$4
        WHERE user_id=$1 AND system='todoist'
        "#,
    )
    .bind(principal.user_id)
    .bind(format!("{label}-cursor-sentinel"))
    .bind(if label == "a" { 7_i64 } else { 17_i64 })
    .bind(format!("{label}-outcome-sentinel"))
    .execute(pool)
    .await
    .expect("customize Todoist sync-state fixture");
    sqlx::query(
        r#"
        INSERT INTO brunn.task_todoist_projects(
          user_id,external_id,name,is_deleted
        ) VALUES($1,$2,$3,false)
        "#,
    )
    .bind(principal.user_id)
    .bind(format!("{label}-project"))
    .bind(format!("{label}-project-name"))
    .execute(pool)
    .await
    .expect("insert Todoist project-cache fixture");
}

async fn begin_as<'a>(
    pool: &'a PgPool,
    auth: &AuthContext,
    role: &str,
) -> Transaction<'a, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("begin Todoist RLS transaction");
    let set_role = match role {
        "app_rw" => "SET LOCAL ROLE app_rw",
        "app_ro" => "SET LOCAL ROLE app_ro",
        unexpected => panic!("unsupported test role {unexpected}"),
    };
    sqlx::query(set_role)
        .execute(&mut *tx)
        .await
        .expect("assume application database role");
    set_context(&mut tx, auth)
        .await
        .expect("install real Todoist RLS transaction context");
    tx
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|value| value.into_owned())
}

async fn assert_uuid_statement_denied(
    pool: &PgPool,
    auth: &AuthContext,
    role: &str,
    statement: &'static str,
    user_id: Uuid,
) {
    let mut tx = begin_as(pool, auth, role).await;
    let error = sqlx::query(statement)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .expect_err("database privilege or RLS policy must reject the statement");
    assert_eq!(
        database_code(&error).as_deref(),
        Some("42501"),
        "unexpected database error for {role}: {error}"
    );
    tx.rollback().await.expect("rollback rejected statement");
}

async fn snapshot_tenant(pool: &PgPool, user_id: Uuid) -> TodoistTenantSnapshot {
    TodoistTenantSnapshot {
        occurrence: sqlx::query_as(
            r#"
            SELECT series_id,occurrence_key,task_id,entry_id
            FROM brunn.task_todoist_occurrences
            WHERE user_id=$1
            "#,
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("snapshot Todoist occurrence"),
        config: sqlx::query_as(
            r#"
            SELECT mode,configuration_generation
            FROM brunn.task_integration_config
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("snapshot Todoist config"),
        state: sqlx::query_as(
            r#"
            SELECT cursor,configuration_generation,last_outcome
            FROM brunn.task_sync_state
            WHERE user_id=$1 AND system='todoist'
            "#,
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("snapshot Todoist sync state"),
        project: sqlx::query_as(
            r#"
            SELECT external_id,name,is_deleted
            FROM brunn.task_todoist_projects
            WHERE user_id=$1
            "#,
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("snapshot Todoist project cache"),
    }
}

async fn assert_exposed_rows_are_tenant_scoped(
    pool: &PgPool,
    principal: &Principal,
    other_user_id: Uuid,
    role: &str,
) {
    let mut tx = begin_as(pool, &principal.auth, role).await;
    for (table, visible_sql, other_sql) in [
        (
            "task_todoist_occurrences",
            "SELECT DISTINCT user_id FROM brunn.task_todoist_occurrences ORDER BY user_id",
            "SELECT EXISTS(SELECT 1 FROM brunn.task_todoist_occurrences WHERE user_id=$1)",
        ),
        (
            "task_integration_config",
            "SELECT DISTINCT user_id FROM brunn.task_integration_config ORDER BY user_id",
            "SELECT EXISTS(SELECT 1 FROM brunn.task_integration_config WHERE user_id=$1)",
        ),
        (
            "task_sync_state",
            "SELECT DISTINCT user_id FROM brunn.task_sync_state ORDER BY user_id",
            "SELECT EXISTS(SELECT 1 FROM brunn.task_sync_state WHERE user_id=$1)",
        ),
    ] {
        let visible_users = sqlx::query_scalar::<_, Uuid>(visible_sql)
            .fetch_all(&mut *tx)
            .await
            .unwrap_or_else(|error| panic!("read {table} as {role}: {error}"));
        assert_eq!(
            visible_users,
            vec![principal.user_id],
            "{role} must see exactly the current tenant in {table}"
        );
        let other_is_visible = sqlx::query_scalar::<_, bool>(other_sql)
            .bind(other_user_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap_or_else(|error| panic!("probe other tenant in {table} as {role}: {error}"));
        assert!(
            !other_is_visible,
            "{role} exposed another tenant in {table}"
        );
    }
    tx.rollback().await.expect("finish tenant-scoped reads");
}

#[tokio::test]
async fn todoist_tables_enforce_cross_user_rls_and_keep_project_cache_worker_private() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let owner_a = insert_principal(&pool, "a").await;
    let owner_b = insert_principal(&pool, "b").await;
    seed_todoist_tenant(&pool, &owner_a, "a").await;
    seed_todoist_tenant(&pool, &owner_b, "b").await;
    let owner_b_before = snapshot_tenant(&pool, owner_b.user_id).await;

    // Both application roles can read the public integration state, but forced
    // RLS exposes exactly owner A's tenant even though this credential carries
    // the broad admin capability.
    assert_exposed_rows_are_tenant_scoped(&pool, &owner_a, owner_b.user_id, "app_rw").await;
    assert_exposed_rows_are_tenant_scoped(&pool, &owner_a, owner_b.user_id, "app_ro").await;

    // The incremental project-name cache is intentionally not an owner-facing
    // table. Neither application role may read even the current tenant's row.
    for role in ["app_rw", "app_ro"] {
        for user_id in [owner_a.user_id, owner_b.user_id] {
            assert_uuid_statement_denied(
                &pool,
                &owner_a.auth,
                role,
                "SELECT 1 FROM brunn.task_todoist_projects WHERE user_id=$1",
                user_id,
            )
            .await;
        }
    }

    // app_rw can mutate owner A's exposed integration rows, but another
    // tenant's rows are invisible to UPDATE/DELETE and rejected by WITH CHECK
    // on INSERT.
    let mut rw_tx = begin_as(&pool, &owner_a.auth, "app_rw").await;
    for statement in [
        "UPDATE brunn.task_todoist_occurrences SET occurrence_key='cross-user-update' WHERE user_id=$1",
        "DELETE FROM brunn.task_todoist_occurrences WHERE user_id=$1",
        "UPDATE brunn.task_integration_config SET mode='off' WHERE user_id=$1",
        "DELETE FROM brunn.task_integration_config WHERE user_id=$1",
        "UPDATE brunn.task_sync_state SET cursor='cross-user-update' WHERE user_id=$1",
        "DELETE FROM brunn.task_sync_state WHERE user_id=$1",
    ] {
        let result = sqlx::query(statement)
            .bind(owner_b.user_id)
            .execute(&mut *rw_tx)
            .await
            .expect("RLS-filtered cross-user mutation remains a safe no-op");
        assert_eq!(
            result.rows_affected(),
            0,
            "app_rw mutated another tenant with {statement}"
        );
    }
    rw_tx.rollback().await.expect("finish app_rw no-op probes");

    let owner_b_entry_id = owner_b_before.occurrence.3;
    let mut occurrence_insert_tx = begin_as(&pool, &owner_a.auth, "app_rw").await;
    let occurrence_insert = sqlx::query(
        r#"
        INSERT INTO brunn.task_todoist_occurrences(
          user_id,series_id,occurrence_key,task_id,entry_id
        ) VALUES($1,'cross-user-series','cross-user-insert',$2,$3)
        "#,
    )
    .bind(owner_b.user_id)
    .bind(Uuid::now_v7())
    .bind(owner_b_entry_id)
    .execute(&mut *occurrence_insert_tx)
    .await
    .expect_err("app_rw cannot insert another tenant's occurrence");
    assert_eq!(database_code(&occurrence_insert).as_deref(), Some("42501"));
    occurrence_insert_tx
        .rollback()
        .await
        .expect("rollback cross-user occurrence insert");

    assert_uuid_statement_denied(
        &pool,
        &owner_a.auth,
        "app_rw",
        "INSERT INTO brunn.task_integration_config(user_id,system,mode) VALUES($1,'cross-user-probe','off')",
        owner_b.user_id,
    )
    .await;
    assert_uuid_statement_denied(
        &pool,
        &owner_a.auth,
        "app_rw",
        "INSERT INTO brunn.task_sync_state(user_id,system) VALUES($1,'cross-user-probe')",
        owner_b.user_id,
    )
    .await;

    // Project-cache access is privilege-denied before row visibility can leak,
    // for every read/write verb and for both application roles.
    for role in ["app_rw", "app_ro"] {
        for statement in [
            "INSERT INTO brunn.task_todoist_projects(user_id,external_id,name) VALUES($1,'cross-user-probe','forbidden')",
            "UPDATE brunn.task_todoist_projects SET name='forbidden' WHERE user_id=$1",
            "DELETE FROM brunn.task_todoist_projects WHERE user_id=$1",
        ] {
            assert_uuid_statement_denied(&pool, &owner_a.auth, role, statement, owner_b.user_id)
                .await;
        }
    }

    // app_ro has no mutation privileges at all. Exercise every DML verb on the
    // occurrence ledger and write probes on the two owner-visible state tables.
    for statement in [
        "INSERT INTO brunn.task_todoist_occurrences(user_id,series_id,occurrence_key,task_id,entry_id) SELECT $1,'ro-cross-user-series','ro-cross-user-insert',gen_random_uuid(),entry_id FROM brunn.task_todoist_occurrences WHERE user_id=$1 LIMIT 1",
        "UPDATE brunn.task_todoist_occurrences SET occurrence_key='ro-cross-user-update' WHERE user_id=$1",
        "DELETE FROM brunn.task_todoist_occurrences WHERE user_id=$1",
        "INSERT INTO brunn.task_integration_config(user_id,system,mode) VALUES($1,'ro-cross-user-probe','off')",
        "UPDATE brunn.task_integration_config SET mode='off' WHERE user_id=$1",
        "DELETE FROM brunn.task_integration_config WHERE user_id=$1",
        "INSERT INTO brunn.task_sync_state(user_id,system) VALUES($1,'ro-cross-user-probe')",
        "UPDATE brunn.task_sync_state SET cursor='ro-cross-user-update' WHERE user_id=$1",
        "DELETE FROM brunn.task_sync_state WHERE user_id=$1",
    ] {
        assert_uuid_statement_denied(&pool, &owner_a.auth, "app_ro", statement, owner_b.user_id)
            .await;
    }

    let owner_b_after = snapshot_tenant(&pool, owner_b.user_id).await;
    assert_eq!(
        owner_b_after, owner_b_before,
        "cross-user read/write probes changed owner B's Todoist state"
    );
}

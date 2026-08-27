use std::collections::HashSet;

use serde_json::json;
use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use straylight::{
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
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping task database test");
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
        .expect("apply Straylight migrations");
    Some(pool)
}

async fn insert_principal(pool: &PgPool, label: &str, capabilities: &[&str]) -> Principal {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-{scope_id}");
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    let read_only = !capabilities
        .iter()
        .any(|value| value == "save" || value == "checkpoint");
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-test:{label}:{user_id}"))
        .bind(format!("Task test {label}"))
        .execute(pool)
        .await
        .expect("insert task test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Task test {label}"))
        .execute(pool)
        .await
        .expect("insert task test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Task test {label}"))
    .bind(format!("task-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert task test credential");
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
    .expect("grant task test scope");
    Principal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only,
        },
        user_id,
        credential_id,
    }
}

async fn begin_as_app_rw<'a>(
    pool: &'a PgPool,
    auth: &AuthContext,
) -> Transaction<'a, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("begin task RLS transaction");
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .expect("assume app_rw");
    set_context(&mut tx, auth)
        .await
        .expect("install task RLS context");
    tx
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|value| value.into_owned())
}

#[tokio::test]
async fn task_schema_forces_rls_seeds_registries_and_scopes_entry_writes() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let writer = insert_principal(&pool, "writer", &["task.read", "task.write"]).await;
    let reader = insert_principal(&pool, "reader", &["task.read"]).await;
    assert!(
        writer.auth.read_only,
        "narrow task.write must not unlock generic workspace mutation affordances"
    );

    let expected_tables = vec![
        "task_audit_events",
        "task_checkpoint_links",
        "task_context_aliases",
        "task_contexts",
        "task_corrections",
        "task_external_refs",
        "task_integration_config",
        "task_index",
        "task_project_aliases",
        "task_projects",
        "task_settings",
        "task_surface_defaults",
        "task_sync_state",
    ];
    let rows = sqlx::query(
        r#"
        SELECT class.relname,class.relrowsecurity,class.relforcerowsecurity
        FROM pg_class AS class
        JOIN pg_namespace AS namespace ON namespace.oid=class.relnamespace
        WHERE namespace.nspname='straylight' AND class.relname=ANY($1)
        ORDER BY class.relname
        "#,
    )
    .bind(&expected_tables)
    .fetch_all(&pool)
    .await
    .expect("read task RLS metadata");
    assert_eq!(rows.len(), expected_tables.len());
    assert!(rows.iter().all(|row| {
        row.get::<bool, _>("relrowsecurity") && row.get::<bool, _>("relforcerowsecurity")
    }));

    let context_slugs = sqlx::query_scalar::<_, String>(
        "SELECT slug FROM straylight.task_contexts WHERE user_id=$1 ORDER BY slug",
    )
    .bind(writer.user_id)
    .fetch_all(&pool)
    .await
    .expect("read seeded task contexts");
    assert_eq!(
        context_slugs,
        ["errands", "home", "online", "phone", "quick"]
    );
    let defaults: Vec<(String, Vec<String>)> = sqlx::query_as(
        "SELECT surface,contexts FROM straylight.task_surface_defaults WHERE user_id=$1 ORDER BY surface",
    )
    .bind(writer.user_id)
    .fetch_all(&pool)
    .await
    .expect("read seeded surface defaults");
    assert_eq!(
        defaults,
        [
            (
                "ios".to_owned(),
                vec!["phone".to_owned(), "online".to_owned()]
            ),
            ("web".to_owned(), vec!["online".to_owned()]),
        ]
    );

    let arbitrary_entry = Uuid::now_v7();
    let mut arbitrary_tx = begin_as_app_rw(&pool, &writer.auth).await;
    let arbitrary = sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,'Notes/not-a-task.md','Denied','markdown','text/markdown',0)
        "#,
    )
    .bind(arbitrary_entry)
    .bind(writer.user_id)
    .execute(&mut *arbitrary_tx)
    .await
    .expect_err("task.write must not create arbitrary workspace entries");
    assert_eq!(database_code(&arbitrary).as_deref(), Some("42501"));
    arbitrary_tx
        .rollback()
        .await
        .expect("rollback denied arbitrary write");

    let task_id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let path = format!(".straylight/tasks/{task_id}.md");
    let mut task_tx = begin_as_app_rw(&pool, &writer.auth).await;
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,'Scoped task','markdown','text/markdown',0)
        "#,
    )
    .bind(entry_id)
    .bind(writer.user_id)
    .bind(&path)
    .execute(&mut *task_tx)
    .await
    .expect("task.write creates only a task-path entry");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata,
          created_by_credential_id
        ) VALUES ($1,$2,$3,1,$4,'# Scoped task\n',14,$5,$6)
        "#,
    )
    .bind(version_id)
    .bind(writer.user_id)
    .bind(entry_id)
    .bind("a".repeat(64))
    .bind(json!({
        "kind": "task",
        "schema": "task.v1",
        "task": {"id": task_id, "title": "Scoped task", "status": "open"}
    }))
    .bind(writer.credential_id)
    .execute(&mut *task_tx)
    .await
    .expect("task.write creates a version only beneath a task entry");
    sqlx::query("UPDATE straylight.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(writer.user_id)
        .bind(entry_id)
        .execute(&mut *task_tx)
        .await
        .expect("advance task entry head");
    let chunk = sqlx::query(
        r#"
        INSERT INTO straylight.search_chunks (
          user_id,entry_id,entry_version_id,path,chunk_index,heading,content,
          token_estimate
        ) VALUES ($1,$2,$3,$4,0,'','forbidden',1)
        "#,
    )
    .bind(writer.user_id)
    .bind(entry_id)
    .bind(version_id)
    .bind(&path)
    .execute(&mut *task_tx)
    .await
    .expect_err("task.write never gains search-chunk mutation authority");
    assert_eq!(database_code(&chunk).as_deref(), Some("42501"));
    task_tx.rollback().await.expect("rollback scoped fixture");

    let owner_entry = Uuid::now_v7();
    let owner_version = Uuid::now_v7();
    let mut owner_tx = pool.begin().await.expect("begin owned task fixture");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,'Visible task','markdown','text/markdown',0)
        "#,
    )
    .bind(owner_entry)
    .bind(writer.user_id)
    .bind(&path)
    .execute(&mut *owner_tx)
    .await
    .expect("insert owned entry fixture");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES ($1,$2,$3,1,$4,'# Visible task\n',15,$5)
        "#,
    )
    .bind(owner_version)
    .bind(writer.user_id)
    .bind(owner_entry)
    .bind("c".repeat(64))
    .bind(json!({"kind":"task","schema":"task.v1","task":{"id":task_id,"title":"Visible task","status":"open"}}))
    .execute(&mut *owner_tx)
    .await
    .expect("insert owned task version fixture");
    sqlx::query("UPDATE straylight.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(writer.user_id)
        .bind(owner_entry)
        .execute(&mut *owner_tx)
        .await
        .expect("advance owned task entry");
    sqlx::query(
        r#"
        INSERT INTO straylight.task_index (
          user_id,task_id,entry_id,entry_version,title,status,required_contexts,
          snooze_count,parked,task,created_at,updated_at
        ) VALUES ($1,$2,$3,1,'Visible task','open','{}',0,false,$4,clock_timestamp(),clock_timestamp())
        "#,
    )
    .bind(writer.user_id)
    .bind(task_id)
    .bind(owner_entry)
    .bind(json!({"id":task_id,"title":"Visible task","status":"open"}))
    .execute(&mut *owner_tx)
    .await
    .expect("insert task projection fixture");
    owner_tx.commit().await.expect("commit owned task fixture");

    let note_entry = Uuid::now_v7();
    let note_version = Uuid::now_v7();
    let mut note_tx = pool.begin().await.expect("begin non-task fixture");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,'Notes/private.md','Private','markdown','text/markdown',0)
        "#,
    )
    .bind(note_entry)
    .bind(writer.user_id)
    .execute(&mut *note_tx)
    .await
    .expect("insert non-task entry fixture");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES ($1,$2,$3,1,$4,'# Private\n',10,'{}'::jsonb)
        "#,
    )
    .bind(note_version)
    .bind(writer.user_id)
    .bind(note_entry)
    .bind("d".repeat(64))
    .execute(&mut *note_tx)
    .await
    .expect("insert non-task entry version fixture");
    sqlx::query("UPDATE straylight.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(writer.user_id)
        .bind(note_entry)
        .execute(&mut *note_tx)
        .await
        .expect("advance non-task entry head");
    note_tx.commit().await.expect("commit non-task fixture");

    let mut scoped_read_tx = begin_as_app_rw(&pool, &writer.auth).await;
    let visible_paths = sqlx::query_scalar::<_, String>(
        "SELECT path FROM straylight.entries WHERE user_id=$1 ORDER BY path",
    )
    .bind(writer.user_id)
    .fetch_all(&mut *scoped_read_tx)
    .await
    .expect("read entries through task-only RLS");
    assert_eq!(visible_paths, [path.clone()]);
    let visible_versions = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.entry_versions WHERE user_id=$1 ORDER BY id",
    )
    .bind(writer.user_id)
    .fetch_all(&mut *scoped_read_tx)
    .await
    .expect("read versions through task-only RLS");
    assert_eq!(visible_versions, [owner_version]);
    let integration_change = sqlx::query(
        "UPDATE straylight.task_integration_config SET mode='pull' WHERE user_id=$1 AND system='todoist'",
    )
    .bind(writer.user_id)
    .execute(&mut *scoped_read_tx)
    .await
    .expect("RLS hides integration configuration from task.write mutation");
    assert_eq!(integration_change.rows_affected(), 0);
    scoped_read_tx
        .rollback()
        .await
        .expect("rollback scoped reads");

    let mut cross_user_tx = begin_as_app_rw(&pool, &reader.auth).await;
    let visible = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM straylight.task_index WHERE task_id=$1)",
    )
    .bind(task_id)
    .fetch_one(&mut *cross_user_tx)
    .await
    .expect("query cross-user task through RLS");
    assert!(!visible);
    let denied = sqlx::query(
        "INSERT INTO straylight.task_projects (user_id,slug,title,created_by) VALUES ($1,'denied','Denied','owner')",
    )
    .bind(reader.user_id)
    .execute(&mut *cross_user_tx)
    .await
    .expect_err("task.read may not mutate a project");
    assert_eq!(database_code(&denied).as_deref(), Some("42501"));
    cross_user_tx
        .rollback()
        .await
        .expect("rollback task isolation query");

    for function_name in [
        "admin_issue_credential",
        "issue_credential",
        "admin_provision_user",
    ] {
        let definition = sqlx::query_scalar::<_, String>(
            r#"
            SELECT pg_get_functiondef(procedure.oid)
            FROM pg_proc AS procedure
            JOIN pg_namespace AS namespace ON namespace.oid=procedure.pronamespace
            WHERE namespace.nspname='straylight_auth' AND procedure.proname=$1
            "#,
        )
        .bind(function_name)
        .fetch_one(&pool)
        .await
        .expect("read task-aware credential function");
        for capability in ["task.read", "task.write", "integration.manage"] {
            assert!(
                definition.contains(capability),
                "{function_name} omits {capability}"
            );
        }
    }
}

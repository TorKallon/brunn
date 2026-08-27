use std::{collections::HashSet, path::PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use straylight::{
    auth::AuthContext,
    db::set_context,
    models::{CredentialId, UserId},
};

const MESSAGING_TABLES: [&str; 6] = [
    "messaging_agents",
    "messaging_conversations",
    "messaging_credential_bindings",
    "messaging_message_index",
    "messaging_participants",
    "messaging_sync_state",
];

struct Principal {
    auth: AuthContext,
    user_id: Uuid,
    credential_id: Uuid,
}

struct EntryFixture {
    entry_id: Uuid,
    version_id: Uuid,
    path: String,
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping messaging database test");
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
    let scope_ref = format!("scope:messaging-{scope_id}");
    let capabilities = capabilities
        .iter()
        .map(|capability| (*capability).to_owned())
        .collect::<Vec<_>>();
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("messaging-test:{label}:{user_id}"))
        .bind(format!("Messaging test {label}"))
        .execute(pool)
        .await
        .expect("insert messaging test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Messaging test {label}"))
        .execute(pool)
        .await
        .expect("insert messaging test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Messaging test {label}"))
    .bind(format!("messaging-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert messaging test credential");
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
    .expect("grant messaging test scope");
    let read_only = !capabilities
        .iter()
        .any(|capability| capability == "message.write");
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

async fn insert_credential_for_user(
    pool: &PgPool,
    user_id: Uuid,
    label: &str,
    capabilities: &[&str],
) -> Result<Uuid, sqlx::Error> {
    let credential_id = Uuid::now_v7();
    let capabilities = capabilities
        .iter()
        .map(|capability| (*capability).to_owned())
        .collect::<Vec<_>>();
    let credential_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        RETURNING id
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(label)
    .bind(format!("messaging-test-token-{credential_id}"))
    .bind(capabilities)
    .fetch_one(pool)
    .await?;
    let scope_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.scopes WHERE user_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
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
    .await?;
    Ok(credential_id)
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

fn conversation_metadata(conversation_id: Uuid) -> Value {
    json!({
        "kind": "conversation",
        "schema": "conversation.v1",
        "conversation": {
            "id": conversation_id,
            "kind": "direct",
            "subject": "Database contract",
            "participants": ["owner", "echo"],
            "continues_from": null
        }
    })
}

fn conversation_body(conversation_id: Uuid) -> String {
    format!(
        "# Database contract\n\n<!-- straylight:conversation.v1 {{\"id\":\"{conversation_id}\"}} -->\n"
    )
}

async fn insert_canonical_entry_as(
    pool: &PgPool,
    principal: &Principal,
    conversation_id: Uuid,
) -> EntryFixture {
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let path = format!(".straylight/conversations/{conversation_id}.md");
    let content = conversation_body(conversation_id);
    let content_sha256 = hex::encode(Sha256::digest(content.as_bytes()));
    let mut tx = begin_as_app_rw(pool, &principal.auth).await;
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,'Database contract','markdown','text/markdown',1)
        "#,
    )
    .bind(entry_id)
    .bind(principal.user_id)
    .bind(&path)
    .execute(&mut *tx)
    .await
    .expect("message.write inserts a canonical conversation entry");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,
          metadata,created_by_credential_id
        ) VALUES ($1,$2,$3,1,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(version_id)
    .bind(principal.user_id)
    .bind(entry_id)
    .bind(&content_sha256)
    .bind(&content)
    .bind(i64::try_from(content.len()).expect("fixture length fits i64"))
    .bind(conversation_metadata(conversation_id))
    .bind(principal.credential_id)
    .execute(&mut *tx)
    .await
    .expect("message.write inserts a typed conversation version");
    sqlx::query(
        r#"
        INSERT INTO straylight.workspace_changes (
          user_id,entry_id,entry_version,operation,path,content_sha256
        ) VALUES ($1,$2,1,'create',$3,$4)
        "#,
    )
    .bind(principal.user_id)
    .bind(entry_id)
    .bind(&path)
    .bind(&content_sha256)
    .execute(&mut *tx)
    .await
    .expect("message.write emits a canonical conversation workspace change");
    tx.commit()
        .await
        .expect("commit canonical conversation entry fixture");
    EntryFixture {
        entry_id,
        version_id,
        path,
    }
}

async fn insert_ordinary_entry(pool: &PgPool, user_id: Uuid) {
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let content = "ordinary workspace content\n";
    let content_sha256 = hex::encode(Sha256::digest(content.as_bytes()));
    let mut tx = pool.begin().await.expect("begin ordinary fixture insert");
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,'Notes/ordinary.md','Ordinary','markdown','text/markdown',1)
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .expect("insert ordinary entry fixture");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES ($1,$2,$3,1,$4,$5,$6,'{}'::jsonb)
        "#,
    )
    .bind(version_id)
    .bind(user_id)
    .bind(entry_id)
    .bind(content_sha256)
    .bind(content)
    .bind(i64::try_from(content.len()).expect("fixture length fits i64"))
    .execute(&mut *tx)
    .await
    .expect("insert ordinary entry version fixture");
    tx.commit().await.expect("commit ordinary entry fixture");
}

async fn assert_narrow_notification_side_effect(
    pool: &PgPool,
    writer: &Principal,
    entry: &EntryFixture,
    conversation_id: Uuid,
) {
    let agent_id = format!("writer-{}", &conversation_id.simple().to_string()[..12]);
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_agents (
          user_id,agent_id,display_name,principal_kind,delivery_mode,
          created_by_credential_id
        ) VALUES ($1,$2,'Notification writer','resident','pull',$3)
        "#,
    )
    .bind(writer.user_id)
    .bind(&agent_id)
    .bind(writer.credential_id)
    .execute(pool)
    .await
    .expect("seed messaging notification principal");
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_conversations (
          user_id,conversation_id,entry_id,path,conversation_kind,direct_key,
          created_by_agent_id,last_seq,last_message_at,latest_sync_cursor
        ) VALUES ($1,$2,$3,$4,'direct',$5,$6,1,clock_timestamp(),1)
        "#,
    )
    .bind(writer.user_id)
    .bind(conversation_id)
    .bind(entry.entry_id)
    .bind(&entry.path)
    .bind(format!("notification:{conversation_id}"))
    .bind(&agent_id)
    .execute(pool)
    .await
    .expect("seed messaging notification conversation");
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_participants (
          user_id,conversation_id,agent_id,role
        ) VALUES ($1,$2,$3,'participant')
        "#,
    )
    .bind(writer.user_id)
    .bind(conversation_id)
    .bind(&agent_id)
    .execute(pool)
    .await
    .expect("seed messaging notification participant");
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_message_index (
          user_id,conversation_id,seq,message_id,from_agent_id,client_key,
          request_hash,kind,body_md,sync_cursor
        ) VALUES ($1,$2,1,$3,$4,'01ARZ3NDEKTSV4RRFFQ69G5FAV',$5,'text','hello',1)
        "#,
    )
    .bind(writer.user_id)
    .bind(conversation_id)
    .bind(Uuid::now_v7())
    .bind(&agent_id)
    .bind("a".repeat(64))
    .execute(pool)
    .await
    .expect("seed messaging notification message");

    let target = json!({
        "type": "conversation",
        "conversation_id": conversation_id,
        "seq": 1
    });
    let event_key = format!("message:{conversation_id}:1");
    let installation_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO straylight.notification_installations (
          id,user_id,client_installation_id,registered_by_credential_id,
          platform,environment,app_id,token_ciphertext,token_nonce,token_hash,
          preview
        ) VALUES ($1,$2,$3,$4,'ios','development','com.straylight.test',
                  $5,$6,$7,'generic')
        "#,
    )
    .bind(installation_id)
    .bind(writer.user_id)
    .bind(Uuid::now_v7())
    .bind(writer.credential_id)
    .bind(vec![7_u8; 32])
    .bind(vec![8_u8; 12])
    .bind(hex::encode(Sha256::digest(conversation_id.as_bytes())))
    .execute(pool)
    .await
    .expect("seed live notification installation");
    let notification_id = Uuid::now_v7();
    let mut tx = begin_as_app_rw(pool, &writer.auth).await;
    sqlx::query(
        r#"
        INSERT INTO straylight.notifications (
          id,user_id,producer_credential_id,event_key,request_hash,
          correlation_id,kind,importance,title,body,source,target,
          occurred_at,expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,$4,'operational','normal','New agent message',
          'Open Straylight to view the conversation.',NULL,$6,
          clock_timestamp(),clock_timestamp()+interval '24 hours'
        )
        "#,
    )
    .bind(notification_id)
    .bind(writer.user_id)
    .bind(writer.credential_id)
    .bind(&event_key)
    .bind("b".repeat(64))
    .bind(&target)
    .execute(&mut *tx)
    .await
    .expect("message.write publishes only its typed generic conversation side effect");
    let deliveries = sqlx::query(
        r#"
        INSERT INTO straylight.notification_deliveries (
          user_id,notification_id,installation_id,state,last_error_code
        )
        SELECT $1,$2,installation.id,'suppressed','transport_disabled'
        FROM straylight.notification_installations AS installation
        WHERE installation.user_id=$1
          AND installation.enabled AND installation.revoked_at IS NULL
        "#,
    )
    .bind(writer.user_id)
    .bind(notification_id)
    .execute(&mut *tx)
    .await
    .expect("message.write fans its typed alert into the existing delivery outbox")
    .rows_affected();
    assert_eq!(deliveries, 1, "the live installation gets one outbox row");

    let forged = sqlx::query(
        r#"
        INSERT INTO straylight.notifications (
          id,user_id,producer_credential_id,event_key,request_hash,
          correlation_id,kind,importance,title,body,source,target,
          occurred_at,expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,$4,'operational','normal','New agent message',
          'private message text',NULL,$6,
          clock_timestamp(),clock_timestamp()+interval '24 hours'
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(writer.user_id)
    .bind(writer.credential_id)
    .bind(format!("message-system:{conversation_id}:1"))
    .bind("c".repeat(64))
    .bind(target)
    .execute(&mut *tx)
    .await
    .expect_err("message.write cannot publish private or arbitrary notification copy");
    assert_eq!(database_code(&forged).as_deref(), Some("42501"));
    tx.rollback()
        .await
        .expect("rollback messaging notification policy contract");
}

async fn assert_entry_insert_denied(
    pool: &PgPool,
    principal: &Principal,
    user_id: Uuid,
    path: &str,
) {
    let mut tx = begin_as_app_rw(pool, &principal.auth).await;
    let error = sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,'Denied','markdown','text/markdown',0)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(path)
    .execute(&mut *tx)
    .await
    .expect_err("noncanonical or unauthorized messaging entry insert must fail closed");
    assert_eq!(database_code(&error).as_deref(), Some("42501"));
    tx.rollback().await.expect("rollback denied entry insert");
}

async fn assert_schema_contract(pool: &PgPool) {
    let rows = sqlx::query(
        r#"
        SELECT class.relname,class.relrowsecurity,class.relforcerowsecurity
        FROM pg_class AS class
        JOIN pg_namespace AS namespace ON namespace.oid=class.relnamespace
        WHERE namespace.nspname='straylight'
          AND class.relkind IN ('r','p')
          AND class.relname LIKE 'messaging\_%' ESCAPE '\'
        ORDER BY class.relname
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("read messaging table contract");
    let table_names = rows
        .iter()
        .map(|row| row.get::<String, _>("relname"))
        .collect::<Vec<_>>();
    assert_eq!(
        table_names,
        MESSAGING_TABLES.map(str::to_owned),
        "messaging v1 has exactly the six approved tables"
    );
    for row in &rows {
        assert!(
            row.get::<bool, _>("relrowsecurity"),
            "{} must enable RLS",
            row.get::<String, _>("relname")
        );
        assert!(
            row.get::<bool, _>("relforcerowsecurity"),
            "{} must force RLS",
            row.get::<String, _>("relname")
        );
    }

    let user_columns = sqlx::query(
        r#"
        SELECT table_name,is_nullable
        FROM information_schema.columns
        WHERE table_schema='straylight'
          AND table_name=ANY($1)
          AND column_name='user_id'
        ORDER BY table_name
        "#,
    )
    .bind(MESSAGING_TABLES.map(str::to_owned).to_vec())
    .fetch_all(pool)
    .await
    .expect("read messaging user columns");
    assert_eq!(user_columns.len(), MESSAGING_TABLES.len());
    assert!(
        user_columns
            .iter()
            .all(|row| row.get::<String, _>("is_nullable") == "NO"),
        "every messaging table has direct non-null user ownership"
    );

    let foreign_keys = sqlx::query(
        r#"
        SELECT class.relname,pg_get_constraintdef(constraint_row.oid) AS definition
        FROM pg_constraint AS constraint_row
        JOIN pg_class AS class ON class.oid=constraint_row.conrelid
        JOIN pg_namespace AS namespace ON namespace.oid=class.relnamespace
        WHERE namespace.nspname='straylight'
          AND class.relname=ANY($1)
          AND constraint_row.contype='f'
        "#,
    )
    .bind(MESSAGING_TABLES.map(str::to_owned).to_vec())
    .fetch_all(pool)
    .await
    .expect("read messaging foreign keys");
    for table in MESSAGING_TABLES {
        let definitions = foreign_keys
            .iter()
            .filter(|row| row.get::<String, _>("relname") == table)
            .map(|row| row.get::<String, _>("definition"))
            .collect::<Vec<_>>();
        assert!(
            definitions.iter().any(|definition| {
                definition.contains("FOREIGN KEY (user_id)")
                    && definition.contains("REFERENCES straylight.users(id)")
                    && definition.contains("ON DELETE CASCADE")
            }),
            "{table} must cascade from its direct user owner"
        );
    }
    let binding_composite_fks = foreign_keys
        .iter()
        .filter(|row| row.get::<String, _>("relname") == "messaging_credential_bindings")
        .map(|row| row.get::<String, _>("definition"))
        .filter(|definition| definition.contains("FOREIGN KEY (user_id,"))
        .count();
    assert!(
        binding_composite_fks >= 2,
        "credential bindings need same-user credential and principal foreign keys"
    );

    let policies = sqlx::query(
        r#"
        SELECT tablename,cmd,
               coalesce(qual,'') || ' ' || coalesce(with_check,'') AS expression
        FROM pg_policies
        WHERE schemaname='straylight' AND tablename=ANY($1)
        "#,
    )
    .bind(MESSAGING_TABLES.map(str::to_owned).to_vec())
    .fetch_all(pool)
    .await
    .expect("read messaging RLS policies");
    for table in MESSAGING_TABLES {
        let expressions = policies
            .iter()
            .filter(|row| row.get::<String, _>("tablename") == table)
            .map(|row| row.get::<String, _>("expression"))
            .collect::<Vec<_>>();
        assert!(
            expressions
                .iter()
                .all(|expression| expression.contains("can_access_user")),
            "every {table} policy must enforce validated direct-user access"
        );
        assert!(
            expressions
                .iter()
                .any(|expression| expression.contains("message.read")),
            "{table} must have narrow message.read access"
        );
        assert!(
            expressions
                .iter()
                .any(|expression| expression.contains("message.write")),
            "{table} must have narrow message.write access"
        );
    }

    let index_definitions = sqlx::query_scalar::<_, String>(
        r#"
        SELECT indexdef
        FROM pg_indexes
        WHERE schemaname='straylight' AND tablename='messaging_message_index'
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("read message projection indexes");
    assert!(
        index_definitions.iter().any(|definition| {
            definition.contains("UNIQUE")
                && definition.contains("user_id")
                && definition.contains("conversation_id")
                && definition.contains("seq")
        }),
        "message projection needs a per-conversation gapless-sequence unique"
    );
    assert!(
        index_definitions.iter().any(|definition| {
            definition.contains("UNIQUE")
                && definition.contains("user_id")
                && definition.contains("from_agent_id")
                && definition.contains("client_key")
        }),
        "message projection needs sender-scoped client-key idempotency"
    );
    assert!(
        index_definitions.iter().any(|definition| {
            definition.contains("user_id")
                && definition.contains("sync_cursor")
                && definition.contains("conversation_id")
                && definition.contains("seq")
        }),
        "message projection needs an indexed cursor sync path"
    );
}

#[tokio::test]
async fn messaging_schema_capabilities_rls_and_managed_entries_fail_closed() {
    let migration =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0076_agent_messaging.sql");
    assert!(
        migration.is_file(),
        "missing 0076 agent-messaging database surface: {}",
        migration.display()
    );
    let migration_sql = std::fs::read_to_string(&migration).expect("read messaging migration");
    assert!(
        migration_sql.contains("^[0-7][0-9A-HJKMNP-TV-Z]{25}$"),
        "the database must reject ULIDs outside the 128-bit leading range"
    );

    let Some(pool) = connect_test_pool().await else {
        return;
    };
    assert_schema_contract(&pool).await;

    let writer = insert_principal(&pool, "writer", &["message.read", "message.write"]).await;
    let neighbor = insert_principal(&pool, "neighbor", &["message.read", "message.write"]).await;
    insert_principal(
        &pool,
        "task-regression",
        &["task.read", "task.write", "integration.manage"],
    )
    .await;

    let unknown_capability = insert_credential_for_user(
        &pool,
        writer.user_id,
        "Messaging unknown capability",
        &["message.manage"],
    )
    .await
    .expect_err("unapproved messaging capability must remain fail closed");
    assert_eq!(database_code(&unknown_capability).as_deref(), Some("23514"));

    let own_conversation_id = Uuid::now_v7();
    let own_entry = insert_canonical_entry_as(&pool, &writer, own_conversation_id).await;
    assert_narrow_notification_side_effect(&pool, &writer, &own_entry, own_conversation_id).await;
    insert_ordinary_entry(&pool, writer.user_id).await;
    let neighbor_entry = insert_canonical_entry_as(&pool, &neighbor, Uuid::now_v7()).await;

    // The reader fixture owns a different user; issue an actual read credential
    // for the writer's user so validated DB context cannot be forged in-memory.
    let same_user_reader_id = insert_credential_for_user(
        &pool,
        writer.user_id,
        "Messaging same-user reader",
        &["message.read"],
    )
    .await
    .expect("message.read remains an allowed narrow credential capability");
    let same_user_reader = Principal {
        auth: AuthContext {
            credential_id: CredentialId(same_user_reader_id),
            user_id: UserId(writer.user_id),
            capabilities: ["message.read".to_owned()].into_iter().collect(),
            scope_refs: writer.auth.scope_refs.clone(),
            read_only: true,
        },
        user_id: writer.user_id,
        credential_id: same_user_reader_id,
    };

    let mut read_tx = begin_as_app_rw(&pool, &same_user_reader.auth).await;
    let visible_paths =
        sqlx::query_scalar::<_, String>("SELECT path FROM straylight.entries ORDER BY path")
            .fetch_all(&mut *read_tx)
            .await
            .expect("message.read lists its canonical conversation entries");
    assert_eq!(visible_paths, vec![own_entry.path.clone()]);
    read_tx.rollback().await.expect("end messaging read check");

    assert_entry_insert_denied(
        &pool,
        &same_user_reader,
        writer.user_id,
        &format!(".straylight/conversations/{}.md", Uuid::now_v7()),
    )
    .await;
    assert_entry_insert_denied(&pool, &writer, writer.user_id, "Notes/not-messaging.md").await;
    assert_entry_insert_denied(
        &pool,
        &writer,
        writer.user_id,
        ".straylight/conversations/not-a-uuid.md",
    )
    .await;
    assert_entry_insert_denied(&pool, &writer, neighbor.user_id, &neighbor_entry.path).await;

    let invalid_conversation_id = Uuid::now_v7();
    let invalid_entry_id = Uuid::now_v7();
    let invalid_path = format!(".straylight/conversations/{invalid_conversation_id}.md");
    let invalid_content = conversation_body(invalid_conversation_id);
    let mut invalid_metadata_tx = begin_as_app_rw(&pool, &writer.auth).await;
    sqlx::query(
        r#"
        INSERT INTO straylight.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,$3,'Invalid metadata','markdown','text/markdown',1)
        "#,
    )
    .bind(invalid_entry_id)
    .bind(writer.user_id)
    .bind(invalid_path)
    .execute(&mut *invalid_metadata_tx)
    .await
    .expect("canonical path reaches the typed-version boundary");
    let invalid_metadata = sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,
          metadata,created_by_credential_id
        ) VALUES ($1,$2,$3,1,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(writer.user_id)
    .bind(invalid_entry_id)
    .bind(hex::encode(Sha256::digest(invalid_content.as_bytes())))
    .bind(&invalid_content)
    .bind(i64::try_from(invalid_content.len()).expect("fixture length fits i64"))
    .bind(json!({"kind": "note", "schema": "conversation.v1"}))
    .bind(writer.credential_id)
    .execute(&mut *invalid_metadata_tx)
    .await
    .expect_err("conversation path with untyped metadata must fail closed");
    assert_eq!(database_code(&invalid_metadata).as_deref(), Some("42501"));
    invalid_metadata_tx
        .rollback()
        .await
        .expect("rollback invalid conversation metadata");

    let mut chunk_tx = begin_as_app_rw(&pool, &writer.auth).await;
    let chunk_error = sqlx::query(
        r#"
        INSERT INTO straylight.search_chunks (
          id,user_id,entry_id,entry_version_id,chunk_index,path,heading,
          content,token_estimate
        ) VALUES ($1,$2,$3,$4,0,$5,'','must not index',3)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(writer.user_id)
    .bind(own_entry.entry_id)
    .bind(own_entry.version_id)
    .bind(&own_entry.path)
    .execute(&mut *chunk_tx)
    .await
    .expect_err("message.write must not gain search projection authority");
    assert_eq!(database_code(&chunk_error).as_deref(), Some("42501"));
    chunk_tx
        .rollback()
        .await
        .expect("rollback denied search chunk insert");

    let mut job_tx = begin_as_app_rw(&pool, &writer.auth).await;
    let job_error = sqlx::query(
        "INSERT INTO straylight.jobs (user_id,kind,payload) VALUES ($1,'embed_entry',$2)",
    )
    .bind(writer.user_id)
    .bind(json!({"entry_id": own_entry.entry_id, "version": 1}))
    .execute(&mut *job_tx)
    .await
    .expect_err("message.write must not gain embedding-job authority");
    assert_eq!(database_code(&job_error).as_deref(), Some("42501"));
    job_tx
        .rollback()
        .await
        .expect("rollback denied embedding job insert");

    let search_artifacts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM straylight.search_chunks WHERE user_id=$1 AND entry_id=$2),
          (SELECT count(*) FROM straylight.jobs
           WHERE user_id=$1 AND payload->>'entry_id'=$2::text)
        "#,
    )
    .bind(writer.user_id)
    .bind(own_entry.entry_id)
    .fetch_one(&pool)
    .await
    .expect("read conversation search artifacts");
    assert_eq!(
        search_artifacts,
        (0, 0),
        "canonical conversation entries create neither chunks nor embed jobs"
    );
}

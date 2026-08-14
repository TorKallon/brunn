use std::collections::HashSet;

use sqlx::{PgPool, Row, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use straylight::{
    auth::AuthContext,
    db::set_context,
    models::{CredentialId, UserId},
    secret_service::{decrypt_secret_value, encrypt_secret_value, secret_value_aad},
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
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping secret database test");
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
    let scope_ref = format!("scope:secret-{scope_id}");
    let capabilities: Vec<String> = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("secret-test:{label}:{user_id}"))
        .bind(format!("Secret test {label}"))
        .execute(pool)
        .await
        .expect("insert secret test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Secret test {label}"))
        .execute(pool)
        .await
        .expect("insert secret test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Secret test {label}"))
    .bind(format!("secret-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert secret test credential");
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
    .expect("grant secret test scope");
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

async fn insert_secret(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    principal: &Principal,
    secret_id: Uuid,
    name: &str,
    ciphertext: &[u8],
    nonce: &[u8],
    version: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO straylight.secrets (
          id,user_id,name,description,value_ciphertext,value_nonce,
          version,created_by_credential_id,updated_by_credential_id
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8)
        "#,
    )
    .bind(secret_id)
    .bind(principal.user_id)
    .bind(name)
    .bind("test secret")
    .bind(ciphertext)
    .bind(nonce)
    .bind(version)
    .bind(principal.credential_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn secret_schema_enforces_rls_capabilities_and_tenancy() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let vault = insert_principal(&pool, "vault", &["read", "secret:read", "secret:write"]).await;
    let memory = insert_principal(&pool, "memory", &["read", "save", "checkpoint"]).await;
    let neighbor =
        insert_principal(&pool, "neighbor", &["read", "secret:read", "secret:write"]).await;

    let rls_rows = sqlx::query(
        r#"
        SELECT class.relname,class.relrowsecurity,class.relforcerowsecurity
        FROM pg_class AS class
        JOIN pg_namespace AS namespace ON namespace.oid=class.relnamespace
        WHERE namespace.nspname='straylight'
          AND class.relname=ANY($1)
        ORDER BY class.relname
        "#,
    )
    .bind(vec!["secrets".to_owned(), "secret_access_log".to_owned()])
    .fetch_all(&pool)
    .await
    .expect("read RLS state");
    assert_eq!(rls_rows.len(), 2, "both secret tables exist");
    for row in &rls_rows {
        assert!(
            row.try_get::<bool, _>("relrowsecurity").unwrap(),
            "row security is enabled"
        );
        assert!(
            row.try_get::<bool, _>("relforcerowsecurity").unwrap(),
            "row security is forced"
        );
    }

    // Realistic round trip: encrypt, store under RLS, read back, decrypt.
    let key = [42u8; 32];
    let secret_id = Uuid::now_v7();
    let value = "canary-3f7c1d value\nwith a second line";
    let aad = secret_value_aad("development", vault.user_id, secret_id, 1);
    let (ciphertext, nonce) = encrypt_secret_value(&key, &aad, value).expect("encrypt");
    let mut tx = begin_as_app_rw(&pool, &vault.auth).await;
    insert_secret(
        &mut tx,
        &vault,
        secret_id,
        "deploy-key",
        &ciphertext,
        &nonce,
        1,
    )
    .await
    .expect("insert secret through secret:write RLS");
    sqlx::query(
        r#"
        INSERT INTO straylight.secret_access_log (
          user_id,secret_id,credential_id,operation
        ) VALUES ($1,$2,$3,'put')
        "#,
    )
    .bind(vault.user_id)
    .bind(secret_id)
    .bind(vault.credential_id)
    .execute(&mut *tx)
    .await
    .expect("record put through RLS");
    tx.commit().await.expect("commit vault secret");

    let mut tx = begin_as_app_rw(&pool, &vault.auth).await;
    let row = sqlx::query(
        "SELECT value_ciphertext,value_nonce,version FROM straylight.secrets \
         WHERE user_id=$1 AND name=$2",
    )
    .bind(vault.user_id)
    .bind("deploy-key")
    .fetch_one(&mut *tx)
    .await
    .expect("read secret through secret:read RLS");
    tx.rollback().await.expect("rollback read transaction");
    let stored_ciphertext: Vec<u8> = row.try_get("value_ciphertext").unwrap();
    let stored_nonce: Vec<u8> = row.try_get("value_nonce").unwrap();
    let stored_version: i32 = row.try_get("version").unwrap();
    let read_aad = secret_value_aad("development", vault.user_id, secret_id, stored_version);
    assert_eq!(
        decrypt_secret_value(&key, &read_aad, &stored_ciphertext, &stored_nonce).unwrap(),
        value
    );
    assert!(
        !String::from_utf8_lossy(&stored_ciphertext).contains("canary-3f7c1d"),
        "stored bytes never contain the plaintext canary"
    );

    // An ordinary memory credential sees no rows and cannot write.
    let mut tx = begin_as_app_rw(&pool, &memory.auth).await;
    let visible =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM straylight.secrets WHERE user_id=$1")
            .bind(memory.user_id)
            .fetch_one(&mut *tx)
            .await
            .expect("count secrets as memory credential");
    assert_eq!(visible, 0, "memory tokens cannot see secret rows");
    let denied = insert_secret(
        &mut tx,
        &memory,
        Uuid::now_v7(),
        "smuggled",
        &[7u8; 32],
        &[9u8; 12],
        1,
    )
    .await
    .expect_err("memory tokens cannot insert secrets");
    assert_eq!(database_code(&denied).as_deref(), Some("42501"));
    tx.rollback().await.ok();

    // Same-capability neighbor in another tenant sees nothing, even unbounded.
    let mut tx = begin_as_app_rw(&pool, &neighbor.auth).await;
    let visible = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM straylight.secrets")
        .fetch_one(&mut *tx)
        .await
        .expect("count all visible secrets as neighbor");
    assert_eq!(visible, 0, "tenancy isolates secret rows");
    tx.rollback().await.ok();

    // The access log is immutable.
    let mut tx = begin_as_app_rw(&pool, &vault.auth).await;
    let frozen =
        sqlx::query("UPDATE straylight.secret_access_log SET operation='get' WHERE user_id=$1")
            .bind(vault.user_id)
            .execute(&mut *tx)
            .await
            .expect_err("access log rows cannot be rewritten");
    assert!(database_code(&frozen).is_some());
    tx.rollback().await.ok();

    // Names are normalized lowercase identifiers and unique per user.
    let mut tx = begin_as_app_rw(&pool, &vault.auth).await;
    let bad_name = insert_secret(
        &mut tx,
        &vault,
        Uuid::now_v7(),
        "Not Valid",
        &[7u8; 32],
        &[9u8; 12],
        1,
    )
    .await
    .expect_err("mixed-case names violate the name check");
    assert_eq!(database_code(&bad_name).as_deref(), Some("23514"));
    tx.rollback().await.ok();
    let mut tx = begin_as_app_rw(&pool, &vault.auth).await;
    let duplicate = insert_secret(
        &mut tx,
        &vault,
        Uuid::now_v7(),
        "deploy-key",
        &[7u8; 32],
        &[9u8; 12],
        1,
    )
    .await
    .expect_err("duplicate names per user are rejected");
    assert_eq!(database_code(&duplicate).as_deref(), Some("23505"));
    tx.rollback().await.ok();
}

#[tokio::test]
async fn secret_capabilities_are_constrained_and_owner_credentials_carry_them() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };

    // Unknown capabilities remain rejected by the allowlist.
    let unknown = sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (id,user_id,label,token_hash,capabilities)
        SELECT $1,users.id,'bad caps',$2,ARRAY['secret:everything']
        FROM straylight.users LIMIT 1
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(format!("secret-test-bad-{}", Uuid::now_v7()))
    .execute(&pool)
    .await;
    if let Err(error) = unknown {
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }

    // Owner credentials must include the secret capabilities after backfill.
    let principal = insert_principal(&pool, "owner-check", &["read"]).await;
    let partial_owner = sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (id,user_id,label,token_hash,capabilities)
        VALUES ($1,$2,'partial owner',$3,ARRAY[
          'open','query','read','compute','verify','status',
          'checkpoint','save','stage','correct','delete','dream',
          'credential:manage','notification:publish','notification:manage','admin'
        ])
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.user_id)
    .bind(format!("secret-test-partial-{}", Uuid::now_v7()))
    .execute(&pool)
    .await
    .expect_err("credential:manage without secret capabilities violates the owner check");
    assert_eq!(database_code(&partial_owner).as_deref(), Some("23514"));

    let full_owner = sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (id,user_id,label,token_hash,capabilities)
        VALUES ($1,$2,'full owner',$3,ARRAY[
          'open','query','read','compute','verify','status',
          'checkpoint','save','stage','correct','delete','dream',
          'credential:manage','notification:publish','notification:manage',
          'secret:read','secret:write','admin'
        ])
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.user_id)
    .bind(format!("secret-test-full-{}", Uuid::now_v7()))
    .execute(&pool)
    .await;
    full_owner.expect("full owner credential satisfies both capability constraints");
}

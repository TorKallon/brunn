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
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping notification database test");
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

async fn insert_principal(pool: &PgPool, label: &str) -> Principal {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:notification-{scope_id}");
    let capabilities = vec![
        "read".to_owned(),
        "notification:publish".to_owned(),
        "notification:manage".to_owned(),
    ];
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("notification-test:{label}:{user_id}"))
        .bind(format!("Notification test {label}"))
        .execute(pool)
        .await
        .expect("insert notification test user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(format!("Notification test {label}"))
        .execute(pool)
        .await
        .expect("insert notification test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("Notification test {label}"))
    .bind(format!("notification-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert notification test credential");
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
    .expect("grant notification test scope");
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

async fn insert_manage_credential(pool: &PgPool, principal: &Principal, label: &str) -> Principal {
    let credential_id = Uuid::now_v7();
    let capabilities = vec!["read".to_owned(), "notification:manage".to_owned()];
    let scope_ref = principal.auth.scope_refs[0].clone();
    let scope_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM straylight.scopes WHERE user_id=$1 AND scope_ref=$2",
    )
    .bind(principal.user_id)
    .bind(&scope_ref)
    .fetch_one(pool)
    .await
    .expect("read notification test scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(principal.user_id)
    .bind(format!("Notification manage test {label}"))
    .bind(format!("notification-manage-test-token-{credential_id}"))
    .bind(&capabilities)
    .execute(pool)
    .await
    .expect("insert notification manage credential");
    sqlx::query(
        r#"
        INSERT INTO straylight.credential_scope_grants (
          credential_id,user_id,scope_id
        ) VALUES ($1,$2,$3)
        "#,
    )
    .bind(credential_id)
    .bind(principal.user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant notification manage test scope");
    Principal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(principal.user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: false,
        },
        user_id: principal.user_id,
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

async fn insert_notification(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    principal: &Principal,
    notification_id: Uuid,
    event_key: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO straylight.notifications (
          id,user_id,producer_credential_id,event_key,request_hash,
          correlation_id,kind,importance,title,body,target,occurred_at,expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,$6,'news_alert','important',$7,$8,$9,
          clock_timestamp(),clock_timestamp()+interval '1 day'
        )
        "#,
    )
    .bind(notification_id)
    .bind(principal.user_id)
    .bind(principal.credential_id)
    .bind(event_key)
    .bind("a".repeat(64))
    .bind(format!("correlation:{notification_id}"))
    .bind("Notification database test")
    .bind("Private detail remains in Postgres.")
    .bind(json!({"type": "notification"}))
    .execute(&mut **tx)
    .await
    .expect("insert notification through app_rw RLS");
}

async fn insert_installation(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    principal: &Principal,
    client_installation_id: Uuid,
    token_byte: u8,
    nonce_byte: u8,
    token_hash: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO straylight.notification_installations (
          user_id,client_installation_id,registered_by_credential_id,
          platform,environment,app_id,
          token_ciphertext,token_nonce,token_hash,preview
        ) VALUES (
          $1,$2,$3,'ios','development','com.example.Straylight',$4,$5,$6,'generic'
        )
        RETURNING id
        "#,
    )
    .bind(principal.user_id)
    .bind(client_installation_id)
    .bind(principal.credential_id)
    .bind(vec![token_byte; 80])
    .bind(vec![nonce_byte; 12])
    .bind(token_hash)
    .fetch_one(&mut **tx)
    .await
    .expect("insert installation through notification:manage RLS")
}

#[tokio::test]
async fn notification_schema_enforces_tenancy_transport_truth_and_receipt_attribution() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let first = insert_principal(&pool, "first").await;
    let second = insert_principal(&pool, "second").await;

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
    .bind(vec![
        "notifications",
        "notification_user_state",
        "notification_installations",
        "notification_deliveries",
        "notification_attempts",
        "notification_receipts",
    ])
    .fetch_all(&pool)
    .await
    .expect("read notification RLS metadata");
    assert_eq!(rls_rows.len(), 6);
    assert!(rls_rows.iter().all(|row| {
        row.get::<bool, _>("relrowsecurity") && row.get::<bool, _>("relforcerowsecurity")
    }));

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
            WHERE namespace.nspname='straylight_auth'
              AND procedure.proname=$1
            "#,
        )
        .bind(function_name)
        .fetch_one(&pool)
        .await
        .expect("read credential function definition");
        assert!(
            definition.contains("notification:publish"),
            "{function_name}"
        );
        assert!(
            definition.contains("notification:manage"),
            "{function_name}"
        );
    }

    let first_notification = Uuid::now_v7();
    let second_notification = Uuid::now_v7();
    let first_client_installation = Uuid::now_v7();
    let first_token_hash = first_client_installation.simple().to_string().repeat(2);
    let first_delivery = Uuid::now_v7();
    let second_client_installation = Uuid::now_v7();
    let second_token_hash = second_client_installation.simple().to_string().repeat(2);
    let second_delivery = Uuid::now_v7();

    let mut first_tx = begin_as_app_rw(&pool, &first.auth).await;
    insert_notification(
        &mut first_tx,
        &first,
        first_notification,
        &format!("event:{first_notification}"),
    )
    .await;
    let first_installation = insert_installation(
        &mut first_tx,
        &first,
        first_client_installation,
        7,
        8,
        &first_token_hash,
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO straylight.notification_deliveries (
          id,user_id,notification_id,installation_id
        ) VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(first_delivery)
    .bind(first.user_id)
    .bind(first_notification)
    .bind(first_installation)
    .execute(&mut *first_tx)
    .await
    .expect("insert first delivery through publish RLS");
    first_tx
        .commit()
        .await
        .expect("commit first notification fixture");

    sqlx::query(
        r#"
        UPDATE straylight.notification_deliveries
        SET provider_block_count=provider_block_count+1,
            available_at=clock_timestamp()+interval '1 minute',
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(first.user_id)
    .bind(first_delivery)
    .execute(&pool)
    .await
    .expect("record schema-level provider-auth block requeue");
    let retry_accounting = sqlx::query(
        r#"
        SELECT attempt_count,provider_block_count
        FROM straylight.notification_deliveries
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(first.user_id)
    .bind(first_delivery)
    .fetch_one(&pool)
    .await
    .expect("read provider-auth retry accounting");
    assert_eq!(retry_accounting.get::<i32, _>("attempt_count"), 0);
    assert_eq!(retry_accounting.get::<i32, _>("provider_block_count"), 1);

    let mut second_tx = begin_as_app_rw(&pool, &second.auth).await;
    insert_notification(
        &mut second_tx,
        &second,
        second_notification,
        &format!("event:{second_notification}"),
    )
    .await;
    let second_installation = insert_installation(
        &mut second_tx,
        &second,
        second_client_installation,
        9,
        10,
        &second_token_hash,
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO straylight.notification_deliveries (
          id,user_id,notification_id,installation_id
        ) VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(second_delivery)
    .bind(second.user_id)
    .bind(second_notification)
    .bind(second_installation)
    .execute(&mut *second_tx)
    .await
    .expect("insert second delivery through publish RLS");
    second_tx
        .commit()
        .await
        .expect("commit second notification fixture");

    let mut isolation_tx = begin_as_app_rw(&pool, &first.auth).await;
    let visible_second = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM straylight.notifications WHERE id=$1)",
    )
    .bind(second_notification)
    .fetch_one(&mut *isolation_tx)
    .await
    .expect("query cross-user notification through RLS");
    assert!(!visible_second);
    isolation_tx
        .rollback()
        .await
        .expect("rollback isolation query");

    let mut cross_account_write_tx = begin_as_app_rw(&pool, &first.auth).await;
    let cross_account_write = sqlx::query(
        r#"
        INSERT INTO straylight.notifications (
          id,user_id,producer_credential_id,event_key,request_hash,
          correlation_id,kind,importance,title,body,target,occurred_at,expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,$6,'news_alert','normal','Cross account','Denied',
          '{"type":"notification"}'::jsonb,
          clock_timestamp(),clock_timestamp()+interval '1 day'
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(second.user_id)
    .bind(second.credential_id)
    .bind(format!("cross-account:{}", Uuid::now_v7()))
    .bind("d".repeat(64))
    .bind(format!("cross-account:{}", Uuid::now_v7()))
    .execute(&mut *cross_account_write_tx)
    .await
    .expect_err("RLS must reject a cross-account notification insert");
    assert_eq!(
        database_code(&cross_account_write).as_deref(),
        Some("42501")
    );
    cross_account_write_tx
        .rollback()
        .await
        .expect("rollback cross-account write");

    let mut invalid_transport_tx = begin_as_app_rw(&pool, &first.auth).await;
    let invalid_transport =
        sqlx::query("UPDATE straylight.notification_deliveries SET state='opened' WHERE id=$1")
            .bind(first_delivery)
            .execute(&mut *invalid_transport_tx)
            .await
            .expect_err("opened is user state, never a delivery transport state");
    assert_eq!(database_code(&invalid_transport).as_deref(), Some("23514"));
    invalid_transport_tx
        .rollback()
        .await
        .expect("rollback invalid transport state");

    let mut mismatched_receipt_tx = begin_as_app_rw(&pool, &first.auth).await;
    let mismatched_receipt = sqlx::query(
        r#"
        INSERT INTO straylight.notification_receipts (
          user_id,notification_id,delivery_id,kind,recorded_by_credential_id
        ) VALUES ($1,$2,$3,'opened',$4)
        "#,
    )
    .bind(first.user_id)
    .bind(first_notification)
    .bind(second_delivery)
    .bind(first.credential_id)
    .execute(&mut *mismatched_receipt_tx)
    .await
    .expect_err("receipt delivery must belong to the same user and notification");
    assert_eq!(database_code(&mismatched_receipt).as_deref(), Some("23503"));
    mismatched_receipt_tx
        .rollback()
        .await
        .expect("rollback mismatched receipt");

    let mut valid_receipt_tx = begin_as_app_rw(&pool, &first.auth).await;
    let valid_receipt = sqlx::query(
        r#"
        INSERT INTO straylight.notification_receipts (
          user_id,notification_id,delivery_id,kind,recorded_by_credential_id
        ) VALUES ($1,$2,$3,'opened',$4)
        "#,
    )
    .bind(first.user_id)
    .bind(first_notification)
    .bind(first_delivery)
    .bind(first.credential_id)
    .execute(&mut *valid_receipt_tx)
    .await
    .expect("insert valid attributed receipt");
    assert_eq!(valid_receipt.rows_affected(), 1);
    valid_receipt_tx
        .commit()
        .await
        .expect("commit valid receipt");

    sqlx::query("UPDATE straylight.api_credentials SET disabled_at=clock_timestamp() WHERE id=$1")
        .bind(first.credential_id)
        .execute(&pool)
        .await
        .expect("disable registration credential");
    let lifecycle = sqlx::query(
        r#"
        SELECT installation.enabled,installation.revoked_at,
               installation.token_ciphertext,installation.token_nonce,
               installation.token_hash,delivery.state,delivery.last_error_code
        FROM straylight.notification_installations AS installation
        JOIN straylight.notification_deliveries AS delivery
          ON delivery.user_id=installation.user_id
         AND delivery.installation_id=installation.id
        WHERE installation.user_id=$1 AND installation.id=$2 AND delivery.id=$3
        "#,
    )
    .bind(first.user_id)
    .bind(first_installation)
    .bind(first_delivery)
    .fetch_one(&pool)
    .await
    .expect("read revoked credential notification lifecycle");
    assert!(!lifecycle.get::<bool, _>("enabled"));
    assert!(
        lifecycle
            .try_get::<chrono::DateTime<chrono::Utc>, _>("revoked_at")
            .is_ok()
    );
    assert_eq!(lifecycle.get::<String, _>("state"), "expired");
    assert_eq!(
        lifecycle.get::<String, _>("last_error_code"),
        "registration_credential_revoked"
    );
    assert_eq!(
        lifecycle.get::<Option<Vec<u8>>, _>("token_ciphertext"),
        None
    );
    assert_eq!(lifecycle.get::<Option<Vec<u8>>, _>("token_nonce"), None);
    assert_eq!(lifecycle.get::<Option<String>, _>("token_hash"), None);
}

#[tokio::test]
async fn installation_claims_support_account_switch_and_same_user_management() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let first = insert_principal(&pool, "account-switch-first").await;
    let second = insert_principal(&pool, "account-switch-second").await;
    let client_installation_id = Uuid::now_v7();
    let shared_token_hash = client_installation_id.simple().to_string().repeat(2);
    let first_notification = Uuid::now_v7();
    let first_delivery = Uuid::now_v7();

    let mut first_tx = begin_as_app_rw(&pool, &first.auth).await;
    insert_notification(
        &mut first_tx,
        &first,
        first_notification,
        &format!("account-switch:{first_notification}"),
    )
    .await;
    let first_installation = insert_installation(
        &mut first_tx,
        &first,
        client_installation_id,
        11,
        12,
        &shared_token_hash,
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO straylight.notification_deliveries (
          id,user_id,notification_id,installation_id
        ) VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(first_delivery)
    .bind(first.user_id)
    .bind(first_notification)
    .bind(first_installation)
    .execute(&mut *first_tx)
    .await
    .expect("insert pre-switch delivery");
    first_tx.commit().await.expect("commit pre-switch fixture");

    let mut duplicate_token_tx = begin_as_app_rw(&pool, &second.auth).await;
    let duplicate_token = sqlx::query(
        r#"
        INSERT INTO straylight.notification_installations (
          user_id,client_installation_id,registered_by_credential_id,
          platform,environment,app_id,
          token_ciphertext,token_nonce,token_hash,preview
        ) VALUES (
          $1,$2,$3,'ios','development','com.example.Straylight',$4,$5,$6,'generic'
        )
        "#,
    )
    .bind(second.user_id)
    .bind(client_installation_id)
    .bind(second.credential_id)
    .bind(vec![13_u8; 80])
    .bind(vec![14_u8; 12])
    .bind(&shared_token_hash)
    .execute(&mut *duplicate_token_tx)
    .await
    .expect_err("one live APNs token cannot belong to two accounts");
    assert_eq!(database_code(&duplicate_token).as_deref(), Some("23505"));
    duplicate_token_tx
        .rollback()
        .await
        .expect("rollback duplicate live token");

    let mut switch_tx = begin_as_app_rw(&pool, &second.auth).await;
    let reassigned = sqlx::query_scalar::<_, i64>(
        "SELECT straylight.claim_notification_device_token($1,$2,$3,$4)",
    )
    .bind(client_installation_id)
    .bind("development")
    .bind("com.example.Straylight")
    .bind(&shared_token_hash)
    .fetch_one(&mut *switch_tx)
    .await
    .expect("claim live token for the newly authenticated account");
    assert_eq!(reassigned, 1);
    let second_installation = insert_installation(
        &mut switch_tx,
        &second,
        client_installation_id,
        13,
        14,
        &shared_token_hash,
    )
    .await;
    switch_tx.commit().await.expect("commit account switch");

    assert_ne!(first_installation, second_installation);
    let installation_rows = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.notification_installations WHERE client_installation_id=$1",
    )
    .bind(client_installation_id)
    .fetch_one(&pool)
    .await
    .expect("count account-scoped installation history");
    assert_eq!(installation_rows, 2);
    let live_token_rows = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM straylight.notification_installations
        WHERE environment='development' AND app_id='com.example.Straylight'
          AND token_hash=$1 AND enabled
        "#,
    )
    .bind(&shared_token_hash)
    .fetch_one(&pool)
    .await
    .expect("count globally live provider token ownership");
    assert_eq!(live_token_rows, 1);

    let old_assignment = sqlx::query(
        r#"
        SELECT installation.enabled,installation.revoked_at,
               installation.token_ciphertext,installation.token_nonce,
               installation.token_hash,delivery.state,delivery.last_error_code
        FROM straylight.notification_installations AS installation
        JOIN straylight.notification_deliveries AS delivery
          ON delivery.user_id=installation.user_id
         AND delivery.installation_id=installation.id
        WHERE installation.user_id=$1 AND installation.id=$2 AND delivery.id=$3
        "#,
    )
    .bind(first.user_id)
    .bind(first_installation)
    .bind(first_delivery)
    .fetch_one(&pool)
    .await
    .expect("read historical assignment after account switch");
    assert!(!old_assignment.get::<bool, _>("enabled"));
    assert!(old_assignment
        .try_get::<chrono::DateTime<chrono::Utc>, _>("revoked_at")
        .is_ok());
    assert_eq!(
        old_assignment.get::<Option<Vec<u8>>, _>("token_ciphertext"),
        None
    );
    assert_eq!(
        old_assignment.get::<Option<Vec<u8>>, _>("token_nonce"),
        None
    );
    assert_eq!(
        old_assignment.get::<Option<String>, _>("token_hash"),
        None
    );
    assert_eq!(old_assignment.get::<String, _>("state"), "expired");
    assert_eq!(
        old_assignment.get::<String, _>("last_error_code"),
        "installation_reassigned"
    );

    for (principal, expected_installation) in [
        (&first, first_installation),
        (&second, second_installation),
    ] {
        let mut tx = begin_as_app_rw(&pool, &principal.auth).await;
        let visible = sqlx::query_scalar::<_, Vec<Uuid>>(
            r#"
            SELECT coalesce(array_agg(id ORDER BY id),'{}'::uuid[])
            FROM straylight.notification_installations
            WHERE client_installation_id=$1
            "#,
        )
        .bind(client_installation_id)
        .fetch_one(&mut *tx)
        .await
        .expect("read account-scoped installation through RLS");
        assert_eq!(visible, vec![expected_installation]);
        tx.rollback().await.expect("rollback RLS visibility check");
    }

    let mut cross_account_update_tx = begin_as_app_rw(&pool, &first.auth).await;
    let cross_account_updated = sqlx::query(
        "UPDATE straylight.notification_installations SET last_seen_at=clock_timestamp() WHERE id=$1",
    )
    .bind(second_installation)
    .execute(&mut *cross_account_update_tx)
    .await
    .expect("cross-account installation update is hidden by RLS");
    assert_eq!(cross_account_updated.rows_affected(), 0);
    cross_account_update_tx
        .rollback()
        .await
        .expect("rollback cross-account installation update");

    let second_manager = insert_manage_credential(&pool, &second, "account-switch").await;
    let replacement_token_hash = Uuid::now_v7().simple().to_string().repeat(2);
    let mut manage_tx = begin_as_app_rw(&pool, &second_manager.auth).await;
    let updated_installation = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE straylight.notification_installations
        SET registered_by_credential_id=$1,token_ciphertext=$2,token_nonce=$3,
            token_hash=$4,last_seen_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE user_id=$5 AND client_installation_id=$6
        RETURNING id
        "#,
    )
    .bind(second_manager.credential_id)
    .bind(vec![15_u8; 80])
    .bind(vec![16_u8; 12])
    .bind(&replacement_token_hash)
    .bind(second.user_id)
    .bind(client_installation_id)
    .fetch_one(&mut *manage_tx)
    .await
    .expect("same-user notification:manage credential updates installation");
    assert_eq!(updated_installation, second_installation);
    let manager_visible = sqlx::query_scalar::<_, String>(
        "SELECT token_hash FROM straylight.notification_installations WHERE id=$1",
    )
    .bind(second_installation)
    .fetch_one(&mut *manage_tx)
    .await
    .expect("same-user manager reads updated installation");
    assert_eq!(manager_visible, replacement_token_hash);
    let revoked_installation = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE straylight.notification_installations
        SET enabled=false,revoked_at=clock_timestamp(),
            token_ciphertext=NULL,token_nonce=NULL,token_hash=NULL,
            updated_at=clock_timestamp()
        WHERE user_id=$1 AND client_installation_id=$2
        RETURNING id
        "#,
    )
    .bind(second.user_id)
    .bind(client_installation_id)
    .fetch_one(&mut *manage_tx)
    .await
    .expect("same-user notification:manage credential revokes installation");
    assert_eq!(revoked_installation, second_installation);
    manage_tx
        .commit()
        .await
        .expect("commit same-user management");

    let revoked = sqlx::query(
        r#"
        SELECT enabled,revoked_at,token_ciphertext,token_nonce,token_hash
        FROM straylight.notification_installations
        WHERE user_id=$1 AND id=$2
        "#,
    )
    .bind(second.user_id)
    .bind(second_installation)
    .fetch_one(&pool)
    .await
    .expect("read same-user revoked installation");
    assert!(!revoked.get::<bool, _>("enabled"));
    assert!(revoked
        .try_get::<chrono::DateTime<chrono::Utc>, _>("revoked_at")
        .is_ok());
    assert_eq!(revoked.get::<Option<Vec<u8>>, _>("token_ciphertext"), None);
    assert_eq!(revoked.get::<Option<Vec<u8>>, _>("token_nonce"), None);
    assert_eq!(revoked.get::<Option<String>, _>("token_hash"), None);
}

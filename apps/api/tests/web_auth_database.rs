use chrono::{Duration as ChronoDuration, Utc};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

use brunn::{auth::hash_token, operator_service};

async fn connect_test_pool() -> Option<(String, PgPool)> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping web auth database test");
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
    Some((database_url, pool))
}

#[tokio::test]
async fn web_identity_sessions_resets_and_account_purge_are_fail_closed() {
    let Some((database_url, pool)) = connect_test_pool().await else {
        return;
    };
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let suffix = user_id.simple().to_string();
    let username = format!("owner-{}", &suffix[..12]);
    let email = format!("{username}@example.com");
    let updated_email = format!("updated-{username}@example.com");
    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("web-auth-test:{user_id}"))
        .bind("Web auth integration test")
        .execute(&pool)
        .await
        .expect("insert web auth test user");
    sqlx::query("INSERT INTO brunn.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(format!("scope:web-auth-test:{scope_id}"))
        .bind("Web auth integration test")
        .execute(&pool)
        .await
        .expect("insert web auth test scope");

    let configured = operator_service::configure_web_identity(
        &database_url,
        &user_id.to_string(),
        &username,
        &email,
    )
    .await
    .expect("configure initial web identity");
    assert_eq!(configured["password_status"], "reset_required");
    assert_eq!(
        configured["web_credential"]["bearer_token_status"],
        "discarded"
    );
    assert!(configured["web_credential"].get("token").is_none());

    let app_rw_can_create_web_sessions = sqlx::query_scalar::<_, bool>(
        "SELECT has_function_privilege('app_rw', 'brunn_auth.create_web_session(uuid,text,timestamptz,text,text)', 'EXECUTE')",
    )
    .fetch_one(&pool)
    .await
    .expect("check Web-session function privilege");
    assert!(app_rw_can_create_web_sessions);

    let identity = sqlx::query(
        "SELECT password_hash,web_credential_id FROM brunn.web_identities WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("read configured web identity");
    assert!(
        identity
            .try_get::<Option<String>, _>("password_hash")
            .unwrap()
            .is_none()
    );
    let web_credential_id: Uuid = identity.try_get("web_credential_id").unwrap();
    for login_identifier in [&username, &email] {
        let resolved_user_id =
            sqlx::query_scalar::<_, Uuid>("SELECT user_id FROM brunn_auth.lookup_web_identity($1)")
                .bind(login_identifier)
                .fetch_one(&pool)
                .await
                .expect("resolve Web identity by login alias");
        assert_eq!(resolved_user_id, user_id);
    }
    let grants = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.credential_scope_grants WHERE user_id=$1 AND credential_id=$2",
    )
    .bind(user_id)
    .bind(web_credential_id)
    .fetch_one(&pool)
    .await
    .expect("count web principal grants");
    let user_scopes =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.scopes WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("count user scopes");
    assert!(user_scopes > 0);
    assert_eq!(grants, user_scopes);

    sqlx::query(
        "UPDATE brunn.web_identities SET password_hash='$argon2id$fixture' WHERE user_id=$1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("establish fixture password");
    let overlong_session =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(hash_token(&format!("overlong-session:{user_id}")))
            .bind(Utc::now() + ChronoDuration::days(31))
            .bind("$argon2id$fixture")
            .bind(&username)
            .fetch_one(&pool)
            .await
            .expect_err("a web session must not exceed the 30-day lifetime");
    assert_eq!(
        overlong_session
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("22023")
    );
    let stale_login =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(hash_token(&format!("stale-session:{user_id}")))
            .bind(Utc::now() + ChronoDuration::days(30))
            .bind("$argon2id$stale")
            .bind(&username)
            .fetch_one(&pool)
            .await
            .expect_err("a stale verified password hash must not create a session");
    assert_eq!(
        stale_login
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("P0002")
    );
    let stale_username_login =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(hash_token(&format!("stale-username-session:{user_id}")))
            .bind(Utc::now() + ChronoDuration::days(30))
            .bind("$argon2id$fixture")
            .bind(format!("stale-{username}"))
            .fetch_one(&pool)
            .await
            .expect_err("a stale verified username must not create a session");
    assert_eq!(
        stale_username_login
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("P0002")
    );
    let first_session_hash = hash_token(&format!("first-session:{user_id}"));
    let first_session_id =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(&first_session_hash)
            .bind(Utc::now() + ChronoDuration::days(30))
            .bind("$argon2id$fixture")
            .bind(&email)
            .fetch_one(&pool)
            .await
            .expect("create first web session with the account email");
    let first_session_has_30_day_lifetime = sqlx::query_scalar::<_, bool>(
        "SELECT expires_at - created_at > interval '29 days' FROM brunn.web_sessions WHERE id=$1",
    )
    .bind(first_session_id)
    .fetch_one(&pool)
    .await
    .expect("read first web session lifetime");
    assert!(first_session_has_30_day_lifetime);

    let reconfigured = operator_service::configure_web_identity(
        &database_url,
        &format!("user:{user_id}"),
        &username,
        &email,
    )
    .await
    .expect("idempotently reconfigure web identity");
    assert_eq!(reconfigured["password_status"], "configured");
    assert_eq!(reconfigured["web_credential"]["created"], false);
    assert_session_revoked(&pool, first_session_id).await;

    let second_session_hash = hash_token(&format!("second-session:{user_id}"));
    let second_session_id =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(&second_session_hash)
            .bind(Utc::now() + ChronoDuration::days(30))
            .bind("$argon2id$fixture")
            .bind(&username)
            .fetch_one(&pool)
            .await
            .expect("create second web session");
    let disable_reset_hash = hash_token(&format!("disable-reset:{user_id}"));
    let disable_reset_id =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.issue_password_reset($1,$2,$3,$4)")
            .bind(user_id)
            .bind(&disable_reset_hash)
            .bind(Utc::now() + ChronoDuration::minutes(30))
            .bind(&email)
            .fetch_one(&pool)
            .await
            .expect("issue reset before disabling web principal");
    sqlx::query("UPDATE brunn.api_credentials SET disabled_at=clock_timestamp() WHERE id=$1")
        .bind(web_credential_id)
        .execute(&pool)
        .await
        .expect("disable web principal");
    assert_session_revoked(&pool, second_session_id).await;
    assert_reset_used(&pool, disable_reset_id).await;
    operator_service::configure_web_identity(
        &database_url,
        &user_id.to_string(),
        &username,
        &email,
    )
    .await
    .expect("re-enable web principal without resurrecting sessions");
    assert_session_revoked(&pool, second_session_id).await;

    let old_email_reset_hash = hash_token(&format!("old-email-reset:{user_id}"));
    let old_email_reset_id =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.issue_password_reset($1,$2,$3,$4)")
            .bind(user_id)
            .bind(&old_email_reset_hash)
            .bind(Utc::now() + ChronoDuration::minutes(30))
            .bind(&email)
            .fetch_one(&pool)
            .await
            .expect("issue reset before email reconfiguration");
    operator_service::configure_web_identity(
        &database_url,
        &user_id.to_string(),
        &username,
        &updated_email,
    )
    .await
    .expect("change web identity email");
    assert_reset_used(&pool, old_email_reset_id).await;
    let stale_email_login =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(hash_token(&format!("stale-email-session:{user_id}")))
            .bind(Utc::now() + ChronoDuration::days(30))
            .bind("$argon2id$fixture")
            .bind(&email)
            .fetch_one(&pool)
            .await
            .expect_err("the pre-change email must not create a web session");
    assert_eq!(
        stale_email_login
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("P0002")
    );
    let stale_email_issue =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.issue_password_reset($1,$2,$3,$4)")
            .bind(user_id)
            .bind(hash_token(&format!("stale-email-issue:{user_id}")))
            .bind(Utc::now() + ChronoDuration::minutes(30))
            .bind(&email)
            .fetch_one(&pool)
            .await
            .expect_err("a reset must not be issued for the pre-change email");
    assert_eq!(
        stale_email_issue
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("P0002")
    );

    let third_session_hash = hash_token(&format!("third-session:{user_id}"));
    let third_session_id =
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.create_web_session($1,$2,$3,$4,$5)")
            .bind(user_id)
            .bind(&third_session_hash)
            .bind(Utc::now() + ChronoDuration::days(30))
            .bind("$argon2id$fixture")
            .bind(&updated_email)
            .fetch_one(&pool)
            .await
            .expect("create third web session with the updated account email");
    let first_reset_hash = hash_token(&format!("first-reset:{user_id}"));
    let second_reset_hash = hash_token(&format!("second-reset:{user_id}"));
    for reset_hash in [&first_reset_hash, &second_reset_hash] {
        sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.issue_password_reset($1,$2,$3,$4)")
            .bind(user_id)
            .bind(reset_hash)
            .bind(Utc::now() + ChronoDuration::minutes(30))
            .bind(&updated_email)
            .fetch_one(&pool)
            .await
            .expect("issue password reset");
    }
    let active_resets = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.password_reset_tokens WHERE user_id=$1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("count simultaneously valid reset tokens");
    assert_eq!(active_resets, 2);
    sqlx::query_scalar::<_, Uuid>("SELECT brunn_auth.consume_password_reset($1,$2)")
        .bind(&second_reset_hash)
        .bind("$argon2id$replacement")
        .fetch_one(&pool)
        .await
        .expect("consume latest delivered reset token");
    assert_session_revoked(&pool, third_session_id).await;
    let remaining_resets = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.password_reset_tokens WHERE user_id=$1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("count reset tokens after password change");
    assert_eq!(remaining_resets, 0);

    sqlx::query(
        "INSERT INTO brunn.web_auth_rate_limits (kind,identifier_hash,user_id) VALUES ('login',$1,$2)",
    )
    .bind(hash_token(&format!("rate-limit:{user_id}")))
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("insert rate-limit fixture");
    let deletion_request_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO brunn.account_deletion_requests (
          id,user_id,requested_by_credential_id,status,confirmation_hash,reason,
          backup_expiry_due_at
        ) VALUES ($1,$2,$3,'queued',$4,'web auth purge test',$5)
        "#,
    )
    .bind(deletion_request_id)
    .bind(user_id)
    .bind(web_credential_id)
    .bind(hash_token("confirmed"))
    .bind(Utc::now() + ChronoDuration::days(30))
    .execute(&pool)
    .await
    .expect("insert account deletion request");
    sqlx::query(
        "UPDATE brunn.users SET account_status='deleting',deletion_requested_at=clock_timestamp() WHERE id=$1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("activate account deletion fence");
    sqlx::query_scalar::<_, serde_json::Value>("SELECT brunn.purge_account_user_rows($1)")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("purge user-owned rows");
    for (table, query) in [
        (
            "web_identities",
            "SELECT count(*) FROM brunn.web_identities WHERE user_id=$1",
        ),
        (
            "web_sessions",
            "SELECT count(*) FROM brunn.web_sessions WHERE user_id=$1",
        ),
        (
            "password_reset_tokens",
            "SELECT count(*) FROM brunn.password_reset_tokens WHERE user_id=$1",
        ),
        (
            "web_auth_rate_limits",
            "SELECT count(*) FROM brunn.web_auth_rate_limits WHERE user_id=$1",
        ),
    ] {
        let count = sqlx::query_scalar::<_, i64>(query)
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("count web auth rows after purge");
        assert_eq!(count, 0, "{table} rows survived account purge");
    }
}

async fn assert_session_revoked(pool: &PgPool, session_id: Uuid) {
    let revoked = sqlx::query_scalar::<_, bool>(
        "SELECT revoked_at IS NOT NULL FROM brunn.web_sessions WHERE id=$1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("read web session revocation state");
    assert!(revoked, "web session {session_id} remained active");
}

async fn assert_reset_used(pool: &PgPool, reset_id: Uuid) {
    let used = sqlx::query_scalar::<_, bool>(
        "SELECT used_at IS NOT NULL FROM brunn.password_reset_tokens WHERE id=$1",
    )
    .bind(reset_id)
    .fetch_one(pool)
    .await
    .expect("read password reset consumption state");
    assert!(used, "password reset token {reset_id} remained active");
}

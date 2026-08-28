use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

use straylight::{AppState, Config, auth::hash_token, router};

const MESSAGING_ROOT: &str = "/v1/workspace/messaging";
const CLIENT_KEY_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

#[derive(Debug)]
struct HttpResponse {
    status: StatusCode,
    body: Value,
}

struct CredentialFixture {
    id: Uuid,
    token: String,
}

struct WorkspaceFixture {
    owner: CredentialFixture,
    agent_writer: CredentialFixture,
    agent_reader: CredentialFixture,
    unbound_writer: CredentialFixture,
}

fn database_url_as_role(database_url: &str, role: &str) -> String {
    let mut url = Url::parse(database_url).expect("parse disposable PostgreSQL URL");
    url.query_pairs_mut()
        .append_pair("options", &format!("-c role={role}"));
    url.into()
}

async fn connect_test_state() -> Option<(PgPool, AppState)> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping messaging endpoint contract");
        return None;
    };

    let seed_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&seed_pool)
        .await
        .expect("apply Straylight migrations");

    // The endpoint contract intentionally uses the production router and AppState.
    // Its isolated-stack invocation supplies the normal disposable object-store
    // configuration in addition to STRAYLIGHT_TEST_DATABASE_URL.
    let mut config = Config::from_env().expect("load disposable API configuration");
    let app_database_url = database_url_as_role(&database_url, "app_rw");
    config.database_url_rw = app_database_url.clone();
    config.database_url_ro = app_database_url;
    config.database_url_admin = None;
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    let state = AppState::connect(config)
        .await
        .expect("connect disposable API state");
    Some((seed_pool, state))
}

async fn insert_user(pool: &PgPool, label: &str) -> (Uuid, Uuid) {
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("messaging-endpoint-test:{label}:{user_id}"))
        .bind(format!("Messaging endpoint {label}"))
        .execute(pool)
        .await
        .expect("insert messaging endpoint user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(format!("scope:messaging-endpoint-{scope_id}"))
        .bind(format!("Messaging endpoint {label}"))
        .execute(pool)
        .await
        .expect("insert messaging endpoint scope");
    (user_id, scope_id)
}

async fn insert_credential(
    pool: &PgPool,
    user_id: Uuid,
    scope_id: Uuid,
    label: &str,
    capabilities: &[&str],
) -> CredentialFixture {
    let id = Uuid::now_v7();
    let token = format!("messaging-endpoint-test-{}", Uuid::now_v7());
    let capabilities = capabilities
        .iter()
        .map(|capability| (*capability).to_owned())
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(label)
    .bind(hash_token(&token))
    .bind(capabilities)
    .execute(pool)
    .await
    .expect("insert messaging endpoint credential");
    sqlx::query(
        r#"
        INSERT INTO straylight.credential_scope_grants (
          credential_id,user_id,scope_id
        ) VALUES ($1,$2,$3)
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant messaging endpoint scope");
    CredentialFixture { id, token }
}

async fn insert_agent(
    pool: &PgPool,
    user_id: Uuid,
    creator_id: Uuid,
    agent_id: &str,
    principal_kind: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_agents (
          user_id,agent_id,display_name,principal_kind,delivery_mode,
          created_by_credential_id
        ) VALUES ($1,$2,$3,$4,'pull',$5)
        "#,
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(format!("Endpoint {agent_id}"))
    .bind(principal_kind)
    .bind(creator_id)
    .execute(pool)
    .await
    .expect("insert messaging endpoint principal");
}

async fn bind_credential(
    pool: &PgPool,
    user_id: Uuid,
    credential_id: Uuid,
    agent_id: &str,
    owner_id: Uuid,
) {
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_credential_bindings (
          user_id,credential_id,agent_id,bound_by_credential_id
        ) VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(user_id)
    .bind(credential_id)
    .bind(agent_id)
    .bind(owner_id)
    .execute(pool)
    .await
    .expect("bind messaging endpoint credential");
}

async fn insert_web_session(pool: &PgPool, user_id: Uuid, credential_id: Uuid) -> (String, String) {
    let username = format!("messaging-bind-{}", user_id.simple());
    let email = format!("{username}@example.test");
    sqlx::query(
        r#"
        INSERT INTO straylight.web_identities (
          user_id,username,username_normalized,email,email_normalized,
          password_hash,web_credential_id
        ) VALUES ($1,$2,$2,$3,$3,'$argon2id$fixture',$4)
        "#,
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .bind(credential_id)
    .execute(pool)
    .await
    .expect("insert messaging binding Web identity");
    let session_token = format!("sws_{}{}", credential_id.simple(), "0".repeat(11));
    sqlx::query(
        "INSERT INTO straylight.web_sessions(user_id,credential_id,token_hash,expires_at) VALUES($1,$2,$3,clock_timestamp()+interval '1 hour')",
    )
    .bind(user_id)
    .bind(credential_id)
    .bind(hash_token(&session_token))
    .execute(pool)
    .await
    .expect("insert messaging binding Web session");
    let mut digest = Sha256::new();
    digest.update(b"straylight.web-csrf.v1\0");
    digest.update(session_token.as_bytes());
    (session_token, URL_SAFE_NO_PAD.encode(digest.finalize()))
}

async fn seed_workspace(pool: &PgPool, label: &str) -> WorkspaceFixture {
    let (user_id, scope_id) = insert_user(pool, label).await;
    let owner = insert_credential(
        pool,
        user_id,
        scope_id,
        &format!("{label} owner"),
        &["message.read", "message.write", "status"],
    )
    .await;
    let agent_writer = insert_credential(
        pool,
        user_id,
        scope_id,
        &format!("{label} agent writer"),
        &["message.read", "message.write"],
    )
    .await;
    let agent_reader = insert_credential(
        pool,
        user_id,
        scope_id,
        &format!("{label} agent reader"),
        &["message.read"],
    )
    .await;
    let unbound_writer = insert_credential(
        pool,
        user_id,
        scope_id,
        &format!("{label} unbound writer"),
        &["message.read", "message.write", "admin"],
    )
    .await;

    insert_agent(pool, user_id, owner.id, "owner", "owner").await;
    insert_agent(pool, user_id, owner.id, "agent-a", "resident").await;
    bind_credential(pool, user_id, owner.id, "owner", owner.id).await;
    bind_credential(pool, user_id, agent_writer.id, "agent-a", owner.id).await;
    bind_credential(pool, user_id, agent_reader.id, "agent-a", owner.id).await;

    WorkspaceFixture {
        owner,
        agent_writer,
        agent_reader,
        unbound_writer,
    }
}

async fn seed_other_owner(pool: &PgPool) -> CredentialFixture {
    let (user_id, scope_id) = insert_user(pool, "other-workspace").await;
    let owner = insert_credential(
        pool,
        user_id,
        scope_id,
        "other workspace owner",
        &["message.read", "message.write"],
    )
    .await;
    insert_agent(pool, user_id, owner.id, "owner", "owner").await;
    bind_credential(pool, user_id, owner.id, "owner", owner.id).await;
    owner
}

async fn request_bytes(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    bytes: Option<Vec<u8>>,
) -> HttpResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = if let Some(bytes) = bytes {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(bytes)
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).expect("build endpoint request"))
        .await
        .expect("serve endpoint request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect endpoint response")
        .to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    HttpResponse { status, body }
}

async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> HttpResponse {
    request_bytes(
        app,
        method,
        uri,
        Some(token),
        Some(serde_json::to_vec(&body).expect("serialize endpoint request")),
    )
    .await
}

async fn request_web(
    app: &Router,
    method: Method,
    uri: &str,
    session_token: &str,
    csrf: &str,
    production_cookies: bool,
    body: Option<Value>,
) -> HttpResponse {
    let cookie_prefix = if production_cookies { "__Host-" } else { "" };
    let mut builder = Request::builder().method(method.clone()).uri(uri).header(
        header::COOKIE,
        format!(
            "{cookie_prefix}straylight_session={session_token}; {cookie_prefix}straylight_csrf={csrf}"
        ),
    );
    let body = if let Some(body) = body {
        builder = builder
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-csrf-token", csrf);
        Body::from(serde_json::to_vec(&body).expect("serialize Web endpoint request"))
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).expect("build Web endpoint request"))
        .await
        .expect("serve Web endpoint request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect Web endpoint response")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    HttpResponse { status, body }
}

fn assert_status(response: &HttpResponse, expected: StatusCode) {
    assert_eq!(response.status, expected, "unexpected endpoint status");
}

fn data(response: &HttpResponse) -> &Value {
    response
        .body
        .get("data")
        .expect("successful endpoint response has data")
}

fn assert_error(response: &HttpResponse, expected_status: StatusCode, expected_code: &str) {
    assert_status(response, expected_status);
    assert_eq!(
        response.body.pointer("/error/code").and_then(Value::as_str),
        Some(expected_code),
        "unexpected endpoint error code"
    );
}

fn client_key(suffix: char) -> String {
    format!("{CLIENT_KEY_PREFIX}{suffix}")
}

fn send_body(client_key: String, body_md: impl Into<String>) -> Value {
    json!({
        "client_key": client_key,
        "kind": "text",
        "body_md": body_md.into()
    })
}

#[tokio::test]
async fn messaging_registry_binds_active_credentials_and_rejects_disabled_credentials() {
    let Some((pool, mut state)) = connect_test_state().await else {
        return;
    };

    let (user_id, scope_id) = insert_user(&pool, "credential-binding").await;
    let owner = insert_credential(
        &pool,
        user_id,
        scope_id,
        "credential-binding owner",
        &["admin", "message.read", "message.write", "read"],
    )
    .await;
    let active = insert_credential(
        &pool,
        user_id,
        scope_id,
        "credential-binding active",
        &["message.read", "message.write"],
    )
    .await;
    let disabled = insert_credential(
        &pool,
        user_id,
        scope_id,
        "credential-binding disabled",
        &["message.read", "message.write"],
    )
    .await;
    insert_agent(&pool, user_id, owner.id, "owner", "owner").await;
    insert_agent(
        &pool,
        user_id,
        owner.id,
        "credential-binding-target",
        "resident",
    )
    .await;
    bind_credential(&pool, user_id, owner.id, "owner", owner.id).await;
    sqlx::query(
        "UPDATE straylight.api_credentials SET disabled_at=clock_timestamp() WHERE user_id=$1 AND id=$2",
    )
    .bind(user_id)
    .bind(disabled.id)
    .execute(&pool)
    .await
    .expect("disable messaging endpoint credential");

    let (session_token, csrf) = insert_web_session(&pool, user_id, owner.id).await;
    let production_cookies = state.config.deployment_environment == "production";
    state.config.messaging_enabled = true;
    let app = router(state);
    let path = "/v1/workspace/messaging/agents/credential-binding-target/credential";

    let active_response = request_web(
        &app,
        Method::PUT,
        path,
        &session_token,
        &csrf,
        production_cookies,
        Some(json!({"credential_id": active.id})),
    )
    .await;
    assert_eq!(
        active_response.status,
        StatusCode::OK,
        "active credential binding failed: {}",
        active_response.body
    );
    assert_eq!(
        data(&active_response).get("bound").and_then(Value::as_bool),
        Some(true)
    );
    let active_binding = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM straylight.messaging_credential_bindings
          WHERE user_id=$1 AND credential_id=$2
            AND agent_id='credential-binding-target'
        )
        "#,
    )
    .bind(user_id)
    .bind(active.id)
    .fetch_one(&pool)
    .await
    .expect("read active credential binding");
    assert!(active_binding, "the active credential must be bound");

    let listed = request_web(
        &app,
        Method::GET,
        "/v1/workspace/messaging/agents",
        &session_token,
        &csrf,
        production_cookies,
        None,
    )
    .await;
    assert_eq!(
        listed.status,
        StatusCode::OK,
        "cookie-authenticated agent list failed: {}",
        listed.body
    );
    let agents = data(&listed)
        .get("agents")
        .and_then(Value::as_array)
        .expect("agent list is an array");
    let target_agents = agents
        .iter()
        .filter(|agent| {
            agent.get("agent_id").and_then(Value::as_str) == Some("credential-binding-target")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        target_agents.len(),
        1,
        "bound agent must appear exactly once"
    );
    assert_eq!(
        target_agents[0].get("credential_names"),
        Some(&json!(["credential-binding active"])),
        "the active credential label must be revealed exactly once"
    );

    let disabled_response = request_web(
        &app,
        Method::PUT,
        path,
        &session_token,
        &csrf,
        production_cookies,
        Some(json!({"credential_id": disabled.id})),
    )
    .await;
    assert_error(
        &disabled_response,
        StatusCode::NOT_FOUND,
        "messaging_not_found",
    );
    let disabled_binding = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM straylight.messaging_credential_bindings WHERE user_id=$1 AND credential_id=$2)",
    )
    .bind(user_id)
    .bind(disabled.id)
    .fetch_one(&pool)
    .await
    .expect("check disabled credential binding");
    assert!(
        !disabled_binding,
        "a disabled credential must never be bound"
    );
}

#[tokio::test]
async fn messaging_routes_enforce_the_flag_identity_idempotency_sync_and_authority_contract() {
    let Some((pool, mut state)) = connect_test_state().await else {
        return;
    };

    let gate_off = router(state.clone());
    let gated_conversation_id = Uuid::now_v7();
    let gate_off_routes = vec![
        (
            "sync",
            Method::GET,
            format!("{MESSAGING_ROOT}/sync?cursor=0&wait=0"),
            None,
        ),
        (
            "list agents",
            Method::GET,
            format!("{MESSAGING_ROOT}/agents"),
            None,
        ),
        (
            "create conversation",
            Method::POST,
            format!("{MESSAGING_ROOT}/conversations"),
            Some(b"{}".to_vec()),
        ),
        (
            "send message",
            Method::POST,
            format!("{MESSAGING_ROOT}/conversations/{gated_conversation_id}/messages"),
            Some(b"{}".to_vec()),
        ),
        (
            "mark read",
            Method::POST,
            format!("{MESSAGING_ROOT}/conversations/{gated_conversation_id}/read"),
            Some(b"{}".to_vec()),
        ),
        (
            "resume conversation",
            Method::POST,
            format!("{MESSAGING_ROOT}/conversations/{gated_conversation_id}/resume"),
            Some(b"{}".to_vec()),
        ),
        (
            "close conversation",
            Method::POST,
            format!("{MESSAGING_ROOT}/conversations/{gated_conversation_id}/close"),
            Some(b"{}".to_vec()),
        ),
        (
            "create agent",
            Method::POST,
            format!("{MESSAGING_ROOT}/agents"),
            Some(b"{}".to_vec()),
        ),
        (
            "update agent",
            Method::PATCH,
            format!("{MESSAGING_ROOT}/agents/agent-a"),
            Some(b"{}".to_vec()),
        ),
        (
            "bind agent credential",
            Method::PUT,
            format!("{MESSAGING_ROOT}/agents/agent-a/credential"),
            Some(b"{}".to_vec()),
        ),
    ];
    for (route_name, method, uri, bytes) in gate_off_routes {
        let response = request_bytes(&gate_off, method.clone(), &uri, None, bytes).await;
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "messaging gate leaked the {route_name} route ({method} {uri})"
        );
    }

    let fixture = seed_workspace(&pool, "primary-workspace").await;
    let other_owner = seed_other_owner(&pool).await;
    let status_off = request_bytes(
        &gate_off,
        Method::GET,
        "/v1/status",
        Some(&fixture.owner.token),
        None,
    )
    .await;
    assert_status(&status_off, StatusCode::OK);
    assert_eq!(
        status_off.body.pointer("/feature_flags/messaging_enabled"),
        Some(&Value::Bool(false)),
        "service status must expose the disabled messaging runtime flag"
    );

    state.config.messaging_enabled = true;
    let app = router(state);
    let status_on = request_bytes(
        &app,
        Method::GET,
        "/v1/status",
        Some(&fixture.owner.token),
        None,
    )
    .await;
    assert_status(&status_on, StatusCode::OK);
    assert_eq!(
        status_on.body.pointer("/feature_flags/messaging_enabled"),
        Some(&Value::Bool(true)),
        "service status must expose the enabled messaging runtime flag"
    );

    let listed_agents = request_bytes(
        &app,
        Method::GET,
        &format!("{MESSAGING_ROOT}/agents"),
        Some(&fixture.owner.token),
        None,
    )
    .await;
    assert_status(&listed_agents, StatusCode::OK);
    assert!(
        data(&listed_agents)
            .get("agents")
            .and_then(Value::as_array)
            .is_some_and(|agents| agents.len() == 2),
        "gate-on agents route reaches its handler and lists the seeded principals"
    );

    let create_request = json!({
        "participants": ["agent-a"],
        "subject": "Endpoint contract"
    });
    let created = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations"),
        &fixture.owner.token,
        create_request.clone(),
    )
    .await;
    assert_status(&created, StatusCode::OK);
    assert_eq!(
        created.body.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        data(&created).get("duplicate").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        data(&created)
            .pointer("/conversation/conversation_kind")
            .and_then(Value::as_str),
        Some("direct")
    );
    let conversation_id = data(&created)
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .expect("create returns a conversation UUID");

    let duplicate_create = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations"),
        &fixture.owner.token,
        create_request,
    )
    .await;
    assert_status(&duplicate_create, StatusCode::OK);
    assert_eq!(
        duplicate_create.body.get("status").and_then(Value::as_str),
        Some("committed")
    );
    let second_subject_id = data(&duplicate_create)
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .expect("second subject conversation returns a UUID");
    assert_ne!(
        second_subject_id, conversation_id,
        "subject conversations are distinct threads even for the same principals"
    );
    assert_eq!(
        data(&duplicate_create)
            .get("duplicate")
            .and_then(Value::as_bool),
        Some(false)
    );

    let default_request = json!({"participants": ["agent-a"]});
    let default_created = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations"),
        &fixture.owner.token,
        default_request.clone(),
    )
    .await;
    assert_status(&default_created, StatusCode::OK);
    let default_id = data(&default_created)
        .get("conversation_id")
        .and_then(Value::as_str)
        .expect("default direct returns an id")
        .to_owned();
    let default_replay = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations"),
        &fixture.owner.token,
        default_request,
    )
    .await;
    assert_status(&default_replay, StatusCode::OK);
    assert_eq!(
        default_replay.body.get("status").and_then(Value::as_str),
        Some("no_op")
    );
    assert_eq!(
        data(&default_replay)
            .get("conversation_id")
            .and_then(Value::as_str),
        Some(default_id.as_str())
    );

    let messages_path = format!("{MESSAGING_ROOT}/conversations/{conversation_id}/messages");
    let first_body = send_body(client_key('V'), "first agent message");
    let first = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        first_body.clone(),
    )
    .await;
    assert_status(&first, StatusCode::OK);
    assert_eq!(data(&first).get("seq").and_then(Value::as_i64), Some(1));
    assert_eq!(
        data(&first)
            .pointer("/message/from_agent_id")
            .and_then(Value::as_str),
        Some("agent-a")
    );
    assert_eq!(
        data(&first).get("duplicate").and_then(Value::as_bool),
        Some(false)
    );
    let first_message_id = data(&first)
        .pointer("/message/message_id")
        .and_then(Value::as_str)
        .expect("send returns a message id")
        .to_owned();

    let replay = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        first_body,
    )
    .await;
    assert_status(&replay, StatusCode::OK);
    assert_eq!(
        data(&replay).get("duplicate").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data(&replay)
            .pointer("/message/message_id")
            .and_then(Value::as_str),
        Some(first_message_id.as_str())
    );

    let conflict = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        send_body(client_key('V'), "changed retry body"),
    )
    .await;
    assert_error(&conflict, StatusCode::CONFLICT, "idempotency_conflict");

    let claimed_sender = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        json!({
            "client_key": client_key('W'),
            "kind": "text",
            "body_md": "claimed sender must be rejected",
            "from": "owner"
        }),
    )
    .await;
    assert_status(&claimed_sender, StatusCode::UNPROCESSABLE_ENTITY);

    let malformed_json = request_bytes(
        &app,
        Method::POST,
        &messages_path,
        Some(&fixture.agent_writer.token),
        Some(b"{]".to_vec()),
    )
    .await;
    assert_status(&malformed_json, StatusCode::BAD_REQUEST);

    let invalid_key = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        send_body("not-a-client-key".to_owned(), "invalid key"),
    )
    .await;
    assert_error(&invalid_key, StatusCode::BAD_REQUEST, "invalid_request");

    let system_kind = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        json!({
            "client_key": client_key('X'),
            "kind": "system",
            "body_md": "clients cannot mint system messages"
        }),
    )
    .await;
    assert_error(&system_kind, StatusCode::BAD_REQUEST, "invalid_request");

    let boundary = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        send_body(client_key('Y'), "x".repeat(16 * 1024)),
    )
    .await;
    assert_status(&boundary, StatusCode::OK);
    assert_eq!(data(&boundary).get("seq").and_then(Value::as_i64), Some(2));
    assert_eq!(
        data(&boundary)
            .pointer("/message/body_md")
            .and_then(Value::as_str)
            .map(str::len),
        Some(16 * 1024)
    );

    let owner_question = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        json!({
            "client_key": client_key('N'),
            "kind": "question",
            "body_md": "Owner attention is required",
            "expects_reply": true
        }),
    )
    .await;
    assert_status(&owner_question, StatusCode::OK);

    let owner_sync = request_bytes(
        &app,
        Method::GET,
        &format!(
            "{MESSAGING_ROOT}/sync?conversation_id={conversation_id}&after_seq=0&cursor=0&wait=0"
        ),
        Some(&fixture.owner.token),
        None,
    )
    .await;
    assert_status(&owner_sync, StatusCode::OK);
    assert_eq!(
        data(&owner_sync)
            .pointer("/conversations/0/needs_human")
            .and_then(Value::as_bool),
        Some(true),
        "a resident question in an owner-participant thread needs owner attention"
    );
    assert!(
        data(&owner_sync)
            .get("unread")
            .and_then(|unread| unread.get(conversation_id.to_string()))
            .and_then(Value::as_i64)
            .is_some_and(|count| count > 0),
        "sync exposes top-level per-conversation unread state"
    );
    let owner_read_after_sync = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT last_read_seq
        FROM straylight.messaging_participants
        WHERE conversation_id=$1 AND agent_id='owner'
        "#,
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("read owner delivery position");
    assert_eq!(
        owner_read_after_sync, 0,
        "owner sync never implicitly marks a conversation read"
    );

    let oversized = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        send_body(client_key('Z'), "é".repeat(8_193)),
    )
    .await;
    assert_error(&oversized, StatusCode::BAD_REQUEST, "invalid_request");

    let unbound = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.unbound_writer.token,
        send_body(client_key('T'), "unbound principal"),
    )
    .await;
    assert_error(
        &unbound,
        StatusCode::FORBIDDEN,
        "messaging_principal_unbound",
    );

    let read_only_send = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_reader.token,
        send_body(client_key('S'), "read-only send"),
    )
    .await;
    assert_error(&read_only_send, StatusCode::FORBIDDEN, "capability_denied");
    let read_only_mark = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/read"),
        &fixture.agent_reader.token,
        json!({"last_read_seq": 1}),
    )
    .await;
    assert_error(&read_only_mark, StatusCode::FORBIDDEN, "capability_denied");

    let initial_sync = request_bytes(
        &app,
        Method::GET,
        &format!("{MESSAGING_ROOT}/sync?cursor=0&wait=0"),
        Some(&fixture.agent_reader.token),
        None,
    )
    .await;
    assert_status(&initial_sync, StatusCode::OK);
    assert_eq!(
        data(&initial_sync)
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3),
        "invalid and denied sends must not create messages"
    );
    let initial_cursor = data(&initial_sync)
        .get("cursor")
        .and_then(Value::as_i64)
        .expect("sync returns a cursor");

    let owner_send = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.owner.token,
        send_body(client_key('R'), "owner delta"),
    )
    .await;
    assert_status(&owner_send, StatusCode::OK);
    assert_eq!(
        data(&owner_send)
            .pointer("/message/from_agent_id")
            .and_then(Value::as_str),
        Some("owner")
    );

    let immediate_wait = request_bytes(
        &app,
        Method::GET,
        &format!("{MESSAGING_ROOT}/sync?cursor={initial_cursor}&wait=1"),
        Some(&fixture.agent_writer.token),
        None,
    )
    .await;
    assert_status(&immediate_wait, StatusCode::OK);
    assert_eq!(
        data(&immediate_wait).get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        data(&immediate_wait)
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    let wait_cursor = data(&immediate_wait)
        .get("cursor")
        .and_then(Value::as_i64)
        .expect("wait returns a cursor");
    assert!(wait_cursor > initial_cursor);
    assert!(
        data(&immediate_wait)
            .get("presence")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|agent| {
                agent.get("agent_id").and_then(Value::as_str) == Some("agent-a")
                    && agent.get("online").and_then(Value::as_bool) == Some(true)
            }),
        "a successful wait start renews the bound principal lease"
    );

    let unread_owner_send = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.owner.token,
        send_body(client_key('Q'), "read-position delta"),
    )
    .await;
    assert_status(&unread_owner_send, StatusCode::OK);
    let unread_seq = data(&unread_owner_send)
        .get("seq")
        .and_then(Value::as_i64)
        .expect("send returns sequence");

    let marked = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/read"),
        &fixture.agent_writer.token,
        json!({"last_read_seq": unread_seq}),
    )
    .await;
    assert_status(&marked, StatusCode::OK);
    assert_eq!(
        marked.body.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        data(&marked).get("duplicate").and_then(Value::as_bool),
        Some(false)
    );
    let read_cursor = data(&marked)
        .get("cursor")
        .and_then(Value::as_i64)
        .expect("read mutation returns a cursor");

    let lower_read = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/read"),
        &fixture.agent_writer.token,
        json!({"last_read_seq": unread_seq - 1}),
    )
    .await;
    assert_status(&lower_read, StatusCode::OK);
    assert_eq!(
        lower_read.body.get("status").and_then(Value::as_str),
        Some("no_op")
    );
    assert_eq!(
        data(&lower_read)
            .get("last_read_seq")
            .and_then(Value::as_i64),
        Some(unread_seq)
    );

    let timeout = request_bytes(
        &app,
        Method::GET,
        &format!("{MESSAGING_ROOT}/sync?cursor={read_cursor}&wait=1"),
        Some(&fixture.agent_writer.token),
        None,
    )
    .await;
    assert_status(&timeout, StatusCode::OK);
    assert_eq!(
        data(&timeout).get("status").and_then(Value::as_str),
        Some("timeout")
    );
    assert_eq!(
        data(&timeout).get("resume_cursor").and_then(Value::as_i64),
        Some(read_cursor)
    );
    assert_eq!(
        data(&timeout)
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let agent_resume = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/resume"),
        &fixture.agent_writer.token,
        json!({}),
    )
    .await;
    assert_error(&agent_resume, StatusCode::FORBIDDEN, "owner_required");
    let agent_close = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/close"),
        &fixture.agent_writer.token,
        json!({}),
    )
    .await;
    assert_error(&agent_close, StatusCode::FORBIDDEN, "owner_required");

    let cross_workspace_read = request_bytes(
        &app,
        Method::GET,
        &format!("{MESSAGING_ROOT}/sync?conversation_id={conversation_id}&after_seq=0&wait=0"),
        Some(&other_owner.token),
        None,
    )
    .await;
    assert_error(
        &cross_workspace_read,
        StatusCode::NOT_FOUND,
        "messaging_not_found",
    );
    let cross_workspace_close = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/close"),
        &other_owner.token,
        json!({}),
    )
    .await;
    assert_error(
        &cross_workspace_close,
        StatusCode::NOT_FOUND,
        "messaging_not_found",
    );

    let owner_resume = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/resume"),
        &fixture.owner.token,
        json!({}),
    )
    .await;
    assert_status(&owner_resume, StatusCode::OK);
    assert_eq!(
        owner_resume.body.get("status").and_then(Value::as_str),
        Some("no_op")
    );

    let owner_close = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/close"),
        &fixture.owner.token,
        json!({}),
    )
    .await;
    assert_status(&owner_close, StatusCode::OK);
    assert_eq!(
        owner_close.body.get("status").and_then(Value::as_str),
        Some("committed")
    );

    let duplicate_close = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_id}/close"),
        &fixture.owner.token,
        json!({}),
    )
    .await;
    assert_status(&duplicate_close, StatusCode::OK);
    assert_eq!(
        duplicate_close.body.get("status").and_then(Value::as_str),
        Some("no_op")
    );

    let closed_send = request_json(
        &app,
        Method::POST,
        &messages_path,
        &fixture.agent_writer.token,
        send_body(client_key('P'), "closed conversation"),
    )
    .await;
    assert_error(&closed_send, StatusCode::CONFLICT, "conversation_closed");
}

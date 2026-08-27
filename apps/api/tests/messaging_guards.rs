use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

use straylight::{AppState, Config, auth::hash_token, messaging_service, router};

const MESSAGING_ROOT: &str = "/v1/workspace/messaging";

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
    user_id: Uuid,
    owner: CredentialFixture,
    agent: CredentialFixture,
    agent_b: CredentialFixture,
}

async fn connect_test_state() -> Option<(PgPool, AppState)> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping messaging guard contract");
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

    let mut config = Config::from_env().expect("load disposable API configuration");
    config.database_url_rw = database_url.clone();
    config.database_url_ro = database_url.clone();
    config.database_url_admin = Some(database_url);
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = true;
    let state = AppState::connect(config)
        .await
        .expect("connect disposable API state");
    Some((seed_pool, state))
}

async fn insert_credential(
    pool: &PgPool,
    user_id: Uuid,
    scope_id: Uuid,
    label: &str,
) -> CredentialFixture {
    let id = Uuid::now_v7();
    let token = format!("messaging-guard-test-{}", Uuid::now_v7());
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
    .bind(vec!["message.read", "message.write"])
    .execute(pool)
    .await
    .expect("insert narrow messaging credential");
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
    .expect("grant messaging test scope");
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
    .bind(format!("Guard {agent_id}"))
    .bind(principal_kind)
    .bind(creator_id)
    .execute(pool)
    .await
    .expect("insert messaging guard principal");
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
    .expect("bind messaging guard credential");
}

async fn seed_workspace(pool: &PgPool, label: &str) -> WorkspaceFixture {
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    sqlx::query("INSERT INTO straylight.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("messaging-guard-test:{label}:{user_id}"))
        .bind(format!("Messaging guard {label}"))
        .execute(pool)
        .await
        .expect("insert messaging guard user");
    sqlx::query("INSERT INTO straylight.scopes (id,user_id,scope_ref,name) VALUES ($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(format!("scope:messaging-guard-{scope_id}"))
        .bind(format!("Messaging guard {label}"))
        .execute(pool)
        .await
        .expect("insert messaging guard scope");

    let owner = insert_credential(pool, user_id, scope_id, &format!("{label} owner")).await;
    let agent = insert_credential(pool, user_id, scope_id, &format!("{label} agent-a")).await;
    let agent_b = insert_credential(pool, user_id, scope_id, &format!("{label} agent-b")).await;
    insert_agent(pool, user_id, owner.id, "owner", "owner").await;
    for agent_id in ["agent-a", "agent-b", "agent-c", "agent-d"] {
        insert_agent(pool, user_id, owner.id, agent_id, "resident").await;
    }
    bind_credential(pool, user_id, owner.id, "owner", owner.id).await;
    bind_credential(pool, user_id, agent.id, "agent-a", owner.id).await;
    bind_credential(pool, user_id, agent_b.id, "agent-b", owner.id).await;
    WorkspaceFixture {
        user_id,
        owner,
        agent,
        agent_b,
    }
}

async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> HttpResponse {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&body).expect("serialize guard request"),
        ))
        .expect("build guard request");
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("serve messaging guard request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect messaging guard response")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    HttpResponse { status, body }
}

fn data(response: &HttpResponse) -> &Value {
    response
        .body
        .get("data")
        .expect("successful messaging response has data")
}

fn assert_error(response: &HttpResponse, status: StatusCode, code: &str) {
    assert_eq!(response.status, status, "unexpected guard response status");
    assert_eq!(
        response.body.pointer("/error/code").and_then(Value::as_str),
        Some(code),
        "unexpected guard error code"
    );
}

fn client_key(number: i64) -> String {
    format!("{number:026}")
}

fn text_send(number: i64, body_md: &str) -> Value {
    json!({
        "client_key": client_key(number),
        "kind": "text",
        "body_md": body_md
    })
}

async fn create_conversation(
    app: &Router,
    token: &str,
    participants: &[&str],
    subject: &str,
) -> Uuid {
    let response = request_json(
        app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations"),
        token,
        json!({"participants": participants, "subject": subject}),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "create guard fixture");
    data(&response)
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .expect("conversation response contains a UUID")
}

#[allow(clippy::too_many_arguments)]
async fn seed_messages(
    pool: &PgPool,
    user_id: Uuid,
    conversation_id: Uuid,
    start_seq: i64,
    count: i64,
    senders: &[&str],
    created_at: DateTime<Utc>,
    agent_streak: i32,
) {
    assert!(count > 0);
    assert!(!senders.is_empty());
    let mut tx = pool.begin().await.expect("begin message seed");
    let base_cursor = sqlx::query_scalar::<_, i64>(
        "SELECT current_cursor FROM straylight.messaging_sync_state WHERE user_id=$1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .expect("lock messaging cursor");
    let senders = senders
        .iter()
        .map(|sender| (*sender).to_owned())
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_message_index (
          user_id,conversation_id,seq,message_id,from_agent_id,client_key,
          system_key,request_hash,kind,body_md,refs,in_reply_to,
          correlation_id,expects_reply,reply_by,reply_by_handled_at,
          sync_cursor,created_at
        )
        SELECT
          $1,$2,$3 + seed.offset,gen_random_uuid(),
          $5[((seed.offset % cardinality($5)) + 1)::integer],
          lpad(($3 + seed.offset)::text,26,'0'),
          NULL,repeat('a',64),'text','seed message','[]'::jsonb,
          NULL,NULL,false,NULL,NULL,$6 + seed.offset + 1,$7
        FROM generate_series(0::bigint,$4::bigint - 1) AS seed(offset)
        "#,
    )
    .bind(user_id)
    .bind(conversation_id)
    .bind(start_seq)
    .bind(count)
    .bind(senders)
    .bind(base_cursor)
    .bind(created_at)
    .execute(&mut *tx)
    .await
    .expect("seed indexed conversation messages");
    let final_seq = start_seq + count - 1;
    let final_cursor = base_cursor + count;
    sqlx::query(
        r#"
        UPDATE straylight.messaging_sync_state
        SET current_cursor=$2,updated_at=clock_timestamp()
        WHERE user_id=$1
        "#,
    )
    .bind(user_id)
    .bind(final_cursor)
    .execute(&mut *tx)
    .await
    .expect("advance seeded messaging cursor");
    sqlx::query(
        r#"
        UPDATE straylight.messaging_conversations
        SET last_seq=$3,last_message_at=$4,agent_streak=$5,
            latest_sync_cursor=$6,updated_at=clock_timestamp()
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(user_id)
    .bind(conversation_id)
    .bind(final_seq)
    .bind(created_at)
    .bind(agent_streak)
    .bind(final_cursor)
    .execute(&mut *tx)
    .await
    .expect("advance seeded conversation projection");
    tx.commit().await.expect("commit message seed");
}

fn assert_typed_rate(response: &HttpResponse, code: &str, maximum_retry: i64) {
    assert_error(response, StatusCode::TOO_MANY_REQUESTS, code);
    let retry_after = response
        .body
        .pointer("/error/details/retry_after_seconds")
        .and_then(Value::as_i64)
        .expect("rate error contains retry_after_seconds");
    assert!(
        (1..=maximum_retry).contains(&retry_after),
        "retry metadata must be positive and bounded"
    );
}

#[tokio::test]
async fn messaging_guards_preserve_replay_budgets_rollover_and_reply_deadlines() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let app = router(state.clone());

    let sender_rate = seed_workspace(&pool, "sender-rate").await;
    let sender_rate_conversation = create_conversation(
        &app,
        &sender_rate.agent.token,
        &["agent-b"],
        "Replay precedes sender rate",
    )
    .await;
    let sender_rate_path =
        format!("{MESSAGING_ROOT}/conversations/{sender_rate_conversation}/messages");
    let original_body = text_send(900, "original idempotent send");
    let original = request_json(
        &app,
        Method::POST,
        &sender_rate_path,
        &sender_rate.agent.token,
        original_body.clone(),
    )
    .await;
    assert_eq!(original.status, StatusCode::OK);
    let original_message_id = data(&original)
        .pointer("/message/message_id")
        .and_then(Value::as_str)
        .expect("original send has message id")
        .to_owned();
    seed_messages(
        &pool,
        sender_rate.user_id,
        sender_rate_conversation,
        2,
        59,
        &["agent-a"],
        Utc::now(),
        0,
    )
    .await;
    let replay = request_json(
        &app,
        Method::POST,
        &sender_rate_path,
        &sender_rate.agent.token,
        original_body,
    )
    .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(
        data(&replay).get("duplicate").and_then(Value::as_bool),
        Some(true),
        "an exact replay bypasses the saturated sender guard"
    );
    assert_eq!(
        data(&replay)
            .pointer("/message/message_id")
            .and_then(Value::as_str),
        Some(original_message_id.as_str())
    );
    let sender_limited = request_json(
        &app,
        Method::POST,
        &sender_rate_path,
        &sender_rate.agent.token,
        text_send(901, "new logical send must be limited"),
    )
    .await;
    assert_typed_rate(&sender_limited, "sender_rate_limited", 60);

    let conversation_rate = seed_workspace(&pool, "conversation-rate").await;
    let conversation_rate_id = create_conversation(
        &app,
        &conversation_rate.agent.token,
        &["agent-b", "agent-c", "agent-d"],
        "Conversation hourly rate",
    )
    .await;
    seed_messages(
        &pool,
        conversation_rate.user_id,
        conversation_rate_id,
        1,
        200,
        &["agent-a", "agent-b", "agent-c", "owner"],
        Utc::now(),
        0,
    )
    .await;
    let conversation_limited = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{conversation_rate_id}/messages"),
        &conversation_rate.agent.token,
        text_send(901, "new conversation send must be limited"),
    )
    .await;
    assert_typed_rate(&conversation_limited, "conversation_rate_limited", 3_600);

    let streak = seed_workspace(&pool, "agent-streak").await;
    let streak_conversation = create_conversation(
        &app,
        &streak.agent.token,
        &["agent-b"],
        "Exact twentieth message",
    )
    .await;
    seed_messages(
        &pool,
        streak.user_id,
        streak_conversation,
        1,
        19,
        &["agent-a"],
        Utc::now(),
        19,
    )
    .await;
    let twentieth = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{streak_conversation}/messages"),
        &streak.agent.token,
        text_send(900, "twentieth consecutive agent message"),
    )
    .await;
    assert_eq!(
        twentieth.status,
        StatusCode::OK,
        "the twentieth message commits before pausing"
    );
    assert_eq!(
        data(&twentieth).get("seq").and_then(Value::as_i64),
        Some(20)
    );
    let streak_row = sqlx::query(
        r#"
        SELECT status,agent_streak,needs_human,last_seq
        FROM straylight.messaging_conversations
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .fetch_one(&pool)
    .await
    .expect("read exact streak state");
    assert_eq!(streak_row.get::<String, _>("status"), "paused_for_human");
    assert_eq!(streak_row.get::<i32, _>("agent_streak"), 20);
    assert!(streak_row.get::<bool, _>("needs_human"));
    assert_eq!(streak_row.get::<i64, _>("last_seq"), 21);
    let new_message_shape = sqlx::query(
        r#"
        SELECT count(*)::bigint AS total,
               count(*) FILTER (WHERE kind='system')::bigint AS systems
        FROM straylight.messaging_message_index
        WHERE user_id=$1 AND conversation_id=$2 AND seq>=20
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .fetch_one(&pool)
    .await
    .expect("read twentieth-message shape");
    assert_eq!(new_message_shape.get::<i64, _>("total"), 2);
    assert_eq!(new_message_shape.get::<i64, _>("systems"), 1);
    let needs_human_notifications = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)::bigint
        FROM straylight.notifications
        WHERE user_id=$1 AND event_key LIKE $2
        "#,
    )
    .bind(streak.user_id)
    .bind(format!("needs-human:{streak_conversation}:%"))
    .fetch_one(&pool)
    .await
    .expect("count needs-human notifications");
    let all_streak_notifications = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint FROM straylight.notifications WHERE user_id=$1",
    )
    .bind(streak.user_id)
    .fetch_one(&pool)
    .await
    .expect("count streak notifications");
    assert_eq!(needs_human_notifications, 1);
    assert_eq!(
        all_streak_notifications, 1,
        "observer-only owner gets the one needs-human event, not the ordinary message"
    );

    let paused_agent = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{streak_conversation}/messages"),
        &streak.agent.token,
        text_send(902, "must wait for an owner"),
    )
    .await;
    assert_error(&paused_agent, StatusCode::CONFLICT, "conversation_paused");

    let resumed = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{streak_conversation}/resume"),
        &streak.owner.token,
        json!({}),
    )
    .await;
    assert_eq!(resumed.status, StatusCode::OK);
    assert_eq!(
        data(&resumed).get("status").and_then(Value::as_str),
        Some("open")
    );
    let resumed_state = sqlx::query_as::<_, (String, i32, bool)>(
        r#"
        SELECT status,agent_streak,needs_human
        FROM straylight.messaging_conversations
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .fetch_one(&pool)
    .await
    .expect("read resumed state");
    assert_eq!(resumed_state, ("open".to_owned(), 0, false));

    sqlx::query(
        r#"
        UPDATE straylight.messaging_conversations
        SET status='paused_for_human',agent_streak=20,needs_human=true
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .execute(&pool)
    .await
    .expect("restore paused state for owner-post contract");
    let owner_role_before_post = sqlx::query_scalar::<_, String>(
        r#"
        SELECT role
        FROM straylight.messaging_participants
        WHERE user_id=$1 AND conversation_id=$2 AND agent_id='owner'
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .fetch_one(&pool)
    .await
    .expect("read owner observer role");
    assert_eq!(owner_role_before_post, "observer");
    let owner_post = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{streak_conversation}/messages"),
        &streak.owner.token,
        text_send(901, "owner clears the pause"),
    )
    .await;
    assert_eq!(owner_post.status, StatusCode::OK);
    let owner_cleared = sqlx::query_as::<_, (String, i32, bool)>(
        r#"
        SELECT status,agent_streak,needs_human
        FROM straylight.messaging_conversations
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .fetch_one(&pool)
    .await
    .expect("read owner-cleared state");
    assert_eq!(owner_cleared, ("open".to_owned(), 0, false));
    let owner_role_after_post = sqlx::query_scalar::<_, String>(
        r#"
        SELECT role
        FROM straylight.messaging_participants
        WHERE user_id=$1 AND conversation_id=$2 AND agent_id='owner'
        "#,
    )
    .bind(streak.user_id)
    .bind(streak_conversation)
    .fetch_one(&pool)
    .await
    .expect("read promoted owner role");
    assert_eq!(
        owner_role_after_post, "participant",
        "an owner post promotes an observer for subsequent delivery"
    );

    let rollover = seed_workspace(&pool, "rollover").await;
    let rollover_conversation = create_conversation(
        &app,
        &rollover.agent.token,
        &["agent-b"],
        "Five hundred message rollover",
    )
    .await;
    seed_messages(
        &pool,
        rollover.user_id,
        rollover_conversation,
        1,
        499,
        &["agent-a", "owner"],
        Utc::now() - ChronoDuration::hours(2),
        1,
    )
    .await;
    let rollover_path = format!("{MESSAGING_ROOT}/conversations/{rollover_conversation}/messages");
    let first_rollover = request_json(
        &app,
        Method::POST,
        &rollover_path,
        &rollover.agent.token,
        text_send(900, "one concurrent rollover send"),
    );
    let second_rollover = request_json(
        &app,
        Method::POST,
        &rollover_path,
        &rollover.agent_b.token,
        text_send(901, "the other concurrent rollover send"),
    );
    let (first_rollover, second_rollover) = tokio::join!(first_rollover, second_rollover);
    assert_eq!(first_rollover.status, StatusCode::OK);
    assert_eq!(second_rollover.status, StatusCode::OK);
    let responses = [&first_rollover, &second_rollover];
    let rollover_response = responses
        .iter()
        .find(|response| data(response).get("continuation_id").is_some())
        .expect("exactly one concurrent send rolls the source entry");
    assert_eq!(
        data(rollover_response).get("seq").and_then(Value::as_i64),
        Some(500)
    );
    let continuation_id = data(rollover_response)
        .get("continuation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .expect("five-hundredth send returns a continuation");
    let followed_response = responses
        .iter()
        .find(|response| data(response).get("continuation_id").is_none())
        .expect("the other send follows the continuation");
    assert_eq!(
        data(followed_response)
            .get("conversation_id")
            .and_then(Value::as_str),
        Some(continuation_id.to_string().as_str())
    );
    assert_eq!(
        data(followed_response).get("seq").and_then(Value::as_i64),
        Some(2),
        "the send waiting on a rolled source follows the locked continuation"
    );
    let old_rollover = sqlx::query_as::<_, (String, i64, Option<Uuid>)>(
        r#"
        SELECT status,last_seq,continues_from
        FROM straylight.messaging_conversations
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(rollover.user_id)
    .bind(rollover_conversation)
    .fetch_one(&pool)
    .await
    .expect("read closed rollover source");
    assert_eq!(old_rollover, ("closed".to_owned(), 500, None));
    let continuation = sqlx::query_as::<_, (String, i64, Option<Uuid>)>(
        r#"
        SELECT status,last_seq,continues_from
        FROM straylight.messaging_conversations
        WHERE user_id=$1 AND conversation_id=$2
        "#,
    )
    .bind(rollover.user_id)
    .bind(continuation_id)
    .fetch_one(&pool)
    .await
    .expect("read rollover continuation");
    assert_eq!(
        continuation,
        ("open".to_owned(), 2, Some(rollover_conversation))
    );
    let oversized_entries = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)::bigint
        FROM (
          SELECT conversation_id,count(*) AS message_count
          FROM straylight.messaging_message_index
          WHERE user_id=$1
            AND conversation_id IN ($2,$3)
          GROUP BY conversation_id
          HAVING count(*) > 500
        ) AS oversized
        "#,
    )
    .bind(rollover.user_id)
    .bind(rollover_conversation)
    .bind(continuation_id)
    .fetch_one(&pool)
    .await
    .expect("check rollover entry message caps");
    assert_eq!(oversized_entries, 0);
    let continuation_systems = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)::bigint
        FROM straylight.messaging_message_index
        WHERE user_id=$1 AND conversation_id=$2 AND seq=1 AND kind='system'
        "#,
    )
    .bind(rollover.user_id)
    .bind(continuation_id)
    .fetch_one(&pool)
    .await
    .expect("count continuation system record");
    assert_eq!(continuation_systems, 1);

    let deadline = seed_workspace(&pool, "reply-deadline").await;
    let deadline_conversation = create_conversation(
        &app,
        &deadline.agent.token,
        &["agent-b"],
        "Injected reply deadline",
    )
    .await;
    let reply_by = Utc::now() + ChronoDuration::minutes(1);
    let question = request_json(
        &app,
        Method::POST,
        &format!("{MESSAGING_ROOT}/conversations/{deadline_conversation}/messages"),
        &deadline.agent.token,
        json!({
            "client_key": client_key(900),
            "kind": "question",
            "body_md": "Reply before the injected deadline",
            "expects_reply": true,
            "reply_by": reply_by
        }),
    )
    .await;
    assert_eq!(question.status, StatusCode::OK);
    let question_seq = data(&question)
        .get("seq")
        .and_then(Value::as_i64)
        .expect("deadline question returns a sequence");
    let as_of = reply_by + ChronoDuration::seconds(1);
    assert!(
        messaging_service::process_due_reply_by(&state, as_of)
            .await
            .expect("process one due reply deadline")
    );
    assert!(
        !messaging_service::process_due_reply_by(&state, as_of)
            .await
            .expect("due reply deadline is idempotent")
    );
    let handled_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        r#"
        SELECT reply_by_handled_at
        FROM straylight.messaging_message_index
        WHERE user_id=$1 AND conversation_id=$2 AND seq=$3
        "#,
    )
    .bind(deadline.user_id)
    .bind(deadline_conversation)
    .bind(question_seq)
    .fetch_one(&pool)
    .await
    .expect("read handled reply deadline");
    assert!(handled_at.is_some());
    let deadline_key = format!("reply-by:{deadline_conversation}:{question_seq}");
    let deadline_systems = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)::bigint
        FROM straylight.messaging_message_index
        WHERE user_id=$1 AND conversation_id=$2 AND system_key=$3
        "#,
    )
    .bind(deadline.user_id)
    .bind(deadline_conversation)
    .bind(&deadline_key)
    .fetch_one(&pool)
    .await
    .expect("count deadline system messages");
    let deadline_notifications = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)::bigint
        FROM straylight.notifications
        WHERE user_id=$1 AND event_key=$2
        "#,
    )
    .bind(deadline.user_id)
    .bind(deadline_key)
    .fetch_one(&pool)
    .await
    .expect("count deadline notifications");
    assert_eq!(deadline_systems, 1);
    assert_eq!(deadline_notifications, 1);
}

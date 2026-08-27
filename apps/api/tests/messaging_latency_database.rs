use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use axum::{
    Extension, Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

use straylight::{
    AppState, Config,
    auth::{AuthContext, hash_token},
    messaging_protocol::{MessageKind, SendMessageInput, request_hash},
    messaging_service,
    models::{CredentialId, UserId},
};

const CONVERSATION_COUNT: usize = 50;
const MESSAGES_PER_CONVERSATION: usize = 200;
const SEEDED_MESSAGE_COUNT: usize = CONVERSATION_COUNT * MESSAGES_PER_CONVERSATION;
const WARMUP_SAMPLE_COUNT: usize = 5;
const TIMED_SAMPLE_COUNT: usize = 40;
const HANDLER_P95_LIMIT: Duration = Duration::from_millis(100);
const TYPICAL_DELTA_LIMIT_BYTES: usize = 8 * 1024;

const MESSAGE_CURSOR_QUERY: &str = r#"
SELECT message.conversation_id,message.seq,message.sync_cursor
FROM straylight.messaging_message_index AS message
JOIN straylight.messaging_participants AS participant
  ON participant.user_id=message.user_id
 AND participant.conversation_id=message.conversation_id
 AND participant.agent_id=$2
WHERE message.user_id=$1
  AND message.sync_cursor>$3
  AND message.sync_cursor<=$4
ORDER BY message.sync_cursor,message.conversation_id,message.seq
LIMIT $5
"#;

const CONVERSATION_CURSOR_QUERY: &str = r#"
SELECT conversation.conversation_id,conversation.latest_sync_cursor
FROM straylight.messaging_conversations AS conversation
JOIN straylight.messaging_participants AS participant
  ON participant.user_id=conversation.user_id
 AND participant.conversation_id=conversation.conversation_id
 AND participant.agent_id=$2
WHERE conversation.user_id=$1
  AND (
    (conversation.latest_sync_cursor>$3
      AND conversation.latest_sync_cursor<=$4)
    OR EXISTS (
      SELECT 1
      FROM straylight.messaging_message_index AS page_message
      WHERE page_message.user_id=conversation.user_id
        AND page_message.conversation_id=conversation.conversation_id
        AND page_message.sync_cursor>$3
        AND page_message.sync_cursor<=$4
    )
  )
ORDER BY conversation.last_message_at DESC NULLS LAST,
         conversation.created_at DESC,conversation.conversation_id
LIMIT $5
"#;

struct Fixture {
    user_id: Uuid,
    conversation_ids: Vec<Uuid>,
    auth: AuthContext,
}

struct HttpSample {
    elapsed: Duration,
    status: StatusCode,
    bytes: Vec<u8>,
    body: Value,
}

#[derive(Default, Debug)]
struct PlanInspection {
    sequential_relations: HashSet<String>,
    index_names: HashSet<String>,
}

async fn connect_test_state() -> Option<(PgPool, AppState)> {
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "STRAYLIGHT_TEST_DATABASE_URL is unset; skipping messaging latency database contract"
        );
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
    config.database_url_ro = database_url;
    config.database_url_admin = None;
    config.database_max_connections = 8;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = true;
    let state = AppState::connect(config)
        .await
        .expect("connect disposable gate-on API state");
    assert!(state.config.messaging_enabled, "messaging gate must be on");
    Some((seed_pool, state))
}

fn benchmark_client_key(value: usize) -> String {
    format!("{value:026X}")
}

fn text_input(conversation_id: Uuid, ordinal: usize, prefix: &str) -> (SendMessageInput, String) {
    let input = SendMessageInput {
        client_key: benchmark_client_key(ordinal),
        kind: MessageKind::Text,
        body_md: format!("{prefix} {ordinal}"),
        refs: Vec::new(),
        in_reply_to: None,
        correlation_id: None,
        expects_reply: false,
        reply_by: None,
    };
    let hash = request_hash(conversation_id, &input);
    (input, hash)
}

async fn seed_fixture(pool: &PgPool) -> Fixture {
    let mut tx = pool.begin().await.expect("begin messaging latency fixture");
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_ref = format!("scope:messaging-latency-{scope_id}");
    let capabilities = vec!["message.read".to_owned(), "message.write".to_owned()];
    let token = format!("messaging-latency-test-{credential_id}-secret");

    sqlx::query("INSERT INTO straylight.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("messaging-latency-test:{user_id}"))
        .bind("Messaging latency database contract")
        .execute(&mut *tx)
        .await
        .expect("insert messaging latency user");
    sqlx::query("INSERT INTO straylight.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind("Messaging latency database contract")
        .execute(&mut *tx)
        .await
        .expect("insert messaging latency scope");
    sqlx::query(
        r#"
        INSERT INTO straylight.api_credentials(
          id,user_id,label,token_hash,capabilities
        ) VALUES($1,$2,$3,$4,$5)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind("Messaging latency owner")
    .bind(hash_token(&token))
    .bind(&capabilities)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency credential");
    sqlx::query(
        r#"
        INSERT INTO straylight.credential_scope_grants(credential_id,user_id,scope_id)
        VALUES($1,$2,$3)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(&mut *tx)
    .await
    .expect("grant messaging latency scope");

    for (agent_id, display_name, principal_kind) in [
        ("agent-a", "Latency Agent", "resident"),
        ("owner", "Latency Owner", "owner"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO straylight.messaging_agents(
              user_id,agent_id,display_name,principal_kind,delivery_mode,
              created_by_credential_id
            ) VALUES($1,$2,$3,$4,'pull',$5)
            "#,
        )
        .bind(user_id)
        .bind(agent_id)
        .bind(display_name)
        .bind(principal_kind)
        .bind(credential_id)
        .execute(&mut *tx)
        .await
        .expect("insert messaging latency principal");
    }
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_credential_bindings(
          user_id,credential_id,agent_id,bound_by_credential_id
        ) VALUES($1,$2,'owner',$2)
        "#,
    )
    .bind(user_id)
    .bind(credential_id)
    .execute(&mut *tx)
    .await
    .expect("bind messaging latency owner credential");

    let conversation_ids = (0..CONVERSATION_COUNT)
        .map(|_| Uuid::now_v7())
        .collect::<Vec<_>>();
    let placeholder = "# Seeded messaging latency conversation\n";
    let placeholder_hash = hex::encode(Sha256::digest(placeholder.as_bytes()));
    let fixture_time = Utc::now() - ChronoDuration::hours(2);

    sqlx::query(
        r#"
        INSERT INTO straylight.entries(
          id,user_id,path,title,kind,media_type,current_version,created_at,updated_at
        )
        SELECT
          fixture.conversation_id,
          $1,
          '.straylight/conversations/' || fixture.conversation_id::text || '.md',
          format('Latency conversation %s',fixture.ordinal),
          'markdown',
          'text/markdown',
          1,
          $3,
          $3
        FROM unnest($2::uuid[]) WITH ORDINALITY
          AS fixture(conversation_id,ordinal)
        "#,
    )
    .bind(user_id)
    .bind(&conversation_ids)
    .bind(fixture_time)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency entries");
    sqlx::query(
        r#"
        INSERT INTO straylight.entry_versions(
          user_id,entry_id,version,content_sha256,content,size_bytes,metadata,
          created_by_credential_id,created_at
        )
        SELECT
          $1,
          fixture.conversation_id,
          1,
          $3,
          $4,
          octet_length($4),
          jsonb_build_object(
            'kind','conversation','schema','conversation.v1',
            'conversation_id',fixture.conversation_id
          ),
          $5,
          $6
        FROM unnest($2::uuid[]) AS fixture(conversation_id)
        "#,
    )
    .bind(user_id)
    .bind(&conversation_ids)
    .bind(placeholder_hash)
    .bind(placeholder)
    .bind(credential_id)
    .bind(fixture_time)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency entry versions");
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_conversations(
          user_id,conversation_id,entry_id,path,conversation_kind,direct_key,
          subject,status,created_by_agent_id,last_seq,last_message_at,
          agent_streak,needs_human,latest_sync_cursor,created_at,updated_at
        )
        SELECT
          $1,
          fixture.conversation_id,
          fixture.conversation_id,
          '.straylight/conversations/' || fixture.conversation_id::text || '.md',
          'direct',
          NULL,
          format('Latency conversation %s',fixture.ordinal),
          'open',
          'owner',
          $3,
          $4,
          0,
          false,
          fixture.ordinal * $3,
          $4 - interval '1 minute',
          $4
        FROM unnest($2::uuid[]) WITH ORDINALITY
          AS fixture(conversation_id,ordinal)
        "#,
    )
    .bind(user_id)
    .bind(&conversation_ids)
    .bind(MESSAGES_PER_CONVERSATION as i64)
    .bind(fixture_time)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency conversations");
    sqlx::query(
        r#"
        INSERT INTO straylight.messaging_participants(
          user_id,conversation_id,agent_id,role,last_read_seq,joined_at,updated_at
        )
        SELECT $1,fixture.conversation_id,member.agent_id,'participant',0,$3,$3
        FROM unnest($2::uuid[]) AS fixture(conversation_id)
        CROSS JOIN (VALUES ('agent-a'),('owner')) AS member(agent_id)
        "#,
    )
    .bind(user_id)
    .bind(&conversation_ids)
    .bind(fixture_time)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency participants");

    let mut message_ids = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    let mut message_conversation_ids = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    let mut sequences = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    let mut client_keys = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    let mut bodies = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    let mut request_hashes = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    let mut sync_cursors = Vec::with_capacity(SEEDED_MESSAGE_COUNT);
    for ordinal in 0..SEEDED_MESSAGE_COUNT {
        let conversation_id = conversation_ids[ordinal / MESSAGES_PER_CONVERSATION];
        let sequence = (ordinal % MESSAGES_PER_CONVERSATION) as i64 + 1;
        let (input, hash) = text_input(conversation_id, ordinal, "Seeded latency message");
        message_ids.push(Uuid::now_v7());
        message_conversation_ids.push(conversation_id);
        sequences.push(sequence);
        client_keys.push(input.client_key);
        bodies.push(input.body_md);
        request_hashes.push(hash);
        sync_cursors.push(ordinal as i64 + 1);
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO straylight.messaging_message_index(
          user_id,conversation_id,seq,message_id,from_agent_id,client_key,
          request_hash,kind,body_md,refs,expects_reply,sync_cursor,created_at
        )
        SELECT
          $1,
          fixture.conversation_id,
          fixture.seq,
          fixture.message_id,
          'owner',
          fixture.client_key,
          fixture.request_hash,
          'text',
          fixture.body_md,
          '[]'::jsonb,
          false,
          fixture.sync_cursor,
          $9
        FROM unnest(
          $2::uuid[],$3::uuid[],$4::bigint[],$5::text[],$6::text[],
          $7::text[],$8::bigint[]
        ) AS fixture(
          conversation_id,message_id,seq,client_key,body_md,request_hash,sync_cursor
        )
        "#,
    )
    .bind(user_id)
    .bind(&message_conversation_ids)
    .bind(&message_ids)
    .bind(&sequences)
    .bind(&client_keys)
    .bind(&bodies)
    .bind(&request_hashes)
    .bind(&sync_cursors)
    .bind(fixture_time)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency messages");
    assert_eq!(inserted.rows_affected(), SEEDED_MESSAGE_COUNT as u64);
    sqlx::query(
        "INSERT INTO straylight.messaging_sync_state(user_id,current_cursor) VALUES($1,$2)",
    )
    .bind(user_id)
    .bind(SEEDED_MESSAGE_COUNT as i64)
    .execute(&mut *tx)
    .await
    .expect("insert messaging latency cursor");
    tx.commit().await.expect("commit messaging latency fixture");

    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.messaging_message_index WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("count seeded messaging rows");
    assert_eq!(stored, SEEDED_MESSAGE_COUNT as i64);

    Fixture {
        user_id,
        conversation_ids,
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect(),
            scope_refs: vec![scope_ref],
            read_only: false,
        },
    }
}

async fn request_json(app: &Router, method: Method, uri: &str, body: Option<Value>) -> HttpSample {
    let mut builder = Request::builder().method(method).uri(uri);
    let request_body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("serialize latency request"))
    } else {
        Body::empty()
    };
    let request = builder.body(request_body).expect("build latency request");
    let started = Instant::now();
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("serve messaging latency request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect messaging latency response")
        .to_bytes()
        .to_vec();
    let elapsed = started.elapsed();
    let body = serde_json::from_slice(&bytes).expect("decode messaging latency response");
    HttpSample {
        elapsed,
        status,
        bytes,
        body,
    }
}

fn percentile95<T: Copy + Ord>(samples: &mut [T]) -> T {
    assert!(!samples.is_empty());
    samples.sort_unstable();
    let rank = (samples.len() * 95).div_ceil(100);
    samples[rank - 1]
}

fn inspect_plan(value: &Value, inspection: &mut PlanInspection) {
    match value {
        Value::Array(values) => {
            for value in values {
                inspect_plan(value, inspection);
            }
        }
        Value::Object(object) => {
            if object
                .get("Node Type")
                .and_then(Value::as_str)
                .is_some_and(|node| node.contains("Seq Scan"))
                && let Some(relation) = object.get("Relation Name").and_then(Value::as_str)
            {
                inspection.sequential_relations.insert(relation.to_owned());
            }
            if let Some(index_name) = object.get("Index Name").and_then(Value::as_str) {
                inspection.index_names.insert(index_name.to_owned());
            }
            for value in object.values() {
                inspect_plan(value, inspection);
            }
        }
        _ => {}
    }
}

async fn explain_cursor_plan(
    state: &AppState,
    auth: &AuthContext,
    sql: &str,
    user_id: Uuid,
    after_cursor: i64,
    through_cursor: i64,
) -> PlanInspection {
    let mut tx = state
        .begin_write(auth)
        .await
        .expect("begin messaging cursor plan transaction");
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}");
    let value = sqlx::query_scalar::<_, Value>(AssertSqlSafe(explain.as_str()))
        .bind(user_id)
        .bind("owner")
        .bind(after_cursor)
        .bind(through_cursor)
        .bind(200_i64)
        .fetch_one(&mut *tx)
        .await
        .expect("explain messaging cursor query");
    tx.rollback()
        .await
        .expect("roll back messaging cursor plan transaction");
    let mut inspection = PlanInspection::default();
    inspect_plan(&value, &mut inspection);
    inspection
}

#[tokio::test]
async fn messaging_handlers_meet_latency_payload_and_cursor_plan_gates_at_target_scale() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    sqlx::query("ANALYZE straylight.messaging_message_index")
        .execute(&pool)
        .await
        .expect("analyze messaging message projection");
    sqlx::query("ANALYZE straylight.messaging_conversations")
        .execute(&pool)
        .await
        .expect("analyze messaging conversations");
    sqlx::query("ANALYZE straylight.messaging_participants")
        .execute(&pool)
        .await
        .expect("analyze messaging participants");

    // The production API route is deliberately wired in a shared-file milestone.
    // This contract still exercises the exact send/sync Axum handlers and gate-on
    // AppState by mounting the messaging-owned router with a real bound principal.
    let app = messaging_service::router()
        .layer(Extension(fixture.auth.clone()))
        .with_state(state.clone());
    let conversation_id = fixture.conversation_ids[0];
    let mut cursor = SEEDED_MESSAGE_COUNT as i64;
    let mut send_samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    let mut sync_samples = Vec::with_capacity(TIMED_SAMPLE_COUNT);
    let mut delta_sizes = Vec::with_capacity(TIMED_SAMPLE_COUNT);

    for sample in 0..(WARMUP_SAMPLE_COUNT + TIMED_SAMPLE_COUNT) {
        let ordinal = SEEDED_MESSAGE_COUNT + sample;
        let send = request_json(
            &app,
            Method::POST,
            &format!("/workspace/messaging/conversations/{conversation_id}/messages"),
            Some(json!({
                "client_key": benchmark_client_key(ordinal),
                "kind": "text",
                "body_md": format!("Timed messaging send {sample}")
            })),
        )
        .await;
        assert_eq!(send.status, StatusCode::OK, "send failed: {}", send.body);
        let sent_cursor = send
            .body
            .pointer("/data/message/sync_cursor")
            .and_then(Value::as_i64)
            .expect("send returns the allocated cursor");
        assert!(sent_cursor > cursor);

        let sync = request_json(
            &app,
            Method::GET,
            &format!("/workspace/messaging/sync?cursor={cursor}&wait=0"),
            None,
        )
        .await;
        assert_eq!(sync.status, StatusCode::OK, "sync failed: {}", sync.body);
        assert_eq!(
            sync.body.pointer("/data/cursor").and_then(Value::as_i64),
            Some(sent_cursor)
        );
        assert_eq!(
            sync.body
                .pointer("/data/messages")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            sync.body
                .pointer("/data/conversations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            sync.body.pointer("/data/has_more").and_then(Value::as_bool),
            Some(false)
        );
        cursor = sent_cursor;

        if sample >= WARMUP_SAMPLE_COUNT {
            send_samples.push(send.elapsed);
            sync_samples.push(sync.elapsed);
            delta_sizes.push(sync.bytes.len());
        }
    }

    let send_p95 = percentile95(&mut send_samples);
    let sync_p95 = percentile95(&mut sync_samples);
    let delta_p95 = percentile95(&mut delta_sizes);
    eprintln!(
        "messaging_latency conversations={CONVERSATION_COUNT} seeded_messages={SEEDED_MESSAGE_COUNT} samples={TIMED_SAMPLE_COUNT} send_p95_ms={:.3} sync_p95_ms={:.3} delta_p95_bytes={delta_p95}",
        send_p95.as_secs_f64() * 1_000.0,
        sync_p95.as_secs_f64() * 1_000.0,
    );
    assert!(
        send_p95 <= HANDLER_P95_LIMIT,
        "messaging send p95 exceeded 100 ms: {send_p95:?}"
    );
    assert!(
        sync_p95 <= HANDLER_P95_LIMIT,
        "messaging cursor sync p95 exceeded 100 ms: {sync_p95:?}"
    );
    assert!(
        delta_p95 <= TYPICAL_DELTA_LIMIT_BYTES,
        "typical serialized messaging delta exceeded 8 KiB: {delta_p95} bytes"
    );

    let message_plan = explain_cursor_plan(
        &state,
        &fixture.auth,
        MESSAGE_CURSOR_QUERY,
        fixture.user_id,
        cursor - 1,
        cursor,
    )
    .await;
    eprintln!(
        "messaging_message_cursor_explain seq_scans={:?} indexes={:?}",
        message_plan.sequential_relations, message_plan.index_names
    );
    assert!(
        !message_plan
            .sequential_relations
            .contains("messaging_message_index"),
        "message cursor plan performed a sequential scan on messaging_message_index"
    );
    assert!(
        message_plan
            .index_names
            .iter()
            .any(|index| index.starts_with("messaging_message_index_cursor_idx")),
        "message cursor plan did not use the deployed cursor index"
    );

    let conversation_plan = explain_cursor_plan(
        &state,
        &fixture.auth,
        CONVERSATION_CURSOR_QUERY,
        fixture.user_id,
        cursor - 1,
        cursor,
    )
    .await;
    eprintln!(
        "messaging_conversation_cursor_explain seq_scans={:?} indexes={:?}",
        conversation_plan.sequential_relations, conversation_plan.index_names
    );
    assert!(
        !conversation_plan
            .sequential_relations
            .contains("messaging_conversations"),
        "conversation cursor plan performed a sequential scan on messaging_conversations"
    );
    assert!(
        conversation_plan
            .index_names
            .iter()
            .any(|index| index.starts_with("messaging_conversations_")),
        "conversation cursor plan did not use a deployed conversation index"
    );
}

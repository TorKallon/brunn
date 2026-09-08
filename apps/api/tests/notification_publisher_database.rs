//! Exercises the production HTTP router with app_rw RLS and fixture-only users.
//! APNs delivery is disabled; no transport is invoked.
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

use brunn::{AppState, Config, auth, router};

struct Principal {
    user_id: Uuid,
    credential_id: Uuid,
    token: String,
}

async fn fixture() -> Option<(PgPool, AppState)> {
    let Some(url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "BRUNN_TEST_DATABASE_URL unset; skipping notification publisher database contract"
        );
        return None;
    };
    let seed = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&seed).await.unwrap();
    let mut role_url = Url::parse(&url).unwrap();
    role_url
        .query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    let mut config = Config::from_env().unwrap();
    config.database_url_rw = role_url.to_string();
    config.database_url_ro = role_url.to_string();
    config.database_url_admin = None;
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    Some((seed, AppState::connect(config).await.unwrap()))
}

async fn principal(seed: &PgPool, user_id: Option<Uuid>, capabilities: &[&str]) -> Principal {
    let user_id = if let Some(user_id) = user_id {
        user_id
    } else {
        let user_id = Uuid::now_v7();
        sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,'Publisher contract fixture')")
            .bind(user_id).bind(format!("publisher-contract:{user_id}"))
            .execute(seed).await.unwrap();
        user_id
    };
    let credential_id = Uuid::now_v7();
    let token = format!("publisher-contract-{credential_id}");
    sqlx::query("INSERT INTO brunn.api_credentials (id,user_id,label,token_hash,capabilities) VALUES ($1,$2,'Publisher contract fixture',$3,$4)")
        .bind(credential_id).bind(user_id).bind(auth::hash_token(&token)).bind(capabilities)
        .execute(seed).await.unwrap();
    sqlx::query("INSERT INTO brunn.credential_scope_grants (credential_id,user_id,scope_id) SELECT $1,$2,id FROM brunn.scopes WHERE user_id=$2 AND scope_ref='scope:root'")
        .bind(credential_id).bind(user_id).execute(seed).await.unwrap();
    Principal {
        user_id,
        credential_id,
        token,
    }
}

async fn install(seed: &PgPool, owner: &Principal) {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO brunn.notification_installations (id,user_id,client_installation_id,registered_by_credential_id,platform,environment,app_id,token_ciphertext,token_nonce,token_hash,preview) VALUES ($1,$2,$1,$3,'ios','development','com.brunn.fixture',$4,$5,$6,'generic')")
        .bind(id).bind(owner.user_id).bind(owner.credential_id)
        .bind(vec![23_u8; 64]).bind(vec![23_u8; 12]).bind(auth::hash_token(&id.to_string()))
        .execute(seed).await.unwrap();
}

async fn entry(seed: &PgPool, owner: &Principal, path: &str, accepted: bool) -> Uuid {
    let id = Uuid::now_v7();
    let date = path
        .trim_start_matches("dreams/runs/")
        .trim_end_matches(".md");
    let mut tx = seed.begin().await.unwrap();
    sqlx::query("INSERT INTO brunn.entries (id,user_id,path,title,kind,media_type,current_version) VALUES ($1,$2,$3,'Private fixture source','markdown','text/markdown',1)")
        .bind(id).bind(owner.user_id).bind(path).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions (entry_id,user_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) VALUES ($1,$2,1,$3,$4,$5,$6,$7)")
        .bind(id).bind(owner.user_id).bind(auth::hash_token("Private fixture source"))
        .bind("Private fixture source").bind(22_i64)
        .bind(json!({"dreamer_run": {"schema":"dream.run.v1","accepted":accepted,"date":date,"attempt_id":"fixture-attempt","producer_credential_id":owner.credential_id}}))
        .bind(owner.credential_id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    id
}

fn request(target: Value) -> Value {
    json!({
        "event_key":format!("dreamer:fixture:{}",Uuid::now_v7()),
        "correlation_id":"dreamer:fixture",
        "kind":"operational", "importance":"important",
        "title":"Dreamer review ready", "body":"Open Brunn to review pending items.",
        "target":target
    })
}

fn review_request(id: Uuid) -> Value {
    let mut body = request(json!({"type":"entry","entry_ref":format!("entry:{id}")}));
    body["source"] = json!({"type":"dreamer_run","ref":format!("entry:{id}"),"version_ref":format!("entry:{id}@1")});
    body
}

async fn http(
    app: &Router,
    actor: &Principal,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {}", actor.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(
                    body.map(|body| Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap_or_default(),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn publish(app: &Router, actor: &Principal, body: Value) -> (StatusCode, Value) {
    http(
        app,
        actor,
        Method::POST,
        "/v1/workspace/notifications/publish",
        Some(body),
    )
    .await
}

#[tokio::test]
async fn publisher_only_events_replay_fan_out_and_keep_inbox_sources_and_other_owners_private() {
    let Some((seed, state)) = fixture().await else {
        return;
    };
    let publisher = principal(&seed, None, &["notification:publish"]).await;
    let reader = principal(
        &seed,
        Some(publisher.user_id),
        &["read", "notification:publish"],
    )
    .await;
    let sibling = principal(&seed, Some(publisher.user_id), &["notification:publish"]).await;
    let other = principal(&seed, None, &["read", "notification:publish"]).await;
    let save_only = principal(&seed, Some(publisher.user_id), &["save"]).await;
    install(&seed, &reader).await;
    install(&seed, &other).await;
    let source_id = entry(&seed, &publisher, "sources/private.md", true).await;
    let run_id = entry(&seed, &publisher, "dreams/runs/2026-09-07.md", true).await;
    let unaccepted_run = entry(&seed, &publisher, "dreams/runs/2026-09-06.md", false).await;
    let foreign_run = entry(&seed, &other, "dreams/runs/2026-09-07.md", true).await;
    let app = router(state.clone());

    let operational = request(json!({"type":"notification"}));
    let (status, created) = publish(&app, &publisher, operational.clone()).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["delivery_count"], 1);
    assert_eq!(created["delivery_status"], "suppressed");
    assert_eq!(created["replayed"], false);
    assert_eq!(
        created.as_object().unwrap().len(),
        4,
        "minimal publisher acknowledgement"
    );
    assert!(created.get("notification").is_none());
    let notification_ref = created["notification_ref"].as_str().unwrap();
    let notification_id =
        Uuid::parse_str(notification_ref.strip_prefix("notification:").unwrap()).unwrap();
    let (_, replayed) = publish(&app, &publisher, operational.clone()).await;
    assert_eq!(replayed["notification_ref"], created["notification_ref"]);
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["delivery_count"], 1);
    sqlx::query("UPDATE brunn.notification_deliveries SET state='failed',last_error_code='fixture_failure' WHERE notification_id=$1")
        .bind(notification_id).execute(&seed).await.unwrap();
    let (_, failed_delivery) = publish(&app, &publisher, operational.clone()).await;
    assert_eq!(
        failed_delivery["delivery_status"], "failed",
        "publication replay reports persisted delivery outcome"
    );
    sqlx::query("UPDATE brunn.notification_deliveries SET state='suppressed',last_error_code='transport_disabled' WHERE notification_id=$1")
        .bind(notification_id).execute(&seed).await.unwrap();
    let mut changed = operational.clone();
    changed["body"] = json!("Different content");
    assert_eq!(
        publish(&app, &publisher, changed).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        publish(&app, &sibling, operational.clone()).await.0,
        StatusCode::CONFLICT,
        "another producer cannot replay this event"
    );
    assert_eq!(
        publish(&app, &save_only, operational.clone()).await.0,
        StatusCode::FORBIDDEN
    );

    // Read-bearing response retains its original full JSON shape.
    let (status, full) = publish(&app, &reader, request(json!({"type":"today"}))).await;
    assert_eq!(status, StatusCode::OK, "{full}");
    assert!(full["notification"]["deliveries"].is_array());
    assert_eq!(full.as_object().unwrap().len(), 3);
    let (status, foreign) = publish(&app, &other, operational.clone()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "event key is isolated by owner: {foreign}"
    );
    let foreign_notification = foreign["notification"]["notification_ref"]
        .as_str()
        .unwrap();

    for path in [
        "/v1/workspace/notifications".to_owned(),
        format!("/v1/workspace/notifications/{notification_ref}"),
        format!("/v1/workspace/notifications/{foreign_notification}"),
    ] {
        assert_eq!(
            http(&app, &publisher, Method::GET, &path, None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        http(&app,&publisher,Method::POST,"/v1/workspace/read",Some(json!({"requests":[{"ref":format!("entry:{source_id}")},{"ref":format!("entry:{foreign_run}")}]}))).await.0,
        StatusCode::FORBIDDEN,
        "publication does not confer source read access"
    );
    let publisher_auth = auth::authenticate(&state, &publisher.token).await.unwrap();
    let mut tx = state.begin_write(&publisher_auth).await.unwrap();
    for table in [
        "entries",
        "entry_versions",
        "notification_installations",
        "notification_deliveries",
        "notification_receipts",
        "notification_attempts",
        "notification_user_state",
    ] {
        let statement = format!("SELECT count(*) FROM brunn.{table}");
        let count = sqlx::query_scalar::<_, i64>(AssertSqlSafe(statement.as_str()))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 0, "publisher cannot read {table}");
    }
    let visible_notifications: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM brunn.notifications")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible_notifications, vec![notification_id]);
    tx.rollback().await.unwrap();

    // The database repeats the narrow target rule; bypassing the HTTP
    // validator cannot insert an arbitrary destination with this capability.
    let mut tx = state.begin_write(&publisher_auth).await.unwrap();
    let error = sqlx::query("INSERT INTO brunn.notifications (user_id,producer_credential_id,event_key,request_hash,correlation_id,kind,importance,title,body,target,occurred_at,expires_at) VALUES ($1,$2,$3,$4,'fixture','operational','normal','Fixture','Fixture','{\"type\":\"today\"}',clock_timestamp(),clock_timestamp()+interval '1 hour')")
        .bind(publisher.user_id).bind(publisher.credential_id)
        .bind(format!("fixture:{}",Uuid::now_v7())).bind(auth::hash_token("fixture"))
        .execute(&mut *tx).await.expect_err("RLS blocks arbitrary publisher target");
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
    let mut tx = state.begin_write(&publisher_auth).await.unwrap();
    let foreign_id =
        Uuid::parse_str(foreign_notification.strip_prefix("notification:").unwrap()).unwrap();
    let error =
        sqlx::query("SELECT * FROM brunn.publisher_notification_fanout($1,true,false,NULL)")
            .bind(foreign_id)
            .execute(&mut *tx)
            .await
            .expect_err("foreign fan-out forbidden");
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();

    for body in [
        request(json!({"type":"today"})),
        review_request(source_id),
        review_request(unaccepted_run),
        review_request(foreign_run),
        review_request(Uuid::now_v7()),
    ] {
        let (status, error) = publish(&app, &publisher, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    }
    let mut wrong_kind = operational.clone();
    wrong_kind["event_key"] = json!(Uuid::now_v7());
    wrong_kind["kind"] = json!("news_alert");
    assert_eq!(
        publish(&app, &publisher, wrong_kind).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut arbitrary_source = request(json!({"type":"notification"}));
    arbitrary_source["source"] = json!({"type":"entry","ref":format!("entry:{source_id}")});
    assert_eq!(
        publish(&app, &publisher, arbitrary_source).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut stale_version = review_request(run_id);
    stale_version["source"]["version_ref"] = json!(format!("entry:{run_id}@2"));
    assert_eq!(
        publish(&app, &publisher, stale_version).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        publish(&app, &sibling, review_request(run_id)).await.0,
        StatusCode::BAD_REQUEST
    );

    // A terminal version can follow candidate acceptance before notification
    // publication. The exact accepted earlier version remains a valid target.
    let mut tx = seed.begin().await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions (entry_id,user_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) SELECT entry_id,user_id,2,content_sha256,content,size_bytes,metadata,created_by_credential_id FROM brunn.entry_versions WHERE entry_id=$1 AND version=1")
        .bind(run_id).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE brunn.entries SET current_version=2 WHERE id=$1")
        .bind(run_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let review = review_request(run_id);
    let (status, accepted) = publish(&app, &publisher, review.clone()).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
        .bind(run_id)
        .execute(&seed)
        .await
        .unwrap();
    let (status, replayed) = publish(&app, &publisher, review).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "exact replay survives source retirement: {replayed}"
    );
    assert_eq!(replayed["replayed"], true);
    assert_eq!(
        publish(&app, &publisher, review_request(run_id)).await.0,
        StatusCode::BAD_REQUEST,
        "a new event cannot target a deleted run"
    );
    let no_installations = principal(&seed, None, &["notification:publish"]).await;
    let (status, no_transport) = publish(
        &app,
        &no_installations,
        request(json!({"type":"notification"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{no_transport}");
    assert_eq!(no_transport["delivery_count"], 0);
    assert_eq!(no_transport["delivery_status"], "no_installations");
    let pending = sqlx::query_scalar::<_,i64>("SELECT count(*) FROM brunn.notification_deliveries WHERE user_id=ANY($1) AND state <> 'suppressed'")
        .bind(vec![publisher.user_id,other.user_id]).fetch_one(&seed).await.unwrap();
    assert_eq!(pending, 0, "fixture never queues real APNs delivery");
}

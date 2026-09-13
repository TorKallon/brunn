//! Today list and quick view through the real router.
//! BRUNN_TEST_DATABASE_URL must name a disposable database (same as other gates).

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

use brunn::{AppState, Config, auth::hash_token, router};

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping task today test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");
    Some(pool)
}

async fn test_router() -> Router {
    let database = std::env::var("BRUNN_TEST_DATABASE_URL").unwrap();
    let role_url = |role: &str| {
        let mut url = url::Url::parse(&database).unwrap();
        url.query_pairs_mut()
            .append_pair("options", &format!("-c role={role}"));
        url.to_string()
    };
    let mut config = Config::from_env().unwrap();
    config.database_url_rw = role_url("app_rw");
    config.database_url_ro = role_url("app_ro");
    config.database_url_admin = None;
    config.apns_delivery_enabled = false;
    config.semantic_lane = false;
    router(AppState::connect(config).await.unwrap())
}

/// One user with two credentials: an owner device (exactly task.write plus
/// notification:manage, which lets it assert `source: owner`) and a reader.
async fn owner_with_reader(pool: &PgPool) -> (Uuid, String, String) {
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-today-test:{user_id}"))
        .bind("Task today test")
        .execute(pool)
        .await
        .expect("insert test user");
    let owner = credential(pool, user_id, &["task.write", "notification:manage"]).await;
    let reader = credential(pool, user_id, &["task.read"]).await;
    (user_id, owner, reader)
}

async fn credential(pool: &PgPool, user_id: Uuid, capabilities: &[&str]) -> String {
    let credential_id = Uuid::now_v7();
    let token = format!("task-today-{credential_id}");
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    sqlx::query("INSERT INTO brunn.api_credentials (id,user_id,label,token_hash,capabilities) VALUES ($1,$2,'Task today fixture',$3,$4)")
        .bind(credential_id).bind(user_id).bind(hash_token(&token)).bind(&capabilities).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO brunn.credential_scope_grants (credential_id,user_id,scope_id) SELECT $1,$2,id FROM brunn.scopes WHERE user_id=$2 AND scope_ref='scope:root'")
        .bind(credential_id).bind(user_id).execute(pool).await.unwrap();
    token
}

async fn call(
    app: &Router,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .method(method)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(
                    body.map(|value| Body::from(serde_json::to_vec(&value).unwrap()))
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

fn titles(response: &Value) -> Vec<&str> {
    response["data"]["items"]
        .as_array()
        .expect("candidate items")
        .iter()
        .filter_map(|item| item["title"].as_str())
        .collect()
}

async fn indexed_today_since(pool: &PgPool, user_id: Uuid, task_ref: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT today_since::text FROM brunn.task_index WHERE user_id=$1 AND task_id=$2",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(task_ref).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn native_routine_capture_deferral_completion_and_replay_are_one_coherent_flow() {
    use chrono::{Duration, Utc};
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let app = test_router().await;
    let (user, owner, reader) = owner_with_reader(&pool).await;
    sqlx::query("UPDATE brunn.task_settings SET timezone='America/Los_Angeles',quiet_hours_end='08:00' WHERE user_id=$1")
        .bind(user).execute(&pool).await.unwrap();
    let intended = (Utc::now() - Duration::days(7)).date_naive();
    let raw =
        "Care routine, every 14 days after actual completion; plants can be lost if care slips";
    let (status, capture) = call(&app, &owner, Method::POST, "/v1/workspace/tasks/capture", Some(json!({
        "idempotency_key":"native-capture", "items":[{"raw_text":raw,"today":true,
        "soft_due":{"value":intended,"source":"owner"},
        "consequence":{"value":{"description":"Plants can be lost","severity":"serious","timing":{"kind":"after_due","days":2}},"source":"owner"},
        "recurrence":{"value":{"kind":"native","mode":"after_completion","every_days":14,"timezone":"UTC"},"source":"owner"}}]
    }))).await;
    assert!(status.is_success(), "{capture}");
    let id = capture["data"]["items"][0]["task_ref"].as_str().unwrap();
    let path = format!("/v1/workspace/tasks/{id}");
    let (_, timing) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=timing",
        None,
    )
    .await;
    assert_eq!(timing["data"]["items"][0]["timing"]["serious"], true);
    for version in 1..=3 {
        let (status, result) = call(&app, &owner, Method::PATCH, &path, Some(json!({"expected_version":version,
            "idempotency_key":format!("defer-{version}"),"operation":{"type":"snooze","source":"owner","tomorrow":true}}))).await;
        assert!(status.is_success(), "{result}");
        assert_eq!(result["data"]["task"]["task"]["parked"]["value"], false);
        assert_eq!(
            result["data"]["task"]["task"]["soft_due"]["value"],
            intended.to_string()
        );
        assert!(result["data"]["deferral_warning"].is_string());
        let ready = result["data"]["task"]["task"]["ready_at"]["value"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        assert_eq!(
            ready
                .with_timezone(&chrono_tz::America::Los_Angeles)
                .time()
                .to_string(),
            "08:00:00"
        );
    }
    let (_, today) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=today",
        None,
    )
    .await;
    assert!(
        titles(&today).is_empty(),
        "deferral hides, but does not sweep Today"
    );
    assert!(indexed_today_since(&pool, user, id).await.is_some());
    let completed = Utc::now() - Duration::days(1);
    let body = json!({"expected_version":4,"idempotency_key":"native-done","operation":{"type":"complete","source":"owner","completed_via":"ios","completed_at":completed}});
    let (status, done) = call(&app, &owner, Method::PATCH, &path, Some(body.clone())).await;
    assert!(status.is_success(), "{done}");
    let next_id = done["data"]["next_occurrence_task_ref"].as_str().unwrap();
    let (status, replay) = call(&app, &owner, Method::PATCH, &path, Some(body)).await;
    assert!(status.is_success());
    assert_eq!(replay["data"]["next_occurrence_task_ref"], next_id);
    let (_, next) = call(
        &app,
        &reader,
        Method::GET,
        &format!("/v1/workspace/tasks/{next_id}"),
        None,
    )
    .await;
    assert_eq!(
        next["data"]["task"]["task"]["soft_due"]["value"],
        (completed + Duration::days(14)).date_naive().to_string()
    );
    assert_eq!(next["data"]["task"]["task"]["provenance"]["raw_text"], raw);
    assert_eq!(
        next["data"]["task"]["task"]["provenance"]["recurrence_previous_task_ref"],
        id
    );
    let (_, reopened) = call(&app, &owner, Method::PATCH, &path, Some(json!({"expected_version":5,"idempotency_key":"native-reopen","operation":{"type":"reopen","source":"owner"}}))).await;
    assert_eq!(reopened["data"]["task"]["version"], 6);
    let (_, redone) = call(&app, &owner, Method::PATCH, &path, Some(json!({"expected_version":6,"idempotency_key":"native-redone","operation":{"type":"complete","source":"owner","completed_via":"ios"}}))).await;
    assert_eq!(redone["data"]["next_occurrence_task_ref"], next_id);
}

#[tokio::test]
async fn timing_unknown_and_nonurgent_budget_remain_honest() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let app = test_router().await;
    let (_, owner, reader) = owner_with_reader(&pool).await;
    let mut items = (0..5)
        .map(|n| json!({"raw_text":format!("ordinary {n}")}))
        .collect::<Vec<_>>();
    items.push(json!({"raw_text":"Unknown risk", "consequence":{"value":{"description":"Meaningful loss","severity":"serious"},"source":"owner"}}));
    items.push(json!({"raw_text":"Real deadline","hard_due":{"value":"2026-01-01T12:00:00Z","source":"owner"}}));
    let (_, capture) = call(
        &app,
        &owner,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(json!({"idempotency_key":"timing-budget","items":items})),
    )
    .await;
    assert!(capture["data"]["items"].is_array(), "{capture}");
    let (_, available) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=available&limit=5",
        None,
    )
    .await;
    assert_eq!(titles(&available).len(), 5);
    assert!(!titles(&available).contains(&"Real deadline"));
    let (_, urgent) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=urgent",
        None,
    )
    .await;
    assert_eq!(titles(&urgent), vec!["Real deadline"]);
    let (_, timing) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=timing",
        None,
    )
    .await;
    let unknown = timing["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == "Unknown risk")
        .unwrap();
    assert_eq!(unknown["timing"]["timing_unknown"], true);
    assert_eq!(unknown["timing"]["needs_attention"], false);
}

#[tokio::test]
async fn agent_completes_owner_tasks_and_reopened_tasks_without_overriding_owner_details() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let app = test_router().await;
    let (user_id, owner, reader) = owner_with_reader(&pool).await;
    let agent = credential(&pool, user_id, &["task.read", "task.write"]).await;
    let (_, captured) = call(
        &app,
        &owner,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(json!({
            "idempotency_key":"owner-completion-capture",
            "items":[{"raw_text":"Finished owner task", "today":true,
                      "notes":{"value":"Keep owner notes","source":"owner"},
                      "soft_due":{"value":"2026-09-13","source":"owner"}}]
        })),
    )
    .await;
    let task_ref = captured["data"]["items"][0]["task_ref"]
        .as_str()
        .expect("captured task");
    let path = format!("/v1/workspace/tasks/{task_ref}");
    let completion = |version, key: &str| {
        json!({"expected_version":version,"idempotency_key":key,
        "operation":{"type":"complete","source":"agent:aether","completed_via":"agent:aether"}})
    };
    let (status, _) = call(
        &app,
        &reader,
        Method::PATCH,
        &path,
        Some(completion(1, "reader-cannot-complete")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for field in ["notes", "soft_due"] {
        let value = if field == "notes" {
            json!("Overwrite")
        } else {
            json!("2026-09-14")
        };
        let (status, _) = call(
            &app,
            &agent,
            Method::PATCH,
            &path,
            Some(json!({
                "expected_version":1,"idempotency_key":format!("no-override-{field}"),
                "operation":{"type":"correct","field":field,"value":value,"source":"agent:aether"}
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
    for version in [1, 3] {
        let body = completion(version, &format!("agent-complete-{version}"));
        let (status, done) = call(&app, &agent, Method::PATCH, &path, Some(body.clone())).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        let task = &done["data"]["task"];
        let fields = &task["task"];
        assert_eq!(task["version"], version + 1);
        assert_eq!(fields["status"]["value"], "done");
        assert!(
            fields["status"]["source"]
                .as_str()
                .unwrap()
                .starts_with("agent:")
        );
        assert_eq!(fields["status"]["source"], fields["completed_via"]["value"]);
        assert_eq!(fields["notes"]["value"], "Keep owner notes");
        assert_eq!(fields["notes"]["source"], "owner");
        assert_eq!(fields["soft_due"]["value"], "2026-09-13");
        assert!(fields.get("today_since").is_none_or(Value::is_null));
        assert!(fields["done_at"].is_string());
        assert_eq!(done["data"]["done_today_count"], 1);
        let (status, replay) = call(&app, &agent, Method::PATCH, &path, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(replay["status"], "no_op");
        assert_eq!(replay["data"]["task"]["version"], version + 1);
        if version == 1 {
            let (status, reopened) = call(
                &app,
                &owner,
                Method::PATCH,
                &path,
                Some(json!({
                    "expected_version":2,"idempotency_key":"owner-reopened",
                    "operation":{"type":"reopen","source":"owner"}
                })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{reopened}");
            assert_eq!(
                reopened["data"]["task"]["task"]["status"]["source"],
                "owner"
            );
        }
    }
    let (status, _) = call(
        &app,
        &agent,
        Method::PATCH,
        &path,
        Some(completion(1, "stale-completion")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, current) = call(&app, &reader, Method::GET, &path, None).await;
    assert_eq!(current["data"]["task"]["task"]["status"]["value"], "done");
    assert_eq!(indexed_today_since(&pool, user_id, task_ref).await, None);
    let previous_owner_changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM brunn.task_corrections WHERE user_id=$1 AND task_id=$2 AND field_name='status' AND previous_source='owner' AND corrected_source LIKE 'agent:%'"
    ).bind(user_id).bind(Uuid::parse_str(task_ref).unwrap()).fetch_one(&pool).await.unwrap();
    // Capture initializes status as derived; reopening explicitly sets it to owner.
    assert_eq!(previous_owner_changes, 1);
}

#[tokio::test]
async fn capture_today_and_quick_views_and_add_today_sweep_round_trip() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let app = test_router().await;
    let (user_id, token, reader) = owner_with_reader(&pool).await;
    let owner_today = sqlx::query_scalar::<_, String>(
        "SELECT (clock_timestamp() AT TIME ZONE timezone)::date::text FROM brunn.task_settings WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("owner-local date from task settings");

    let capture = json!({
        "idempotency_key": "today-capture-1",
        "items": [
            {"raw_text": "Call the dentist", "today": true,
             "estimate_minutes": {"value": 5, "source": "owner"}},
            {"raw_text": "Write the report",
             "estimate_minutes": {"value": 6, "source": "owner"}},
            {"raw_text": "Plain task"}
        ]
    });
    let (status, captured) = call(
        &app,
        &token,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(capture),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{captured}");
    let dentist = &captured["data"]["items"][0];
    assert_eq!(dentist["enrichment"]["today_since"]["value"], owner_today);
    assert_eq!(dentist["enrichment"]["today_since"]["source"], "owner");
    assert!(captured["data"]["items"][1]["enrichment"]["today_since"].is_null());
    let dentist_ref = dentist["task_ref"].as_str().unwrap().to_owned();
    assert_eq!(
        indexed_today_since(&pool, user_id, &dentist_ref).await,
        Some(owner_today.clone())
    );

    let (status, today) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=today",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{today}");
    assert_eq!(today["data"]["view"], "today");
    assert_eq!(titles(&today), ["Call the dentist"]);
    assert_eq!(today["data"]["items"][0]["today_since"], owner_today);
    assert_eq!(today["data"]["items"][0]["estimate_minutes"], 5);

    let (status, quick) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=quick",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{quick}");
    assert_eq!(quick["data"]["view"], "quick");
    assert_eq!(titles(&quick), ["Call the dentist"]);

    let (status, next) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=next",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(titles(&next).len(), 3);
    let plain = &next["data"]["items"][2];
    assert_eq!(plain["title"], "Plain task");
    assert!(plain["estimate_minutes"].is_null());
    assert!(plain["today_since"].is_null());
    assert!(plain.get("estimate_minutes").is_some() && plain.get("today_since").is_some());

    for bad in [
        "/v1/workspace/tasks/candidates?view=soon",
        "/v1/workspace/tasks/candidates?view=quick&limit=26",
        "/v1/workspace/tasks/candidates?view=today&limit=26",
    ] {
        let (status, _) = call(&app, &reader, Method::GET, bad, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let patch = |key: &str, version: i64, action: &str| {
        json!({
            "expected_version": version,
            "idempotency_key": key,
            "operation": {"type": action, "source": "owner"}
        })
    };
    let task_path = format!("/v1/workspace/tasks/{dentist_ref}");
    let (status, swept) = call(
        &app,
        &token,
        Method::PATCH,
        &task_path,
        Some(patch("sweep-1", 1, "sweep")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{swept}");
    assert_eq!(
        indexed_today_since(&pool, user_id, &dentist_ref).await,
        None
    );
    let (_, today) = call(
        &app,
        &reader,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=today",
        None,
    )
    .await;
    assert!(titles(&today).is_empty());

    let (status, added) = call(
        &app,
        &token,
        Method::PATCH,
        &task_path,
        Some(patch("add-today-1", 2, "add_today")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(
        indexed_today_since(&pool, user_id, &dentist_ref).await,
        Some(owner_today)
    );
    let actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM brunn.task_audit_events WHERE user_id=$1 AND task_id=$2 ORDER BY created_at",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(&dentist_ref).unwrap())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actions, ["task.sweep", "task.add_today"]);
}

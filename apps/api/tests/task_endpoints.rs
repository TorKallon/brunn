use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

use brunn::{
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
};

struct LivePrincipal {
    auth: AuthContext,
    token: String,
}

fn live_api_url() -> Option<String> {
    std::env::var("BRUNN_TEST_API_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
}

async fn live_principal(pool: &PgPool, label: &str, capabilities: &[&str]) -> LivePrincipal {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-http-live-{scope_id}");
    let token = format!("task-http-live-{credential_id}-secret");
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name)VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-http-live:{label}:{user_id}"))
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name)VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities)VALUES($1,$2,$3,$4,$5)")
        .bind(credential_id)
        .bind(user_id)
        .bind(label)
        .bind(hash_token(&token))
        .bind(&capabilities)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id)VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .unwrap();
    LivePrincipal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: false,
        },
        token,
    }
}

async fn live_same_user_principal(
    pool: &PgPool,
    parent: &LivePrincipal,
    label: &str,
    capabilities: &[&str],
) -> LivePrincipal {
    let credential_id = Uuid::now_v7();
    let token = format!("task-http-live-{credential_id}-secret");
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities)VALUES($1,$2,$3,$4,$5)",
    )
    .bind(credential_id)
    .bind(parent.auth.user_id.0)
    .bind(label)
    .bind(hash_token(&token))
    .bind(&capabilities)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id)
        SELECT $1,$2,scope.id
        FROM brunn.scopes AS scope
        WHERE scope.user_id=$2 AND scope.scope_ref::text=ANY($3)
        "#,
    )
    .bind(credential_id)
    .bind(parent.auth.user_id.0)
    .bind(&parent.auth.scope_refs)
    .execute(pool)
    .await
    .unwrap();
    LivePrincipal {
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: parent.auth.user_id,
            capabilities: capabilities.into_iter().collect(),
            scope_refs: parent.auth.scope_refs.clone(),
            read_only: false,
        },
        token,
    }
}

async fn api_call(
    client: &Client,
    base_url: &str,
    principal: &LivePrincipal,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let mut request = client
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(&principal.token);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.expect("send live API request");
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .expect("live API response is JSON");
    (status, value)
}

fn error_code(value: &Value) -> Option<&str> {
    value.get("error")?.get("code")?.as_str()
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping task endpoint database test");
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
    Some(pool)
}

async fn test_principal(pool: &PgPool, label: &str, capabilities: &[&str]) -> AuthContext {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-http-{scope_id}");
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name)VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-http:{label}:{user_id}"))
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name)VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities)VALUES($1,$2,$3,$4,$5)").bind(credential_id).bind(user_id).bind(label).bind(format!("task-http-token-{credential_id}")).bind(&capabilities).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id)VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .unwrap();
    AuthContext {
        credential_id: CredentialId(credential_id),
        user_id: UserId(user_id),
        capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
        scope_refs: vec![scope_ref],
        read_only: false,
    }
}

#[test]
fn task_http_routes_and_patch_cors_are_frozen() {
    let api = include_str!("../src/api.rs");
    for route in [
        "/workspace/tasks/capture",
        "/workspace/tasks/candidates",
        "/workspace/tasks/corrections",
        "/workspace/tasks/done-summary",
        "/workspace/tasks/{task_ref}",
        "/workspace/contexts",
        "/workspace/contexts/merge",
        "/workspace/contexts/{slug}",
        "/workspace/contexts/available/{surface}",
        "/workspace/projects",
        "/workspace/projects/{slug}",
        "/workspace/projects/{slug}/state",
        "/workspace/projects/{slug}/interest",
        "/workspace/tasks/settings",
        "/workspace/tasks/guard/status",
        "/workspace/integrations/todoist/status",
    ] {
        assert!(api.contains(route), "missing frozen task route {route}");
    }
    assert!(api.contains("Method::PATCH"), "CORS must allow PATCH");
}

#[tokio::test]
async fn live_owner_issues_exact_ios_task_credential_once_and_inventory_stays_content_free() {
    let (Some(pool), Some(base_url)) = (connect_test_pool().await, live_api_url()) else {
        eprintln!("BRUNN_TEST_API_URL is unset; skipping live iOS task credential contract");
        return;
    };
    let client = Client::new();
    let owner = live_principal(
        &pool,
        "ios-task-owner",
        &[
            "open",
            "query",
            "read",
            "compute",
            "verify",
            "status",
            "checkpoint",
            "save",
            "stage",
            "correct",
            "delete",
            "dream",
            "credential:manage",
            "notification:publish",
            "notification:manage",
            "secret:read",
            "secret:write",
            "task.read",
            "task.write",
            "integration.manage",
            "admin",
        ],
    )
    .await;
    let ordinary = live_principal(
        &pool,
        "ios-task-ordinary",
        &["task.read", "task.write", "notification:manage"],
    )
    .await;
    let read_only = live_principal(&pool, "ios-task-read-only", &["task.read"]).await;
    let body = json!({"name":"Rourke iPhone","access":"ios_tasks"});
    for denied in [&ordinary, &read_only] {
        let (status, response) = api_call(
            &client,
            &base_url,
            denied,
            Method::POST,
            "/v1/credentials",
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{response}");
        assert_eq!(error_code(&response), Some("capability_denied"));
    }

    let username = format!("ios-owner-{}", owner.auth.user_id.0.simple());
    let email = format!("{username}@example.com");
    sqlx::query(
        r#"
        INSERT INTO brunn.web_identities (
          user_id,username,username_normalized,email,email_normalized,
          password_hash,web_credential_id
        ) VALUES ($1,$2,$2,$3,$3,'$argon2id$fixture',$4)
        "#,
    )
    .bind(owner.auth.user_id.0)
    .bind(&username)
    .bind(&email)
    .bind(owner.auth.credential_id.0)
    .execute(&pool)
    .await
    .expect("create owner Web identity");
    let session_token = format!(
        "sws_{}{}",
        owner.auth.credential_id.0.simple(),
        "0".repeat(11)
    );
    assert_eq!(session_token.len(), 47);
    sqlx::query(
        "INSERT INTO brunn.web_sessions(user_id,credential_id,token_hash,expires_at) VALUES($1,$2,$3,clock_timestamp()+interval '1 hour')",
    )
    .bind(owner.auth.user_id.0)
    .bind(owner.auth.credential_id.0)
    .bind(hash_token(&session_token))
    .execute(&pool)
    .await
    .expect("create owner Web session");
    let mut digest = Sha256::new();
    digest.update(b"brunn.web-csrf.v1\0");
    digest.update(session_token.as_bytes());
    let csrf = URL_SAFE_NO_PAD.encode(digest.finalize());
    let session_cookie = format!("brunn_session={session_token}");
    let missing_csrf = client
        .post(format!("{base_url}/v1/credentials"))
        .header(reqwest::header::COOKIE, &session_cookie)
        .json(&body)
        .send()
        .await
        .expect("attempt iOS credential issue without CSRF");
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    let missing_csrf = missing_csrf.json::<Value>().await.unwrap();
    assert_eq!(error_code(&missing_csrf), Some("csrf_validation_failed"));
    let response = client
        .post(format!("{base_url}/v1/credentials"))
        .header(
            reqwest::header::COOKIE,
            format!("{session_cookie}; brunn_csrf={csrf}"),
        )
        .header("x-csrf-token", &csrf)
        .json(&body)
        .send()
        .await
        .expect("issue iOS credential through owner Web session");
    let status = response.status();
    let issued = response
        .json::<Value>()
        .await
        .expect("decode credential issue");
    assert_eq!(status, StatusCode::OK, "credential issue failed: {issued}");
    assert_eq!(issued["access"], "ios_tasks");
    assert_eq!(issued["name"], "iOS Tasks — Rourke iPhone");
    assert_eq!(
        issued["capabilities"],
        json!(["task.write", "notification:manage"])
    );
    let token = issued["token"]
        .as_str()
        .expect("one-time issued token")
        .to_owned();
    assert!(token.starts_with("sl_"));
    let credential_id = Uuid::parse_str(
        issued["id"]
            .as_str()
            .and_then(|value| value.strip_prefix("credential:"))
            .expect("credential ref"),
    )
    .expect("credential UUID");

    let stored = sqlx::query(
        "SELECT label,token_hash,capabilities FROM brunn.api_credentials WHERE user_id=$1 AND id=$2",
    )
    .bind(owner.auth.user_id.0)
    .bind(credential_id)
    .fetch_one(&pool)
    .await
    .expect("read issued iOS task credential");
    assert_eq!(
        stored.get::<String, _>("label"),
        "iOS Tasks — Rourke iPhone"
    );
    assert_ne!(stored.get::<String, _>("token_hash"), token);
    assert_eq!(
        stored.get::<Vec<String>, _>("capabilities"),
        ["task.write", "notification:manage"]
    );
    let audit = sqlx::query(
        "SELECT details,content_free FROM brunn.audit_events WHERE user_id=$1 AND action='auth.credential.issue' AND details->>'credential_id'=$2 ORDER BY recorded_at DESC LIMIT 1",
    )
    .bind(owner.auth.user_id.0)
    .bind(credential_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("read credential issue audit event");
    assert!(audit.get::<bool, _>("content_free"));
    assert!(
        !audit
            .get::<Value, _>("details")
            .to_string()
            .contains(&token)
    );

    let (list_status, inventory) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        "/v1/credentials",
        None,
    )
    .await;
    assert_eq!(list_status, StatusCode::OK, "inventory failed: {inventory}");
    let listed = inventory["items"]
        .as_array()
        .expect("credential inventory")
        .iter()
        .find(|item| item["id"] == issued["id"])
        .expect("issued credential appears in inventory");
    assert_eq!(listed["access"], "ios_tasks");
    assert_eq!(listed["name"], "iOS Tasks — Rourke iPhone");
    assert!(listed.get("token").is_none());

    let me = client
        .get(format!("{base_url}/v1/me"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("authenticate issued iOS task credential");
    assert_eq!(me.status(), StatusCode::OK);
    let me = me.json::<Value>().await.expect("decode iOS task identity");
    assert_eq!(
        me["capabilities"],
        json!(["notification:manage", "task.write"])
    );
    let guard = client
        .get(format!("{base_url}/v1/workspace/tasks/guard/status"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("attempt guard read with write-only iOS token");
    assert_eq!(guard.status(), StatusCode::FORBIDDEN);

    let (guard_status, guard_body) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        "/v1/workspace/tasks/guard/status",
        None,
    )
    .await;
    assert_eq!(
        guard_status,
        StatusCode::OK,
        "guard status failed: {guard_body}"
    );
    let fields = guard_body["data"]
        .as_object()
        .expect("guard status object")
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        fields,
        [
            "effective_enabled",
            "environment_enabled",
            "last_error_code",
            "last_outcome",
            "last_run_at",
            "next_run_at",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );

    let (revoke_status, revoked) = api_call(
        &client,
        &base_url,
        &owner,
        Method::DELETE,
        &format!("/v1/credentials/{}", issued["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(revoke_status, StatusCode::OK, "revoke failed: {revoked}");
    assert_eq!(revoked["status"], "revoked");
    let (_, inventory) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        "/v1/credentials",
        None,
    )
    .await;
    let listed = inventory["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == issued["id"])
        .expect("revoked iOS task credential remains in inventory");
    assert_eq!(listed["status"], "revoked");
    assert!(listed["revoked_at"].is_string());
    assert!(listed.get("token").is_none());
    let revoked_auth = client
        .get(format!("{base_url}/v1/me"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("attempt revoked iOS task token");
    assert_eq!(revoked_auth.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn live_deliberate_all_filters_terminal_history_before_count_and_cursor() {
    let (Some(_pool), Some(base_url)) = (connect_test_pool().await, live_api_url()) else {
        eprintln!("BRUNN_TEST_API_URL is unset; skipping live deliberate-all contract");
        return;
    };
    let client = Client::new();
    let owner = live_principal(&_pool, "http-deliberate-all", &["task.read"]).await;
    let owner_device = live_same_user_principal(
        &_pool,
        &owner,
        "http-deliberate-all-owner-device",
        &["task.write", "notification:manage"],
    )
    .await;
    let (project_status, project) = api_call(
        &client,
        &base_url,
        &owner_device,
        Method::PUT,
        "/v1/workspace/projects/filter-project",
        Some(&json!({
            "title":"Filter project",
            "source":"owner",
            "idempotency_key":"filter-project-create"
        })),
    )
    .await;
    assert_eq!(project_status, StatusCode::OK, "project failed: {project}");

    let capture = json!({
        "idempotency_key":"filter-task-capture",
        "items":[
            {
                "raw_text":"Done phone hard",
                "project":{"value":"filter-project","source":"owner"},
                "required_contexts":{"value":["phone"],"source":"agent:filter"},
                "hard_due":{"value":"2030-09-01T12:00:00Z","source":"owner"}
            },
            {
                "raw_text":"Dropped soft",
                "project":{"value":"filter-project","source":"owner"},
                "soft_due":{"value":"2030-09-02","source":"agent:filter"}
            },
            {
                "raw_text":"Waiting phone online",
                "project":{"value":"filter-project","source":"owner"},
                "required_contexts":{"value":["phone","online"],"source":"agent:filter"}
            },
            {
                "raw_text":"Parked future",
                "project":{"value":"filter-project","source":"owner"},
                "ready_at":{"value":"2030-09-03T12:00:00Z","source":"agent:filter"}
            },
            {
                "raw_text":"Open phone hard future",
                "project":{"value":"filter-project","source":"owner"},
                "required_contexts":{"value":["phone"],"source":"owner"},
                "ready_at":{"value":"2030-09-04T12:00:00Z","source":"owner"},
                "hard_due":{"value":"2030-09-05T12:00:00Z","source":"owner"}
            }
        ]
    });
    let (capture_status, captured) = api_call(
        &client,
        &base_url,
        &owner_device,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(&capture),
    )
    .await;
    assert_eq!(capture_status, StatusCode::OK, "capture failed: {captured}");
    let refs = captured["data"]["items"]
        .as_array()
        .expect("captured items")
        .iter()
        .map(|item| item["task_ref"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(refs.len(), 5);
    let operations = [
        (
            0,
            json!({"type":"complete","source":"owner","completed_via":"ios"}),
        ),
        (
            1,
            json!({"type":"drop","source":"owner","reason":"explicit history"}),
        ),
        (
            2,
            json!({"type":"wait_on","source":"owner","who_or_what":"vendor"}),
        ),
    ];
    for (index, operation) in operations {
        let (status, response) = api_call(
            &client,
            &base_url,
            &owner_device,
            Method::PATCH,
            &format!("/v1/workspace/tasks/{}", refs[index]),
            Some(&json!({
                "expected_version":1,
                "idempotency_key":format!("filter-transition-{index}"),
                "operation":operation
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "transition failed: {response}");
    }
    for version in 1..=3 {
        let (status, response) = api_call(
            &client,
            &base_url,
            &owner_device,
            Method::PATCH,
            &format!("/v1/workspace/tasks/{}", refs[3]),
            Some(&json!({
                "expected_version":version,
                "idempotency_key":format!("filter-snooze-{version}"),
                "operation":{"type":"snooze","source":"owner","days":1}
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "snooze failed: {response}");
    }

    let all_path = "/v1/workspace/tasks/candidates?view=all&deliberate_all=true&limit=2&include_waiting=true&include_parked=true&project=filter-project";
    let mut cursor = None;
    let mut seen = Vec::new();
    for expected_remaining in [3, 1, 0] {
        let path = cursor.as_ref().map_or_else(
            || all_path.to_owned(),
            |cursor| format!("{all_path}&cursor={cursor}"),
        );
        let (status, response) =
            api_call(&client, &base_url, &owner, Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "all page failed: {response}");
        assert_eq!(response["data"]["backlog_total"], 5);
        assert_eq!(response["data"]["next_remaining"], expected_remaining);
        for item in response["data"]["items"].as_array().unwrap() {
            seen.push((
                item["task_ref"].as_str().unwrap().to_owned(),
                item["status"].as_str().unwrap().to_owned(),
                item["reason"].as_str().unwrap().to_owned(),
            ));
        }
        cursor = response["data"]["next_cursor"].as_str().map(str::to_owned);
    }
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        5,
        "cursor pages must neither skip nor duplicate"
    );
    assert!(
        seen.iter()
            .any(|(_, status, reason)| status == "done" && reason == "Completed")
    );
    assert!(
        seen.iter()
            .any(|(_, status, reason)| status == "dropped" && reason == "Dropped")
    );

    for (suffix, expected_status, expected_count) in [
        ("", None, 3_u64),
        ("&status=waiting", Some("waiting"), 1),
        ("&status=done", Some("done"), 1),
        ("&status=dropped", Some("dropped"), 1),
        ("&status=open&include_parked=true", Some("open"), 2),
        (
            "&context=phone&include_waiting=true&include_parked=true&contexts_available=home",
            None,
            3,
        ),
        (
            "&date_type=hard&include_waiting=true&include_parked=true",
            None,
            2,
        ),
        (
            "&source=agent&include_waiting=true&include_parked=true",
            None,
            3,
        ),
        (
            "&source=owner&include_waiting=true&include_parked=true",
            None,
            5,
        ),
    ] {
        let path = format!(
            "/v1/workspace/tasks/candidates?view=all&deliberate_all=true&project=filter-project{suffix}"
        );
        let (status, response) =
            api_call(&client, &base_url, &owner, Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "filter {suffix}: {response}");
        assert_eq!(
            response["data"]["backlog_total"].as_u64(),
            Some(expected_count)
        );
        assert_eq!(
            response["data"]["items"].as_array().unwrap().len() as u64,
            expected_count
        );
        if let Some(expected_status) = expected_status {
            assert!(
                response["data"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|item| item["status"] == expected_status)
            );
        }
    }

    let filtered_page = "/v1/workspace/tasks/candidates?view=all&deliberate_all=true&project=filter-project&source=agent&include_waiting=true&include_parked=true&limit=2";
    let (first_status, first_filtered) =
        api_call(&client, &base_url, &owner, Method::GET, filtered_page, None).await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "first filtered page: {first_filtered}"
    );
    assert_eq!(first_filtered["data"]["backlog_total"], 3);
    assert_eq!(first_filtered["data"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(first_filtered["data"]["next_remaining"], 1);
    let filtered_cursor = first_filtered["data"]["next_cursor"]
        .as_str()
        .expect("filtered cursor");
    let (second_status, second_filtered) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        &format!("{filtered_page}&cursor={filtered_cursor}"),
        None,
    )
    .await;
    assert_eq!(
        second_status,
        StatusCode::OK,
        "second filtered page: {second_filtered}"
    );
    assert_eq!(second_filtered["data"]["backlog_total"], 3);
    assert_eq!(
        second_filtered["data"]["items"].as_array().unwrap().len(),
        1
    );
    assert_eq!(second_filtered["data"]["next_remaining"], 0);
    let filtered_refs = first_filtered["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second_filtered["data"]["items"].as_array().unwrap())
        .map(|item| item["task_ref"].as_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(filtered_refs.len(), 3);

    for invalid in [
        "/v1/workspace/tasks/candidates?view=next&status=open",
        "/v1/workspace/tasks/candidates?view=all&deliberate_all=true&status=done&status=open",
        "/v1/workspace/tasks/candidates?view=all&deliberate_all=true&date_type=due",
        "/v1/workspace/tasks/candidates?view=all&deliberate_all=true&source=inferred",
    ] {
        let (status, response) =
            api_call(&client, &base_url, &owner, Method::GET, invalid, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {response}");
        assert_eq!(error_code(&response), Some("invalid_request"));
    }

    let (next_status, next_response) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=next",
        None,
    )
    .await;
    assert_eq!(next_status, StatusCode::OK, "bounded next: {next_response}");
    assert!(
        next_response["data"].get("filters").is_none(),
        "bounded candidate response contract must remain byte-compatible"
    );

    let (before_status, before_done) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        "/v1/workspace/tasks/done-summary",
        None,
    )
    .await;
    assert_eq!(
        before_status,
        StatusCode::OK,
        "done summary failed: {before_done}"
    );
    assert_eq!(before_done["data"]["done_today_count"], 1);
    let (drop_status, dropped) = api_call(
        &client,
        &base_url,
        &owner_device,
        Method::PATCH,
        &format!("/v1/workspace/tasks/{}", refs[0]),
        Some(&json!({
            "expected_version":2,
            "idempotency_key":"filter-drop-completed",
            "operation":{"type":"drop","source":"owner","reason":"production canary complete"}
        })),
    )
    .await;
    assert_eq!(
        drop_status,
        StatusCode::OK,
        "done-to-dropped failed: {dropped}"
    );
    assert_eq!(
        dropped["data"]["task"]["task"]["status"]["value"],
        "dropped"
    );
    assert!(dropped["data"]["task"]["task"].get("done_at").is_none());
    assert_eq!(
        dropped["data"]["task"]["task"]["completed_via"]["value"],
        Value::Null
    );
    let (_, after_done) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        "/v1/workspace/tasks/done-summary",
        None,
    )
    .await;
    assert_eq!(after_done["data"]["done_today_count"], 0);
    assert_eq!(after_done["data"]["count"], 0);
    let (_, corrections) = api_call(
        &client,
        &base_url,
        &owner,
        Method::GET,
        &format!("/v1/workspace/tasks/corrections?task_ref={}", refs[0]),
        None,
    )
    .await;
    assert!(
        corrections["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["field"] == "status"
                    && item["previous_value"] == "done"
                    && item["corrected_value"] == "dropped"
            })
    );
}

#[tokio::test]
async fn task_mutations_have_durable_receipts_and_optimistic_versions() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let receipt_table = sqlx::query_scalar::<_, Option<String>>(
        "SELECT to_regclass('brunn.task_operation_receipts')::text",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect task receipt table");
    assert_eq!(
        receipt_table.as_deref(),
        Some("brunn.task_operation_receipts")
    );
    for table in [
        "task_contexts",
        "task_projects",
        "task_surface_defaults",
        "task_settings",
    ] {
        let version_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM information_schema.columns
              WHERE table_schema='brunn' AND table_name=$1 AND column_name='version'
            )
            "#,
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("inspect optimistic version column");
        assert!(version_exists, "{table} lacks an optimistic version");
    }
    for column in ["quiet_hours_start", "quiet_hours_end"] {
        let exists=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='brunn' AND table_name='task_settings' AND column_name=$1)").bind(column).fetch_one(&pool).await.unwrap();
        assert!(exists, "task_settings lacks {column}");
    }
    let helper_args=sqlx::query_scalar::<_,i16>("SELECT pronargs FROM pg_proc JOIN pg_namespace ON pg_namespace.oid=pg_proc.pronamespace WHERE nspname='brunn' AND proname='touch_task_project_from_checkpoint'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        helper_args, 3,
        "checkpoint activity helper must not accept caller time"
    );
}

#[tokio::test]
async fn generic_memory_principals_cannot_read_or_write_canonical_task_rows() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let auth = test_principal(&pool, "memory-only", &["read", "save"]).await;
    let task_id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let path = format!(".brunn/tasks/{task_id}.md");
    let mut fixture = pool.begin().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *fixture)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.entries(id,user_id,path,title,kind,media_type,current_version)VALUES($1,$2,$3,'Hidden task','markdown','text/markdown',1)").bind(entry_id).bind(auth.user_id.0).bind(&path).execute(&mut *fixture).await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions(id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata)VALUES($1,$2,$3,1,$4,'# Hidden task\n',14,$5)").bind(version_id).bind(auth.user_id.0).bind(entry_id).bind("f".repeat(64)).bind(json!({"kind":"task","schema":"task.v1","task":{"id":task_id,"title":"Hidden task","status":{"value":"open","source":"owner","set_at":"2026-08-27T07:00:00Z"}}})).execute(&mut *fixture).await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,path,entry_version,operation,content_sha256)VALUES($1,$2,$3,1,'create',$4)").bind(auth.user_id.0).bind(entry_id).bind(&path).bind("f".repeat(64)).execute(&mut *fixture).await.unwrap();
    fixture.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .unwrap();
    set_context(&mut tx, &auth).await.unwrap();
    let entries = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.entries WHERE user_id=$1 AND path LIKE '.brunn/tasks/%'",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let versions = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2",
    )
    .bind(auth.user_id.0)
    .bind(entry_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let changes=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM brunn.workspace_changes WHERE user_id=$1 AND path LIKE '.brunn/tasks/%'").bind(auth.user_id.0).fetch_one(&mut *tx).await.unwrap();
    assert_eq!((entries, versions, changes), (0, 0, 0));
    let update_hidden = sqlx::query(
        "UPDATE brunn.entry_versions SET content='tampered' WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(version_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let delete_hidden = sqlx::query("DELETE FROM brunn.entry_versions WHERE user_id=$1 AND id=$2")
        .bind(auth.user_id.0)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        (update_hidden.rows_affected(), delete_hidden.rows_affected()),
        (0, 0),
        "RLS makes known task versions non-addressable to generic memory principals"
    );
    let version_denied=sqlx::query("INSERT INTO brunn.entry_versions(id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata)VALUES($1,$2,$3,2,$4,'# overwritten\n',14,$5)").bind(Uuid::now_v7()).bind(auth.user_id.0).bind(entry_id).bind("e".repeat(64)).bind(json!({"kind":"task","schema":"task.v1","task":{"id":task_id,"title":"overwritten"}})).execute(&mut *tx).await.expect_err("save without task.write must not append a task version");
    assert_eq!(
        version_denied
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .unwrap();
    set_context(&mut tx, &auth).await.unwrap();
    let denied=sqlx::query("INSERT INTO brunn.entries(id,user_id,path,title,kind,media_type,current_version)VALUES($1,$2,$3,'Denied','markdown','text/markdown',1)").bind(Uuid::now_v7()).bind(auth.user_id.0).bind(format!(".brunn/tasks/{}.md",Uuid::now_v7())).execute(&mut *tx).await.expect_err("save without task.write must be denied");
    assert_eq!(
        denied
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn live_http_denies_every_task_mutation_and_hides_cross_user_tasks() {
    let (Some(pool), Some(base_url)) = (connect_test_pool().await, live_api_url()) else {
        eprintln!("BRUNN_TEST_API_URL is unset; skipping live task HTTP authorization contract");
        return;
    };
    let client = Client::new();
    let reader = live_principal(&pool, "http-reader", &["task.read"]).await;
    let writer_a = live_principal(&pool, "http-writer-a", &["task.read", "task.write"]).await;
    let writer_b = live_principal(&pool, "http-writer-b", &["task.read", "task.write"]).await;
    let task_id = Uuid::now_v7();
    let mutations = [
        (
            Method::POST,
            "/v1/workspace/tasks/capture".to_owned(),
            json!({"idempotency_key":"deny-capture","items":[{"raw_text":"denied"}]}),
        ),
        (
            Method::PATCH,
            format!("/v1/workspace/tasks/{task_id}"),
            json!({"expected_version":1,"idempotency_key":"deny-update","operation":{"type":"complete","source":"agent:test","completed_via":"agent:test"}}),
        ),
        (
            Method::POST,
            "/v1/workspace/contexts".to_owned(),
            json!({"display_name":"Denied","source":"agent:test","idempotency_key":"deny-context"}),
        ),
        (
            Method::POST,
            "/v1/workspace/contexts/merge".to_owned(),
            json!({"from":"phone","into":"online","expected_from_version":1,"expected_into_version":1,"source":"agent:test","idempotency_key":"deny-merge"}),
        ),
        (
            Method::PATCH,
            "/v1/workspace/contexts/phone".to_owned(),
            json!({"archived":true,"expected_version":1,"source":"agent:test","idempotency_key":"deny-archive"}),
        ),
        (
            Method::PUT,
            "/v1/workspace/contexts/available/test".to_owned(),
            json!({"contexts_available":[],"expected_version":0,"source":"agent:test","idempotency_key":"deny-available"}),
        ),
        (
            Method::PUT,
            "/v1/workspace/projects/denied".to_owned(),
            json!({"title":"Denied","source":"agent:test","idempotency_key":"deny-project"}),
        ),
        (
            Method::PUT,
            "/v1/workspace/projects/denied/interest".to_owned(),
            json!({"interest":"hot","expected_version":1,"source":"agent:test","idempotency_key":"deny-interest"}),
        ),
        (
            Method::PUT,
            "/v1/workspace/tasks/settings".to_owned(),
            json!({"expected_version":1,"idempotency_key":"deny-settings","hard_lead_days":7}),
        ),
    ];
    for (method, path, body) in mutations {
        let (status, response) =
            api_call(&client, &base_url, &reader, method, &path, Some(&body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "mutation {path}: {response}");
        assert_eq!(error_code(&response), Some("capability_denied"));
    }

    let capture = json!({
        "idempotency_key":"cross-user-capture",
        "items":[{"raw_text":"private task","hard_due":{"value":"2026-09-01T12:00:00Z","source":"agent:writer-a"}}]
    });
    let (status, captured) = api_call(
        &client,
        &base_url,
        &writer_a,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(&capture),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "capture failed: {captured}");
    let task_ref = captured["data"]["items"][0]["task_ref"]
        .as_str()
        .expect("capture task ref");
    let missing = Uuid::now_v7().to_string();
    for reference in [task_ref, missing.as_str()] {
        let (get_status, get_response) = api_call(
            &client,
            &base_url,
            &writer_b,
            Method::GET,
            &format!("/v1/workspace/tasks/{reference}"),
            None,
        )
        .await;
        assert_eq!(get_status, StatusCode::NOT_FOUND);
        assert_eq!(error_code(&get_response), Some("task_not_found"));
        let patch = json!({"expected_version":1,"idempotency_key":format!("hidden-{reference}"),"operation":{"type":"complete","source":"agent:writer-b","completed_via":"agent:writer-b"}});
        let (patch_status, patch_response) = api_call(
            &client,
            &base_url,
            &writer_b,
            Method::PATCH,
            &format!("/v1/workspace/tasks/{reference}"),
            Some(&patch),
        )
        .await;
        assert_eq!(patch_status, StatusCode::NOT_FOUND);
        assert_eq!(error_code(&patch_response), Some("task_not_found"));
    }
}

#[tokio::test]
async fn live_http_receipts_serialize_replay_before_cas_and_reject_bad_input_atomically() {
    let (Some(pool), Some(base_url)) = (connect_test_pool().await, live_api_url()) else {
        eprintln!("BRUNN_TEST_API_URL is unset; skipping live task HTTP idempotency contract");
        return;
    };
    let client = Client::new();
    let writer = live_principal(&pool, "http-idempotency", &["task.read", "task.write"]).await;
    let capture = json!({
        "idempotency_key":"capture-once",
        "items":[{"raw_text":"complete exactly once","hard_due":{"value":"2026-09-01T12:00:00Z","source":"agent:spoofed"}}]
    });
    let (first_status, first) = api_call(
        &client,
        &base_url,
        &writer,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(&capture),
    )
    .await;
    assert_eq!(first_status, StatusCode::OK, "capture failed: {first}");
    let expected_agent = format!("agent:{}", writer.auth.credential_id.0);
    assert_eq!(
        first["data"]["items"][0]["enrichment"]["hard_due"]["source"],
        expected_agent
    );
    let task_ref = first["data"]["items"][0]["task_ref"]
        .as_str()
        .expect("capture task ref")
        .to_owned();
    let (past_status, past_candidates) = api_call(
        &client,
        &base_url,
        &writer,
        Method::GET,
        "/v1/workspace/tasks/candidates?view=next&as_of=2000-01-01T00%3A00%3A00Z",
        None,
    )
    .await;
    assert_eq!(
        past_status,
        StatusCode::OK,
        "past candidates failed: {past_candidates}"
    );
    assert_eq!(past_candidates["data"]["items"], json!([]));
    assert_eq!(past_candidates["data"]["backlog_total"], 0);
    let (_, replay) = api_call(
        &client,
        &base_url,
        &writer,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(&capture),
    )
    .await;
    assert_eq!(replay["status"], "no_op");
    assert_eq!(replay["data"]["replayed"], true);
    let changed_capture =
        json!({"idempotency_key":"capture-once","items":[{"raw_text":"different"}]});
    let (changed_status, changed) = api_call(
        &client,
        &base_url,
        &writer,
        Method::POST,
        "/v1/workspace/tasks/capture",
        Some(&changed_capture),
    )
    .await;
    assert_eq!(changed_status, StatusCode::CONFLICT);
    assert_eq!(error_code(&changed), Some("idempotency_conflict"));

    let update = json!({"expected_version":1,"idempotency_key":"complete-once","operation":{"type":"complete","source":"agent:spoofed","completed_via":"agent:spoofed"}});
    let path = format!("/v1/workspace/tasks/{task_ref}");
    let left = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&update),
    );
    let right = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&update),
    );
    let ((left_status, left_body), (right_status, right_body)) = tokio::join!(left, right);
    assert_eq!(
        (left_status, right_status),
        (StatusCode::OK, StatusCode::OK)
    );
    let replay_count = [
        left_body["data"]["replayed"].as_bool(),
        right_body["data"]["replayed"].as_bool(),
    ]
    .into_iter()
    .filter(|value| *value == Some(true))
    .count();
    assert_eq!(replay_count, 1, "one concurrent request must replay");
    for response in [&left_body, &right_body] {
        assert_eq!(
            response["data"]["task"]["task"]["completed_via"]["value"],
            expected_agent
        );
        assert_eq!(response["data"]["task"]["version"], 2);
    }
    let (_, replay_after_cas) = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&update),
    )
    .await;
    assert_eq!(replay_after_cas["status"], "no_op");
    assert_eq!(replay_after_cas["data"]["task"]["version"], 2);
    let changed_update = json!({"expected_version":1,"idempotency_key":"complete-once","operation":{"type":"complete","source":"agent:different","completed_via":"agent:different"}});
    let (changed_status, changed_body) = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&changed_update),
    )
    .await;
    assert_eq!(changed_status, StatusCode::CONFLICT);
    assert_eq!(error_code(&changed_body), Some("idempotency_conflict"));
    let stale_update = json!({"expected_version":1,"idempotency_key":"stale-new-key","operation":{"type":"reopen","source":"agent:spoofed"}});
    let (stale_status, stale_body) = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&stale_update),
    )
    .await;
    assert_eq!(stale_status, StatusCode::CONFLICT);
    assert_eq!(error_code(&stale_body), Some("task_version_conflict"));

    let ios_spoof = json!({"expected_version":2,"idempotency_key":"ios-spoof","operation":{"type":"reopen","source":"agent:spoofed"}});
    let (_, reopened) = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&ios_spoof),
    )
    .await;
    assert_eq!(reopened["data"]["task"]["version"], 3);
    let forbidden_channel = json!({"expected_version":3,"idempotency_key":"ios-spoof-complete","operation":{"type":"complete","source":"agent:spoofed","completed_via":"ios"}});
    let (forbidden_status, forbidden_body) = api_call(
        &client,
        &base_url,
        &writer,
        Method::PATCH,
        &path,
        Some(&forbidden_channel),
    )
    .await;
    assert_eq!(forbidden_status, StatusCode::FORBIDDEN);
    assert_eq!(
        error_code(&forbidden_body),
        Some("completion_channel_denied")
    );

    let baseline_receipts = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.task_operation_receipts WHERE user_id=$1",
    )
    .bind(writer.auth.user_id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    let baseline_entries =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.entries WHERE user_id=$1")
            .bind(writer.auth.user_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let invalid_requests = [
        (
            Method::POST,
            "/v1/workspace/tasks/capture".to_owned(),
            json!({"idempotency_key":"bad-control-capture","items":[{"raw_text":"bad","client_ref":"bad\0ref"}]}),
        ),
        (
            Method::PATCH,
            path.clone(),
            json!({"expected_version":3,"idempotency_key":"bad-control-update","operation":{"type":"correct","field":"title","value":"bad\0title","source":"agent:self"}}),
        ),
        (
            Method::POST,
            "/v1/workspace/contexts".to_owned(),
            json!({"display_name":"bad\0context","source":"agent:self","idempotency_key":"bad-control-context"}),
        ),
        (
            Method::PUT,
            "/v1/workspace/projects/bad-project".to_owned(),
            json!({"title":"bad\0project","source":"agent:self","idempotency_key":"bad-control-project"}),
        ),
        (
            Method::PUT,
            "/v1/workspace/tasks/settings".to_owned(),
            json!({"expected_version":1,"idempotency_key":"bad-control-settings","timezone":"UTC\0evil"}),
        ),
    ];
    for (method, request_path, body) in invalid_requests {
        let (status, response) = api_call(
            &client,
            &base_url,
            &writer,
            method,
            &request_path,
            Some(&body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{request_path}: {response}"
        );
        assert_eq!(error_code(&response), Some("invalid_request"));
    }
    let after_receipts = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.task_operation_receipts WHERE user_id=$1",
    )
    .bind(writer.auth.user_id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    let after_entries =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.entries WHERE user_id=$1")
            .bind(writer.auth.user_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_receipts, baseline_receipts,
        "400s must not leave receipts"
    );
    assert_eq!(
        after_entries, baseline_entries,
        "400s must not write entries"
    );
}

#[tokio::test]
async fn live_portable_task_import_rejects_projection_ranges_before_any_durable_write() {
    let (Some(pool), Some(base_url)) = (connect_test_pool().await, live_api_url()) else {
        eprintln!("BRUNN_TEST_API_URL is unset; skipping live portable task range contract");
        return;
    };
    let client = Client::new();
    let owner = live_principal(
        &pool,
        "http-portable-owner",
        &[
            "open",
            "query",
            "read",
            "compute",
            "verify",
            "status",
            "checkpoint",
            "save",
            "stage",
            "correct",
            "delete",
            "dream",
            "notification:publish",
            "notification:manage",
            "secret:read",
            "secret:write",
            "task.read",
            "task.write",
            "integration.manage",
            "credential:manage",
            "admin",
        ],
    )
    .await;
    let durable_counts = |pool: PgPool, user_id: Uuid| async move {
        let entries =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM brunn.entries WHERE user_id=$1")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let versions = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.entry_versions WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let changes = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.workspace_changes WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let workspace_receipts = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.workspace_idempotency_receipts WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let task_receipts = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.task_operation_receipts WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        (
            entries,
            versions,
            changes,
            workspace_receipts,
            task_receipts,
        )
    };
    let before = durable_counts(pool.clone(), owner.auth.user_id.0).await;
    for (field, value) in [
        ("hard_due_lead_days", json!(3651)),
        ("estimate_minutes", json!(0)),
    ] {
        let task_id = Uuid::now_v7();
        let path = format!(".brunn/tasks/{task_id}.md");
        let metadata = json!({
            "_brunn_import":{"format":"brunn-workspace-import-manifest@v1"},
            "portable":{"modified_unix_ns":null,"mode":null},
            "client":{
                "kind":"task",
                "schema":"task.v1",
                "task":{
                    "id":task_id,
                    "title":"Invalid portable range",
                    "status":{"value":"open","source":"owner","set_at":"2026-08-27T07:00:00Z"},
                    field:{"value":value,"source":"owner","set_at":"2026-08-27T07:00:00Z"}
                }
            }
        });
        let body = json!({
            "path":path,
            "content":"# Invalid portable range\n",
            "media_type":"text/markdown",
            "metadata":metadata,
            "expected_version":0,
            "idempotency_key":format!("portable-invalid-{field}-{task_id}"),
        });
        let (status, response) = api_call(
            &client,
            &base_url,
            &owner,
            Method::POST,
            "/v1/workspace/write",
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {response}");
        assert_eq!(error_code(&response), Some("invalid_request"));
        let stored = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM brunn.entries WHERE user_id=$1 AND path=$2)",
        )
        .bind(owner.auth.user_id.0)
        .bind(&path)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!stored, "invalid portable task must leave no entry");
    }
    let after = durable_counts(pool.clone(), owner.auth.user_id.0).await;
    assert_eq!(after, before, "400 portable imports must be fully atomic");
}

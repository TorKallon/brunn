use std::collections::HashSet;

use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use straylight::{
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
};

struct LivePrincipal {
    auth: AuthContext,
    token: String,
}

fn live_api_url() -> Option<String> {
    std::env::var("STRAYLIGHT_TEST_API_URL")
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
    sqlx::query("INSERT INTO straylight.users(id,external_ref,display_name)VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-http-live:{label}:{user_id}"))
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO straylight.scopes(id,user_id,scope_ref,name)VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO straylight.api_credentials(id,user_id,label,token_hash,capabilities)VALUES($1,$2,$3,$4,$5)")
        .bind(credential_id)
        .bind(user_id)
        .bind(label)
        .bind(hash_token(&token))
        .bind(&capabilities)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO straylight.credential_scope_grants(credential_id,user_id,scope_id)VALUES($1,$2,$3)")
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
    let Some(database_url) = std::env::var("STRAYLIGHT_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("STRAYLIGHT_TEST_DATABASE_URL is unset; skipping task endpoint database test");
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

async fn test_principal(pool: &PgPool, label: &str, capabilities: &[&str]) -> AuthContext {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:task-http-{scope_id}");
    let capabilities = capabilities
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    sqlx::query("INSERT INTO straylight.users(id,external_ref,display_name)VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("task-http:{label}:{user_id}"))
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO straylight.scopes(id,user_id,scope_ref,name)VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind(label)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO straylight.api_credentials(id,user_id,label,token_hash,capabilities)VALUES($1,$2,$3,$4,$5)").bind(credential_id).bind(user_id).bind(label).bind(format!("task-http-token-{credential_id}")).bind(&capabilities).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO straylight.credential_scope_grants(credential_id,user_id,scope_id)VALUES($1,$2,$3)").bind(credential_id).bind(user_id).bind(scope_id).execute(pool).await.unwrap();
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
        "/workspace/integrations/todoist/status",
    ] {
        assert!(api.contains(route), "missing frozen task route {route}");
    }
    assert!(api.contains("Method::PATCH"), "CORS must allow PATCH");
}

#[tokio::test]
async fn task_mutations_have_durable_receipts_and_optimistic_versions() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let receipt_table = sqlx::query_scalar::<_, Option<String>>(
        "SELECT to_regclass('straylight.task_operation_receipts')::text",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect task receipt table");
    assert_eq!(
        receipt_table.as_deref(),
        Some("straylight.task_operation_receipts")
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
              WHERE table_schema='straylight' AND table_name=$1 AND column_name='version'
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
        let exists=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='straylight' AND table_name='task_settings' AND column_name=$1)").bind(column).fetch_one(&pool).await.unwrap();
        assert!(exists, "task_settings lacks {column}");
    }
    let helper_args=sqlx::query_scalar::<_,i16>("SELECT pronargs FROM pg_proc JOIN pg_namespace ON pg_namespace.oid=pg_proc.pronamespace WHERE nspname='straylight' AND proname='touch_task_project_from_checkpoint'").fetch_one(&pool).await.unwrap();
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
    let path = format!(".straylight/tasks/{task_id}.md");
    let mut fixture = pool.begin().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *fixture)
        .await
        .unwrap();
    sqlx::query("INSERT INTO straylight.entries(id,user_id,path,title,kind,media_type,current_version)VALUES($1,$2,$3,'Hidden task','markdown','text/markdown',1)").bind(entry_id).bind(auth.user_id.0).bind(&path).execute(&mut *fixture).await.unwrap();
    sqlx::query("INSERT INTO straylight.entry_versions(id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata)VALUES($1,$2,$3,1,$4,'# Hidden task\n',14,$5)").bind(version_id).bind(auth.user_id.0).bind(entry_id).bind("f".repeat(64)).bind(json!({"kind":"task","schema":"task.v1","task":{"id":task_id,"title":"Hidden task","status":{"value":"open","source":"owner","set_at":"2026-08-27T07:00:00Z"}}})).execute(&mut *fixture).await.unwrap();
    sqlx::query("INSERT INTO straylight.workspace_changes(user_id,entry_id,path,entry_version,operation,content_sha256)VALUES($1,$2,$3,1,'create',$4)").bind(auth.user_id.0).bind(entry_id).bind(&path).bind("f".repeat(64)).execute(&mut *fixture).await.unwrap();
    fixture.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE app_rw")
        .execute(&mut *tx)
        .await
        .unwrap();
    set_context(&mut tx, &auth).await.unwrap();
    let entries=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM straylight.entries WHERE user_id=$1 AND path LIKE '.straylight/tasks/%'").bind(auth.user_id.0).fetch_one(&mut *tx).await.unwrap();
    let versions = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM straylight.entry_versions WHERE user_id=$1 AND entry_id=$2",
    )
    .bind(auth.user_id.0)
    .bind(entry_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let changes=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM straylight.workspace_changes WHERE user_id=$1 AND path LIKE '.straylight/tasks/%'").bind(auth.user_id.0).fetch_one(&mut *tx).await.unwrap();
    assert_eq!((entries, versions, changes), (0, 0, 0));
    let update_hidden = sqlx::query(
        "UPDATE straylight.entry_versions SET content='tampered' WHERE user_id=$1 AND id=$2",
    )
    .bind(auth.user_id.0)
    .bind(version_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let delete_hidden =
        sqlx::query("DELETE FROM straylight.entry_versions WHERE user_id=$1 AND id=$2")
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
    let version_denied=sqlx::query("INSERT INTO straylight.entry_versions(id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata)VALUES($1,$2,$3,2,$4,'# overwritten\n',14,$5)").bind(Uuid::now_v7()).bind(auth.user_id.0).bind(entry_id).bind("e".repeat(64)).bind(json!({"kind":"task","schema":"task.v1","task":{"id":task_id,"title":"overwritten"}})).execute(&mut *tx).await.expect_err("save without task.write must not append a task version");
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
    let denied=sqlx::query("INSERT INTO straylight.entries(id,user_id,path,title,kind,media_type,current_version)VALUES($1,$2,$3,'Denied','markdown','text/markdown',1)").bind(Uuid::now_v7()).bind(auth.user_id.0).bind(format!(".straylight/tasks/{}.md",Uuid::now_v7())).execute(&mut *tx).await.expect_err("save without task.write must be denied");
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
        eprintln!(
            "STRAYLIGHT_TEST_API_URL is unset; skipping live task HTTP authorization contract"
        );
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
        eprintln!("STRAYLIGHT_TEST_API_URL is unset; skipping live task HTTP idempotency contract");
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
        "SELECT count(*) FROM straylight.task_operation_receipts WHERE user_id=$1",
    )
    .bind(writer.auth.user_id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    let baseline_entries =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM straylight.entries WHERE user_id=$1")
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
        "SELECT count(*) FROM straylight.task_operation_receipts WHERE user_id=$1",
    )
    .bind(writer.auth.user_id.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    let after_entries =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM straylight.entries WHERE user_id=$1")
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
        eprintln!("STRAYLIGHT_TEST_API_URL is unset; skipping live portable task range contract");
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
        let entries = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.entries WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let versions = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.entry_versions WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let changes = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.workspace_changes WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let workspace_receipts = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.workspace_idempotency_receipts WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let task_receipts = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM straylight.task_operation_receipts WHERE user_id=$1",
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
        let path = format!(".straylight/tasks/{task_id}.md");
        let metadata = json!({
            "_straylight_import":{"format":"straylight-workspace-import-manifest@v1"},
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
            "SELECT EXISTS(SELECT 1 FROM straylight.entries WHERE user_id=$1 AND path=$2)",
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

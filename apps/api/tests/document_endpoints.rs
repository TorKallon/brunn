use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use brunn::{
    AppState, Config, auth::hash_token, document_service::get_document_in_tx, error::ApiError,
    router,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping document endpoint test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to disposable Postgres");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");
    Some(pool)
}

async fn insert_test_user(pool: &PgPool) -> Uuid {
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("document-endpoint-test:{user_id}"))
        .bind("Document endpoint test")
        .execute(pool)
        .await
        .expect("insert test user");
    user_id
}

async fn document_principal(pool: &PgPool) -> (Uuid, String) {
    let user = insert_test_user(pool).await;
    let id = Uuid::now_v7();
    let token = format!("document-contract-{id}");
    sqlx::query("INSERT INTO brunn.api_credentials (id,user_id,label,token_hash,capabilities) VALUES ($1,$2,'Document fixture',$3,ARRAY['save','read'])")
        .bind(id).bind(user).bind(hash_token(&token)).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO brunn.credential_scope_grants (credential_id,user_id,scope_id) SELECT $1,$2,id FROM brunn.scopes WHERE user_id=$2 AND scope_ref='scope:root'")
        .bind(id).bind(user).execute(pool).await.unwrap();
    (user, token)
}

async fn document_http(
    app: &Router,
    token: Option<&str>,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request =
        Request::builder()
            .uri(path)
            .method(if body.is_some() { "POST" } else { "GET" });
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
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

#[tokio::test]
async fn authenticated_publish_and_get_return_additive_links_through_the_real_router() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let app = test_router().await;
    let (owner, token) = document_principal(&pool).await;
    let (_, other) = document_principal(&pool).await;
    let body = json!({"slug":"http-plan","title":"HTTP plan","body_md":"First body.","expected_version":0});
    let (status, first) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/publish",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["data"]["app_url"], "brunn://document/http-plan");
    assert_eq!(
        first["data"]["app_version_url"],
        "brunn://document/http-plan?version=1"
    );
    let (status, current) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/http-plan",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for key in ["url", "version_url", "app_url", "app_version_url"] {
        assert_eq!(first["data"][key], current["data"][key]);
    }
    assert_eq!(current["data"]["body_md"], "First body.");
    let body = json!({"slug":"http-plan","title":"HTTP plan","body_md":"Updated body.","expected_version":1});
    let (status, second) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/publish",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["data"]["app_version_url"],
        "brunn://document/http-plan?version=2"
    );
    assert_eq!(second["data"]["url"], first["data"]["url"]);
    let (_, latest) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/http-plan",
        None,
    )
    .await;
    assert_eq!(latest["data"]["version"], 2);
    let (_, pinned) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/http-plan?version=1",
        None,
    )
    .await;
    assert_eq!(pinned["data"]["body_md"], "First body.");
    assert_eq!(pinned["data"]["current_version"], 2);
    assert_eq!(
        pinned["data"]["app_version_url"],
        first["data"]["app_version_url"]
    );
    assert_eq!(
        document_http(&app, None, "/v1/workspace/documents/http-plan", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for (reader, path) in [
        (&other, "/v1/workspace/documents/http-plan"),
        (&other, "/v1/workspace/documents/http-plan?version=1"),
        (&token, "/v1/workspace/documents/http-plan?version=99"),
    ] {
        assert_eq!(
            document_http(&app, Some(reader), path, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    insert_entry_version(
        &pool,
        owner,
        Uuid::now_v7(),
        1,
        "Private raw file",
        json!({}),
    )
    .await;
    assert_eq!(
        document_http(
            &app,
            Some(&token),
            "/v1/workspace/documents/trip-plan",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

async fn insert_entry_version(
    pool: &PgPool,
    user_id: Uuid,
    entry_id: Uuid,
    version: i64,
    content: &str,
    metadata: Value,
) {
    let mut tx = pool.begin().await.expect("begin entry insert");
    sqlx::query(
        r#"
        INSERT INTO brunn.entries (
          id,user_id,path,title,kind,media_type,current_version
        ) VALUES ($1,$2,'Documents/trip-plan.md','Trip plan','markdown','text/markdown',$3)
        ON CONFLICT (user_id,(lower(normalize(path, NFC)))) DO UPDATE
        SET current_version=EXCLUDED.current_version
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(version)
    .execute(&mut *tx)
    .await
    .expect("insert or advance entry");
    sqlx::query(
        r#"
        INSERT INTO brunn.entry_versions (
          id,user_id,entry_id,version,content_sha256,content,size_bytes,metadata
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(entry_id)
    .bind(version)
    .bind(hex::encode(Sha256::digest(content.as_bytes())))
    .bind(content)
    .bind(i64::try_from(content.len()).expect("content fits"))
    .bind(metadata)
    .execute(&mut *tx)
    .await
    .expect("insert entry version");
    tx.commit().await.expect("commit entry insert");
}

fn document_metadata(summary: &str) -> Value {
    json!({
        "kind": "human_document",
        "document": {
            "schema": "document.v1",
            "slug": "trip-plan",
            "title": "Trip plan",
            "summary": summary,
            "sources": []
        }
    })
}

#[tokio::test]
async fn reads_only_marked_history_and_unmarked_current_head_unpublishes() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let user_id = insert_test_user(&pool).await;
    let other_user_id = insert_test_user(&pool).await;
    let entry_id = Uuid::now_v7();
    insert_entry_version(&pool, user_id, entry_id, 1, "raw predecessor\n", json!({})).await;
    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        2,
        "# Trip plan\n\nFirst body.\n",
        document_metadata("First summary."),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        3,
        "# Trip plan\n\nCurrent body.\n",
        document_metadata("Current summary."),
    )
    .await;
    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        4,
        "uncurated later workspace edit\n",
        json!({}),
    )
    .await;

    let mut tx = pool.begin().await.expect("begin hidden-head read");
    let error = get_document_in_tx(&mut tx, user_id, "https://brunn.example", "trip-plan", None)
        .await
        .expect_err("an unmarked current head must unpublish the stable route");
    assert!(matches!(
        error,
        ApiError::Public {
            code: "document_not_found",
            ..
        }
    ));
    tx.rollback().await.expect("rollback hidden-head read");

    sqlx::query("UPDATE brunn.entries SET current_version=3 WHERE user_id=$1 AND id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&pool)
        .await
        .expect("restore marked current head");
    let mut tx = pool.begin().await.expect("begin document reads");
    let current = get_document_in_tx(&mut tx, user_id, "https://brunn.example", "trip-plan", None)
        .await
        .expect("current document loads");
    assert_eq!(current["version"], 3);
    assert_eq!(current["current_version"], 3);
    assert_eq!(current["body_md"], "Current body.");
    assert_eq!(current["summary"], "Current summary.");
    assert_eq!(
        current["versions"]
            .as_array()
            .expect("versions array")
            .iter()
            .map(|item| item["version"].as_i64().expect("numeric version"))
            .collect::<Vec<_>>(),
        [2, 3],
        "the raw predecessor must not be promoted into document history",
    );
    assert_eq!(
        current["version_url"],
        "https://brunn.example/documents/trip-plan?version=3",
    );
    assert_eq!(current["url"], "https://brunn.example/documents/trip-plan");
    assert_eq!(current["app_url"], "brunn://document/trip-plan");
    assert_eq!(
        current["app_version_url"],
        "brunn://document/trip-plan?version=3"
    );
    assert_eq!(
        current["versions"][0]["app_version_url"],
        "brunn://document/trip-plan?version=2"
    );

    let historical = get_document_in_tx(
        &mut tx,
        user_id,
        "https://brunn.example",
        "trip-plan",
        Some(2),
    )
    .await
    .expect("marked historical version loads");
    assert_eq!(historical["body_md"], "First body.");
    assert_eq!(historical["version"], 2);
    assert_eq!(historical["current_version"], 3);
    assert_eq!(historical["app_url"], current["app_url"]);
    assert_eq!(
        historical["app_version_url"],
        "brunn://document/trip-plan?version=2"
    );

    for (reader, version) in [
        (user_id, Some(1)),
        (user_id, Some(99)),
        (other_user_id, None),
        (other_user_id, Some(2)),
    ] {
        let error = get_document_in_tx(
            &mut tx,
            reader,
            "https://brunn.example",
            "trip-plan",
            version,
        )
        .await
        .expect_err("unmarked or cross-user document reads must be hidden");
        assert!(matches!(
            error,
            ApiError::Public {
                code: "document_not_found",
                ..
            }
        ));
    }
    tx.commit().await.expect("commit document reads");

    insert_entry_version(
        &pool,
        user_id,
        entry_id,
        5,
        "# Trip plan\n\nRepublished body.\n",
        document_metadata("Republished summary."),
    )
    .await;
    let mut tx = pool.begin().await.expect("begin republished read");
    let republished =
        get_document_in_tx(&mut tx, user_id, "https://brunn.example", "trip-plan", None)
            .await
            .unwrap();
    assert_eq!(republished["version"], 5);
    assert_eq!(republished["app_url"], current["app_url"]);
    assert_eq!(republished["url"], current["url"]);
    let pinned = get_document_in_tx(
        &mut tx,
        user_id,
        "https://brunn.example",
        "trip-plan",
        Some(2),
    )
    .await
    .unwrap();
    assert_eq!(pinned["body_md"], historical["body_md"]);
    assert_eq!(pinned["app_version_url"], historical["app_version_url"]);
    tx.rollback().await.unwrap();
}

const AGENT_ORIENTATION_MD: &str = include_str!("../src/orientation/agent-orientation.md");

#[test]
fn agent_orientation_stays_short() {
    let lines = AGENT_ORIENTATION_MD.lines().collect::<Vec<_>>();
    assert!(lines.len() < 70, "{} lines", lines.len());
    for line in lines {
        assert!(line.chars().count() <= 100, "line too long: {line}");
    }
    assert!(AGENT_ORIENTATION_MD.contains("owner asks to mark finished work done"));
    assert!(AGENT_ORIENTATION_MD.contains("completed_via: agent:<id>"));
    assert!(AGENT_ORIENTATION_MD.contains("stopping the series is a separate decision"));
}

#[tokio::test]
async fn agent_orientation_is_served_from_the_build_and_cannot_be_published() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    let app = test_router().await;
    let (_, token) = document_principal(&pool).await;
    let (status, current) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/agent-orientation",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{current}");
    let data = &current["data"];
    assert_eq!(data["slug"], "agent-orientation");
    assert_eq!(data["title"], "Brunn agent orientation");
    assert_eq!(data["markdown"], AGENT_ORIENTATION_MD);
    assert_eq!(
        AGENT_ORIENTATION_MD,
        format!(
            "# Brunn agent orientation\n\n{}\n",
            data["body_md"].as_str().unwrap()
        )
    );
    assert_eq!(data["version"], 1);
    assert_eq!(data["current_version"], 1);
    assert_eq!(data["versions"], json!([]));
    assert_eq!(data["sources"], json!([]));
    assert_eq!(data["entry_ref"], Value::Null);
    assert_eq!(
        data["app_version_url"],
        "brunn://document/agent-orientation?version=1"
    );
    let (status, pinned) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/agent-orientation?version=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pinned["data"]["body_md"], data["body_md"]);
    assert_eq!(
        document_http(
            &app,
            Some(&token),
            "/v1/workspace/documents/agent-orientation?version=2",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let body = json!({"slug":"agent-orientation","title":"Mine","body_md":"Override.","expected_version":0});
    let (status, refused) = document_http(
        &app,
        Some(&token),
        "/v1/workspace/documents/publish",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(
        refused["error"]["message"],
        "agent-orientation is a reserved slug served from the Brunn build"
    );
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.entries WHERE lower(path)='documents/agent-orientation.md'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, 0);
}

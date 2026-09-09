//! Real HTTP/database contract for narrow runner authority and owner review.
//! Fixture users only, hashing embeddings, no model or push transport.
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use brunn::{AppState, Config, auth::hash_token, router};
use chrono::Utc;
use futures::FutureExt;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

const OWNER_CAPS: &[&str] = &[
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
    "location.write",
    "integration.manage",
    "message.read",
    "message.write",
    "admin",
];
struct Actor {
    user: Uuid,
    id: Uuid,
    token: String,
}
struct Fixture {
    pool: PgPool,
    app: Router,
    owner: Actor,
    runner: Actor,
    model: Actor,
}
struct Response {
    status: StatusCode,
    body: Value,
}

async fn actor(pool: &PgPool, user: Option<Uuid>, caps: &[&str]) -> Actor {
    let user = match user {
        Some(user) => user,
        None => {
            let user = Uuid::now_v7();
            sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,'Dreamer API fixture')")
            .bind(user).bind(format!("dreamer-api-fixture:{user}")).execute(pool).await.unwrap();
            user
        }
    };
    let id = Uuid::now_v7();
    let token = format!("dreamer-api-fixture-{id}");
    sqlx::query("INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) VALUES($1,$2,'Dreamer API fixture',$3,$4)")
        .bind(id).bind(user).bind(hash_token(&token)).bind(caps).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) SELECT $1,$2,id FROM brunn.scopes WHERE user_id=$2 AND scope_ref='scope:root'")
        .bind(id).bind(user).execute(pool).await.unwrap();
    Actor { user, id, token }
}
async fn fixture() -> Option<Fixture> {
    fixture_with_deadline(None).await
}
async fn fixture_with_deadline(request_timeout: Option<std::time::Duration>) -> Option<Fixture> {
    let Some(url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL unset; skipping Dreamer API fixture");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let mut config = Config::from_env().unwrap();
    let mut rw = Url::parse(&url).unwrap();
    rw.query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    let mut ro = Url::parse(&url).unwrap();
    ro.query_pairs_mut()
        .append_pair("options", "-c role=app_ro");
    config.database_url_rw = rw.to_string();
    config.database_url_ro = ro.to_string();
    config.database_url_admin = None;
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    if let Some(timeout) = request_timeout {
        config.request_timeout = timeout;
    }
    let state = AppState::connect(config).await.unwrap();
    let owner = actor(&pool, None, OWNER_CAPS).await;
    let runner = actor(
        &pool,
        Some(owner.user),
        &["dreamer:run", "notification:publish"],
    )
    .await;
    let model = actor(&pool, Some(owner.user), &["open", "query", "read"]).await;
    Some(Fixture {
        pool,
        app: router(state),
        owner,
        runner,
        model,
    })
}
async fn request(
    app: &Router,
    actor: &Actor,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Response {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {}", actor.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(
                    body.map(|v| Body::from(serde_json::to_vec(&v).unwrap()))
                        .unwrap_or_default(),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Response {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}
async fn post(f: &Fixture, actor: &Actor, path: &str, body: Value) -> Response {
    let response = request(&f.app, actor, Method::POST, path, Some(body)).await;
    if path == "/v1/dreamer/review/decisions" && response.status == StatusCode::OK {
        assert_eq!(
            response.body["status"], "complete",
            "browser requires a complete workspace envelope"
        );
    }
    response
}
fn ok(response: Response) -> Value {
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    response.body
}
async fn write(f: &Fixture, path: &str, body: &str, version: i64) -> Value {
    ok(post(
        f,
        &f.owner,
        "/v1/workspace/write",
        json!({"path":path,"content":body,"expected_version":version,"metadata":{}}),
    )
    .await)["data"]
        .clone()
}
async fn control(f: &Fixture, mode: &str, version: i64) {
    write(
        f,
        "dreams/CONTROL.md",
        &format!("enabled: true\nmode: {mode}\nadvance_after: 2099-01-01\n"),
        version,
    )
    .await;
}
fn date() -> String {
    Utc::now()
        .with_timezone(&chrono_tz::America::Los_Angeles)
        .date_naive()
        .to_string()
}
async fn admit(f: &Fixture) -> Value {
    ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60}),
    )
    .await)
}
fn attempt(a: &Value, version: i64) -> Value {
    json!({"attempt_id":a["attempt_id"],"fence":a["fence"],"expected_state_version":version})
}
fn candidate(source: &Value, name: &str) -> Value {
    json!({"kind":"summary","title":format!("{name} summary"),"summary":"Inspect the exact candidate.","reason":"Keep a compact sourced observation.","path":format!("derived/entities/{name}.md"),"content":format!("# {name}\n\nA source-backed observation.[^s1]\n"),"expected_version":0,"sources":[{"entry_ref":source["entry_ref"],"version":source["version"],"start_line":3,"end_line":3}]})
}
async fn submit(f: &Fixture, a: &Value, version: i64, items: Vec<Value>) -> (Value, Value) {
    let mut body = attempt(a, version);
    body["candidates"] = json!(items);
    body["processed_inputs"] = a["inputs"].clone();
    body["findings"] = json!(["Fixture sources processed."]);
    let result = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        body.clone(),
    )
    .await);
    (body, result)
}
async fn finish(f: &Fixture, a: &Value, version: i64, outcome: &str) -> (Value, Value) {
    let mut body = attempt(a, version);
    body["outcome"] = json!(outcome);
    body["detail"] = json!("Fixture terminal result.");
    body["auth_persistence"] = json!({"status":"persisted"});
    body["notification"] = json!({"status":"not_needed"});
    let result = ok(post(f, &f.runner, "/v1/workspace/dreamer/finish", body.clone()).await);
    (body, result)
}
async fn review(f: &Fixture) -> Value {
    let envelope = ok(request(&f.app, &f.owner, Method::GET, "/v1/dreamer/review", None).await);
    assert_eq!(
        envelope["status"], "complete",
        "browser requires a complete workspace envelope"
    );
    envelope["data"].clone()
}
fn decision(view: &Value, item: &Value, choice: &str) -> Value {
    json!({"item_id":item["id"],"decision":choice,"expected_decisions_version":view["decision_version"],"candidate_hash":item["candidate_hash"],"run_entry_ref":item["run_entry_ref"],"run_version":item["run_version"],"idempotency_key":format!("decision-fixture:{}",Uuid::now_v7()),"correction":if choice=="correct"{"Please preserve the original qualification."}else{""}})
}
async fn current(f: &Fixture, path: &str) -> Option<(i64, String, Value)> {
    sqlx::query("SELECT e.current_version,v.content,v.metadata FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.path=$2 AND e.deleted_at IS NULL")
        .bind(f.owner.user).bind(path).fetch_optional(&f.pool).await.unwrap().map(|r|(r.get("current_version"),r.get("content"),r.get("metadata")))
}

#[tokio::test]
async fn narrative_context_is_scoped_frozen_replayable_and_does_not_consume_inputs() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let canonical = write(
        &f,
        "sources/Projects/Orchid/Orchid.md",
        "# Orchid\n\nOrchid is a garden planning project.\n",
        0,
    )
    .await;
    let changed = write(
        &f,
        "sources/Projects/Orchid/Decision.md",
        "# Orchid decision\n\nEarlier decision.\n",
        0,
    )
    .await;
    let unrequested = write(
        &f,
        "sources/Projects/Unrelated/Note.md",
        "# Other\n\nUnrelated source.\n",
        0,
    )
    .await;
    write(
        &f,
        "sources/Orchid/credentials.md",
        "# Orchid\n\nSENSITIVE_BODY_CANARY\n",
        0,
    )
    .await;
    ok(post(&f,&f.owner,"/v1/workspace/write",json!({"path":"sources/Orchid/Evaluation.md","content":"# Orchid\n\nGENERATED_ANSWER_CANARY\n","expected_version":0,"metadata":{"evaluation_output":true}})).await);
    let first = admit(&f).await;
    let (_, disposed) = submit(&f, &first, first["state_version"].as_i64().unwrap(), vec![]).await;
    finish(
        &f,
        &first,
        disposed["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let seed = write(
        &f,
        "sources/Projects/Orchid/Inbox.md",
        "# Orchid update\n\nThe next review needs the canonical project context.\n",
        0,
    )
    .await;
    let a = admit(&f).await;
    assert_eq!(a["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(a["inputs"][0]["entry_ref"], seed["entry_ref"]);
    write(
        &f,
        "sources/Projects/Orchid/Decision.md",
        "# Orchid decision\n\nAFTER_FENCE_CANARY\n",
        1,
    )
    .await;
    let future = write(
        &f,
        "sources/Projects/Orchid/Future.md",
        "# Orchid\n\nNEW_AFTER_FENCE_CANARY\n",
        0,
    )
    .await;
    let other = actor(&f.pool, None, OWNER_CAPS).await;
    ok(post(&f,&other,"/v1/workspace/write",json!({"path":"sources/Orchid.md","content":"# Orchid\n\nOTHER_OWNER_CANARY\n","expected_version":0,"metadata":{}})).await);
    let mut request = attempt(&a, a["state_version"].as_i64().unwrap());
    request["queries"] = json!(["Orchid"]);
    assert_eq!(
        post(
            &f,
            &f.model,
            "/v1/workspace/dreamer/narrative-discover",
            request.clone()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let mut wrong = request.clone();
    wrong["fence"] = json!(Uuid::now_v7());
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            wrong
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let discovered = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        request.clone(),
    )
    .await)["data"]
        .clone();
    assert_eq!(discovered["inputs"], a["inputs"]);
    assert_eq!(discovered["scanned_generation"], a["scanned_generation"]);
    assert_eq!(
        discovered["processed_generation"],
        a["processed_generation"]
    );
    let context = discovered["narrative_context"].as_array().unwrap();
    assert!(
        context
            .iter()
            .any(|source| source["entry_ref"] == canonical["entry_ref"] && source["version"] == 1),
        "{discovered}"
    );
    assert!(
        !context
            .iter()
            .any(|source| source["entry_ref"] == changed["entry_ref"]
                || source["entry_ref"] == future["entry_ref"])
    );
    for source in context {
        assert!(source["generation"].as_i64().unwrap() <= a["frozen_generation"].as_i64().unwrap());
    }
    let visible = discovered.to_string();
    for canary in [
        "SENSITIVE_BODY_CANARY",
        "GENERATED_ANSWER_CANARY",
        "AFTER_FENCE_CANARY",
        "OTHER_OWNER_CANARY",
        "credentials.md",
        "Evaluation.md",
    ] {
        assert!(
            !serde_json::to_string(context).unwrap().contains(canary),
            "{canary} entered context"
        );
    }
    assert!(!visible.contains("NEW_AFTER_FENCE_CANARY"));
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/narrative-discover",
        request.clone(),
    )
    .await);
    assert_eq!(replay["no_op"], true);
    assert_eq!(replay["data"]["state_version"], discovered["state_version"]);
    assert_eq!(
        replay["data"]["narrative_context"],
        discovered["narrative_context"]
    );
    let version = discovered["state_version"].as_i64().unwrap();
    let mut broaden = attempt(&a, version);
    broaden["queries"] = json!(["Unrelated"]);
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/narrative-discover",
            broaden
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let mut invalid = attempt(&a, version);
    invalid["candidates"] = json!([candidate(&unrequested, "unadmitted")]);
    invalid["processed_inputs"] = json!([]);
    invalid["findings"] = json!([]);
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/candidates",
            invalid.clone()
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    invalid["candidates"] = json!([]);
    let source = context
        .iter()
        .find(|s| s["entry_ref"] == canonical["entry_ref"])
        .unwrap();
    invalid["processed_inputs"] = json!([{"entry_ref":source["entry_ref"],"version":source["version"],"generation":source["generation"]}]);
    invalid["findings"] = json!(["Discovery is not input processing."]);
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/candidates", invalid)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    let mut item = candidate(&canonical, "orchid");
    item["content"] = json!("# Orchid\n\nOrchid is a garden planning project.[^s1]\n");
    let (_, accepted) = submit(&f, &discovered, version, vec![item]).await;
    assert_eq!(
        accepted["accepted_candidate_ids"].as_array().unwrap().len(),
        1
    );
    // New relevant notes after the frozen boundary already make the preview stale.
    let view = review(&f).await;
    assert_eq!(view["items"][0]["stale"], true);
    write(
        &f,
        "sources/Projects/Orchid/Orchid.md",
        "# Orchid\n\nOrchid has been retired.\n",
        1,
    )
    .await;
    let view = review(&f).await;
    assert_eq!(view["items"][0]["stale"], true);
    let approval = decision(&view, &view["items"][0], "approve");
    assert_eq!(
        post(&f, &f.owner, "/v1/dreamer/review/decisions", approval)
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert!(current(&f, "derived/entities/orchid.md").await.is_none());
}

#[tokio::test]
async fn entity_revisions_preserve_identity_and_cannot_duplicate_held_views() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Projects/Orchid.md",
        "# Orchid\n\nCurrent garden plan.\n",
        0,
    )
    .await;
    let a = admit(&f).await;
    let item = candidate(&source, "orchid");
    let (_, receipt) = submit(
        &f,
        &a,
        a["state_version"].as_i64().unwrap(),
        vec![item.clone()],
    )
    .await;
    let id = receipt["accepted_candidate_ids"][0].clone();
    let mut duplicate = attempt(&a, receipt["state_version"].as_i64().unwrap());
    let mut revised = item.clone();
    revised["title"] = json!("A clearer Orchid view");
    duplicate["candidates"] = json!([revised.clone()]);
    duplicate["processed_inputs"] = json!([]);
    duplicate["findings"] = json!([]);
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/candidates",
            duplicate.clone()
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    revised["revises_item_id"] = id.clone();
    let mut wrong = revised.clone();
    wrong["path"] = json!("derived/entities/someone-else.md");
    duplicate["candidates"] = json!([wrong]);
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/candidates",
            duplicate.clone()
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let mut competing = revised.clone();
    competing["title"] = json!("A competing view");
    duplicate["candidates"] = json!([revised.clone(), competing]);
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/candidates",
            duplicate.clone()
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    duplicate["candidates"] = json!([revised]);
    let changed = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        duplicate.clone(),
    )
    .await);
    assert_eq!(changed["accepted_candidate_ids"], json!([id]));
    let view = review(&f).await;
    assert_eq!(view["items"].as_array().unwrap().len(), 1);
    let approval = decision(&view, &view["items"][0], "approve");
    let held =
        ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", approval).await)["data"].clone();
    duplicate["expected_state_version"] = held["state_version"].clone();
    duplicate["candidates"][0]["title"] = json!("Do not replace an approval");
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/candidates", duplicate)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    let view = review(&f).await;
    assert_eq!(view["items"][0]["status"], "approved_held");
    assert!(current(&f, "derived/entities/orchid.md").await.is_none());
}

#[tokio::test]
async fn corrected_entity_replaces_current_facts_and_preserves_immutable_history() {
    let Some(f) = fixture().await else { return };
    control(&f, "full", 0).await;
    let path = "sources/Projects/Orchid/Plan.md";
    let original = write(&f, path, "# Orchid\n\nThe launch is June 10.\n", 0).await;
    let a = admit(&f).await;
    let mut first = candidate(&original, "orchid-current");
    first["content"] = json!("# Orchid\n\nThe planned launch is June 10.[^s1]\n");
    let (_, accepted) = submit(&f, &a, a["state_version"].as_i64().unwrap(), vec![first]).await;
    finish(
        &f,
        &a,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(&f).await;
    ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &view["items"][0], "approve"),
    )
    .await);
    let target = "derived/entities/orchid-current.md";
    let (version, old_body, _) = current(&f, target).await.unwrap();
    assert_eq!(version, 1);
    let read = json!({"requests":[{"path":target,"view":"current_state","max_chars":12000}]});
    let initial = ok(post(&f, &f.owner, "/v1/workspace/read", read.clone()).await);
    assert_eq!(initial["data"]["items"][0]["freshness"]["status"], "fresh");
    let correction = write(
        &f,
        path,
        "# Orchid\n\nCorrection: the launch is June 17, replacing the June 10 plan.\n",
        1,
    )
    .await;
    let stale = ok(post(&f, &f.owner, "/v1/workspace/read", read.clone()).await);
    assert_ne!(stale["data"]["items"][0]["freshness"]["status"], "fresh");
    assert!(
        !stale["data"]["items"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("The planned launch is June 10.")
    );
    let next = admit(&f).await;
    let mut updated = candidate(&correction, "orchid-current");
    updated["expected_version"] = json!(1);
    updated["content"] = json!(
        "# Orchid\n\nCurrent launch: June 17.[^s1]\n\nThe June 10 plan was replaced by the explicit correction.[^s1]\n"
    );
    let (_, accepted) = submit(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        vec![updated],
    )
    .await;
    finish(
        &f,
        &next,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(&f).await;
    let item = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["candidate"]["target_path"] == target)
        .unwrap();
    ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, item, "approve"),
    )
    .await);
    let (version, body, metadata) = current(&f, target).await.unwrap();
    assert_eq!(version, 2);
    assert!(body.contains("Current launch: June 17."));
    assert_eq!(metadata["dreamer_summary"]["sources"][0]["version"], 2);
    let fresh = ok(post(&f, &f.owner, "/v1/workspace/read", read).await);
    assert_eq!(fresh["data"]["items"][0]["freshness"]["status"], "fresh");
    assert!(
        fresh["data"]["items"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Current launch: June 17.")
    );
    let saved_old:String=sqlx::query_scalar("SELECT v.content FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id WHERE e.user_id=$1 AND e.path=$2 AND v.version=1")
        .bind(f.owner.user).bind(target).fetch_one(&f.pool).await.unwrap();
    assert_eq!(saved_old, old_body);
}

// Historical runs predate the server-only Dreamer writer. Seed those versions
// only for this isolated fixture user, without weakening the live write guard.
async fn historical_run(f: &Fixture, path: &str, content: &str, version: i64) -> Value {
    let staging = format!(
        "sources/LegacyFixture/{}",
        path.strip_prefix("dreams/runs/").unwrap()
    );
    if version > 0 {
        sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND path=$2")
            .bind(f.owner.user)
            .bind(path)
            .bind(&staging)
            .execute(&f.pool)
            .await
            .unwrap();
    }
    let result = write(f, &staging, content, version).await;
    sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND path=$2")
        .bind(f.owner.user)
        .bind(&staging)
        .bind(path)
        .execute(&f.pool)
        .await
        .unwrap();
    result
}

#[tokio::test]
async fn ordinary_summary_tables_retain_uncited_work_and_accept_cited_rows() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Experiments/Notes.md",
        "# Experiments\n\nThe first trial used oak and the second used birch.\n",
        0,
    )
    .await;
    let admission = admit(&f).await;
    let mut valid = candidate(&source, "experiment-comparison");
    valid["content"] = json!(
        "# Recorded experiments\n\n| Element | First trial | Second trial |\n| --- | :---: | ---: |\n| Material | Oak.[^s1] | Birch.[^s1] |\n"
    );
    let mut invalid = valid.clone();
    invalid["content"] = json!(valid["content"].as_str().unwrap().replace("[^s1]", ""));
    let before = current(&f, "dreams/state.md").await.unwrap();
    let mut body = attempt(&admission, admission["state_version"].as_i64().unwrap());
    body["candidates"] = json!([invalid]);
    body["processed_inputs"] = admission["inputs"].clone();
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert!(rejected.body.to_string().contains("needs a declared"));
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
    let (_, accepted) = submit(
        &f,
        &admission,
        admission["state_version"].as_i64().unwrap(),
        vec![valid.clone()],
    )
    .await;
    finish(
        &f,
        &admission,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let result = review(&f).await;
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["reviewable"], true);
    assert!(
        result["items"][0]["candidate"]["after_md"]
            .as_str()
            .unwrap()
            .contains("| Material | Oak.[^s1] | Birch.[^s1] |")
    );
    assert!(current(&f, valid["path"].as_str().unwrap()).await.is_none());
}

#[tokio::test]
async fn coherent_large_source_fits_candidate_but_hydrated_total_stays_bounded() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let content = format!("# Measurements\n\n{}\n", "Recorded data. ".repeat(1100));
    let source = write(&f, "sources/Measurements.md", &content, 0).await;
    let admission = admit(&f).await;
    let valid = candidate(&source, "measurements");
    let mut oversized = valid.clone();
    oversized["content"] = json!("Evidence.[^s1]\n".repeat(1400));
    let before = current(&f, "dreams/state.md").await.unwrap();
    let mut body = attempt(&admission, admission["state_version"].as_i64().unwrap());
    body["candidates"] = json!([oversized]);
    body["processed_inputs"] = admission["inputs"].clone();
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert!(rejected.body.to_string().contains("32 KiB"));
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
    let (_, accepted) = submit(
        &f,
        &admission,
        admission["state_version"].as_i64().unwrap(),
        vec![valid],
    )
    .await;
    finish(
        &f,
        &admission,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let result = review(&f).await;
    assert_eq!(result["items"][0]["reviewable"], true);
    assert_eq!(
        result["items"][0]["sources"][0]["excerpt"],
        content.lines().nth(2).unwrap()
    );
}

async fn expire_fixture_lease(f: &Fixture) {
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=jsonb_set(metadata,'{dreamer_state,active,lease_until}',to_jsonb($2::text)) FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind((Utc::now()-chrono::Duration::seconds(1)).to_rfc3339()).execute(&f.pool).await.unwrap();
}

async fn cleanup_scale_fixture(pool: &PgPool, user: Uuid) {
    let fixture_owner: bool =
        sqlx::query_scalar("SELECT external_ref=$2 FROM brunn.users WHERE id=$1")
            .bind(user)
            .bind(format!("dreamer-api-fixture:{user}"))
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        fixture_owner,
        "refuse cleanup outside the exact disposable fixture owner"
    );
    let unexpected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM brunn.entries WHERE user_id=$1 AND NOT starts_with(path,'sources/Scale/') AND path NOT IN ('dreams/CONTROL.md','dreams/state.md','dreams/latest-receipt.md') AND NOT starts_with(path,'dreams/runs/'))")
        .bind(user).fetch_one(pool).await.unwrap();
    assert!(
        !unexpected,
        "refuse cleanup when the scale owner has unrelated entries"
    );
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Credential activity may flush after the request returns. Keep the tiny
    // disposable principal; remove only this run's workspace fixture data.
    sqlx::query("DELETE FROM brunn.search_chunks WHERE user_id=$1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    let changes = sqlx::query("DELETE FROM brunn.workspace_changes WHERE user_id=$1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    let versions = sqlx::query("DELETE FROM brunn.entry_versions WHERE user_id=$1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    let entries = sqlx::query("DELETE FROM brunn.entries WHERE user_id=$1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    tx.commit().await.unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM brunn.entries WHERE user_id=$1)+(SELECT count(*) FROM brunn.entry_versions WHERE user_id=$1)+(SELECT count(*) FROM brunn.workspace_changes WHERE user_id=$1)")
        .bind(user).fetch_one(pool).await.unwrap();
    assert_eq!(remaining, 0);
    eprintln!(
        "Dreamer scale cleanup: owner={user}, entries={entries}, versions={versions}, changes={changes}, remaining=0"
    );
}

#[tokio::test]
async fn owner_scale_admission_freezes_latest_versions_and_retains_only_128_inputs() {
    let Some(f) = fixture_with_deadline(Some(std::time::Duration::from_secs(30))).await else {
        return;
    };
    // Catch assertion panics so both a deadline regression and a successful
    // run remove their large fixture before preserving the original result.
    let outcome = std::panic::AssertUnwindSafe(async {
    control(&f, "report-only", 0).await;
    // A fresh database has statistics from before its first large import.
    // Exercise that cold estimate explicitly; a prior fixture's analyze must
    // not make this deadline regression disappear on repeated local runs.
    for statement in ["ANALYZE brunn.entries", "ANALYZE brunn.entry_versions", "ANALYZE brunn.workspace_changes"] {
        sqlx::query(statement).execute(&f.pool).await.unwrap();
    }
    const ENTRY_COUNT: i64 = 21_000;
    const UPDATED_COUNT: i64 = 15_000;
    let original = "# Scale source\n\nOriginal immutable observation.\n";
    let updated = "# Scale source\n\nLatest observation before the frozen boundary.\n";
    let original_hash = hash_token(original);
    let updated_hash = hash_token(updated);
    let seed_started = std::time::Instant::now();
    // Seed only this disposable owner. Keep the real FK constraints and
    // workspace commit-order trigger; no index or planner shortcuts. A large
    // history with few versions per entry matches the production lookup shape.
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO brunn.entries(user_id,path,title,kind,media_type,current_version) SELECT $1,'sources/Scale/'||lpad(n::text,6,'0')||'.md','Scale source','markdown','text/markdown',1 FROM generate_series(1,$2::bigint) AS n")
        .bind(f.owner.user).bind(ENTRY_COUNT).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions(user_id,entry_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) SELECT user_id,id,1,$2,$3,$4,'{}'::jsonb,$5 FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Scale/')")
        .bind(f.owner.user).bind(&original_hash).bind(original).bind(original.len() as i64).bind(f.owner.id).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT user_id,id,1,'create',path,$2 FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Scale/') ORDER BY path")
        .bind(f.owner.user).bind(&original_hash).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions(user_id,entry_id,version,content_sha256,content,size_bytes,metadata,created_by_credential_id) SELECT user_id,id,2,$2,$3,$4,'{}'::jsonb,$5 FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Scale/') AND substring(path from '([0-9]+)[.]md$')::bigint<=$6")
        .bind(f.owner.user).bind(&updated_hash).bind(updated).bind(updated.len() as i64).bind(f.owner.id).bind(UPDATED_COUNT).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE brunn.entries SET current_version=2 WHERE user_id=$1 AND starts_with(path,'sources/Scale/') AND substring(path from '([0-9]+)[.]md$')::bigint<=$2")
        .bind(f.owner.user).bind(UPDATED_COUNT).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT user_id,id,2,'update',path,$2 FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Scale/') AND current_version=2 ORDER BY path")
        .bind(f.owner.user).bind(&updated_hash).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let seed_ms = seed_started.elapsed().as_millis();
    let source_entries: i64 = sqlx::query_scalar("SELECT count(*) FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'sources/Scale/')")
        .bind(f.owner.user).fetch_one(&f.pool).await.unwrap();
    let source_changes: i64 = sqlx::query_scalar("SELECT count(*) FROM brunn.workspace_changes WHERE user_id=$1 AND starts_with(path,'sources/Scale/')")
        .bind(f.owner.user).fetch_one(&f.pool).await.unwrap();
    assert_eq!(source_entries, ENTRY_COUNT);
    assert_eq!(source_changes, ENTRY_COUNT + UPDATED_COUNT);
    let frozen: i64 =
        sqlx::query_scalar("SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1")
            .bind(f.owner.user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let expected = sqlx::query("SELECT entry_id,generation,path FROM brunn.workspace_changes WHERE user_id=$1 AND operation='create' AND starts_with(path,'sources/Scale/') ORDER BY generation LIMIT 129")
        .bind(f.owner.user).fetch_all(&f.pool).await.unwrap();
    assert_eq!(expected.len(), 129);
    let body =
        json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60});
    let started = std::time::Instant::now();
    let response = post(&f, &f.runner, "/v1/workspace/dreamer/admit", body.clone()).await;
    let elapsed = started.elapsed();
    eprintln!(
        "Dreamer scale admission: entries={source_entries}, source_changes={source_changes}, seed_ms={seed_ms}, admission_ms={}, status={}, request_deadline_ms=30000, sql_deadline_ms=25000",
        elapsed.as_millis(),
        response.status.as_u16()
    );
    assert_eq!(
        response.status,
        StatusCode::OK,
        "owner-scale admission must complete inside the normal production SQL deadline: {}",
        response.body
    );
    assert!(
        elapsed < std::time::Duration::from_secs(25),
        "scale admission exhausted the normal SQL deadline"
    );
    let admitted = response.body;
    assert_eq!(admitted["admitted"], true);
    assert_eq!(admitted["frozen_generation"], frozen);
    let inputs = admitted["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 128);
    for (actual, expected) in inputs.iter().zip(&expected[..128]) {
        assert_eq!(
            actual["entry_ref"],
            format!("entry:{}", expected.get::<Uuid, _>("entry_id"))
        );
        assert_eq!(actual["path"], expected.get::<String, _>("path"));
        assert_eq!(
            actual["version"], 2,
            "snapshot must select the later pre-fence version"
        );
        assert_eq!(actual["operation"], "update");
        assert_eq!(actual["content_hash"], format!("sha256:{updated_hash}"));
        assert_eq!(
            actual["generation"],
            expected.get::<i64, _>("generation"),
            "retain the original unprocessed position, not the newer snapshot generation"
        );
    }
    assert_eq!(
        admitted["scanned_generation"],
        expected[127].get::<i64, _>("generation")
    );
    assert!(
        admitted["scanned_generation"].as_i64().unwrap()
            < expected[128].get::<i64, _>("generation"),
        "do not consume the first source beyond retained capacity"
    );
    assert_eq!(
        admitted["processed_generation"],
        expected[0].get::<i64, _>("generation") - 1
    );
    let written = write(
        &f,
        expected[0].get::<&str, _>("path"),
        "# Scale source\n\nCommitted after admission.\n",
        2,
    )
    .await;
    assert_eq!(written["version"], 3);
    let replay_started = std::time::Instant::now();
    let replay = ok(post(&f, &f.runner, "/v1/workspace/dreamer/admit", body).await);
    eprintln!(
        "Dreamer scale frozen replay: replay_ms={}, inputs={}",
        replay_started.elapsed().as_millis(),
        replay["inputs"].as_array().unwrap().len()
    );
    assert_eq!(replay["fence"], admitted["fence"]);
    assert_eq!(replay["frozen_generation"], frozen);
    assert_eq!(
        replay["inputs"], admitted["inputs"],
        "a later committed version cannot alter the accepted frozen input set"
    );
    finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let retained = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(retained.2["dreamer_state"]["inputs"], admitted["inputs"]);
    }).catch_unwind().await;
    cleanup_scale_fixture(&f.pool, f.owner.user).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn legacy_review_restores_full_exact_version_text_without_changing_decision_identity() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let title = format!(
        "Keep the complete proposal: {} END_OF_LONG_TITLE",
        "descriptive context ".repeat(15)
    );
    let body = format!(
        "{title}\n\n{}\n\n### Proposed details\n\n```markdown\n2. This is code, not a different proposal.\n## Needs your call\n```\n\nEND_OF_COMPLETE_PROPOSAL",
        "Evidence and rationale. ".repeat(90)
    );
    let run = historical_run(&f, "dreams/runs/2020-01-02.md", &format!("# Legacy run\n\n## Proposed\n\n1. {body}\n\n## Needs your call\n\n1. Which source should win?\n\nKeep the qualification.\n\n## Applied\n\nNothing.\n"), 0).await;
    let admitted = admit(&f).await;
    finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let before = current(&f, "dreams/state.md").await.unwrap();
    let stored = before.2["dreamer_state"]["legacy_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "2020-01-02/1")
        .unwrap()
        .clone();
    assert!(stored["candidate"]["title"].as_str().unwrap().len() < title.len());
    assert!(stored["candidate"]["summary"].as_str().unwrap().len() < body.len());
    // Newer source text must never replace the exact version imported earlier.
    historical_run(
        &f,
        "dreams/runs/2020-01-02.md",
        "# Rewritten run\n\n## Proposed\n\n1. UNREVIEWED_NEW_HEAD\n",
        1,
    )
    .await;
    let view = review(&f).await;
    assert_eq!(view["items"], json!([]));
    assert_eq!(view["counts"]["pending"], 0);
    let restored = view["legacy_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == stored["id"])
        .unwrap();
    assert_eq!(restored["title"], title);
    assert_eq!(restored["body_md"], body);
    assert_eq!(restored["candidate"], Value::Null);
    assert_eq!(restored["sources"][0]["entry_ref"], run["entry_ref"]);
    assert_eq!(restored["sources"][0]["version"], 1);
    assert_eq!(restored["reviewable"], false);
    assert_eq!(restored["legacy"], true);
    for key in ["id", "candidate_hash", "run_entry_ref", "run_version"] {
        assert_eq!(restored[key], stored[key], "projection cannot change {key}");
    }
    assert!(!view.to_string().contains("UNREVIEWED_NEW_HEAD"));
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2,
        before.2,
        "GET cannot mutate stored proposal or decision state"
    );
    let reader = ok(request(&f.app, &f.model, Method::GET, "/v1/dreamer/review", None).await);
    assert!(reader.to_string().contains("END_OF_COMPLETE_PROPOSAL"));
    assert_eq!(
        post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, restored, "approve")
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    for choice in ["reject", "defer", "correct"] {
        assert_eq!(
            post(
                &f,
                &f.owner,
                "/v1/dreamer/review/decisions",
                decision(&view, restored, choice),
            )
            .await
            .status,
            StatusCode::CONFLICT
        );
    }
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
    assert!(current(&f, "dreams/decisions.md").await.is_none());
}

#[tokio::test]
async fn unavailable_legacy_run_withholds_cached_text_and_foreign_versions() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let run = historical_run(&f, "dreams/runs/2020-01-03.md", "## Proposed\n\n1. PRIVATE_LEGACY_TEXT\n\n## Needs your call\n\n1. PRIVATE_LEGACY_QUESTION\n", 0).await;
    let admitted = admit(&f).await;
    finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    assert!(review(&f).await.to_string().contains("PRIVATE_LEGACY_TEXT"));
    let state_read = json!({"requests":[{"path":"dreams/state.md"}]});
    let visible_state = ok(post(&f, &f.model, "/v1/workspace/read", state_read.clone()).await);
    assert!(visible_state.to_string().contains("PRIVATE_LEGACY_TEXT"));
    let reference = run["entry_ref"].as_str().unwrap();
    // Reserved run paths cannot be deleted through ordinary workspace writes;
    // emulate historical retention/removal directly for this fixture only.
    sqlx::query("UPDATE brunn.entries SET deleted_at=now() WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(Uuid::parse_str(reference.strip_prefix("entry:").unwrap()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let deleted = review(&f).await;
    assert!(!deleted.to_string().contains("PRIVATE_LEGACY_"));
    let deleted_state = ok(post(&f, &f.model, "/v1/workspace/read", state_read.clone()).await);
    assert!(!deleted_state.to_string().contains("PRIVATE_LEGACY_"));
    assert_eq!(
        deleted_state["data"]["items"][0]["representation"],
        "audit_withheld"
    );
    assert!(
        deleted["legacy_items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["stale"] == true && i["reviewable"] == false)
    );
    let foreign = actor(&f.pool, None, OWNER_CAPS).await;
    let other = ok(post(&f, &foreign, "/v1/workspace/write", json!({"path":"sources/ForeignLegacy.md","content":"## Proposed\n\n1. FOREIGN_LEGACY_SECRET\n","expected_version":0})).await)["data"].clone();
    sqlx::query("UPDATE brunn.entries SET path='dreams/runs/2020-01-03.md' WHERE user_id=$1 AND path='sources/ForeignLegacy.md'")
        .bind(foreign.user).execute(&f.pool).await.unwrap();
    // Even a corrupt retained reference cannot expose another owner's version.
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=jsonb_set(v.metadata,'{dreamer_state,legacy_items,0,run_entry_ref}',$2::jsonb) FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(other["entry_ref"].clone()).execute(&f.pool).await.unwrap();
    let unavailable = review(&f).await;
    assert!(!unavailable.to_string().contains("FOREIGN_LEGACY_SECRET"));
    assert!(!unavailable.to_string().contains("PRIVATE_LEGACY_"));
    let foreign_state = ok(post(&f, &f.model, "/v1/workspace/read", state_read).await);
    assert!(!foreign_state.to_string().contains("FOREIGN_LEGACY_SECRET"));
    assert!(!foreign_state.to_string().contains("PRIVATE_LEGACY_"));
}

#[tokio::test]
async fn legacy_history_capacity_does_not_starve_candidates_or_current_questions() {
    for size in [64usize, 96] {
        let Some(f) = fixture().await else {
            return;
        };
        control(&f, "report-only", 0).await;
        let mut old = "## Proposed\n\n".to_owned();
        for number in 1..size {
            old.push_str(&format!("{number}. HISTORICAL_PROMISE_{number}\n\n"));
        }
        old.push_str("## Needs your call\n\n1. HISTORICAL_QUESTION\n");
        if size == 96 {
            old.push_str("\n2. HISTORICAL_OVERFLOW_REMAINS_IN_ORIGINAL_REPORT\n");
        }
        historical_run(&f, "dreams/runs/2020-01-04.md", &old, 0).await;
        let source = write(
            &f,
            "sources/CurrentWork.md",
            "# Current work\n\nA source-backed observation.\n",
            0,
        )
        .await;
        let admitted = admit(&f).await;
        assert_eq!(admitted["pending"], json!([]));
        assert!(!admitted.to_string().contains("HISTORICAL_"));
        assert!(
            admitted["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|input| input["entry_ref"] == source["entry_ref"])
        );
        let before = current(&f, "dreams/state.md").await.unwrap();
        assert_eq!(
            before.2["dreamer_state"]["legacy_items"]
                .as_array()
                .unwrap()
                .len(),
            size
        );
        assert_eq!(before.2["dreamer_state"]["items"], json!([]));
        let question = json!({"kind":"question","title":"Which current source should win?","question":"Resolve the current source conflict.","sources":[{"entry_ref":source["entry_ref"],"version":source["version"],"start_line":3,"end_line":3}]});
        let mut body = attempt(&admitted, admitted["state_version"].as_i64().unwrap());
        body["candidates"] = json!([candidate(&source, "current-work"), question]);
        body["processed_inputs"] = json!([]);
        let submitted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await);
        assert_eq!(
            submitted["accepted_candidate_ids"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "full history must not consume active slots"
        );
        let (_, finished) = finish(
            &f,
            &admitted,
            submitted["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        assert_eq!(finished["counts"]["pending"], 2);
        assert_eq!(finished["counts"]["legacy"], size);
        let view = review(&f).await;
        assert_eq!(view["items"].as_array().unwrap().len(), 2);
        assert_eq!(view["counts"]["questions"], 1);
        assert_eq!(view["counts"]["proposals"], 1);
        assert_eq!(view["legacy_items"].as_array().unwrap().len(), size);
        assert_eq!(view["history"], json!([]));
        assert!(
            view["legacy_items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["legacy"] == true
                    && item["candidate"].is_null()
                    && item["reviewable"] == false)
        );
        let current_question = view["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["kind"] == "question")
            .unwrap();
        assert_eq!(current_question["legacy"], false);
        assert_eq!(
            current_question["body_md"],
            "Resolve the current source conflict."
        );
        assert_eq!(current_question["blocked_reason"], Value::Null);
        let after = current(&f, "dreams/state.md").await.unwrap();
        assert_eq!(
            after.2["dreamer_state"]["inputs"], before.2["dreamer_state"]["inputs"],
            "archiving history cannot discard unprocessed source work"
        );
        assert_eq!(
            after.2["dreamer_state"]["legacy_items"],
            before.2["dreamer_state"]["legacy_items"]
        );
        let audit = current(&f, &format!("dreams/runs/{}.md", date()))
            .await
            .unwrap();
        let receipt = current(&f, "dreams/latest-receipt.md").await.unwrap();
        assert!(!audit.1.contains("HISTORICAL_"));
        assert!(!audit.2.to_string().contains("2020-01-04/"));
        assert!(!receipt.1.contains("2020-01-04/"));
        let successor = admit(&f).await;
        assert_eq!(successor["pending"].as_array().unwrap().len(), 2);
        assert!(!successor.to_string().contains("HISTORICAL_"));
        let (_, processed) = submit(
            &f,
            &successor,
            successor["state_version"].as_i64().unwrap(),
            vec![],
        )
        .await;
        let (_, completed) = finish(
            &f,
            &successor,
            processed["state_version"].as_i64().unwrap(),
            "completed",
        )
        .await;
        assert_eq!(
            completed["latest_receipt"]["status"], "completed",
            "historical overflow cannot make current work partial"
        );
        if size == 96 {
            let state = current(&f, "dreams/state.md").await.unwrap();
            assert_eq!(state.2["dreamer_state"]["legacy_complete"], false);
            assert_eq!(
                state.2["dreamer_state"]["legacy_scan_after"],
                before.2["dreamer_state"]["legacy_scan_after"]
            );
            assert!(
                current(&f, "dreams/runs/2020-01-04.md")
                    .await
                    .unwrap()
                    .1
                    .contains("HISTORICAL_OVERFLOW_REMAINS_IN_ORIGINAL_REPORT")
            );
        }
    }
}

#[tokio::test]
async fn old_state_history_migrates_without_get_writes_and_keeps_exact_decision_replay() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    historical_run(&f, "dreams/runs/2020-01-05.md", "## Proposed\n\n1. Original historical proposal\n\n## Needs your call\n\n1. Original historical question\n", 0).await;
    write(
        &f,
        "sources/Unprocessed.md",
        "# Source\n\nRetained source work.\n",
        0,
    )
    .await;
    let admitted = admit(&f).await;
    finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let initial = current(&f, "dreams/state.md").await.unwrap();
    let view = review(&f).await;
    let mut replay = decision(&view, &view["legacy_items"][0], "defer");
    replay["expected_decisions_version"] = json!(1);
    let recorded = json!({"id":"decision:historical-fixture","item_id":replay["item_id"],"idempotency_key":replay["idempotency_key"],"request_hash":hex::encode(Sha256::digest(serde_json::to_vec(&replay).unwrap())),"decision":"defer","application_status":"deferred","candidate_hash":replay["candidate_hash"],"run_entry_ref":replay["run_entry_ref"],"run_version":replay["run_version"]});
    let mut old_metadata = initial.2;
    let data = old_metadata["dreamer_state"].as_object_mut().unwrap();
    let mut old_items = data.remove("legacy_items").unwrap();
    old_items[0]["status"] = json!("deferred");
    data.insert("items".into(), old_items);
    data.insert("history".into(), json!([recorded]));
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=$2 FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(&old_metadata).execute(&f.pool).await.unwrap();
    let before = current(&f, "dreams/state.md").await.unwrap();
    let migrated_view = review(&f).await;
    assert_eq!(migrated_view["items"], json!([]));
    assert_eq!(migrated_view["counts"]["legacy"], 2);
    assert_eq!(migrated_view["counts"]["questions"], 0);
    for (original, shown) in old_metadata["dreamer_state"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .zip(migrated_view["legacy_items"].as_array().unwrap())
    {
        for field in [
            "id",
            "candidate_hash",
            "run_id",
            "run_entry_ref",
            "run_version",
            "status",
        ] {
            assert_eq!(original[field], shown[field], "migration preserves {field}");
        }
        assert!(shown["candidate"].is_null());
    }
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        before,
        "GET migration is a projection only"
    );
    let result = ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", replay.clone()).await);
    assert_eq!(result["data"]["application_status"], "deferred");
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap(),
        before,
        "replay cannot append a replacement decision"
    );
    let mut changed = replay;
    changed["decision"] = json!("reject");
    assert_eq!(
        post(&f, &f.owner, "/v1/dreamer/review/decisions", changed)
            .await
            .status,
        StatusCode::CONFLICT
    );
    let successor = admit(&f).await;
    assert_eq!(successor["pending"], json!([]));
    let persisted = current(&f, "dreams/state.md").await.unwrap();
    for key in [
        "inputs",
        "processed_generation",
        "processed_count",
        "next_item",
        "history",
        "legacy_scan_after",
        "legacy_complete",
    ] {
        assert_eq!(
            persisted.2["dreamer_state"][key], before.2["dreamer_state"][key],
            "migration preserves {key}"
        );
    }
    assert_eq!(
        persisted.2["dreamer_state"]["legacy_items"],
        before.2["dreamer_state"]["items"]
    );
    assert_eq!(persisted.2["dreamer_state"]["items"], json!([]));
    assert!(
        persisted.2["dreamer_state"]["scanned_generation"]
            .as_i64()
            .unwrap()
            >= before.2["dreamer_state"]["scanned_generation"]
                .as_i64()
                .unwrap(),
        "normal admission may scan subsequent operational writes"
    );
    finish(
        &f,
        &successor,
        successor["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn two_line_control_can_pause_resume_and_admit_without_calendar_eligibility() {
    let Some(f) = fixture().await else {
        return;
    };
    let policy = "enabled: true\nmode: report-only\n";
    write(&f, "dreams/CONTROL.md", policy, 0).await;
    let paused = ok(post(&f, &f.owner, "/v1/workspace/dreaming/pause", json!({})).await);
    assert_eq!(paused["control"]["enabled"], false);
    assert_eq!(
        current(&f, "dreams/CONTROL.md").await.unwrap().1,
        "enabled: false\nmode: report-only\n"
    );
    let resumed = ok(post(&f, &f.owner, "/v1/workspace/dreaming/resume", json!({})).await);
    assert_eq!(resumed["control"]["enabled"], true);
    assert_eq!(resumed["control"]["advance_after"], Value::Null);
    assert_eq!(current(&f, "dreams/CONTROL.md").await.unwrap().1, policy);
    let admitted = admit(&f).await;
    assert_eq!(admitted["admitted"], true);
    assert_eq!(admitted["mode"], "report-only");
}

#[tokio::test]
async fn real_runner_to_review_decisions_preserves_report_only_and_exact_idempotence() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Projects/Fixture.md",
        "# Fixture\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let forbidden =
        json!({"path":"sources/forbidden.md","content":"Must not write","expected_version":0});
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/write", forbidden.clone())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&f, &f.model, "/v1/workspace/write", forbidden)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/read",
            json!({"requests":[{"ref":source["entry_ref"]}]})
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let forged = post(&f, &f.owner, "/v1/workspace/write", json!({
        "path":format!("dreams/runs/{}.md",date()),"content":"Forged accepted run", "expected_version":0,
        "metadata":{"dreamer_run":{"schema":"dream.run.v1","accepted":true,"date":date(),"attempt_id":Uuid::now_v7(),"producer_credential_id":f.runner.id}}
    })).await;
    assert_eq!(forged.status, StatusCode::BAD_REQUEST, "{}", forged.body);
    let a = admit(&f).await;
    assert_eq!(a["admitted"], true);
    assert_eq!(a["inputs"].as_array().unwrap().len(), 1);
    let checkpoint = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/checkpoint",
        attempt(&a, a["state_version"].as_i64().unwrap()),
    )
    .await);
    let stale_checkpoint = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/checkpoint",
        attempt(&a, a["state_version"].as_i64().unwrap()),
    )
    .await;
    assert_eq!(stale_checkpoint.status, StatusCode::CONFLICT);
    let (submission, submitted) = submit(
        &f,
        &a,
        checkpoint["state_version"].as_i64().unwrap(),
        ["approve", "reject", "defer", "correct"]
            .iter()
            .map(|name| candidate(&source, name))
            .collect(),
    )
    .await;
    assert_eq!(
        submitted["accepted_candidate_ids"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/candidates",
            submission
        )
        .await),
        submitted,
        "same candidate submission replays exact immutable identities"
    );
    let (terminal, finished) = finish(
        &f,
        &a,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    assert_eq!(
        ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/finish",
            terminal.clone()
        )
        .await),
        finished
    );
    let mut altered = terminal;
    altered["detail"] = json!("Different terminal facts");
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/finish", altered)
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(finished["counts"]["processed"], 1);
    assert_eq!(finished["latest_receipt"]["applied_writes"], json!([]));
    let receipt_before =
        current(&f, "dreams/latest-receipt.md").await.unwrap().2["dreamer_receipt"].clone();
    let original = review(&f).await;
    assert_eq!(original["items"].as_array().unwrap().len(), 4);
    assert_eq!(
        original["items"][0]["sources"][0]["excerpt"],
        "A source-backed observation."
    );
    assert!(
        original["items"][0]["candidate"]["after_md"]
            .as_str()
            .unwrap()
            .contains("[^s1]: sources/Projects/Fixture.md")
    );
    for (name, expected) in [
        ("approve", "approved_held"),
        ("reject", "rejected"),
        ("defer", "deferred"),
        ("correct", "needs_changes"),
    ] {
        let view = review(&f).await;
        let item = view["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["title"] == format!("{name} summary"))
            .unwrap();
        let body = decision(&view, item, name);
        assert_eq!(
            post(&f, &f.model, "/v1/dreamer/review/decisions", body.clone())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        let result = ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", body.clone()).await);
        assert_eq!(result["data"]["application_status"], expected);
        assert_eq!(
            ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", body.clone()).await)["data"]["application_status"],
            expected
        );
        let mut conflicting = body;
        conflicting["comment"] = json!("Changed decision request");
        assert_eq!(
            post(&f, &f.owner, "/v1/dreamer/review/decisions", conflicting)
                .await
                .status,
            StatusCode::CONFLICT
        );
        assert!(
            current(&f, &format!("derived/entities/{name}.md"))
                .await
                .is_none()
        );
    }
    let view = review(&f).await;
    assert_eq!(view["history"].as_array().unwrap().len(), 4);
    assert_eq!(view["items"].as_array().unwrap().len(), 3);
    let receipt_after =
        current(&f, "dreams/latest-receipt.md").await.unwrap().2["dreamer_receipt"].clone();
    for field in [
        "run_id",
        "receipt_ref",
        "receipt_version",
        "completed_at",
        "status",
    ] {
        assert_eq!(
            receipt_after[field], receipt_before[field],
            "owner review preserves actual nightly {field}"
        );
    }
}

#[tokio::test]
async fn review_state_retry_preserves_owner_approval_and_attempt_fencing() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/ReviewRace.md",
        "# Fixture\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let first = admit(&f).await;
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "original")],
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let second = admit(&f).await;
    let view = review(&f).await;
    let original = view["items"][0].clone();
    ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &original, "approve"),
    )
    .await);

    let mut body = attempt(&second, second["state_version"].as_i64().unwrap());
    body["candidates"] = json!([candidate(&source, "independent")]);
    body["processed_inputs"] = json!([]);
    body["findings"] = json!([]);
    let stale = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        body.clone(),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(
        stale.body["error"]["message"],
        "Review or run state changed; reload before retrying"
    );
    body["expected_state_version"] = stale.body["error"]["details"]["actual_version"].clone();
    let mut unfenced = body.clone();
    unfenced["fence"] = json!(Uuid::now_v7().to_string());
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/candidates", unfenced)
            .await
            .status,
        StatusCode::CONFLICT
    );
    let accepted = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        body.clone(),
    )
    .await);
    let current_view = review(&f).await;
    let held = current_view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == original["id"])
        .unwrap();
    assert_eq!(held["status"], "approved_held");
    assert_eq!(held["candidate_hash"], original["candidate_hash"]);
    assert_eq!(current_view["items"].as_array().unwrap().len(), 2);

    body["expected_state_version"] = accepted["state_version"].clone();
    let mut replacement = candidate(&source, "original");
    replacement["revises_item_id"] = original["id"].clone();
    body["candidates"] = json!([replacement]);
    let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
    assert_eq!(
        rejected.status,
        StatusCode::BAD_REQUEST,
        "{}",
        rejected.body
    );
    assert!(current(&f, "derived/entities/original.md").await.is_none());
}

#[tokio::test]
async fn compact_related_intent_is_assembled_reviewed_and_published_without_rewriting_owner_text() {
    let Some(f) = fixture().await else { return };
    control(&f, "report-only", 0).await;
    let before = "# Original\n\nOwner text stays exactly as written.\n\n## Related\n\n- [[Previous]]\n\n## Notes\n\nMore owner text.\n";
    let source = write(&f, "sources/Original.md", before, 0).await;
    let target = write(
        &f,
        "sources/Target.md",
        "# Target\n\nA related source-backed observation.\n",
        0,
    )
    .await;
    let admission = admit(&f).await;
    let related = json!({"kind":"related","title":"Connect the notes","summary":"Add the evidence-backed project link.","reason":"Keep the source relationship easy to find.","path":"sources/Original.md","expected_version":source["version"],"content":"- [[sources/Target.md]]","sources":[
        {"entry_ref":source["entry_ref"],"version":source["version"],"start_line":1,"end_line":3},
        {"entry_ref":target["entry_ref"],"version":target["version"],"start_line":1,"end_line":3}
    ]});
    let question = json!({"kind":"question","title":"Confirm the relationship","summary":"One owner decision.","question":"Should these notes share a project?","sources":[{"entry_ref":target["entry_ref"],"version":target["version"],"start_line":1,"end_line":3}]});
    let (request, submitted) = submit(
        &f,
        &admission,
        admission["state_version"].as_i64().unwrap(),
        vec![related, question],
    )
    .await;
    assert_eq!(
        submitted["accepted_candidate_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", request).await),
        submitted
    );
    let (_, finished) = finish(
        &f,
        &admission,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    assert_eq!(finished["counts"]["processed"], 2);
    assert_eq!(current(&f, "sources/Original.md").await.unwrap().1, before);
    let view = review(&f).await;
    let item = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["candidate"]["target_path"] == "sources/Original.md")
        .unwrap();
    let after = before.replace("- [[Previous]]", "- [[sources/Target.md]]");
    assert_eq!(item["candidate"]["before_md"], before);
    assert_eq!(item["candidate"]["after_md"], after);
    control(&f, "full", 1).await;
    let approved = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, item, "approve"),
    )
    .await);
    assert_eq!(approved["data"]["application_status"], "applied");
    let published = current(&f, "sources/Original.md").await.unwrap();
    assert_eq!(published.0, 2);
    assert_eq!(published.1, after);
}

#[tokio::test]
async fn full_review_publishes_exact_candidate_and_rejects_foreign_or_stale_authority() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "full", 0).await;
    let source = write(
        &f,
        "sources/Projects/Fixture.md",
        "# Fixture\n\nA source-backed observation.\n",
        0,
    )
    .await;
    // Competing proposals for one destination are rejected at submission. An
    // owner edit to a Related destination exercises the independent output CAS.
    let target_path = "sources/Projects/Related target.md";
    let target_source = write(&f, target_path, "# Target\n\nOwner text.\n", 0).await;
    let a = admit(&f).await;
    let competing_target = json!({"kind":"related","title":"stale-target summary",
        "summary":"Connect the target to its source.","reason":"An evidenced relationship.",
        "path":target_path,"expected_version":target_source["version"],"content":"- [[sources/Projects/Fixture.md]]",
        "sources":[{"entry_ref":target_source["entry_ref"],"version":target_source["version"],"start_line":1,"end_line":3},
            {"entry_ref":source["entry_ref"],"version":source["version"],"start_line":1,"end_line":3}]});
    let (_, submitted) = submit(
        &f,
        &a,
        a["state_version"].as_i64().unwrap(),
        vec![
            candidate(&source, "publish"),
            candidate(&source, "stale-source"),
            competing_target,
        ],
    )
    .await;
    finish(
        &f,
        &a,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(&f).await;
    let item = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "publish summary")
        .unwrap();
    let approve = decision(&view, item, "approve");
    let foreign = actor(&f.pool, None, OWNER_CAPS).await;
    let mut foreign_request = approve.clone();
    foreign_request["expected_decisions_version"] = json!(0);
    assert_eq!(
        post(
            &f,
            &foreign,
            "/v1/dreamer/review/decisions",
            foreign_request
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let mut wrong_identity = approve.clone();
    wrong_identity["candidate_hash"] = json!("incorrect");
    assert_eq!(
        post(&f, &f.owner, "/v1/dreamer/review/decisions", wrong_identity)
            .await
            .status,
        StatusCode::CONFLICT
    );
    let applied = ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", approve).await);
    assert_eq!(applied["data"]["application_status"], "applied");
    let output = current(&f, "derived/entities/publish.md").await.unwrap();
    assert_eq!(output.0, 1);
    assert!(output.1.contains("A source-backed observation.[^s1]"));
    assert_eq!(output.2["dreamer_summary"]["state"], "published");
    let direct_edit=post(&f,&f.owner,"/v1/workspace/write",json!({"path":"derived/entities/publish.md","content":"# Unvalidated output","expected_version":1})).await;
    assert_eq!(
        direct_edit.status,
        StatusCode::BAD_REQUEST,
        "published summary paths require server validation"
    );
    write(&f, target_path, "# Target\n\nUpdated owner text.\n", 1).await;
    let view = review(&f).await;
    let target = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "stale-target summary")
        .unwrap();
    assert_eq!(target["stale"], true);
    assert_eq!(
        post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, target, "approve")
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    write(
        &f,
        "sources/Projects/Fixture.md",
        "# Fixture\n\nAn owner correction.\n",
        1,
    )
    .await;
    let view = review(&f).await;
    let stale = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "stale-source summary")
        .unwrap();
    assert_eq!(stale["stale"], true);
    assert_eq!(
        post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, stale, "approve")
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    assert!(
        current(&f, "derived/entities/stale-source.md")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn failed_attempts_retain_admitted_inputs_and_expired_fences_cannot_publish() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Projects/Fixture.md",
        "# Fixture\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let a = admit(&f).await;
    write(
        &f,
        "sources/Projects/Fixture.md",
        "# Fixture\n\nA corrected observation.\n",
        1,
    )
    .await;
    let mut body = attempt(&a, a["state_version"].as_i64().unwrap());
    body["candidates"] = json!([candidate(&source, "stale")]);
    body["processed_inputs"] = a["inputs"].clone();
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body)
            .await
            .status,
        StatusCode::CONFLICT
    );
    let (_, failed) = finish(&f, &a, a["state_version"].as_i64().unwrap(), "failed").await;
    assert_eq!(failed["counts"]["retained"], 1);
    assert_eq!(failed["counts"]["processed"], 0);
    let retry = admit(&f).await;
    assert_eq!(retry["inputs"][0]["version"], 2);
    assert_eq!(
        retry["inputs"][0]["generation"], a["inputs"][0]["generation"],
        "original pending generation is retained across newer source versions"
    );
    expire_fixture_lease(&f).await;
    let expired = attempt(&retry, retry["state_version"].as_i64().unwrap());
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/checkpoint",
            expired.clone()
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let successor = admit(&f).await;
    assert_ne!(successor["fence"], retry["fence"]);
    assert_eq!(successor["inputs"].as_array().unwrap().len(), 1);
    let mut invalid_finish = expired;
    invalid_finish["outcome"] = json!("completed");
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/finish",
            invalid_finish
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn held_approvals_apply_in_the_next_full_attempt_with_fresh_evidence() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let ready = write(
        &f,
        "sources/Ready/Ready.md",
        "# Ready\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let stale = write(
        &f,
        "sources/Stale/Stale.md",
        "# Stale\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let first = admit(&f).await;
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![
            candidate(&ready, "held-ready"),
            candidate(&stale, "held-stale"),
        ],
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    for name in ["held-ready", "held-stale"] {
        let view = review(&f).await;
        let item = view["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["title"] == format!("{name} summary"))
            .unwrap();
        let result = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, item, "approve"),
        )
        .await);
        assert_eq!(result["data"]["application_status"], "approved_held");
        assert!(
            current(&f, &format!("derived/entities/{name}.md"))
                .await
                .is_none()
        );
    }
    let held_view = review(&f).await;
    let original = held_view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "held-ready summary")
        .unwrap()
        .clone();
    write(
        &f,
        "sources/Stale/Stale.md",
        "# Stale\n\nA changed source observation.\n",
        1,
    )
    .await;
    control(&f, "full", 1).await;
    let second = admit(&f).await;
    let output = current(&f, "derived/entities/held-ready.md").await.unwrap();
    assert_eq!(output.0, 1);
    assert_eq!(output.1, original["candidate"]["after_md"]);
    assert_eq!(
        output.2["dreamer_summary"]["run_entry_ref"],
        original["run_entry_ref"]
    );
    assert_eq!(
        output.2["dreamer_summary"]["run_version"],
        original["run_version"]
    );
    assert!(
        current(&f, "derived/entities/held-stale.md")
            .await
            .is_none()
    );
    assert_eq!(second["pending"].as_array().unwrap().len(), 1);
    assert_eq!(second["pending"][0]["status"], "needs_changes");
    let (_, terminal) = finish(
        &f,
        &second,
        second["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    assert_eq!(
        terminal["latest_receipt"]["applied_writes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        terminal["latest_receipt"]["applied_writes"][0]["path"],
        "derived/entities/held-ready.md"
    );
    assert_eq!(terminal["counts"]["retained"], 1);
    // A later run must neither duplicate the accepted output nor drop the stale held item.
    let third = admit(&f).await;
    assert_eq!(
        current(&f, "derived/entities/held-ready.md")
            .await
            .unwrap()
            .0,
        1
    );
    assert_eq!(third["pending"].as_array().unwrap().len(), 1);
    finish(
        &f,
        &third,
        third["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
}

#[tokio::test]
async fn disabled_control_and_cross_owner_sources_cannot_create_run_state() {
    let Some(f) = fixture().await else {
        return;
    };
    let denied = admit(&f).await;
    assert_eq!(denied["admitted"], false);
    assert!(current(&f, "dreams/state.md").await.is_none());
    write(
        &f,
        "dreams/CONTROL.md",
        "enabled: false\nmode: report-only\n",
        0,
    )
    .await;
    assert_eq!(admit(&f).await["admitted"], false);
    assert!(current(&f, "dreams/state.md").await.is_none());
    control(&f, "report-only", 1).await;
    let other = actor(&f.pool, None, OWNER_CAPS).await;
    let foreign=ok(post(&f,&other,"/v1/workspace/write",json!({"path":"sources/Projects/Foreign.md","content":"# Foreign\n\nPrivate owner source.\n","expected_version":0})).await)["data"].clone();
    let admitted = admit(&f).await;
    let mut request_body = attempt(&admitted, admitted["state_version"].as_i64().unwrap());
    request_body["candidates"] = json!([candidate(&foreign, "foreign")]);
    let response = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        request_body,
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.body
    );
    assert!(!response.body.to_string().contains("Private owner source"));
    assert!(review(&f).await["items"].as_array().unwrap().is_empty());
    let before = current(&f, "dreams/state.md").await.unwrap().0;
    write(
        &f,
        "dreams/CONTROL.md",
        "enabled: false\nmode: report-only\n",
        2,
    )
    .await;
    let response = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/checkpoint",
        attempt(&admitted, before),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.body
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap().0, before);
    assert!(
        current(&f, &format!("dreams/runs/{}.md", date()))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn owner_commit_fence_reassigns_early_allocations_after_the_frozen_boundary() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Commit/Fixture.md",
        "# Commit\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let id = Uuid::parse_str(
        source["entry_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("entry:")
            .unwrap(),
    )
    .unwrap();
    let mut fence = f.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("brunn-workspace-commit:{}", f.owner.user))
        .execute(&mut *fence)
        .await
        .unwrap();
    let mut slow = f.pool.begin().await.unwrap();
    let early: i64 = sqlx::query_scalar(
        "SELECT nextval(pg_get_serial_sequence('brunn.workspace_changes','generation')::regclass)",
    )
    .fetch_one(&mut *slow)
    .await
    .unwrap();
    let high:i64=sqlx::query_scalar("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT user_id,entry_id,version,'update','sources/Commit/Fixture.md',content_sha256 FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=1 RETURNING generation")
        .bind(f.owner.user).bind(id).fetch_one(&mut *fence).await.unwrap();
    let frozen: i64 =
        sqlx::query_scalar("SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1")
            .bind(f.owner.user)
            .fetch_one(&mut *fence)
            .await
            .unwrap();
    assert_eq!(frozen, high);
    assert!(early < frozen);
    let user = f.owner.user;
    let mut pending = tokio::spawn(async move {
        let generation:i64=sqlx::query_scalar("INSERT INTO brunn.workspace_changes(generation,user_id,entry_id,entry_version,operation,path,content_sha256) OVERRIDING SYSTEM VALUE SELECT $3,user_id,entry_id,version,'update','sources/Commit/Fixture.md',content_sha256 FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=1 RETURNING generation")
            .bind(user).bind(id).bind(early).fetch_one(&mut *slow).await.unwrap();
        slow.commit().await.unwrap();
        generation
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending)
            .await
            .is_err(),
        "late writer must wait behind the owner snapshot fence"
    );
    fence.commit().await.unwrap();
    let committed = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(
        committed > frozen,
        "a delayed old allocation must not land behind the accepted cursor"
    );
    let after:Vec<i64>=sqlx::query_scalar("SELECT generation FROM brunn.workspace_changes WHERE user_id=$1 AND generation>$2 ORDER BY generation").bind(user).bind(frozen).fetch_all(&f.pool).await.unwrap();
    assert_eq!(after, vec![committed]);
    let admitted = admit(&f).await;
    assert!(admitted["frozen_generation"].as_i64().unwrap() >= committed);
    assert_eq!(admitted["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(admitted["inputs"][0]["entry_ref"], source["entry_ref"]);
    finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "failed",
    )
    .await;
}

#[tokio::test]
async fn corrections_revise_the_original_item_and_pin_both_immutable_candidates() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Revisions/Fixture.md",
        "# Fixture\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let first = admit(&f).await;
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "revision")],
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(&f).await;
    let original = view["items"][0].clone();
    ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, &original, "correct"),
    )
    .await);
    let changed = write(
        &f,
        "sources/Revisions/Fixture.md",
        "# Fixture\n\nA qualified and corrected observation.\n",
        1,
    )
    .await;
    let second = admit(&f).await;
    let mut replacement = candidate(&changed, "revision");
    replacement["revises_item_id"] = original["id"].clone();
    replacement["content"] = json!("# Revision\n\nA qualified and corrected observation.[^s1]\n");
    let (_, accepted) = submit(
        &f,
        &second,
        second["state_version"].as_i64().unwrap(),
        vec![replacement],
    )
    .await;
    assert_eq!(
        accepted["accepted_candidate_ids"],
        json!([original["id"].clone()])
    );
    finish(
        &f,
        &second,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let current_view = review(&f).await;
    assert_eq!(current_view["items"].as_array().unwrap().len(), 1);
    let revised = &current_view["items"][0];
    assert_eq!(revised["id"], original["id"]);
    assert_eq!(revised["sources"][0]["version"], 2);
    assert_eq!(
        revised["sources"][0]["excerpt"],
        "A qualified and corrected observation."
    );
    assert_ne!(revised["candidate_hash"], original["candidate_hash"]);
    assert_ne!(revised["run_version"], original["run_version"]);
    let old_decision = decision(&current_view, &original, "approve");
    assert_eq!(
        post(&f, &f.owner, "/v1/dreamer/review/decisions", old_decision)
            .await
            .status,
        StatusCode::CONFLICT
    );
    let original_id = Uuid::parse_str(
        original["run_entry_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("entry:")
            .unwrap(),
    )
    .unwrap();
    let audit:Value=sqlx::query_scalar("SELECT metadata->'dreamer_run' FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3").bind(f.owner.user).bind(original_id).bind(original["run_version"].as_i64().unwrap()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(audit["items"][0]["candidate"]["sources"][0]["version"], 1);
    assert_eq!(
        audit["items"][0]["candidate"]["sources"][0]["excerpt"],
        "A source-backed observation."
    );
}

#[tokio::test]
async fn original_decision_replays_after_compact_history_rolls_over() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/History/Fixture.md",
        "# History\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let a = admit(&f).await;
    let (_, submitted) = submit(
        &f,
        &a,
        a["state_version"].as_i64().unwrap(),
        vec![candidate(&source, "history")],
    )
    .await;
    let (terminal_request, terminal_response) = finish(
        &f,
        &a,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let mut original = Value::Null;
    for n in 0..34 {
        let view = review(&f).await;
        let mut body = decision(&view, &view["items"][0], "defer");
        body["comment"] = json!(format!("Deferred review fixture {n}"));
        if n == 0 {
            original = body.clone();
        }
        assert_eq!(
            ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", body).await)["data"]["application_status"],
            "deferred"
        );
    }
    let view = review(&f).await;
    assert!(view["history"].as_array().unwrap().len() <= 32);
    let before = current(&f, "dreams/state.md").await.unwrap().0;
    assert_eq!(
        ok(post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/finish",
            terminal_request
        )
        .await),
        terminal_response,
        "owner history rollover preserves exact terminal replay"
    );
    let replay = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        original.clone(),
    )
    .await);
    assert_eq!(replay["data"]["application_status"], "deferred");
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().0,
        before,
        "old request replay creates no state mutation"
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM brunn.entry_versions WHERE user_id=$1 AND metadata#>>'{dreamer_review,decision,idempotency_key}'=$2").bind(f.owner.user).bind(original["idempotency_key"].as_str().unwrap()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(count, 1);
    original["comment"] = json!("Conflicting old request body");
    assert_eq!(
        post(&f, &f.owner, "/v1/dreamer/review/decisions", original)
            .await
            .status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn deleted_sources_are_disposed_without_model_reads_or_resurrection() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Deleted/Fixture.md",
        "# Deleted\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let reference = source["entry_ref"].as_str().unwrap();
    let deleted = ok(request(
        &f.app,
        &f.owner,
        Method::DELETE,
        &format!("/v1/workspace/entries/{reference}?expected_version=1"),
        None,
    )
    .await);
    let deleted_generation = deleted["data"]["generation"]
        .as_i64()
        .or_else(|| deleted["data"]["workspace_generation"].as_i64())
        .unwrap();
    let admitted = admit(&f).await;
    assert_eq!(admitted["inputs"], json!([]));
    assert!(admitted["scanned_generation"].as_i64().unwrap() >= deleted_generation);
    assert!(admitted["processed_generation"].as_i64().unwrap() >= deleted_generation);
    let (_, finished) = finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    assert_eq!(finished["counts"]["retained"], 0);
    let run_id = Uuid::parse_str(
        finished["run_entry_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("entry:")
            .unwrap(),
    )
    .unwrap();
    let audit:Value=sqlx::query_scalar("SELECT metadata->'dreamer_run' FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3").bind(f.owner.user).bind(run_id).bind(finished["run_version"].as_i64().unwrap()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(audit["source_dispositions"].as_array().unwrap().len(), 1);
    assert_eq!(audit["source_dispositions"][0]["entry_ref"], reference);
    assert_eq!(
        audit["source_dispositions"][0]["disposition"],
        "deleted_source"
    );
    assert!(current(&f, "sources/Deleted/Fixture.md").await.is_none());
    let next = admit(&f).await;
    assert_eq!(next["inputs"], json!([]));
    finish(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
}

#[tokio::test]
async fn admission_replays_exactly_and_expired_attempt_is_audited_before_scheduled_dedupe() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let body =
        json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60});
    let first = ok(post(&f, &f.runner, "/v1/workspace/dreamer/admit", body.clone()).await);
    assert_eq!(
        ok(post(&f, &f.runner, "/v1/workspace/dreamer/admit", body.clone()).await),
        first
    );
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().0,
        first["state_version"]
    );
    let mut altered = body;
    altered["lease_seconds"] = json!(61);
    assert_eq!(
        post(&f, &f.runner, "/v1/workspace/dreamer/admit", altered)
            .await
            .status,
        StatusCode::CONFLICT
    );
    finish(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let retry = admit(&f).await;
    expire_fixture_lease(&f).await;
    let denied = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"nightly","lease_seconds":60}),
    )
    .await);
    assert_eq!(denied["admitted"], false);
    let persisted = current(&f, "dreams/state.md").await.unwrap();
    assert!(
        persisted.2["dreamer_state"]["active"].is_null(),
        "expired attempt is cleared even when scheduled work is already complete"
    );
    assert_eq!(
        persisted.2["dreamer_state"]["last_attempt"]["attempt_id"],
        retry["attempt_id"]
    );
    assert_eq!(
        persisted.2["dreamer_state"]["last_attempt"]["recovered"],
        true
    );
    let audit = current(&f, &format!("dreams/runs/{}.md", date()))
        .await
        .unwrap();
    assert_eq!(
        audit.2["dreamer_run"]["attempt"]["attempt_id"],
        retry["attempt_id"]
    );
    assert_eq!(audit.2["dreamer_run"]["attempt"]["outcome"], "partial");
}

#[tokio::test]
async fn deleted_review_evidence_withholds_cached_fields_from_review_and_runner_admission() {
    let Some(f) = fixture().await else {
        return;
    };
    const MARKER: &str = "DELETED_REVIEW_EVIDENCE_CANARY_57ac";
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/DeletedReview/Fixture.md",
        &format!("# Source\n\n{MARKER}\n"),
        0,
    )
    .await;
    let first = admit(&f).await;
    let mut item = candidate(&source, "delete-review");
    item["title"] = json!(MARKER);
    item["summary"] = json!(MARKER);
    item["reason"] = json!(MARKER);
    item["content"] = json!(format!("# Candidate\n\n{MARKER}.[^s1]\n"));
    let (_, submitted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![item],
    )
    .await;
    finish(
        &f,
        &first,
        submitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let initial = review(&f).await;
    assert!(initial.to_string().contains(MARKER));
    let reference = source["entry_ref"].as_str().unwrap();
    ok(request(
        &f.app,
        &f.owner,
        Method::DELETE,
        &format!("/v1/workspace/entries/{reference}?expected_version=1"),
        None,
    )
    .await);
    let unavailable = review(&f).await;
    assert!(
        !unavailable.to_string().contains(MARKER),
        "deleted-source cached summary, excerpt and title must be withheld"
    );
    assert_eq!(unavailable["items"][0]["stale"], true);
    assert_eq!(unavailable["items"][0]["reviewable"], false);
    assert_eq!(
        post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&unavailable, &unavailable["items"][0], "approve")
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let successor = admit(&f).await;
    assert!(
        !successor.to_string().contains(MARKER),
        "runner admission must also withhold unavailable cached evidence"
    );
    assert_eq!(successor["inputs"], json!([]));
    finish(
        &f,
        &successor,
        successor["state_version"].as_i64().unwrap(),
        "skipped",
    )
    .await;
}

async fn seed_location_pilot(f: &Fixture) -> (chrono::DateTime<Utc>, String, String) {
    // Keep this explicit pilot outside the automatically selected latest closed
    // Pacific day, including the hours when UTC has already advanced a date.
    let from = (Utc::now() - chrono::Duration::days(3))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let at = from + chrono::Duration::hours(12);
    let path = format!("Location/Visits/{}.md", from.format("%Y-%m"));
    let content = format!(
        "---\nkind: location-visits\nmonth: {}\n---\n| Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| {} | {} | 1h | A bounded stop | visit | Bellevue | medium | 47.0000,-122.0000 |\n",
        from.format("%Y-%m"),
        at.format("%Y-%m-%dT%H:%M%:z"),
        (at + chrono::Duration::hours(1)).format("%Y-%m-%dT%H:%M%:z")
    );
    write(f, &path, &content, 0).await;
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m) VALUES($1,$2,'ping',0,47,-122,5)").bind(f.owner.user).bind(at).execute(&f.pool).await.unwrap();
    (from, path, content)
}

async fn queue_pilot(f: &Fixture, from: chrono::DateTime<Utc>) -> Value {
    let response = ok(post(
        f,
        &f.owner,
        "/v1/dreamer/review/location-pilot",
        json!({"date":from.date_naive(),"timezone":"UTC"}),
    )
    .await);
    assert_eq!(response["status"], "complete");
    assert_eq!(response["data"]["queued"], true);
    response
}

const PILOT_UNCERTAINTY: &str =
    "Point samples do not establish continuous presence or movement between observations.";

fn pilot_candidate(admission: &Value) -> Value {
    let work = &admission["location_work"];
    let packet = &admission["location_evidence"];
    assert_eq!(packet["fingerprint_complete"], true);
    assert_eq!(work["fingerprint"], packet["evidence_fingerprint"]);
    let document = &packet["canonical_months"][0];
    let selector = &document["selectors"][0];
    json!({"kind":"summary","title":"Historical location evidence","summary":"A bounded historical day with exact sources.","reason":"Review a sourced reconstruction with explicit coverage uncertainty.",
        "path":format!("derived/location/{}.md",work["date"].as_str().unwrap()),"expected_version":0,
        "content":format!("# Historical location evidence\n\nThe canonical record gives minute-rounded boundaries 12:00 and 13:00.[^s1]\nThe retained sample at 12:00:00 has five-meter reported accuracy.[^r1]\n\n{PILOT_UNCERTAINTY}[^r1]\n"),
        "uncertainty":PILOT_UNCERTAINTY,
        "sources":[{"entry_ref":document["ref"],"version":document["version"],"start_line":selector["start_line"],"end_line":selector["end_line"]}],
        "raw_sources":[{"natural_key":packet["reports"][0]["natural_key"],"fields":["at","lat","lon","accuracy_m","first_received_at"]}],
        "evidence_scope":{"from":work["from"],"to":work["to"],"timezone":work["timezone"],"fingerprint":work["fingerprint"]}})
}

async fn historical_context(
    f: &Fixture,
    from: chrono::DateTime<Utc>,
    path: &str,
    text: &str,
) -> Value {
    let source = write(f, path, text, 0).await;
    sqlx::query("UPDATE brunn.entry_versions SET created_at=$3 WHERE user_id=$1 AND entry_id=$2")
        .bind(f.owner.user)
        .bind(
            Uuid::parse_str(
                source["entry_ref"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("entry:"),
            )
            .unwrap(),
        )
        .bind(from - chrono::Duration::days(1))
        .execute(&f.pool)
        .await
        .unwrap();
    source
}
async fn discover_context(f: &Fixture, admission: &Value, query: &str) -> Value {
    let mut body = attempt(admission, admission["state_version"].as_i64().unwrap());
    body["context_queries"] = json!([query]);
    body["web_sources"] = json!([]);
    ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/location-discover",
        body,
    )
    .await)["data"]
        .clone()
}

fn context_pilot_candidate(admission: &Value) -> Value {
    let mut candidate = pilot_candidate(admission);
    candidate["evidence_scope"]["context_sources"] =
        admission["location_work"]["context_sources"].clone();
    for context in admission["location_context"].as_array().unwrap() {
        candidate["sources"].as_array_mut().unwrap().push(json!({"entry_ref":context["entry_ref"],"version":context["version"],"start_line":context["start_line"],"end_line":context["end_line"]}));
    }
    candidate["content"] = json!(format!(
        "{}\nThe owner identifies this stop as Example Garden.[^s2]\n",
        candidate["content"].as_str().unwrap()
    ));
    candidate
}

#[tokio::test]
async fn latest_closed_day_is_automatically_admitted_and_retained_across_failure() {
    use chrono::TimeZone;
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let yesterday = Utc::now()
        .with_timezone(&chrono_tz::America::Los_Angeles)
        .date_naive()
        .pred_opt()
        .unwrap();
    let at = chrono_tz::America::Los_Angeles
        .from_local_datetime(&yesterday.and_hms_opt(12, 0, 0).unwrap())
        .single()
        .unwrap()
        .to_utc();
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m) VALUES($1,$2,'ping',-420,47,-122,5)")
        .bind(f.owner.user).bind(at).execute(&f.pool).await.unwrap();
    let first = admit(&f).await;
    assert_eq!(first["location_work"]["date"], yesterday.to_string());
    assert_eq!(first["location_work"]["timezone"], "America/Los_Angeles");
    assert_eq!(
        first["location_evidence"]["reports"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    finish(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        "failed",
    )
    .await;
    let next = admit(&f).await;
    assert_eq!(
        next["location_work"]["date"],
        first["location_work"]["date"]
    );
    assert_eq!(
        next["location_work"]["fingerprint"],
        first["location_work"]["fingerprint"]
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(
        state.2["dreamer_state"]["location_work"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    finish(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn discovery_refinement_is_bounded_fenced_and_replay_safe() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    historical_context(
        &f,
        from,
        "sources/Context/Park.md",
        "# Park\n\nPublic Park hosts the Observatory.\n",
    )
    .await;
    let alias = historical_context(
        &f,
        from,
        "sources/Context/Observatory.md",
        "# Observatory\n\nThe Observatory is a distinct public venue.\n",
    )
    .await;
    queue_pilot(&f, from).await;
    let admitted = admit(&f).await;
    let first = discover_context(&f, &admitted, "Public Park").await;
    assert_eq!(first["location_work"]["discovery"]["round"], 1);
    let mut request = attempt(&first, first["state_version"].as_i64().unwrap());
    request["context_queries"] = json!(["Observatory", "Public Park"]);
    request["web_sources"] = json!([]);
    request["refines_request_hash"] = json!("wrong-parent");
    assert!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/location-discover",
            request.clone()
        )
        .await
        .status
        .is_client_error()
    );
    request["refines_request_hash"] = first["location_work"]["discovery"]["request_hash"].clone();
    let mut stale = request.clone();
    stale["expected_state_version"] = admitted["state_version"].clone();
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/location-discover",
            stale
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let refined = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/location-discover",
        request.clone(),
    )
    .await)["data"]
        .clone();
    assert_eq!(refined["location_work"]["discovery"]["round"], 2);
    assert_eq!(
        refined["location_work"]["fingerprint"],
        first["location_work"]["fingerprint"]
    );
    assert!(
        refined["location_context"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["entry_ref"] == alias["entry_ref"])
    );
    assert_eq!(refined["inputs"], admitted["inputs"]);
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/location-discover",
        request.clone(),
    )
    .await)["data"]
        .clone();
    assert_eq!(replay["state_version"], refined["state_version"]);
    let first_replay = discover_context(&f, &admitted, "Public Park").await;
    assert_eq!(first_replay["state_version"], refined["state_version"]);
    request["expected_state_version"] = refined["state_version"].clone();
    request["refines_request_hash"] = refined["location_work"]["discovery"]["request_hash"].clone();
    request["context_queries"] = json!(["third pass"]);
    assert!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/location-discover",
            request
        )
        .await
        .status
        .is_client_error()
    );
    finish(
        &f,
        &refined,
        refined["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let next = admit(&f).await;
    assert!(next["location_work"]["discovery"].is_null());
    assert_eq!(next["location_context"], json!([]));
    finish(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn place_discovery_excludes_substring_hits_credentials_and_secret_adjacent_excerpts() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    let valid = historical_context(
        &f,
        from,
        "sources/Places/Familiar.md",
        "# Familiar venues\n\nFogo is a restaurant.\n",
    )
    .await;
    for (path, content) in [
        (
            "sources/Notes/Unrelated.md",
            "# Unrelated\n\nPREFIXFogoSUFFIX\n".to_owned(),
        ),
        (
            "sources/Private/API Keys.md",
            "# Credential fixture\n\nFogo CREDENTIAL_DOCUMENT_CANARY\n".to_owned(),
        ),
        (
            "sources/Private/passwords.md",
            "# Fixture\n\nFogo PASSWORD_DOCUMENT_CANARY\n".to_owned(),
        ),
        (
            "sources/Notes/Mixed.md",
            format!(
                "# Fogo\n\n{}\nSECRET_ADJACENT_CANARY\n",
                "fake1234".repeat(8)
            ),
        ),
    ] {
        historical_context(&f, from, path, &content).await;
    }
    // Either missing pre-cap predicate would let 33 newer generated hits hide
    // the eligible primary before the historical query's LIMIT 32.
    for index in 0..66 {
        let path = format!("Briefings/2026/Generated-{index:02}.md");
        let briefing = historical_context(
            &f,
            from,
            &path,
            "# Edition\n\nFogo GENERATED_BRIEFING_CANARY\n",
        )
        .await;
        let id = Uuid::parse_str(
            briefing["entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:"),
        )
        .unwrap();
        sqlx::query("UPDATE brunn.entry_versions SET created_at=$3 WHERE user_id=$1 AND entry_id=$2 AND version=1")
            .bind(f.owner.user).bind(id).bind(from-chrono::Duration::hours(12)).execute(&f.pool).await.unwrap();
        write(
            &f,
            &path,
            "# Later context\n\nA later Fogo observation.\n",
            1,
        )
        .await;
        sqlx::query("UPDATE brunn.entry_versions SET metadata=metadata||'{\"kind\":\"briefing_edition\"}'::jsonb WHERE user_id=$1 AND entry_id=$2 AND version=$3")
            .bind(f.owner.user).bind(id).bind(if index<33 {1_i64}else{2}).execute(&f.pool).await.unwrap();
    }
    queue_pilot(&f, from).await;
    let admitted = admit(&f).await;
    assert_eq!(
        admitted["session_id"],
        format!("session:{}", admitted["attempt_id"].as_str().unwrap())
    );
    assert!(!admitted["inputs"].to_string().contains("API Keys.md"));
    assert!(!admitted["inputs"].to_string().contains("passwords.md"));
    assert!(!admitted["inputs"].to_string().contains("Generated-33.md"));
    let found = discover_context(&f, &admitted, "Fogo").await;
    let sources = found["location_context"].as_array().unwrap();
    assert_eq!(sources.len(), 1, "unexpected discovery sources");
    assert_eq!(sources[0]["entry_ref"], valid["entry_ref"]);
    assert!(!found.to_string().contains("GENERATED_BRIEFING_CANARY"));
    assert_eq!(
        sources[0]["excerpt"],
        "# Familiar venues\n\nFogo is a restaurant."
    );
    finish(
        &f,
        &found,
        found["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn autonomous_context_uses_historical_versions_before_matching_and_preserves_progress() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    let source = historical_context(
        &f,
        from,
        "sources/Context/Garden.md",
        "# Garden\n\nExample Garden is a familiar place.\n",
    )
    .await;
    write(
        &f,
        "sources/Context/Garden.md",
        "# Future correction\n\nExample Garden LATER_ITINERARY_CANARY.\n",
        1,
    )
    .await;
    write(
        &f,
        "sources/Context/New.md",
        "# New answer\n\nExample Garden NEW_ANSWER_CANARY.\n",
        0,
    )
    .await;
    let generated = historical_context(
        &f,
        from,
        "fixtures/old.md",
        "# Example Garden\n\nGENERATED_ANSWER_CANARY.\n",
    )
    .await;
    // Deliberately place an old generated answer behind the protected namespace
    // using fixture authority; the ordinary write API correctly forbids this.
    sqlx::query(
        "UPDATE brunn.entries SET path='derived/location/old.md' WHERE user_id=$1 AND id=$2",
    )
    .bind(f.owner.user)
    .bind(
        Uuid::parse_str(
            generated["entry_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("entry:"),
        )
        .unwrap(),
    )
    .execute(&f.pool)
    .await
    .unwrap();
    queue_pilot(&f, from).await;
    let admitted = admit(&f).await;
    let first = discover_context(&f, &admitted, "Example Garden").await;
    assert_eq!(first["location_context"].as_array().unwrap().len(), 1);
    assert_eq!(
        first["location_context"][0]["entry_ref"],
        source["entry_ref"]
    );
    assert_eq!(first["location_context"][0]["version"], 1);
    assert_eq!(first["location_context"][0]["current_version"], 2);
    assert_eq!(first["inputs"], admitted["inputs"]);
    let shown = first["location_context"].to_string();
    for marker in [
        "LATER_ITINERARY_CANARY",
        "NEW_ANSWER_CANARY",
        "GENERATED_ANSWER_CANARY",
    ] {
        assert!(!shown.contains(marker));
    }
    let replay = discover_context(&f, &admitted, "Example Garden").await;
    assert_eq!(replay["state_version"], first["state_version"]);
    let mut valid = context_pilot_candidate(&first);
    valid["content"] = json!(format!(
        "Times are approximate observation windows.\n\n{}",
        valid["content"].as_str().unwrap()
    ));
    let envelope = json!({"schema":"dream.candidates.v1","candidates":[valid.clone()],"processed_inputs":[],"findings":[]});
    assert_eq!(
        brunn::dreamer::prompt::location_submission_issues(&envelope, &first),
        Vec::<String>::new()
    );
    let mut lean = envelope.clone();
    lean["candidates"][0]["evidence_scope"]
        .as_object_mut()
        .unwrap()
        .remove("context_sources");
    let rebound =
        brunn::dreamer::prompt::compile_location_evidence_inventory(&lean, &first).unwrap();
    assert_eq!(
        rebound["candidates"][0]["evidence_scope"],
        valid["evidence_scope"]
    );
    lean["candidates"][0]["evidence_scope"]["context_sources"] = json!([]);
    assert!(
        brunn::dreamer::prompt::compile_location_evidence_inventory(&lean, &first)
            .unwrap_err()
            .contains("changed the frozen")
    );
    for invalid in [
        {
            let mut c = valid.clone();
            c["evidence_scope"]["context_sources"][0]["excerpt"] = json!("Invented");
            c
        },
        {
            let mut c = valid.clone();
            c["sources"][1]["version"] = json!(2);
            c
        },
    ] {
        let mut body = attempt(&first, first["state_version"].as_i64().unwrap());
        body["candidates"] = json!([invalid]);
        body["processed_inputs"] = json!([]);
        let r = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
        assert!(r.status.is_client_error(), "{}", r.body);
    }
    let mut body = attempt(&first, first["state_version"].as_i64().unwrap());
    body["candidates"] = json!([valid]);
    body["processed_inputs"] = json!([]);
    body["findings"] = json!([]);
    let accepted = ok(post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await);
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(state.2["dreamer_state"]["inputs"], first["inputs"]);
    assert_eq!(state.2["dreamer_state"]["location_work"], json!([]));
    finish(
        &f,
        &first,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    write(
        &f,
        "sources/Context/Garden.md",
        "# Changed again\n\nExample Garden changed.\n",
        2,
    )
    .await;
    assert_eq!(review(&f).await["items"][0]["stale"], true);
    let fresh = admit(&f).await;
    assert!(fresh["location_work"].is_object());
    assert_eq!(fresh["location_context"], json!([]));
    let rediscovered = discover_context(&f, &fresh, "Example Garden").await;
    assert_eq!(rediscovered["location_context"][0]["version"], 1);
    assert_eq!(rediscovered["location_context"][0]["current_version"], 3);
    finish(
        &f,
        &rediscovered,
        rediscovered["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn autonomous_discovery_is_fenced_scoped_and_does_not_accept_selected_itineraries() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    let source = historical_context(
        &f,
        from,
        "sources/Context/Private.md",
        "# Private\n\nPRIVATE_CONTEXT_MARKER Example Garden.\n",
    )
    .await;
    let bad=post(&f,&f.owner,"/v1/dreamer/review/location-pilot",json!({"date":from.date_naive(),"timezone":"UTC","context_sources":[{"entry_ref":source["entry_ref"]}]})).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    queue_pilot(&f, from).await;
    let admitted = admit(&f).await;
    let mut body = attempt(&admitted, admitted["state_version"].as_i64().unwrap());
    body["context_queries"] = json!(["Example Garden"]);
    body["web_sources"] = json!([]);
    for actor in [&f.model, &actor(&f.pool, None, OWNER_CAPS).await] {
        let denied = post(
            &f,
            actor,
            "/v1/workspace/dreamer/location-discover",
            body.clone(),
        )
        .await;
        assert!(denied.status.is_client_error());
        assert!(!denied.body.to_string().contains("PRIVATE_CONTEXT_MARKER"));
    }
    let first = discover_context(&f, &admitted, "Example Garden").await;
    assert!(
        first["location_context"]
            .to_string()
            .contains("PRIVATE_CONTEXT_MARKER")
    );
    sqlx::query("UPDATE brunn.entries SET path='.brunn/tasks/'||id::text||'.md' WHERE user_id=$1 AND path='sources/Context/Private.md'")
        .bind(f.owner.user).execute(&f.pool).await.unwrap();
    let hidden = ok(post(
        &f,
        &f.model,
        "/v1/workspace/read",
        json!({"requests":[{"path":"dreams/state.md","view":"full","max_chars":20000}]}),
    )
    .await);
    assert!(!hidden.to_string().contains("PRIVATE_CONTEXT_MARKER"));
    assert_eq!(
        hidden["data"]["items"][0]["representation"],
        "audit_withheld"
    );
    finish(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn verified_web_evidence_is_retained_once_and_sealed_to_the_attempt() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    queue_pilot(&f, from).await;
    let first = admit(&f).await;
    let mut body = attempt(&first, first["state_version"].as_i64().unwrap());
    body["context_queries"] = json!([]);
    body["web_sources"] = json!([{"url":"https://example.org/garden","quote":"Example Garden 12 Main Street","fetched_at":Utc::now(),"body_sha256":format!("sha256:{}","a".repeat(64)),"verification":"fetched_exact_quote"}]);
    let accepted = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/location-discover",
        body.clone(),
    )
    .await)["data"]
        .clone();
    let source = &accepted["location_context"][0];
    assert!(
        source["path"]
            .as_str()
            .unwrap()
            .starts_with("Evidence/Location/")
    );
    assert_eq!(source["discovery_origin"], "verified_web");
    assert!(
        source["excerpt"]
            .as_str()
            .unwrap()
            .contains("Example Garden 12 Main Street")
    );
    let replay = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/location-discover",
        body.clone(),
    )
    .await);
    assert_eq!(replay["data"]["state_version"], accepted["state_version"]);
    body["expected_state_version"] = accepted["state_version"].clone();
    body["web_sources"][0]["quote"] = json!("different quote");
    assert_eq!(
        post(
            &f,
            &f.runner,
            "/v1/workspace/dreamer/location-discover",
            body
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    finish(
        &f,
        &accepted,
        accepted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
}

#[tokio::test]
async fn published_location_context_stales_reads_and_held_publication_on_source_change() {
    for held in [false, true] {
        let Some(f) = fixture().await else {
            return;
        };
        control(&f, if held { "report-only" } else { "full" }, 0).await;
        let (from, _, _) = seed_location_pilot(&f).await;
        let source = historical_context(
            &f,
            from,
            "sources/Context/Owner.md",
            "# Owner statement\n\nI identify the stop as Example Garden.\n",
        )
        .await;
        queue_pilot(&f, from).await;
        let admitted = admit(&f).await;
        let first = discover_context(&f, &admitted, "Example Garden").await;
        let (_, accepted) = submit(
            &f,
            &first,
            first["state_version"].as_i64().unwrap(),
            vec![context_pilot_candidate(&first)],
        )
        .await;
        finish(
            &f,
            &first,
            accepted["state_version"].as_i64().unwrap(),
            "completed",
        )
        .await;
        let view = review(&f).await;
        ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, &view["items"][0], "approve"),
        )
        .await);
        let target = format!("derived/location/{}.md", from.date_naive());
        let read = json!({"requests":[{"path":target,"view":"full","max_chars":20000}]});
        if !held {
            let fresh = ok(post(&f, &f.owner, "/v1/workspace/read", read.clone()).await);
            let rendered = &fresh["data"]["items"][0];
            assert_eq!(rendered["freshness"]["status"], "fresh");
            assert!(rendered.get("metadata").is_none());
            assert!(
                rendered["source_documents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["reference"] == source["entry_ref"])
            );
        }
        write(
            &f,
            "sources/Context/Owner.md",
            "# Owner statement\n\nI correct the stop to Example Park.\n",
            1,
        )
        .await;
        if held {
            control(&f, "full", 1).await;
            let next = admit(&f).await;
            assert!(current(&f, &target).await.is_none());
            assert_eq!(review(&f).await["items"][0]["status"], "stale");
            finish(
                &f,
                &next,
                next["state_version"].as_i64().unwrap(),
                "partial",
            )
            .await;
        } else {
            let stale = ok(post(&f, &f.owner, "/v1/workspace/read", read.clone()).await);
            assert_ne!(stale["data"]["items"][0]["freshness"]["status"], "fresh");
            sqlx::query("UPDATE brunn.entries SET deleted_at=now() WHERE user_id=$1 AND path='sources/Context/Owner.md'")
                .bind(f.owner.user).execute(&f.pool).await.unwrap();
            let hidden = ok(post(&f, &f.owner, "/v1/workspace/read", read).await);
            assert!(!hidden.to_string().contains("Example Garden"));
        }
    }
}

#[tokio::test]
async fn location_candidate_guards_retain_work_and_revision_preserves_published_uncertainty() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "full", 0).await;
    let narrative = write(
        &f,
        "sources/Other/Note.md",
        "# Other\n\nA source-backed observation.\n",
        0,
    )
    .await;
    let (from, _, _) = seed_location_pilot(&f).await;
    queue_pilot(&f, from).await;
    let first = admit(&f).await;
    let valid = pilot_candidate(&first);
    let target = valid["path"].as_str().unwrap();
    let invalid_variants = |valid: &Value| {
        let mut raw_omitted = valid.clone();
        raw_omitted["raw_sources"] = json!([]);
        raw_omitted["content"] = json!(format!(
            "# Canonical only\n\nA bounded stop is recorded.[^s1]\n{PILOT_UNCERTAINTY}[^s1]\n"
        ));
        let mut canonical_omitted = valid.clone();
        canonical_omitted["sources"] = json!([]);
        canonical_omitted["content"] = json!(format!(
            "# Raw only\n\nA point sample has five-meter reported accuracy.[^r1]\n{PILOT_UNCERTAINTY}[^r1]\n"
        ));
        let mut unused_raw = valid.clone();
        // Isolate citation participation from the independently checked clock claims.
        let without_clocks = valid["content"]
            .as_str()
            .unwrap()
            .replace("12:00:00", "the retained timestamp")
            .replace("12:00 and 13:00", "as recorded");
        unused_raw["content"] = json!(without_clocks.replace("[^r1]", "[^s1]"));
        let mut overlong = valid.clone();
        overlong["content"] = json!(format!(
            "{}\n{}[^r1]",
            valid["content"].as_str().unwrap(),
            "Audit detail. ".repeat(130)
        ));
        let mut undeclared = valid.clone();
        undeclared["content"] = json!(format!(
            "{}\nAn unsupported marker.[^r1][^r999]",
            valid["content"].as_str().unwrap()
        ));
        let mut sidebar_only = valid.clone();
        sidebar_only["uncertainty"] =
            json!("Material gaps remain unresolved; the day may contain unobserved stops.");
        let mut heading_only_raw = unused_raw.clone();
        heading_only_raw["content"] = json!(format!(
            "# Raw sources [^r1]\n\n{}",
            unused_raw["content"].as_str().unwrap()
        ));
        let mut wrong_clock = valid.clone();
        wrong_clock["content"] = json!(
            valid["content"]
                .as_str()
                .unwrap()
                .replace("12:00:00", "12:00:37")
        );
        let mut canonical_seconds = valid.clone();
        canonical_seconds["content"] = json!(valid["content"].as_str().unwrap().replace(
            "boundaries 12:00 and 13:00",
            "boundaries 12:00:00 and 13:00:00"
        ));
        let mut too_short = valid.clone();
        too_short["content"] = json!(format!(
            "{}\n| 12:00 pm–12:00 pm | Named destination — high confidence[^r1] |",
            valid["content"].as_str().unwrap()
        ));
        vec![
            ("must cite retained raw observations", raw_omitted),
            (
                "must reconcile the relevant canonical visit rows",
                canonical_omitted,
            ),
            ("every declared raw source must be cited", unused_raw),
            ("every declared raw source must be cited", heading_only_raw),
            ("at most 250 words", overlong),
            ("undeclared citation marker", undeclared),
            ("summary uncertainty must appear verbatim", sidebar_only),
            ("location content validation failed", wrong_clock),
            ("location content validation failed", canonical_seconds),
            ("longer than two minutes", too_short),
        ]
    };
    let initial_state = current(&f, "dreams/state.md").await.unwrap();
    for (message, invalid) in invalid_variants(&valid) {
        let mut body = attempt(&first, first["state_version"].as_i64().unwrap());
        body["candidates"] = json!([invalid]);
        body["processed_inputs"] = json!([]);
        let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
        assert_eq!(
            rejected.status,
            StatusCode::BAD_REQUEST,
            "{}",
            rejected.body
        );
        assert!(
            rejected.body.to_string().contains(message),
            "{}",
            rejected.body
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), initial_state);
        assert!(current(&f, target).await.is_none());
    }
    let (_, accepted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![valid.clone(), candidate(&narrative, "unrelated-review")],
    )
    .await;
    finish(
        &f,
        &first,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let initial_view = review(&f).await;
    let original = initial_view["items"][0].clone();
    let unrelated = initial_view["items"][1].clone();
    let old_run_id = Uuid::parse_str(
        original["run_entry_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("entry:")
            .unwrap(),
    )
    .unwrap();
    let old_version = original["run_version"].as_i64().unwrap();
    let old_audit: (String, Value) = sqlx::query_as("SELECT content,metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
        .bind(f.owner.user).bind(old_run_id).bind(old_version).fetch_one(&f.pool).await.unwrap();
    queue_pilot(&f, from).await;
    let next = admit(&f).await;
    let mut revision = pilot_candidate(&next);
    revision["revises_item_id"] = original["id"].clone();
    revision["content"] = json!(revision["content"].as_str().unwrap().replace(
        "# Historical location evidence",
        "# Reviewed historical location evidence"
    ));
    let pending_state = current(&f, "dreams/state.md").await.unwrap();
    let mut invalid_revisions = invalid_variants(&revision);
    let mut omitted_id = revision.clone();
    omitted_id
        .as_object_mut()
        .unwrap()
        .remove("revises_item_id");
    invalid_revisions.push((
        "must revise the existing item for this destination",
        omitted_id,
    ));
    let mut unrelated_id = revision.clone();
    unrelated_id["revises_item_id"] = unrelated["id"].clone();
    invalid_revisions.push((
        "must preserve the original destination, kind and evidence window",
        unrelated_id,
    ));
    let mut wrong_kind = candidate(&narrative, "unrelated-review");
    wrong_kind["revises_item_id"] = original["id"].clone();
    invalid_revisions.push((
        "must preserve the original destination, kind and evidence window",
        wrong_kind,
    ));
    for (message, invalid) in invalid_revisions {
        let mut body = attempt(&next, next["state_version"].as_i64().unwrap());
        body["candidates"] = json!([invalid]);
        body["processed_inputs"] = json!([]);
        let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
        assert_eq!(
            rejected.status,
            StatusCode::BAD_REQUEST,
            "{}",
            rejected.body
        );
        assert!(
            rejected.body.to_string().contains(message),
            "{}",
            rejected.body
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), pending_state);
        let unchanged = review(&f).await;
        assert_eq!(unchanged["items"].as_array().unwrap().len(), 2);
        for key in ["id", "candidate_hash", "run_entry_ref", "run_version"] {
            assert_eq!(unchanged["items"][0][key], original[key]);
            assert_eq!(unchanged["items"][1][key], unrelated[key]);
        }
        assert!(current(&f, target).await.is_none());
    }
    let mut two_locations = attempt(&next, next["state_version"].as_i64().unwrap());
    two_locations["candidates"] = json!([revision.clone(), pilot_candidate(&next)]);
    two_locations["processed_inputs"] = json!([]);
    let rejected = post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/candidates",
        two_locations,
    )
    .await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    assert!(
        rejected
            .body
            .to_string()
            .contains("at most one location candidate is allowed per submission")
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), pending_state);
    // The canonical inventory remains exact metadata even when only raw
    // observations are useful in the human-readable timeline.
    revision["content"] = json!(format!(
        "# Reviewed day\n\n| When | Where |\n| --- | --- |\n| Around 12:00 | A bounded stop was observed.[^r1] |\n\n{PILOT_UNCERTAINTY}[^r1]"
    ));
    let (_, recompiled) = submit(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        vec![revision],
    )
    .await;
    assert_eq!(
        recompiled["accepted_candidate_ids"],
        json!([original["id"].clone()])
    );
    finish(
        &f,
        &next,
        recompiled["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let updated = review(&f).await;
    assert_eq!(updated["items"].as_array().unwrap().len(), 2);
    let item = &updated["items"][0];
    assert_eq!(item["id"], original["id"]);
    assert_ne!(item["candidate_hash"], original["candidate_hash"]);
    assert_eq!(item["uncertainty_md"], PILOT_UNCERTAINTY);
    let exact_preview = item["candidate"]["after_md"].as_str().unwrap();
    assert_eq!(exact_preview.matches(PILOT_UNCERTAINTY).count(), 1);
    assert!(!exact_preview.contains("[^"));
    assert!(exact_preview.contains("| Around 12:00 |"));
    let applied = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&updated, item, "approve"),
    )
    .await);
    assert_eq!(applied["data"]["application_status"], "applied");
    let published = current(&f, target).await.unwrap();
    assert_eq!(
        published.1, exact_preview,
        "publication must preserve exact reviewed bytes, including uncertainty"
    );
    let manifest = &published.2["dreamer_summary"];
    assert_eq!(manifest["presentation"], "location-timeline.v1");
    assert_eq!(manifest["sources"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["raw_sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        manifest["claim_sources"][0],
        json!({"line":5,"sources":["r1"]})
    );
    // Default reads preserve the exact primary body and a small evidence
    // pointer, without reinjecting raw keys, excerpts, or repeated footnotes.
    for view in ["full", "current_state"] {
        let read = ok(post(
            &f,
            &f.owner,
            "/v1/workspace/read",
            json!({"requests":[{"path":target,"view":view,"max_chars":20000}]}),
        )
        .await);
        let rendered = &read["data"]["items"][0];
        assert_eq!(rendered["text"], exact_preview);
        assert!(rendered.get("metadata").is_none());
        assert_eq!(rendered["evidence"]["reference"], item["run_entry_ref"]);
        assert_eq!(rendered["evidence"]["version"], item["run_version"]);
        assert_eq!(
            rendered["source_documents"][0]["reference"],
            manifest["sources"][0]["entry_ref"]
        );
        assert_eq!(rendered["location_evidence_request"]["timezone"], "UTC");
        assert!(!rendered.to_string().contains("natural_key"));
        assert!(!rendered.to_string().contains("first_received_at"));
    }
    let still_historical: (String, Value) = sqlx::query_as("SELECT content,metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
        .bind(f.owner.user).bind(old_run_id).bind(old_version).fetch_one(&f.pool).await.unwrap();
    assert_eq!(
        still_historical, old_audit,
        "a rejected or successful revision cannot mutate its prior immutable run"
    );
}

#[tokio::test]
async fn location_readable_timeline_cannot_discard_uncited_canonical_evidence_rows() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, path, mut content) = seed_location_pilot(&f).await;
    content.push_str(&format!("| {}T14:00+00:00 | {}T15:00+00:00 | 1h | Another stop | visit | Bellevue | medium | 47.1000,-122.1000 |\n",from.date_naive(),from.date_naive()));
    write(&f, &path, &content, 1).await;
    queue_pilot(&f, from).await;
    let admission = admit(&f).await;
    assert_eq!(
        admission["location_evidence"]["canonical_months"][0]["selectors"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut body = attempt(&admission, admission["state_version"].as_i64().unwrap());
    body["candidates"] = json!([pilot_candidate(&admission)]);
    body["processed_inputs"] = json!([]);
    let before = current(&f, "dreams/state.md").await.unwrap();
    let response = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        response
            .body
            .to_string()
            .contains("must retain every relevant canonical row")
    );
    assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
}

#[derive(Clone, Copy)]
enum ObsoleteLocationCandidate {
    SidebarUncertainty,
    MissingRaw,
    WrongClock,
}

async fn seed_obsolete_location_candidate(
    f: &Fixture,
    obsolete: ObsoleteLocationCandidate,
    status: &str,
) -> (chrono::DateTime<Utc>, Value, &'static str) {
    let (from, _, _) = seed_location_pilot(f).await;
    queue_pilot(f, from).await;
    let admitted = admit(f).await;
    let (_, accepted) = submit(
        f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        vec![pilot_candidate(&admitted)],
    )
    .await;

    // Simulate a candidate accepted before these validation rules existed.
    // Copy its exact submitted source data back into an unfinalized
    // fixture candidate. Normal finish creates a new immutable audit;
    // the already-written valid audit version is never rewritten.
    let mut stored = current(f, "dreams/state.md").await.unwrap();
    let item = &mut stored.2["dreamer_state"]["items"][0];
    let run_id = Uuid::parse_str(
        item["run_entry_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("entry:")
            .unwrap(),
    )
    .unwrap();
    let audit: Value = sqlx::query_scalar("SELECT metadata->'dreamer_run' FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
            .bind(f.owner.user).bind(run_id).bind(item["run_version"].as_i64().unwrap()).fetch_one(&f.pool).await.unwrap();
    let original_candidate = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|old| old["id"] == item["id"])
        .unwrap();
    item["candidate"] = original_candidate["candidate"].clone();
    item["before_md"] = original_candidate["before_md"].clone();
    item["run_entry_ref"] = json!("");
    item["run_version"] = json!(0);
    item["status"] = json!(status);
    let candidate = &mut item["candidate"];
    let expected_error = match obsolete {
        ObsoleteLocationCandidate::MissingRaw => {
            candidate["raw_sources"] = json!([]);
            candidate["content"] = json!(
                candidate["content"]
                    .as_str()
                    .unwrap()
                    .replace("[^r1]", "[^s1]")
            );
            "must cite retained raw observations"
        }
        ObsoleteLocationCandidate::SidebarUncertainty => {
            candidate["uncertainty"] =
                json!("A material historical caveat appears only outside the publishable body.");
            "summary uncertainty must appear verbatim"
        }
        ObsoleteLocationCandidate::WrongClock => {
            candidate["content"] = json!(
                candidate["content"]
                    .as_str()
                    .unwrap()
                    .replace("12:00:00", "12:00:37")
            );
            "location content validation failed"
        }
    };
    let historical: brunn::dreamer_review::Candidate =
        serde_json::from_value(candidate.clone()).unwrap();
    item["candidate_hash"] = json!(hash_token(&serde_json::to_string(&historical).unwrap()));
    let changed = sqlx::query("UPDATE brunn.entry_versions v SET metadata=$2 FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version AND v.version=$3")
            .bind(f.owner.user).bind(&stored.2).bind(stored.0).execute(&f.pool).await.unwrap();
    assert_eq!(changed.rows_affected(), 1);
    finish(
        f,
        &admitted,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(f).await;
    (from, view, expected_error)
}

#[tokio::test]
async fn report_only_approval_revalidates_obsolete_candidates_without_holding_them() {
    for obsolete in [
        ObsoleteLocationCandidate::SidebarUncertainty,
        ObsoleteLocationCandidate::MissingRaw,
        ObsoleteLocationCandidate::WrongClock,
    ] {
        let Some(f) = fixture().await else {
            return;
        };
        control(&f, "report-only", 0).await;
        let (from, view, expected_error) =
            seed_obsolete_location_candidate(&f, obsolete, "pending").await;
        let original = &view["items"][0];
        assert_eq!(view["mode"], "report-only");
        assert_eq!(original["status"], "pending");
        assert_eq!(original["reviewable"], true);
        assert_eq!(original["stale"], false);
        assert!(original["run_version"].as_i64().unwrap() > 0);
        let before = current(&f, "dreams/state.md").await.unwrap();
        let refused = post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, original, "approve"),
        )
        .await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
        assert!(
            refused.body.to_string().contains(expected_error),
            "{}",
            refused.body
        );
        assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
        assert_eq!(review(&f).await["items"], view["items"]);
        if matches!(obsolete, ObsoleteLocationCandidate::WrongClock) {
            control(&f, "full", 1).await;
            let full_view = review(&f).await;
            assert_eq!(full_view["mode"], "full");
            let refused = post(
                &f,
                &f.owner,
                "/v1/dreamer/review/decisions",
                decision(&full_view, &full_view["items"][0], "approve"),
            )
            .await;
            assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
            assert!(refused.body.to_string().contains(expected_error));
            assert_eq!(current(&f, "dreams/state.md").await.unwrap(), before);
        }
        assert!(
            current(&f, &format!("derived/location/{}.md", from.date_naive()))
                .await
                .is_none()
        );
        assert!(current(&f, "dreams/decisions.md").await.is_none());
        let review_audits: i64 = sqlx::query_scalar("SELECT count(*) FROM brunn.entries WHERE user_id=$1 AND starts_with(path,'dreams/reviews/')")
            .bind(f.owner.user).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            review_audits, 0,
            "rejected approval must not create a durable held decision"
        );
    }
}

#[tokio::test]
async fn obsolete_held_location_candidates_requeue_without_blocking_full_admission() {
    for obsolete in [
        ObsoleteLocationCandidate::SidebarUncertainty,
        ObsoleteLocationCandidate::MissingRaw,
        ObsoleteLocationCandidate::WrongClock,
    ] {
        let Some(f) = fixture().await else {
            return;
        };
        control(&f, "report-only", 0).await;
        let (from, view, expected_error) =
            seed_obsolete_location_candidate(&f, obsolete, "approved_held").await;
        let original = &view["items"][0];
        assert_eq!(original["status"], "approved_held");
        let old_run_id = Uuid::parse_str(
            original["run_entry_ref"]
                .as_str()
                .unwrap()
                .strip_prefix("entry:")
                .unwrap(),
        )
        .unwrap();
        let old_version = original["run_version"].as_i64().unwrap();
        let old_audit: (String, Value) = sqlx::query_as("SELECT content,metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
            .bind(f.owner.user).bind(old_run_id).bind(old_version).fetch_one(&f.pool).await.unwrap();
        control(&f, "full", 1).await;
        let recovered = admit(&f).await;
        assert_eq!(recovered["admitted"], true);
        assert_eq!(recovered["mode"], "full");
        assert!(
            recovered["location_work"].is_null(),
            "recovery queues the day after this attempt's location snapshot is set"
        );
        let pending = &recovered["pending"][0];
        assert_eq!(pending["status"], "needs_changes");
        for key in ["id", "candidate_hash", "run_entry_ref", "run_version"] {
            assert_eq!(pending[key], original[key]);
        }
        let state = current(&f, "dreams/state.md").await.unwrap();
        assert_eq!(
            state.2["dreamer_state"]["location_work"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            state.2["dreamer_state"]["location_work"][0]["date"],
            json!(from.date_naive())
        );
        let dispositions = &state.2["dreamer_state"]["candidate_dispositions"];
        assert_eq!(dispositions.as_array().unwrap().len(), 1);
        let disposition = &dispositions[0];
        assert_eq!(disposition["disposition"], "held_approval_invalidated");
        assert_eq!(disposition["item_id"], original["id"]);
        for key in ["candidate_hash", "run_entry_ref", "run_version"] {
            assert_eq!(disposition[key], original[key]);
        }
        assert!(
            disposition["code"]
                .as_str()
                .is_some_and(|code| !code.is_empty())
        );
        assert!(
            disposition["reason"]
                .as_str()
                .unwrap()
                .contains(expected_error)
        );
        let target = format!("derived/location/{}.md", from.date_naive());
        assert!(current(&f, &target).await.is_none());
        let (_, finished) = finish(
            &f,
            &recovered,
            recovered["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        let recovery_run_id = Uuid::parse_str(
            finished["run_entry_ref"]
                .as_str()
                .unwrap()
                .strip_prefix("entry:")
                .unwrap(),
        )
        .unwrap();
        let recorded: Value = sqlx::query_scalar("SELECT metadata->'dreamer_run'->'candidate_dispositions' FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
            .bind(f.owner.user).bind(recovery_run_id).bind(finished["run_version"].as_i64().unwrap()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            &recorded, dispositions,
            "recovery must retain its exact invalidation in immutable run audit"
        );
        let historical: (String, Value) = sqlx::query_as("SELECT content,metadata FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=$3")
            .bind(f.owner.user).bind(old_run_id).bind(old_version).fetch_one(&f.pool).await.unwrap();
        assert_eq!(historical, old_audit);
        let next = admit(&f).await;
        assert_eq!(next["location_work"]["date"], json!(from.date_naive()));
        assert_eq!(next["location_evidence"]["fingerprint_complete"], true);
        assert_eq!(next["pending"][0]["id"], original["id"]);
        assert_eq!(next["pending"][0]["status"], "needs_changes");
        assert_eq!(
            current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["candidate_dispositions"],
            json!([])
        );
        assert!(current(&f, &target).await.is_none());
        finish(
            &f,
            &next,
            next["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
    }
}

async fn age_fixture_location_work(f: &Fixture) -> Value {
    // Simulate a retained queue crossing the real retention horizon without
    // changing clocks or another owner's reports.
    let from = (Utc::now() - chrono::Duration::days(40))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let work = json!({"date":from.date_naive(),"timezone":"UTC","from":from,"to":from+chrono::Duration::days(1)});
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=jsonb_set(metadata,'{dreamer_state,location_work}',$2) FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind(json!([work.clone()])).execute(&f.pool).await.unwrap();
    work
}

#[tokio::test]
async fn expired_location_queue_is_audited_and_replaced_before_admission() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    queue_pilot(&f, from).await;
    let expired = age_fixture_location_work(&f).await;
    let queued = queue_pilot(&f, from).await;
    let state = current(&f, "dreams/state.md").await.unwrap();
    let data = &state.2["dreamer_state"];
    assert_eq!(
        data["location_work"],
        json!([queued["data"]["work"].clone()])
    );
    let disposition = &data["location_dispositions"][0];
    assert_eq!(disposition["disposition"], "raw_retention_expired");
    assert_eq!(disposition["work"], expired);
    let expected_expiry = chrono::DateTime::parse_from_rfc3339(expired["from"].as_str().unwrap())
        .unwrap()
        .to_utc()
        + chrono::Duration::days(30);
    assert_eq!(disposition["expired_at"], json!(expected_expiry));
    assert_eq!(data["active"], Value::Null);
    assert_eq!(data["last_attempt"], Value::Null);
    assert!(current(&f, "dreams/latest-receipt.md").await.is_none());
    let repeated = queue_pilot(&f, from).await;
    assert_eq!(repeated["data"]["no_op"], true);
    assert_eq!(
        repeated["data"]["state_version"],
        queued["data"]["state_version"]
    );
    let retained = admit(&f).await;
    assert_eq!(retained["location_work"]["date"], json!(from.date_naive()));
    assert_eq!(retained["location_evidence"]["fingerprint_complete"], true);
    finish(
        &f,
        &retained,
        retained["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let run = current(&f, &format!("dreams/runs/{}.md", date()))
        .await
        .unwrap();
    assert_eq!(
        run.2["dreamer_run"]["location_dispositions"],
        json!([disposition.clone()])
    );
}

#[tokio::test]
async fn expired_location_queue_is_audited_on_admission_without_relabeling_the_day() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    queue_pilot(&f, from).await;
    let expired = age_fixture_location_work(&f).await;
    let admitted = admit(&f).await;
    assert_eq!(admitted["location_work"], Value::Null);
    assert_eq!(admitted["location_evidence"], Value::Null);
    assert_eq!(admitted["inputs"], json!([]));
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(state.2["dreamer_state"]["location_work"], json!([]));
    assert_eq!(
        state.2["dreamer_state"]["location_dispositions"][0]["work"],
        expired
    );
    finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    assert!(
        current(
            &f,
            &format!("derived/location/{}.md", expired["date"].as_str().unwrap())
        )
        .await
        .is_none()
    );
    queue_pilot(&f, from).await;
    age_fixture_location_work(&f).await;
    let deduped = ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({"attempt_id":Uuid::now_v7(),"date":date(),"kind":"scheduled"}),
    )
    .await);
    assert_eq!(deduped["admitted"], false);
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(state.2["dreamer_state"]["location_work"], json!([]));
    assert_eq!(
        state.2["dreamer_state"]["location_dispositions"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "replayed original expiry has one deterministic disposition"
    );
    queue_pilot(&f, from).await;
}

#[tokio::test]
async fn changed_location_evidence_revokes_held_approval_before_report_only_revision() {
    for explicit_requeue in [false, true] {
        let Some(f) = fixture().await else {
            return;
        };
        control(&f, "report-only", 0).await;
        let (from, _, _) = seed_location_pilot(&f).await;
        queue_pilot(&f, from).await;
        let first = admit(&f).await;
        let (_, accepted) = submit(
            &f,
            &first,
            first["state_version"].as_i64().unwrap(),
            vec![pilot_candidate(&first)],
        )
        .await;
        finish(
            &f,
            &first,
            accepted["state_version"].as_i64().unwrap(),
            "completed",
        )
        .await;
        let view = review(&f).await;
        let original = view["items"][0].clone();
        let approval = decision(&view, &original, "approve");
        let approved = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            approval.clone(),
        )
        .await);
        assert_eq!(approved["data"]["application_status"], "approved_held");
        if explicit_requeue {
            queue_pilot(&f, from).await;
        }
        let generation: i64 = sqlx::query_scalar(
            "SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1",
        )
        .bind(f.owner.user)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m,arrived_at,departed_at) VALUES($1,$2,'visit_departure',0,47.002,-122.002,8,$3,$4)").bind(f.owner.user).bind(from+chrono::Duration::hours(26)).bind(from+chrono::Duration::hours(14)).bind(from+chrono::Duration::hours(15)).execute(&f.pool).await.unwrap();
        let unchanged: i64 = sqlx::query_scalar(
            "SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1",
        )
        .bind(f.owner.user)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(generation, unchanged);
        let next = admit(&f).await;
        assert_eq!(next["mode"], "report-only");
        assert_eq!(next["pending"][0]["status"], "needs_changes");
        for key in ["id", "candidate_hash", "run_entry_ref", "run_version"] {
            assert_eq!(next["pending"][0][key], original[key]);
        }
        assert_eq!(
            next["location_work"]["date"],
            first["location_work"]["date"]
        );
        assert_ne!(
            next["location_work"]["fingerprint"],
            first["location_work"]["fingerprint"]
        );
        let state = current(&f, "dreams/state.md").await.unwrap();
        let invalidation = &state.2["dreamer_state"]["location_dispositions"][0];
        assert_eq!(invalidation["disposition"], "held_approval_invalidated");
        assert_eq!(invalidation["candidate_hash"], original["candidate_hash"]);
        assert_eq!(invalidation["work"], first["location_work"]);
        let target = format!("derived/location/{}.md", from.date_naive());
        assert!(current(&f, &target).await.is_none());
        let mut revised = pilot_candidate(&next);
        revised["revises_item_id"] = original["id"].clone();
        let (_, recompiled) = submit(
            &f,
            &next,
            next["state_version"].as_i64().unwrap(),
            vec![revised],
        )
        .await;
        assert_eq!(
            recompiled["accepted_candidate_ids"],
            json!([original["id"].clone()])
        );
        finish(
            &f,
            &next,
            recompiled["state_version"].as_i64().unwrap(),
            "completed",
        )
        .await;
        let updated = review(&f).await;
        assert_eq!(updated["items"][0]["status"], "pending");
        assert_eq!(updated["items"][0]["stale"], false);
        assert_ne!(
            updated["items"][0]["candidate_hash"],
            original["candidate_hash"]
        );
        ok(post(&f, &f.owner, "/v1/dreamer/review/decisions", approval).await);
        assert_eq!(
            review(&f).await["items"][0]["status"],
            "pending",
            "replaying approval for the old exact candidate cannot reapprove its revision"
        );
        let renewed = ok(post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&updated, &updated["items"][0], "approve"),
        )
        .await);
        assert_eq!(renewed["data"]["application_status"], "approved_held");
        assert!(current(&f, &target).await.is_none());
    }
}

#[tokio::test]
async fn full_review_inbox_accepts_existing_revision_and_rejects_atomic_overflow() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let source = write(
        &f,
        "sources/Projects/Capacity.md",
        "# Capacity\n\nAn exact source for retained candidates.\n",
        0,
    )
    .await;
    for batch in 0..6 {
        let admitted = admit(&f).await;
        let candidates = (0..16)
            .map(|n| candidate(&source, &format!("capacity-{}", batch * 16 + n)))
            .collect();
        let (_, accepted) = submit(
            &f,
            &admitted,
            admitted["state_version"].as_i64().unwrap(),
            candidates,
        )
        .await;
        finish(
            &f,
            &admitted,
            accepted["state_version"].as_i64().unwrap(),
            "completed",
        )
        .await;
    }
    let next = admit(&f).await;
    assert_eq!(next["pending"].as_array().unwrap().len(), 96);
    let original = &next["pending"][0];
    let mut revision = candidate(&source, "capacity-0");
    revision["revises_item_id"] = original["id"].clone();
    revision["reason"] = json!("An updated explanation preserves the original proposal identity.");
    let version = next["state_version"].as_i64().unwrap();
    for list in [
        vec![revision.clone(), revision.clone()],
        vec![revision.clone(), candidate(&source, "capacity-overflow")],
    ] {
        let mut body = attempt(&next, version);
        body["candidates"] = json!(list);
        body["processed_inputs"] = next["inputs"].clone();
        let rejected = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
        assert_eq!(
            rejected.status,
            StatusCode::BAD_REQUEST,
            "{}",
            rejected.body
        );
        let unchanged = current(&f, "dreams/state.md").await.unwrap();
        assert_eq!(unchanged.0, version);
        assert_eq!(
            unchanged.2["dreamer_state"]["items"][0]["candidate_hash"],
            original["candidate_hash"]
        );
    }
    let (_, accepted) = submit(&f, &next, version, vec![revision]).await;
    assert_eq!(
        accepted["accepted_candidate_ids"],
        json!([original["id"].clone()])
    );
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(
        state.2["dreamer_state"]["items"].as_array().unwrap().len(),
        96
    );
    assert_eq!(state.2["dreamer_state"]["items"][0]["id"], original["id"]);
    assert_eq!(
        state.2["dreamer_state"]["items"][0]["created_at"],
        original["created_at"]
    );
    assert_ne!(
        state.2["dreamer_state"]["items"][0]["candidate_hash"],
        original["candidate_hash"]
    );
    finish(
        &f,
        &next,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
}

#[tokio::test]
async fn historical_location_pilot_flows_from_queue_to_held_review_and_full_publication() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "report-only", 0).await;
    let (from, path, content) = seed_location_pilot(&f).await;
    queue_pilot(&f, from).await;
    let repeated = queue_pilot(&f, from).await;
    assert_eq!(repeated["data"]["no_op"], true);
    let first = admit(&f).await;
    assert_eq!(
        first["inputs"],
        json!([]),
        "canonical location evidence is separate from ordinary input intake"
    );
    let candidate = pilot_candidate(&first);
    let (_, accepted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![candidate],
    )
    .await;
    let state = current(&f, "dreams/state.md").await.unwrap();
    assert_eq!(
        state.2["dreamer_state"]["location_work"],
        json!([]),
        "accepted candidate consumes the retained location compilation work"
    );
    finish(
        &f,
        &first,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let view = review(&f).await;
    let item = &view["items"][0];
    assert_eq!(item["reviewable"], true);
    assert_eq!(item["stale"], false);
    assert!(item["sources"].as_array().unwrap().iter().any(|s| {
        s["entry_ref"]
            .as_str()
            .is_some_and(|r| r.starts_with("location-report:"))
    }));
    let raw_preview = item["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| {
            s["entry_ref"]
                .as_str()
                .is_some_and(|r| r.starts_with("location-report:"))
        })
        .unwrap();
    assert!(
        raw_preview["excerpt"]
            .as_str()
            .unwrap()
            .contains("accuracy_m: 5")
    );
    assert!(
        raw_preview["excerpt"]
            .as_str()
            .unwrap()
            .contains("first_received_at: null")
    );
    assert!(raw_preview.get("version").is_none());
    assert!(raw_preview.get("path").is_none());
    assert!(
        item["candidate"]["after_md"]
            .as_str()
            .unwrap()
            .contains("Point samples")
    );
    assert!(
        !item["candidate"]["after_md"]
            .as_str()
            .unwrap()
            .contains("[^")
    );
    let approved = ok(post(
        &f,
        &f.owner,
        "/v1/dreamer/review/decisions",
        decision(&view, item, "approve"),
    )
    .await);
    assert_eq!(approved["data"]["application_status"], "approved_held");
    let target = format!("derived/location/{}.md", from.date_naive());
    assert!(current(&f, &target).await.is_none());
    let next = from + chrono::Duration::days(1) + chrono::Duration::hours(12);
    let appended = format!(
        "{content}| {} | {} | 1h | Unrelated later stop | visit | Seattle | medium | 47.1000,-122.1000 |\n",
        next.format("%Y-%m-%dT%H:%M%:z"),
        (next + chrono::Duration::hours(1)).format("%Y-%m-%dT%H:%M%:z")
    );
    write(&f, &path, &appended, 1).await;
    control(&f, "full", 1).await;
    let second = admit(&f).await;
    assert!(second["location_work"].is_null());
    let output = current(&f, &target).await.unwrap();
    assert_eq!(output.0, 1);
    assert_eq!(
        output.2["dreamer_summary"]["evidence_scope"]["sources_validated"],
        true
    );
    assert_eq!(
        output.2["dreamer_summary"]["sources"][0]["version"], 1,
        "unrelated month append preserves exact original citation"
    );
    assert_eq!(
        output.2["dreamer_summary"]["raw_sources"][0]["natural_key"],
        first["location_evidence"]["reports"][0]["natural_key"]
    );
    let (_, finished) = finish(
        &f,
        &second,
        second["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    assert_eq!(
        finished["latest_receipt"]["applied_writes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn historical_location_pilot_late_raw_change_blocks_owner_approval() {
    let Some(f) = fixture().await else {
        return;
    };
    control(&f, "full", 0).await;
    let (from, _, _) = seed_location_pilot(&f).await;
    let forbidden = post(
        &f,
        &f.model,
        "/v1/dreamer/review/location-pilot",
        json!({"date":from.date_naive(),"timezone":"UTC"}),
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    queue_pilot(&f, from).await;
    let first = admit(&f).await;
    let (_, accepted) = submit(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        vec![pilot_candidate(&first)],
    )
    .await;
    finish(
        &f,
        &first,
        accepted["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
    let generation: i64 =
        sqlx::query_scalar("SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1")
            .bind(f.owner.user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m,arrived_at,departed_at) VALUES($1,$2,'visit_departure',0,47.002,-122.002,8,$3,$4)").bind(f.owner.user).bind(from+chrono::Duration::hours(26)).bind(from+chrono::Duration::hours(14)).bind(from+chrono::Duration::hours(15)).execute(&f.pool).await.unwrap();
    let unchanged: i64 =
        sqlx::query_scalar("SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1")
            .bind(f.owner.user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(generation, unchanged);
    let view = review(&f).await;
    let item = &view["items"][0];
    assert_eq!(item["stale"], true);
    assert_eq!(
        post(
            &f,
            &f.owner,
            "/v1/dreamer/review/decisions",
            decision(&view, item, "approve")
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    assert!(
        current(&f, &format!("derived/location/{}.md", from.date_naive()))
            .await
            .is_none()
    );
    let next = admit(&f).await;
    assert_eq!(
        next["location_work"]["date"], first["location_work"]["date"],
        "late raw-only evidence requeues its original tracked day"
    );
    assert_ne!(
        next["location_work"]["fingerprint"],
        first["location_work"]["fingerprint"]
    );
    assert_eq!(
        next["location_work"]["fingerprint"],
        next["location_evidence"]["evidence_fingerprint"]
    );
    assert_eq!(
        next["inputs"], first["inputs"],
        "location requeue does not invent ordinary retained inputs"
    );
    let mut revised = pilot_candidate(&next);
    revised["revises_item_id"] = item["id"].clone();
    let (_, recompiled) = submit(
        &f,
        &next,
        next["state_version"].as_i64().unwrap(),
        vec![revised],
    )
    .await;
    assert_eq!(
        recompiled["accepted_candidate_ids"],
        json!([item["id"].clone()]),
        "recompilation retains the original review identity"
    );
    finish(
        &f,
        &next,
        recompiled["state_version"].as_i64().unwrap(),
        "completed",
    )
    .await;
}

#[tokio::test]
async fn unavailable_change_pages_advance_without_consuming_retained_evidence() {
    let Some(f) = fixture().await else {
        return;
    };
    const MARKER: &str = "UNAVAILABLE_SOURCE_BODY_MUST_NOT_REACH_MODEL";
    control(&f, "report-only", 0).await;
    let original = write(
        &f,
        "sources/Unavailable/Retained.md",
        &format!("# Retained\n\n{MARKER}\n"),
        0,
    )
    .await;
    let first = admit(&f).await;
    assert_eq!(first["inputs"].as_array().unwrap().len(), 1);
    finish(
        &f,
        &first,
        first["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let never_admitted = write(
        &f,
        "sources/Unavailable/Unseen.md",
        &format!("# Unseen\n\n{MARKER}\n"),
        0,
    )
    .await;

    // Reproduce imported historical paths that remain visible after their
    // entries become task-managed. The actual RLS policies then hide all of
    // these versions from the runner's internal read/save lane and model.
    // A full page must not disappear at an inner version join and stall.
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT e.user_id,e.id,e.current_version,'update',e.path,v.content_sha256 FROM generate_series(1,1000) n CROSS JOIN brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND starts_with(e.path,'sources/Unavailable/') ORDER BY n,e.path")
        .bind(f.owner.user).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE brunn.entries SET path='.brunn/tasks/'||id::text||'.md' WHERE user_id=$1 AND starts_with(path,'sources/Unavailable/')")
        .bind(f.owner.user).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT e.user_id,e.id,e.current_version,'update',e.path,v.content_sha256 FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND starts_with(e.path,'.brunn/tasks/')")
        .bind(f.owner.user).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let readable = write(
        &f,
        "sources/AfterUnavailable/Readable.md",
        "# Readable\n\nA supported observation after the unavailable page.\n",
        0,
    )
    .await;

    let hidden_read = ok(post(
        &f,
        &f.model,
        "/v1/workspace/read",
        json!({"requests":[{"ref":original["entry_ref"],"version":1},{"ref":never_admitted["entry_ref"],"version":1}]}),
    )
    .await);
    assert_eq!(hidden_read["data"]["missing_requests"], 2);
    assert!(
        hidden_read["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["status"] == "not_found")
    );
    assert!(!hidden_read.to_string().contains(MARKER));

    let mut cursor = first["scanned_generation"].as_i64().unwrap();
    let mut reached_readable = false;
    // The bounded disposition budget may split one 2,000-event page across
    // attempts. Each attempt must advance without evicting prior input.
    for round in 0..18 {
        let admitted = admit(&f).await;
        let next_cursor = admitted["scanned_generation"].as_i64().unwrap();
        assert!(next_cursor > cursor, "unavailable page stalled at {cursor}");
        cursor = next_cursor;
        let inputs = admitted["inputs"].as_array().unwrap();
        assert!(inputs.contains(&first["inputs"][0]));
        assert!(
            !inputs
                .iter()
                .any(|i| i["entry_ref"] == never_admitted["entry_ref"])
        );
        assert_eq!(
            admitted["processed_generation"],
            first["processed_generation"]
        );
        assert!(!admitted.to_string().contains(MARKER));
        let stored = current(&f, "dreams/state.md").await.unwrap();
        let dispositions = stored.2["dreamer_state"]["source_dispositions"]
            .as_array()
            .unwrap();
        assert!(!dispositions.is_empty());
        assert!(dispositions.len() <= 128);
        assert!(
            dispositions
                .iter()
                .all(|d| d["disposition"] == "source_unavailable")
        );

        if round == 0 {
            assert_eq!(
                admitted["inputs"], first["inputs"],
                "first page is wholly unavailable"
            );
            let mut body = attempt(&admitted, admitted["state_version"].as_i64().unwrap());
            body["candidates"] = json!([candidate(&original, "unavailable-rejected")]);
            body["processed_inputs"] = json!([]);
            let refused = post(&f, &f.runner, "/v1/workspace/dreamer/candidates", body).await;
            assert_eq!(refused.status, StatusCode::BAD_REQUEST);
            assert!(refused.body.to_string().contains("missing or inaccessible"));
        }
        reached_readable = inputs
            .iter()
            .any(|i| i["entry_ref"] == readable["entry_ref"]);
        let (_, finished) = finish(
            &f,
            &admitted,
            admitted["state_version"].as_i64().unwrap(),
            "partial",
        )
        .await;
        assert_eq!(finished["counts"]["processed"], 0);
        assert_eq!(finished["counts"]["retained"], inputs.len());
        if reached_readable {
            break;
        }
    }
    assert!(
        reached_readable,
        "unavailable events prevented the next readable source from admission"
    );
}

mod subject_research;

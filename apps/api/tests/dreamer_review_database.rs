//! Real HTTP/database contract for narrow runner authority and owner review.
//! Fixture users only, hashing embeddings, no model or push transport.
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use brunn::{AppState, Config, auth::hash_token, router};
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::{Value, json};
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

async fn expire_fixture_lease(f: &Fixture) {
    sqlx::query("UPDATE brunn.entry_versions v SET metadata=jsonb_set(metadata,'{dreamer_state,active,lease_until}',to_jsonb($2::text)) FROM brunn.entries e WHERE e.user_id=$1 AND e.path='dreams/state.md' AND v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version")
        .bind(f.owner.user).bind((Utc::now()-chrono::Duration::seconds(1)).to_rfc3339()).execute(&f.pool).await.unwrap();
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
    let a = admit(&f).await;
    let mut competing_target = candidate(&source, "stale-target");
    competing_target["path"] = json!("derived/entities/publish.md");
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
    let from = (Utc::now() - chrono::Duration::days(2))
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

fn pilot_candidate(admission: &Value) -> Value {
    let work = &admission["location_work"];
    let packet = &admission["location_evidence"];
    assert_eq!(packet["fingerprint_complete"], true);
    assert_eq!(work["fingerprint"], packet["evidence_fingerprint"]);
    let document = &packet["canonical_months"][0];
    let selector = &document["selectors"][0];
    json!({"kind":"summary","title":"Historical location evidence","summary":"A bounded historical day with exact sources.","reason":"Review a sourced reconstruction with explicit coverage uncertainty.",
        "path":format!("derived/location/{}.md",work["date"].as_str().unwrap()),"expected_version":0,
        "content":"# Historical location evidence\n\nA bounded stop is recorded in canonical history.[^s1]\nThe retained sample has five-meter reported accuracy and does not establish continuous presence.[^r1]\n",
        "sources":[{"entry_ref":document["ref"],"version":document["version"],"start_line":selector["start_line"],"end_line":selector["end_line"]}],
        "raw_sources":[{"natural_key":packet["reports"][0]["natural_key"],"fields":["at","lat","lon","accuracy_m","first_received_at"]}],
        "evidence_scope":{"from":work["from"],"to":work["to"],"timezone":work["timezone"],"fingerprint":work["fingerprint"]}})
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
            .contains("[^r1]: Retained location report")
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

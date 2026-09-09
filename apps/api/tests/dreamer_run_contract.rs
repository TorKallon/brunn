//! End-to-end wrapper contract against a fake HTTP API and Codex process.
//! Real transaction/fence validation lives in dreamer_review database tests.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use brunn::dreamer::{
    receipt,
    run::{AUTH_SECRET, CONTROL_PATH, Dreamer, DreamerConfig, RunKind, RunOutcome},
};
use chrono::NaiveDate;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const SOURCE: &str = "entry:019fba27-687b-7582-8b99-e9371dbe2ce5";
const RUN: &str = "entry:01a07b52-2bf0-7d62-8969-9ea0b3c49399";
const SECRET_REF: &str = "secret:01a07b52-2bf0-7d62-8969-9ea0b3c49388";
const WORKSPACE_TOKEN: &str = "brunn-test-workspace-secret-should-never-reach-model";
const RUNNER_TOKEN: &str = "brunn-test-runner-secret-should-never-reach-model";
#[derive(Default)]
struct Mock {
    control: Option<String>,
    secrets: BTreeMap<String, (String, i64)>,
    writes: usize,
    run_version: i64,
    state_version: i64,
    admissions: usize,
    admission_requests: Vec<Value>,
    submissions: usize,
    submitted: Vec<Value>,
    auth_puts: usize,
    runs: Vec<Value>,
    latest: Option<Value>,
    notifications: Vec<Value>,
    reject_candidates: bool,
    reject_narrative: bool,
    review_conflicts: usize,
    fail_finish: bool,
    fail_auth_put: bool,
    fail_notify: bool,
    retained_location_work: bool,
    location_admission: Option<Value>,
    model_read_only: bool,
    pending: Vec<Value>,
    prior_pending_notification: Option<Value>,
    discoveries: Vec<Value>,
    narrative_discoveries: Vec<Value>,
    narrative_context: Vec<Value>,
    current_admission: Option<Value>,
    research_jobs: Vec<Value>,
    research_enabled: bool,
    research_next_count: usize,
    research_progress: Vec<Value>,
}
#[path = "dreamer_run_contract/research_contract.rs"]
mod research_contract;
type Shared = Arc<Mutex<Mock>>;
fn error(code: StatusCode, message: &str) -> Response {
    (code, Json(json!({"error":{"message":message}}))).into_response()
}
async fn read(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let s = shared.lock().unwrap();
    let path = body["requests"][0]["path"].as_str().unwrap();
    let item = if path == CONTROL_PATH {
        s.control.as_ref().map(|c| json!({"text":c,"version":1}))
    } else {
        None
    };
    Json(json!({"data":{"items":[item.unwrap_or(json!({"status":"not_found"}))]}}))
}
async fn me(State(shared): State<Shared>) -> Json<Value> {
    let s = shared.lock().unwrap();
    Json(
        json!({"read_only":s.model_read_only,"capabilities":if s.model_read_only {vec!["read","open","query","compute","verify","status","task.read","message.read"]}else{vec!["read","save"]}}),
    )
}
async fn secret_get(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let s = shared.lock().unwrap();
    let name = body["name"].as_str().unwrap();
    match s.secrets.get(name) {
        Some((value, version)) => {
            Json(json!({"secret_ref":SECRET_REF,"value":value,"version":version})).into_response()
        }
        None => error(StatusCode::NOT_FOUND, "missing"),
    }
}
async fn location_discover(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut s = shared.lock().unwrap();
    s.state_version += 1;
    s.discoveries.push(body.clone());
    let mut value = s.location_admission.clone().unwrap();
    value["attempt_id"] = body["attempt_id"].clone();
    value["session_id"] = json!(format!("session:{}", body["attempt_id"].as_str().unwrap()));
    value["fence"] = body["fence"].clone();
    value["state_version"] = json!(s.state_version);
    value["frozen_generation"] = json!(17);
    value["pending"] = json!(s.pending);
    value["mode"] = json!("report-only");
    value["location_work"]["discovery"] =
        json!({"request_hash":format!("discovery-{}",s.discoveries.len())});
    if value.get("location_context").is_none() {
        value["location_context"] = json!([]);
    }
    Json(json!({"data":value}))
}
async fn secret_put(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut s = shared.lock().unwrap();
    let name = body["name"].as_str().unwrap();
    if name == AUTH_SECRET {
        s.auth_puts += 1;
    }
    if name == AUTH_SECRET && s.fail_auth_put {
        return error(StatusCode::CONFLICT, "secret changed");
    }
    let actual = s.secrets.get(name).map_or(0, |(_, v)| *v);
    if body["expected_version"]
        .as_i64()
        .is_some_and(|v| v != actual)
    {
        return error(StatusCode::CONFLICT, "secret changed");
    }
    s.secrets.insert(
        name.into(),
        (body["value"].as_str().unwrap().into(), actual + 1),
    );
    Json(json!({"version":actual+1})).into_response()
}
async fn narrative_discover(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut s = shared.lock().unwrap();
    s.narrative_discoveries.push(body.clone());
    s.state_version += 1;
    let mut value = s.current_admission.clone().unwrap();
    value["state_version"] = json!(s.state_version);
    value["narrative_context"] = json!(s.narrative_context);
    value["narrative_discovery"] =
        json!({"request_hash":"narrative-fixture","queries":body["queries"]});
    if s.research_enabled {
        research_contract::assert_operation(&body);
        value["research"]["sources"] = json!(s.narrative_context);
        if body["queries"] == json!([])
            && body["targets"] == json!([])
            && let Some(cursor) = value["research"]["coverage"]["change_cursor"].as_i64()
        {
            let upper = value["research"]["coverage"]["change_upper"]
                .as_i64()
                .unwrap();
            let next = (cursor + 1000).min(upper);
            value["research"]["coverage"]["change_cursor"] = json!(next);
            if next == upper {
                value["research"]["coverage"]["change_status"] = json!("complete");
            }
        }
        value["research"]["version"] = json!(value["research"]["version"].as_i64().unwrap() + 1);
        s.current_admission = Some(value.clone());
    }
    Json(json!({"data":value}))
}
async fn write(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    if headers.get("authorization").unwrap() == "Bearer model" {
        return error(StatusCode::FORBIDDEN, "read only");
    }
    shared.lock().unwrap().writes += 1;
    Json(json!({})).into_response()
}
async fn admit(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut s = shared.lock().unwrap();
    s.admission_requests.push(body.clone());
    if body["kind"] == "nightly"
        && s.latest
            .as_ref()
            .is_some_and(|v| v["status"] == "completed")
    {
        return Json(json!({"admitted":false,"reason":"scheduled date already completed"}));
    }
    s.admissions += 1;
    s.state_version += 1;
    s.writes += 2;
    let mut response = json!({"admitted":true,"session_id":format!("session:{}",body["attempt_id"].as_str().unwrap()),"attempt_id":body["attempt_id"],"fence":s.admissions,"state_version":s.state_version,"mode":"report-only","frozen_generation":17,"scanned_generation":17,"processed_generation":0,
      "inputs":[{"entry_ref":SOURCE,"path":"sources/Project.md","version":2,"generation":17,"operation":"update","content_hash":"sha256:source"}],
      "pending":s.pending,"pending_notifications":s.prior_pending_notification.iter().collect::<Vec<_>>()});
    if let Some(location) = &s.location_admission {
        response
            .as_object_mut()
            .unwrap()
            .extend(location.as_object().unwrap().clone());
    }
    if s.research_enabled {
        response["research_protocol"] = json!(1);
    }
    s.current_admission = Some(response.clone());
    Json(response)
}
async fn checkpoint(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut s = shared.lock().unwrap();
    if body["expected_state_version"] != s.state_version {
        return error(StatusCode::CONFLICT, "state changed");
    }
    s.state_version += 1;
    Json(json!({"state_version":s.state_version})).into_response()
}
async fn candidates(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut s = shared.lock().unwrap();
    if body["subject_ref"].is_string() {
        research_contract::assert_operation(&body);
    }
    s.submissions += 1;
    s.submitted.push(body.clone());
    if s.review_conflicts > 0 {
        s.review_conflicts -= 1;
        s.state_version += 1;
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":{
                "message":"Review or run state changed; reload before retrying",
                "details":{"actual_version":s.state_version}
            }})),
        )
            .into_response();
    }
    if s.reject_candidates || (s.reject_narrative && s.submissions > 1) {
        return error(StatusCode::CONFLICT, "source changed");
    }
    let ids: Vec<_> = body["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(n, candidate)| {
            if candidate["evidence_scope"].is_object() {
                s.retained_location_work = false;
            }
            if let Some(id) = candidate["revises_item_id"].as_str() {
                let item = s.pending.iter_mut().find(|item| item["id"] == id).unwrap();
                item["candidate"] = candidate.clone();
                item["status"] = json!("pending");
                id.to_owned()
            } else {
                s.pending.push(candidate.clone());
                format!("{}/{}", body["attempt_id"].as_str().unwrap(), n)
            }
        })
        .collect();
    s.state_version += 1;
    s.run_version += 1;
    s.writes += 2;
    let mut result = json!({"state_version":s.state_version,"run_entry_ref":RUN,"run_version":s.run_version,"accepted_candidate_ids":ids,"pending_count":s.pending.len()});
    if s.research_enabled {
        let mut current = s.current_admission.clone().unwrap();
        let processed = body["processed_inputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(inputs) = current["inputs"].as_array_mut() {
            inputs.retain(|input| {
                !processed
                    .iter()
                    .any(|p| input["entry_ref"] == p["entry_ref"])
            });
        }
        current["state_version"] = json!(s.state_version);
        result
            .as_object_mut()
            .unwrap()
            .extend(current.as_object().unwrap().clone());
        s.current_admission = Some(current);
    }
    Json(result).into_response()
}
async fn finish(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut s = shared.lock().unwrap();
    if s.fail_finish {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable");
    }
    s.run_version += 1;
    s.writes += 3;
    let terminal_outcome = if s.retained_location_work && body["outcome"] == "completed" {
        json!("partial")
    } else {
        body["outcome"].clone()
    };
    let latest = json!({"schema":receipt::SCHEMA,"run_id":"2026-09-07","receipt_ref":RUN,"receipt_version":s.run_version,"receipt_path":"dreams/runs/2026-09-07.md","status":terminal_outcome,"completed_at":receipt::format_timestamp(chrono::DateTime::parse_from_rfc3339(body["completed_at"].as_str().unwrap()).unwrap().with_timezone(&chrono::Utc)),"mode":"report-only","runner":"brunn-rust-dreamer","mode_flip":false,"probe_monitoring":null,"applied_writes":[],"entering_veto_window_today":[],"pending_owner":[],"pending_review_surfaces":[],"next_run_at":"2030-01-01T10:00:00Z"});
    s.latest = Some(latest.clone());
    s.runs.push(body.clone());
    if body["notification"]["status"] == "failed" {
        s.prior_pending_notification = Some(body["notification"].clone());
    } else if body["notification"]["status"] == "accepted" {
        s.prior_pending_notification = None;
    }
    Json(json!({"state_version":s.state_version,"run_entry_ref":RUN,"run_version":s.run_version,"latest_receipt":latest,"counts":{"pending":s.pending.len(),"published":0}})).into_response()
}
async fn notify(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut s = shared.lock().unwrap();
    s.notifications.push(body.clone());
    if s.fail_notify {
        return error(StatusCode::FORBIDDEN, "notification denied");
    }
    assert_eq!(body["importance"], "important");
    Json(json!({"notification_ref":"notification:one","replayed":false,"delivery_count":0,"delivery_status":"no_installations"})).into_response()
}
async fn build(behavior: &str) -> (Shared, Dreamer, tempfile::TempDir) {
    build_with_budget(behavior, Duration::from_secs(2)).await
}
async fn build_with_budget(
    behavior: &str,
    budget: Duration,
) -> (Shared, Dreamer, tempfile::TempDir) {
    let shared = Arc::new(Mutex::new(Mock {
        model_read_only: true,
        ..Mock::default()
    }));
    let app = Router::new()
        .route("/v1/me", get(me))
        .route("/v1/workspace/read", post(read))
        .route("/v1/workspace/write", post(write))
        .route("/v1/workspace/secrets/get", post(secret_get))
        .route("/v1/workspace/secrets/put", post(secret_put))
        .route("/v1/workspace/dreamer/admit", post(admit))
        .route("/v1/workspace/dreamer/checkpoint", post(checkpoint))
        .route(
            "/v1/workspace/dreamer/research-next",
            post(research_contract::next),
        )
        .route(
            "/v1/workspace/dreamer/research-progress",
            post(research_contract::progress),
        )
        .route(
            "/v1/workspace/dreamer/location-discover",
            post(location_discover),
        )
        .route("/v1/workspace/dreamer/candidates", post(candidates))
        .route(
            "/v1/workspace/dreamer/narrative-discover",
            post(narrative_discover),
        )
        .route("/v1/workspace/dreamer/finish", post(finish))
        .route("/v1/workspace/notifications/publish", post(notify))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let stub = stub(dir.path(), behavior);
    let dreamer = Dreamer::new(DreamerConfig {
        api_url: format!("http://{address}"),
        workspace_token: WORKSPACE_TOKEN.into(),
        model_token: "model".into(),
        runner_token: RUNNER_TOKEN.into(),
        codex_path: stub,
        codex_model: "test-model".into(),
        mcp_server_entry: PathBuf::from("/dev/null"),
        work_root: dir.path().join("work"),
        host_env: BTreeMap::from([
            ("PATH".into(), std::env::var("PATH").unwrap()),
            ("OPENAI_API_KEY".into(), "must-not-escape".into()),
        ]),
        time_budget_override: Some(budget),
    });
    (shared, dreamer, dir)
}
fn stub(dir: &Path, behavior: &str) -> PathBuf {
    let path = dir.join("codex");
    std::fs::write(dir.join("behavior.sh"), behavior).unwrap();
    let script = format!(
        r#"#!/bin/sh
case "$1" in
 login) echo 'Logged in using ChatGPT'; exit 0;;
 --version) echo 'codex-cli 0.153.4'; exit 0;;
esac
DIR='{dir}'
export DIR
while [ "$#" -gt 0 ]; do
 if [ "$1" = '--output-last-message' ]; then shift; OUTPUT_PATH="$1"; fi
 shift
done
OUTPUT_NAME=${{OUTPUT_PATH##*/}}
export OUTPUT_PATH OUTPUT_NAME
printf '%s\n' "$OUTPUT_NAME" >> "$DIR/calls"
printf '%s\n' "$OUTPUT_PATH" >> "$DIR/output-paths"
cat > "$DIR/prompt"
cp "$DIR/prompt" "$DIR/prompt-$OUTPUT_NAME"
env > "$DIR/model-env"
cp "$DIR/model-env" "$DIR/env-$OUTPUT_NAME"
if [ "$OUTPUT_NAME" = 'narrative-discovery-answer.md' ] && [ ! -f "$DIR/custom-narrative-discovery" ]; then
 echo '{{"schema":"dream.narrative.discovery.v1","queries":[]}}' > "$OUTPUT_PATH"
 exit 0
fi
exec /bin/sh "$DIR/behavior.sh"
"#,
        dir = dir.display()
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}
const HAPPY: &str = r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
cat > "$OUTPUT_PATH" <<'JSON'
{"schema":"dream.candidates.v1","candidates":[{"kind":"summary","title":"Project summary","summary":"Current project status","reason":"Consolidate the project evidence","path":"derived/entities/project.md","content":"Observed: project is active.[^s1]\nNo uncertainty identified.[^s1]","expected_version":0,"sources":[{"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce5","version":2,"start_line":1,"end_line":2}],"uncertainty":"No uncertainty identified"}],"processed_inputs":[{"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce5","version":2,"generation":17}],"findings":[]}
JSON
"#;
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()
}
fn enable(s: &Shared) {
    let mut s = s.lock().unwrap();
    s.control = Some("enabled: true\nmode: report-only\nadvance_after: 2020-01-01\n".into());
    s.secrets.insert(
        AUTH_SECRET.into(),
        ("{\"tokens\":{\"access_token\":\"old\"}}".into(), 1),
    );
}

#[tokio::test]
async fn control_off_is_zero_workspace_writes() {
    let (s, d, _dir) = build(HAPPY).await;
    for control in [
        None,
        Some("enabled: false\nmode: report-only\nadvance_after: 2020-01-01\n"),
        Some("garbage"),
    ] {
        s.lock().unwrap().control = control.map(str::to_owned);
        let report = d.run_once(today(), RunKind::Nightly).await;
        assert!(matches!(report.outcome, RunOutcome::Disabled { .. }));
        assert_eq!(s.lock().unwrap().writes, 0);
    }
}
#[tokio::test]
async fn accepted_candidates_have_exact_receipt_and_read_only_model() {
    let (s, d, dir) = build(HAPPY).await;
    enable(&s);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.receipt_persistence, "accepted");
    assert_eq!(report.auth_persistence, "verified");
    assert!(!report.mode_flipped);
    let env = std::fs::read_to_string(dir.path().join("model-env")).unwrap();
    assert!(env.contains("BRUNN_API_TOKEN=model"));
    // The CI home directory contains "runner"; inspect the actual secrets.
    assert!(!env.contains(WORKSPACE_TOKEN));
    assert!(!env.contains(RUNNER_TOKEN));
    assert!(!env.contains("OPENAI_API_KEY"));
    let s = s.lock().unwrap();
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.notifications.len(), 1);
    assert!(
        s.notifications[0]["body"]
            .as_str()
            .unwrap()
            .contains("[Open Review](https://brunn.ai/dreams)")
    );
    assert_eq!(s.latest.as_ref().unwrap()["applied_writes"], json!([]));
    assert_eq!(
        receipt::parse_latest(&receipt::render_latest(s.latest.as_ref().unwrap()).unwrap())
            .unwrap(),
        s.latest.clone().unwrap()
    );
    assert_eq!(
        s.control.as_deref(),
        Some("enabled: true\nmode: report-only\nadvance_after: 2020-01-01\n")
    );
}
#[tokio::test]
async fn owner_review_race_retries_the_same_evidence_without_rerunning_the_model() {
    let (s, d, dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().review_conflicts = 1;
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    let state = s.lock().unwrap();
    assert_eq!(state.submitted.len(), 2);
    let mut expected_retry = state.submitted[0].clone();
    expected_retry["expected_state_version"] =
        json!(expected_retry["expected_state_version"].as_i64().unwrap() + 1);
    assert_eq!(state.submitted[1], expected_retry);
    assert_eq!(state.pending.len(), 1);
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|line| *line == "answer.md").count(), 1);
}

#[tokio::test]
async fn ordinary_consolidation_uses_frozen_context_without_consuming_it() {
    let behavior = format!(
        r#"
if [ "$OUTPUT_NAME" = 'narrative-discovery-answer.md' ]; then
 echo '{{"schema":"dream.narrative.discovery.v1","queries":["Project Orchid"]}}' > "$OUTPUT_PATH"
 exit 0
fi
{HAPPY}
"#
    );
    let (s, d, dir) = build(&behavior).await;
    enable(&s);
    std::fs::write(dir.path().join("custom-narrative-discovery"), "").unwrap();
    s.lock().unwrap().narrative_context = vec![
        json!({"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2cf0","path":"sources/Projects/Orchid/Canonical.md","version":4,"generation":12,"operation":"context","content_hash":"sha256:context"}),
    ];
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    let prompt = std::fs::read_to_string(dir.path().join("prompt-answer.md")).unwrap();
    assert!(prompt.contains("sources/Projects/Orchid/Canonical.md"));
    let state = s.lock().unwrap();
    assert_eq!(state.narrative_discoveries.len(), 1);
    assert_eq!(
        state.narrative_discoveries[0]["queries"],
        json!(["Project Orchid"])
    );
    assert_eq!(
        state.submitted[0]["processed_inputs"],
        json!([{"entry_ref":SOURCE,"version":2,"generation":17}])
    );
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert!(
        calls.find("narrative-discovery-answer.md").unwrap() < calls.find("\nanswer.md").unwrap()
    );
}

#[tokio::test]
async fn invalid_narrative_discovery_retains_work_and_finishes_auth() {
    let behavior = format!(
        r#"
if [ "$OUTPUT_NAME" = 'narrative-discovery-answer.md' ]; then
 echo '{{"schema":"dream.narrative.discovery.v1","queries":[],"write":"not allowed"}}' > "$OUTPUT_PATH"
 exit 0
fi
{HAPPY}
"#
    );
    let (s, d, dir) = build(&behavior).await;
    enable(&s);
    std::fs::write(dir.path().join("custom-narrative-discovery"), "").unwrap();
    let report = d.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
    assert_eq!(s.lock().unwrap().submissions, 0);
}

#[tokio::test]
async fn repeated_owner_review_races_are_bounded_and_do_not_claim_progress() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().review_conflicts = 2;
    let report = d.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Failed { .. }),
        "{report:?}"
    );
    assert_eq!(report.receipt_persistence, "accepted");
    let state = s.lock().unwrap();
    assert_eq!(state.submissions, 2);
    assert!(state.pending.is_empty());
    assert_eq!(state.runs[0]["outcome"], "failed");
}

#[tokio::test]
async fn enabled_auth_skip_has_durable_receipt_and_no_model_work() {
    let (s, d, dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().secrets.remove(AUTH_SECRET);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::SkippedAuth { .. }));
    assert_eq!(report.receipt_persistence, "accepted");
    assert!(!dir.path().join("prompt").exists());
    assert_eq!(s.lock().unwrap().submissions, 0);
}
#[tokio::test]
async fn retained_location_work_does_not_report_a_complete_attempt() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().retained_location_work = true;
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::Partial { .. }));
    assert_eq!(report.receipt_persistence, "accepted");
    assert_eq!(d.runtime_status().await.last_run_date, None);
    assert_eq!(
        s.lock().unwrap().latest.as_ref().unwrap()["status"],
        "partial"
    );
}
#[tokio::test]
async fn same_day_attempts_use_new_identity_and_exact_report_versions() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().run_version = 8;
    let a = d.run_once(today(), RunKind::Nightly).await;
    let b = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(a.outcome, RunOutcome::Completed);
    assert_eq!(b.outcome, RunOutcome::Completed);
    assert_ne!(a.attempt_id, b.attempt_id);
    assert!(b.run_version > a.run_version);
    assert_eq!(s.lock().unwrap().runs.len(), 2);
}
#[tokio::test]
async fn zero_exit_without_candidate_file_is_failed_even_with_old_run() {
    let (s, d, _dir) = build("echo READY; exit 0").await;
    enable(&s);
    s.lock().unwrap().run_version = 4;
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
    assert_eq!(report.run_version, Some(5));
    assert_eq!(s.lock().unwrap().submissions, 0);
}
#[tokio::test]
async fn malformed_candidate_cannot_advance_processed_work() {
    let(s,d,_dir)=build("if grep -q 'single word READY' \"$DIR/prompt\"; then echo READY; exit 0; fi\necho '{bad' > \"$OUTPUT_PATH\"").await;
    enable(&s);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
    assert_eq!(s.lock().unwrap().submissions, 0);
}
#[tokio::test]
async fn timeout_and_refresh_are_both_reported() {
    let(s,d,_dir)=build("if grep -q 'single word READY' \"$DIR/prompt\"; then echo READY; exit 0; fi\necho '{\"refreshed\":true}' > \"$CODEX_HOME/auth.json\"\nexec sleep 20").await;
    enable(&s);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::Partial { .. }));
    assert_eq!(report.auth_persistence, "verified");
    assert!(
        s.lock().unwrap().secrets[AUTH_SECRET]
            .0
            .contains("refreshed")
    );
}
#[tokio::test]
async fn failed_limits_probe_still_preserves_refresh() {
    let (s, d, _dir) = build(
        "echo '{\"refreshed\":true}' > \"$CODEX_HOME/auth.json\"\necho 'usage limit'; exit 1",
    )
    .await;
    enable(&s);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::SkippedLimits);
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
}
#[tokio::test]
async fn auth_cas_failure_cannot_claim_clean_readiness() {
    let (s, d, _dir) = build(
        "echo '{\"refreshed\":true}' > \"$CODEX_HOME/auth.json\"\necho 'usage limit'; exit 1",
    )
    .await;
    enable(&s);
    s.lock().unwrap().fail_auth_put = true;
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
    assert_eq!(report.auth_persistence, "failed");
    assert!(
        !s.lock().unwrap().secrets[AUTH_SECRET]
            .0
            .contains("refreshed")
    );
}
#[tokio::test]
async fn writable_model_credential_is_refused_before_probe() {
    let (s, d, dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().model_read_only = false;
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
    assert!(!dir.path().join("prompt").exists());
}
#[tokio::test]
async fn terminal_persistence_failure_is_visible() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().fail_finish = true;
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.receipt_persistence, "failed");
    assert!(report.persistence_error.is_some());
    assert!(report.run_version.is_none());
}
#[tokio::test]
async fn source_conflict_retains_work_without_false_completion() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().reject_candidates = true;
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(
        matches!(&report.outcome, RunOutcome::Failed { detail } if detail.contains("source changed"))
    );
    assert_eq!(s.lock().unwrap().admissions, 1);
    assert!(s.lock().unwrap().notifications.is_empty());
}
#[tokio::test]
async fn notification_failure_preserves_candidates_and_retries_original_key() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().fail_notify = true;
    let a = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(a.outcome, RunOutcome::Completed);
    assert_eq!(a.notification["status"], "failed");
    {
        let mut s = s.lock().unwrap();
        assert_eq!(s.pending.len(), 1);
        s.fail_notify = false;
    }
    let _ = d.run_once(today(), RunKind::Manual).await;
    let s = s.lock().unwrap();
    assert_eq!(
        s.notifications[0]["event_key"],
        s.notifications[1]["event_key"]
    );
}
#[tokio::test]
async fn model_cannot_mutate_workspace_even_if_prompt_is_ignored() {
    let behavior = format!(
        "if grep -q 'single word READY' \"$DIR/prompt\"; then echo READY; exit 0; fi\ncode=$(curl -s -o /dev/null -w '%{{http_code}}' -X POST \"$BRUNN_API_URL/v1/workspace/write\" -H \"Authorization: Bearer $BRUNN_API_TOKEN\"); test \"$code\" = 403 || exit 1\n{HAPPY}"
    );
    let (s, d, _dir) = build(&behavior).await;
    enable(&s);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::Completed);
    assert!(report.confinement_violations.is_empty());
}

#[tokio::test]
async fn terminal_tail_recovers_before_any_new_model_work() {
    let (s, d, _dir) = build(HAPPY).await;
    enable(&s);
    s.lock().unwrap().fail_finish = true;
    let first = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(first.receipt_persistence, "failed");
    s.lock().unwrap().fail_finish = false;
    let recovered = d.run_once(today(), RunKind::Nightly).await;
    assert_eq!(recovered.outcome, RunOutcome::SkippedAlreadyRan);
    let state = s.lock().unwrap();
    assert_eq!(state.submissions, 1, "recovery must not rerun the model");
    assert_eq!(state.runs[0]["attempt_id"], first.attempt_id);
}

#[tokio::test]
async fn valid_partial_output_reports_retained_work() {
    let behavior = HAPPY.replace("\"processed_inputs\":[{\"entry_ref\":\"entry:019fba27-687b-7582-8b99-e9371dbe2ce5\",\"version\":2,\"generation\":17}]", "\"processed_inputs\":[]");
    let (s, d, _dir) = build(&behavior).await;
    enable(&s);
    let report = d.run_once(today(), RunKind::Nightly).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.receipt_persistence, "accepted");
}

const LOCATION_ITEM: &str = "fixture-location/7";
const LOCATION_CAVEAT: &str =
    "The receipt time is unknown; this point does not establish continuous presence.";
const CANONICAL_BOUNDARY_ROW: &str = "| 2040-02-02T23:40+00:00 | 2040-02-03T00:05+00:00 | 25m | — | unknown | Fixture | low | 1.0,2.0 |";
const CANONICAL_POINT_ROW: &str =
    "| 2040-02-03T10:01+00:00 | — | — | — | unknown | Fixture | low | 1.0,2.0 |";

fn location_candidate(observation: &str) -> Value {
    json!({
        "kind":"summary", "title":"Synthetic location reconciliation",
        "summary":"Reconcile one synthetic observation", "reason":"Preserve the source qualifications",
        "path":"derived/location/2040-02-03.md", "expected_version":0,
        "revises_item_id":LOCATION_ITEM,
        "evidence_scope":{
            "from":"2040-02-03T00:00:00Z", "to":"2040-02-04T00:00:00Z", "timezone":"UTC",
            "fingerprint":format!("sha256:{}", "a".repeat(64))
        },
        "content":format!("- {observation}[^r1][^s1]\n- {LOCATION_CAVEAT}[^r1]"),
        "uncertainty":LOCATION_CAVEAT,
        "sources":[{"entry_ref":SOURCE,"version":2,"start_line":1,"end_line":2}],
        "raw_sources":[{"natural_key":{"at":"2040-02-03T10:01:00Z","type":"ping"},
            "fields":["at","lat","lon","accuracy_m","first_received_at"]}]
    })
}

fn location_output(observation: &str) -> Value {
    json!({"schema":"dream.candidates.v1", "candidates":[location_candidate(observation)],
        "processed_inputs":[], "findings":["Retain source qualifications."]})
}

fn enable_location(s: &Shared) {
    enable(s);
    let candidate = location_candidate("Original pending observation.");
    let scope = &candidate["evidence_scope"];
    let mut s = s.lock().unwrap();
    s.retained_location_work = true;
    s.pending = vec![
        json!({"id":LOCATION_ITEM,"status":"needs_changes", "candidate":candidate,
        "run_entry_ref":RUN,"run_version":4,"candidate_hash":"sha256:original-pending"}),
    ];
    s.location_admission = Some(json!({
        "inputs":[],
        "location_work":{"date":"2040-02-03","from":scope["from"],"to":scope["to"],
            "timezone":scope["timezone"],"fingerprint":scope["fingerprint"]},
        "location_evidence":{
            "schema":"location.evidence.v1", "evidence_fingerprint":scope["fingerprint"],
            "fingerprint_complete":true, "completeness":{"complete":true},
            "reports":[{"natural_key":{"at":"2040-02-03T10:01:00Z","type":"ping"},
                "at":"2040-02-03T10:01:00Z","type":"ping","lat":1.0,"lon":2.0,
                "accuracy_m":5.0,"first_received_at":null}],
            "canonical_months":[{"ref":SOURCE,"path":"Location/Visits/2040-02.md","version":2,"selectors":[{
                "start_line":1,"end_line":1,"text":CANONICAL_BOUNDARY_ROW,
                "arrived_at":"2040-02-02T23:40:00Z","departed_at":"2040-02-03T00:05:00Z",
                "origin":"canonical_visit","precision":"canonical minute"}, {
                "start_line":2,"end_line":2,"text":CANONICAL_POINT_ROW,
                "arrived_at":"2040-02-03T10:01:00Z","departed_at":null,
                "origin":"canonical_visit","precision":"canonical minute"}]}],
            "boundary_observations":{"before":null,"after":null}, "sample_gaps":[],
            "time_semantics":{"first_received_at":"Null means receipt time is unknown."}
        }
    }));
}

async fn build_location_audit(
    audit_script: &str,
    draft_script: &str,
    budget: Duration,
) -> (Shared, Dreamer, tempfile::TempDir, Value) {
    let behavior = format!(
        r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
case "$OUTPUT_NAME" in
 location-discovery-answer.md)
  echo '{{"schema":"dream.location.discovery.v1","context_queries":[],"lookups":[],"findings":[]}}' > "$OUTPUT_PATH"
  ;;
 location-answer.md)
  {draft_script}
  cat "$DIR/draft.json" > "$OUTPUT_PATH"
  ;;
 location-audit-answer.md|location-correction-answer.md)
  echo '{{"audit_refreshed":true}}' > "$CODEX_HOME/auth.json"
  {audit_script}
  ;;
 *) exit 99;;
esac
"#
    );
    let (s, d, dir) = build_with_budget(&behavior, budget).await;
    enable_location(&s);
    let draft = location_output("Draft observation requiring independent review.");
    let mut audited = location_output("Audited observation with corrected source qualification.");
    audited["findings"]
        .as_array_mut()
        .unwrap()
        .push(json!("Checked the bounded packet against the draft."));
    for (name, value) in [("draft.json", &draft), ("audited.json", &audited)] {
        std::fs::write(dir.path().join(name), value.to_string()).unwrap();
    }
    (s, d, dir, audited)
}

fn model_calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls"))
        .unwrap()
        .lines()
        .filter(|name| *name != "location-discovery-answer.md")
        .map(str::to_owned)
        .collect()
}

fn assert_canonical_evidence_inventory(candidate: &Value) {
    let body = candidate["content"].as_str().unwrap();
    assert!(!body.contains("Canonical interval inventory"));
    assert!(!body.contains(CANONICAL_BOUNDARY_ROW));
    assert!(!body.contains(CANONICAL_POINT_ROW));
    // The fixture's exact two-line source covers both canonical rows; neither
    // needs to be duplicated in the primary answer to retain that evidence.
    assert_eq!(candidate["sources"][0]["start_line"], 1);
    assert_eq!(candidate["sources"][0]["end_line"], 2);
}

#[tokio::test]
async fn location_audit_submits_only_the_corrected_artifact_under_the_original_identity() {
    let (s, d, dir, audited) = build_location_audit(
        "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"",
        "",
        Duration::from_secs(3),
    )
    .await;
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
    let calls = model_calls(dir.path());
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(
        &calls[1..],
        ["location-answer.md", "location-audit-answer.md"]
    );
    let paths = std::fs::read_to_string(dir.path().join("output-paths")).unwrap();
    let paths: Vec<_> = paths.lines().collect();
    assert_ne!(paths[1], paths[2]);
    let audit_prompt =
        std::fs::read_to_string(dir.path().join("prompt-location-audit-answer.md")).unwrap();
    for evidence in [
        "location.evidence.v1",
        "Draft observation requiring independent review.",
        CANONICAL_BOUNDARY_ROW,
        "first_received_at",
        LOCATION_ITEM,
    ] {
        assert!(audit_prompt.contains(evidence), "missing {evidence}");
    }
    let audit_env =
        std::fs::read_to_string(dir.path().join("env-location-audit-answer.md")).unwrap();
    assert!(!audit_env.contains("BRUNN_API_TOKEN="));
    for forbidden in [RUNNER_TOKEN, WORKSPACE_TOKEN, "OPENAI_API_KEY"] {
        assert!(!audit_env.contains(forbidden));
    }
    let s = s.lock().unwrap();
    assert_eq!(s.submissions, 1);
    let accepted_content = s.submitted[0]["candidates"][0]["content"].as_str().unwrap();
    assert!(accepted_content.starts_with(audited["candidates"][0]["content"].as_str().unwrap()));
    assert_canonical_evidence_inventory(&s.submitted[0]["candidates"][0]);
    let mut expected_candidates = audited["candidates"].clone();
    expected_candidates[0]["content"] = json!(accepted_content);
    assert_eq!(s.submitted[0]["candidates"], expected_candidates);
    assert_eq!(s.submitted[0]["findings"], audited["findings"]);
    assert_eq!(s.submitted[0]["processed_inputs"], json!([]));
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.pending[0]["id"], LOCATION_ITEM);
    assert_eq!(s.pending[0]["candidate"], s.submitted[0]["candidates"][0]);
    assert!(!s.retained_location_work);
    assert_eq!(
        s.auth_puts, 1,
        "one finalizer owns auth persistence across both calls"
    );
    assert!(s.secrets[AUTH_SECRET].0.contains("audit_refreshed"));
}

#[tokio::test]
async fn server_structural_contract_enters_the_bounded_location_correction_pass() {
    let (s, d, dir, audited) = build_location_audit(
        "if [ \"$OUTPUT_NAME\" = \"location-audit-answer.md\" ]; then cat \"$DIR/invalid.json\" > \"$OUTPUT_PATH\"; else cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"; fi",
        "",
        Duration::from_secs(6),
    ).await;
    let mut invalid = audited.clone();
    invalid["candidates"][0]["title"] = json!("x".repeat(301));
    std::fs::write(dir.path().join("invalid.json"), invalid.to_string()).unwrap();
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    let correction =
        std::fs::read_to_string(dir.path().join("prompt-location-correction-answer.md")).unwrap();
    assert!(correction.contains("candidate title, summary or explanation exceeds its bound"));
    assert_eq!(s.lock().unwrap().submissions, 1);
    assert_eq!(
        s.lock().unwrap().submitted[0]["candidates"][0]["title"],
        audited["candidates"][0]["title"]
    );
}

#[tokio::test]
async fn location_discovery_follows_historical_aliases_before_drafting() {
    let (shared, d, dir, _) = build_location_audit(
        "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"",
        "",
        Duration::from_secs(8),
    )
    .await;
    let behavior=std::fs::read_to_string(dir.path().join("behavior.sh")).unwrap()
        .replace("\"context_queries\":[]","\"context_queries\":[\"Public Park\"]")
        .replace(" location-answer.md)"," location-discovery-followup-answer.md)\n echo '{\"schema\":\"dream.location.discovery.v1\",\"context_queries\":[\"New alias\"],\"lookups\":[],\"findings\":[]}' > \"$OUTPUT_PATH\"\n ;;\n location-answer.md)");
    std::fs::write(dir.path().join("behavior.sh"), behavior).unwrap();
    shared.lock().unwrap().location_admission.as_mut().unwrap()["location_context"] =
        json!([{"excerpt":"HISTORICAL_ALIAS_CANARY"}]);
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    let state = shared.lock().unwrap();
    assert_eq!(state.discoveries.len(), 2);
    assert_eq!(state.discoveries[1]["refines_request_hash"], "discovery-1");
    assert_eq!(
        state.discoveries[1]["context_queries"],
        json!(["New alias", "Public Park"])
    );
    assert!(
        state.discoveries[1]["expected_state_version"]
            .as_i64()
            .unwrap()
            > state.discoveries[0]["expected_state_version"]
                .as_i64()
                .unwrap()
    );
    let initial =
        std::fs::read_to_string(dir.path().join("prompt-location-discovery-answer.md")).unwrap();
    let followup = std::fs::read_to_string(
        dir.path()
            .join("prompt-location-discovery-followup-answer.md"),
    )
    .unwrap();
    assert!(!initial.contains("HISTORICAL_ALIAS_CANARY"));
    assert!(followup.contains("HISTORICAL_ALIAS_CANARY"));
    assert!(!followup.contains("Original pending observation."));
    let env = std::fs::read_to_string(dir.path().join("env-location-discovery-followup-answer.md"))
        .unwrap();
    assert!(!env.contains("BRUNN_API_TOKEN="));
    assert!(!env.contains("BRUNN_API_URL="));
}

#[tokio::test]
async fn location_and_narrative_publish_separately_and_keep_location_on_narrative_failure() {
    for reject in [false, true] {
        let (shared, d, dir, _) = build_location_audit(
            "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"",
            "",
            Duration::from_secs(6),
        )
        .await;
        let behavior = std::fs::read_to_string(dir.path().join("behavior.sh"))
            .unwrap()
            .replace(
                " *) exit 99;;",
                &format!(" narrative-answer.md)\n{HAPPY}\n ;;\n *) exit 99;;"),
            );
        std::fs::write(dir.path().join("behavior.sh"), behavior).unwrap();
        {
            let mut state = shared.lock().unwrap();
            state.reject_narrative = reject;
            state.location_admission.as_mut().unwrap()["inputs"] = json!([{"entry_ref":SOURCE,"version":2,"generation":17,"path":"NARRATIVE_SOURCE_CANARY"}]);
        }
        let report = d.run_once(today(), RunKind::Manual).await;
        if reject {
            assert!(
                matches!(&report.outcome, RunOutcome::Partial { detail } if detail.contains("narrative submission rejected") && detail.contains("source changed")),
                "{report:?}"
            );
        } else {
            assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
        }
        let state = shared.lock().unwrap();
        assert_eq!(state.submissions, 2);
        assert!(!state.retained_location_work);
        assert_eq!(state.submitted[0]["processed_inputs"], json!([]));
        assert!(state.submitted[0]["candidates"][0]["evidence_scope"].is_object());
        assert_eq!(
            state.submitted[1]["processed_inputs"][0]["entry_ref"],
            SOURCE
        );
        assert_eq!(state.notifications.len(), 1);
        for name in [
            "location-discovery-answer.md",
            "location-answer.md",
            "location-audit-answer.md",
        ] {
            let prompt =
                std::fs::read_to_string(dir.path().join(format!("prompt-{name}"))).unwrap();
            assert!(!prompt.contains("NARRATIVE_SOURCE_CANARY"));
            assert!(!prompt.contains("Original pending observation."));
            let env = std::fs::read_to_string(dir.path().join(format!("env-{name}"))).unwrap();
            assert!(!env.contains("BRUNN_API_TOKEN="));
        }
        let narrative =
            std::fs::read_to_string(dir.path().join("prompt-narrative-answer.md")).unwrap();
        assert!(narrative.contains("NARRATIVE_SOURCE_CANARY"));
        assert!(!narrative.contains("Original pending observation."));
        assert!(!narrative.contains(CANONICAL_BOUNDARY_ROW));
    }
}

#[tokio::test]
async fn a_location_queue_without_a_location_draft_does_not_trigger_an_audit() {
    for location_queued in [false, true] {
        let behavior = if location_queued {
            r#"
if grep -q 'single word READY' "$DIR/prompt"; then echo READY; exit 0; fi
if [ "$OUTPUT_NAME" = "location-discovery-answer.md" ]; then
 echo '{"schema":"dream.location.discovery.v1","context_queries":[],"lookups":[],"findings":[]}' > "$OUTPUT_PATH"
else
 echo '{"schema":"dream.candidates.v1","candidates":[],"processed_inputs":[],"findings":["No supported location candidate; retain the day."]}' > "$OUTPUT_PATH"
fi
"#
        } else {
            HAPPY
        };
        let (s, d, dir) = build(behavior).await;
        enable(&s);
        if location_queued {
            enable_location(&s);
        }
        let report = d.run_once(today(), RunKind::Manual).await;
        if location_queued {
            assert!(
                matches!(report.outcome, RunOutcome::Partial { .. }),
                "{report:?}"
            );
            assert!(s.lock().unwrap().retained_location_work);
        } else {
            assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
        }
        let calls = model_calls(dir.path());
        assert_eq!(
            calls,
            if location_queued {
                vec!["probe-answer.md", "location-answer.md"]
            } else {
                vec![
                    "probe-answer.md",
                    "narrative-discovery-answer.md",
                    "answer.md",
                ]
            }
        );
        assert!(!dir.path().join("prompt-location-audit-answer.md").exists());
    }
}

#[tokio::test]
async fn invalid_location_audits_never_submit_the_draft_and_still_finalize_auth() {
    let variants = [
        ("malformed", "echo '{bad' > \"$OUTPUT_PATH\"", false),
        ("missing", "exit 0", false),
        (
            "failed",
            "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"; exit 7",
            false,
        ),
        (
            "timed_out",
            "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"; exec sleep 20",
            true,
        ),
    ];
    for (name, script, timeout) in variants {
        let (s, d, dir, _) = build_location_audit(
            script,
            // A pre-existing valid audit slot must be cleared before the second process.
            "cp \"$DIR/audited.json\" \"${OUTPUT_PATH%/*}/location-audit-answer.md\"",
            Duration::from_secs(3),
        )
        .await;
        let original = s.lock().unwrap().pending.clone();
        let report = d.run_once(today(), RunKind::Manual).await;
        let detail = match &report.outcome {
            RunOutcome::Partial { detail } if timeout => detail,
            RunOutcome::Failed { detail } if !timeout => detail,
            _ => panic!("{name}: {report:?}"),
        };
        assert!(detail.contains("location audit"), "{name}: {detail}");
        assert!(
            detail.contains("unchecked draft not submitted"),
            "{name}: {detail}"
        );
        assert_eq!(report.auth_persistence, "verified", "{name}: {report:?}");
        assert_eq!(report.receipt_persistence, "accepted", "{name}: {report:?}");
        assert_eq!(model_calls(dir.path()).len(), 3, "{name}");
        let s = s.lock().unwrap();
        assert_eq!(
            s.submissions, 0,
            "{name}: neither draft nor audit may be submitted"
        );
        assert_eq!(s.pending, original, "{name}");
        assert!(s.retained_location_work, "{name}");
        assert!(s.notifications.is_empty(), "{name}");
        assert_eq!(s.auth_puts, 1, "{name}");
        assert!(
            s.secrets[AUTH_SECRET].0.contains("audit_refreshed"),
            "{name}"
        );
        let latest = s.latest.as_ref().unwrap();
        assert_eq!(latest["status"], if timeout { "partial" } else { "failed" });
        assert!(receipt::parse_latest(&receipt::render_latest(latest).unwrap()).is_ok());
    }
}

#[tokio::test]
async fn an_audit_can_remove_only_location_work_with_an_explicit_retention_finding() {
    let (s, d, dir, mut audited) = build_location_audit(
        "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"",
        "",
        Duration::from_secs(3),
    )
    .await;
    audited["candidates"] = json!([]);
    audited["findings"][1] =
        json!("The bounded evidence does not support a complete reconciliation; retain the day.");
    std::fs::write(dir.path().join("audited.json"), audited.to_string()).unwrap();
    let original = s.lock().unwrap().pending.clone();
    let report = d.run_once(today(), RunKind::Manual).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
    let s = s.lock().unwrap();
    assert_eq!(s.submissions, 1);
    assert_eq!(s.submitted[0]["candidates"], json!([]));
    assert_eq!(s.submitted[0]["findings"], audited["findings"]);
    assert_eq!(s.pending, original);
    assert!(s.retained_location_work);
    assert!(s.notifications.is_empty());
}

#[tokio::test]
async fn location_audit_uses_the_remaining_shared_model_deadline() {
    // Leave several seconds between the correct shared allowance and a fresh
    // full-budget mutant; process startup and auth finalization are outside
    // the model deadline and can be delayed by concurrent subprocess tests.
    let (s, d, dir, _) =
        build_location_audit("exec sleep 30", "sleep 5", Duration::from_secs(12)).await;
    let started = std::time::Instant::now();
    let report = d.run_once(today(), RunKind::Manual).await;
    let elapsed = started.elapsed();
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(
        model_calls(dir.path()).last().unwrap(),
        "location-audit-answer.md"
    );
    assert!(
        elapsed < Duration::from_secs(13),
        "audit received a fresh total budget: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(5),
        "draft delay was not exercised"
    );
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
    let s = s.lock().unwrap();
    assert_eq!(s.submissions, 0);
    assert_eq!(s.auth_puts, 1);
    assert!(s.retained_location_work);
}

#[tokio::test]
async fn an_early_draft_returns_unused_time_to_the_location_audit() {
    let (s, d, _dir, _) = build_location_audit(
        "sleep 3\ncat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"",
        "",
        Duration::from_secs(6),
    )
    .await;
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(s.lock().unwrap().submissions, 1);
}

async fn build_location_correction(
    correction_script: &str,
    audit_delay: &str,
    budget: Duration,
) -> (Shared, Dreamer, tempfile::TempDir, Value) {
    let audit_script = format!(
        r#"
if [ "$OUTPUT_NAME" = "location-audit-answer.md" ]; then
 {audit_delay}
 cp "$DIR/corrected.json" "${{OUTPUT_PATH%/*}}/location-correction-answer.md"
 cat "$DIR/audited.json" > "$OUTPUT_PATH"
else
 echo '{{"correction_refreshed":true}}' > "$CODEX_HOME/auth.json"
 {correction_script}
fi
"#
    );
    let (s, d, dir, mut audited) = build_location_audit(&audit_script, "", budget).await;
    // The precise observation exists in the frozen packet, but the audited
    // draft cites only the earlier observation. Truth without its supporting
    // citation must require correction before submission.
    audited["candidates"][0]["content"] = json!(format!(
        "- An observation occurs at 10:02:00.[^r1][^s1]\n- {LOCATION_CAVEAT}[^r1]"
    ));
    s.lock().unwrap().location_admission.as_mut().unwrap()["location_evidence"]["reports"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "natural_key":{"at":"2040-02-03T10:02:00Z","type":"ping"},
            "at":"2040-02-03T10:02:00Z","type":"ping","lat":1.0,"lon":2.0,
            "accuracy_m":5.0,"first_received_at":null
        }));
    let mut corrected = audited.clone();
    corrected["candidates"][0]["raw_sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "natural_key":{"at":"2040-02-03T10:02:00Z","type":"ping"},
            "fields":["at","lat","lon","accuracy_m","first_received_at"]
        }));
    corrected["candidates"][0]["content"] = json!(format!(
        "- An observation occurs at 10:02:00.[^r2][^s1]\n- {LOCATION_CAVEAT}[^r1][^r2]"
    ));
    corrected["findings"].as_array_mut().unwrap().push(json!(
        "Corrected the observation's exact timestamp citation."
    ));
    for (name, value) in [("audited.json", &audited), ("corrected.json", &corrected)] {
        std::fs::write(dir.path().join(name), value.to_string()).unwrap();
    }
    (s, d, dir, corrected)
}

#[tokio::test]
async fn a_citation_valid_but_verbose_audit_gets_the_same_bounded_correction() {
    let (s, d, dir, corrected) = build_location_correction(
        "cat \"$DIR/corrected.json\" > \"$OUTPUT_PATH\"",
        "",
        Duration::from_secs(6),
    )
    .await;
    let mut verbose = corrected.clone();
    verbose["candidates"][0]["content"] = json!(format!(
        "{}\n{}[^r1]",
        corrected["candidates"][0]["content"].as_str().unwrap(),
        "Repeated evidence detail. ".repeat(90)
    ));
    std::fs::write(dir.path().join("audited.json"), verbose.to_string()).unwrap();
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(model_calls(dir.path()).len(), 4);
    let correction_prompt =
        std::fs::read_to_string(dir.path().join("prompt-location-correction-answer.md")).unwrap();
    assert!(correction_prompt.contains("at most 250 words"));
    let state = s.lock().unwrap();
    assert_eq!(state.submissions, 1);
    assert_eq!(
        state.submitted[0]["candidates"][0]["content"],
        corrected["candidates"][0]["content"]
    );
    assert_canonical_evidence_inventory(&state.submitted[0]["candidates"][0]);
    assert!(!state.retained_location_work);
    assert_eq!(report.auth_persistence, "verified");
}

#[tokio::test]
async fn unsupported_audited_clock_requires_one_correction_before_submission() {
    let (s, d, dir, corrected) = build_location_correction(
        "cat \"$DIR/corrected.json\" > \"$OUTPUT_PATH\"",
        "",
        Duration::from_secs(6),
    )
    .await;
    let report = d.run_once(today(), RunKind::Manual).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert_eq!(report.auth_persistence, "verified");
    assert_eq!(report.receipt_persistence, "accepted");
    assert_eq!(
        &model_calls(dir.path())[1..],
        [
            "location-answer.md",
            "location-audit-answer.md",
            "location-correction-answer.md"
        ]
    );
    let correction_prompt =
        std::fs::read_to_string(dir.path().join("prompt-location-correction-answer.md")).unwrap();
    assert!(correction_prompt.contains("10:02:00"));
    assert!(correction_prompt.contains("2040-02-03T10:02:00Z"));
    assert!(correction_prompt.contains(LOCATION_ITEM));
    let correction_env =
        std::fs::read_to_string(dir.path().join("env-location-correction-answer.md")).unwrap();
    assert!(!correction_env.contains("BRUNN_API_TOKEN="));
    assert!(!correction_env.contains(RUNNER_TOKEN));
    assert!(!correction_env.contains(WORKSPACE_TOKEN));
    assert!(!correction_env.contains("OPENAI_API_KEY"));
    let s = s.lock().unwrap();
    assert_eq!(s.submissions, 1);
    assert_eq!(s.pending.len(), 1);
    assert_eq!(s.pending[0]["id"], LOCATION_ITEM);
    let actual = &s.submitted[0]["candidates"][0];
    assert_eq!(
        actual["raw_sources"],
        corrected["candidates"][0]["raw_sources"]
    );
    let body = actual["content"].as_str().unwrap();
    assert!(body.starts_with(corrected["candidates"][0]["content"].as_str().unwrap()));
    assert_canonical_evidence_inventory(actual);
    assert_eq!(s.submitted[0]["findings"], corrected["findings"]);
    assert!(!s.retained_location_work);
    assert_eq!(s.auth_puts, 1);
    assert!(s.secrets[AUTH_SECRET].0.contains("correction_refreshed"));
}

#[tokio::test]
async fn failed_clock_corrections_never_submit_an_earlier_or_stale_artifact() {
    for (name, script, timeout) in [
        ("malformed", "echo '{bad' > \"$OUTPUT_PATH\"", false),
        ("missing", "exit 0", false),
        (
            "nonzero",
            "cat \"$DIR/corrected.json\" > \"$OUTPUT_PATH\"; exit 7",
            false,
        ),
        (
            "uncorrected",
            "cat \"$DIR/audited.json\" > \"$OUTPUT_PATH\"",
            false,
        ),
        (
            "timeout",
            "cat \"$DIR/corrected.json\" > \"$OUTPUT_PATH\"; exec sleep 20",
            true,
        ),
    ] {
        let (s, d, dir, _) = build_location_correction(script, "", Duration::from_secs(3)).await;
        let original = s.lock().unwrap().pending.clone();
        let report = d.run_once(today(), RunKind::Manual).await;
        match &report.outcome {
            RunOutcome::Partial { .. } if timeout => (),
            RunOutcome::Failed { .. } if !timeout => (),
            _ => panic!("{name}: {report:?}"),
        }
        assert_eq!(
            model_calls(dir.path()).len(),
            4,
            "{name}: one corrective attempt only"
        );
        assert_eq!(report.auth_persistence, "verified", "{name}");
        assert_eq!(report.receipt_persistence, "accepted", "{name}");
        let s = s.lock().unwrap();
        assert_eq!(s.submissions, 0, "{name}");
        assert_eq!(s.pending, original, "{name}");
        assert!(s.retained_location_work, "{name}");
        assert!(s.notifications.is_empty(), "{name}");
        assert_eq!(s.auth_puts, 1, "{name}");
        assert!(
            s.secrets[AUTH_SECRET].0.contains("correction_refreshed"),
            "{name}"
        );
    }
}

#[tokio::test]
async fn clock_correction_shares_the_first_audit_deadline() {
    // Audit + correction share the remaining 18s. A fresh pool after the 7s
    // audit would exceed 25s. Allow startup overhead without admitting a reset.
    let (s, d, dir, _) =
        build_location_correction("exec sleep 30", "sleep 7", Duration::from_secs(20)).await;
    let started = std::time::Instant::now();
    let report = d.run_once(today(), RunKind::Manual).await;
    let elapsed = started.elapsed();
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    assert_eq!(
        model_calls(dir.path()).last().unwrap(),
        "location-correction-answer.md"
    );
    assert!(
        elapsed < Duration::from_secs(22),
        "correction received a fresh audit allowance: {elapsed:?}"
    );
    assert!(elapsed >= Duration::from_secs(7));
    let s = s.lock().unwrap();
    assert_eq!(s.submissions, 0);
    assert_eq!(s.auth_puts, 1);
    assert!(s.retained_location_work);
}

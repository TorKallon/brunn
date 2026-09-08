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
#[derive(Default)]
struct Mock {
    control: Option<String>,
    secrets: BTreeMap<String, (String, i64)>,
    writes: usize,
    run_version: i64,
    state_version: i64,
    admissions: usize,
    submissions: usize,
    runs: Vec<Value>,
    latest: Option<Value>,
    notifications: Vec<Value>,
    reject_candidates: bool,
    fail_finish: bool,
    fail_auth_put: bool,
    fail_notify: bool,
    retained_location_work: bool,
    model_read_only: bool,
    pending: Vec<Value>,
    prior_pending_notification: Option<Value>,
}
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
async fn secret_put(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut s = shared.lock().unwrap();
    let name = body["name"].as_str().unwrap();
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
async fn write(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    if headers.get("authorization").unwrap() == "Bearer model" {
        return error(StatusCode::FORBIDDEN, "read only");
    }
    shared.lock().unwrap().writes += 1;
    Json(json!({})).into_response()
}
async fn admit(State(shared): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut s = shared.lock().unwrap();
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
    Json(
        json!({"admitted":true,"attempt_id":body["attempt_id"],"fence":s.admissions,"state_version":s.state_version,"mode":"report-only","frozen_generation":17,"scanned_generation":17,"processed_generation":0,
      "inputs":[{"entry_ref":SOURCE,"path":"sources/Project.md","version":2,"generation":17,"operation":"update","content_hash":"sha256:source"}],
      "pending":s.pending,"pending_notifications":s.prior_pending_notification.iter().collect::<Vec<_>>()}),
    )
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
    s.submissions += 1;
    if s.reject_candidates {
        return error(StatusCode::CONFLICT, "source changed");
    }
    s.pending
        .extend(body["candidates"].as_array().unwrap().iter().cloned());
    let ids: Vec<_> = body["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(n, _)| format!("{}/{}", body["attempt_id"].as_str().unwrap(), n))
        .collect();
    s.state_version += 1;
    s.run_version += 1;
    s.writes += 2;
    Json(json!({"state_version":s.state_version,"run_entry_ref":RUN,"run_version":s.run_version,"accepted_candidate_ids":ids,"pending_count":s.pending.len()})).into_response()
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
        .route("/v1/workspace/dreamer/candidates", post(candidates))
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
        workspace_token: "workspace".into(),
        model_token: "model".into(),
        runner_token: "runner".into(),
        codex_path: stub,
        codex_model: "test-model".into(),
        mcp_server_entry: PathBuf::from("/dev/null"),
        work_root: dir.path().join("work"),
        host_env: BTreeMap::from([
            ("PATH".into(), std::env::var("PATH").unwrap()),
            ("OPENAI_API_KEY".into(), "must-not-escape".into()),
        ]),
        time_budget_override: Some(Duration::from_secs(2)),
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
export OUTPUT_PATH
cat > "$DIR/prompt"
env > "$DIR/model-env"
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
{"schema":"dream.candidates.v1","candidates":[{"kind":"summary","title":"Project summary","summary":"Current project status","reason":"Consolidate the project evidence","path":"derived/entities/project.md","content":"Observed: project is active.[^s1]","expected_version":0,"sources":[{"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce5","version":2,"start_line":1,"end_line":2}],"uncertainty":"No uncertainty identified"}],"processed_inputs":[{"entry_ref":"entry:019fba27-687b-7582-8b99-e9371dbe2ce5","version":2,"generation":17}],"findings":[]}
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
    assert!(!env.contains("workspace"));
    assert!(!env.contains("runner"));
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
    assert!(matches!(report.outcome, RunOutcome::Failed { .. }));
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

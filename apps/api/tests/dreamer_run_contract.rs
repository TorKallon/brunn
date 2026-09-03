//! The dreamer run contract, end to end against a mock Brunn API and a
//! stub codex binary: CONTROL fail-closed, auth fail-closed with env
//! stripping, skipped(limits), the advance flip and hold-advance, CAS
//! conflict re-read-once-retry-once, the kill timer, the run-file fallback,
//! decisions reaching the prompt, and the confinement cross-check.

use std::{
    collections::BTreeMap,
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use brunn::dreamer::run::{
    AUTH_SECRET, CONTROL_PATH, DECISIONS_PATH, Dreamer, DreamerConfig, RUNTIME_SECRET, RunKind,
    RunOutcome,
};
use chrono::NaiveDate;
use serde_json::{Value, json};

#[derive(Default)]
struct MockState {
    files: BTreeMap<String, (String, i64)>,
    changes: Vec<Value>,
    generation: i64,
    secrets: BTreeMap<String, String>,
    notifications: Vec<Value>,
    /// Paths that 409 this many more times before accepting a write.
    conflicts: BTreeMap<String, usize>,
    write_count: usize,
}

type Shared = Arc<Mutex<MockState>>;

fn record_write(state: &mut MockState, path: &str, content: &str) -> (i64, i64) {
    let version = state.files.get(path).map_or(0, |(_, v)| *v) + 1;
    state
        .files
        .insert(path.to_owned(), (content.to_owned(), version));
    state.generation += 1;
    let operation = if version == 1 { "create" } else { "update" };
    let change = json!({
        "generation": state.generation,
        "operation": operation,
        "path": path,
        "version": version,
    });
    state.changes.push(change);
    state.write_count += 1;
    (version, state.generation)
}

async fn mock_read(State(shared): State<Shared>, Json(request): Json<Value>) -> Json<Value> {
    let state = shared.lock().expect("mock state");
    let items: Vec<Value> = request["requests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let path = item["path"].as_str().unwrap_or_default();
            match state.files.get(path) {
                Some((content, version)) => json!({
                    "path": path,
                    "text": content,
                    "version": version,
                }),
                None => json!({"status": "not_found", "path": path}),
            }
        })
        .collect();
    Json(json!({"data": {"items": items}}))
}

async fn mock_write(State(shared): State<Shared>, Json(request): Json<Value>) -> Response {
    let mut state = shared.lock().expect("mock state");
    let path = request["path"].as_str().unwrap_or_default().to_owned();
    if let Some(remaining) = state.conflicts.get_mut(&path)
        && *remaining > 0
    {
        *remaining -= 1;
        let actual = state.files.get(&path).map_or(0, |(_, v)| *v);
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": {
                    "code": "entry_version_conflict",
                    "message": "the entry changed since it was read",
                    "details": {"path": path, "actual_version": actual}
                }
            })),
        )
            .into_response();
    }
    if let Some(expected) = request["expected_version"].as_i64() {
        let actual = state.files.get(&path).map_or(0, |(_, v)| *v);
        if expected != actual {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": {
                        "code": "entry_version_conflict",
                        "message": "the entry changed since it was read",
                        "details": {"path": path, "actual_version": actual}
                    }
                })),
            )
                .into_response();
        }
    }
    let content = request["content"].as_str().unwrap_or_default().to_owned();
    let (version, generation) = record_write(&mut state, &path, &content);
    Json(json!({
        "data": {"version": version, "workspace_generation": generation, "no_op": false}
    }))
    .into_response()
}

async fn mock_changes(
    State(shared): State<Shared>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Json<Value> {
    let state = shared.lock().expect("mock state");
    let since: i64 = query
        .get("since_generation")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let changes: Vec<Value> = state
        .changes
        .iter()
        .filter(|change| change["generation"].as_i64().unwrap_or(0) > since)
        .cloned()
        .collect();
    Json(json!({
        "data": {
            "since_generation": since,
            "workspace_generation": state.generation,
            "changes": changes,
            "truncated": false,
            "next_generation": state.generation,
        }
    }))
}

async fn mock_secret_get(State(shared): State<Shared>, Json(request): Json<Value>) -> Response {
    let state = shared.lock().expect("mock state");
    let name = request["name"].as_str().unwrap_or_default();
    match state.secrets.get(name) {
        Some(value) => Json(json!({"data": {"name": name, "value": value}})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"code": "not_found", "message": "no such secret"}})),
        )
            .into_response(),
    }
}

async fn mock_secret_put(State(shared): State<Shared>, Json(request): Json<Value>) -> Json<Value> {
    let mut state = shared.lock().expect("mock state");
    let name = request["name"].as_str().unwrap_or_default().to_owned();
    let value = request["value"].as_str().unwrap_or_default().to_owned();
    state.secrets.insert(name.clone(), value);
    Json(json!({"data": {"name": name, "status": "committed"}}))
}

async fn mock_secret_delete(
    State(shared): State<Shared>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let mut state = shared.lock().expect("mock state");
    let name = request["name"].as_str().unwrap_or_default().to_owned();
    state.secrets.remove(&name);
    Json(json!({"data": {"name": name, "status": "deleted"}}))
}

async fn mock_notify(State(shared): State<Shared>, Json(request): Json<Value>) -> Json<Value> {
    let mut state = shared.lock().expect("mock state");
    state.notifications.push(request);
    Json(json!({"data": {"status": "committed"}}))
}

async fn start_mock() -> (Shared, String) {
    let shared: Shared = Arc::new(Mutex::new(MockState::default()));
    let app = Router::new()
        .route("/v1/workspace/read", post(mock_read))
        .route("/v1/workspace/write", post(mock_write))
        .route("/v1/workspace/changes", get(mock_changes))
        .route("/v1/workspace/secrets/get", post(mock_secret_get))
        .route("/v1/workspace/secrets/put", post(mock_secret_put))
        .route("/v1/workspace/secrets/delete", post(mock_secret_delete))
        .route("/v1/workspace/notifications/publish", post(mock_notify))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock listener");
    let address = listener.local_addr().expect("mock address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });
    (shared, format!("http://{address}"))
}

/// A stub codex: `login status` and `--version` behave; `exec` records its
/// stdin and environment, then runs the per-test behavior script.
fn write_stub(dir: &Path, behavior: &str, chatgpt_login: bool) -> PathBuf {
    let login_line = if chatgpt_login {
        "Logged in using ChatGPT"
    } else {
        "Logged in using an API key"
    };
    let stub_path = dir.join("codex");
    let behavior_path = dir.join("behavior.sh");
    std::fs::write(&behavior_path, behavior).expect("behavior script");
    let script = format!(
        r#"#!/bin/sh
DIR="{dir}"
case "$1" in
  login) echo "{login_line}"; exit 0 ;;
  --version) echo "codex-cli 0.0.0-test"; exit 0 ;;
  exec)
    N=$(ls "$DIR"/exec-*.stdin 2>/dev/null | wc -l | tr -d ' ')
    cat > "$DIR/exec-$N.stdin"
    env > "$DIR/exec-$N.env"
    export N DIR
    exec /bin/sh "$DIR/behavior.sh"
    ;;
  *) exit 0 ;;
esac
"#,
        dir = dir.display(),
        login_line = login_line,
    );
    let mut file = std::fs::File::create(&stub_path).expect("stub file");
    file.write_all(script.as_bytes()).expect("stub body");
    drop(file);
    std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755))
        .expect("stub permissions");
    stub_path
}

/// Behavior: probe (N=0) answers READY; the main exec (N=1) writes a run
/// file through the API with curl, exactly like real codex through MCP.
const HAPPY_BEHAVIOR: &str = r#"
if [ "$N" = "0" ]; then echo READY; exit 0; fi
RUN_PATH=$(grep -o 'dreams/runs/[0-9-]*\.md' "$DIR/exec-$N.stdin" | tail -1)
BODY=$(cat <<'EOF'
Linked 2 notes.
Recompiled 1 entity view.
Nothing needs your call tonight.
No contradictions were found.
Budget: 3 of 40 writes used.

## Applied

## Proposed

## Needs your call

## Findings

## Watermark

generation: 1
EOF
)
curl -sf -X POST "$BRUNN_API_URL/v1/workspace/write" \
  -H "Authorization: Bearer $BRUNN_API_TOKEN" \
  -H 'Content-Type: application/json' \
  --data "$(printf '%s' "$BODY" | python3 -c 'import json,sys;print(json.dumps({"path":sys.argv[1],"content":sys.stdin.read(),"expected_version":0}))' "$RUN_PATH")" \
  > /dev/null
exit 0
"#;

fn test_config(stub: PathBuf, work_root: PathBuf, api_url: &str) -> DreamerConfig {
    let mut host_env: BTreeMap<String, String> = BTreeMap::new();
    host_env.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    // Must never reach codex: the env-strip gate is part of this contract.
    host_env.insert("OPENAI_API_KEY".into(), "sk-forbidden".into());
    DreamerConfig {
        api_url: api_url.to_owned(),
        workspace_token: "sl_workspace_test".into(),
        runner_token: "sl_runner_test".into(),
        codex_path: stub,
        codex_model: "test-model".into(),
        mcp_server_entry: PathBuf::from("/dev/null"),
        work_root,
        host_env,
        time_budget_override: Some(Duration::from_secs(30)),
    }
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 30).expect("date")
}

fn seed_control(shared: &Shared, mode: &str, advance_after: &str) {
    let mut state = shared.lock().expect("mock state");
    let content = format!("enabled: true\nmode: {mode}\nadvance_after: {advance_after}\n");
    record_write(&mut state, CONTROL_PATH, &content);
}

fn seed_auth(shared: &Shared) {
    shared.lock().expect("mock state").secrets.insert(
        AUTH_SECRET.to_owned(),
        r#"{"tokens":{"account_id":"acct_test","access_token":"tok"}}"#.to_owned(),
    );
}

fn workspace_write_count(shared: &Shared) -> usize {
    shared.lock().expect("mock state").write_count
}

fn notification_titles(shared: &Shared) -> Vec<String> {
    shared
        .lock()
        .expect("mock state")
        .notifications
        .iter()
        .filter_map(|n| n["title"].as_str().map(str::to_owned))
        .collect()
}

async fn build(behavior: &str, chatgpt_login: bool) -> (Shared, Dreamer, tempfile::TempDir) {
    let (shared, api_url) = start_mock().await;
    let dir = tempfile::tempdir().expect("test dir");
    let stub = write_stub(dir.path(), behavior, chatgpt_login);
    let config = test_config(stub, dir.path().join("work"), &api_url);
    (shared.clone(), Dreamer::new(config), dir)
}

#[tokio::test]
async fn control_fail_closed_means_zero_writes() {
    let (shared, dreamer, _dir) = build(HAPPY_BEHAVIOR, true).await;
    // Missing file, malformed line, unknown mode, disabled — all identical.
    for control in [
        None,
        Some("enabled true\nmode: full\nadvance_after: 2026-09-05\n"),
        Some("enabled: true\nmode: aggressive\nadvance_after: 2026-09-05\n"),
        Some("enabled: false\nmode: full\nadvance_after: 2026-09-05\n"),
    ] {
        {
            let mut state = shared.lock().expect("mock state");
            state.files.remove(CONTROL_PATH);
            if let Some(content) = control {
                let version = 1;
                state
                    .files
                    .insert(CONTROL_PATH.to_owned(), (content.to_owned(), version));
            }
            state.write_count = 0;
        }
        seed_auth(&shared);
        let report = dreamer.run_once(today(), RunKind::Nightly).await;
        assert!(
            matches!(report.outcome, RunOutcome::Disabled { .. }),
            "expected disabled for {control:?}, got {:?}",
            report.outcome
        );
        assert_eq!(workspace_write_count(&shared), 0, "for {control:?}");
    }
}

#[tokio::test]
async fn missing_vault_auth_skips_before_any_write() {
    let (shared, dreamer, _dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    shared.lock().expect("mock state").write_count = 0;
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::SkippedAuth { .. }));
    assert_eq!(workspace_write_count(&shared), 0);
    assert!(
        notification_titles(&shared)
            .iter()
            .any(|title| title.contains("not connected"))
    );
}

#[tokio::test]
async fn api_key_login_skips_and_env_is_stripped() {
    let (shared, dreamer, dir) = build(HAPPY_BEHAVIOR, false).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    shared.lock().expect("mock state").write_count = 0;
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(matches!(report.outcome, RunOutcome::SkippedAuth { .. }));
    assert_eq!(workspace_write_count(&shared), 0);
    assert!(
        notification_titles(&shared)
            .iter()
            .any(|title| title.contains("auth failed"))
    );
    // No exec ever ran, so no env files exist — the strip check runs in the
    // happy-path test below.
    assert!(!dir.path().join("exec-0.env").exists());
}

#[tokio::test]
async fn rate_limited_probe_skips_before_any_write() {
    let behavior = "echo 'You have hit your usage limit for the plan.'; exit 1\n";
    let (shared, dreamer, _dir) = build(behavior, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    shared.lock().expect("mock state").write_count = 0;
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::SkippedLimits);
    assert_eq!(workspace_write_count(&shared), 0);
    assert!(
        notification_titles(&shared)
            .iter()
            .any(|title| title.contains("plan limits"))
    );
}

#[tokio::test]
async fn happy_path_completes_and_strips_the_environment() {
    let (shared, dreamer, dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    {
        let mut state = shared.lock().expect("mock state");
        let decisions = "- 2026-08-29 veto 2026-08-28/2 — wrong person\n";
        record_write(&mut state, DECISIONS_PATH, decisions);
    }
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    assert!(report.confinement_violations.is_empty());
    // The run file exists in the workspace.
    let state = shared.lock().expect("mock state");
    assert!(state.files.contains_key("dreams/runs/2026-08-30.md"));
    drop(state);
    // Decisions reached the prompt verbatim.
    let main_prompt =
        std::fs::read_to_string(dir.path().join("exec-1.stdin")).expect("main exec prompt");
    assert!(main_prompt.contains("veto 2026-08-28/2"));
    assert!(main_prompt.contains("VERBATIM"));
    assert!(main_prompt.contains("Needs your call"));
    // The environment reaching codex was stripped of the API key and carried
    // the ephemeral home.
    let exec_env = std::fs::read_to_string(dir.path().join("exec-1.env")).expect("exec env");
    assert!(!exec_env.contains("OPENAI_API_KEY"));
    assert!(exec_env.contains("CODEX_HOME="));
}

#[tokio::test]
async fn advance_flips_control_and_notifies() {
    let (shared, dreamer, dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-08-30");
    seed_auth(&shared);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(report.mode_flipped);
    let state = shared.lock().expect("mock state");
    let (control, _) = state.files.get(CONTROL_PATH).expect("control");
    assert!(control.contains("mode: full"));
    drop(state);
    assert!(
        notification_titles(&shared)
            .iter()
            .any(|title| title.contains("advanced to full mode"))
    );
    let main_prompt =
        std::fs::read_to_string(dir.path().join("exec-1.stdin")).expect("main exec prompt");
    assert!(main_prompt.contains("Apply last run's unvetoed Proposed items first"));
}

#[tokio::test]
async fn hold_advance_blocks_the_flip() {
    let (shared, dreamer, _dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-08-30");
    seed_auth(&shared);
    {
        let mut state = shared.lock().expect("mock state");
        record_write(&mut state, DECISIONS_PATH, "- 2026-08-29 hold-advance\n");
    }
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(!report.mode_flipped);
    let state = shared.lock().expect("mock state");
    let (control, _) = state.files.get(CONTROL_PATH).expect("control");
    assert!(control.contains("mode: report-only"));
}

#[tokio::test]
async fn cas_conflict_retries_once_then_defers() {
    // One conflict: the flip retries and succeeds.
    let (shared, dreamer, _dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-08-30");
    seed_auth(&shared);
    shared
        .lock()
        .expect("mock state")
        .conflicts
        .insert(CONTROL_PATH.to_owned(), 1);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(report.mode_flipped, "one conflict must be retried");

    // Persistent conflicts: the flip is deferred and the run proceeds
    // report-only.
    let (shared, dreamer, _dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-08-30");
    seed_auth(&shared);
    shared
        .lock()
        .expect("mock state")
        .conflicts
        .insert(CONTROL_PATH.to_owned(), 99);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(!report.mode_flipped);
    assert!(matches!(
        report.outcome,
        RunOutcome::Completed | RunOutcome::Partial { .. }
    ));
}

#[tokio::test]
async fn kill_timer_yields_partial_with_fallback_run_file() {
    let behavior = r#"
if [ "$N" = "0" ]; then echo READY; exit 0; fi
sleep 20
exit 0
"#;
    let (shared, dreamer, dir) = build(behavior, true).await;
    let config = test_config(
        dir.path().join("codex"),
        dir.path().join("work"),
        dreamer.workspace.base_url(),
    );
    let dreamer = Dreamer::new(DreamerConfig {
        time_budget_override: Some(Duration::from_secs(2)),
        ..config
    });
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(
        matches!(report.outcome, RunOutcome::Partial { .. }),
        "{report:?}"
    );
    let state = shared.lock().expect("mock state");
    let (run_file, _) = state
        .files
        .get("dreams/runs/2026-08-30.md")
        .expect("fallback run file");
    assert!(run_file.contains("Status: partial."));
}

#[tokio::test]
async fn codex_death_yields_failed_with_fallback_run_file() {
    let behavior = r#"
if [ "$N" = "0" ]; then echo READY; exit 0; fi
echo 'boom' >&2
exit 1
"#;
    let (shared, dreamer, _dir) = build(behavior, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert!(
        matches!(report.outcome, RunOutcome::Failed { .. }),
        "{report:?}"
    );
    let state = shared.lock().expect("mock state");
    let (run_file, _) = state
        .files
        .get("dreams/runs/2026-08-30.md")
        .expect("fallback run file");
    assert!(run_file.contains("Status: failed."));
}

#[tokio::test]
async fn confinement_cross_check_reports_stray_writes() {
    // The exec writes its run file AND a file outside the allowed surfaces
    // that the run file does not enumerate.
    let behavior = r#"
if [ "$N" = "0" ]; then echo READY; exit 0; fi
write() {
  curl -sf -X POST "$BRUNN_API_URL/v1/workspace/write" \
    -H "Authorization: Bearer $BRUNN_API_TOKEN" \
    -H 'Content-Type: application/json' \
    --data "{\"path\":\"$1\",\"content\":\"$2\"}" > /dev/null
}
write "sources/Projects/Sneaky.md" "should not happen"
RUN_PATH=$(grep -o 'dreams/runs/[0-9-]*\.md' "$DIR/exec-$N.stdin" | tail -1)
write "$RUN_PATH" "A run happened.\nLine two.\nLine three.\nLine four.\nLine five.\n\n## Watermark\n\ngeneration: 5\n"
exit 0
"#;
    let (shared, dreamer, _dir) = build(behavior, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert_eq!(
        report.confinement_violations,
        vec!["sources/Projects/Sneaky.md".to_owned()]
    );
    assert!(
        notification_titles(&shared)
            .iter()
            .any(|title| title.contains("outside its surfaces"))
    );
}

#[tokio::test]
async fn refreshed_tokens_are_persisted_back_to_the_vault() {
    // The stub rewrites auth.json during the run, as codex does on refresh.
    let behavior = r#"
if [ "$N" = "0" ]; then echo READY; exit 0; fi
echo '{"tokens":{"account_id":"acct_test","access_token":"refreshed"}}' > "$CODEX_HOME/auth.json"
RUN_PATH=$(grep -o 'dreams/runs/[0-9-]*\.md' "$DIR/exec-$N.stdin" | tail -1)
curl -sf -X POST "$BRUNN_API_URL/v1/workspace/write" \
  -H "Authorization: Bearer $BRUNN_API_TOKEN" \
  -H 'Content-Type: application/json' \
  --data "{\"path\":\"$RUN_PATH\",\"content\":\"Done.\n\n## Watermark\n\ngeneration: 2\n\",\"expected_version\":0}" > /dev/null
exit 0
"#;
    let (shared, dreamer, _dir) = build(behavior, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");
    let state = shared.lock().expect("mock state");
    assert!(
        state
            .secrets
            .get(AUTH_SECRET)
            .expect("auth secret")
            .contains("refreshed")
    );
}

/// Location × dreaming coherence contract (2026-09-03): structured evidence
/// never enters the change set, so it never reaches LINKS, VIEWS, FRESHNESS,
/// or neighborhood selection.
#[tokio::test]
async fn structured_evidence_never_reaches_the_change_set() {
    let (shared, dreamer, dir) = build(HAPPY_BEHAVIOR, true).await;
    seed_control(&shared, "report-only", "2026-12-01");
    seed_auth(&shared);
    {
        let mut state = shared.lock().expect("mock state");
        // Previous run file whose watermark is its own generation (2, after
        // CONTROL at 1): everything written below is newer than it.
        record_write(
            &mut state,
            "dreams/runs/2026-08-29.md",
            "Done.\n\n## Watermark\n\ngeneration: 2\n",
        );
        record_write(
            &mut state,
            "Location/Visits/2026-09.md",
            "---\nkind: location-visits\nmonth: 2026-09\n---\n\
             | Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- |\n\
             | 2026-09-02T08:30-07:00 | 2026-09-02T13:30-07:00 | 5h00m | Crystal Mountain | resort | Enumclaw, WA, US | high | 46.9350,-121.4740 |\n",
        );
        record_write(
            &mut state,
            "Location/Places.md",
            "---\nkind: location-places\n---\n| Label | Kind | Lat | Lon | Radius m |\n| --- | --- | --- | --- | --- |\n",
        );
        record_write(
            &mut state,
            "sources/Projects/Crystal Mountain season.md",
            "# Crystal Mountain season\n\nSki notes mentioning Crystal Mountain and Home.\n",
        );
        // A second month-file version, as every ping-driven transition produces.
        record_write(
            &mut state,
            "Location/Visits/2026-09.md",
            "---\nkind: location-visits\nmonth: 2026-09\n---\n\
             | Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n\
             | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        state.secrets.insert(
            RUNTIME_SECRET.to_owned(),
            r#"{"last_run_date":"2026-08-29"}"#.to_owned(),
        );
    }
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert_eq!(report.outcome, RunOutcome::Completed, "{report:?}");

    let main_prompt =
        std::fs::read_to_string(dir.path().join("exec-1.stdin")).expect("main exec prompt");
    let change_set_section = main_prompt
        .split("# CHANGE SET")
        .nth(1)
        .and_then(|rest| rest.split("# WORK").next())
        .expect("prompt has a change set section");
    assert!(
        change_set_section.contains("- sources/Projects/Crystal Mountain season.md"),
        "{change_set_section}"
    );
    assert!(change_set_section.contains("exactly the 1 paths listed below"));
    assert!(!change_set_section.contains("Location/Visits/2026-09.md"));
    assert!(!change_set_section.contains("Location/Places.md"));
    assert!(!change_set_section.contains("dreams/runs/2026-08-29.md\n- "));
    assert!(!main_prompt.contains("since_generation="));
}

/// The write gate: the dreamer's location paths are confinement violations
/// even when the run file enumerates them as applied.
#[tokio::test]
async fn write_gate_rejects_location_paths_even_when_enumerated() {
    let behavior = r#"
if [ "$N" = "0" ]; then echo READY; exit 0; fi
write() {
  curl -sf -X POST "$BRUNN_API_URL/v1/workspace/write" \
    -H "Authorization: Bearer $BRUNN_API_TOKEN" \
    -H 'Content-Type: application/json' \
    --data "{\"path\":\"$1\",\"content\":\"$2\"}" > /dev/null
}
write "Location/Visits/2026-09.md" "rewritten history"
write "Location/Places.md" "rewritten places"
write "derived/entities/crystal-mountain.md" "Visit history: Location/Visits/"
RUN_PATH=$(grep -o 'dreams/runs/[0-9-]*\.md' "$DIR/exec-$N.stdin" | tail -1)
write "$RUN_PATH" "A run happened.\nLine two.\nLine three.\nLine four.\nLine five.\n\n## Applied\n- Location/Visits/2026-09.md@2 — tidied\n- Location/Places.md@2 — radius\n\n## Watermark\n\ngeneration: 5\n"
exit 0
"#;
    let (shared, dreamer, _dir) = build(behavior, true).await;
    seed_control(&shared, "full", "2026-08-01");
    seed_auth(&shared);
    let report = dreamer.run_once(today(), RunKind::Nightly).await;
    assert_eq!(
        report.confinement_violations,
        vec![
            "Location/Visits/2026-09.md".to_owned(),
            "Location/Places.md".to_owned(),
        ]
    );
    assert!(
        notification_titles(&shared)
            .iter()
            .any(|title| title.contains("outside its surfaces"))
    );
}

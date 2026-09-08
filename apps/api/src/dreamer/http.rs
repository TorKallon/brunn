//! The dreamer's private HTTP surface and the nightly scheduler.
//!
//! Reached only by the API over private networking with the shared internal
//! token. This is not a public surface: no CORS, no sessions, no capability
//! model — one bearer token, six endpoints.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{NaiveDate, NaiveTime, TimeZone as _, Utc};
use chrono_tz::America::Los_Angeles;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use super::{
    connect::ConnectFlow,
    run::{Dreamer, RunKind, RunReport},
};

pub struct DreamerApp {
    pub dreamer: Dreamer,
    pub connect: Arc<ConnectFlow>,
    pub internal_token: String,
    /// Held for the duration of a run; the scheduler and /run both skip when
    /// a run is already in flight.
    run_lock: Arc<Mutex<()>>,
    last_report: Mutex<Option<RunReport>>,
}

impl DreamerApp {
    pub fn new(dreamer: Dreamer, internal_token: String) -> Arc<Self> {
        Arc::new(Self {
            dreamer,
            connect: ConnectFlow::new(),
            internal_token,
            run_lock: Arc::new(Mutex::new(())),
            last_report: Mutex::new(None),
        })
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/healthz", get(healthz))
            .route("/status", get(status))
            .route("/connect/start", post(connect_start))
            .route("/connect/wait", get(connect_wait))
            .route("/disconnect", post(disconnect))
            .route("/verify", post(verify))
            .route("/run", post(run_now))
            .with_state(self.clone())
    }

    /// Today in the run's home timezone.
    pub fn today() -> NaiveDate {
        Utc::now().with_timezone(&Los_Angeles).date_naive()
    }

    pub async fn execute(self: &Arc<Self>, kind: RunKind) -> Option<RunReport> {
        let Ok(_guard) = self.run_lock.try_lock() else {
            return None;
        };
        let report = self.dreamer.run_once(Self::today(), kind).await;
        *self.last_report.lock().await = Some(report.clone());
        Some(report)
    }

    /// One bounded startup catch-up for the latest due slot, then nightly.
    /// Durable server admission deduplicates completed slots across restarts.
    pub async fn nightly_loop(self: Arc<Self>) {
        let now = Utc::now().with_timezone(&Los_Angeles);
        let today = now.date_naive();
        let due = if now.time() >= NaiveTime::from_hms_opt(3, 0, 0).unwrap() {
            today
        } else {
            today.pred_opt().unwrap_or(today)
        };
        let runtime = self.dreamer.runtime_status().await;
        if runtime.last_run_date.as_deref() != Some(&due.format("%Y-%m-%d").to_string()) {
            if let Ok(_guard) = self.run_lock.try_lock() {
                let report = self.dreamer.run_once(due, RunKind::Nightly).await;
                *self.last_report.lock().await = Some(report);
            }
        }
        loop {
            let now = Utc::now().with_timezone(&Los_Angeles);
            let three_am = NaiveTime::from_hms_opt(3, 0, 0).expect("03:00");
            let mut next = now.date_naive().and_time(three_am);
            if now.time() >= three_am {
                next += chrono::Duration::days(1);
            }
            // DST gaps/overlaps: take the earliest valid interpretation, or
            // slide forward an hour if 03:00 does not exist that day.
            let next_local = match Los_Angeles.from_local_datetime(&next) {
                chrono::LocalResult::Single(when) | chrono::LocalResult::Ambiguous(when, _) => when,
                chrono::LocalResult::None => Los_Angeles
                    .from_local_datetime(&(next + chrono::Duration::hours(1)))
                    .earliest()
                    .expect("04:00 exists"),
            };
            let wait = (next_local.with_timezone(&Utc) - Utc::now())
                .to_std()
                .unwrap_or_default();
            tracing::info!(?wait, "dreamer sleeping until the next nightly run");
            tokio::time::sleep(wait).await;
            match self.execute(RunKind::Nightly).await {
                Some(report) => {
                    tracing::info!(
                        outcome = report.outcome.label(),
                        "nightly dreaming run finished"
                    );
                }
                None => tracing::warn!("nightly run skipped: another run is in flight"),
            }
        }
    }
}

fn authorized(app: &DreamerApp, headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| {
            // Constant-time-ish comparison; the token is high entropy.
            token.len() == app.internal_token.len()
                && token
                    .bytes()
                    .zip(app.internal_token.bytes())
                    .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
        })
}

macro_rules! require_auth {
    ($app:expr, $headers:expr) => {
        if !authorized(&$app, &$headers) {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"})))
                .into_response();
        }
    };
}

async fn healthz() -> &'static str {
    "ok"
}

async fn status(State(app): State<Arc<DreamerApp>>, headers: HeaderMap) -> Response {
    require_auth!(app, headers);
    let runtime = app.dreamer.runtime_status().await;
    let connect = app.connect.current().await;
    let last_report = app.last_report.lock().await.clone();
    Json(json!({
        "connect": connect,
        "runtime": runtime,
        "last_report": last_report,
    }))
    .into_response()
}

async fn connect_start(State(app): State<Arc<DreamerApp>>, headers: HeaderMap) -> Response {
    require_auth!(app, headers);
    Json(app.connect.start(&app.dreamer).await).into_response()
}

async fn connect_wait(State(app): State<Arc<DreamerApp>>, headers: HeaderMap) -> Response {
    require_auth!(app, headers);
    Json(app.connect.wait(&app.dreamer).await).into_response()
}

async fn disconnect(State(app): State<Arc<DreamerApp>>, headers: HeaderMap) -> Response {
    require_auth!(app, headers);
    Json(app.connect.disconnect(&app.dreamer).await).into_response()
}

async fn verify(State(app): State<Arc<DreamerApp>>, headers: HeaderMap) -> Response {
    require_auth!(app, headers);
    // Read recorded verification; this endpoint does not run a model probe.
    let runtime = app.dreamer.runtime_status().await;
    Json(json!({"runtime": runtime,"verification":"recorded","live_probe":false})).into_response()
}

#[derive(Deserialize)]
struct RunRequest {
    #[serde(default)]
    kind: Option<String>,
}

async fn run_now(
    State(app): State<Arc<DreamerApp>>,
    headers: HeaderMap,
    body: Option<Json<RunRequest>>,
) -> Response {
    require_auth!(app, headers);
    let kind = match body.and_then(|Json(request)| request.kind).as_deref() {
        Some("backfill") => RunKind::Backfill,
        _ => RunKind::Manual,
    };
    let Ok(guard) = app.run_lock.clone().try_lock_owned() else {
        return (StatusCode::CONFLICT, Json(json!({"status":"busy"}))).into_response();
    };
    let app_for_run = app.clone();
    tokio::spawn(async move {
        let _guard = guard;
        let report = app_for_run
            .dreamer
            .run_once(DreamerApp::today(), kind)
            .await;
        *app_for_run.last_report.lock().await = Some(report);
    });
    // Admission is asynchronous and authoritative at the API. Never tell the
    // caller an attempt started before its server lease has been accepted.
    (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_token_comparison() {
        let app = DreamerApp {
            dreamer: Dreamer::new(super::super::run::DreamerConfig {
                api_url: "http://localhost".into(),
                model_token: "test-model-read-only".into(),
                workspace_token: "w".into(),
                runner_token: "r".into(),
                codex_path: "/usr/bin/true".into(),
                codex_model: "test".into(),
                mcp_server_entry: "/dev/null".into(),
                work_root: std::env::temp_dir(),
                host_env: Default::default(),
                time_budget_override: None,
            }),
            connect: ConnectFlow::new(),
            internal_token: "secret-token".into(),
            run_lock: Arc::new(Mutex::new(())),
            last_report: Mutex::new(None),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer secret-token".parse().expect("header"),
        );
        assert!(authorized(&app, &headers));
        headers.insert(
            "authorization",
            "Bearer wrong-tokenn".parse().expect("header"),
        );
        assert!(!authorized(&app, &headers));
        assert!(!authorized(&app, &HeaderMap::new()));
    }
}

//! Owner-facing dreaming routes for the SPA: status, Connect, Disconnect,
//! Pause, Resume.
//!
//! Pause/Resume rewrite `dreams/CONTROL.md` through the ordinary workspace
//! write path (CAS included). Connect, Disconnect, and live status proxy to
//! the dreamer service over private networking; without a configured dreamer
//! the file-backed parts still work and the proxy parts report unavailable.

use std::sync::OnceLock;

use axum::{
    Extension, Json, Router,
    extract::State,
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::{
    auth::AuthContext,
    db::AppState,
    dreamer::control::{self, ControlState, Mode},
    error::{ApiError, ApiResult},
    models::Capability,
    simple_core::{self, ReadItem, ReadRequest, WriteRequest},
};

const CONTROL_PATH: &str = "dreams/CONTROL.md";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspace/dreaming/status", get(status))
        .route("/workspace/dreaming/connect/start", post(connect_start))
        .route("/workspace/dreaming/connect/wait", get(connect_wait))
        .route("/workspace/dreaming/disconnect", post(disconnect))
        .route("/workspace/dreaming/pause", post(pause))
        .route("/workspace/dreaming/resume", post(resume))
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("reqwest client")
    })
}

async fn proxy(state: &AppState, method: reqwest::Method, path: &str) -> ApiResult<Value> {
    let (Some(url), Some(token)) = (
        state.config.dreamer_internal_url.as_deref(),
        state.config.dreamer_internal_token.as_deref(),
    ) else {
        return Err(ApiError::public(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "dreamer_unavailable",
            "the dreamer service is not configured",
        ));
    };
    let response = http()
        .request(method, format!("{}{path}", url.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, path, "dreamer proxy request failed");
            ApiError::public(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "dreamer_unavailable",
                "the dreamer service is unreachable",
            )
        })?;
    if !response.status().is_success() {
        return Err(ApiError::public(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "dreamer_unavailable",
            "the dreamer service refused the request",
        ));
    }
    response.json().await.map_err(|_| {
        ApiError::public(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "dreamer_unavailable",
            "the dreamer service returned an invalid response",
        )
    })
}

async fn read_control(state: &AppState, auth: &AuthContext) -> ApiResult<(Option<String>, i64)> {
    let request = ReadRequest {
        session_id: None,
        requests: vec![ReadItem {
            reference: None,
            path: Some(CONTROL_PATH.to_owned()),
            link_target: None,
            view: None,
            start: None,
            end: None,
            max_chars: None,
            version: None,
        }],
    };
    let envelope = simple_core::read(State(state.clone()), Extension(auth.clone()), Json(request))
        .await?
        .0;
    let item = envelope
        .data
        .pointer("/items/0")
        .cloned()
        .unwrap_or(Value::Null);
    if item.get("status").and_then(Value::as_str) == Some("not_found") {
        return Ok((None, 0));
    }
    let content = item.get("text").and_then(Value::as_str).map(str::to_owned);
    let version = item.get("version").and_then(Value::as_i64).unwrap_or(0);
    Ok((content, version))
}

async fn write_control(
    state: &AppState,
    auth: &AuthContext,
    content: String,
    expected_version: i64,
) -> ApiResult<()> {
    let request = WriteRequest {
        path: CONTROL_PATH.to_owned(),
        content,
        media_type: "text/markdown".to_owned(),
        expected_version: Some(expected_version),
        idempotency_key: None,
        metadata: Value::Null,
    };
    let _ =
        simple_core::write(State(state.clone()), Extension(auth.clone()), Json(request)).await?;
    Ok(())
}

fn control_view(content: Option<&str>) -> Value {
    match control::parse(content) {
        ControlState::Enabled(control) => json!({
            "enabled": true,
            "mode": control.mode.as_str(),
            "advance_after": null,
        }),
        ControlState::Disabled { reason } => json!({
            "enabled": false,
            "reason": reason,
        }),
    }
}

pub async fn status(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    let (control_content, _) = read_control(&state, &auth).await?;
    let dreamer = match proxy(&state, reqwest::Method::GET, "/status").await {
        Ok(body) => body,
        Err(_) => json!({"unavailable": true}),
    };
    Ok(Json(json!({
        "control": control_view(control_content.as_deref()),
        "dreamer": dreamer,
    })))
}

pub async fn connect_start(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    proxy(&state, reqwest::Method::POST, "/connect/start")
        .await
        .map(Json)
}

pub async fn connect_wait(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    proxy(&state, reqwest::Method::GET, "/connect/wait")
        .await
        .map(Json)
}

pub async fn disconnect(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    proxy(&state, reqwest::Method::POST, "/disconnect")
        .await
        .map(Json)
}

/// Pause preserves a well-formed mode and removes retired calendar metadata.
pub async fn pause(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    let (content, version) = read_control(&state, &auth).await?;
    let mode = parse_ignoring_enabled(content.as_deref()).unwrap_or(Mode::ReportOnly);
    let rendered = control::render(false, mode, None);
    write_control(&state, &auth, rendered.clone(), version).await?;
    Ok(Json(json!({"control": control_view(Some(&rendered))})))
}

/// Resume: `enabled: true`; a missing or malformed file becomes a fresh
/// report-only CONTROL. Calendar passage never changes the mode.
pub async fn resume(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<Value>> {
    auth.require(Capability::CredentialManage)?;
    let (content, version) = read_control(&state, &auth).await?;
    let mode = parse_ignoring_enabled(content.as_deref()).unwrap_or(Mode::ReportOnly);
    let rendered = control::render(true, mode, None);
    write_control(&state, &auth, rendered.clone(), version).await?;
    Ok(Json(json!({"control": control_view(Some(&rendered))})))
}

/// A paused CONTROL parses as Disabled; recover its explicitly chosen mode.
fn parse_ignoring_enabled(content: Option<&str>) -> Option<Mode> {
    let content = content?;
    let forced = content
        .lines()
        .map(|line| {
            if line
                .split_once(':')
                .is_some_and(|(key, value)| key.trim() == "enabled" && value.trim() == "false")
            {
                "enabled: true"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    match control::parse(Some(&forced)) {
        ControlState::Enabled(control) => Some(control.mode),
        ControlState::Disabled { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_recovers_mode_without_restoring_a_calendar_transition() {
        let paused = "enabled: false\nmode: full\nadvance_after: 2026-09-05\n";
        let mode = parse_ignoring_enabled(Some(paused)).expect("recoverable control");
        assert_eq!(mode, Mode::Full);
        assert_eq!(
            control::render(true, mode, None),
            "enabled: true\nmode: full\n"
        );
        assert_eq!(
            parse_ignoring_enabled(Some("enabled:false\nmode: report-only\n")),
            Some(Mode::ReportOnly)
        );
        assert!(parse_ignoring_enabled(Some("garbage")).is_none());
        assert!(parse_ignoring_enabled(None).is_none());
    }

    #[test]
    fn control_view_renders_both_states() {
        let enabled = control_view(Some(
            "enabled: true\nmode: report-only\nadvance_after: 2026-09-05\n",
        ));
        assert_eq!(enabled["enabled"], Value::Bool(true));
        assert_eq!(enabled["mode"], Value::String("report-only".into()));
        assert_eq!(enabled["advance_after"], Value::Null);
        assert_eq!(
            control_view(Some("enabled: true\nmode: report-only\n"))["enabled"],
            true
        );
        let disabled = control_view(None);
        assert_eq!(disabled["enabled"], Value::Bool(false));
    }
}

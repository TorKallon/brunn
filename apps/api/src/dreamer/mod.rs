//! The Straylight dreamer: one nightly consolidation run, a Connect flow,
//! and a private HTTP surface — a pure HTTP client of the Straylight API.
//!
//! Design: Documents/dreaming-lean-design-2026-08-28 (rev 3). The wrapper is
//! deterministic; all judgment lives in the `codex exec` prompt. Dreaming
//! state lives in workspace files (`dreams/`, `derived/`) and the secrets
//! vault — never in this process and never in new database tables.

pub mod client;
pub mod codex;
pub mod connect;
pub mod control;
pub mod decisions;
pub mod http;
pub mod prompt;
pub mod run;
pub mod runfile;

use std::collections::BTreeMap;

use run::DreamerConfig;

/// Build the dreamer's configuration from the environment. The dreamer never
/// reads the API's `Config`: it is a client, and its knobs are its own.
pub fn config_from_env() -> Result<DreamerConfig, String> {
    let require =
        |name: &str| std::env::var(name).map_err(|_| format!("{name} is required for the dreamer"));
    let host_env: BTreeMap<String, String> = std::env::vars().collect();
    Ok(DreamerConfig {
        api_url: require("STRAYLIGHT_API_URL")?,
        workspace_token: require("DREAMER_WORKSPACE_TOKEN")?,
        runner_token: require("DREAMER_RUNNER_TOKEN")?,
        codex_path: std::env::var("DREAMER_CODEX_PATH")
            .unwrap_or_else(|_| "codex".to_owned())
            .into(),
        codex_model: std::env::var("DREAMER_CODEX_MODEL")
            .unwrap_or_else(|_| "gpt-5-codex".to_owned()),
        mcp_server_entry: require("DREAMER_MCP_ENTRY")?.into(),
        work_root: std::env::var("DREAMER_WORK_ROOT")
            .unwrap_or_else(|_| "/tmp/dreamer".to_owned())
            .into(),
        host_env,
        time_budget_override: None,
    })
}

/// The bind address for `dreamer serve`.
pub fn bind_from_env() -> String {
    std::env::var("DREAMER_BIND").unwrap_or_else(|_| "0.0.0.0:8090".to_owned())
}

/// The shared token the API must present on the private surface.
pub fn internal_token_from_env() -> Result<String, String> {
    std::env::var("DREAMER_INTERNAL_TOKEN")
        .map_err(|_| "DREAMER_INTERNAL_TOKEN is required for the dreamer".to_owned())
}

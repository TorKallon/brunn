//! Codex subprocess plumbing: subscription-only environment stripping, the
//! ChatGPT-plan login check, and `codex exec` command construction.
//!
//! The environment and auth rules port the tested harness behavior from
//! `agent_work_eval.py` (`subscription_reasoning_environment`,
//! `require_codex_subscription`): the dreamer's model calls are
//! ChatGPT-subscription Codex, never an API key, and the run fails closed.

use std::{collections::BTreeMap, path::Path, time::Duration};

use tokio::process::Command;

/// Environment variables that must never reach a codex subprocess.
const EXPLICIT_DENIALS: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENAI_API_BASE",
    "OPENAI_BASE_URL",
    "OPENAI_ORGANIZATION",
    "OPENAI_ORG_ID",
    "OPENAI_PROJECT",
    "OPENAI_PROJECT_ID",
    "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT",
    "CODEX_API_KEY",
    "BRUNN_STATE_EVAL_DIRECT_OPENAI",
];

/// The exact login line codex must report for subscription auth.
pub const CHATGPT_LOGIN_LINE: &str = "Logged in using ChatGPT";

/// Remove every API-key-shaped or routing-override variable so codex can only
/// authenticate through the ChatGPT subscription in `CODEX_HOME`.
pub fn strip_reasoning_env(env: &mut BTreeMap<String, String>) {
    env.retain(|key, _| {
        let upper = key.to_uppercase();
        let denied = EXPLICIT_DENIALS.contains(&upper.as_str())
            || upper.starts_with("OPENAI_")
            || upper.starts_with("AZURE_OPENAI_")
            || upper.ends_with("_API_KEY")
            || (upper.starts_with("CODEX_")
                && [
                    "API_BASE",
                    "BASE_URL",
                    "ENDPOINT",
                    "ORGANIZATION",
                    "PROJECT",
                ]
                .iter()
                .any(|marker| upper.contains(marker)));
        !denied
    });
}

/// Build the minimal subprocess environment for codex: an allowlisted copy of
/// the host environment plus the ephemeral `HOME`/`CODEX_HOME`, stripped of
/// anything API-key-shaped.
pub fn codex_environment(
    host: &BTreeMap<String, String>,
    home: &Path,
    codex_home: &Path,
) -> BTreeMap<String, String> {
    const ALLOWED_HOST_KEYS: &[&str] = &[
        "PATH",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TERM",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
    ];
    let mut env: BTreeMap<String, String> = ALLOWED_HOST_KEYS
        .iter()
        .filter_map(|key| {
            host.get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect();
    env.entry("PATH".to_owned())
        .or_insert_with(|| "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_owned());
    env.insert("HOME".to_owned(), home.display().to_string());
    env.insert("CODEX_HOME".to_owned(), codex_home.display().to_string());
    // Plain output keeps the Connect flow's device-prompt parsing simple.
    env.insert("NO_COLOR".to_owned(), "1".to_owned());
    strip_reasoning_env(&mut env);
    env
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexIdentity {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthCheck {
    ChatGpt(CodexIdentity),
    Refused { detail: String },
}

/// `codex login status` must report a ChatGPT-plan login. Anything else —
/// a nonzero exit, an API-key login, a missing binary, a hang — refuses the
/// run (the caller exits as `skipped(auth)` before any write).
pub async fn verify_subscription(codex: &Path, env: &BTreeMap<String, String>) -> AuthCheck {
    let status = match run_with_timeout(codex, &["login", "status"], env).await {
        Ok(output) => output,
        Err(detail) => return AuthCheck::Refused { detail },
    };
    let has_chatgpt_line = status
        .rendered
        .lines()
        .any(|line| line.trim() == CHATGPT_LOGIN_LINE);
    if !status.success || !has_chatgpt_line {
        return AuthCheck::Refused {
            detail: format!(
                "codex login status did not report a ChatGPT-plan login: {}",
                first_line(&status.rendered)
            ),
        };
    }
    let version = match run_with_timeout(codex, &["--version"], env).await {
        Ok(output) if output.success && !output.rendered.trim().is_empty() => {
            first_line(&output.rendered).to_owned()
        }
        Ok(_) | Err(_) => {
            return AuthCheck::Refused {
                detail: "could not record the codex version".to_owned(),
            };
        }
    };
    AuthCheck::ChatGpt(CodexIdentity { version })
}

struct CommandOutput {
    success: bool,
    rendered: String,
}

async fn run_with_timeout(
    codex: &Path,
    args: &[&str],
    env: &BTreeMap<String, String>,
) -> Result<CommandOutput, String> {
    let mut command = Command::new(codex);
    command.args(args).env_clear().envs(env).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .map_err(|_| format!("codex {} timed out", args.join(" ")))?
        .map_err(|error| format!("could not run codex {}: {error}", args.join(" ")))?;
    let rendered = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_owned();
    Ok(CommandOutput {
        success: output.status.success(),
        rendered,
    })
}

fn first_line(rendered: &str) -> &str {
    rendered.lines().next().unwrap_or_default().trim()
}

/// Arguments for one `codex exec` invocation with the Brunn MCP server
/// on stdio. The MCP server authenticates with the scoped `dreamer` token via
/// forwarded environment variables; codex itself never sees vault-capable
/// auth.
pub struct ExecSpec<'a> {
    pub codex: &'a Path,
    pub model: &'a str,
    pub mcp_server_entry: &'a Path,
    pub working_dir: &'a Path,
    pub last_message_path: &'a Path,
}

pub fn exec_command(spec: &ExecSpec<'_>) -> Vec<String> {
    let forwarded = serde_json::json!(["BRUNN_API_URL", "BRUNN_API_TOKEN"]);
    let args = serde_json::json!([spec.mcp_server_entry.display().to_string()]);
    vec![
        spec.codex.display().to_string(),
        "exec".into(),
        "--ephemeral".into(),
        "--ignore-user-config".into(),
        "--ignore-rules".into(),
        "--disable".into(),
        "apps".into(),
        "--disable".into(),
        "plugins".into(),
        "--disable".into(),
        "remote_plugin".into(),
        "--disable".into(),
        "plugin_sharing".into(),
        "--skip-git-repo-check".into(),
        "--model".into(),
        spec.model.into(),
        "--config".into(),
        "mcp_servers.brunn.command=\"node\"".into(),
        "--config".into(),
        format!("mcp_servers.brunn.args={args}"),
        "--config".into(),
        format!("mcp_servers.brunn.env_vars={forwarded}"),
        "--config".into(),
        "mcp_servers.brunn.startup_timeout_sec=30".into(),
        "--config".into(),
        "mcp_servers.brunn.default_tools_approval_mode=\"approve\"".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        "--cd".into(),
        spec.working_dir.display().to_string(),
        "--output-last-message".into(),
        spec.last_message_path.display().to_string(),
        "--json".into(),
        "-".into(),
    ]
}

/// Whether probe/exec output looks like plan-capacity exhaustion. Matched
/// leniently on the strings codex emits for subscription limits.
pub fn looks_rate_limited(rendered: &str) -> bool {
    let lower = rendered.to_lowercase();
    [
        "rate limit",
        "usage limit",
        "quota",
        "too many requests",
        "429",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn strips_api_key_shaped_variables() {
        let mut env = env_of(&[
            ("OPENAI_API_KEY", "sk-x"),
            ("openai_base_url", "http://proxy"),
            ("AZURE_OPENAI_API_KEY", "az"),
            ("ANTHROPIC_API_KEY", "sk-a"),
            ("CODEX_API_BASE", "http://other"),
            ("CODEX_API_KEY", "ck"),
            ("BRUNN_STATE_EVAL_DIRECT_OPENAI", "1"),
            ("PATH", "/usr/bin"),
            ("HOME", "/home/dreamer"),
            ("CODEX_HOME", "/home/dreamer/.codex"),
        ]);
        strip_reasoning_env(&mut env);
        assert_eq!(
            env.keys().collect::<Vec<_>>(),
            vec!["CODEX_HOME", "HOME", "PATH"]
        );
    }

    #[test]
    fn codex_environment_is_allowlisted_and_ephemeral() {
        let host = env_of(&[
            ("PATH", "/usr/bin"),
            ("OPENAI_API_KEY", "sk-x"),
            ("BRUNN_API_TOKEN", "secret"),
            ("LANG", "en_US.UTF-8"),
        ]);
        let env = codex_environment(
            &host,
            Path::new("/tmp/run/.home"),
            Path::new("/tmp/run/.home/.codex"),
        );
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("LANG").map(String::as_str), Some("en_US.UTF-8"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/tmp/run/.home"));
        assert_eq!(
            env.get("CODEX_HOME").map(String::as_str),
            Some("/tmp/run/.home/.codex")
        );
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert!(!env.contains_key("BRUNN_API_TOKEN"));
    }

    #[tokio::test]
    async fn refuses_api_key_login() {
        let stub = write_stub(
            "#!/bin/sh\nif [ \"$1\" = login ]; then echo 'Logged in using an API key'; fi\nexit 0\n",
        );
        let check = verify_subscription(&stub, &BTreeMap::new()).await;
        assert!(matches!(check, AuthCheck::Refused { .. }));
    }

    #[tokio::test]
    async fn refuses_missing_binary() {
        let check = verify_subscription(Path::new("/nonexistent/codex"), &BTreeMap::new()).await;
        assert!(matches!(check, AuthCheck::Refused { .. }));
    }

    #[tokio::test]
    async fn accepts_chatgpt_login_and_records_version() {
        let stub = write_stub(concat!(
            "#!/bin/sh\n",
            "if [ \"$1\" = login ]; then echo 'Logged in using ChatGPT'; fi\n",
            "if [ \"$1\" = --version ]; then echo 'codex-cli 1.2.3'; fi\n",
            "exit 0\n",
        ));
        let check = verify_subscription(&stub, &BTreeMap::new()).await;
        assert_eq!(
            check,
            AuthCheck::ChatGpt(CodexIdentity {
                version: "codex-cli 1.2.3".to_owned()
            })
        );
    }

    #[test]
    fn exec_command_wires_the_mcp_server() {
        let spec = ExecSpec {
            codex: Path::new("/usr/local/bin/codex"),
            model: "gpt-5-codex",
            mcp_server_entry: Path::new("/srv/mcp/dist/index.js"),
            working_dir: Path::new("/tmp/run"),
            last_message_path: Path::new("/tmp/run/answer.md"),
        };
        let command = exec_command(&spec);
        let rendered = command.join(" ");
        assert!(rendered.contains("mcp_servers.brunn.command=\"node\""));
        assert!(rendered.contains("/srv/mcp/dist/index.js"));
        assert!(rendered.contains("BRUNN_API_TOKEN"));
        assert!(rendered.contains("--ephemeral"));
        assert!(rendered.contains("--output-last-message /tmp/run/answer.md"));
    }

    #[test]
    fn rate_limit_detection() {
        assert!(looks_rate_limited("You've hit your usage limit."));
        assert!(looks_rate_limited("HTTP 429 Too Many Requests"));
        assert!(!looks_rate_limited("All good."));
    }

    fn write_stub(script: &str) -> tempfile::TempPath {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let mut file = tempfile::NamedTempFile::new().expect("stub file");
        file.write_all(script.as_bytes()).expect("stub body");
        let path = file.into_temp_path();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("stub permissions");
        path
    }
}

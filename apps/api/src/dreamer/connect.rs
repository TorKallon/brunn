//! Connect: codex's headless device-code login, run inside the production
//! runner, with token custody in the vault.
//!
//! start() spawns `codex login` in device-code mode under an ephemeral
//! CODEX_HOME and captures the verification URL + user code from its output.
//! wait() reports progress; when the login process exits successfully the
//! tokens go straight to the vault, one live verification exec runs, and the
//! state becomes Connected. Nothing auth-shaped is ever logged or returned.

use std::{path::PathBuf, sync::Arc, time::Duration};

use serde::Serialize;
use tokio::{io::AsyncReadExt as _, process::Child, sync::Mutex};

use super::{
    codex::{self, AuthCheck},
    run::{AUTH_SECRET, Dreamer, RUNTIME_SECRET, auth_identity},
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ConnectState {
    Disconnected,
    Pending {
        url: String,
        code: String,
    },
    Verifying,
    Connected {
        #[serde(skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Failed {
        detail: String,
    },
}

pub struct ConnectFlow {
    state: Mutex<ConnectInner>,
}

struct ConnectInner {
    public: ConnectState,
    child: Option<Child>,
    codex_home: Option<PathBuf>,
}

impl ConnectFlow {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ConnectInner {
                public: ConnectState::Disconnected,
                child: None,
                codex_home: None,
            }),
        })
    }

    /// Begin a device-code login. Returns the URL + code the owner must
    /// visit. A connect already in flight is returned as-is.
    pub async fn start(&self, dreamer: &Dreamer) -> ConnectState {
        let mut inner = self.state.lock().await;
        if let ConnectState::Pending { .. } = inner.public {
            return inner.public.clone();
        }
        let connect_root = dreamer.config.work_root.join("connect");
        let _ = std::fs::remove_dir_all(&connect_root);
        let codex_home = connect_root.join(".codex");
        if let Err(error) = std::fs::create_dir_all(&codex_home) {
            inner.public = ConnectState::Failed {
                detail: format!("could not prepare the login home: {error}"),
            };
            return inner.public.clone();
        }
        let env = codex::codex_environment(&dreamer.config.host_env, &connect_root, &codex_home);
        let mut command = tokio::process::Command::new(&dreamer.config.codex_path);
        command
            .args(["login", "--device-auth"])
            .env_clear()
            .envs(&env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                inner.public = ConnectState::Failed {
                    detail: format!("could not start codex login: {error}"),
                };
                return inner.public.clone();
            }
        };
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let capture = async {
            let mut buffer = String::new();
            let mut out_chunk = [0_u8; 4_096];
            let mut err_chunk = [0_u8; 4_096];
            loop {
                let read = tokio::select! {
                    read = read_some(&mut stdout, &mut out_chunk) => {
                        read.map(|bytes| String::from_utf8_lossy(&out_chunk[..bytes]).into_owned())
                    }
                    read = read_some(&mut stderr, &mut err_chunk) => {
                        read.map(|bytes| String::from_utf8_lossy(&err_chunk[..bytes]).into_owned())
                    }
                };
                match read {
                    Some(text) if !text.is_empty() => {
                        buffer.push_str(&text);
                        if let Some(found) = extract_device_prompt(&buffer) {
                            return Some(found);
                        }
                    }
                    _ => return extract_device_prompt(&buffer),
                }
            }
        };
        match tokio::time::timeout(Duration::from_secs(60), capture).await {
            Ok(Some((url, code))) => {
                inner.child = Some(child);
                inner.codex_home = Some(codex_home);
                inner.public = ConnectState::Pending { url, code };
            }
            Ok(None) => {
                inner.public = ConnectState::Failed {
                    detail: "codex login ended without offering a device code".to_owned(),
                };
            }
            Err(_) => {
                inner.public = ConnectState::Failed {
                    detail: "codex login did not offer a device code within 60 seconds".to_owned(),
                };
            }
        }
        inner.public.clone()
    }

    /// Poll the login. On completion: tokens → vault, one live verification
    /// exec, runtime status updated, state Connected.
    pub async fn wait(&self, dreamer: &Dreamer) -> ConnectState {
        let mut inner = self.state.lock().await;
        let Some(child) = inner.child.as_mut() else {
            return inner.public.clone();
        };
        match child.try_wait() {
            Ok(None) => inner.public.clone(),
            Ok(Some(status)) if status.success() => {
                let codex_home = inner.codex_home.clone();
                inner.child = None;
                inner.public = ConnectState::Verifying;
                drop(inner);
                let outcome = self.complete(dreamer, codex_home).await;
                let mut inner = self.state.lock().await;
                inner.public = outcome;
                inner.public.clone()
            }
            Ok(Some(status)) => {
                inner.child = None;
                inner.public = ConnectState::Failed {
                    detail: format!("codex login exited with {status}"),
                };
                inner.public.clone()
            }
            Err(error) => {
                inner.child = None;
                inner.public = ConnectState::Failed {
                    detail: format!("could not observe codex login: {error}"),
                };
                inner.public.clone()
            }
        }
    }

    async fn complete(&self, dreamer: &Dreamer, codex_home: Option<PathBuf>) -> ConnectState {
        let Some(codex_home) = codex_home else {
            return ConnectState::Failed {
                detail: "the login home vanished".to_owned(),
            };
        };
        let auth_json = match std::fs::read_to_string(codex_home.join("auth.json")) {
            Ok(auth) => auth,
            Err(error) => {
                return ConnectState::Failed {
                    detail: format!("codex login finished but wrote no auth.json: {error}"),
                };
            }
        };
        // Vault first, then verification: the tokens must never exist only on
        // this container's disk.
        if let Err(error) = dreamer
            .runner
            .secret_put(AUTH_SECRET, &auth_json, "Codex auth.json for the dreamer")
            .await
        {
            return ConnectState::Failed {
                detail: format!("could not store the tokens in the vault: {error}"),
            };
        }
        let home = codex_home.parent().map(PathBuf::from).unwrap_or_default();
        let env = codex::codex_environment(&dreamer.config.host_env, &home, &codex_home);
        let verification = codex::verify_subscription(&dreamer.config.codex_path, &env).await;
        let (account, plan) = auth_identity(&auth_json);
        let _ = std::fs::remove_dir_all(home);
        match verification {
            AuthCheck::ChatGpt(identity) => {
                let now = chrono::Utc::now().to_rfc3339();
                let mut status = dreamer.runtime_status().await;
                status.account = account.clone();
                status.plan = plan.clone();
                status.connected_at = Some(now.clone());
                status.verified_at = Some(now);
                status.codex_version = Some(identity.version);
                if let Ok(raw) = serde_json::to_string(&status) {
                    let _ = dreamer
                        .runner
                        .secret_put(
                            RUNTIME_SECRET,
                            &raw,
                            "Dreamer connection and last-run status (no token material)",
                        )
                        .await;
                }
                ConnectState::Connected { account, plan }
            }
            AuthCheck::Refused { detail } => ConnectState::Failed {
                detail: format!("the login completed but verification refused it: {detail}"),
            },
        }
    }

    /// Disconnect: delete both vault records and reset.
    pub async fn disconnect(&self, dreamer: &Dreamer) -> ConnectState {
        let mut inner = self.state.lock().await;
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill().await;
        }
        let _ = dreamer.runner.secret_delete(AUTH_SECRET).await;
        let _ = dreamer.runner.secret_delete(RUNTIME_SECRET).await;
        inner.public = ConnectState::Disconnected;
        inner.public.clone()
    }

    pub async fn current(&self) -> ConnectState {
        self.state.lock().await.public.clone()
    }
}

async fn read_some(
    stream: &mut Option<impl tokio::io::AsyncRead + Unpin>,
    chunk: &mut [u8],
) -> Option<usize> {
    match stream {
        Some(reader) => reader.read(chunk).await.ok(),
        None => std::future::pending().await,
    }
}

/// Find the verification URL and user code in codex login output, leniently:
/// the first http(s) URL, and the first token that looks like a device code
/// (letters/digits with a dash, or explicitly labeled "code").
fn strip_ansi(buffer: &str) -> String {
    let mut cleaned = String::with_capacity(buffer.len());
    let mut chars = buffer.chars().peekable();
    while let Some(current) = chars.next() {
        if current != '\u{1b}' {
            cleaned.push(current);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for terminator in chars.by_ref() {
                if terminator.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    cleaned
}

fn extract_device_prompt(buffer: &str) -> Option<(String, String)> {
    // Codex colors its login output, so escape sequences glue onto tokens.
    let cleaned = strip_ansi(buffer);
    let url = cleaned
        .split_whitespace()
        .find(|token| token.starts_with("https://") || token.starts_with("http://"))
        .map(|token| token.trim_end_matches(['.', ',']).to_owned())?;
    // The code is on the labeled line or, as codex prints it, the line after.
    let mut labeled_code = None;
    let mut lines_since_label = usize::MAX;
    for line in cleaned.lines() {
        if line.to_lowercase().contains("code") {
            lines_since_label = 0;
        } else {
            lines_since_label = lines_since_label.saturating_add(1);
        }
        if lines_since_label <= 1 {
            labeled_code = line
                .split_whitespace()
                .map(|token| token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
                .filter(|token| {
                    token.len() >= 6
                        && token.contains('-')
                        && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                        && !url.contains(token)
                })
                .max_by_key(|token| token.len())
                .map(str::to_owned)
                .or(labeled_code);
        }
    }
    labeled_code.map(|code| (url, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_url_and_code_from_login_output() {
        let output = concat!(
            "To finish signing in, visit:\n",
            "  https://auth.openai.com/activate\n",
            "and enter the code: ABCD-EFGH\n",
        );
        assert_eq!(
            extract_device_prompt(output),
            Some((
                "https://auth.openai.com/activate".to_owned(),
                "ABCD-EFGH".to_owned()
            ))
        );
    }

    #[test]
    fn extracts_url_and_code_from_colored_next_line_output() {
        let output = concat!(
            "Welcome to Codex [v\u{1b}[90m0.151.0\u{1b}[0m]\n",
            "\u{1b}[90mOpenAI's command-line coding agent\u{1b}[0m\n\n",
            "Follow these steps to sign in with ChatGPT using device code authorization:\n\n",
            "1. Open this link in your browser and sign in to your account\n",
            "   \u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n",
            "2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n",
            "   \u{1b}[94mJN4E-84S1M\u{1b}[0m\n",
        );
        assert_eq!(
            extract_device_prompt(output),
            Some((
                "https://auth.openai.com/codex/device".to_owned(),
                "JN4E-84S1M".to_owned()
            ))
        );
    }

    #[test]
    fn incomplete_output_yields_nothing() {
        assert_eq!(extract_device_prompt("Working on it..."), None);
        assert_eq!(
            extract_device_prompt("visit https://auth.openai.com/activate soon"),
            None
        );
    }
}

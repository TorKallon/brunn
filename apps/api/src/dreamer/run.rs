//! The nightly run: a deterministic wrapper around one `codex exec`.
//!
//! Order is the contract: CONTROL fail-closed → vault auth → subscription
//! check → limits probe → advance check → codex exec → run-file fallback →
//! confinement cross-check → token persist-back → runtime status. Every skip
//! happens before any workspace write.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{io::AsyncWriteExt as _, process::Command};

use super::{
    client::{ApiClient, FileVersion},
    codex::{self, AuthCheck, ExecSpec},
    control::{self, ControlState, Mode},
    decisions,
    prompt::{self, PromptParams},
    runfile,
};

pub const CONTROL_PATH: &str = "dreams/CONTROL.md";
pub const DECISIONS_PATH: &str = "dreams/decisions.md";
pub const AUTH_SECRET: &str = "dreamer-codex-auth";
pub const RUNTIME_SECRET: &str = "dreamer-runtime";

#[derive(Debug, Clone)]
pub struct DreamerConfig {
    pub api_url: String,
    /// The `dreamer` credential (read_write): wrapper workspace ops and the
    /// MCP server codex talks to.
    pub workspace_token: String,
    /// The `dreamer_runner` credential: vault custody and notifications.
    pub runner_token: String,
    pub codex_path: PathBuf,
    pub codex_model: String,
    pub mcp_server_entry: PathBuf,
    /// Scratch root for ephemeral per-run homes.
    pub work_root: PathBuf,
    pub host_env: BTreeMap<String, String>,
    /// Test hook: overrides the kind's time budget for the main exec.
    pub time_budget_override: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// The scheduled or manually triggered normal run: 40 writes, 30 minutes.
    Nightly,
    /// The one supervised backfill: 300 writes, 120 minutes, owner present.
    Backfill,
}

impl RunKind {
    pub fn write_budget(self) -> usize {
        match self {
            RunKind::Nightly => 40,
            RunKind::Backfill => 300,
        }
    }

    pub fn time_budget(self) -> Duration {
        match self {
            RunKind::Nightly => Duration::from_secs(30 * 60),
            RunKind::Backfill => Duration::from_secs(120 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RunOutcome {
    Disabled { reason: String },
    SkippedAuth { detail: String },
    SkippedLimits,
    SkippedAlreadyRan,
    Completed,
    Partial { detail: String },
    Failed { detail: String },
}

impl RunOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            RunOutcome::Disabled { .. } => "disabled",
            RunOutcome::SkippedAuth { .. } => "skipped(auth)",
            RunOutcome::SkippedLimits => "skipped(limits)",
            RunOutcome::SkippedAlreadyRan => "skipped(already-ran)",
            RunOutcome::Completed => "completed",
            RunOutcome::Partial { .. } => "partial",
            RunOutcome::Failed { .. } => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub date: String,
    pub outcome: RunOutcome,
    pub mode_flipped: bool,
    /// Paths written outside the dreamer's allowed surfaces, from the
    /// post-run cross-check. Report-only.
    pub confinement_violations: Vec<String>,
    pub run_file_path: Option<String>,
}

/// `dreamer-runtime` vault record: connection identity and last-attempt
/// status for the settings card and the briefing. Never contains token
/// material.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_detail: Option<String>,
    /// Date of the most recent run that produced a run file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_date: Option<String>,
}

pub struct Dreamer {
    pub config: DreamerConfig,
    pub workspace: ApiClient,
    pub runner: ApiClient,
}

impl Dreamer {
    pub fn new(config: DreamerConfig) -> Self {
        let workspace = ApiClient::new(&config.api_url, &config.workspace_token);
        let runner = ApiClient::new(&config.api_url, &config.runner_token);
        Self {
            config,
            workspace,
            runner,
        }
    }

    pub async fn runtime_status(&self) -> RuntimeStatus {
        match self.runner.secret_get(RUNTIME_SECRET).await {
            Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
            _ => RuntimeStatus::default(),
        }
    }

    async fn store_runtime_status(&self, status: &RuntimeStatus) {
        if let Ok(raw) = serde_json::to_string(status) {
            let _ = self
                .runner
                .secret_put(
                    RUNTIME_SECRET,
                    &raw,
                    "Dreamer connection and last-run status (no token material)",
                )
                .await;
        }
    }

    async fn notify_once(&self, date: NaiveDate, event: &str, title: &str, body: &str) {
        let event_key = format!("dreaming-{}-{event}", date.format("%Y-%m-%d"));
        if let Err(error) = self.runner.notify(&event_key, title, body).await {
            tracing::warn!(%error, event_key, "dreaming notification failed");
        }
    }

    /// Execute one run. `today` is the run date in America/Los_Angeles.
    pub async fn run_once(&self, today: NaiveDate, kind: RunKind) -> RunReport {
        let mut report = RunReport {
            date: today.format("%Y-%m-%d").to_string(),
            outcome: RunOutcome::Failed {
                detail: "not started".into(),
            },
            mode_flipped: false,
            confinement_violations: Vec::new(),
            run_file_path: None,
        };
        let mut status = self.runtime_status().await;
        status.last_attempt_date = Some(report.date.clone());

        // 1. CONTROL — fail closed, zero writes when not enabled.
        let control_file = match self.workspace.read_markdown(CONTROL_PATH).await {
            Ok(file) => file,
            Err(error) => {
                report.outcome = RunOutcome::Failed {
                    detail: format!("could not read CONTROL: {error}"),
                };
                self.finish(&mut status, &report).await;
                return report;
            }
        };
        let control = match control::parse(control_file.as_ref().map(|f| f.content.as_str())) {
            ControlState::Disabled { reason } => {
                report.outcome = RunOutcome::Disabled { reason };
                self.finish(&mut status, &report).await;
                return report;
            }
            ControlState::Enabled(control) => control,
        };

        // 2. One run file per date: a nightly never replaces a report that a
        // manual or backfill run already wrote today (the overwrite clobbered
        // a backfill's proposals with a no-change stub). Manual runs may
        // deliberately rewrite today's record.
        if kind == RunKind::Nightly {
            match self
                .workspace
                .read_markdown(&runfile::run_path(today))
                .await
            {
                Ok(Some(_)) => {
                    report.outcome = RunOutcome::SkippedAlreadyRan;
                    self.finish(&mut status, &report).await;
                    return report;
                }
                Ok(None) => {}
                Err(error) => {
                    report.outcome = RunOutcome::Failed {
                        detail: format!("could not check today's run file: {error}"),
                    };
                    self.finish(&mut status, &report).await;
                    return report;
                }
            }
        }

        // 3. Owner decisions.
        let decisions_raw = match self.workspace.read_markdown(DECISIONS_PATH).await {
            Ok(file) => file.map(|f| f.content).unwrap_or_default(),
            Err(error) => {
                report.outcome = RunOutcome::Failed {
                    detail: format!("could not read decisions.md: {error}"),
                };
                self.finish(&mut status, &report).await;
                return report;
            }
        };
        let decisions = decisions::parse(&decisions_raw);

        // 3. Vault auth → ephemeral CODEX_HOME.
        let auth_json = match self.runner.secret_get(AUTH_SECRET).await {
            Ok(Some(value)) => value,
            Ok(None) => {
                report.outcome = RunOutcome::SkippedAuth {
                    detail: "no codex auth in the vault; Connect has not completed".into(),
                };
                self.notify_once(
                    today,
                    "auth",
                    "Dreaming skipped: not connected",
                    "The nightly dreaming run was skipped because no Codex account is \
                     connected. Open Settings → Dreaming to connect.",
                )
                .await;
                self.finish(&mut status, &report).await;
                return report;
            }
            Err(error) => {
                report.outcome = RunOutcome::Failed {
                    detail: format!("vault read failed: {error}"),
                };
                self.finish(&mut status, &report).await;
                return report;
            }
        };
        let run_home = match RunHome::create(&self.config.work_root, &report.date, &auth_json) {
            Ok(home) => home,
            Err(detail) => {
                report.outcome = RunOutcome::Failed { detail };
                self.finish(&mut status, &report).await;
                return report;
            }
        };
        let env =
            codex::codex_environment(&self.config.host_env, &run_home.home, &run_home.codex_home);

        // 4. Subscription check — fail closed.
        match codex::verify_subscription(&self.config.codex_path, &env).await {
            AuthCheck::ChatGpt(identity) => {
                status.codex_version = Some(identity.version);
            }
            AuthCheck::Refused { detail } => {
                report.outcome = RunOutcome::SkippedAuth {
                    detail: detail.clone(),
                };
                self.notify_once(
                    today,
                    "auth",
                    "Dreaming skipped: Codex auth failed",
                    &format!(
                        "The nightly dreaming run was skipped before any write: {detail}. \
                         Open Settings → Dreaming to reconnect."
                    ),
                )
                .await;
                self.finish(&mut status, &report).await;
                return report;
            }
        }

        // 5. Limits probe — before any write.
        match self.probe(&run_home, &env).await {
            ProbeResult::Ready => {}
            ProbeResult::RateLimited => {
                report.outcome = RunOutcome::SkippedLimits;
                self.notify_once(
                    today,
                    "limits",
                    "Dreaming skipped: plan limits",
                    "The nightly dreaming run was skipped before any write because the \
                     Codex plan is out of capacity. It will retry tomorrow night.",
                )
                .await;
                self.finish(&mut status, &report).await;
                return report;
            }
            ProbeResult::Failed(detail) => {
                report.outcome = RunOutcome::Failed {
                    detail: format!("codex probe failed: {detail}"),
                };
                self.finish(&mut status, &report).await;
                return report;
            }
        }

        // 6. Advance check.
        let mut mode = control.mode;
        if mode == Mode::ReportOnly && today >= control.advance_after && !decisions.hold_advance {
            match self
                .workspace
                .write_with_conflict_retry(CONTROL_PATH, |_| {
                    control::render(true, Mode::Full, control.advance_after)
                })
                .await
            {
                Ok(_) => {
                    mode = Mode::Full;
                    report.mode_flipped = true;
                    self.notify_once(
                        today,
                        "advance",
                        "Dreaming advanced to full mode",
                        "The seven-day report-only window ended with no hold recorded, so \
                         dreaming now applies unvetoed proposals. Say \"hold-advance\" to \
                         any agent to pause it.",
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(%error, "advance flip deferred: CONTROL write conflict");
                }
            }
        }

        // 7. Last run watermark (via the runtime record; first run has none).
        // A backfill deliberately ignores it: its whole purpose is a
        // full-corpus supervised pass, not the incremental changed-neighborhood
        // scope, and budget alone does not change what the prompt asks for.
        let last_run = match &status.last_run_date {
            _ if kind == RunKind::Backfill => None,
            Some(date_text) => {
                let path = NaiveDate::parse_from_str(date_text, "%Y-%m-%d")
                    .ok()
                    .map(runfile::run_path);
                match path {
                    Some(path) => match self.workspace.read_markdown(&path).await {
                        Ok(Some(file)) => runfile::watermark(&file.content)
                            .map(|watermark| (path.clone(), watermark)),
                        _ => None,
                    },
                    None => None,
                }
            }
            None => None,
        };

        // 8. Pre-run generation for the confinement cross-check.
        let pre_generation = match self.workspace.current_generation().await {
            Ok(generation) => generation,
            Err(error) => {
                report.outcome = RunOutcome::Failed {
                    detail: format!("could not read the workspace generation: {error}"),
                };
                self.finish(&mut status, &report).await;
                return report;
            }
        };

        // 9. The dream itself.
        let run_file_path = runfile::run_path(today);
        report.run_file_path = Some(run_file_path.clone());
        let dream_prompt = prompt::run_prompt(&PromptParams {
            today,
            mode,
            mode_flipped_tonight: report.mode_flipped,
            last_run: last_run
                .as_ref()
                .map(|(path, watermark)| (path.as_str(), *watermark)),
            decisions_raw: &decisions_raw,
            write_budget: kind.write_budget(),
            run_file_path: &run_file_path,
        });
        let time_budget = self
            .config
            .time_budget_override
            .unwrap_or_else(|| kind.time_budget());
        let exec = self
            .exec_codex(&run_home, &env, &dream_prompt, time_budget)
            .await;

        // 10. Run-file fallback: the audit trail must exist even when codex
        // died without writing it.
        let run_file = self.workspace.read_markdown(&run_file_path).await;
        let run_file = match run_file {
            Ok(file) => file,
            Err(_) => None,
        };
        if run_file.is_none() {
            let (status_label, detail) = match &exec {
                ExecResult::Finished => ("failed", "codex finished without writing a run file."),
                ExecResult::TimedOut => (
                    "partial",
                    "the 30-minute budget elapsed and the run was stopped.",
                ),
                ExecResult::Failed(_) => ("failed", "codex exited before writing a run file."),
            };
            let fallback = runfile::fallback_run_file(today, status_label, detail, pre_generation);
            let _ = self
                .workspace
                .write_markdown(&run_file_path, &fallback, Some(0), None)
                .await;
        }

        // 11. Confinement cross-check: creates confined to dreams/ + derived/;
        // updates must be enumerated in the run file. Report-only.
        let applied = run_file
            .as_ref()
            .map(|file: &FileVersion| runfile::applied_paths(&file.content))
            .unwrap_or_default();
        match self.workspace.changes_since(pre_generation, 2_000).await {
            Ok(page) => {
                for change in &page.changes {
                    let path = change.path.as_str();
                    let inside_surface =
                        path.starts_with("dreams/") || path.starts_with("derived/");
                    let enumerated = applied.iter().any(|(applied_path, _)| applied_path == path);
                    if !inside_surface && !enumerated {
                        report.confinement_violations.push(path.to_owned());
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "confinement cross-check could not list changes");
            }
        }
        if !report.confinement_violations.is_empty() {
            let listed = report.confinement_violations.join(", ");
            self.notify_once(
                today,
                "confinement",
                "Dreaming wrote outside its surfaces",
                &format!(
                    "The run recorded changes not enumerated in the run file: {listed}. \
                     Nothing was deleted; review the run file."
                ),
            )
            .await;
        }

        // 12. Persist refreshed tokens back to the vault.
        if let Some(refreshed) = run_home.read_auth() {
            if refreshed != auth_json {
                let _ = self
                    .runner
                    .secret_put(AUTH_SECRET, &refreshed, "Codex auth.json for the dreamer")
                    .await;
            }
        }

        report.outcome = match exec {
            ExecResult::Finished => {
                let is_partial = run_file
                    .as_ref()
                    .map(|file| {
                        runfile::summary_lines(&file.content)
                            .join(" ")
                            .to_lowercase()
                            .contains("partial")
                    })
                    .unwrap_or(false);
                if is_partial {
                    RunOutcome::Partial {
                        detail: "the run stopped at its write budget".into(),
                    }
                } else if run_file.is_some() {
                    RunOutcome::Completed
                } else {
                    RunOutcome::Failed {
                        detail: "codex finished without writing a run file".into(),
                    }
                }
            }
            ExecResult::TimedOut => RunOutcome::Partial {
                detail: "the time budget elapsed".into(),
            },
            ExecResult::Failed(detail) => RunOutcome::Failed { detail },
        };
        status.last_run_date = Some(report.date.clone());
        self.finish(&mut status, &report).await;
        run_home.cleanup();
        report
    }

    async fn finish(&self, status: &mut RuntimeStatus, report: &RunReport) {
        status.last_attempt_result = Some(report.outcome.label().to_owned());
        status.last_attempt_detail = match &report.outcome {
            RunOutcome::Disabled { reason } => Some(reason.clone()),
            RunOutcome::SkippedAuth { detail }
            | RunOutcome::Partial { detail }
            | RunOutcome::Failed { detail } => Some(detail.clone()),
            RunOutcome::SkippedLimits | RunOutcome::SkippedAlreadyRan | RunOutcome::Completed => {
                None
            }
        };
        self.store_runtime_status(status).await;
    }

    async fn probe(&self, run_home: &RunHome, env: &BTreeMap<String, String>) -> ProbeResult {
        match self
            .exec_codex_raw(
                run_home,
                env,
                prompt::PROBE_PROMPT,
                Duration::from_secs(180),
                "probe-answer.md",
            )
            .await
        {
            RawExec::Finished { rendered, success } => {
                if codex::looks_rate_limited(&rendered) {
                    ProbeResult::RateLimited
                } else if success {
                    ProbeResult::Ready
                } else {
                    ProbeResult::Failed(first_lines(&rendered, 3))
                }
            }
            RawExec::TimedOut => ProbeResult::Failed("probe timed out".into()),
            RawExec::SpawnFailed(detail) => ProbeResult::Failed(detail),
        }
    }

    async fn exec_codex(
        &self,
        run_home: &RunHome,
        env: &BTreeMap<String, String>,
        dream_prompt: &str,
        budget: Duration,
    ) -> ExecResult {
        match self
            .exec_codex_raw(run_home, env, dream_prompt, budget, "answer.md")
            .await
        {
            RawExec::Finished { rendered, success } => {
                if success {
                    ExecResult::Finished
                } else if codex::looks_rate_limited(&rendered) {
                    ExecResult::Failed(format!(
                        "plan limits mid-run: {}",
                        first_lines(&rendered, 2)
                    ))
                } else {
                    ExecResult::Failed(first_lines(&rendered, 3))
                }
            }
            RawExec::TimedOut => ExecResult::TimedOut,
            RawExec::SpawnFailed(detail) => ExecResult::Failed(detail),
        }
    }

    async fn exec_codex_raw(
        &self,
        run_home: &RunHome,
        env: &BTreeMap<String, String>,
        input: &str,
        budget: Duration,
        answer_name: &str,
    ) -> RawExec {
        let mut env = env.clone();
        // The MCP server codex spawns needs the workspace credential; it is
        // forwarded by name through the codex MCP config.
        env.insert("STRAYLIGHT_API_URL".into(), self.config.api_url.clone());
        env.insert(
            "STRAYLIGHT_API_TOKEN".into(),
            self.config.workspace_token.clone(),
        );
        let argv = codex::exec_command(&ExecSpec {
            codex: &self.config.codex_path,
            model: &self.config.codex_model,
            mcp_server_entry: &self.config.mcp_server_entry,
            working_dir: &run_home.work_dir,
            last_message_path: &run_home.work_dir.join(answer_name),
        });
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .env_clear()
            .envs(&env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return RawExec::SpawnFailed(format!("could not spawn codex: {error}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes()).await;
            drop(stdin);
        }
        match tokio::time::timeout(budget, child.wait_with_output()).await {
            Ok(Ok(output)) => RawExec::Finished {
                success: output.status.success(),
                rendered: format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            },
            Ok(Err(error)) => RawExec::SpawnFailed(format!("codex did not finish: {error}")),
            Err(_) => RawExec::TimedOut,
        }
    }
}

enum ProbeResult {
    Ready,
    RateLimited,
    Failed(String),
}

enum RawExec {
    Finished { success: bool, rendered: String },
    TimedOut,
    SpawnFailed(String),
}

enum ExecResult {
    Finished,
    TimedOut,
    Failed(String),
}

/// The ephemeral per-run home: auth.json lives here for the duration of the
/// run and nowhere else on disk.
struct RunHome {
    home: PathBuf,
    codex_home: PathBuf,
    work_dir: PathBuf,
}

impl RunHome {
    fn create(work_root: &std::path::Path, date: &str, auth_json: &str) -> Result<Self, String> {
        use std::os::unix::fs::PermissionsExt as _;
        let root = work_root.join(format!("run-{date}"));
        let home = root.join(".home");
        let codex_home = home.join(".codex");
        let work_dir = root.join("work");
        for dir in [&root, &home, &codex_home, &work_dir] {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("could not chmod {}: {error}", dir.display()))?;
        }
        let auth_path = codex_home.join("auth.json");
        std::fs::write(&auth_path, auth_json)
            .map_err(|error| format!("could not write auth.json: {error}"))?;
        std::fs::set_permissions(&auth_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not chmod auth.json: {error}"))?;
        Ok(Self {
            home,
            codex_home,
            work_dir,
        })
    }

    fn read_auth(&self) -> Option<String> {
        std::fs::read_to_string(self.codex_home.join("auth.json")).ok()
    }

    fn cleanup(&self) {
        if let Some(root) = self.home.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn first_lines(rendered: &str, count: usize) -> String {
    rendered
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(count)
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Extract account and plan from a codex auth.json, best effort, for the
/// settings card. Never logs or returns token material.
pub fn auth_identity(auth_json: &str) -> (Option<String>, Option<String>) {
    let parsed: Value = match serde_json::from_str(auth_json) {
        Ok(value) => value,
        Err(_) => return (None, None),
    };
    let account = [
        "/tokens/account_id",
        "/account_id",
        "/tokens/id_token/email",
        "/email",
    ]
    .iter()
    .find_map(|pointer| parsed.pointer(pointer))
    .and_then(Value::as_str)
    .map(str::to_owned);
    let plan = [
        "/tokens/id_token/chatgpt_plan_type",
        "/plan",
        "/chatgpt_plan_type",
    ]
    .iter()
    .find_map(|pointer| parsed.pointer(pointer))
    .and_then(Value::as_str)
    .map(str::to_owned);
    (account, plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_kind_budgets_are_locked() {
        assert_eq!(RunKind::Nightly.write_budget(), 40);
        assert_eq!(RunKind::Nightly.time_budget(), Duration::from_secs(1_800));
        assert_eq!(RunKind::Backfill.write_budget(), 300);
        assert_eq!(RunKind::Backfill.time_budget(), Duration::from_secs(7_200));
    }

    #[test]
    fn outcome_labels() {
        assert_eq!(
            RunOutcome::SkippedAuth { detail: "x".into() }.label(),
            "skipped(auth)"
        );
        assert_eq!(RunOutcome::SkippedLimits.label(), "skipped(limits)");
    }

    #[test]
    fn auth_identity_extracts_without_leaking() {
        let (account, plan) = auth_identity(
            r#"{"tokens":{"account_id":"acct_1","access_token":"secret"},"plan":"pro"}"#,
        );
        assert_eq!(account.as_deref(), Some("acct_1"));
        assert_eq!(plan.as_deref(), Some("pro"));
        let (none_account, none_plan) = auth_identity("not json");
        assert!(none_account.is_none() && none_plan.is_none());
    }

    #[test]
    fn runtime_status_never_serializes_token_fields() {
        let status = RuntimeStatus {
            account: Some("acct".into()),
            ..RuntimeStatus::default()
        };
        let raw = serde_json::to_string(&status).expect("serialize");
        assert!(!raw.contains("token"));
        assert!(!raw.contains("auth"));
    }
}

//! The nightly run: read-only drafting and a bounded location evidence audit.
//!
//! CONTROL fail-closed → server admission/frozen intake → read-only reasoning
//! → checked auth custody → server-validated terminal run and v2 projection.
//! CONTROL-off performs no workspace writes; enabled skips have audit receipts.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt as _, process::Command};

use super::{
    client::{ApiClient, ClientError, SecretVersion},
    codex::{self, AuthCheck, ExecSpec},
    control::{self, ControlState},
    prompt, receipt, runfile,
};

pub const CONTROL_PATH: &str = "dreams/CONTROL.md";
pub const DECISIONS_PATH: &str = "dreams/decisions.md";
pub const AUTH_SECRET: &str = "dreamer-codex-auth";
pub const RUNTIME_SECRET: &str = "dreamer-runtime";

#[derive(Debug, Clone)]
pub struct DreamerConfig {
    pub api_url: String,
    /// Existing wrapper workspace read credential, never handed to the model.
    pub workspace_token: String,
    /// Dedicated read-only model identity; it must differ from wrapper credentials.
    pub model_token: String,
    /// The `dreamer_runner` credential: vault custody and notifications.
    pub runner_token: String,
    pub codex_path: PathBuf,
    pub codex_model: String,
    pub mcp_server_entry: PathBuf,
    /// Scratch root for ephemeral per-run homes.
    pub work_root: PathBuf,
    pub host_env: BTreeMap<String, String>,
    /// Test hook: overrides the shared draft/audit budget, excluding the probe.
    pub time_budget_override: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// The scheduled or manually triggered normal run: 40 writes, 30 minutes.
    Nightly,
    /// Explicit same-day retry, independent of the scheduled-slot dedupe.
    Manual,
    /// The one supervised backfill: 300 writes, 120 minutes, owner present.
    Backfill,
}

impl RunKind {
    pub fn write_budget(self) -> usize {
        match self {
            RunKind::Nightly | RunKind::Manual => 40,
            RunKind::Backfill => 300,
        }
    }

    pub fn time_budget(self) -> Duration {
        match self {
            RunKind::Nightly | RunKind::Manual => Duration::from_secs(30 * 60),
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
    pub attempt_id: String,
    pub stage: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub run_entry_ref: Option<String>,
    pub run_version: Option<i64>,
    pub auth_persistence: String,
    pub receipt_persistence: String,
    pub notification: Value,
    pub persistence_error: Option<String>,
    pub counts: Value,
}

/// `dreamer-runtime` vault record: connection identity and last-attempt
/// status for the settings card and the briefing. Never contains token
/// material.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeStatus {
    #[serde(skip)]
    pub custody_version: Option<i64>,
    #[serde(skip)]
    pub custody_ref: Option<String>,
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
    /// Date of the most recent completed attempt with an accepted exact receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_terminal: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_error: Option<String>,
}

pub struct Dreamer {
    pub config: DreamerConfig,
    pub workspace: ApiClient,
    pub runner: ApiClient,
    pub(crate) auth_lock: tokio::sync::Mutex<()>,
}

impl Dreamer {
    pub fn new(config: DreamerConfig) -> Self {
        let workspace = ApiClient::new(&config.api_url, &config.workspace_token);
        let runner = ApiClient::new(&config.api_url, &config.runner_token);
        Self {
            config,
            workspace,
            runner,
            auth_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn runtime_status(&self) -> RuntimeStatus {
        match self.runner.secret_get_version(RUNTIME_SECRET).await {
            Ok(Some(secret)) => {
                let mut status = serde_json::from_str::<RuntimeStatus>(&secret.value)
                    .unwrap_or_else(|_| RuntimeStatus {
                        persistence_error: Some("runtime vault record is malformed".into()),
                        ..RuntimeStatus::default()
                    });
                status.custody_version = Some(secret.version);
                status.custody_ref = Some(secret.secret_ref);
                status
            }
            Err(_) => RuntimeStatus {
                persistence_error: Some("runtime vault read unavailable".into()),
                ..RuntimeStatus::default()
            },
            Ok(None) => RuntimeStatus::default(),
        }
    }

    pub(crate) async fn store_runtime_status(&self, status: &RuntimeStatus) -> Result<(), String> {
        let raw = serde_json::to_string(status).map_err(|_| "runtime serialization failed")?;
        let version = self
            .runner
            .secret_put_checked(
                RUNTIME_SECRET,
                &raw,
                status.custody_version.unwrap_or(0),
                status.custody_ref.as_deref(),
            )
            .await
            .map_err(|_| "runtime custody CAS failed")?;
        let saved = self
            .runner
            .secret_get_version(RUNTIME_SECRET)
            .await
            .map_err(|_| "runtime custody read-back failed")?
            .ok_or("runtime record disappeared")?;
        if saved.version != version || saved.value != raw {
            return Err("runtime custody read-back mismatch".into());
        }
        Ok(())
    }

    /// All paths after an enabled admission reach one terminal persistence path.
    /// A process killed before it does is recovered by the server lease/fence.
    pub async fn run_once(&self, today: NaiveDate, kind: RunKind) -> RunReport {
        let _auth_guard = self.auth_lock.lock().await;
        let mut report = RunReport {
            date: today.format("%Y-%m-%d").to_string(),
            outcome: RunOutcome::Failed {
                detail: "not started".into(),
            },
            mode_flipped: false,
            confinement_violations: vec![],
            run_file_path: None,
            attempt_id: uuid::Uuid::now_v7().to_string(),
            stage: "control".into(),
            started_at: Utc::now().to_rfc3339(),
            completed_at: None,
            run_entry_ref: None,
            run_version: None,
            auth_persistence: "not_used".into(),
            receipt_persistence: "not_attempted".into(),
            notification: json!({"status":"not_needed"}),
            persistence_error: None,
            counts: json!({}),
        };
        let mut runtime = self.runtime_status().await;
        let control_file = match self.workspace.read_markdown(CONTROL_PATH).await {
            Ok(file) => file,
            Err(error) => {
                report.outcome = RunOutcome::Failed {
                    detail: format!("could not read CONTROL: {error}"),
                };
                self.finish_runtime(&mut runtime, &mut report).await;
                return report;
            }
        };
        if let ControlState::Disabled { reason } =
            control::parse(control_file.as_ref().map(|file| file.content.as_str()))
        {
            report.outcome = RunOutcome::Disabled { reason };
            self.finish_runtime(&mut runtime, &mut report).await;
            return report;
        }
        // Retry an ambiguous terminal commit with its original identity before
        // starting more model work. No candidate or model output is recreated.
        if let Some(request) = runtime.pending_terminal.clone() {
            match self.runner.dreamer("finish", request).await {
                Ok(value) if receipt::validate(&value["latest_receipt"]).is_ok() => {
                    runtime.pending_terminal = None;
                    if value["latest_receipt"]["status"] == "completed" {
                        runtime.last_run_date = value["latest_receipt"]["run_id"]
                            .as_str()
                            .map(str::to_owned);
                    }
                }
                Err(ClientError::Conflict { .. }) => {
                    // A newer server fence or expired lease owns recovery now.
                    // Re-admission retains the old inputs and candidates.
                    runtime.pending_terminal = None;
                }
                _ => {
                    report.outcome = RunOutcome::Failed { detail: "pending terminal receipt could not be recovered; no new model work started".into() };
                    self.finish_runtime(&mut runtime, &mut report).await;
                    return report;
                }
            }
        }
        // A daily file is evidence, not an admission decision. The server owns
        // scheduled-slot dedupe, same-day attempt identity and the frozen input.
        report.stage = "admission".into();
        let admission = match self.runner.dreamer("admit", json!({
            "attempt_id":report.attempt_id, "date":report.date,
            "kind":match kind { RunKind::Nightly=>"nightly", RunKind::Manual=>"manual", RunKind::Backfill=>"backfill" },
            "lease_seconds": kind.time_budget().as_secs()+600
        })).await {
            Ok(value) if value["admitted"] == true => value,
            Ok(value) => {
                report.outcome = if value["reason"].as_str().is_some_and(|s| s.contains("completed")) {
                    RunOutcome::SkippedAlreadyRan
                } else { RunOutcome::Failed { detail: value["reason"].as_str().unwrap_or("attempt not admitted").into() } };
                self.finish_runtime(&mut runtime, &mut report).await; return report;
            }
            Err(error) => {
                report.outcome = RunOutcome::Failed { detail: format!("attempt admission failed: {error}") };
                self.finish_runtime(&mut runtime, &mut report).await; return report;
            }
        };
        let mut state_version = admission["state_version"].as_i64().unwrap_or(0);
        let fence = admission["fence"].clone();
        report.run_file_path = Some(runfile::run_path(today));
        let result = self
            .execute_admitted(
                &admission,
                &mut state_version,
                &mut runtime,
                &mut report,
                kind,
            )
            .await;
        report.outcome = result;
        if matches!(
            report.outcome,
            RunOutcome::SkippedAuth { .. } | RunOutcome::SkippedLimits
        ) {
            let event = if matches!(report.outcome, RunOutcome::SkippedLimits) {
                "limits"
            } else {
                "auth"
            };
            let key = format!("dreaming-{}-{event}", report.date);
            let title = if event == "limits" {
                "Dreaming skipped: plan limits"
            } else {
                "Dreaming skipped: account verification"
            };
            let body = "The enabled attempt was recorded and pending work retained. Open Settings → Dreaming to inspect the account and run status.";
            let retries = report
                .notification
                .get("retry_results")
                .cloned()
                .unwrap_or(json!([]));
            report.notification = match self.runner.notify(&key, title, body).await {
                Ok(ack) => {
                    json!({"status":"accepted","event_key":key,"target_kind":"operational","title":title,"body":body,"ack":ack,"retry_results":retries})
                }
                Err(_) => {
                    json!({"status":"failed","event_key":key,"target_kind":"operational","title":title,"body":body,"retry_results":retries})
                }
            };
        }
        report.completed_at = Some(Utc::now().to_rfc3339());
        report.stage = "terminal_commit".into();
        let terminal_request = json!({
            "attempt_id": report.attempt_id,"fence":fence,"expected_state_version":state_version,
            "outcome":match &report.outcome { RunOutcome::Completed=>"completed",RunOutcome::Partial{..}=>"partial",RunOutcome::SkippedAuth{..}|RunOutcome::SkippedLimits|RunOutcome::SkippedAlreadyRan=>"skipped",_=>"failed" },
            "detail": outcome_detail(&report.outcome), "execution_outcome":report.outcome,
            "auth_persistence":report.auth_persistence,"notification":report.notification,
            "completed_at":report.completed_at,"model":self.config.codex_model,
            "codex_version":runtime.codex_version
        });
        let terminal = self
            .runner
            .dreamer("finish", terminal_request.clone())
            .await;
        match terminal {
            Ok(value) => {
                runtime.pending_terminal = None;
                report.run_entry_ref = value["run_entry_ref"].as_str().map(str::to_owned);
                report.run_version = value["run_version"].as_i64();
                report.counts = value.get("counts").cloned().unwrap_or(json!({}));
                let latest = &value["latest_receipt"];
                match receipt::validate(latest) {
                    Ok(())
                        if latest["receipt_ref"] == value["run_entry_ref"]
                            && latest["receipt_version"] == value["run_version"] =>
                    {
                        report.receipt_persistence = "accepted".into();
                        if report.outcome == RunOutcome::Completed && latest["status"] == "partial"
                        {
                            report.outcome = RunOutcome::Partial {
                                detail:
                                    "server retained unfinished admitted work for a later attempt"
                                        .into(),
                            };
                        }
                        if report.outcome == RunOutcome::Completed
                            && latest["status"] == "completed"
                        {
                            runtime.last_run_date = Some(report.date.clone());
                        }
                    }
                    _ => {
                        report.receipt_persistence = "failed".into();
                        report.persistence_error = Some(
                            "terminal response has no valid receipt bound to its exact run version"
                                .into(),
                        );
                    }
                }
            }
            Err(error) => {
                runtime.pending_terminal = Some(terminal_request);
                report.receipt_persistence = "failed".into();
                report.persistence_error = Some(format!("terminal persistence failed: {error}"));
            }
        }
        report.stage = "finished".into();
        self.finish_runtime(&mut runtime, &mut report).await;
        report
    }

    async fn execute_admitted(
        &self,
        admission: &Value,
        state_version: &mut i64,
        runtime: &mut RuntimeStatus,
        report: &mut RunReport,
        kind: RunKind,
    ) -> RunOutcome {
        let mut retry_results = vec![];
        if let Some(retries) = admission["pending_notifications"].as_array() {
            for retry in retries.iter().take(8) {
                let Some(key) = retry["event_key"].as_str() else {
                    continue;
                };
                let outcome = if retry["target_kind"] == "operational" {
                    self.runner
                        .notify(
                            key,
                            retry["title"]
                                .as_str()
                                .unwrap_or("Dreaming operational update"),
                            retry["body"]
                                .as_str()
                                .unwrap_or("Open Dreaming status for details."),
                        )
                        .await
                } else if let (Some(run_ref), Some(version), Some(count)) = (
                    retry["run_entry_ref"].as_str(),
                    retry["run_version"].as_i64(),
                    retry["count"].as_u64(),
                ) {
                    self.runner
                        .review_ready(key, run_ref, version, count as usize)
                        .await
                } else {
                    continue;
                };
                let mut result = retry.clone();
                if let Ok(ack) = outcome {
                    result["status"] = json!("accepted");
                    result["ack"] = ack;
                }
                retry_results.push(result);
            }
        }
        report.notification["retry_results"] = json!(retry_results);
        report.stage = "auth".into();
        let original = match self.runner.secret_get_version(AUTH_SECRET).await {
            Ok(Some(auth)) => auth,
            Ok(None) => {
                return RunOutcome::SkippedAuth {
                    detail: "no Codex account connected".into(),
                };
            }
            Err(_) => {
                return RunOutcome::Failed {
                    detail: "vault auth read failed".into(),
                };
            }
        };
        let run_home =
            match RunHome::create(&self.config.work_root, &report.attempt_id, &original.value) {
                Ok(home) => home,
                Err(detail) => return RunOutcome::Failed { detail },
            };
        let env =
            codex::codex_environment(&self.config.host_env, &run_home.home, &run_home.codex_home);
        let outcome = self
            .reason_and_submit(
                admission,
                state_version,
                runtime,
                report,
                kind,
                &run_home,
                &env,
            )
            .await;
        // One checked finalizer after subscription, probe and execution, even
        // on timeout/failure. Raw output and tokens never enter the report.
        report.auth_persistence = match self.finalize_auth(&run_home.codex_home, &original).await {
            Ok(()) => "verified".into(),
            Err(_) => "failed".into(),
        };
        if report.auth_persistence == "failed" {
            return RunOutcome::Failed {
                detail: format!(
                    "auth custody failed after {}; refreshed credentials were not verified in the vault",
                    outcome.label()
                ),
            };
        }
        outcome
    }

    async fn reason_and_submit(
        &self,
        admission: &Value,
        state_version: &mut i64,
        runtime: &mut RuntimeStatus,
        report: &mut RunReport,
        kind: RunKind,
        run_home: &RunHome,
        env: &BTreeMap<String, String>,
    ) -> RunOutcome {
        match codex::verify_subscription(&self.config.codex_path, env).await {
            AuthCheck::ChatGpt(identity) => {
                if self
                    .config
                    .host_env
                    .get("DREAMER_CODEX_VERSION")
                    .is_some_and(|v| v != &identity.version)
                {
                    return RunOutcome::SkippedAuth {
                        detail: "Codex build differs from the qualified production pin".into(),
                    };
                }
                runtime.codex_version = Some(identity.version);
            }
            AuthCheck::Refused { .. } => {
                return RunOutcome::SkippedAuth {
                    detail: "Codex does not have a verified ChatGPT-plan login".into(),
                };
            }
        }
        if let Err(detail) = self.verify_model_identity().await {
            return RunOutcome::Failed { detail };
        }
        report.stage = "probe".into();
        match self.probe(run_home, env).await {
            ProbeResult::Ready => {}
            ProbeResult::RateLimited => return RunOutcome::SkippedLimits,
            ProbeResult::Failed(detail) => {
                return RunOutcome::Failed {
                    detail: if detail.contains("timed out") {
                        "Codex capacity probe timed out"
                    } else {
                        "Codex capacity probe failed"
                    }
                    .into(),
                };
            }
        }
        let checkpoint = self.runner.dreamer("checkpoint", json!({"attempt_id":report.attempt_id,"fence":admission["fence"],"expected_state_version":state_version,"lease_seconds":kind.time_budget().as_secs()+600})).await;
        match checkpoint {
            Ok(value) => *state_version = value["state_version"].as_i64().unwrap_or(*state_version),
            Err(_) => {
                return RunOutcome::Failed {
                    detail: "attempt lease renewal rejected".into(),
                };
            }
        }
        let budget = self
            .config
            .time_budget_override
            .unwrap_or_else(|| kind.time_budget());
        let finalizer_reserve = Duration::from_secs(15).min(budget / 10);
        let usable = budget.saturating_sub(finalizer_reserve);
        let total_deadline = tokio::time::Instant::now() + usable;
        let narrative_allowance = if admission["location_work"].is_object()
            && admission["inputs"]
                .as_array()
                .is_some_and(|inputs| !inputs.is_empty())
        {
            Duration::from_secs(600).min(usable / 3)
        } else {
            Duration::ZERO
        };
        let reasoning_deadline = total_deadline - narrative_allowance;
        let mut enriched = admission.clone();
        if admission["location_work"].is_object() {
            report.stage = "location_discovery".into();
            let discovery_budget = Duration::from_secs(600).min(usable / 2);
            match tokio::time::timeout(
                discovery_budget,
                self.discover_location(admission, state_version, run_home, env, discovery_budget),
            )
            .await
            {
                Ok(Ok(value)) => enriched = value,
                Ok(Err(detail)) => return RunOutcome::Partial { detail },
                Err(_) => {
                    return RunOutcome::Partial {
                        detail: "location discovery timed out; day retained for retry".into(),
                    };
                }
            }
        }
        let admission = &enriched;
        report.stage = "reasoning".into();
        let input = prompt::candidate_prompt(
            &report.attempt_id,
            admission,
            kind.write_budget().saturating_sub(8).min(16),
        );
        let remaining = reasoning_deadline.saturating_duration_since(tokio::time::Instant::now());
        let audit_allowance = if admission["location_work"].is_object() {
            Duration::from_secs(360).min(remaining / 3)
        } else {
            Duration::ZERO
        };
        let draft_budget = remaining.saturating_sub(audit_allowance);
        let draft_deadline = reasoning_deadline - audit_allowance;
        let answer_name = if admission["location_work"].is_object() {
            "location-answer.md"
        } else {
            "answer.md"
        };
        let draft_result = tokio::time::timeout_at(
            draft_deadline,
            self.exec_codex(run_home, env, &input, draft_budget, answer_name),
        )
        .await
        .unwrap_or(ExecResult::TimedOut);
        match draft_result {
            ExecResult::TimedOut => {
                return RunOutcome::Partial {
                    detail: "model time budget elapsed; admitted inputs remain pending".into(),
                };
            }
            ExecResult::Failed(detail) => {
                return RunOutcome::Failed {
                    detail: if detail.starts_with("plan limits mid-run") {
                        "model plan capacity exhausted mid-run; admitted inputs remain pending"
                    } else if detail.starts_with("could not spawn") {
                        "model process could not start; admitted inputs remain pending"
                    } else {
                        "model execution failed; admitted inputs remain pending"
                    }
                    .into(),
                };
            }
            ExecResult::Finished => {}
        }
        let raw = match std::fs::read_to_string(run_home.work_dir.join(answer_name)) {
            Ok(raw) if raw.len() <= 1024 * 1024 => raw,
            _ => {
                return RunOutcome::Failed {
                    detail: "model produced no bounded candidate file".into(),
                };
            }
        };
        let mut output = match prompt::parse_candidate_output(&raw, admission) {
            Ok(output) => output,
            Err(detail) => return RunOutcome::Failed { detail },
        };
        if admission["location_work"].is_object()
            && (output["processed_inputs"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
                || output["candidates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|c| {
                        !c["path"]
                            .as_str()
                            .is_some_and(|p| p.starts_with("derived/location/"))
                    }))
        {
            return RunOutcome::Failed {
                detail: "location draft crossed its isolated evidence boundary; work retained"
                    .into(),
            };
        }
        if prompt::has_location_candidate(&output) {
            report.stage = "location_audit".into();
            let audit_prompt =
                prompt::location_audit_prompt(&report.attempt_id, admission, &output);
            let audit_budget = audit_allowance
                .min(reasoning_deadline.saturating_duration_since(tokio::time::Instant::now()));
            if audit_budget.is_zero() {
                return RunOutcome::Partial {
                    detail: "location audit budget exhausted; unchecked draft not submitted and admitted work retained".into(),
                };
            }
            let audit_deadline = reasoning_deadline.min(tokio::time::Instant::now() + audit_budget);
            let audit_result = tokio::time::timeout_at(
                audit_deadline,
                self.exec_codex(
                    run_home,
                    env,
                    &audit_prompt,
                    audit_budget,
                    "location-audit-answer.md",
                ),
            )
            .await
            .unwrap_or(ExecResult::TimedOut);
            match audit_result {
                ExecResult::Finished => {}
                ExecResult::TimedOut => return RunOutcome::Partial {
                    detail: "location audit timed out; unchecked draft not submitted and admitted work retained".into(),
                },
                ExecResult::Failed(_) => return RunOutcome::Failed {
                    detail: "location audit failed; unchecked draft not submitted and admitted work retained".into(),
                },
            }
            let audited = match std::fs::read_to_string(
                run_home.work_dir.join("location-audit-answer.md"),
            ) {
                Ok(raw) if raw.len() <= 1024 * 1024 => raw,
                _ => return RunOutcome::Failed {
                    detail:
                        "location audit produced no bounded output; unchecked draft not submitted"
                            .into(),
                },
            };
            output = match prompt::parse_location_audit_output(&audited, admission, &output) {
                Ok(output) => output,
                Err(detail) => {
                    return RunOutcome::Failed {
                        detail: format!(
                            "location audit output rejected: {detail}; unchecked draft not submitted"
                        ),
                    };
                }
            };
            let issues = prompt::location_content_issues(&output, admission);
            if !issues.is_empty() {
                report.stage = "location_correction".into();
                let correction_prompt = prompt::location_correction_prompt(
                    &report.attempt_id,
                    admission,
                    &output,
                    &issues,
                );
                // The correction shares the first audit's absolute deadline.
                // Prompt construction and process setup cannot reset it.
                let remaining =
                    audit_deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return RunOutcome::Partial {
                        detail: "location correction budget exhausted; unchecked output not submitted and admitted work retained".into(),
                    };
                }
                let result = tokio::time::timeout_at(
                    audit_deadline,
                    self.exec_codex(
                        run_home,
                        env,
                        &correction_prompt,
                        remaining,
                        "location-correction-answer.md",
                    ),
                )
                .await
                .unwrap_or(ExecResult::TimedOut);
                match result {
                    ExecResult::Finished => {},
                    ExecResult::TimedOut => return RunOutcome::Partial {
                        detail: "location correction timed out; unchecked output not submitted and admitted work retained".into(),
                    },
                    ExecResult::Failed(_) => return RunOutcome::Failed {
                        detail: "location correction failed; unchecked output not submitted and admitted work retained".into(),
                    },
                }
                let corrected = match std::fs::read_to_string(run_home.work_dir.join("location-correction-answer.md")) {
                    Ok(raw) if raw.len() <= 1024 * 1024 => raw,
                    _ => return RunOutcome::Failed {
                        detail: "location correction produced no bounded output; unchecked output not submitted".into(),
                    },
                };
                output = match prompt::parse_location_audit_output(&corrected, admission, &output) {
                    Ok(output) => output,
                    Err(detail) => {
                        return RunOutcome::Failed {
                            detail: format!(
                                "location correction output rejected: {detail}; unchecked output not submitted"
                            ),
                        };
                    }
                };
                if !prompt::location_content_issues(&output, admission).is_empty() {
                    return RunOutcome::Failed {
                        detail: "location content validation failed after one correction; unchecked output not submitted and admitted work retained".into(),
                    };
                }
            }
            output = match prompt::compile_location_evidence_inventory(&output, admission) {
                Ok(output) => output,
                Err(detail) => {
                    return RunOutcome::Failed {
                        detail: format!(
                            "location evidence inventory validation failed: {detail}; unchecked output not submitted and admitted work retained"
                        ),
                    };
                }
            };
        }
        report.stage = "candidate_validation".into();
        let candidates = self.runner.dreamer("candidates", json!({
            "attempt_id":report.attempt_id,"fence":admission["fence"],"expected_state_version":state_version,
            "candidates":output["candidates"],"processed_inputs":output["processed_inputs"],"findings":output["findings"]
        })).await;
        let mut value = match candidates {
            Ok(value) => value,
            Err(_) => {
                return RunOutcome::Failed {
                    detail: "candidate validation/publication rejected; admitted work retained"
                        .into(),
                };
            }
        };
        *state_version = value["state_version"].as_i64().unwrap_or(*state_version);
        let mut accepted = value["accepted_candidate_ids"]
            .as_array()
            .map_or(0, Vec::len);
        let mut processed = output["processed_inputs"].as_array().map_or(0, Vec::len);
        let mut partial = None;
        if !narrative_allowance.is_zero() {
            report.stage = "narrative_reasoning".into();
            // The accepted location artifact survives any narrative failure.
            // This pass has no location packet, candidate body or authority.
            let narrative = prompt::narrative_admission(admission);
            let remaining = total_deadline.saturating_duration_since(tokio::time::Instant::now());
            let input = prompt::candidate_prompt(&report.attempt_id, &narrative, 15);
            let result = tokio::time::timeout_at(
                total_deadline,
                self.exec_codex(run_home, env, &input, remaining, "narrative-answer.md"),
            )
            .await;
            let parsed = match result {
                Ok(ExecResult::Finished) => {
                    std::fs::read_to_string(run_home.work_dir.join("narrative-answer.md"))
                        .ok()
                        .filter(|raw| raw.len() <= 1024 * 1024)
                        .and_then(|raw| prompt::parse_candidate_output(&raw, &narrative).ok())
                        .filter(|output| !prompt::has_location_candidate(output))
                }
                _ => None,
            };
            if let Some(narrative_output) = parsed {
                report.stage = "narrative_validation".into();
                match self.runner.dreamer("candidates",json!({
                    "attempt_id":report.attempt_id,"fence":admission["fence"],"expected_state_version":state_version,
                    "candidates":narrative_output["candidates"],"processed_inputs":narrative_output["processed_inputs"],"findings":narrative_output["findings"]
                })).await {
                    Ok(receipt) => {
                        *state_version=receipt["state_version"].as_i64().unwrap_or(*state_version);
                        accepted+=receipt["accepted_candidate_ids"].as_array().map_or(0,Vec::len);
                        processed+=narrative_output["processed_inputs"].as_array().map_or(0,Vec::len);
                        value=receipt;
                    },
                    Err(_) => partial=Some("checked location work retained; narrative submission failed and its inputs remain pending"),
                }
            } else {
                partial = Some(
                    "checked location work retained; narrative pass did not finish valid output and its inputs remain pending",
                );
            }
        }
        if accepted > 0 {
            if let (Some(run_ref), Some(version)) = (
                value["run_entry_ref"].as_str(),
                value["run_version"].as_i64(),
            ) {
                let event_key = format!("dreaming-review-{}", report.attempt_id);
                let retry_results = report
                    .notification
                    .get("retry_results")
                    .cloned()
                    .unwrap_or(json!([]));
                report.notification = match self
                    .runner
                    .review_ready(&event_key, run_ref, version, accepted)
                    .await
                {
                    Ok(ack) => {
                        json!({"status":"accepted","event_key":event_key,"run_entry_ref":run_ref,"run_version":version,"count":accepted,"ack":ack})
                    }
                    Err(_) => {
                        json!({"status":"failed","event_key":event_key,"run_entry_ref":run_ref,"run_version":version,"count":accepted,"detail":"review-ready notification publication failed; pending work retained"})
                    }
                };
                report.notification["retry_results"] = retry_results;
                report.notification["target_kind"] = json!("review");
            }
        }
        if let Some(detail) = partial {
            return RunOutcome::Partial {
                detail: detail.into(),
            };
        }
        if processed < admission["inputs"].as_array().map_or(0, Vec::len) {
            RunOutcome::Partial {
                detail: "bounded model work completed; unprocessed admitted inputs remain pending"
                    .into(),
            }
        } else if admission["location_work"].is_object() && !prompt::has_location_candidate(&output)
        {
            RunOutcome::Partial { detail:"bounded reasoning completed; historical day remains pending without a supported location candidate".into() }
        } else {
            RunOutcome::Completed
        }
    }

    async fn discover_location(
        &self,
        admission: &Value,
        state_version: &mut i64,
        run_home: &RunHome,
        env: &BTreeMap<String, String>,
        budget: Duration,
    ) -> Result<Value, String> {
        let input = super::discovery::prompt(admission);
        match self
            .exec_codex(
                run_home,
                env,
                &input,
                budget,
                "location-discovery-answer.md",
            )
            .await
        {
            ExecResult::Finished => {}
            ExecResult::TimedOut => return Err("location discovery timed out; day retained".into()),
            ExecResult::Failed(_) => {
                return Err("location discovery model failed; day retained".into());
            }
        }
        let raw = std::fs::read_to_string(run_home.work_dir.join("location-discovery-answer.md"))
            .map_err(|_| "location discovery output missing")?;
        if raw.len() > 64 * 1024 {
            return Err("location discovery output exceeded bound".into());
        }
        let discovered = super::discovery::parse(&raw)?;
        let (verified, failures) = super::discovery::verify_lookups(&discovered.lookups).await;
        let mut next=self.runner.dreamer("location-discover",json!({
            "attempt_id":admission["attempt_id"],"fence":admission["fence"],
            "expected_state_version":state_version,"context_queries":discovered.context_queries,
            "web_sources":verified
        })).await.map_err(|_|"location discovery source admission failed; day retained")?;
        *state_version = next["state_version"]
            .as_i64()
            .ok_or("discovery state receipt missing")?;
        let mut findings = discovered.findings;
        findings.extend(failures);
        next["location_discovery_findings"] = json!(findings);
        Ok(next)
    }

    async fn verify_model_identity(&self) -> Result<(), String> {
        if self.config.model_token.is_empty()
            || self.config.model_token == self.config.workspace_token
            || self.config.model_token == self.config.runner_token
        {
            return Err("a dedicated read-only model credential is required".into());
        }
        let value = ApiClient::new(&self.config.api_url, &self.config.model_token)
            .get("/v1/me")
            .await
            .map_err(|_| "model credential inspection failed")?;
        let identity = value.get("data").unwrap_or(&value);
        let caps = identity["capabilities"]
            .as_array()
            .ok_or("model capabilities missing")?;
        let allowed = [
            "read",
            "open",
            "query",
            "status",
            "changes",
            "asset:read",
            "message:receive",
            "receive",
            "list",
            "compute",
            "verify",
            "task.read",
            "message.read",
        ];
        if identity["read_only"] != true
            || !caps.iter().any(|v| v == "read")
            || caps
                .iter()
                .any(|v| !v.as_str().is_some_and(|cap| allowed.contains(&cap)))
        {
            return Err("model credential is not strictly read-only".into());
        }
        Ok(())
    }

    pub(crate) async fn finalize_auth(
        &self,
        codex_home: &std::path::Path,
        original: &SecretVersion,
    ) -> Result<(), String> {
        let refreshed = std::fs::read_to_string(codex_home.join("auth.json"))
            .map_err(|_| "auth file unavailable after execution")?;
        serde_json::from_str::<Value>(&refreshed).map_err(|_| "refreshed auth is malformed")?;
        let expected_version = if refreshed != original.value {
            self.runner
                .secret_put_checked(
                    AUTH_SECRET,
                    &refreshed,
                    original.version,
                    Some(&original.secret_ref),
                )
                .await
                .map_err(|_| "vault auth CAS failed")?
        } else {
            original.version
        };
        let stored = self
            .runner
            .secret_get_version(AUTH_SECRET)
            .await
            .map_err(|_| "vault custody read-back failed")?
            .ok_or("auth was disconnected")?;
        if stored.secret_ref != original.secret_ref
            || stored.version != expected_version
            || stored.value != refreshed
        {
            return Err("auth custody read-back mismatch".into());
        }
        Ok(())
    }

    async fn finish_runtime(&self, status: &mut RuntimeStatus, report: &mut RunReport) {
        report
            .completed_at
            .get_or_insert_with(|| Utc::now().to_rfc3339());
        status.last_attempt_date = Some(report.date.clone());
        status.last_attempt_result = Some(report.outcome.label().into());
        status.last_attempt_detail = outcome_detail(&report.outcome);
        status.last_attempt = serde_json::to_value(&report).ok();
        if let Err(error) = self.store_runtime_status(status).await {
            report.persistence_error = Some(format!("runtime persistence failed: {error}"));
        }
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
        answer_name: &str,
    ) -> ExecResult {
        match self
            .exec_codex_raw(run_home, env, dream_prompt, budget, answer_name)
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
        // Distinct per-stage files and a clean output slot prevent stale draft
        // bytes from being accepted as a successful audit with no output.
        if let Err(error) = std::fs::remove_file(run_home.work_dir.join(answer_name))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return RawExec::SpawnFailed("could not clear model output slot".into());
        }
        let mut env = env.clone();
        // The MCP server codex spawns needs the model credential; it is
        // forwarded by name through the codex MCP config.
        env.insert("BRUNN_API_URL".into(), self.config.api_url.clone());
        env.insert("BRUNN_API_TOKEN".into(), self.config.model_token.clone());
        let mut argv = codex::exec_command(&ExecSpec {
            codex: &self.config.codex_path,
            model: &self.config.codex_model,
            mcp_server_entry: &self.config.mcp_server_entry,
            working_dir: &run_home.work_dir,
            last_message_path: &run_home.work_dir.join(answer_name),
        });
        if answer_name.starts_with("location-") {
            codex::restrict_to_location_evidence(
                &mut argv,
                answer_name == "location-discovery-answer.md",
            );
            env.remove("BRUNN_API_TOKEN");
            env.remove("BRUNN_API_URL");
        }
        if let Some(effort) = self.config.host_env.get("DREAMER_REASONING_EFFORT") {
            if ["low", "medium", "high", "xhigh", "max", "ultra"].contains(&effort.as_str()) {
                let at = argv.len() - 1;
                argv.splice(
                    at..at,
                    [
                        "--config".into(),
                        format!("model_reasoning_effort=\"{effort}\""),
                    ],
                );
            }
        }
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .env_clear()
            .envs(&env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // A timeout ends the whole model/MCP process group before custody is
        // finalized, so a surviving child cannot refresh credentials later.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return RawExec::SpawnFailed(format!("could not spawn codex: {error}")),
        };
        #[cfg(unix)]
        let _process_group = ProcessGroup(child.id().map(|id| id as i32));
        let stdin = child.stdin.take();
        let execution = async {
            // Start draining output while feeding input: either pipe can fill.
            // Both directions are inside the deadline, including a child which
            // never reads its prompt at all.
            let feed = async {
                if let Some(mut stdin) = stdin {
                    let _ = stdin.write_all(input.as_bytes()).await;
                }
            };
            let (_, result) = tokio::join!(feed, child.wait_with_output());
            result
        };
        match tokio::time::timeout(budget, execution).await {
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

#[cfg(unix)]
struct ProcessGroup(Option<i32>);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // SAFETY: the child was created in a new group with this positive
            // pid. A negative pid targets only that model process group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
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
        let result = (|| {
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
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&root);
        }
        result
    }

    fn cleanup(&self) {
        if let Some(root) = self.home.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn outcome_detail(outcome: &RunOutcome) -> Option<String> {
    match outcome {
        RunOutcome::Disabled { reason } => Some(reason.clone()),
        RunOutcome::SkippedAuth { detail }
        | RunOutcome::Partial { detail }
        | RunOutcome::Failed { detail } => Some(detail.clone()),
        _ => None,
    }
}

impl Drop for RunHome {
    fn drop(&mut self) {
        self.cleanup();
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

    #[tokio::test]
    async fn nonreading_child_cannot_block_the_prompt_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        std::fs::write(&codex, "#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dreamer = Dreamer::new(DreamerConfig {
            api_url: "http://127.0.0.1:1".into(),
            workspace_token: "wrapper".into(),
            model_token: "model".into(),
            runner_token: "runner".into(),
            codex_path: codex,
            codex_model: "fixture".into(),
            mcp_server_entry: "/dev/null".into(),
            work_root: dir.path().join("work"),
            host_env: BTreeMap::new(),
            time_budget_override: None,
        });
        let home = RunHome::create(&dreamer.config.work_root, "fixture", "{}").unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            dreamer.exec_codex_raw(
                &home,
                &BTreeMap::new(),
                &"x".repeat(2 * 1024 * 1024),
                Duration::from_millis(50),
                "answer.md",
            ),
        )
        .await
        .expect("prompt feed escaped the execution deadline");
        assert!(matches!(result, RawExec::TimedOut));
    }

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

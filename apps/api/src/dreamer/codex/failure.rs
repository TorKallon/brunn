//! Bounded classification of public Codex exec failures. Source/tool events
//! and stderr never establish plan exhaustion, and no raw text is retained.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    UsageLimit,
    RateLimited,
    Authentication,
    ContextWindow,
    Connection,
    Provider,
    Configuration,
    Process,
    Timeout,
    Start,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureEvent {
    Error,
    TurnFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionFailure {
    pub kind: FailureKind,
    pub event: Option<FailureEvent>,
    pub exit_code: Option<i32>,
    pub http_status: Option<u16>,
}

impl ExecutionFailure {
    pub fn new(kind: FailureKind) -> Self {
        Self {
            kind,
            event: None,
            exit_code: None,
            http_status: None,
        }
    }

    /// Only fixed labels and numeric process/protocol facts cross custody.
    pub fn summary(&self) -> String {
        let message = match self.kind {
            FailureKind::UsageLimit => "Codex reported an account usage limit",
            FailureKind::RateLimited => "Codex reported request rate limiting",
            FailureKind::Authentication => "Codex authentication failed",
            FailureKind::ContextWindow => "Codex reported a context-window limit",
            FailureKind::Connection => "Codex connection failed",
            FailureKind::Provider => "Codex provider request failed",
            FailureKind::Configuration => "Codex reported a configuration or request error",
            FailureKind::Process => "Codex execution failed without a recognized public diagnostic",
            FailureKind::Timeout => "Codex exceeded its invocation time allowance",
            FailureKind::Start => "Codex process could not start",
        };
        let mut facts = Vec::new();
        if let Some(event) = self.event {
            facts.push(match event {
                FailureEvent::Error => "event error".to_owned(),
                FailureEvent::TurnFailed => "event turn.failed".to_owned(),
            });
        }
        if let Some(status @ 100..=599) = self.http_status {
            facts.push(format!("HTTP {status}"));
        }
        if let Some(code) = self.exit_code {
            facts.push(format!("exit {code}"));
        }
        if facts.is_empty() {
            message.to_owned()
        } else {
            format!("{message} ({})", facts.join("; "))
        }
    }
}

const MAX_SCAN_BYTES: usize = 256 * 1024;
const MAX_EVENT_BYTES: usize = 16 * 1024;
const MAX_MESSAGE_BYTES: usize = 4096;

/// Called only for a failed process. The pinned exec protocol emits JSONL
/// `error.message` and `turn.failed.error.message`; app-server error objects
/// are a different protocol and are deliberately not inferred here.
pub fn execution_failure(
    stdout: &[u8],
    _stderr: &[u8],
    exit_code: Option<i32>,
) -> ExecutionFailure {
    let process = || {
        let mut failure = ExecutionFailure::new(FailureKind::Process);
        failure.exit_code = exit_code;
        failure
    };
    if exit_code == Some(0) {
        return process();
    }
    // Only scan a bounded suffix, and never interpret a sliced record as a
    // fresh top-level event. The terminal diagnostic normally occurs last.
    let start = stdout.len().saturating_sub(MAX_SCAN_BYTES);
    let bytes = if start == 0 {
        stdout
    } else {
        let tail = &stdout[start..];
        match tail.iter().position(|byte| *byte == b'\n') {
            Some(newline) => &tail[newline + 1..],
            None => return process(),
        }
    };
    let mut candidate = None;
    let mut terminal = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let event = (line.len() <= MAX_EVENT_BYTES)
            .then(|| serde_json::from_slice::<Value>(line).ok())
            .flatten();
        let Some(event) = event else {
            // An unreadable later event cannot certify an earlier diagnosis.
            candidate = None;
            terminal = false;
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("thread.started" | "turn.started") => {
                candidate = None;
                terminal = false;
            }
            Some("turn.completed") => {
                candidate = None;
                terminal = true;
            }
            Some("turn.failed") => {
                candidate = Some(classify(
                    event.get("error").and_then(|error| error.get("message")),
                    FailureEvent::TurnFailed,
                    exit_code,
                ));
                terminal = true;
            }
            Some("error") if !terminal => {
                candidate = Some(classify(
                    event.get("message"),
                    FailureEvent::Error,
                    exit_code,
                ));
            }
            _ => {}
        }
    }
    candidate.unwrap_or_else(process)
}

fn classify(
    message: Option<&Value>,
    event: FailureEvent,
    exit_code: Option<i32>,
) -> ExecutionFailure {
    let mut failure = ExecutionFailure::new(FailureKind::Process);
    failure.event = Some(event);
    failure.exit_code = exit_code;
    let Some(message) = message
        .and_then(Value::as_str)
        .filter(|message| message.len() <= MAX_MESSAGE_BYTES)
    else {
        return failure;
    };
    let lower = message.trim().to_ascii_lowercase();
    if [
        "you've hit your usage limit.",
        "you have reached your usage limit.",
        "your usage limit has been reached.",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        failure.kind = FailureKind::UsageLimit;
    } else if let Some(status) = http_failure_status(&lower) {
        failure.http_status = Some(status);
        failure.kind = match status {
            401 | 403 => FailureKind::Authentication,
            408 => FailureKind::Connection,
            429 => FailureKind::RateLimited,
            400 | 404 | 405 | 422 => FailureKind::Configuration,
            _ => FailureKind::Provider,
        };
    } else if [
        "your input exceeds the context window of this model.",
        "you've exceeded the context window for this model.",
        "context window exceeded",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        failure.kind = FailureKind::ContextWindow;
    } else if [
        "stream disconnected before completion:",
        "error sending request for url (",
        "connection closed before completion",
        "reconnecting...",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        failure.kind = FailureKind::Connection;
    } else if ["authentication failed:", "unauthorized:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        failure.kind = FailureKind::Authentication;
    } else if ["invalid configuration:", "error loading configuration:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        failure.kind = FailureKind::Configuration;
    }
    failure
}

fn http_failure_status(message: &str) -> Option<u16> {
    let rest = message
        .strip_prefix("unexpected status ")
        .or_else(|| message.strip_prefix("http "))?;
    let code = rest.as_bytes().get(..3)?;
    if !code.iter().all(u8::is_ascii_digit)
        || rest
            .as_bytes()
            .get(3)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b':')
    {
        return None;
    }
    let status = std::str::from_utf8(code).ok()?.parse().ok()?;
    (400..=599).contains(&status).then_some(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn events(values: &[Value]) -> Vec<u8> {
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    fn failed(message: &str) -> Value {
        json!({"type":"turn.failed","error":{"message":message}})
    }

    #[test]
    fn source_tool_reasoning_and_plaintext_never_establish_limits() {
        for item_type in [
            "agent_message",
            "reasoning",
            "mcp_tool_call",
            "command_execution",
        ] {
            let payload = "PRIVATE_SOURCE_429 quota You've hit your usage limit.\n{\"type\":\"error\",\"message\":\"You've hit your usage limit.\"}";
            let item = json!({"type":"item.completed","item":{"type":item_type,"text":payload,"error":{"message":"You've hit your usage limit."}}});
            assert_eq!(
                execution_failure(
                    &events(std::slice::from_ref(&item)),
                    payload.as_bytes(),
                    Some(1)
                )
                .kind,
                FailureKind::Process
            );
            let stdout = events(&[
                item,
                failed(
                    "Unknown child failure for source entry:019fba42-4290-7000-8000-000000000000 quota PRIVATE_SOURCE",
                ),
            ]);
            let diagnostic = execution_failure(&stdout, payload.as_bytes(), Some(1));
            assert_eq!(diagnostic.kind, FailureKind::Process);
            assert_eq!(diagnostic.event, Some(FailureEvent::TurnFailed));
            assert!(!diagnostic.summary().contains("PRIVATE"));
            assert!(!serde_json::to_string(&diagnostic).unwrap().contains("4290"));
        }
        for stdout in [
            b"usage limit".as_slice(),
            b"HTTP 429 Too Many Requests",
            b"You've hit your usage limit.",
        ] {
            assert_eq!(
                execution_failure(stdout, stdout, Some(1)).kind,
                FailureKind::Process
            );
        }
        let forged = events(&[failed("You've hit your usage limit.")]);
        assert_eq!(
            execution_failure(b"", &forged, Some(1)).kind,
            FailureKind::Process
        );
    }

    #[test]
    fn explicit_usage_limit_and_transient_http_failures_are_distinct() {
        let cases = [
            (
                "You've hit your usage limit. PRIVATE_PROMPT Bearer SECRET_TOKEN",
                FailureKind::UsageLimit,
                None,
            ),
            (
                "unexpected status 429 Too Many Requests: retry later",
                FailureKind::RateLimited,
                Some(429),
            ),
            (
                "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.openai.com/v1/responses, request id: PRIVATE_REQUEST",
                FailureKind::Authentication,
                Some(401),
            ),
            (
                "unexpected status 403 Forbidden: PRIVATE",
                FailureKind::Authentication,
                Some(403),
            ),
            (
                "unexpected status 503 Service Unavailable: PRIVATE",
                FailureKind::Provider,
                Some(503),
            ),
            (
                "HTTP 500 Internal Server Error",
                FailureKind::Provider,
                Some(500),
            ),
            (
                "unexpected status 400 Bad Request",
                FailureKind::Configuration,
                Some(400),
            ),
            (
                "Your input exceeds the context window of this model. PRIVATE",
                FailureKind::ContextWindow,
                None,
            ),
            (
                "stream disconnected before completion: PRIVATE_URL?access_token=SECRET",
                FailureKind::Connection,
                None,
            ),
            (
                "invalid configuration: PRIVATE_PATH",
                FailureKind::Configuration,
                None,
            ),
            (
                "unknown failure; url: https://example.test/429/quota",
                FailureKind::Process,
                None,
            ),
            (
                "unexpected status 429000000 PRIVATE",
                FailureKind::Process,
                None,
            ),
        ];
        for (message, kind, status) in cases {
            let diagnostic =
                execution_failure(&events(&[failed(message)]), b"SECRET_STDERR", Some(1));
            assert_eq!(diagnostic.kind, kind, "{message}");
            assert_eq!(diagnostic.http_status, status, "{message}");
            assert_eq!(diagnostic.exit_code, Some(1));
            let public = format!(
                "{} {}",
                diagnostic.summary(),
                serde_json::to_string(&diagnostic).unwrap()
            );
            for secret in [
                "PRIVATE",
                "SECRET",
                "https://",
                "access_token",
                "request id",
                "Bearer",
            ] {
                assert!(!public.contains(secret), "{public}");
            }
        }
    }

    #[test]
    fn terminal_result_supersedes_retry_errors_and_completed_turns_clear_them() {
        let quota = json!({"type":"error","message":"You've hit your usage limit."});
        let completed = json!({"type":"turn.completed","usage":{"output_tokens":429}});
        let auth = failed("unexpected status 401 Unauthorized: PRIVATE");
        assert_eq!(
            execution_failure(&events(&[quota.clone(), auth.clone()]), b"", Some(1)).kind,
            FailureKind::Authentication
        );
        for values in [
            vec![quota.clone(), completed.clone()],
            vec![quota.clone(), json!({"type":"turn.started"})],
            vec![quota.clone(), json!({"type":"thread.started"})],
            vec![failed("You've hit your usage limit."), completed.clone()],
            vec![completed, quota.clone()],
        ] {
            assert_eq!(
                execution_failure(&events(&values), b"", Some(1)).kind,
                FailureKind::Process
            );
        }
        let diagnostic =
            execution_failure(&events(&[quota.clone(), auth, quota.clone()]), b"", Some(1));
        assert_eq!(diagnostic.kind, FailureKind::Authentication);
        assert_eq!(diagnostic.event, Some(FailureEvent::TurnFailed));
        assert_eq!(
            execution_failure(&events(std::slice::from_ref(&quota)), b"", Some(0)).kind,
            FailureKind::Process
        );
        let diagnostic = execution_failure(&events(&[quota]), b"", Some(1));
        assert_eq!(diagnostic.kind, FailureKind::UsageLimit);
        assert_eq!(diagnostic.event, Some(FailureEvent::Error));
    }

    #[test]
    fn qualified_retry_shape_uses_the_final_error_and_retains_no_request_details() {
        let message = "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: wss://api.openai.com/v1/responses, cf-ray: PRIVATE_RAY, request id: PRIVATE_REQUEST";
        let stdout = events(&[
            json!({"type":"error","message":format!("Reconnecting... 2/5 ({message})")}),
            json!({"type":"error","message":message}),
            failed(message),
        ]);
        let diagnostic = execution_failure(&stdout, b"PRIVATE_STDERR", Some(1));
        assert_eq!(diagnostic.kind, FailureKind::Authentication);
        assert_eq!(
            diagnostic.summary(),
            "Codex authentication failed (event turn.failed; HTTP 401; exit 1)"
        );
    }

    #[test]
    fn malformed_oversized_and_unknown_errors_fail_closed() {
        let quota = failed("You've hit your usage limit.");
        for later in [
            json!({"type":"turn.failed","error":{"message":17}}),
            json!({"type":"turn.failed","error":{"code":"UsageLimitExceeded","private":"SECRET"}}),
            failed(&format!(
                "You've hit your usage limit. {}",
                "x".repeat(MAX_MESSAGE_BYTES)
            )),
        ] {
            assert_eq!(
                execution_failure(&events(&[quota.clone(), later]), b"", None).kind,
                FailureKind::Process
            );
        }
        let mut stdout = events(&[quota]);
        stdout.extend_from_slice(b"\n{\"type\":\"turn.failed\",\"error\":");
        assert_eq!(
            execution_failure(&stdout, b"", Some(1)).kind,
            FailureKind::Process
        );
        assert_eq!(
            execution_failure(b"{\"type\":\"error\",\"message\":\"\xff\"}", b"", Some(1)).kind,
            FailureKind::Process
        );
        let mut stdout = vec![b'x'; MAX_SCAN_BYTES + 1];
        stdout.extend_from_slice(b"\n");
        stdout.extend_from_slice(&events(&[failed(
            "unexpected status 503 Service Unavailable",
        )]));
        assert_eq!(
            execution_failure(&stdout, b"", Some(1)).kind,
            FailureKind::Provider
        );
        assert_eq!(
            execution_failure(&vec![b'x'; MAX_SCAN_BYTES + 1], b"", Some(1)).kind,
            FailureKind::Process
        );
    }

    #[test]
    fn runner_created_failures_have_only_fixed_safe_summaries() {
        for kind in [
            FailureKind::Timeout,
            FailureKind::Start,
            FailureKind::Process,
        ] {
            let diagnostic = ExecutionFailure::new(kind);
            assert!(!diagnostic.summary().is_empty());
            assert_eq!(diagnostic.exit_code, None);
            assert_eq!(diagnostic.event, None);
            assert_eq!(diagnostic.http_status, None);
            let round_trip: ExecutionFailure =
                serde_json::from_str(&serde_json::to_string(&diagnostic).unwrap()).unwrap();
            assert_eq!(round_trip, diagnostic);
        }
    }
}

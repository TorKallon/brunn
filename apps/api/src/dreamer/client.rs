//! The dreamer's HTTP client for the Brunn API.
//!
//! The dreamer shares this crate's code but never its database authority:
//! every workspace effect goes through the API. The model receives only its
//! dedicated read-only identity. The wrapper retains its existing read identity
//! and a distinct fenced-publication/vault/notification credential.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{Value, json};

#[derive(Debug)]
pub enum ClientError {
    /// The API rejected a CAS write because the entry moved.
    Conflict {
        actual_version: Option<i64>,
        detail: String,
    },
    /// Anything else: transport, auth, or server failure.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Conflict {
                actual_version,
                detail,
            } => {
                write!(f, "conflict (actual {actual_version:?}): {detail}")
            }
            ClientError::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for ClientError {}

pub type ClientResult<T> = Result<T, ClientError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    pub content: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReceipt {
    pub version: Option<i64>,
    pub workspace_generation: Option<i64>,
    pub no_op: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeRecord {
    pub generation: i64,
    pub operation: String,
    pub path: String,
    pub version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangesPage {
    pub workspace_generation: i64,
    pub next_generation: i64,
    pub truncated: bool,
    pub changes: Vec<ChangeRecord>,
}

/// Deliberately no Debug: this value carries plaintext custody material.
pub struct SecretVersion {
    pub secret_ref: String,
    pub value: String,
    pub version: i64,
}

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl ApiClient {
    pub fn new(base_url: &str, token: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn post(&self, path: &str, body: Value) -> ClientResult<Value> {
        let response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|error| ClientError::Failed(format!("POST {path}: {error}")))?;
        Self::decode(path, response).await
    }

    pub async fn get(&self, path: &str) -> ClientResult<Value> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| ClientError::Failed(format!("GET {path}: {error}")))?;
        Self::decode(path, response).await
    }

    async fn decode(path: &str, response: reqwest::Response) -> ClientResult<Value> {
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        // Retain only the bounded public error, never response details or bodies.
        let message: String = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("request failed")
            .chars()
            .take(1024)
            .collect();
        if status == StatusCode::CONFLICT {
            let actual_version = body
                .pointer("/error/details/actual_version")
                .and_then(Value::as_i64);
            return Err(ClientError::Conflict {
                actual_version,
                detail: message,
            });
        }
        if !status.is_success() {
            return Err(ClientError::Failed(format!("{path}: {status} {message}")));
        }
        Ok(body)
    }

    /// Read one markdown file. `Ok(None)` means it does not exist.
    pub async fn read_markdown(&self, path: &str) -> ClientResult<Option<FileVersion>> {
        let body = self
            .post(
                "/v1/workspace/read",
                json!({"requests": [{"path": path, "view": "full"}]}),
            )
            .await?;
        let item = body
            .pointer("/data/items/0")
            .cloned()
            .ok_or_else(|| ClientError::Failed(format!("read {path}: empty response")))?;
        if item.get("status").and_then(Value::as_str) == Some("not_found") {
            return Ok(None);
        }
        let content = item
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ClientError::Failed(format!("read {path}: no text field")))?
            .to_owned();
        let version = item
            .get("version")
            .and_then(Value::as_i64)
            .ok_or_else(|| ClientError::Failed(format!("read {path}: no version field")))?;
        Ok(Some(FileVersion { content, version }))
    }

    /// CAS write. `expected_version` 0 requires a fresh create; `None` is an
    /// unconditional write (the dreamer only uses that for its own new files
    /// under `dreams/runs/`).
    pub async fn write_markdown(
        &self,
        path: &str,
        content: &str,
        expected_version: Option<i64>,
        idempotency_key: Option<&str>,
    ) -> ClientResult<WriteReceipt> {
        let mut request = json!({
            "path": path,
            "content": content,
        });
        if let Some(version) = expected_version {
            request["expected_version"] = json!(version);
        }
        if let Some(key) = idempotency_key {
            request["idempotency_key"] = json!(key);
        }
        let body = self.post("/v1/workspace/write", request).await?;
        Ok(WriteReceipt {
            version: body.pointer("/data/version").and_then(Value::as_i64),
            workspace_generation: body
                .pointer("/data/workspace_generation")
                .and_then(Value::as_i64),
            no_op: body.pointer("/data/no_op").and_then(Value::as_bool) == Some(true),
        })
    }

    /// The locked conflict policy: on 409, re-read once, recompute, retry
    /// once, else give up (the caller defers the change to the report).
    pub async fn write_with_conflict_retry(
        &self,
        path: &str,
        compose: impl Fn(Option<&FileVersion>) -> String,
    ) -> ClientResult<WriteReceipt> {
        let current = self.read_markdown(path).await?;
        let expected = current.as_ref().map_or(0, |file| file.version);
        let content = compose(current.as_ref());
        match self
            .write_markdown(path, &content, Some(expected), None)
            .await
        {
            Ok(receipt) => Ok(receipt),
            Err(ClientError::Conflict { .. }) => {
                let current = self.read_markdown(path).await?;
                let expected = current.as_ref().map_or(0, |file| file.version);
                let content = compose(current.as_ref());
                self.write_markdown(path, &content, Some(expected), None)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    /// The frontmatter `kind` of each path, read from its first lines in
    /// batches. A missing file or one without frontmatter maps to `None`.
    pub async fn frontmatter_kinds(
        &self,
        paths: &[String],
    ) -> ClientResult<std::collections::BTreeMap<String, Option<String>>> {
        let mut kinds = std::collections::BTreeMap::new();
        for chunk in paths.chunks(32) {
            let requests = chunk
                .iter()
                .map(|path| json!({"path": path, "view": "range", "start": 1, "end": 20}))
                .collect::<Vec<_>>();
            let body = self
                .post("/v1/workspace/read", json!({"requests": requests}))
                .await?;
            let items = body
                .pointer("/data/items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (path, item) in chunk.iter().zip(items) {
                let kind = item
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(super::change_set::frontmatter_kind)
                    .map(str::to_owned);
                kinds.insert(path.clone(), kind);
            }
        }
        Ok(kinds)
    }

    pub async fn changes_since(&self, since: i64, limit: usize) -> ClientResult<ChangesPage> {
        let body = self
            .get(&format!(
                "/v1/workspace/changes?since_generation={since}&limit={limit}"
            ))
            .await?;
        let data = body
            .get("data")
            .ok_or_else(|| ClientError::Failed("changes: no data".into()))?;
        let changes = data
            .get("changes")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some(ChangeRecord {
                            generation: entry.get("generation")?.as_i64()?,
                            operation: entry
                                .get("operation")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            path: entry.get("path")?.as_str()?.to_owned(),
                            version: entry.get("version").and_then(Value::as_i64),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ChangesPage {
            workspace_generation: data
                .get("workspace_generation")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            next_generation: data
                .get("next_generation")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            truncated: data.get("truncated").and_then(Value::as_bool) == Some(true),
            changes,
        })
    }

    /// The current workspace generation, used as the pre-run confinement mark.
    pub async fn current_generation(&self) -> ClientResult<i64> {
        Ok(self
            .changes_since(i64::MAX - 1, 1)
            .await?
            .workspace_generation)
    }

    pub async fn secret_get(&self, name: &str) -> ClientResult<Option<String>> {
        match self
            .post("/v1/workspace/secrets/get", json!({"name": name}))
            .await
        {
            Ok(body) => Ok(body
                .pointer("/data/value")
                .or_else(|| body.get("value"))
                .and_then(Value::as_str)
                .map(str::to_owned)),
            Err(ClientError::Failed(detail)) if detail.contains("404") => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn secret_put(&self, name: &str, value: &str, description: &str) -> ClientResult<()> {
        self.post(
            "/v1/workspace/secrets/put",
            json!({"name": name, "value": value, "description": description}),
        )
        .await
        .map(|_| ())
    }

    pub async fn secret_get_version(&self, name: &str) -> ClientResult<Option<SecretVersion>> {
        let response = self
            .http
            .post(format!("{}/v1/workspace/secrets/get", self.base_url))
            .bearer_auth(&self.token)
            .json(&json!({"name": name}))
            .send()
            .await
            .map_err(|_| ClientError::Failed("vault read transport failed".into()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = Self::decode("/v1/workspace/secrets/get", response).await?;
        let data = response.get("data").unwrap_or(&response);
        Ok(Some(SecretVersion {
            secret_ref: data["secret_ref"]
                .as_str()
                .ok_or_else(|| ClientError::Failed("vault identity missing".into()))?
                .to_owned(),
            value: data["value"]
                .as_str()
                .ok_or_else(|| ClientError::Failed("vault value missing".into()))?
                .to_owned(),
            version: data["version"]
                .as_i64()
                .filter(|v| *v > 0)
                .ok_or_else(|| ClientError::Failed("vault version missing".into()))?,
        }))
    }

    pub async fn secret_put_checked(
        &self,
        name: &str,
        value: &str,
        expected_version: i64,
        expected_secret_ref: Option<&str>,
    ) -> ClientResult<i64> {
        let response = self.post("/v1/workspace/secrets/put", json!({"name":name,"value":value,"expected_version":expected_version,"expected_secret_ref":expected_secret_ref})).await?;
        let data = response.get("data").unwrap_or(&response);
        data["version"]
            .as_i64()
            .filter(|v| *v > 0)
            .ok_or_else(|| ClientError::Failed("vault write version missing".into()))
    }

    pub async fn secret_delete(&self, name: &str) -> ClientResult<()> {
        self.post("/v1/workspace/secrets/delete", json!({"name": name}))
            .await
            .map(|_| ())
    }

    /// Server-owned attempt operations. Returned values are unwrapped here so
    /// transport envelopes never leak into lifecycle state.
    pub async fn dreamer(&self, operation: &str, mut body: Value) -> ClientResult<Value> {
        let path = format!("/v1/workspace/dreamer/{operation}");
        let result = self.post(&path, body.clone()).await;
        // An unrelated owner decision can advance the shared review state while
        // the model reasons. Retry only an explicit, pre-mutation state rejection.
        // Keep the same evidence and attempt fence: the server revalidates source
        // freshness, ownership, decisions and destinations under its lock.
        let response = match result {
            Err(ClientError::Conflict {
                actual_version: Some(version),
                detail,
            }) if matches!(operation, "candidates" | "checkpoint" | "location-discover")
                && detail == "Review or run state changed; reload before retrying"
                && body["expected_state_version"]
                    .as_i64()
                    .is_some_and(|expected| version > expected) =>
            {
                body["expected_state_version"] = json!(version);
                self.post(&path, body).await?
            }
            other => other?,
        };
        Ok(response.get("data").cloned().unwrap_or(response))
    }

    pub async fn review_ready(
        &self,
        event_key: &str,
        run_ref: &str,
        version: i64,
        count: usize,
    ) -> ClientResult<Value> {
        let response = self.post("/v1/workspace/notifications/publish", json!({
            "event_key": event_key, "correlation_id": event_key,
            "kind": "operational", "importance": "important",
            "title": "Dreaming is ready for review",
            "body": format!("{count} new review item(s) have evidence and a candidate preview. [Open Review](https://brunn.ai/dreams) to approve, reject, defer, or correct them."),
            "target": {"type": "entry", "entry_ref": run_ref},
            "source": {"type": "dreamer_run", "ref": run_ref, "version_ref": format!("{run_ref}@{version}")}
        })).await?;
        Ok(response.get("data").cloned().unwrap_or(response))
    }

    /// One operational notification. Uses a stable event key so retries and
    /// repeated skip states never fan out into notification spam.
    pub async fn notify(&self, event_key: &str, title: &str, body: &str) -> ClientResult<Value> {
        self.post(
            "/v1/workspace/notifications/publish",
            json!({
                "event_key": event_key,
                "correlation_id": event_key,
                "kind": "operational",
                "importance": "important",
                "title": title,
                "body": body,
                "target": {"type": "notification"}
            }),
        )
        .await
        .map(|response| response.get("data").cloned().unwrap_or(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Transport-level behavior (409 mapping, retry-once) is covered against a
    // mock HTTP server in the integration tests; here we only pin pure logic.

    #[test]
    fn conflict_error_renders() {
        let error = ClientError::Conflict {
            actual_version: Some(4),
            detail: "source changed".into(),
        };
        assert!(error.to_string().contains('4'));
    }
}

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    config::Config,
    error::{ApiError, ApiResult},
};

static TOKEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?u)[\p{L}\p{N}_-]+").expect("token regex"));

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, input: &[String]) -> ApiResult<Vec<Vec<f32>>>;
    fn provider(&self) -> &'static str;
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn is_degraded(&self) -> bool;
}

pub type SharedEmbedder = Arc<dyn Embedder>;

pub fn from_config(config: &Config) -> ApiResult<SharedEmbedder> {
    match config.embedding_provider.as_str() {
        "openai" => {
            if let Some(api_key) = &config.openai_api_key {
                Ok(Arc::new(OpenAiEmbedder::new(
                    api_key.clone(),
                    config.openai_base_url.clone(),
                    config.embedding_model.clone(),
                    config.embedding_dimensions,
                )?))
            } else {
                tracing::warn!("OPENAI_API_KEY is absent; using deterministic degraded embeddings");
                Ok(Arc::new(HashingEmbedder::new(
                    config.embedding_model.clone(),
                    config.embedding_dimensions,
                    true,
                )))
            }
        }
        "hashing" => Ok(Arc::new(HashingEmbedder::new(
            "straylight-hashing-v1".to_owned(),
            config.embedding_dimensions,
            true,
        ))),
        other => Err(ApiError::configuration(format!(
            "unsupported STRAYLIGHT_EMBEDDING_PROVIDER: {other}"
        ))),
    }
}

#[derive(Clone)]
pub struct OpenAiEmbedder {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
    dimensions: usize,
}

impl OpenAiEmbedder {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        dimensions: usize,
    ) -> ApiResult<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|error| {
                ApiError::Internal(format!("could not build OpenAI client: {error}"))
            })?;
        Ok(Self {
            client,
            api_key,
            endpoint: format!("{}/embeddings", base_url.trim_end_matches('/')),
            model,
            dimensions,
        })
    }
}

fn embedding_dependency_unavailable(stage: &str, error: impl std::fmt::Display) -> ApiError {
    tracing::warn!(stage, error = %error, "OpenAI embedding dependency unavailable");
    ApiError::public(
        http::StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
        "OpenAI embeddings are temporarily unavailable",
    )
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
    dimensions: usize,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    mut_data_placeholder: Option<String>,
    #[serde(rename = "data")]
    items: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorEnvelope {
    error: OpenAiError,
}

#[derive(Debug, Deserialize)]
struct OpenAiError {
    message: String,
    #[serde(default)]
    r#type: String,
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, input: &[String]) -> ApiResult<Vec<Vec<f32>>> {
        if input.is_empty() {
            return Ok(vec![]);
        }
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&EmbeddingRequest {
                model: &self.model,
                input,
                encoding_format: "float",
                dimensions: self.dimensions,
            })
            .send()
            .await
            .map_err(|error| embedding_dependency_unavailable("send", error))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| embedding_dependency_unavailable("read_response", error))?;
        if !status.is_success() {
            let message = serde_json::from_slice::<OpenAiErrorEnvelope>(&body)
                .map(|error| format!("{} ({})", error.error.message, error.error.r#type))
                .unwrap_or_else(|_| String::from_utf8_lossy(&body).into_owned());
            return Err(ApiError::public(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "dependency_unavailable",
                format!("OpenAI embeddings unavailable: {message}"),
            ));
        }
        let mut parsed: EmbeddingResponse = serde_json::from_slice(&body)?;
        let _ = parsed.mut_data_placeholder.take();
        parsed.items.sort_by_key(|item| item.index);
        if parsed.items.len() != input.len()
            || parsed
                .items
                .iter()
                .any(|item| item.embedding.len() != self.dimensions)
        {
            return Err(ApiError::Internal(
                "embedding provider returned an unexpected result shape".to_owned(),
            ));
        }
        Ok(parsed
            .items
            .into_iter()
            .map(|item| item.embedding)
            .collect())
    }

    fn provider(&self) -> &'static str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn is_degraded(&self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct HashingEmbedder {
    model: String,
    dimensions: usize,
    degraded: bool,
}

impl HashingEmbedder {
    pub fn new(model: String, dimensions: usize, degraded: bool) -> Self {
        Self {
            model,
            dimensions,
            degraded,
        }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for token in TOKEN.find_iter(text) {
            *counts.entry(token.as_str().to_lowercase()).or_default() += 1;
        }
        let mut vector = vec![0.0f32; self.dimensions];
        for (token, count) in counts {
            let digest = Sha256::digest(token.as_bytes());
            let index = u64::from_be_bytes(digest[0..8].try_into().expect("digest slice")) as usize
                % self.dimensions;
            let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
            vector[index] += sign * (1.0 + count as f32).ln();
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }
        vector
    }
}

#[async_trait]
impl Embedder for HashingEmbedder {
    async fn embed(&self, input: &[String]) -> ApiResult<Vec<Vec<f32>>> {
        Ok(input.iter().map(|text| self.vector(text)).collect())
    }

    fn provider(&self) -> &'static str {
        "hashing"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn is_degraded(&self) -> bool {
        self.degraded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashing_embeddings_are_stable_and_normalized() {
        let embedder = HashingEmbedder::new("test".to_owned(), 64, true);
        let vectors = embedder
            .embed(&[
                "ski team schedule".to_owned(),
                "ski team schedule".to_owned(),
            ])
            .await
            .unwrap();
        assert_eq!(vectors[0], vectors[1]);
        let norm = vectors[0]
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 0.0001);
    }

    #[test]
    fn transient_embedding_transport_failures_are_retryable_and_sanitized() {
        let error = embedding_dependency_unavailable("send", "socket detail");
        match error {
            ApiError::Public {
                status,
                code,
                message,
                ..
            } => {
                assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(code, "dependency_unavailable");
                assert!(!message.contains("socket detail"));
            }
            other => panic!("unexpected error classification: {other:?}"),
        }
    }
}

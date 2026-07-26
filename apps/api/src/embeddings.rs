use std::{collections::HashMap, sync::Arc, time::Instant};

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
const MAX_OPENAI_INPUT_CHARS: usize = 1_900;
const MAX_OPENAI_REQUEST_BYTES: usize = 100_000;
const MAX_OPENAI_REQUEST_ITEMS: usize = 128;

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
            } else if config.allow_degraded_embeddings {
                tracing::warn!("OPENAI_API_KEY is absent; using deterministic degraded embeddings");
                Ok(Arc::new(HashingEmbedder::new(
                    config.embedding_model.clone(),
                    config.embedding_dimensions,
                    true,
                )))
            } else {
                Err(ApiError::configuration(
                    "OPENAI_API_KEY is required when STRAYLIGHT_EMBEDDING_PROVIDER=openai; set STRAYLIGHT_EMBEDDING_PROVIDER=hashing explicitly for an offline degraded deployment",
                ))
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

pub async fn embed_indexing_inputs(
    embedder: &dyn Embedder,
    input: &[String],
) -> ApiResult<Vec<Vec<f32>>> {
    if input.is_empty() || embedder.provider() != "openai" {
        return embedder.embed(input).await;
    }

    let mut segments = Vec::new();
    let mut segmented_inputs = 0usize;
    for (input_index, text) in input.iter().enumerate() {
        let parts = split_at_character_limit(text, MAX_OPENAI_INPUT_CHARS);
        segmented_inputs += usize::from(parts.len() > 1);
        for part in parts {
            segments.push(EmbeddingSegment {
                input_index,
                weight: part.chars().count().max(1) as f32,
                text: part.to_owned(),
            });
        }
    }

    let mut sums = vec![vec![0.0f32; embedder.dimensions()]; input.len()];
    let mut weights = vec![0.0f32; input.len()];
    let mut segment_counts = vec![0usize; input.len()];
    let mut single_vectors = vec![None; input.len()];
    let mut batch_count = 0usize;
    let mut cursor = 0usize;
    while cursor < segments.len() {
        let mut end = cursor;
        let mut request_bytes = 0usize;
        while end < segments.len() && end - cursor < MAX_OPENAI_REQUEST_ITEMS {
            let next_bytes = segments[end].text.len();
            if end > cursor && request_bytes + next_bytes > MAX_OPENAI_REQUEST_BYTES {
                break;
            }
            request_bytes += next_bytes;
            end += 1;
        }
        let batch = segments[cursor..end]
            .iter()
            .map(|segment| segment.text.clone())
            .collect::<Vec<_>>();
        let vectors = embedder.embed(&batch).await?;
        if vectors.len() != batch.len() {
            return Err(ApiError::Internal(
                "embedding provider returned an unexpected result shape".to_owned(),
            ));
        }
        for (segment, vector) in segments[cursor..end].iter().zip(vectors) {
            if vector.len() != embedder.dimensions() {
                return Err(ApiError::Internal(
                    "embedding provider returned an unexpected result shape".to_owned(),
                ));
            }
            let input_index = segment.input_index;
            if segment_counts[input_index] == 0 {
                single_vectors[input_index] = Some(vector.clone());
            } else {
                single_vectors[input_index] = None;
            }
            for (sum, value) in sums[input_index].iter_mut().zip(&vector) {
                *sum += *value * segment.weight;
            }
            weights[input_index] += segment.weight;
            segment_counts[input_index] += 1;
        }
        batch_count += 1;
        cursor = end;
    }

    let output = sums
        .into_iter()
        .enumerate()
        .map(|(index, mut vector)| {
            if segment_counts[index] == 1 {
                return single_vectors[index]
                    .take()
                    .expect("one segment must retain its provider vector");
            }
            if weights[index] > 0.0 {
                for value in &mut vector {
                    *value /= weights[index];
                }
            }
            normalize(&mut vector);
            vector
        })
        .collect::<Vec<_>>();
    metrics::histogram!("embedding.index.expanded_segments").record(segments.len() as f64);
    metrics::histogram!("embedding.index.segmented_inputs").record(segmented_inputs as f64);
    metrics::histogram!("embedding.index.provider_batches").record(batch_count as f64);
    Ok(output)
}

struct EmbeddingSegment {
    input_index: usize,
    weight: f32,
    text: String,
}

fn split_at_character_limit(text: &str, limit: usize) -> Vec<&str> {
    if text.is_empty() {
        return vec![text];
    }
    let mut segments = Vec::new();
    let mut start_byte = 0usize;
    let mut characters = 0usize;
    for (byte, _) in text.char_indices() {
        if characters == limit {
            segments.push(&text[start_byte..byte]);
            start_byte = byte;
            characters = 0;
        }
        characters += 1;
    }
    segments.push(&text[start_byte..]);
    segments
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
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
        let started = Instant::now();
        let result = async {
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
        .await;
        record_embedding_call(
            self.provider(),
            self.model(),
            self.is_degraded(),
            input,
            &result,
            started,
        );
        result
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
        let started = Instant::now();
        let result = Ok(input.iter().map(|text| self.vector(text)).collect());
        record_embedding_call(
            self.provider(),
            self.model(),
            self.is_degraded(),
            input,
            &result,
            started,
        );
        result
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

fn record_embedding_call(
    provider: &str,
    model: &str,
    degraded: bool,
    input: &[String],
    result: &ApiResult<Vec<Vec<f32>>>,
    started: Instant,
) {
    let outcome = if result.is_ok() { "success" } else { "error" };
    metrics::counter!(
        "embedding.requests",
        "provider" => provider.to_owned(),
        "model" => model.to_owned(),
        "degraded" => if degraded { "true" } else { "false" },
        "result" => outcome
    )
    .increment(1);
    metrics::histogram!(
        "embedding.duration_ms",
        "provider" => provider.to_owned(),
        "model" => model.to_owned(),
        "result" => outcome
    )
    .record(started.elapsed().as_secs_f64() * 1_000.0);
    metrics::histogram!("embedding.input_items", "provider" => provider.to_owned())
        .record(input.len() as f64);
    metrics::histogram!("embedding.input_characters", "provider" => provider.to_owned())
        .record(input.iter().map(String::len).sum::<usize>() as f64);
    if let Ok(vectors) = result {
        metrics::histogram!("embedding.output_vectors", "provider" => provider.to_owned())
            .record(vectors.len() as f64);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

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

    struct LengthLimitedEmbedder {
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait]
    impl Embedder for LengthLimitedEmbedder {
        async fn embed(&self, input: &[String]) -> ApiResult<Vec<Vec<f32>>> {
            assert!(
                input
                    .iter()
                    .all(|text| text.chars().count() <= MAX_OPENAI_INPUT_CHARS)
            );
            assert!(input.iter().map(String::len).sum::<usize>() <= MAX_OPENAI_REQUEST_BYTES);
            assert!(input.len() <= MAX_OPENAI_REQUEST_ITEMS);
            self.calls.lock().unwrap().push(input.to_vec());
            Ok(input
                .iter()
                .map(|text| {
                    let first = text.chars().next().unwrap_or_default() as u32;
                    vec![text.chars().count() as f32, first as f32, 1.0]
                })
                .collect())
        }

        fn provider(&self) -> &'static str {
            "openai"
        }

        fn model(&self) -> &str {
            "test"
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn is_degraded(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn indexing_embeddings_segment_oversized_inputs_without_losing_coverage() {
        let embedder = LengthLimitedEmbedder {
            calls: Mutex::new(Vec::new()),
        };
        let oversized = format!("{}{}", "a".repeat(4_000), "b".repeat(1_000));
        let vectors = embed_indexing_inputs(&embedder, &["short".to_owned(), oversized.clone()])
            .await
            .unwrap();

        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0], vec![5.0, 's' as u32 as f32, 1.0]);
        let calls = embedder.calls.lock().unwrap();
        let flattened = calls
            .iter()
            .flatten()
            .skip(1)
            .map(String::as_str)
            .collect::<String>();
        assert_eq!(flattened, oversized);
        let pooled_norm = vectors[1]
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((pooled_norm - 1.0).abs() < 0.0001);
    }

    #[tokio::test]
    async fn indexing_embeddings_bound_aggregate_request_size() {
        let embedder = LengthLimitedEmbedder {
            calls: Mutex::new(Vec::new()),
        };
        let input = (0..80)
            .map(|index| format!("{index:04}{}", "x".repeat(1_800)))
            .collect::<Vec<_>>();
        let vectors = embed_indexing_inputs(&embedder, &input).await.unwrap();

        assert_eq!(vectors.len(), input.len());
        assert!(embedder.calls.lock().unwrap().len() > 1);
    }
}

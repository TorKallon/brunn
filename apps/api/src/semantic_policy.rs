use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::{
    embeddings::SharedEmbedder,
    error::{ApiError, ApiResult},
};

const QUERY_CACHE_CAPACITY: usize = 4_096;
const QUERY_CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const QUERY_CACHE_NEGATIVE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct QueryEmbeddingKey {
    model: String,
    dimensions: usize,
    normalized_query_sha256: [u8; 32],
}

impl QueryEmbeddingKey {
    fn new(model: &str, dimensions: usize, query: &str) -> Self {
        let normalized = normalize_query(query);
        Self {
            model: model.to_owned(),
            dimensions,
            normalized_query_sha256: Sha256::digest(normalized.as_bytes()).into(),
        }
    }
}

fn normalize_query(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone)]
enum CachedQueryEmbedding {
    Vector(Vec<f32>),
    Negative,
}

struct CacheEntry {
    value: CachedQueryEmbedding,
    expires_at: Instant,
    last_used: u64,
}

struct QueryEmbeddingCacheState {
    entries: HashMap<QueryEmbeddingKey, CacheEntry>,
    clock: u64,
}

#[derive(Clone)]
struct QueryEmbeddingCache {
    inner: Arc<Mutex<QueryEmbeddingCacheState>>,
    capacity: usize,
    ttl: Duration,
    negative_ttl: Duration,
}

enum CacheLookup {
    Hit(Vec<f32>),
    NegativeHit,
    Miss,
}

impl QueryEmbeddingCache {
    fn production() -> Self {
        Self::new(
            QUERY_CACHE_CAPACITY,
            QUERY_CACHE_TTL,
            QUERY_CACHE_NEGATIVE_TTL,
        )
    }

    fn new(capacity: usize, ttl: Duration, negative_ttl: Duration) -> Self {
        assert!(
            capacity > 0,
            "semantic query cache capacity must be positive"
        );
        Self {
            inner: Arc::new(Mutex::new(QueryEmbeddingCacheState {
                entries: HashMap::new(),
                clock: 0,
            })),
            capacity,
            ttl,
            negative_ttl,
        }
    }

    fn lookup(&self, key: &QueryEmbeddingKey) -> CacheLookup {
        self.lookup_at(key, Instant::now())
    }

    fn lookup_at(&self, key: &QueryEmbeddingKey, now: Instant) -> CacheLookup {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.clock = state.clock.saturating_add(1);
        let clock = state.clock;
        let Some(entry) = state.entries.get_mut(key) else {
            return CacheLookup::Miss;
        };
        if entry.expires_at <= now {
            state.entries.remove(key);
            return CacheLookup::Miss;
        }
        entry.last_used = clock;
        match &entry.value {
            CachedQueryEmbedding::Vector(vector) => CacheLookup::Hit(vector.clone()),
            CachedQueryEmbedding::Negative => CacheLookup::NegativeHit,
        }
    }

    fn insert_vector(&self, key: QueryEmbeddingKey, vector: Vec<f32>) {
        self.insert_at(
            key,
            CachedQueryEmbedding::Vector(vector),
            Instant::now(),
            self.ttl,
        );
    }

    fn insert_negative(&self, key: QueryEmbeddingKey) {
        self.insert_at(
            key,
            CachedQueryEmbedding::Negative,
            Instant::now(),
            self.negative_ttl,
        );
    }

    fn insert_at(
        &self,
        key: QueryEmbeddingKey,
        value: CachedQueryEmbedding,
        now: Instant,
        ttl: Duration,
    ) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.clock = state.clock.saturating_add(1);
        let clock = state.clock;
        if !state.entries.contains_key(&key) && state.entries.len() >= self.capacity {
            if let Some(evicted) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            {
                state.entries.remove(&evicted);
            }
        }
        state.entries.insert(
            key,
            CacheEntry {
                value,
                expires_at: now + ttl,
                last_used: clock,
            },
        );
    }
}

#[derive(Default)]
struct SemanticCounters {
    requested: AtomicU64,
    disabled: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    negative_cache_hits: AtomicU64,
    cache_bypasses: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
    deferrals: AtomicU64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct SemanticRuntimeSnapshot {
    pub requested: u64,
    pub disabled: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub negative_cache_hits: u64,
    pub cache_bypasses: u64,
    pub successes: u64,
    pub failures: u64,
    pub deferrals: u64,
}

#[derive(Clone)]
pub struct SemanticRuntime {
    cache: QueryEmbeddingCache,
    counters: Arc<SemanticCounters>,
}

impl Default for SemanticRuntime {
    fn default() -> Self {
        Self {
            cache: QueryEmbeddingCache::production(),
            counters: Arc::new(SemanticCounters::default()),
        }
    }
}

impl SemanticRuntime {
    pub fn snapshot(&self) -> SemanticRuntimeSnapshot {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        SemanticRuntimeSnapshot {
            requested: load(&self.counters.requested),
            disabled: load(&self.counters.disabled),
            cache_hits: load(&self.counters.cache_hits),
            cache_misses: load(&self.counters.cache_misses),
            negative_cache_hits: load(&self.counters.negative_cache_hits),
            cache_bypasses: load(&self.counters.cache_bypasses),
            successes: load(&self.counters.successes),
            failures: load(&self.counters.failures),
            deferrals: load(&self.counters.deferrals),
        }
    }

    pub fn record_requested(&self) {
        self.counters.requested.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_disabled(&self) {
        self.counters.disabled.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.counters.successes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.counters.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_deferral(&self) {
        self.counters.deferrals.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn query_embedding(
        &self,
        embedder: SharedEmbedder,
        query: &str,
        cache_enabled: bool,
    ) -> ApiResult<Vec<f32>> {
        if !cache_enabled {
            self.counters.cache_bypasses.fetch_add(1, Ordering::Relaxed);
            return one_query_embedding(embedder.as_ref(), query).await;
        }

        let key = QueryEmbeddingKey::new(embedder.model(), embedder.dimensions(), query);
        match self.cache.lookup(&key) {
            CacheLookup::Hit(vector) => {
                self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("simple.semantic.query_cache", "result" => "hit").increment(1);
                return Ok(vector);
            }
            CacheLookup::NegativeHit => {
                self.counters
                    .negative_cache_hits
                    .fetch_add(1, Ordering::Relaxed);
                metrics::counter!("simple.semantic.query_cache", "result" => "negative_hit")
                    .increment(1);
                return Err(negative_cache_error());
            }
            CacheLookup::Miss => {
                self.counters.cache_misses.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("simple.semantic.query_cache", "result" => "miss").increment(1);
            }
        }

        // Keep the provider call alive if the request's semantic deadline
        // expires. A later equivalent query can use the completed vector.
        let query = query.to_owned();
        let cache = self.cache.clone();
        let dimensions = embedder.dimensions();
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let result = one_query_embedding(embedder.as_ref(), &query).await;
            match &result {
                Ok(vector) if vector.len() == dimensions => {
                    cache.insert_vector(key, vector.clone());
                }
                Ok(_) | Err(_) => cache.insert_negative(key),
            }
            let _ = sender.send(result);
        });
        receiver.await.map_err(|_| {
            ApiError::Internal("semantic query embedding task ended unexpectedly".to_owned())
        })?
    }
}

async fn one_query_embedding(
    embedder: &dyn crate::embeddings::Embedder,
    query: &str,
) -> ApiResult<Vec<f32>> {
    let vectors = embedder.embed(&[query.to_owned()]).await?;
    let vector = vectors.into_iter().next().ok_or_else(|| {
        ApiError::Internal("embedding provider returned no query vector".to_owned())
    })?;
    if vector.len() != embedder.dimensions() {
        return Err(ApiError::Internal(
            "embedding provider returned an unexpected result shape".to_owned(),
        ));
    }
    Ok(vector)
}

fn negative_cache_error() -> ApiError {
    ApiError::public(
        http::StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
        "semantic query embedding is temporarily unavailable",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;

    use super::*;
    use crate::embeddings::Embedder;

    #[test]
    fn cache_key_normalizes_whitespace_and_binds_model_and_dimensions() {
        assert_eq!(
            QueryEmbeddingKey::new("model-a", 3, "  current \n plan  "),
            QueryEmbeddingKey::new("model-a", 3, "current plan")
        );
        assert_ne!(
            QueryEmbeddingKey::new("model-a", 3, "current plan"),
            QueryEmbeddingKey::new("model-b", 3, "current plan")
        );
        assert_ne!(
            QueryEmbeddingKey::new("model-a", 3, "current plan"),
            QueryEmbeddingKey::new("model-a", 4, "current plan")
        );
    }

    #[test]
    fn cache_evicts_the_least_recently_used_entry() {
        let cache = QueryEmbeddingCache::new(2, Duration::from_secs(60), Duration::from_secs(10));
        let a = QueryEmbeddingKey::new("model", 1, "a");
        let b = QueryEmbeddingKey::new("model", 1, "b");
        let c = QueryEmbeddingKey::new("model", 1, "c");
        cache.insert_vector(a.clone(), vec![1.0]);
        cache.insert_vector(b.clone(), vec![2.0]);
        assert!(matches!(cache.lookup(&a), CacheLookup::Hit(_)));
        cache.insert_vector(c.clone(), vec![3.0]);
        assert!(matches!(cache.lookup(&a), CacheLookup::Hit(_)));
        assert!(matches!(cache.lookup(&b), CacheLookup::Miss));
        assert!(matches!(cache.lookup(&c), CacheLookup::Hit(_)));
    }

    #[test]
    fn positive_and_negative_cache_entries_expire_on_their_own_ttls() {
        let cache = QueryEmbeddingCache::new(2, Duration::from_secs(7), Duration::from_secs(2));
        let now = Instant::now();
        let positive = QueryEmbeddingKey::new("model", 1, "positive");
        let negative = QueryEmbeddingKey::new("model", 1, "negative");
        cache.insert_at(
            positive.clone(),
            CachedQueryEmbedding::Vector(vec![1.0]),
            now,
            Duration::from_secs(7),
        );
        cache.insert_at(
            negative.clone(),
            CachedQueryEmbedding::Negative,
            now,
            Duration::from_secs(2),
        );
        assert!(matches!(
            cache.lookup_at(&positive, now + Duration::from_secs(6)),
            CacheLookup::Hit(_)
        ));
        assert!(matches!(
            cache.lookup_at(&negative, now + Duration::from_secs(2)),
            CacheLookup::Miss
        ));
        assert!(matches!(
            cache.lookup_at(&positive, now + Duration::from_secs(7)),
            CacheLookup::Miss
        ));
    }

    struct SlowEmbedder {
        calls: AtomicUsize,
        delay: Duration,
        fail: bool,
        model: String,
        received: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Embedder for SlowEmbedder {
        async fn embed(&self, input: &[String]) -> ApiResult<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.received.lock().unwrap().extend(input.iter().cloned());
            tokio::time::sleep(self.delay).await;
            if self.fail {
                Err(negative_cache_error())
            } else {
                Ok(vec![vec![1.0, 0.0, 0.0]])
            }
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &str {
            &self.model
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn is_degraded(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn timed_out_cached_embedding_finishes_asynchronously_and_warms_retry() {
        let embedder = Arc::new(SlowEmbedder {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(40),
            fail: false,
            model: "mock-v1".to_owned(),
            received: Mutex::new(Vec::new()),
        });
        let runtime = SemanticRuntime {
            cache: QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(1)),
            counters: Arc::new(SemanticCounters::default()),
        };
        let first = runtime.query_embedding(embedder.clone(), "current plan", true);
        assert!(
            tokio::time::timeout(Duration::from_millis(5), first)
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
        let warm = tokio::time::timeout(
            Duration::from_millis(5),
            runtime.query_embedding(embedder.clone(), " current   plan ", true),
        )
        .await
        .expect("the asynchronously warmed cache should be immediate")
        .unwrap();
        assert_eq!(warm, vec![1.0, 0.0, 0.0]);
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.snapshot().cache_misses, 1);
        assert_eq!(runtime.snapshot().cache_hits, 1);
    }

    #[tokio::test]
    async fn embedding_failures_are_negative_cached_for_the_retry_window() {
        let embedder = Arc::new(SlowEmbedder {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
            fail: true,
            model: "mock-v1".to_owned(),
            received: Mutex::new(Vec::new()),
        });
        let runtime = SemanticRuntime {
            cache: QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(60)),
            counters: Arc::new(SemanticCounters::default()),
        };
        assert!(
            runtime
                .query_embedding(embedder.clone(), "query", true)
                .await
                .is_err()
        );
        assert!(
            runtime
                .query_embedding(embedder.clone(), "query", true)
                .await
                .is_err()
        );
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.snapshot().negative_cache_hits, 1);
    }
}

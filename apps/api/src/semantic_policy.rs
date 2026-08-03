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
use tokio::sync::{Semaphore, watch};

use crate::{
    embeddings::SharedEmbedder,
    error::{ApiError, ApiResult},
};

const QUERY_CACHE_CAPACITY: usize = 4_096;
const QUERY_CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const QUERY_CACHE_NEGATIVE_TTL: Duration = Duration::from_secs(60);
const DEFAULT_ONLINE_QUERY_CONCURRENCY: usize = 8;

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
    index_unavailable: AtomicU64,
    readiness_errors: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    negative_cache_hits: AtomicU64,
    cache_bypasses: AtomicU64,
    dedupe_joins: AtomicU64,
    saturation_waits: AtomicU64,
    provider_timeouts: AtomicU64,
    post_deferral_completions: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
    deferrals: AtomicU64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct SemanticRuntimeSnapshot {
    pub requested: u64,
    pub disabled: u64,
    pub index_unavailable: u64,
    pub readiness_errors: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub negative_cache_hits: u64,
    pub cache_bypasses: u64,
    pub dedupe_joins: u64,
    pub saturation_waits: u64,
    pub provider_timeouts: u64,
    pub post_deferral_completions: u64,
    pub successes: u64,
    pub failures: u64,
    pub deferrals: u64,
}

type InflightReceiver = watch::Receiver<Option<CachedQueryEmbedding>>;
type InflightMap = Arc<Mutex<HashMap<QueryEmbeddingKey, InflightReceiver>>>;

#[derive(Clone)]
pub struct SemanticRuntime {
    cache: QueryEmbeddingCache,
    counters: Arc<SemanticCounters>,
    inflight: InflightMap,
    online_permits: Arc<Semaphore>,
}

impl Default for SemanticRuntime {
    fn default() -> Self {
        Self::new(DEFAULT_ONLINE_QUERY_CONCURRENCY)
    }
}

/// Removes the in-flight entry for a key on every leader exit path,
/// including panics, so a dead leader can never strand later queries.
struct InflightGuard {
    inflight: InflightMap,
    key: QueryEmbeddingKey,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.inflight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.key);
    }
}

impl SemanticRuntime {
    pub fn new(online_query_concurrency: usize) -> Self {
        Self {
            cache: QueryEmbeddingCache::production(),
            counters: Arc::new(SemanticCounters::default()),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            online_permits: Arc::new(Semaphore::new(online_query_concurrency.max(1))),
        }
    }

    #[cfg(test)]
    fn for_tests(cache: QueryEmbeddingCache, online_query_concurrency: usize) -> Self {
        Self {
            cache,
            counters: Arc::new(SemanticCounters::default()),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            online_permits: Arc::new(Semaphore::new(online_query_concurrency.max(1))),
        }
    }

    #[cfg(test)]
    fn inflight_len(&self) -> usize {
        self.inflight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    pub fn snapshot(&self) -> SemanticRuntimeSnapshot {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        SemanticRuntimeSnapshot {
            requested: load(&self.counters.requested),
            disabled: load(&self.counters.disabled),
            index_unavailable: load(&self.counters.index_unavailable),
            readiness_errors: load(&self.counters.readiness_errors),
            cache_hits: load(&self.counters.cache_hits),
            cache_misses: load(&self.counters.cache_misses),
            negative_cache_hits: load(&self.counters.negative_cache_hits),
            cache_bypasses: load(&self.counters.cache_bypasses),
            dedupe_joins: load(&self.counters.dedupe_joins),
            saturation_waits: load(&self.counters.saturation_waits),
            provider_timeouts: load(&self.counters.provider_timeouts),
            post_deferral_completions: load(&self.counters.post_deferral_completions),
            successes: load(&self.counters.successes),
            failures: load(&self.counters.failures),
            deferrals: load(&self.counters.deferrals),
        }
    }

    pub fn record_requested(&self) {
        self.counters.requested.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.requested").increment(1);
    }

    pub fn record_disabled(&self) {
        self.counters.disabled.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.unavailable", "reason" => "policy_disabled")
            .increment(1);
    }

    pub fn record_index_unavailable(&self) {
        self.counters
            .index_unavailable
            .fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.unavailable", "reason" => "index_unavailable")
            .increment(1);
    }

    pub fn record_readiness_error(&self) {
        self.counters
            .readiness_errors
            .fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.unavailable", "reason" => "dependency_error")
            .increment(1);
    }

    pub fn record_success(&self) {
        self.counters.successes.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.outcome", "result" => "success").increment(1);
    }

    pub fn record_failure(&self) {
        self.counters.failures.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.outcome", "result" => "failure").increment(1);
    }

    pub fn record_deferral(&self) {
        self.counters.deferrals.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.outcome", "result" => "deferred").increment(1);
    }

    fn record_dedupe_join(&self) {
        self.counters.dedupe_joins.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.query_dedupe_join").increment(1);
    }

    fn record_saturation_wait(&self, waited: Duration) {
        self.counters
            .saturation_waits
            .fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.saturation_wait").increment(1);
        metrics::histogram!("simple.semantic.saturation_wait_ms")
            .record(waited.as_secs_f64() * 1_000.0);
    }

    fn record_provider_timeout(&self) {
        self.counters
            .provider_timeouts
            .fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.provider_timeout").increment(1);
    }

    fn record_post_deferral_completion(&self) {
        self.counters
            .post_deferral_completions
            .fetch_add(1, Ordering::Relaxed);
        metrics::counter!("simple.semantic.post_response_completion").increment(1);
    }

    pub async fn query_embedding(
        &self,
        embedder: SharedEmbedder,
        query: &str,
        cache_enabled: bool,
        provider_timeout: Duration,
    ) -> ApiResult<Vec<f32>> {
        if !cache_enabled {
            self.counters.cache_bypasses.fetch_add(1, Ordering::Relaxed);
            let _permit = self.acquire_online_permit().await;
            return match tokio::time::timeout(
                provider_timeout,
                one_query_embedding(embedder.as_ref(), query),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    self.record_provider_timeout();
                    Err(provider_timeout_error())
                }
            };
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

        let receiver = self.join_or_spawn_inflight(key, embedder, query, provider_timeout);
        wait_for_inflight(receiver).await
    }

    /// Per-key single flight: the first miss spawns one leader provider call;
    /// concurrent equivalent misses subscribe to the same result.
    fn join_or_spawn_inflight(
        &self,
        key: QueryEmbeddingKey,
        embedder: SharedEmbedder,
        query: &str,
        provider_timeout: Duration,
    ) -> InflightReceiver {
        let receiver = {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(existing) = inflight.get(&key) {
                self.record_dedupe_join();
                return existing.clone();
            }
            let (sender, receiver) = watch::channel(None);
            inflight.insert(key.clone(), receiver.clone());
            self.spawn_inflight_leader(sender, key, embedder, query.to_owned(), provider_timeout);
            receiver
        };
        receiver
    }

    // Keeps the provider call alive if the request's semantic deadline
    // expires — a later equivalent query can use the completed vector — but
    // bounds it with the query-specific provider timeout so abandoned work
    // can never run for the shared provider client's full 60-second window.
    fn spawn_inflight_leader(
        &self,
        sender: watch::Sender<Option<CachedQueryEmbedding>>,
        key: QueryEmbeddingKey,
        embedder: SharedEmbedder,
        query: String,
        provider_timeout: Duration,
    ) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let guard = InflightGuard {
                inflight: runtime.inflight.clone(),
                key: key.clone(),
            };
            let _permit = runtime.acquire_online_permit().await;
            let dimensions = embedder.dimensions();
            let value = match tokio::time::timeout(
                provider_timeout,
                one_query_embedding(embedder.as_ref(), &query),
            )
            .await
            {
                Ok(Ok(vector)) if vector.len() == dimensions => {
                    runtime.cache.insert_vector(key.clone(), vector.clone());
                    CachedQueryEmbedding::Vector(vector)
                }
                Ok(_) => {
                    runtime.cache.insert_negative(key.clone());
                    CachedQueryEmbedding::Negative
                }
                Err(_) => {
                    runtime.record_provider_timeout();
                    runtime.cache.insert_negative(key.clone());
                    CachedQueryEmbedding::Negative
                }
            };
            // The cache insert above must precede in-flight removal so a
            // racing lookup either joins this flight or hits the cache.
            drop(guard);
            if sender.send(Some(value)).is_err() {
                runtime.record_post_deferral_completion();
            }
        });
    }

    /// Global cap on concurrent online query-embedding provider calls.
    async fn acquire_online_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        match self.online_permits.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(tokio::sync::TryAcquireError::Closed) => None,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                let started = Instant::now();
                let permit = self.online_permits.clone().acquire_owned().await.ok();
                self.record_saturation_wait(started.elapsed());
                permit
            }
        }
    }
}

async fn wait_for_inflight(mut receiver: InflightReceiver) -> ApiResult<Vec<f32>> {
    loop {
        let current = receiver.borrow_and_update().clone();
        if let Some(value) = current {
            return match value {
                CachedQueryEmbedding::Vector(vector) => Ok(vector),
                CachedQueryEmbedding::Negative => Err(negative_cache_error()),
            };
        }
        if receiver.changed().await.is_err() {
            return Err(ApiError::Internal(
                "semantic query embedding task ended unexpectedly".to_owned(),
            ));
        }
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

fn provider_timeout_error() -> ApiError {
    ApiError::public(
        http::StatusCode::SERVICE_UNAVAILABLE,
        "dependency_unavailable",
        "semantic query embedding timed out",
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
        let runtime = SemanticRuntime::for_tests(
            QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(1)),
            8,
        );
        let first =
            runtime.query_embedding(embedder.clone(), "current plan", true, Duration::from_secs(5));
        assert!(
            tokio::time::timeout(Duration::from_millis(5), first)
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
        let warm = tokio::time::timeout(
            Duration::from_millis(5),
            runtime.query_embedding(
                embedder.clone(),
                " current   plan ",
                true,
                Duration::from_secs(5),
            ),
        )
        .await
        .expect("the asynchronously warmed cache should be immediate")
        .unwrap();
        assert_eq!(warm, vec![1.0, 0.0, 0.0]);
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.snapshot().cache_misses, 1);
        assert_eq!(runtime.snapshot().cache_hits, 1);
        assert_eq!(runtime.snapshot().post_deferral_completions, 1);
        assert_eq!(runtime.inflight_len(), 0);
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
        let runtime = SemanticRuntime::for_tests(
            QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(60)),
            8,
        );
        assert!(
            runtime
                .query_embedding(embedder.clone(), "query", true, Duration::from_secs(5))
                .await
                .is_err()
        );
        assert!(
            runtime
                .query_embedding(embedder.clone(), "query", true, Duration::from_secs(5))
                .await
                .is_err()
        );
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);
        assert_eq!(runtime.snapshot().negative_cache_hits, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_identical_misses_share_one_provider_call() {
        let embedder = Arc::new(SlowEmbedder {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(40),
            fail: false,
            model: "mock-v1".to_owned(),
            received: Mutex::new(Vec::new()),
        });
        let runtime = SemanticRuntime::for_tests(
            QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(1)),
            8,
        );
        let timeout = Duration::from_secs(5);
        let (first, second, third, fourth) = tokio::join!(
            runtime.query_embedding(embedder.clone(), "current plan", true, timeout),
            runtime.query_embedding(embedder.clone(), "current plan", true, timeout),
            runtime.query_embedding(embedder.clone(), " current   plan ", true, timeout),
            runtime.query_embedding(embedder.clone(), "current plan", true, timeout),
        );
        for result in [first, second, third, fourth] {
            assert_eq!(result.unwrap(), vec![1.0, 0.0, 0.0]);
        }
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.cache_misses, 4);
        assert_eq!(snapshot.dedupe_joins, 3);
        assert_eq!(snapshot.cache_hits, 0);
        assert_eq!(runtime.inflight_len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn hung_provider_calls_expire_clean_up_and_negative_cache() {
        let embedder = Arc::new(SlowEmbedder {
            calls: AtomicUsize::new(0),
            delay: Duration::from_secs(3_600),
            fail: false,
            model: "mock-v1".to_owned(),
            received: Mutex::new(Vec::new()),
        });
        let runtime = SemanticRuntime::for_tests(
            QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(60)),
            8,
        );
        let error = runtime
            .query_embedding(embedder.clone(), "query", true, Duration::from_millis(50))
            .await
            .expect_err("a hung provider call must expire at the provider timeout");
        match error {
            ApiError::Public { code, .. } => assert_eq!(code, "dependency_unavailable"),
            other => panic!("unexpected error classification: {other:?}"),
        }
        assert_eq!(runtime.inflight_len(), 0);
        assert!(
            runtime
                .query_embedding(embedder.clone(), "query", true, Duration::from_millis(50))
                .await
                .is_err()
        );
        let snapshot = runtime.snapshot();
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);
        assert_eq!(snapshot.provider_timeouts, 1);
        assert_eq!(snapshot.negative_cache_hits, 1);
    }

    struct ConcurrencyProbeEmbedder {
        current: AtomicUsize,
        max: AtomicUsize,
        calls: AtomicUsize,
        delay: Duration,
    }

    #[async_trait]
    impl Embedder for ConcurrencyProbeEmbedder {
        async fn embed(&self, _input: &[String]) -> ApiResult<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let now = self.current.fetch_add(1, Ordering::Relaxed) + 1;
            self.max.fetch_max(now, Ordering::Relaxed);
            tokio::time::sleep(self.delay).await;
            self.current.fetch_sub(1, Ordering::Relaxed);
            Ok(vec![vec![1.0, 0.0, 0.0]])
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &str {
            "mock-v1"
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn is_degraded(&self) -> bool {
            false
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unique_misses_respect_the_online_concurrency_cap() {
        let embedder = Arc::new(ConcurrencyProbeEmbedder {
            current: AtomicUsize::new(0),
            max: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(25),
        });
        let runtime = SemanticRuntime::for_tests(
            QueryEmbeddingCache::new(16, Duration::from_secs(60), Duration::from_secs(1)),
            2,
        );
        let timeout = Duration::from_secs(5);
        let (a, b, c, d, e, f) = tokio::join!(
            runtime.query_embedding(embedder.clone(), "alpha", true, timeout),
            runtime.query_embedding(embedder.clone(), "bravo", true, timeout),
            runtime.query_embedding(embedder.clone(), "charlie", true, timeout),
            runtime.query_embedding(embedder.clone(), "delta", true, timeout),
            runtime.query_embedding(embedder.clone(), "echo", true, timeout),
            runtime.query_embedding(embedder.clone(), "foxtrot", true, timeout),
        );
        for result in [a, b, c, d, e, f] {
            assert_eq!(result.unwrap(), vec![1.0, 0.0, 0.0]);
        }
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 6);
        assert_eq!(embedder.max.load(Ordering::Relaxed), 2);
        assert_eq!(runtime.snapshot().saturation_waits, 4);
        assert_eq!(runtime.inflight_len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn uncached_query_embeddings_are_bounded_by_the_provider_timeout() {
        let embedder = Arc::new(SlowEmbedder {
            calls: AtomicUsize::new(0),
            delay: Duration::from_secs(3_600),
            fail: false,
            model: "mock-v1".to_owned(),
            received: Mutex::new(Vec::new()),
        });
        let runtime = SemanticRuntime::for_tests(
            QueryEmbeddingCache::new(8, Duration::from_secs(60), Duration::from_secs(1)),
            8,
        );
        let error = runtime
            .query_embedding(embedder.clone(), "query", false, Duration::from_millis(50))
            .await
            .expect_err("uncached embedding must expire at the provider timeout");
        match error {
            ApiError::Public { code, message, .. } => {
                assert_eq!(code, "dependency_unavailable");
                assert!(message.contains("timed out"));
            }
            other => panic!("unexpected error classification: {other:?}"),
        }
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.cache_bypasses, 1);
        assert_eq!(snapshot.provider_timeouts, 1);
    }
}

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{auth::AuthContext, db::set_context, error::ApiResult};

const ENTRY_CHANNEL_CAPACITY: usize = 4_096;
const ACTIVITY_CHANNEL_CAPACITY: usize = 4_096;
const MAX_PENDING_KEYS: usize = 5_000;
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct UsageTracker {
    entry_sender: Option<mpsc::Sender<EntryUsageEvent>>,
    activity_sender: Option<mpsc::Sender<ActivityEvent>>,
    activity_health: Arc<ProductActivityTrackerHealthState>,
}

#[derive(Clone, Copy)]
pub enum UsageOperation {
    Read,
    Search,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductActivityOperation {
    Open,
    Search,
    Read,
    BinaryFetch,
    BriefingList,
    BriefingRead,
    BriefingTopics,
    Write,
    Capture,
    Checkpoint,
    BinaryUpload,
    Delete,
    BriefingPublish,
    BriefingAction,
}

impl ProductActivityOperation {
    // Human documents are a curated view over ordinary workspace Markdown,
    // so they intentionally share the existing read/write telemetry families.
    // Keeping these aliases avoids a database activity-enum migration for the
    // direct-link slice while making the call sites explicit.
    pub const HUMAN_DOCUMENT_READ: Self = Self::Read;
    pub const HUMAN_DOCUMENT_PUBLISH: Self = Self::Write;

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Search => "search",
            Self::Read => "read",
            Self::BinaryFetch => "binary_fetch",
            Self::BriefingList => "briefing_list",
            Self::BriefingRead => "briefing_read",
            Self::BriefingTopics => "briefing_topics",
            Self::Write => "write",
            Self::Capture => "capture",
            Self::Checkpoint => "checkpoint",
            Self::BinaryUpload => "binary_upload",
            Self::Delete => "delete",
            Self::BriefingPublish => "briefing_publish",
            Self::BriefingAction => "briefing_action",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductActivityTrackerStatus {
    Disabled,
    Enabled,
    Degraded,
}

impl ProductActivityTrackerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
            Self::Degraded => "degraded",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductActivityTrackerHealth {
    pub status: ProductActivityTrackerStatus,
    pub queued_events: u64,
    pub dropped_events: u64,
    pub pending_events: u64,
    pub successful_flushes: u64,
    pub failed_flushes: u64,
    pub last_queued_at: Option<DateTime<Utc>>,
    pub last_dropped_at: Option<DateTime<Utc>>,
    pub last_successful_flush_at: Option<DateTime<Utc>>,
    pub last_failed_flush_at: Option<DateTime<Utc>>,
    pub data_through: Option<DateTime<Utc>>,
}

struct EntryUsageEvent {
    auth: AuthContext,
    entry_ids: Vec<Uuid>,
    operation: UsageOperation,
}

enum ActivityEvent {
    Product {
        auth: AuthContext,
        operation: ProductActivityOperation,
        bytes: i64,
        occurred_at: DateTime<Utc>,
    },
    Credential {
        auth: AuthContext,
        operation: &'static str,
        occurred_at: DateTime<Utc>,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PrincipalKey {
    user_id: Uuid,
    credential_id: Uuid,
}

impl PrincipalKey {
    fn from_auth(auth: &AuthContext) -> Self {
        Self {
            user_id: auth.user_id.0,
            credential_id: auth.credential_id.0,
        }
    }
}

struct EntryUsagePending {
    auth: AuthContext,
    entries: HashMap<Uuid, UsageDelta>,
}

struct ActivityPending {
    auth: AuthContext,
    product: HashMap<(DateTime<Utc>, ProductActivityOperation), ProductActivityDelta>,
    credential: Option<CredentialActivityDelta>,
}

#[derive(Default)]
struct UsageDelta {
    reads: i64,
    searches: i64,
}

struct ProductActivityDelta {
    operation_count: i64,
    byte_count: i64,
    first_recorded_at: DateTime<Utc>,
    last_recorded_at: DateTime<Utc>,
}

struct CredentialActivityDelta {
    request_count: i64,
    last_operation: &'static str,
    last_used_at: DateTime<Utc>,
}

#[derive(Default)]
struct ProductActivityTrackerTimestamps {
    last_queued_at: Option<DateTime<Utc>>,
    last_dropped_at: Option<DateTime<Utc>>,
    last_successful_flush_at: Option<DateTime<Utc>>,
    last_failed_flush_at: Option<DateTime<Utc>>,
    data_through: Option<DateTime<Utc>>,
}

struct ProductActivityTrackerHealthState {
    enabled: bool,
    queued_events: AtomicU64,
    dropped_events: AtomicU64,
    pending_events: AtomicU64,
    successful_flushes: AtomicU64,
    failed_flushes: AtomicU64,
    timestamps: Mutex<ProductActivityTrackerTimestamps>,
}

impl ProductActivityTrackerHealthState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            queued_events: AtomicU64::new(0),
            dropped_events: AtomicU64::new(0),
            pending_events: AtomicU64::new(0),
            successful_flushes: AtomicU64::new(0),
            failed_flushes: AtomicU64::new(0),
            timestamps: Mutex::new(ProductActivityTrackerTimestamps::default()),
        }
    }

    fn begin_enqueue(&self) {
        self.pending_events.fetch_add(1, Ordering::Relaxed);
    }

    fn queued(&self, occurred_at: DateTime<Utc>) {
        self.queued_events.fetch_add(1, Ordering::Relaxed);
        self.with_timestamps(|timestamps| timestamps.last_queued_at = Some(occurred_at));
    }

    fn dropped(&self, occurred_at: DateTime<Utc>) {
        self.subtract_pending(1);
        self.dropped_events.fetch_add(1, Ordering::Relaxed);
        self.with_timestamps(|timestamps| timestamps.last_dropped_at = Some(occurred_at));
    }

    fn flush_succeeded(
        &self,
        event_count: u64,
        flushed_at: DateTime<Utc>,
        data_through: DateTime<Utc>,
    ) {
        self.subtract_pending(event_count);
        self.successful_flushes.fetch_add(1, Ordering::Relaxed);
        self.with_timestamps(|timestamps| {
            timestamps.last_successful_flush_at = Some(flushed_at);
            timestamps.data_through = Some(
                timestamps
                    .data_through
                    .map_or(data_through, |current| current.max(data_through)),
            );
        });
    }

    fn flush_failed(&self, event_count: u64, flushed_at: DateTime<Utc>) {
        self.subtract_pending(event_count);
        self.failed_flushes.fetch_add(1, Ordering::Relaxed);
        self.with_timestamps(|timestamps| timestamps.last_failed_flush_at = Some(flushed_at));
    }

    fn snapshot(&self) -> ProductActivityTrackerHealth {
        let queued_events = self.queued_events.load(Ordering::Relaxed);
        let dropped_events = self.dropped_events.load(Ordering::Relaxed);
        let pending_events = self.pending_events.load(Ordering::Relaxed);
        let successful_flushes = self.successful_flushes.load(Ordering::Relaxed);
        let failed_flushes = self.failed_flushes.load(Ordering::Relaxed);
        let timestamps = self
            .timestamps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ProductActivityTrackerHealth {
            status: if !self.enabled {
                ProductActivityTrackerStatus::Disabled
            } else if dropped_events > 0 || failed_flushes > 0 {
                ProductActivityTrackerStatus::Degraded
            } else {
                ProductActivityTrackerStatus::Enabled
            },
            queued_events,
            dropped_events,
            pending_events,
            successful_flushes,
            failed_flushes,
            last_queued_at: timestamps.last_queued_at,
            last_dropped_at: timestamps.last_dropped_at,
            last_successful_flush_at: timestamps.last_successful_flush_at,
            last_failed_flush_at: timestamps.last_failed_flush_at,
            data_through: timestamps.data_through,
        }
    }

    fn subtract_pending(&self, event_count: u64) {
        let _ = self
            .pending_events
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(event_count))
            });
    }

    fn with_timestamps(&self, update: impl FnOnce(&mut ProductActivityTrackerTimestamps)) {
        let mut timestamps = self
            .timestamps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut timestamps);
    }
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self {
            entry_sender: None,
            activity_sender: None,
            activity_health: Arc::new(ProductActivityTrackerHealthState::new(false)),
        }
    }
}

impl UsageTracker {
    pub fn start(pool: PgPool) -> Self {
        let (entry_sender, entry_receiver) = mpsc::channel(ENTRY_CHANNEL_CAPACITY);
        let (activity_sender, activity_receiver) = mpsc::channel(ACTIVITY_CHANNEL_CAPACITY);
        let activity_health = Arc::new(ProductActivityTrackerHealthState::new(true));
        tokio::spawn(run_entry_usage(pool.clone(), entry_receiver));
        tokio::spawn(run_activity(
            pool,
            activity_receiver,
            Arc::clone(&activity_health),
        ));
        Self {
            entry_sender: Some(entry_sender),
            activity_sender: Some(activity_sender),
            activity_health,
        }
    }

    pub fn record(
        &self,
        auth: &AuthContext,
        entry_ids: impl IntoIterator<Item = Uuid>,
        operation: UsageOperation,
    ) {
        let Some(sender) = &self.entry_sender else {
            return;
        };
        let entry_ids = entry_ids
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if entry_ids.is_empty() {
            return;
        }
        if sender
            .try_send(EntryUsageEvent {
                auth: auth.clone(),
                entry_ids,
                operation,
            })
            .is_err()
        {
            metrics::counter!("simple.usage.events", "result" => "dropped").increment(1);
        } else {
            metrics::counter!("simple.usage.events", "result" => "queued").increment(1);
        }
    }

    pub fn record_product_activity(
        &self,
        auth: &AuthContext,
        operation: ProductActivityOperation,
        bytes: u64,
    ) {
        let Some(sender) = &self.activity_sender else {
            return;
        };
        let occurred_at = Utc::now();
        let event = ActivityEvent::Product {
            auth: auth.clone(),
            operation,
            bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
            occurred_at,
        };
        self.activity_health.begin_enqueue();
        if sender.try_send(event).is_err() {
            self.activity_health.dropped(occurred_at);
            metrics::counter!("product.activity.events", "result" => "dropped").increment(1);
        } else {
            self.activity_health.queued(occurred_at);
            metrics::counter!("product.activity.events", "result" => "queued").increment(1);
        }
    }

    pub fn record_credential_activity(&self, auth: &AuthContext, operation: &'static str) {
        let Some(sender) = &self.activity_sender else {
            return;
        };
        let occurred_at = Utc::now();
        let event = ActivityEvent::Credential {
            auth: auth.clone(),
            operation: credential_activity_label(operation),
            occurred_at,
        };
        self.activity_health.begin_enqueue();
        if sender.try_send(event).is_err() {
            self.activity_health.dropped(occurred_at);
            metrics::counter!("credential.activity.events", "result" => "dropped").increment(1);
        } else {
            self.activity_health.queued(occurred_at);
            metrics::counter!("credential.activity.events", "result" => "queued").increment(1);
        }
    }

    pub fn product_activity_health(&self) -> ProductActivityTrackerHealth {
        self.activity_health.snapshot()
    }
}

async fn run_entry_usage(pool: PgPool, mut receiver: mpsc::Receiver<EntryUsageEvent>) {
    let mut entry_pending = HashMap::<PrincipalKey, EntryUsagePending>::new();
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    flush_entry_usage(&pool, &mut entry_pending).await;
                    return;
                };
                aggregate_entry_usage(&mut entry_pending, event);
                if entry_pending
                    .values()
                    .map(|principal| principal.entries.len())
                    .sum::<usize>()
                    >= MAX_PENDING_KEYS
                {
                    flush_entry_usage(&pool, &mut entry_pending).await;
                }
            }
            _ = interval.tick() => {
                flush_entry_usage(&pool, &mut entry_pending).await
            },
        }
    }
}

async fn run_activity(
    pool: PgPool,
    mut receiver: mpsc::Receiver<ActivityEvent>,
    health: Arc<ProductActivityTrackerHealthState>,
) {
    let mut pending = HashMap::<PrincipalKey, ActivityPending>::new();
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    flush_activity(&pool, &mut pending, &health).await;
                    return;
                };
                aggregate_activity(&mut pending, event);
                if pending
                    .values()
                    .map(|principal| {
                        principal.product.len() + usize::from(principal.credential.is_some())
                    })
                    .sum::<usize>() >= MAX_PENDING_KEYS
                {
                    flush_activity(&pool, &mut pending, &health).await;
                }
            }
            _ = interval.tick() => {
                flush_activity(&pool, &mut pending, &health).await
            },
        }
    }
}

fn aggregate_entry_usage(
    entry_pending: &mut HashMap<PrincipalKey, EntryUsagePending>,
    event: EntryUsageEvent,
) {
    let EntryUsageEvent {
        auth,
        entry_ids,
        operation,
    } = event;
    let principal_key = PrincipalKey::from_auth(&auth);
    let principal = entry_pending
        .entry(principal_key)
        .or_insert_with(|| EntryUsagePending {
            auth: auth.clone(),
            entries: HashMap::new(),
        });
    principal.auth = auth;
    for entry_id in entry_ids.into_iter().collect::<HashSet<_>>() {
        let delta = principal.entries.entry(entry_id).or_default();
        match operation {
            UsageOperation::Read => delta.reads = delta.reads.saturating_add(1),
            UsageOperation::Search => delta.searches = delta.searches.saturating_add(1),
        }
    }
}

fn aggregate_activity(pending: &mut HashMap<PrincipalKey, ActivityPending>, event: ActivityEvent) {
    match event {
        ActivityEvent::Product {
            auth,
            operation,
            bytes,
            occurred_at,
        } => {
            let principal_key = PrincipalKey::from_auth(&auth);
            let principal = pending
                .entry(principal_key)
                .or_insert_with(|| ActivityPending {
                    auth: auth.clone(),
                    product: HashMap::new(),
                    credential: None,
                });
            principal.auth = auth;
            let bucket_start = utc_minute_bucket(occurred_at);
            let delta = principal
                .product
                .entry((bucket_start, operation))
                .or_insert_with(|| ProductActivityDelta {
                    operation_count: 0,
                    byte_count: 0,
                    first_recorded_at: occurred_at,
                    last_recorded_at: occurred_at,
                });
            delta.operation_count = delta.operation_count.saturating_add(1);
            delta.byte_count = delta.byte_count.saturating_add(bytes);
            delta.first_recorded_at = delta.first_recorded_at.min(occurred_at);
            delta.last_recorded_at = delta.last_recorded_at.max(occurred_at);
        }
        ActivityEvent::Credential {
            auth,
            operation,
            occurred_at,
        } => {
            let principal_key = PrincipalKey::from_auth(&auth);
            let principal = pending
                .entry(principal_key)
                .or_insert_with(|| ActivityPending {
                    auth: auth.clone(),
                    product: HashMap::new(),
                    credential: None,
                });
            principal.auth = auth;
            let delta = principal.credential.get_or_insert(CredentialActivityDelta {
                request_count: 0,
                last_operation: operation,
                last_used_at: occurred_at,
            });
            delta.request_count = delta.request_count.saturating_add(1);
            if occurred_at >= delta.last_used_at {
                delta.last_operation = operation;
                delta.last_used_at = occurred_at;
            }
        }
    }
}

fn utc_minute_bucket(instant: DateTime<Utc>) -> DateTime<Utc> {
    let timestamp = instant.timestamp().div_euclid(60) * 60;
    DateTime::from_timestamp(timestamp, 0).expect("a rounded UTC timestamp remains representable")
}

fn credential_activity_label(operation: &'static str) -> &'static str {
    match operation {
        "open" | "search" | "read" | "binary_fetch" | "briefing_list" | "briefing_read"
        | "briefing_topics" | "write" | "capture" | "checkpoint" | "binary_upload" | "delete"
        | "briefing_publish" | "briefing_action" | "dashboard" | "status" | "changes"
        | "credential_list" | "credential_create" | "credential_update" | "credential_delete" => {
            operation
        }
        _ => "control",
    }
}

async fn flush_activity(
    pool: &PgPool,
    pending: &mut HashMap<PrincipalKey, ActivityPending>,
    health: &ProductActivityTrackerHealthState,
) {
    for (principal_key, principal) in std::mem::take(pending) {
        flush_principal_activity(pool, principal_key, principal, health).await;
    }
}

async fn flush_entry_usage(pool: &PgPool, pending: &mut HashMap<PrincipalKey, EntryUsagePending>) {
    if pending.is_empty() {
        return;
    }
    for (principal_key, principal) in std::mem::take(pending) {
        flush_principal_entry_usage(pool, principal_key, principal).await;
    }
}

async fn flush_principal_entry_usage(
    pool: &PgPool,
    principal_key: PrincipalKey,
    principal: EntryUsagePending,
) {
    let EntryUsagePending { auth, entries } = principal;
    if entries.is_empty() {
        return;
    }
    let mut entry_ids = Vec::with_capacity(entries.len());
    let mut reads = Vec::with_capacity(entries.len());
    let mut searches = Vec::with_capacity(entries.len());
    for (entry_id, delta) in entries {
        entry_ids.push(entry_id);
        reads.push(delta.reads);
        searches.push(delta.searches);
    }
    let started = Instant::now();
    let result: ApiResult<()> = async {
        let mut tx = pool.begin().await?;
        set_context(&mut tx, &auth).await?;
        sqlx::query(
            r#"
            SELECT straylight_auth.write_entry_usage($1,$2,$3,$4,$5)
            "#,
        )
        .bind(principal_key.user_id)
        .bind(principal_key.credential_id)
        .bind(&entry_ids)
        .bind(&reads)
        .bind(&searches)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    let outcome = if result.is_ok() { "flushed" } else { "dropped" };
    metrics::counter!("simple.usage.flushes", "result" => outcome).increment(1);
    metrics::histogram!("simple.usage.flush_size", "result" => outcome)
        .record(entry_ids.len() as f64);
    metrics::histogram!("simple.usage.flush_duration_ms", "result" => outcome)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    if let Err(error) = result {
        tracing::warn!(?error, events = entry_ids.len(), "usage batch dropped");
    }
}

async fn flush_principal_activity(
    pool: &PgPool,
    principal_key: PrincipalKey,
    principal: ActivityPending,
    health: &ProductActivityTrackerHealthState,
) {
    let ActivityPending {
        auth,
        product,
        credential,
    } = principal;
    let product_key_count = product.len();
    let mut bucket_starts = Vec::with_capacity(product_key_count);
    let mut operations = Vec::with_capacity(product_key_count);
    let mut operation_counts = Vec::with_capacity(product_key_count);
    let mut byte_counts = Vec::with_capacity(product_key_count);
    let mut first_recorded_at = Vec::with_capacity(product_key_count);
    let mut last_recorded_at = Vec::with_capacity(product_key_count);
    let mut product_event_count = 0_u64;
    let mut data_through = None::<DateTime<Utc>>;
    for ((bucket_start, operation), delta) in product {
        bucket_starts.push(bucket_start);
        operations.push(operation.as_str());
        operation_counts.push(delta.operation_count);
        byte_counts.push(delta.byte_count);
        first_recorded_at.push(delta.first_recorded_at);
        last_recorded_at.push(delta.last_recorded_at);
        product_event_count =
            product_event_count.saturating_add(u64::try_from(delta.operation_count).unwrap_or(0));
        data_through = Some(data_through.map_or(delta.last_recorded_at, |current| {
            current.max(delta.last_recorded_at)
        }));
    }
    let credential_event_count = credential
        .as_ref()
        .and_then(|delta| u64::try_from(delta.request_count).ok())
        .unwrap_or(0);
    if let Some(delta) = &credential {
        data_through = Some(data_through.map_or(delta.last_used_at, |current| {
            current.max(delta.last_used_at)
        }));
    }
    let event_count = product_event_count.saturating_add(credential_event_count);
    let Some(data_through) = data_through else {
        return;
    };
    let started = Instant::now();
    let result: ApiResult<()> = async {
        let mut tx = pool.begin().await?;
        set_context(&mut tx, &auth).await?;
        if !bucket_starts.is_empty() {
            sqlx::query(
                r#"
                SELECT straylight_auth.write_product_activity(
                  $1,$2,$3,$4,$5,$6,$7,$8
                )
                "#,
            )
            .bind(principal_key.user_id)
            .bind(principal_key.credential_id)
            .bind(&bucket_starts)
            .bind(&operations)
            .bind(&operation_counts)
            .bind(&byte_counts)
            .bind(&first_recorded_at)
            .bind(&last_recorded_at)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(delta) = &credential {
            sqlx::query(
                r#"
                SELECT straylight_auth.write_credential_activity($1,$2,$3,$4,$5)
                "#,
            )
            .bind(principal_key.user_id)
            .bind(principal_key.credential_id)
            .bind(delta.last_operation)
            .bind(delta.last_used_at)
            .bind(delta.request_count)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    let outcome = if result.is_ok() { "flushed" } else { "dropped" };
    if product_event_count > 0 {
        metrics::counter!("product.activity.flushes", "result" => outcome).increment(1);
        metrics::histogram!("product.activity.flush_size", "result" => outcome)
            .record(product_key_count as f64);
        metrics::histogram!("product.activity.flush_duration_ms", "result" => outcome)
            .record(started.elapsed().as_secs_f64() * 1_000.0);
    }
    if credential_event_count > 0 {
        metrics::counter!("credential.activity.flushes", "result" => outcome).increment(1);
        metrics::histogram!("credential.activity.flush_size", "result" => outcome).record(1.0);
        metrics::histogram!("credential.activity.flush_duration_ms", "result" => outcome)
            .record(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let flushed_at = Utc::now();
    match result {
        Ok(_) => health.flush_succeeded(event_count, flushed_at, data_through),
        Err(error) => {
            health.flush_failed(event_count, flushed_at);
            tracing::warn!(?error, events = event_count, "activity batch dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CredentialId, UserId};

    fn auth_fixture(user_id: Uuid, credential_id: Uuid) -> AuthContext {
        AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: ["open", "query", "read", "status"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            scope_refs: vec!["scope:root".to_owned()],
            read_only: true,
        }
    }

    #[test]
    fn aggregate_combines_repeated_hits() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let auth = auth_fixture(user_id, credential_id);
        let entry_id = Uuid::now_v7();
        let mut entry_pending = HashMap::new();
        aggregate_entry_usage(
            &mut entry_pending,
            EntryUsageEvent {
                auth: auth.clone(),
                entry_ids: vec![entry_id, entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate_entry_usage(
            &mut entry_pending,
            EntryUsageEvent {
                auth: auth.clone(),
                entry_ids: vec![entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate_entry_usage(
            &mut entry_pending,
            EntryUsageEvent {
                auth: auth.clone(),
                entry_ids: vec![entry_id],
                operation: UsageOperation::Search,
            },
        );
        let delta = entry_pending
            .get(&PrincipalKey::from_auth(&auth))
            .unwrap()
            .entries
            .get(&entry_id)
            .unwrap();
        assert_eq!(delta.reads, 2);
        assert_eq!(delta.searches, 1);
    }

    #[test]
    fn product_activity_uses_utc_minutes_and_saturating_totals() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let auth = auth_fixture(user_id, credential_id);
        let first = "2026-08-02T23:59:40Z".parse().unwrap();
        let second = "2026-08-02T23:59:58Z".parse().unwrap();
        let mut pending = HashMap::new();
        for (bytes, occurred_at) in [(41, first), (1, second)] {
            aggregate_activity(
                &mut pending,
                ActivityEvent::Product {
                    auth: auth.clone(),
                    operation: ProductActivityOperation::Read,
                    bytes,
                    occurred_at,
                },
            );
        }
        let key = (
            "2026-08-02T23:59:00Z".parse().unwrap(),
            ProductActivityOperation::Read,
        );
        let delta = pending
            .get(&PrincipalKey::from_auth(&auth))
            .unwrap()
            .product
            .get(&key)
            .unwrap();
        assert_eq!(delta.operation_count, 2);
        assert_eq!(delta.byte_count, 42);
        assert_eq!(delta.first_recorded_at, first);
        assert_eq!(delta.last_recorded_at, second);
    }

    #[test]
    fn minute_buckets_preserve_fractional_offset_local_midnight() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let auth = auth_fixture(user_id, credential_id);
        let before_kathmandu_midnight = "2026-08-02T18:14:59Z".parse().unwrap();
        let kathmandu_midnight = "2026-08-02T18:15:00Z".parse().unwrap();
        let mut pending = HashMap::new();
        for occurred_at in [before_kathmandu_midnight, kathmandu_midnight] {
            aggregate_activity(
                &mut pending,
                ActivityEvent::Product {
                    auth: auth.clone(),
                    operation: ProductActivityOperation::Read,
                    bytes: 1,
                    occurred_at,
                },
            );
        }

        let product = &pending
            .get(&PrincipalKey::from_auth(&auth))
            .unwrap()
            .product;
        assert!(product.contains_key(&(
            "2026-08-02T18:14:00Z".parse().unwrap(),
            ProductActivityOperation::Read,
        )));
        assert!(product.contains_key(&(kathmandu_midnight, ProductActivityOperation::Read,)));
    }

    #[test]
    fn credential_activity_is_allowlisted_and_keeps_the_latest_touch() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let auth = auth_fixture(user_id, credential_id);
        let first = "2026-08-02T23:59:40Z".parse().unwrap();
        let second = "2026-08-02T23:59:58Z".parse().unwrap();
        let mut pending = HashMap::new();
        for (operation, occurred_at) in [("dashboard", first), ("not-allowed", second)] {
            aggregate_activity(
                &mut pending,
                ActivityEvent::Credential {
                    auth: auth.clone(),
                    operation: credential_activity_label(operation),
                    occurred_at,
                },
            );
        }
        let delta = pending
            .get(&PrincipalKey::from_auth(&auth))
            .unwrap()
            .credential
            .as_ref()
            .unwrap();
        assert_eq!(delta.request_count, 2);
        assert_eq!(delta.last_operation, "control");
        assert_eq!(delta.last_used_at, second);
    }

    #[test]
    fn product_queue_saturation_cannot_consume_entry_queue_capacity() {
        let (entry_sender, mut entry_receiver) = mpsc::channel(1);
        let (activity_sender, _activity_receiver) = mpsc::channel(1);
        let activity_health = Arc::new(ProductActivityTrackerHealthState::new(true));
        let tracker = UsageTracker {
            entry_sender: Some(entry_sender),
            activity_sender: Some(activity_sender.clone()),
            activity_health,
        };
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let auth = auth_fixture(user_id, credential_id);
        activity_sender
            .try_send(ActivityEvent::Credential {
                auth: auth.clone(),
                operation: "control",
                occurred_at: Utc::now(),
            })
            .unwrap();

        tracker.record_product_activity(&auth, ProductActivityOperation::Read, 1);
        tracker.record(&auth, [Uuid::now_v7()], UsageOperation::Read);

        assert!(entry_receiver.try_recv().is_ok());
        assert_eq!(tracker.product_activity_health().dropped_events, 1);
    }

    #[test]
    fn tracker_health_reports_disabled_success_and_degradation() {
        let disabled = UsageTracker::default().product_activity_health();
        assert_eq!(disabled.status, ProductActivityTrackerStatus::Disabled);

        let state = ProductActivityTrackerHealthState::new(true);
        let queued_at = "2026-08-02T23:59:40Z".parse().unwrap();
        let flushed_at = "2026-08-02T23:59:41Z".parse().unwrap();
        state.begin_enqueue();
        state.queued(queued_at);
        state.flush_succeeded(1, flushed_at, queued_at);
        let healthy = state.snapshot();
        assert_eq!(healthy.status, ProductActivityTrackerStatus::Enabled);
        assert_eq!(healthy.pending_events, 0);
        assert_eq!(healthy.last_successful_flush_at, Some(flushed_at));
        assert_eq!(healthy.data_through, Some(queued_at));

        state.begin_enqueue();
        state.queued(queued_at);
        state.flush_failed(1, flushed_at);
        let degraded = state.snapshot();
        assert_eq!(degraded.status, ProductActivityTrackerStatus::Degraded);
        assert_eq!(degraded.failed_flushes, 1);
        assert_eq!(degraded.last_failed_flush_at, Some(flushed_at));
    }
}

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
    user_id: Uuid,
    entry_ids: Vec<Uuid>,
    operation: UsageOperation,
}

enum ActivityEvent {
    Product {
        user_id: Uuid,
        credential_id: Uuid,
        operation: ProductActivityOperation,
        bytes: i64,
        occurred_at: DateTime<Utc>,
    },
    Credential {
        user_id: Uuid,
        credential_id: Uuid,
        operation: &'static str,
        occurred_at: DateTime<Utc>,
    },
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
    pub fn start(pool: Option<PgPool>) -> Self {
        let Some(pool) = pool else {
            return Self::default();
        };
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
        user_id: Uuid,
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
                user_id,
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
        user_id: Uuid,
        credential_id: Uuid,
        operation: ProductActivityOperation,
        bytes: u64,
    ) {
        let Some(sender) = &self.activity_sender else {
            return;
        };
        let occurred_at = Utc::now();
        let event = ActivityEvent::Product {
            user_id,
            credential_id,
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

    pub fn record_credential_activity(
        &self,
        user_id: Uuid,
        credential_id: Uuid,
        operation: &'static str,
    ) {
        let Some(sender) = &self.activity_sender else {
            return;
        };
        let occurred_at = Utc::now();
        let event = ActivityEvent::Credential {
            user_id,
            credential_id,
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
    let mut entry_pending = HashMap::<(Uuid, Uuid), UsageDelta>::new();
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
                if entry_pending.len() >= MAX_PENDING_KEYS {
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
    let mut product_pending = HashMap::<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >::new();
    let mut credential_pending = HashMap::<(Uuid, Uuid), CredentialActivityDelta>::new();
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    flush_activity(
                        &pool,
                        &mut product_pending,
                        &mut credential_pending,
                        &health,
                    ).await;
                    return;
                };
                aggregate_activity(&mut product_pending, &mut credential_pending, event);
                if product_pending.len().saturating_add(credential_pending.len())
                    >= MAX_PENDING_KEYS
                {
                    flush_activity(
                        &pool,
                        &mut product_pending,
                        &mut credential_pending,
                        &health,
                    ).await;
                }
            }
            _ = interval.tick() => {
                flush_activity(
                    &pool,
                    &mut product_pending,
                    &mut credential_pending,
                    &health,
                ).await
            },
        }
    }
}

fn aggregate_entry_usage(
    entry_pending: &mut HashMap<(Uuid, Uuid), UsageDelta>,
    event: EntryUsageEvent,
) {
    let EntryUsageEvent {
        user_id,
        entry_ids,
        operation,
    } = event;
    for entry_id in entry_ids.into_iter().collect::<HashSet<_>>() {
        let delta = entry_pending.entry((user_id, entry_id)).or_default();
        match operation {
            UsageOperation::Read => delta.reads = delta.reads.saturating_add(1),
            UsageOperation::Search => delta.searches = delta.searches.saturating_add(1),
        }
    }
}

fn aggregate_activity(
    product_pending: &mut HashMap<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >,
    credential_pending: &mut HashMap<(Uuid, Uuid), CredentialActivityDelta>,
    event: ActivityEvent,
) {
    match event {
        ActivityEvent::Product {
            user_id,
            credential_id,
            operation,
            bytes,
            occurred_at,
        } => {
            let bucket_start = utc_minute_bucket(occurred_at);
            let key = (user_id, credential_id, bucket_start, operation);
            let delta = product_pending
                .entry(key)
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
            user_id,
            credential_id,
            operation,
            occurred_at,
        } => {
            let delta = credential_pending
                .entry((user_id, credential_id))
                .or_insert(CredentialActivityDelta {
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
    product_pending: &mut HashMap<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >,
    credential_pending: &mut HashMap<(Uuid, Uuid), CredentialActivityDelta>,
    health: &ProductActivityTrackerHealthState,
) {
    let ((), ()) = tokio::join!(
        flush_product_activity(pool, product_pending, health),
        flush_credential_activity(pool, credential_pending, health),
    );
}

async fn flush_entry_usage(pool: &PgPool, pending: &mut HashMap<(Uuid, Uuid), UsageDelta>) {
    if pending.is_empty() {
        return;
    }
    let batch = std::mem::take(pending);
    let mut user_ids = Vec::with_capacity(batch.len());
    let mut entry_ids = Vec::with_capacity(batch.len());
    let mut reads = Vec::with_capacity(batch.len());
    let mut searches = Vec::with_capacity(batch.len());
    for ((user_id, entry_id), delta) in batch {
        user_ids.push(user_id);
        entry_ids.push(entry_id);
        reads.push(delta.reads);
        searches.push(delta.searches);
    }
    let started = Instant::now();
    let result = sqlx::query(
        r#"
        INSERT INTO straylight.entry_usage (
          user_id,entry_id,read_count,search_count,first_used_at,last_used_at,
          last_read_at,last_search_at
        )
        SELECT
          item.user_id,item.entry_id,item.reads,item.searches,
          clock_timestamp(),clock_timestamp(),
          CASE WHEN item.reads>0 THEN clock_timestamp() END,
          CASE WHEN item.searches>0 THEN clock_timestamp() END
        FROM unnest($1::uuid[],$2::uuid[],$3::bigint[],$4::bigint[])
          AS item(user_id,entry_id,reads,searches)
        ON CONFLICT (user_id,entry_id) DO UPDATE SET
          read_count=straylight.entry_usage.read_count+EXCLUDED.read_count,
          search_count=straylight.entry_usage.search_count+EXCLUDED.search_count,
          last_used_at=clock_timestamp(),
          last_read_at=CASE WHEN EXCLUDED.read_count>0
            THEN clock_timestamp() ELSE straylight.entry_usage.last_read_at END,
          last_search_at=CASE WHEN EXCLUDED.search_count>0
            THEN clock_timestamp() ELSE straylight.entry_usage.last_search_at END
        "#,
    )
    .bind(&user_ids)
    .bind(&entry_ids)
    .bind(&reads)
    .bind(&searches)
    .execute(pool)
    .await;
    let outcome = if result.is_ok() { "flushed" } else { "dropped" };
    metrics::counter!("simple.usage.flushes", "result" => outcome).increment(1);
    metrics::histogram!("simple.usage.flush_size", "result" => outcome)
        .record(user_ids.len() as f64);
    metrics::histogram!("simple.usage.flush_duration_ms", "result" => outcome)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    if let Err(error) = result {
        tracing::warn!(?error, events = user_ids.len(), "usage batch dropped");
    }
}

async fn flush_product_activity(
    pool: &PgPool,
    pending: &mut HashMap<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >,
    health: &ProductActivityTrackerHealthState,
) {
    if pending.is_empty() {
        return;
    }
    let batch = std::mem::take(pending);
    let mut user_ids = Vec::with_capacity(batch.len());
    let mut credential_ids = Vec::with_capacity(batch.len());
    let mut bucket_starts = Vec::with_capacity(batch.len());
    let mut operations = Vec::with_capacity(batch.len());
    let mut operation_counts = Vec::with_capacity(batch.len());
    let mut byte_counts = Vec::with_capacity(batch.len());
    let mut first_recorded_at = Vec::with_capacity(batch.len());
    let mut last_recorded_at = Vec::with_capacity(batch.len());
    let mut event_count = 0_u64;
    let mut data_through = DateTime::<Utc>::MIN_UTC;
    for ((user_id, credential_id, bucket_start, operation), delta) in batch {
        user_ids.push(user_id);
        credential_ids.push(credential_id);
        bucket_starts.push(bucket_start);
        operations.push(operation.as_str());
        operation_counts.push(delta.operation_count);
        byte_counts.push(delta.byte_count);
        first_recorded_at.push(delta.first_recorded_at);
        last_recorded_at.push(delta.last_recorded_at);
        event_count = event_count.saturating_add(u64::try_from(delta.operation_count).unwrap_or(0));
        data_through = data_through.max(delta.last_recorded_at);
    }
    let started = Instant::now();
    let result = sqlx::query(
        r#"
        INSERT INTO straylight.product_activity_minutely (
          user_id,credential_id,bucket_start,operation,
          operation_count,byte_count,first_recorded_at,last_recorded_at
        )
        SELECT
          item.user_id,item.credential_id,item.bucket_start,item.operation,
          item.operation_count,item.byte_count,
          item.first_recorded_at,item.last_recorded_at
        FROM unnest(
          $1::uuid[],$2::uuid[],$3::timestamptz[],$4::text[],
          $5::bigint[],$6::bigint[],$7::timestamptz[],$8::timestamptz[]
        ) AS item(
          user_id,credential_id,bucket_start,operation,
          operation_count,byte_count,first_recorded_at,last_recorded_at
        )
        ON CONFLICT (user_id,credential_id,bucket_start,operation) DO UPDATE SET
          operation_count=
            straylight.product_activity_minutely.operation_count
              + EXCLUDED.operation_count,
          byte_count=
            straylight.product_activity_minutely.byte_count
              + EXCLUDED.byte_count,
          first_recorded_at=least(
            straylight.product_activity_minutely.first_recorded_at,
            EXCLUDED.first_recorded_at
          ),
          last_recorded_at=greatest(
            straylight.product_activity_minutely.last_recorded_at,
            EXCLUDED.last_recorded_at
          )
        "#,
    )
    .bind(&user_ids)
    .bind(&credential_ids)
    .bind(&bucket_starts)
    .bind(&operations)
    .bind(&operation_counts)
    .bind(&byte_counts)
    .bind(&first_recorded_at)
    .bind(&last_recorded_at)
    .execute(pool)
    .await;
    let outcome = if result.is_ok() { "flushed" } else { "dropped" };
    metrics::counter!("product.activity.flushes", "result" => outcome).increment(1);
    metrics::histogram!("product.activity.flush_size", "result" => outcome)
        .record(user_ids.len() as f64);
    metrics::histogram!("product.activity.flush_duration_ms", "result" => outcome)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    let flushed_at = Utc::now();
    match result {
        Ok(_) => health.flush_succeeded(event_count, flushed_at, data_through),
        Err(error) => {
            health.flush_failed(event_count, flushed_at);
            tracing::warn!(
                ?error,
                events = event_count,
                "product activity batch dropped"
            );
        }
    }
}

async fn flush_credential_activity(
    pool: &PgPool,
    pending: &mut HashMap<(Uuid, Uuid), CredentialActivityDelta>,
    health: &ProductActivityTrackerHealthState,
) {
    if pending.is_empty() {
        return;
    }
    let batch = std::mem::take(pending);
    let mut user_ids = Vec::with_capacity(batch.len());
    let mut credential_ids = Vec::with_capacity(batch.len());
    let mut operations = Vec::with_capacity(batch.len());
    let mut request_counts = Vec::with_capacity(batch.len());
    let mut last_used_at = Vec::with_capacity(batch.len());
    let mut event_count = 0_u64;
    let mut data_through = DateTime::<Utc>::MIN_UTC;
    for ((user_id, credential_id), delta) in batch {
        user_ids.push(user_id);
        credential_ids.push(credential_id);
        operations.push(delta.last_operation);
        request_counts.push(delta.request_count);
        last_used_at.push(delta.last_used_at);
        event_count = event_count.saturating_add(u64::try_from(delta.request_count).unwrap_or(0));
        data_through = data_through.max(delta.last_used_at);
    }
    let started = Instant::now();
    let result = sqlx::query(
        r#"
        INSERT INTO straylight.credential_activity (
          user_id,credential_id,last_operation,last_used_at,request_count
        )
        SELECT
          item.user_id,item.credential_id,item.last_operation,
          item.last_used_at,item.request_count
        FROM unnest(
          $1::uuid[],$2::uuid[],$3::text[],$4::timestamptz[],$5::bigint[]
        ) AS item(
          user_id,credential_id,last_operation,last_used_at,request_count
        )
        ON CONFLICT (user_id,credential_id) DO UPDATE SET
          last_operation=CASE
            WHEN EXCLUDED.last_used_at >= straylight.credential_activity.last_used_at
              THEN EXCLUDED.last_operation
            ELSE straylight.credential_activity.last_operation
          END,
          last_used_at=greatest(
            straylight.credential_activity.last_used_at,
            EXCLUDED.last_used_at
          ),
          request_count=
            straylight.credential_activity.request_count + EXCLUDED.request_count
        "#,
    )
    .bind(&user_ids)
    .bind(&credential_ids)
    .bind(&operations)
    .bind(&last_used_at)
    .bind(&request_counts)
    .execute(pool)
    .await;
    let outcome = if result.is_ok() { "flushed" } else { "dropped" };
    metrics::counter!("credential.activity.flushes", "result" => outcome).increment(1);
    metrics::histogram!("credential.activity.flush_size", "result" => outcome)
        .record(user_ids.len() as f64);
    metrics::histogram!("credential.activity.flush_duration_ms", "result" => outcome)
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    let flushed_at = Utc::now();
    match result {
        Ok(_) => health.flush_succeeded(event_count, flushed_at, data_through),
        Err(error) => {
            health.flush_failed(event_count, flushed_at);
            tracing::warn!(
                ?error,
                events = event_count,
                "credential activity batch dropped"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_combines_repeated_hits() {
        let user_id = Uuid::now_v7();
        let entry_id = Uuid::now_v7();
        let mut entry_pending = HashMap::new();
        aggregate_entry_usage(
            &mut entry_pending,
            EntryUsageEvent {
                user_id,
                entry_ids: vec![entry_id, entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate_entry_usage(
            &mut entry_pending,
            EntryUsageEvent {
                user_id,
                entry_ids: vec![entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate_entry_usage(
            &mut entry_pending,
            EntryUsageEvent {
                user_id,
                entry_ids: vec![entry_id],
                operation: UsageOperation::Search,
            },
        );
        let delta = entry_pending.get(&(user_id, entry_id)).unwrap();
        assert_eq!(delta.reads, 2);
        assert_eq!(delta.searches, 1);
    }

    #[test]
    fn product_activity_uses_utc_minutes_and_saturating_totals() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let first = "2026-08-02T23:59:40Z".parse().unwrap();
        let second = "2026-08-02T23:59:58Z".parse().unwrap();
        let mut product_pending = HashMap::new();
        let mut credential_pending = HashMap::new();
        for (bytes, occurred_at) in [(41, first), (1, second)] {
            aggregate_activity(
                &mut product_pending,
                &mut credential_pending,
                ActivityEvent::Product {
                    user_id,
                    credential_id,
                    operation: ProductActivityOperation::Read,
                    bytes,
                    occurred_at,
                },
            );
        }
        let key = (
            user_id,
            credential_id,
            "2026-08-02T23:59:00Z".parse().unwrap(),
            ProductActivityOperation::Read,
        );
        let delta = product_pending.get(&key).unwrap();
        assert_eq!(delta.operation_count, 2);
        assert_eq!(delta.byte_count, 42);
        assert_eq!(delta.first_recorded_at, first);
        assert_eq!(delta.last_recorded_at, second);
    }

    #[test]
    fn minute_buckets_preserve_fractional_offset_local_midnight() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let before_kathmandu_midnight = "2026-08-02T18:14:59Z".parse().unwrap();
        let kathmandu_midnight = "2026-08-02T18:15:00Z".parse().unwrap();
        let mut product_pending = HashMap::new();
        let mut credential_pending = HashMap::new();
        for occurred_at in [before_kathmandu_midnight, kathmandu_midnight] {
            aggregate_activity(
                &mut product_pending,
                &mut credential_pending,
                ActivityEvent::Product {
                    user_id,
                    credential_id,
                    operation: ProductActivityOperation::Read,
                    bytes: 1,
                    occurred_at,
                },
            );
        }

        assert!(product_pending.contains_key(&(
            user_id,
            credential_id,
            "2026-08-02T18:14:00Z".parse().unwrap(),
            ProductActivityOperation::Read,
        )));
        assert!(product_pending.contains_key(&(
            user_id,
            credential_id,
            kathmandu_midnight,
            ProductActivityOperation::Read,
        )));
    }

    #[test]
    fn credential_activity_is_allowlisted_and_keeps_the_latest_touch() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let first = "2026-08-02T23:59:40Z".parse().unwrap();
        let second = "2026-08-02T23:59:58Z".parse().unwrap();
        let mut product_pending = HashMap::new();
        let mut credential_pending = HashMap::new();
        for (operation, occurred_at) in [("dashboard", first), ("not-allowed", second)] {
            aggregate_activity(
                &mut product_pending,
                &mut credential_pending,
                ActivityEvent::Credential {
                    user_id,
                    credential_id,
                    operation: credential_activity_label(operation),
                    occurred_at,
                },
            );
        }
        let delta = credential_pending.get(&(user_id, credential_id)).unwrap();
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
        activity_sender
            .try_send(ActivityEvent::Credential {
                user_id,
                credential_id,
                operation: "control",
                occurred_at: Utc::now(),
            })
            .unwrap();

        tracker.record_product_activity(user_id, credential_id, ProductActivityOperation::Read, 1);
        tracker.record(user_id, [Uuid::now_v7()], UsageOperation::Read);

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

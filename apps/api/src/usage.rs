use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 4_096;
const MAX_PENDING_KEYS: usize = 5_000;
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Default)]
pub struct UsageTracker {
    sender: Option<mpsc::Sender<UsageEvent>>,
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
    Write,
    Capture,
    Checkpoint,
    BinaryUpload,
    Delete,
}

impl ProductActivityOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Search => "search",
            Self::Read => "read",
            Self::BinaryFetch => "binary_fetch",
            Self::Write => "write",
            Self::Capture => "capture",
            Self::Checkpoint => "checkpoint",
            Self::BinaryUpload => "binary_upload",
            Self::Delete => "delete",
        }
    }
}

enum UsageEvent {
    Entry {
        user_id: Uuid,
        entry_ids: Vec<Uuid>,
        operation: UsageOperation,
    },
    Product {
        user_id: Uuid,
        credential_id: Uuid,
        operation: ProductActivityOperation,
        bytes: i64,
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

impl UsageTracker {
    pub fn start(pool: Option<PgPool>) -> Self {
        let Some(pool) = pool else {
            return Self::default();
        };
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        tokio::spawn(run(pool, receiver));
        Self {
            sender: Some(sender),
        }
    }

    pub fn record(
        &self,
        user_id: Uuid,
        entry_ids: impl IntoIterator<Item = Uuid>,
        operation: UsageOperation,
    ) {
        let Some(sender) = &self.sender else {
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
            .try_send(UsageEvent::Entry {
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
        let Some(sender) = &self.sender else {
            return;
        };
        let event = UsageEvent::Product {
            user_id,
            credential_id,
            operation,
            bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
            occurred_at: Utc::now(),
        };
        if sender.try_send(event).is_err() {
            metrics::counter!("product.activity.events", "result" => "dropped").increment(1);
        } else {
            metrics::counter!("product.activity.events", "result" => "queued").increment(1);
        }
    }
}

async fn run(pool: PgPool, mut receiver: mpsc::Receiver<UsageEvent>) {
    let mut entry_pending = HashMap::<(Uuid, Uuid), UsageDelta>::new();
    let mut product_pending = HashMap::<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >::new();
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    flush(&pool, &mut entry_pending, &mut product_pending).await;
                    return;
                };
                aggregate(&mut entry_pending, &mut product_pending, event);
                if entry_pending.len().saturating_add(product_pending.len()) >= MAX_PENDING_KEYS {
                    flush(&pool, &mut entry_pending, &mut product_pending).await;
                }
            }
            _ = interval.tick() => {
                flush(&pool, &mut entry_pending, &mut product_pending).await
            },
        }
    }
}

fn aggregate(
    entry_pending: &mut HashMap<(Uuid, Uuid), UsageDelta>,
    product_pending: &mut HashMap<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >,
    event: UsageEvent,
) {
    match event {
        UsageEvent::Entry {
            user_id,
            entry_ids,
            operation,
        } => {
            for entry_id in entry_ids.into_iter().collect::<HashSet<_>>() {
                let delta = entry_pending.entry((user_id, entry_id)).or_default();
                match operation {
                    UsageOperation::Read => delta.reads = delta.reads.saturating_add(1),
                    UsageOperation::Search => {
                        delta.searches = delta.searches.saturating_add(1);
                    }
                }
            }
        }
        UsageEvent::Product {
            user_id,
            credential_id,
            operation,
            bytes,
            occurred_at,
        } => {
            let bucket_start = utc_hour_bucket(occurred_at);
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
    }
}

fn utc_hour_bucket(instant: DateTime<Utc>) -> DateTime<Utc> {
    let timestamp = instant.timestamp().div_euclid(3_600) * 3_600;
    DateTime::from_timestamp(timestamp, 0).expect("a rounded UTC timestamp remains representable")
}

async fn flush(
    pool: &PgPool,
    entry_pending: &mut HashMap<(Uuid, Uuid), UsageDelta>,
    product_pending: &mut HashMap<
        (Uuid, Uuid, DateTime<Utc>, ProductActivityOperation),
        ProductActivityDelta,
    >,
) {
    let ((), ()) = tokio::join!(
        flush_entry_usage(pool, entry_pending),
        flush_product_activity(pool, product_pending),
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
    for ((user_id, credential_id, bucket_start, operation), delta) in batch {
        user_ids.push(user_id);
        credential_ids.push(credential_id);
        bucket_starts.push(bucket_start);
        operations.push(operation.as_str());
        operation_counts.push(delta.operation_count);
        byte_counts.push(delta.byte_count);
        first_recorded_at.push(delta.first_recorded_at);
        last_recorded_at.push(delta.last_recorded_at);
    }
    let started = Instant::now();
    let result = sqlx::query(
        r#"
        INSERT INTO straylight.product_activity_hourly (
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
            straylight.product_activity_hourly.operation_count
              + EXCLUDED.operation_count,
          byte_count=
            straylight.product_activity_hourly.byte_count
              + EXCLUDED.byte_count,
          first_recorded_at=least(
            straylight.product_activity_hourly.first_recorded_at,
            EXCLUDED.first_recorded_at
          ),
          last_recorded_at=greatest(
            straylight.product_activity_hourly.last_recorded_at,
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
    if let Err(error) = result {
        tracing::warn!(
            ?error,
            events = user_ids.len(),
            "product activity batch dropped"
        );
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
        let mut product_pending = HashMap::new();
        aggregate(
            &mut entry_pending,
            &mut product_pending,
            UsageEvent::Entry {
                user_id,
                entry_ids: vec![entry_id, entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate(
            &mut entry_pending,
            &mut product_pending,
            UsageEvent::Entry {
                user_id,
                entry_ids: vec![entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate(
            &mut entry_pending,
            &mut product_pending,
            UsageEvent::Entry {
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
    fn product_activity_uses_utc_hours_and_saturating_totals() {
        let user_id = Uuid::now_v7();
        let credential_id = Uuid::now_v7();
        let first = "2026-08-02T23:59:40Z".parse().unwrap();
        let second = "2026-08-02T23:59:58Z".parse().unwrap();
        let mut entry_pending = HashMap::new();
        let mut product_pending = HashMap::new();
        for (bytes, occurred_at) in [(41, first), (1, second)] {
            aggregate(
                &mut entry_pending,
                &mut product_pending,
                UsageEvent::Product {
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
            "2026-08-02T23:00:00Z".parse().unwrap(),
            ProductActivityOperation::Read,
        );
        let delta = product_pending.get(&key).unwrap();
        assert_eq!(delta.operation_count, 2);
        assert_eq!(delta.byte_count, 42);
        assert_eq!(delta.first_recorded_at, first);
        assert_eq!(delta.last_recorded_at, second);
    }
}

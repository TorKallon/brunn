use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

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

struct UsageEvent {
    user_id: Uuid,
    entry_ids: Vec<Uuid>,
    operation: UsageOperation,
}

#[derive(Default)]
struct UsageDelta {
    reads: i64,
    searches: i64,
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
            .try_send(UsageEvent {
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
}

async fn run(pool: PgPool, mut receiver: mpsc::Receiver<UsageEvent>) {
    let mut pending = HashMap::<(Uuid, Uuid), UsageDelta>::new();
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let Some(event) = event else {
                    flush(&pool, &mut pending).await;
                    return;
                };
                aggregate(&mut pending, event);
                if pending.len() >= MAX_PENDING_KEYS {
                    flush(&pool, &mut pending).await;
                }
            }
            _ = interval.tick() => flush(&pool, &mut pending).await,
        }
    }
}

fn aggregate(pending: &mut HashMap<(Uuid, Uuid), UsageDelta>, event: UsageEvent) {
    for entry_id in event.entry_ids.into_iter().collect::<HashSet<_>>() {
        let delta = pending.entry((event.user_id, entry_id)).or_default();
        match event.operation {
            UsageOperation::Read => delta.reads = delta.reads.saturating_add(1),
            UsageOperation::Search => {
                delta.searches = delta.searches.saturating_add(1);
            }
        }
    }
}

async fn flush(pool: &PgPool, pending: &mut HashMap<(Uuid, Uuid), UsageDelta>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_combines_repeated_hits() {
        let user_id = Uuid::now_v7();
        let entry_id = Uuid::now_v7();
        let mut pending = HashMap::new();
        aggregate(
            &mut pending,
            UsageEvent {
                user_id,
                entry_ids: vec![entry_id, entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate(
            &mut pending,
            UsageEvent {
                user_id,
                entry_ids: vec![entry_id],
                operation: UsageOperation::Read,
            },
        );
        aggregate(
            &mut pending,
            UsageEvent {
                user_id,
                entry_ids: vec![entry_id],
                operation: UsageOperation::Search,
            },
        );
        let delta = pending.get(&(user_id, entry_id)).unwrap();
        assert_eq!(delta.reads, 2);
        assert_eq!(delta.searches, 1);
    }
}

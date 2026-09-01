use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const WINDOW: Duration = Duration::from_secs(60);
pub const SNAPSHOT_SCHEMA: &str = "brunn-foreground-latency@v1";

#[derive(Clone, Copy, Debug)]
pub enum ForegroundOperation {
    Open,
    Search,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct OperationLatencySnapshot {
    pub samples: usize,
    pub p95_ms: Option<f64>,
    pub newest_sample_age_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ForegroundLatencySnapshot {
    pub schema: String,
    pub generated_at_unix_ms: u64,
    pub window_seconds: u64,
    pub open: OperationLatencySnapshot,
    pub search: OperationLatencySnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ForegroundGuardDecision {
    pub allow_backfill: bool,
    pub reason: String,
    pub open_p95_ms: Option<f64>,
    pub search_p95_ms: Option<f64>,
    pub open_limit_ms: f64,
    pub search_limit_ms: f64,
}

#[derive(Clone, Debug)]
struct LatencySample {
    observed_at: Instant,
    milliseconds: f64,
}

#[derive(Default)]
struct ForegroundLatencyState {
    open: VecDeque<LatencySample>,
    search: VecDeque<LatencySample>,
}

#[derive(Clone, Default)]
pub struct ForegroundLatencyTracker {
    inner: Arc<Mutex<ForegroundLatencyState>>,
}

impl ForegroundLatencyTracker {
    pub fn record(&self, operation: ForegroundOperation, milliseconds: f64) {
        if !milliseconds.is_finite() || milliseconds < 0.0 {
            return;
        }
        self.record_at(operation, milliseconds, Instant::now());
    }

    fn record_at(&self, operation: ForegroundOperation, milliseconds: f64, now: Instant) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        prune(&mut state.open, now);
        prune(&mut state.search, now);
        let samples = match operation {
            ForegroundOperation::Open => &mut state.open,
            ForegroundOperation::Search => &mut state.search,
        };
        samples.push_back(LatencySample {
            observed_at: now,
            milliseconds,
        });
    }

    pub fn snapshot(&self) -> ForegroundLatencySnapshot {
        self.snapshot_at(Instant::now(), SystemTime::now())
    }

    fn snapshot_at(
        &self,
        monotonic_now: Instant,
        wall_now: SystemTime,
    ) -> ForegroundLatencySnapshot {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        prune(&mut state.open, monotonic_now);
        prune(&mut state.search, monotonic_now);
        ForegroundLatencySnapshot {
            schema: SNAPSHOT_SCHEMA.to_owned(),
            generated_at_unix_ms: wall_now
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            window_seconds: WINDOW.as_secs(),
            open: operation_snapshot(&state.open, monotonic_now),
            search: operation_snapshot(&state.search, monotonic_now),
        }
    }
}

pub fn evaluate_guard(
    snapshot: &ForegroundLatencySnapshot,
    open_limit_ms: f64,
    search_limit_ms: f64,
) -> ForegroundGuardDecision {
    let open_exceeded = snapshot
        .open
        .p95_ms
        .is_some_and(|value| value > open_limit_ms);
    let search_exceeded = snapshot
        .search
        .p95_ms
        .is_some_and(|value| value > search_limit_ms);
    let (allow_backfill, reason) = if open_exceeded && search_exceeded {
        (false, "open_and_search_p95_exceeded")
    } else if open_exceeded {
        (false, "open_p95_exceeded")
    } else if search_exceeded {
        (false, "search_p95_exceeded")
    } else if snapshot.open.samples == 0 && snapshot.search.samples == 0 {
        (true, "no_foreground_samples")
    } else {
        (true, "within_limits")
    };
    ForegroundGuardDecision {
        allow_backfill,
        reason: reason.to_owned(),
        open_p95_ms: snapshot.open.p95_ms,
        search_p95_ms: snapshot.search.p95_ms,
        open_limit_ms,
        search_limit_ms,
    }
}

fn prune(samples: &mut VecDeque<LatencySample>, now: Instant) {
    while samples
        .front()
        .is_some_and(|sample| now.saturating_duration_since(sample.observed_at) > WINDOW)
    {
        samples.pop_front();
    }
}

fn operation_snapshot(samples: &VecDeque<LatencySample>, now: Instant) -> OperationLatencySnapshot {
    let mut values = samples
        .iter()
        .map(|sample| sample.milliseconds)
        .collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    OperationLatencySnapshot {
        samples: values.len(),
        p95_ms: percentile(&values, 0.95),
        newest_sample_age_ms: samples.back().map(|sample| {
            now.saturating_duration_since(sample.observed_at)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX)
        }),
    }
}

fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let rank = ((values.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    Some(values[rank])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_snapshot_expires_old_samples_and_uses_nearest_rank_p95() {
        let tracker = ForegroundLatencyTracker::default();
        let now = Instant::now();
        tracker.record_at(
            ForegroundOperation::Open,
            9_999.0,
            now - Duration::from_secs(61),
        );
        for value in 1..=20 {
            tracker.record_at(ForegroundOperation::Open, value as f64, now);
        }
        tracker.record_at(ForegroundOperation::Search, 77.0, now);
        let snapshot = tracker.snapshot_at(now, UNIX_EPOCH + Duration::from_secs(123));
        assert_eq!(snapshot.generated_at_unix_ms, 123_000);
        assert_eq!(snapshot.open.samples, 20);
        assert_eq!(snapshot.open.p95_ms, Some(19.0));
        assert_eq!(snapshot.search.p95_ms, Some(77.0));
    }

    #[test]
    fn guard_pauses_on_either_foreground_operation() {
        let snapshot = ForegroundLatencySnapshot {
            schema: SNAPSHOT_SCHEMA.to_owned(),
            generated_at_unix_ms: 1,
            window_seconds: 60,
            open: OperationLatencySnapshot {
                samples: 10,
                p95_ms: Some(121.0),
                newest_sample_age_ms: Some(0),
            },
            search: OperationLatencySnapshot {
                samples: 10,
                p95_ms: Some(100.0),
                newest_sample_age_ms: Some(0),
            },
        };
        let decision = evaluate_guard(&snapshot, 120.0, 107.0);
        assert!(!decision.allow_backfill);
        assert_eq!(decision.reason, "open_p95_exceeded");
    }

    #[test]
    fn no_samples_do_not_deadlock_initial_backfill() {
        let snapshot = ForegroundLatencyTracker::default().snapshot();
        let decision = evaluate_guard(&snapshot, 120.0, 107.0);
        assert!(decision.allow_backfill);
        assert_eq!(decision.reason, "no_foreground_samples");
    }
}

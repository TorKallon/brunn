use chrono::Utc;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

use crate::{
    account_worker,
    db::AppState,
    error::{ApiError, ApiResult},
    messaging_service, notification_service, simple_worker, task_guard, telemetry, todoist_sync,
};

const ACCOUNT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);
const TODOIST_QUEUE_SCAN_INTERVAL: Duration = Duration::from_secs(5);
const BACKGROUND_WORK_PAUSE: Duration = Duration::from_millis(250);

pub async fn run(state: AppState) -> ApiResult<()> {
    if state.admin_pool.is_none() {
        return Err(ApiError::configuration(
            "DATABASE_URL_ADMIN is required by the background worker",
        ));
    }
    let notification_provider = if state.config.apns_delivery_enabled {
        let provider =
            notification_service::configured_apns_provider(&state)?.ok_or_else(|| {
                ApiError::configuration(
                    "BRUNN_APNS_DELIVERY_ENABLED requires complete APNs provider credentials",
                )
            })?;
        tracing::info!("APNs notification delivery enabled");
        Some(provider)
    } else {
        tracing::info!("APNs notification delivery disabled");
        None
    };
    tracing::info!("Brunn background worker started");
    // A PID is commonly reused as 1 across overlapping container revisions.
    // A boot-unique suffix is part of the durable lease fence, so a stale pull
    // can never finalize after a replacement worker reclaims the row.
    let worker_id = boot_unique_worker_id();
    let mut next_account_maintenance = Instant::now();
    let mut next_task_guard = Instant::now();
    let mut next_todoist_sync = Instant::now();

    loop {
        let cycle_started = Instant::now();
        let mut cycle_failed = false;
        let now = Instant::now();
        let mut did_work = false;
        if next_task_guard <= now {
            next_task_guard = now + task_guard::TASK_GUARD_SCHEDULER_INTERVAL;
            did_work |= run_task_guard(&state, &mut cycle_failed).await;
        }
        if next_todoist_sync <= now {
            let todoist_did_work = run_todoist_sync(&state, &worker_id, &mut cycle_failed).await;
            next_todoist_sync = if todoist_did_work {
                Instant::now()
            } else {
                now + TODOIST_QUEUE_SCAN_INTERVAL
            };
            did_work |= todoist_did_work;
        }
        if state.config.messaging_enabled {
            did_work |= run_messaging_reply_by(&state, &mut cycle_failed).await;
        }
        did_work |=
            run_notification_delivery(&state, notification_provider.as_ref(), &mut cycle_failed)
                .await;
        did_work |= run_simple_workspace_job(&state, &mut cycle_failed).await;
        let now = Instant::now();

        if next_account_maintenance <= now {
            next_account_maintenance = now + ACCOUNT_MAINTENANCE_INTERVAL;
            did_work |= run_account_maintenance(&state, &worker_id, &mut cycle_failed).await;
        }

        let cycle_result = if did_work { "busy" } else { "idle" };
        metrics::counter!("worker.cycles", "result" => cycle_result).increment(1);
        metrics::gauge!("worker.busy").set(if did_work { 1.0 } else { 0.0 });
        metrics::histogram!("worker.cycle.duration_ms", "result" => cycle_result)
            .record(telemetry::elapsed_ms(cycle_started));
        if cycle_failed {
            tokio::time::sleep(Duration::from_secs(1)).await;
        } else if did_work {
            tokio::time::sleep(BACKGROUND_WORK_PAUSE).await;
        } else if !did_work {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

async fn run_messaging_reply_by(state: &AppState, cycle_failed: &mut bool) -> bool {
    match messaging_service::process_due_reply_by(state, Utc::now()).await {
        Ok(did_work) => did_work,
        Err(_error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "messaging_reply_by").increment(1);
            tracing::warn!("agent messaging reply deadline cycle failed");
            *cycle_failed = true;
            false
        }
    }
}

fn boot_unique_worker_id() -> String {
    format!("worker:{}:{}", std::process::id(), Uuid::now_v7())
}

async fn run_todoist_sync(state: &AppState, worker_id: &str, cycle_failed: &mut bool) -> bool {
    match todoist_sync::process_next(state, worker_id).await {
        Ok(did_work) => did_work,
        Err(_error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "todoist_sync").increment(1);
            // Todoist apply/configuration errors can contain upstream task
            // identifiers in their internal diagnostic details. The durable
            // sync state carries a bounded code; never mirror error content to
            // process logs at this boundary.
            tracing::warn!("Todoist sync cycle failed");
            *cycle_failed = true;
            false
        }
    }
}

async fn run_task_guard(state: &AppState, cycle_failed: &mut bool) -> bool {
    match task_guard::run_once(state, None).await {
        Ok(report) => report.events.iter().any(|event| event.inserted),
        Err(error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "task_guard").increment(1);
            tracing::warn!(?error, "task guard scheduler cycle failed");
            *cycle_failed = true;
            false
        }
    }
}

async fn run_notification_delivery(
    state: &AppState,
    provider: Option<&Arc<dyn notification_service::ApnsProvider>>,
    cycle_failed: &mut bool,
) -> bool {
    let result = match provider {
        Some(provider) => {
            notification_service::process_next_with_provider(state, Arc::clone(provider)).await
        }
        None => notification_service::suppress_queued_deliveries(state).await,
    };
    match result {
        Ok(did_work) => did_work,
        Err(error) => {
            *cycle_failed = true;
            tracing::error!(error = ?error, "notification delivery failed");
            false
        }
    }
}

async fn run_simple_workspace_job(state: &AppState, cycle_failed: &mut bool) -> bool {
    match simple_worker::process_next(state).await {
        Ok(value) => value,
        Err(error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "simple_workspace").increment(1);
            tracing::warn!(?error, "simple workspace queue cycle failed");
            *cycle_failed = true;
            false
        }
    }
}

async fn run_account_maintenance(
    state: &AppState,
    worker_id: &str,
    cycle_failed: &mut bool,
) -> bool {
    let mut did_work = match account_worker::process_account_export(state, worker_id).await {
        Ok(value) => value,
        Err(error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "account_export").increment(1);
            tracing::warn!(?error, "account export queue cycle failed");
            *cycle_failed = true;
            false
        }
    };
    did_work |= match account_worker::process_account_deletion(state, worker_id).await {
        Ok(value) => value,
        Err(error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "account_deletion").increment(1);
            tracing::warn!(?error, "account deletion queue cycle failed");
            *cycle_failed = true;
            false
        }
    };
    did_work
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_lease_identity_is_unique_across_boots_even_when_pid_matches() {
        let first = boot_unique_worker_id();
        let second = boot_unique_worker_id();
        assert_ne!(first, second);
        assert!(first.starts_with(&format!("worker:{}:", std::process::id())));
        assert!(first.len() <= 200);
    }
}

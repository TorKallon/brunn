use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    account_worker,
    db::AppState,
    error::{ApiError, ApiResult},
    notification_service, simple_worker, telemetry, upload_service,
};

const ACCOUNT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);
const UPLOAD_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);
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
                    "STRAYLIGHT_APNS_DELIVERY_ENABLED requires complete APNs provider credentials",
                )
            })?;
        tracing::info!("APNs notification delivery enabled");
        Some(provider)
    } else {
        tracing::info!("APNs notification delivery disabled");
        None
    };
    tracing::info!("Straylight background worker started");
    let worker_id = format!("worker:{}", std::process::id());
    let mut next_account_maintenance = Instant::now();
    let mut next_upload_maintenance = Instant::now();

    loop {
        let cycle_started = Instant::now();
        let mut cycle_failed = false;
        let mut did_work =
            run_notification_delivery(&state, notification_provider.as_ref(), &mut cycle_failed)
                .await;
        did_work |= run_simple_workspace_job(&state, &mut cycle_failed).await;
        let now = Instant::now();

        if state.config.legacy_api_enabled && next_account_maintenance <= now {
            next_account_maintenance = now + ACCOUNT_MAINTENANCE_INTERVAL;
            did_work |= run_account_maintenance(&state, &worker_id, &mut cycle_failed).await;
        }
        if next_upload_maintenance <= now {
            next_upload_maintenance = now + UPLOAD_MAINTENANCE_INTERVAL;
            did_work |= run_upload_maintenance(&state, &mut cycle_failed).await;
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

async fn run_upload_maintenance(state: &AppState, cycle_failed: &mut bool) -> bool {
    let mut did_work = match upload_service::expire_one(state).await {
        Ok(value) => value,
        Err(error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "asset_upload_expiry").increment(1);
            tracing::warn!(?error, "asset upload expiry cycle failed");
            *cycle_failed = true;
            false
        }
    };
    did_work |= match upload_service::expire_one_stage(state).await {
        Ok(value) => value,
        Err(error) => {
            metrics::counter!("worker.cycle.errors", "stage" => "asset_stage_expiry").increment(1);
            tracing::warn!(?error, "asset stage expiry cycle failed");
            *cycle_failed = true;
            false
        }
    };
    did_work
}

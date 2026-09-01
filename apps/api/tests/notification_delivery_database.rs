use std::{collections::VecDeque, sync::Arc};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::Mutex;
use uuid::Uuid;

use brunn::notification_service::{
    ApnsAccepted, ApnsFailure, ApnsProvider, ApnsRequest, process_next_on_pool,
    suppress_queued_deliveries_on_pool,
};

const APP_ID: &str = "com.rourkem.brunn";

struct FakeProvider {
    outcomes: Mutex<VecDeque<Result<ApnsAccepted, ApnsFailure>>>,
    requests: Mutex<Vec<ApnsRequest>>,
    blocked_until: Mutex<Option<DateTime<Utc>>>,
}

impl FakeProvider {
    fn new(outcomes: Vec<Result<ApnsAccepted, ApnsFailure>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            requests: Mutex::new(Vec::new()),
            blocked_until: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ApnsProvider for FakeProvider {
    async fn send(&self, request: ApnsRequest) -> Result<ApnsAccepted, ApnsFailure> {
        self.requests.lock().await.push(request);
        let outcome = self
            .outcomes
            .lock()
            .await
            .pop_front()
            .expect("fake APNs outcome");
        if let Err(failure) = &outcome
            && failure.provider_blocked
        {
            *self.blocked_until.lock().await = Some(Utc::now() + chrono::Duration::minutes(5));
        }
        outcome
    }

    async fn blocked_until(&self) -> Option<DateTime<Utc>> {
        *self.blocked_until.lock().await
    }
}

async fn connect_test_pool() -> Option<PgPool> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping notification delivery test");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply Brunn migrations");
    Some(pool)
}

async fn insert_principal(pool: &PgPool) -> (Uuid, Uuid) {
    let user_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    sqlx::query("INSERT INTO brunn.users (id,external_ref,display_name) VALUES ($1,$2,$3)")
        .bind(user_id)
        .bind(format!("notification-delivery-test:{user_id}"))
        .bind("Notification delivery test")
        .execute(pool)
        .await
        .expect("insert delivery test user");
    sqlx::query(
        r#"
        INSERT INTO brunn.api_credentials (
          id,user_id,label,token_hash,capabilities
        ) VALUES ($1,$2,'Notification delivery test',$3,$4)
        "#,
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(format!("notification-delivery-token-{credential_id}"))
    .bind(vec!["read", "notification:publish", "notification:manage"])
    .execute(pool)
    .await
    .expect("insert delivery test credential");
    (user_id, credential_id)
}

fn encrypt_token(
    key: &[u8; 32],
    user_id: Uuid,
    client_installation_id: Uuid,
    token: &str,
) -> (Vec<u8>, Vec<u8>) {
    let aad =
        format!("brunn.apns-token.v1|{user_id}|{client_installation_id}|development|{APP_ID}");
    let nonce = [client_installation_id.as_bytes()[0]; 12];
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES key");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: token.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .expect("encrypt delivery-test token");
    (ciphertext, nonce.to_vec())
}

async fn insert_installation(
    pool: &PgPool,
    key: &[u8; 32],
    user_id: Uuid,
    credential_id: Uuid,
    token_seed: u8,
) -> (Uuid, Uuid, String) {
    let installation_id = Uuid::now_v7();
    let client_installation_id = Uuid::now_v7();
    let token = hex::encode([token_seed; 32]);
    let (ciphertext, nonce) = encrypt_token(key, user_id, client_installation_id, &token);
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    sqlx::query(
        r#"
        INSERT INTO brunn.notification_installations (
          id,user_id,client_installation_id,registered_by_credential_id,
          platform,environment,app_id,token_ciphertext,token_nonce,
          token_hash,preview
        ) VALUES (
          $1,$2,$3,$4,'ios','development',$5,$6,$7,$8,'generic'
        )
        "#,
    )
    .bind(installation_id)
    .bind(user_id)
    .bind(client_installation_id)
    .bind(credential_id)
    .bind(APP_ID)
    .bind(ciphertext)
    .bind(nonce)
    .bind(token_hash)
    .execute(pool)
    .await
    .expect("insert encrypted delivery-test installation");
    (installation_id, client_installation_id, token)
}

async fn insert_delivery(
    pool: &PgPool,
    user_id: Uuid,
    credential_id: Uuid,
    installation_id: Uuid,
    kind: &str,
    event: &str,
    occurred_offset: &str,
    expires_offset: &str,
) -> (Uuid, Uuid) {
    let notification_id = Uuid::now_v7();
    let delivery_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO brunn.notifications (
          id,user_id,producer_credential_id,event_key,request_hash,
          correlation_id,kind,importance,title,body,target,occurred_at,expires_at
        ) VALUES (
          $1,$2,$3,$4,$5,$6,$7,'important',$8,$9,$10,
          clock_timestamp()+$11::interval,clock_timestamp()+$12::interval
        )
        "#,
    )
    .bind(notification_id)
    .bind(user_id)
    .bind(credential_id)
    .bind(format!("delivery-state:{event}:{notification_id}"))
    .bind(hex::encode(Sha256::digest(notification_id.as_bytes())))
    .bind(format!("delivery-state:{event}"))
    .bind(kind)
    .bind(format!("Private {event} title"))
    .bind(format!("Alert text for {event}."))
    .bind(json!({"type": "notification"}))
    .bind(occurred_offset)
    .bind(expires_offset)
    .execute(pool)
    .await
    .expect("insert delivery-test notification");
    sqlx::query(
        r#"
        INSERT INTO brunn.notification_deliveries (
          id,user_id,notification_id,installation_id
        ) VALUES ($1,$2,$3,$4)
        "#,
    )
    .bind(delivery_id)
    .bind(user_id)
    .bind(notification_id)
    .bind(installation_id)
    .execute(pool)
    .await
    .expect("insert delivery-test outbox row");
    (notification_id, delivery_id)
}

fn accepted() -> Result<ApnsAccepted, ApnsFailure> {
    Ok(ApnsAccepted {
        provider_request_id: Some(Uuid::now_v7().to_string()),
        status: 200,
    })
}

fn failure(
    code: &str,
    retryable: bool,
    provider_blocked: bool,
    invalidate_token: bool,
    retry_after_seconds: Option<i64>,
) -> Result<ApnsAccepted, ApnsFailure> {
    Err(ApnsFailure {
        code: code.to_owned(),
        status: Some(if provider_blocked { 403 } else { 400 }),
        provider_request_id: None,
        retryable,
        provider_blocked,
        invalidate_token,
        retry_after_seconds,
    })
}

#[tokio::test]
async fn notification_delivery_state_machine_preserves_transport_truth_and_preview_policy() {
    let Some(pool) = connect_test_pool().await else {
        return;
    };
    // This binary may be run after the schema/RLS fixture against the same
    // disposable database. Retire any rows that fixture intentionally leaves
    // queued so this worker-state test owns the due queue deterministically.
    sqlx::query(
        r#"
        UPDATE brunn.notification_deliveries
        SET state='expired',failed_at=clock_timestamp(),
            last_error_code='test_fixture_retired',lease_expires_at=NULL
        WHERE state IN ('queued','running')
        "#,
    )
    .execute(&pool)
    .await
    .expect("retire earlier disposable-database fixtures");
    let key = [23_u8; 32];
    let encoded_key = STANDARD.encode(key);
    let (user_id, credential_id) = insert_principal(&pool).await;
    let provider = Arc::new(FakeProvider::new(vec![
        accepted(),
        failure("TooManyRequests", true, false, false, Some(90)),
        failure("BadDeviceToken", false, false, true, None),
        accepted(),
        failure("InvalidProviderToken", true, true, false, None),
    ]));
    let transport: Arc<dyn ApnsProvider> = provider.clone();

    let (accepted_installation, _, accepted_token) =
        insert_installation(&pool, &key, user_id, credential_id, 1).await;
    let (accepted_notification, accepted_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        accepted_installation,
        "operational",
        "accepted",
        "-1 minute",
        "1 day",
    )
    .await;
    assert!(
        process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("accept one delivery")
    );
    let accepted_row = sqlx::query(
        "SELECT state,attempt_count,accepted_at FROM brunn.notification_deliveries WHERE id=$1",
    )
    .bind(accepted_delivery)
    .fetch_one(&pool)
    .await
    .expect("read accepted delivery");
    assert_eq!(accepted_row.get::<String, _>("state"), "accepted_by_apns");
    assert_eq!(accepted_row.get::<i32, _>("attempt_count"), 1);
    assert!(
        accepted_row
            .try_get::<DateTime<Utc>, _>("accepted_at")
            .is_ok()
    );
    let requests = provider.requests.lock().await;
    let accepted_request = requests.first().expect("accepted APNs request");
    assert_eq!(accepted_request.device_token, accepted_token);
    assert_eq!(accepted_request.apns_id, accepted_delivery);
    assert_eq!(
        accepted_request.collapse_id,
        format!("notification-{}", accepted_notification.simple())
    );
    assert!(accepted_request.expiration.is_some());
    let payload = accepted_request.payload.to_string();
    assert!(payload.contains("Alert text for accepted."));
    assert!(!payload.contains("Private accepted title"));
    drop(requests);

    let (retry_installation, _, _) =
        insert_installation(&pool, &key, user_id, credential_id, 2).await;
    let (_, retry_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        retry_installation,
        "news_alert",
        "retry",
        "-1 minute",
        "1 day",
    )
    .await;
    assert!(
        process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("record retryable delivery failure")
    );
    let retry_row = sqlx::query(
        r#"
        SELECT state,attempt_count,last_error_code,
               available_at > clock_timestamp() AS delayed
        FROM brunn.notification_deliveries WHERE id=$1
        "#,
    )
    .bind(retry_delivery)
    .fetch_one(&pool)
    .await
    .expect("read retryable delivery");
    assert_eq!(retry_row.get::<String, _>("state"), "queued");
    assert_eq!(retry_row.get::<i32, _>("attempt_count"), 1);
    assert_eq!(
        retry_row.get::<String, _>("last_error_code"),
        "TooManyRequests"
    );
    assert!(retry_row.get::<bool, _>("delayed"));
    let requests = provider.requests.lock().await;
    let retry_payload = requests
        .get(1)
        .expect("retry APNs request")
        .payload
        .to_string();
    assert!(!retry_payload.contains("Alert text for retry."));
    assert!(!retry_payload.contains("Private retry title"));
    drop(requests);

    let (invalid_installation, _, _) =
        insert_installation(&pool, &key, user_id, credential_id, 3).await;
    let (_, invalid_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        invalid_installation,
        "news_alert",
        "invalid-token",
        "-1 minute",
        "1 day",
    )
    .await;
    assert!(
        process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("record invalid token")
    );
    let invalid_row = sqlx::query(
        r#"
        SELECT installation.enabled,installation.token_ciphertext,
               installation.token_nonce,installation.token_hash,
               delivery.state,delivery.last_error_code
        FROM brunn.notification_installations AS installation
        JOIN brunn.notification_deliveries AS delivery
          ON delivery.installation_id=installation.id
        WHERE installation.id=$1 AND delivery.id=$2
        "#,
    )
    .bind(invalid_installation)
    .bind(invalid_delivery)
    .fetch_one(&pool)
    .await
    .expect("read invalid-token lifecycle");
    assert!(!invalid_row.get::<bool, _>("enabled"));
    assert_eq!(
        invalid_row.get::<Option<Vec<u8>>, _>("token_ciphertext"),
        None
    );
    assert_eq!(invalid_row.get::<Option<Vec<u8>>, _>("token_nonce"), None);
    assert_eq!(invalid_row.get::<Option<String>, _>("token_hash"), None);
    assert_eq!(invalid_row.get::<String, _>("state"), "failed");
    assert_eq!(
        invalid_row.get::<String, _>("last_error_code"),
        "BadDeviceToken"
    );

    let (expired_installation, _, _) =
        insert_installation(&pool, &key, user_id, credential_id, 4).await;
    let (_, expired_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        expired_installation,
        "news_alert",
        "expired",
        "-2 days",
        "-1 day",
    )
    .await;
    assert!(
        process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("expire stale delivery")
    );
    let expired_state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM brunn.notification_deliveries WHERE id=$1",
    )
    .bind(expired_delivery)
    .fetch_one(&pool)
    .await
    .expect("read expired delivery");
    assert_eq!(expired_state, "expired");

    let (stale_installation, _, _) =
        insert_installation(&pool, &key, user_id, credential_id, 5).await;
    let (_, stale_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        stale_installation,
        "news_alert",
        "stale-lease",
        "-1 minute",
        "1 day",
    )
    .await;
    sqlx::query(
        r#"
        UPDATE brunn.notification_deliveries
        SET state='running',attempt_count=1,
            lease_expires_at=clock_timestamp()-interval '1 minute'
        WHERE id=$1
        "#,
    )
    .bind(stale_delivery)
    .execute(&pool)
    .await
    .expect("make delivery lease stale");
    assert!(
        process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("recover stale lease")
    );
    let stale_row =
        sqlx::query("SELECT state,attempt_count FROM brunn.notification_deliveries WHERE id=$1")
            .bind(stale_delivery)
            .fetch_one(&pool)
            .await
            .expect("read stale-lease recovery");
    assert_eq!(stale_row.get::<String, _>("state"), "accepted_by_apns");
    assert_eq!(stale_row.get::<i32, _>("attempt_count"), 1);

    let (blocked_installation, _, _) =
        insert_installation(&pool, &key, user_id, credential_id, 6).await;
    let (_, blocked_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        blocked_installation,
        "news_alert",
        "provider-blocked",
        "-1 minute",
        "1 day",
    )
    .await;
    let (waiting_installation, _, _) =
        insert_installation(&pool, &key, user_id, credential_id, 7).await;
    let (_, waiting_delivery) = insert_delivery(
        &pool,
        user_id,
        credential_id,
        waiting_installation,
        "news_alert",
        "provider-waiting",
        "-1 minute",
        "1 day",
    )
    .await;
    assert!(
        process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("record provider-auth block")
    );
    let blocked_row = sqlx::query(
        r#"
        SELECT state,attempt_count,provider_block_count,last_error_code
        FROM brunn.notification_deliveries WHERE id=$1
        "#,
    )
    .bind(blocked_delivery)
    .fetch_one(&pool)
    .await
    .expect("read provider-blocked delivery");
    assert_eq!(blocked_row.get::<String, _>("state"), "queued");
    assert_eq!(blocked_row.get::<i32, _>("attempt_count"), 0);
    assert_eq!(blocked_row.get::<i32, _>("provider_block_count"), 1);
    assert_eq!(
        blocked_row.get::<String, _>("last_error_code"),
        "InvalidProviderToken"
    );
    assert!(
        !process_next_on_pool(&pool, &encoded_key, Arc::clone(&transport))
            .await
            .expect("provider circuit pauses the queue")
    );
    let waiting_attempts = sqlx::query_scalar::<_, i32>(
        "SELECT attempt_count FROM brunn.notification_deliveries WHERE id=$1",
    )
    .bind(waiting_delivery)
    .fetch_one(&pool)
    .await
    .expect("read delivery waiting behind provider circuit");
    assert_eq!(waiting_attempts, 0);
    assert_eq!(provider.requests.lock().await.len(), 5);

    assert!(
        suppress_queued_deliveries_on_pool(&pool)
            .await
            .expect("suppress transport while the release gate is disabled")
    );
    let suppressed_rows = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM brunn.notification_deliveries
        WHERE id=ANY($1) AND state='suppressed'
          AND last_error_code='transport_disabled'
        "#,
    )
    .bind(vec![blocked_delivery, waiting_delivery])
    .fetch_one(&pool)
    .await
    .expect("read suppressed transport rows");
    assert_eq!(suppressed_rows, 2);
}

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

use brunn::{
    AppState, Config,
    auth::{AuthContext, hash_token},
    db::set_context,
    models::{CredentialId, UserId},
    router,
};

const REPORTS_PER_BATCH: usize = 10;
const WARMUP_BATCH_COUNT: usize = 5;
const TIMED_BATCH_COUNT: usize = 40;
const RETAINED_REPORT_COUNT: i64 = 10_000;
const PRESENCE_WARMUP_COUNT: usize = 5;
const PRESENCE_SAMPLE_COUNT: usize = 100;
const INGEST_P95_LIMIT: Duration = Duration::from_millis(150);
const REDERIVE_LIMIT: Duration = Duration::from_secs(2);
const PRESENCE_PK_P95_LIMIT: Duration = Duration::from_millis(5);

struct Fixture {
    user_id: Uuid,
    token: String,
    auth: AuthContext,
}

struct HttpSample {
    elapsed: Duration,
    status: StatusCode,
    body: Value,
}

fn database_url_as_role(database_url: &str, role: &str) -> String {
    let mut url = Url::parse(database_url).expect("parse disposable PostgreSQL URL");
    url.query_pairs_mut()
        .append_pair("options", &format!("-c role={role}"));
    url.into()
}

async fn connect_test_state() -> Option<(PgPool, AppState)> {
    let Some(database_url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping location performance database gate");
        return None;
    };
    let seed_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to disposable PostgreSQL");
    sqlx::migrate!("./migrations")
        .run(&seed_pool)
        .await
        .expect("apply Brunn migrations");

    let mut config = Config::from_env().expect("load disposable API configuration");
    config.database_url_rw = database_url_as_role(&database_url, "app_rw");
    config.database_url_ro = database_url_as_role(&database_url, "app_ro");
    config.database_url_admin = Some(database_url);
    config.database_max_connections = 8;
    config.embedding_provider = "hashing".to_owned();
    config.openai_api_key = None;
    config.allow_degraded_embeddings = true;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    config.semantic_lane = false;
    config.supersession_demotion = false;
    config.intention_ledger = false;
    config.read_path_roundtrip_v1 = true;
    config.observability_timings_ms = false;
    config.requests_per_minute = 10_000;
    config.location_pings_enabled = true;
    config.location_presence_in_open = true;
    let state = AppState::connect(config)
        .await
        .expect("connect disposable location performance state");
    assert_eq!(state.embedder.provider(), "hashing");
    Some((seed_pool, state))
}

async fn seed_fixture(pool: &PgPool) -> Fixture {
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let scope_ref = format!("scope:location-performance-{scope_id}");
    let token = format!("location-performance-token-{}", Uuid::now_v7());
    let capabilities = vec![
        "location.write".to_owned(),
        "read".to_owned(),
        "save".to_owned(),
    ];
    let mut tx = pool
        .begin()
        .await
        .expect("begin location performance fixture");
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("location-performance:{user_id}"))
        .bind("Location performance gate")
        .execute(&mut *tx)
        .await
        .expect("insert location performance user");
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind("Location performance gate")
        .execute(&mut *tx)
        .await
        .expect("insert location performance scope");
    sqlx::query(
        "INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) \
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind("Location performance gate")
    .bind(hash_token(&token))
    .bind(&capabilities)
    .execute(&mut *tx)
    .await
    .expect("insert location performance credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) \
         VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(&mut *tx)
    .await
    .expect("grant location performance scope");
    tx.commit()
        .await
        .expect("commit location performance fixture");

    Fixture {
        user_id,
        token,
        auth: AuthContext {
            credential_id: CredentialId(credential_id),
            user_id: UserId(user_id),
            capabilities: capabilities.into_iter().collect::<HashSet<_>>(),
            scope_refs: vec![scope_ref],
            read_only: false,
        },
    }
}

fn report_batch(first_ordinal: usize, base: chrono::DateTime<Utc>) -> Value {
    let reports = (0..REPORTS_PER_BATCH)
        .map(|batch_ordinal| {
            let ordinal = first_ordinal + batch_ordinal;
            let at = base + ChronoDuration::minutes(i64::try_from(ordinal * 15).unwrap());
            json!({
                "type": "visit_departure",
                "at": at.to_rfc3339(),
                "lat": 47.6205,
                "lon": -122.3493,
                "accuracy_m": 15,
                "arrived_at": (at - ChronoDuration::minutes(10)).to_rfc3339(),
                "departed_at": (at - ChronoDuration::minutes(2)).to_rfc3339(),
                "geocode": {
                    "city": "Seattle",
                    "region": "WA",
                    "country": "US",
                    "name": "Synthetic place"
                },
                "poi": []
            })
        })
        .collect::<Vec<_>>();
    json!({"timezone": "America/Los_Angeles", "reports": reports})
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> HttpSample {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).expect("serialize performance request"))
        }
        None => Body::empty(),
    };
    let request = builder.body(body).expect("build performance request");
    let started = Instant::now();
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("serve location performance request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect location performance response")
        .to_bytes();
    let elapsed = started.elapsed();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("decode location performance response")
    };
    HttpSample {
        elapsed,
        status,
        body,
    }
}

fn percentile95(samples: &mut [Duration]) -> Duration {
    assert!(!samples.is_empty());
    samples.sort_unstable();
    let rank = (samples.len() * 95).div_ceil(100);
    samples[rank - 1]
}

async fn seed_retained_reports(
    pool: &PgPool,
    user_id: Uuid,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
) {
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.location_reports WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("count ingested location reports");
    assert!(
        existing < RETAINED_REPORT_COUNT,
        "ingest fixture exceeded retained report target"
    );

    let completed_at = to - ChronoDuration::hours(1);
    sqlx::query(
        r#"
        INSERT INTO brunn.location_reports(
          user_id,at,type,offset_min,lat,lon,accuracy_m,arrived_at,
          departed_at,city,region,country,name
        ) VALUES($1,$2,'visit_departure',-420,47.6205,-122.3493,15.0,$3,$4,
                 'Seattle','WA','US','Synthetic place')
        "#,
    )
    .bind(user_id)
    .bind(completed_at)
    .bind(completed_at - ChronoDuration::minutes(30))
    .bind(completed_at - ChronoDuration::minutes(5))
    .execute(pool)
    .await
    .expect("insert retained completed visit");

    let pings_to_seed = RETAINED_REPORT_COUNT - existing - 1;
    let ping_from = from + ChronoDuration::minutes(1);
    let ping_to = to - ChronoDuration::minutes(2);
    let inserted = sqlx::query(
        r#"
        INSERT INTO brunn.location_reports(
          user_id,at,type,offset_min,lat,lon,accuracy_m,city,region,country
        )
        SELECT
          $1,
          $2::timestamptz + (
            ($3::timestamptz - $2::timestamptz)
            * (ordinal::double precision / GREATEST(($4 - 1)::double precision,1.0))
          ),
          'ping',-420,47.6205,-122.3493,15.0,'Seattle','WA','US'
        FROM generate_series(0,$4 - 1) AS fixture(ordinal)
        "#,
    )
    .bind(user_id)
    .bind(ping_from)
    .bind(ping_to)
    .bind(pings_to_seed)
    .execute(pool)
    .await
    .expect("insert retained ping reports");
    assert_eq!(inserted.rows_affected(), pings_to_seed as u64);
    let retained = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM brunn.location_reports WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("count retained location reports");
    assert_eq!(retained, RETAINED_REPORT_COUNT);
}

async fn presence_pk_p95(state: &AppState, fixture: &Fixture) -> Duration {
    let mut tx = state
        .ro_pool
        .begin()
        .await
        .expect("begin presence performance transaction");
    set_context(&mut tx, &fixture.auth)
        .await
        .expect("set presence performance context");
    for _ in 0..PRESENCE_WARMUP_COUNT {
        let _: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT reported_at FROM brunn.location_presence WHERE user_id=$1")
                .bind(fixture.user_id)
                .fetch_one(&mut *tx)
                .await
                .expect("warm presence primary-key lookup");
    }
    let mut samples = Vec::with_capacity(PRESENCE_SAMPLE_COUNT);
    for _ in 0..PRESENCE_SAMPLE_COUNT {
        let started = Instant::now();
        let _: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT reported_at FROM brunn.location_presence WHERE user_id=$1")
                .bind(fixture.user_id)
                .fetch_one(&mut *tx)
                .await
                .expect("measure presence primary-key lookup");
        samples.push(started.elapsed());
    }
    tx.commit()
        .await
        .expect("commit presence performance transaction");
    percentile95(&mut samples)
}

#[tokio::test(flavor = "current_thread")]
async fn location_endpoints_meet_section_11_performance_budgets() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    let app = router(state.clone());
    let now = Utc::now();
    let ingest_base = now - ChronoDuration::days(8);

    for batch_ordinal in 0..WARMUP_BATCH_COUNT {
        let first_ordinal = batch_ordinal * REPORTS_PER_BATCH;
        let sample = request(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.token,
            Some(report_batch(first_ordinal, ingest_base)),
        )
        .await;
        assert_eq!(sample.status, StatusCode::OK);
        assert_eq!(sample.body["accepted"], REPORTS_PER_BATCH);
    }

    let mut ingest_samples = Vec::with_capacity(TIMED_BATCH_COUNT);
    for batch_ordinal in 0..TIMED_BATCH_COUNT {
        let first_ordinal = (WARMUP_BATCH_COUNT + batch_ordinal) * REPORTS_PER_BATCH;
        let sample = request(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.token,
            Some(report_batch(first_ordinal, ingest_base)),
        )
        .await;
        assert_eq!(sample.status, StatusCode::OK);
        assert_eq!(sample.body["accepted"], REPORTS_PER_BATCH);
        ingest_samples.push(sample.elapsed);
    }
    let ingest_p95 = percentile95(&mut ingest_samples);
    assert!(
        ingest_p95 < INGEST_P95_LIMIT,
        "10-report ingest p95 {:.3} ms exceeded 150 ms",
        ingest_p95.as_secs_f64() * 1_000.0
    );

    let rederive_to = now - ChronoDuration::minutes(1);
    // Leave a small wall-clock margin for the route's own `Utc::now()` while
    // still exercising effectively the complete 30-day retention window.
    let rederive_from = rederive_to - ChronoDuration::days(30) + ChronoDuration::minutes(2);
    seed_retained_reports(&pool, fixture.user_id, rederive_from, rederive_to).await;
    let rederive = request(
        &app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.token,
        Some(json!({
            "from": rederive_from.to_rfc3339(),
            "to": rederive_to.to_rfc3339()
        })),
    )
    .await;
    assert_eq!(rederive.status, StatusCode::OK);
    assert_eq!(rederive.body["reports_replayed"], RETAINED_REPORT_COUNT);
    assert!(
        rederive.elapsed < REDERIVE_LIMIT,
        "30-day rederive {:.3} ms exceeded 2000 ms",
        rederive.elapsed.as_secs_f64() * 1_000.0
    );

    let (derived_files, derived_bytes) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT count(*),COALESCE(sum(length(version.content)),0)
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1 AND entry.path LIKE 'Location/Visits/%.md'
        "#,
    )
    .bind(fixture.user_id)
    .fetch_one(&pool)
    .await
    .expect("measure derived location files");
    assert!((1..=2).contains(&derived_files));
    assert!(
        derived_bytes >= 40_000,
        "derived location fixture was only {derived_bytes} bytes"
    );

    let mut presence_route_samples = Vec::with_capacity(PRESENCE_SAMPLE_COUNT);
    for ordinal in 0..PRESENCE_WARMUP_COUNT + PRESENCE_SAMPLE_COUNT {
        let sample = request(
            &app,
            Method::GET,
            "/v1/location/presence",
            &fixture.token,
            None,
        )
        .await;
        assert_eq!(sample.status, StatusCode::OK);
        if ordinal >= PRESENCE_WARMUP_COUNT {
            presence_route_samples.push(sample.elapsed);
        }
    }
    let presence_route_p95 = percentile95(&mut presence_route_samples);
    let presence_pk_p95 = presence_pk_p95(&state, &fixture).await;
    assert!(
        presence_pk_p95 < PRESENCE_PK_P95_LIMIT,
        "presence primary-key p95 {:.3} ms exceeded 5 ms",
        presence_pk_p95.as_secs_f64() * 1_000.0
    );

    eprintln!(
        "location_performance ingest_batches={TIMED_BATCH_COUNT} reports_per_batch={REPORTS_PER_BATCH} ingest_p95_ms={:.3} retained_reports={RETAINED_REPORT_COUNT} derived_files={derived_files} derived_bytes={derived_bytes} rederive_ms={:.3} presence_samples={PRESENCE_SAMPLE_COUNT} presence_route_p95_ms={:.3} presence_pk_p95_ms={:.3}",
        ingest_p95.as_secs_f64() * 1_000.0,
        rederive.elapsed.as_secs_f64() * 1_000.0,
        presence_route_p95.as_secs_f64() * 1_000.0,
        presence_pk_p95.as_secs_f64() * 1_000.0,
    );
}

use std::time::{Duration, Instant};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

use brunn::{AppState, Config, auth::hash_token, router};

const SAMPLE_COUNT: usize = 30;
const WARMUP_PAIR_COUNT: usize = 2;

struct Fixture {
    token: String,
    path: String,
}

struct OpenSample {
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
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping location open database gate");
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
    config.database_url_admin = None;
    config.database_max_connections = 8;
    config.apns_delivery_enabled = false;
    config.messaging_enabled = false;
    config.semantic_lane = false;
    config.supersession_demotion = false;
    config.intention_ledger = false;
    config.read_path_roundtrip_v1 = false;
    config.observability_timings_ms = false;
    let state = AppState::connect(config)
        .await
        .expect("connect disposable API state");
    Some((seed_pool, state))
}

async fn seed_fixture(pool: &PgPool) -> Fixture {
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let token = format!("location-open-gate-{}", Uuid::now_v7());
    let scope_ref = format!("scope:location-open-gate-{scope_id}");
    let path = "Projects/Location open gate.md".to_owned();
    let content = "# Location open gate\n\nPaired location gate marker.\n";
    let content_hash = hex::encode(Sha256::digest(content.as_bytes()));
    let now = Utc::now();
    let mut tx = pool.begin().await.expect("begin location open fixture");

    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("location-open-gate:{user_id}"))
        .bind("Location open database gate")
        .execute(&mut *tx)
        .await
        .expect("insert location open user");
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(&scope_ref)
        .bind("Location open database gate")
        .execute(&mut *tx)
        .await
        .expect("insert location open scope");
    sqlx::query(
        "INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) \
         VALUES($1,$2,$3,$4,ARRAY['open']::text[])",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind("Location open database gate")
    .bind(hash_token(&token))
    .execute(&mut *tx)
    .await
    .expect("insert location open credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) \
         VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(&mut *tx)
    .await
    .expect("grant location open scope");
    sqlx::query(
        r#"
        INSERT INTO brunn.entries(id,user_id,path,title,kind,media_type,current_version)
        VALUES($1,$2,$3,'Location open gate','markdown','text/markdown',0)
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(&path)
    .execute(&mut *tx)
    .await
    .expect("insert location open entry");
    sqlx::query(
        r#"
        INSERT INTO brunn.entry_versions(
          id,user_id,entry_id,version,content_sha256,content,size_bytes,
          created_by_credential_id
        ) VALUES($1,$2,$3,1,$4,$5,$6,$7)
        "#,
    )
    .bind(version_id)
    .bind(user_id)
    .bind(entry_id)
    .bind(&content_hash)
    .bind(content)
    .bind(i64::try_from(content.len()).expect("fixture content fits i64"))
    .bind(credential_id)
    .execute(&mut *tx)
    .await
    .expect("insert location open version");
    sqlx::query("UPDATE brunn.entries SET current_version=1 WHERE user_id=$1 AND id=$2")
        .bind(user_id)
        .bind(entry_id)
        .execute(&mut *tx)
        .await
        .expect("activate location open version");
    sqlx::query(
        r#"
        INSERT INTO brunn.search_chunks(
          user_id,entry_id,entry_version_id,chunk_index,path,heading,content,token_estimate
        ) VALUES($1,$2,$3,0,$4,'Location open gate',$5,10)
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .bind(version_id)
    .bind(&path)
    .bind(content)
    .execute(&mut *tx)
    .await
    .expect("insert location open search chunk");
    sqlx::query(
        r#"
        INSERT INTO brunn.workspace_changes(
          user_id,entry_id,entry_version,operation,path,content_sha256
        ) VALUES($1,$2,1,'create',$3,$4)
        "#,
    )
    .bind(user_id)
    .bind(entry_id)
    .bind(&path)
    .bind(&content_hash)
    .execute(&mut *tx)
    .await
    .expect("insert location open change");
    sqlx::query(
        r#"
        INSERT INTO brunn.location_presence(
          user_id,timezone,reported_at,last_lat,last_lon,last_accuracy_m,
          city,region,country,visit_arrived_at,visit_lat,visit_lon,
          visit_label,visit_kind,visit_confidence
        ) VALUES(
          $1,'America/Los_Angeles',$2,47.6205,-122.3493,12.0,
          'Seattle','Washington','United States',$3,47.6205,-122.3493,
          'Home','home','high'
        )
        "#,
    )
    .bind(user_id)
    .bind(now)
    .bind(now - ChronoDuration::hours(2))
    .execute(&mut *tx)
    .await
    .expect("insert location presence");
    tx.commit().await.expect("commit location open fixture");

    Fixture { token, path }
}

async fn request_open(app: &Router, token: &str) -> OpenSample {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/workspace/open")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "task": "paired location gate marker",
                "hints": {},
                "resume_checkpoint_ref": null,
                "token_budget": 4_000,
                "modes": []
            }))
            .expect("serialize location open request"),
        ))
        .expect("build location open request");
    let started = Instant::now();
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("serve location open request");
    let elapsed = started.elapsed();
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect location open response")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).expect("decode location open response");
    OpenSample {
        elapsed,
        status,
        body,
    }
}

fn assert_paired_responses(on: &OpenSample, off: &OpenSample, fixture: &Fixture) {
    assert_eq!(
        on.status,
        StatusCode::OK,
        "flag-on open failed: {}",
        on.body
    );
    assert_eq!(
        off.status,
        StatusCode::OK,
        "flag-off open failed: {}",
        off.body
    );
    assert_eq!(
        on.body
            .pointer("/data/owner_presence/status")
            .and_then(Value::as_str),
        Some("at_place")
    );
    assert_eq!(
        on.body
            .pointer("/data/owner_presence/place/label")
            .and_then(Value::as_str),
        Some("Home")
    );
    assert!(
        off.body.pointer("/data/owner_presence").is_none(),
        "flag-off open exposed owner presence"
    );
    let on_evidence = on
        .body
        .pointer("/data/evidence")
        .expect("flag-on open returns evidence");
    let off_evidence = off
        .body
        .pointer("/data/evidence")
        .expect("flag-off open returns evidence");
    assert_eq!(
        on_evidence, off_evidence,
        "presence lookup changed retrieval"
    );
    assert!(
        on_evidence
            .as_array()
            .is_some_and(|evidence| evidence.iter().any(|item| {
                item.get("path").and_then(Value::as_str) == Some(fixture.path.as_str())
            })),
        "paired open omitted the seeded retrieval evidence"
    );
    for key in [
        "workspace_generation",
        "evidence_leads",
        "retrieval_sufficiency",
    ] {
        assert_eq!(
            on.body.pointer(&format!("/data/{key}")),
            off.body.pointer(&format!("/data/{key}")),
            "presence lookup changed {key}"
        );
    }
}

fn percentile95(samples: &mut [Duration]) -> Duration {
    assert!(!samples.is_empty());
    samples.sort_unstable();
    let rank = (samples.len() * 95).div_ceil(100);
    samples[rank - 1]
}

#[test]
fn paired_location_presence_open_gate_preserves_retrieval_and_reports_latency() {
    let thread = std::thread::Builder::new()
        .name("location-open-gate".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build location open test runtime");
            runtime.block_on(run_paired_location_presence_open_gate());
        })
        .expect("spawn location open test thread");
    if let Err(panic) = thread.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn run_paired_location_presence_open_gate() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    let mut on_state = state.clone();
    on_state.config.location_presence_in_open = true;
    let mut off_state = state;
    off_state.config.location_presence_in_open = false;
    let on_app = router(on_state);
    let off_app = router(off_state);

    for pair in 0..WARMUP_PAIR_COUNT {
        let (on, off) = if pair % 2 == 0 {
            (
                request_open(&on_app, &fixture.token).await,
                request_open(&off_app, &fixture.token).await,
            )
        } else {
            let off = request_open(&off_app, &fixture.token).await;
            let on = request_open(&on_app, &fixture.token).await;
            (on, off)
        };
        assert_paired_responses(&on, &off, &fixture);
    }

    let mut on_samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut off_samples = Vec::with_capacity(SAMPLE_COUNT);
    for pair in 0..SAMPLE_COUNT {
        let (on, off) = if pair % 2 == 0 {
            (
                request_open(&on_app, &fixture.token).await,
                request_open(&off_app, &fixture.token).await,
            )
        } else {
            let off = request_open(&off_app, &fixture.token).await;
            let on = request_open(&on_app, &fixture.token).await;
            (on, off)
        };
        assert_paired_responses(&on, &off, &fixture);
        on_samples.push(on.elapsed);
        off_samples.push(off.elapsed);
    }

    let on_p95 = percentile95(&mut on_samples);
    let off_p95 = percentile95(&mut off_samples);
    let delta_ms = on_p95.as_secs_f64() * 1_000.0 - off_p95.as_secs_f64() * 1_000.0;
    eprintln!(
        "location_open_latency samples={SAMPLE_COUNT} flag_off_p95_ms={:.3} flag_on_p95_ms={:.3} delta_ms={delta_ms:.3}",
        off_p95.as_secs_f64() * 1_000.0,
        on_p95.as_secs_f64() * 1_000.0,
    );
}

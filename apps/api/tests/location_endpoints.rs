use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use url::Url;
use uuid::Uuid;

use brunn::{AppState, Config, auth::hash_token, router};

#[derive(Debug)]
struct HttpResponse {
    status: StatusCode,
    body: Value,
}

struct CredentialFixture {
    token: String,
}

struct LocationFixture {
    user_id: Uuid,
    device: CredentialFixture,
    saver: CredentialFixture,
    reader: CredentialFixture,
}

#[derive(Debug, PartialEq)]
struct ReplayDatabaseSnapshot {
    reports: Value,
    poi: Value,
    presence: Value,
    month_content: String,
    month_version: i64,
    month_version_count: i64,
    workspace_change_count: i64,
}

#[derive(Clone, Copy)]
struct ReplayStepExpectation {
    accepted: u64,
    ignored: Option<&'static str>,
    presence_status: Option<&'static str>,
    place_label: Option<&'static str>,
    city: Option<&'static str>,
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
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
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping location endpoint contract");
        return None;
    };
    let seed_pool = PgPoolOptions::new()
        .max_connections(6)
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
    config.allow_degraded_embeddings = true;
    config.messaging_enabled = false;
    config.location_pings_enabled = true;
    config.location_presence_in_open = true;
    let state = AppState::connect(config)
        .await
        .expect("connect disposable location API state");
    Some((seed_pool, state))
}

async fn insert_credential(
    pool: &PgPool,
    user_id: Uuid,
    scope_id: Uuid,
    label: &str,
    capabilities: &[&str],
) -> CredentialFixture {
    let credential_id = Uuid::now_v7();
    let token = format!("location-endpoint-token-{}", Uuid::now_v7());
    let capabilities = capabilities
        .iter()
        .map(|capability| (*capability).to_owned())
        .collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) \
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(label)
    .bind(hash_token(&token))
    .bind(capabilities)
    .execute(pool)
    .await
    .expect("insert location endpoint credential");
    sqlx::query(
        "INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) \
         VALUES($1,$2,$3)",
    )
    .bind(credential_id)
    .bind(user_id)
    .bind(scope_id)
    .execute(pool)
    .await
    .expect("grant location endpoint scope");
    CredentialFixture { token }
}

async fn seed_fixture(pool: &PgPool) -> LocationFixture {
    let user_id = Uuid::now_v7();
    let scope_id = Uuid::now_v7();
    let scope_ref = format!("scope:location-endpoint-{scope_id}");
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,$3)")
        .bind(user_id)
        .bind(format!("location-endpoint:{user_id}"))
        .bind("Location endpoint owner")
        .execute(pool)
        .await
        .expect("insert location endpoint user");
    sqlx::query("INSERT INTO brunn.scopes(id,user_id,scope_ref,name) VALUES($1,$2,$3,$4)")
        .bind(scope_id)
        .bind(user_id)
        .bind(scope_ref)
        .bind("Location endpoint root")
        .execute(pool)
        .await
        .expect("insert location endpoint scope");
    let device = insert_credential(
        pool,
        user_id,
        scope_id,
        "iOS location",
        &[
            "open",
            "query",
            "read",
            "compute",
            "verify",
            "status",
            "task.read",
            "location.write",
        ],
    )
    .await;
    let saver = insert_credential(
        pool,
        user_id,
        scope_id,
        "Location owner",
        &[
            "open",
            "query",
            "read",
            "compute",
            "verify",
            "status",
            "save",
            "task.read",
        ],
    )
    .await;
    let reader = insert_credential(
        pool,
        user_id,
        scope_id,
        "Location read only",
        &[
            "open",
            "query",
            "read",
            "compute",
            "verify",
            "status",
            "task.read",
        ],
    )
    .await;
    LocationFixture {
        user_id,
        device,
        saver,
        reader,
    }
}

async fn request_bytes(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    bytes: Option<Vec<u8>>,
) -> HttpResponse {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if let Some(bytes) = bytes {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(bytes)
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).expect("build location endpoint request"))
        .await
        .expect("serve location endpoint request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect location endpoint response")
        .to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    HttpResponse { status, body }
}

async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> HttpResponse {
    request_bytes(
        app,
        method,
        uri,
        token,
        Some(serde_json::to_vec(&body).expect("serialize location request")),
    )
    .await
}

fn assert_error(response: &HttpResponse, status: StatusCode, code: &str) {
    assert_eq!(response.status, status, "unexpected endpoint status");
    assert_eq!(
        response.body.pointer("/error/code").and_then(Value::as_str),
        Some(code),
        "unexpected endpoint error"
    );
}

fn places_document(radius_m: u16) -> String {
    format!(
        "---\nkind: location-places\n---\n\
         | Label | Kind | Lat | Lon | Radius m |\n\
         | --- | --- | --- | --- | --- |\n\
         | Home | home | 47.0000 | -122.0000 | {radius_m} |\n"
    )
}

async fn write_places(app: &Router, token: &str, content: String, expected_version: i64) {
    let response = request_json(
        app,
        Method::POST,
        "/v1/workspace/write",
        token,
        json!({
            "path": "Location/Places.md",
            "content": content,
            "media_type": "text/markdown",
            "expected_version": expected_version,
            "metadata": {}
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "write Places.md");
}

fn completed_report(at: chrono::DateTime<Utc>, label: &str, city: &str, poi: Value) -> Value {
    let offset = FixedOffset::west_opt(7 * 60 * 60).unwrap();
    json!({
        "type": "visit_departure",
        "at": at.with_timezone(&offset).to_rfc3339(),
        "lat": 47.0009,
        "lon": -122.0000,
        "accuracy_m": 20,
        "arrived_at": (at - Duration::minutes(45)).with_timezone(&offset).to_rfc3339(),
        "departed_at": (at - Duration::minutes(5)).with_timezone(&offset).to_rfc3339(),
        "geocode": {"city": city, "region": "WA", "country": "US", "name": label},
        "poi": poi
    })
}

fn batch(report: Value) -> Value {
    json!({"timezone": "America/Los_Angeles", "reports": [report]})
}

async fn current_month_text(pool: &PgPool, user_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT version.content
        FROM brunn.entries AS entry
        JOIN brunn.entry_versions AS version
          ON version.user_id=entry.user_id
         AND version.entry_id=entry.id
         AND version.version=entry.current_version
        WHERE entry.user_id=$1 AND entry.path LIKE 'Location/Visits/%.md'
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("read derived location month")
}

fn replay_day_batches() -> Vec<Value> {
    vec![
        batch(json!({
            "type": "ping",
            "at": "2026-09-01T09:10:00-07:00",
            "lat": 47.6205,
            "lon": -122.2070,
            "accuracy_m": 25,
            "geocode": {"city": "Bellevue", "region": "WA", "country": "US"}
        })),
        batch(json!({
            "type": "ping",
            "at": "2026-09-01T12:42:00-07:00",
            "lat": 46.9965,
            "lon": -120.5478,
            "accuracy_m": 65,
            "geocode": {"city": "Ellensburg", "region": "WA", "country": "US"}
        })),
        batch(json!({
            "type": "visit_departure",
            "at": "2026-09-01T13:45:00-07:00",
            "lat": 47.6156,
            "lon": -122.2035,
            "accuracy_m": 25,
            "arrived_at": "2026-09-01T12:55:00-07:00",
            "departed_at": "2026-09-01T13:40:00-07:00",
            "geocode": {
                "city": "Bellevue",
                "region": "WA",
                "country": "US",
                "name": "Bellevue Square"
            },
            "poi": [
                {"name": "Din Tai Fung", "category": "restaurant", "distance_m": 18},
                {"name": "Bellevue Square", "category": "store", "distance_m": 95}
            ]
        })),
        batch(json!({
            "type": "ping",
            "at": "2026-09-01T14:10:00-07:00",
            "lat": 47.6213,
            "lon": -122.2070,
            "accuracy_m": 25,
            "geocode": {"city": "Bellevue", "region": "WA", "country": "US"}
        })),
        batch(json!({
            "type": "visit_departure",
            "at": "2026-09-01T14:15:00-07:00",
            "lat": 47.6205,
            "lon": -122.2070,
            "accuracy_m": 25,
            "arrived_at": "2026-09-01T09:10:00-07:00",
            "departed_at": "2026-09-01T12:42:00-07:00",
            "geocode": {
                "city": "Bellevue",
                "region": "WA",
                "country": "US",
                "name": "Home"
            }
        })),
        batch(json!({
            "type": "visit_departure",
            "at": "2026-09-01T14:05:00-07:00",
            "lat": 47.6213,
            "lon": -122.2070,
            "accuracy_m": 25,
            "arrived_at": "2026-09-01T13:50:00-07:00",
            "departed_at": "2026-09-01T14:00:00-07:00",
            "geocode": {
                "city": "Bellevue",
                "region": "WA",
                "country": "US",
                "name": "Neighborhood"
            }
        })),
    ]
}

fn replay_places_document() -> String {
    "---\nkind: location-places\n---\n\
     | Label | Kind | Lat | Lon | Radius m |\n\
     | --- | --- | --- | --- | --- |\n\
     | Home | home | 47.6205 | -122.2070 | 150 |\n"
        .to_owned()
}

fn replay_at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("parse fixed replay timestamp")
        .to_utc()
}

async fn replay_database_snapshot(pool: &PgPool, user_id: Uuid) -> ReplayDatabaseSnapshot {
    let reports = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT COALESCE(jsonb_agg(to_jsonb(report_row) ORDER BY at,type),'[]'::jsonb)
        FROM (
          SELECT at,type,offset_min,lat,lon,accuracy_m,arrived_at,departed_at,
                 city,region,country,name
          FROM brunn.location_reports
          WHERE user_id=$1
        ) AS report_row
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("snapshot replay raw reports");
    let poi = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT COALESCE(jsonb_agg(to_jsonb(poi_row) ORDER BY at,type,rank),'[]'::jsonb)
        FROM (
          SELECT at,type,rank,name,category,distance_m
          FROM brunn.location_report_poi
          WHERE user_id=$1
        ) AS poi_row
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("snapshot replay raw POI rows");
    let presence = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT COALESCE(
          (
            SELECT to_jsonb(presence_row) - 'user_id'
            FROM brunn.location_presence AS presence_row
            WHERE user_id=$1
          ),
          'null'::jsonb
        )
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("snapshot replay presence");
    let (month_version, month_content, month_version_count, workspace_change_count) =
        sqlx::query_as::<_, (i64, String, i64, i64)>(
            r#"
            SELECT entry.current_version,version.content,
                   (SELECT count(*) FROM brunn.entry_versions AS all_versions
                    WHERE all_versions.user_id=entry.user_id
                      AND all_versions.entry_id=entry.id),
                   (SELECT count(*) FROM brunn.workspace_changes AS change
                    WHERE change.user_id=entry.user_id
                      AND change.entry_id=entry.id)
            FROM brunn.entries AS entry
            JOIN brunn.entry_versions AS version
              ON version.user_id=entry.user_id
             AND version.entry_id=entry.id
             AND version.version=entry.current_version
            WHERE entry.user_id=$1 AND entry.path='Location/Visits/2026-09.md'
            "#,
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("snapshot replay month file");
    ReplayDatabaseSnapshot {
        reports,
        poi,
        presence,
        month_content,
        month_version,
        month_version_count,
        workspace_change_count,
    }
}

async fn assert_replay_raw_rows(pool: &PgPool, user_id: Uuid, pings_enabled: bool) {
    let actual = sqlx::query_as::<_, (DateTime<Utc>, String)>(
        "SELECT at,type FROM brunn.location_reports WHERE user_id=$1 ORDER BY at,type",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .expect("read exact replay raw rows");
    let mut expected = vec![
        (
            replay_at("2026-09-01T13:45:00-07:00"),
            "visit_departure".to_owned(),
        ),
        (
            replay_at("2026-09-01T14:05:00-07:00"),
            "visit_departure".to_owned(),
        ),
        (
            replay_at("2026-09-01T14:15:00-07:00"),
            "visit_departure".to_owned(),
        ),
    ];
    if pings_enabled {
        expected.extend([
            (replay_at("2026-09-01T09:10:00-07:00"), "ping".to_owned()),
            (replay_at("2026-09-01T12:42:00-07:00"), "ping".to_owned()),
            (replay_at("2026-09-01T14:10:00-07:00"), "ping".to_owned()),
        ]);
        expected.sort();
    }
    assert_eq!(actual, expected, "raw report rows/types differ");

    let poi = sqlx::query_as::<_, (i16, String, Option<String>, f32)>(
        r#"
        SELECT rank,name,category,distance_m
        FROM brunn.location_report_poi
        WHERE user_id=$1
        ORDER BY at,type,rank
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .expect("read exact replay POI rows");
    assert_eq!(
        poi,
        vec![
            (
                1,
                "Din Tai Fung".to_owned(),
                Some("restaurant".to_owned()),
                18.0,
            ),
            (
                2,
                "Bellevue Square".to_owned(),
                Some("store".to_owned()),
                95.0,
            ),
        ]
    );
}

async fn exercise_replay_day_gate(
    pool: &PgPool,
    app: &Router,
    fixture: &LocationFixture,
    pings_enabled: bool,
) {
    write_places(app, &fixture.saver.token, replay_places_document(), 0).await;
    let batches = replay_day_batches();
    let expected = if pings_enabled {
        [
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: Some("Home"),
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: None,
                city: Some("Ellensburg"),
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: None,
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: Some("Home"),
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: Some("Home"),
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 0,
                ignored: Some("late"),
                presence_status: Some("stale"),
                place_label: Some("Home"),
                city: Some("Bellevue"),
            },
        ]
    } else {
        [
            ReplayStepExpectation {
                accepted: 0,
                ignored: Some("pings_off"),
                presence_status: None,
                place_label: None,
                city: None,
            },
            ReplayStepExpectation {
                accepted: 0,
                ignored: Some("pings_off"),
                presence_status: None,
                place_label: None,
                city: None,
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: None,
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 0,
                ignored: Some("pings_off"),
                presence_status: Some("stale"),
                place_label: None,
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: None,
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 0,
                ignored: Some("late"),
                presence_status: Some("stale"),
                place_label: None,
                city: Some("Bellevue"),
            },
        ]
    };
    let expected_stored_presence = if pings_enabled {
        vec![
            Some((
                replay_at("2026-09-01T09:10:00-07:00"),
                Some(replay_at("2026-09-01T09:10:00-07:00")),
                Some("Home"),
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T12:42:00-07:00"),
                None,
                None,
                Some("Ellensburg"),
            )),
            Some((
                replay_at("2026-09-01T13:45:00-07:00"),
                None,
                None,
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T14:10:00-07:00"),
                Some(replay_at("2026-09-01T14:10:00-07:00")),
                Some("Home"),
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T14:15:00-07:00"),
                Some(replay_at("2026-09-01T14:10:00-07:00")),
                Some("Home"),
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T14:15:00-07:00"),
                Some(replay_at("2026-09-01T14:10:00-07:00")),
                Some("Home"),
                Some("Bellevue"),
            )),
        ]
    } else {
        vec![
            None,
            None,
            Some((
                replay_at("2026-09-01T13:45:00-07:00"),
                None,
                None,
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T13:45:00-07:00"),
                None,
                None,
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T14:15:00-07:00"),
                None,
                None,
                Some("Bellevue"),
            )),
            Some((
                replay_at("2026-09-01T14:15:00-07:00"),
                None,
                None,
                Some("Bellevue"),
            )),
        ]
    };

    for (index, ((body, expectation), expected_presence)) in batches
        .iter()
        .zip(expected)
        .zip(expected_stored_presence)
        .enumerate()
    {
        let response = request_json(
            app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            body.clone(),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "replay step {index}");
        assert_eq!(
            response.body["accepted"].as_u64(),
            Some(expectation.accepted),
            "accepted count at replay step {index}"
        );
        let ignored = response.body["ignored"]
            .as_object()
            .expect("replay response has ignored counts");
        match expectation.ignored {
            Some(reason) => {
                assert_eq!(ignored.len(), 1, "ignored shape at replay step {index}");
                assert_eq!(
                    ignored.get(reason).and_then(Value::as_u64),
                    Some(1),
                    "ignored reason at replay step {index}"
                );
            }
            None => assert!(ignored.is_empty(), "unexpected ignored replay step {index}"),
        }
        assert_eq!(
            response
                .body
                .pointer("/presence/status")
                .and_then(Value::as_str),
            expectation.presence_status,
            "presence status at replay step {index}"
        );
        assert_eq!(
            response
                .body
                .pointer("/presence/place/label")
                .and_then(Value::as_str),
            expectation.place_label,
            "presence place at replay step {index}"
        );
        assert_eq!(
            response
                .body
                .pointer("/presence/city")
                .and_then(Value::as_str),
            expectation.city,
            "presence city at replay step {index}"
        );
        let stored_presence = sqlx::query_as::<
            _,
            (
                DateTime<Utc>,
                Option<DateTime<Utc>>,
                Option<String>,
                Option<String>,
            ),
        >(
            r#"
            SELECT reported_at,visit_arrived_at,visit_label,city
            FROM brunn.location_presence
            WHERE user_id=$1
            "#,
        )
        .bind(fixture.user_id)
        .fetch_optional(pool)
        .await
        .expect("read replay presence transition");
        match (stored_presence, expected_presence) {
            (None, None) => {}
            (Some((reported_at, visit_arrived_at, visit_label, city)), Some(expected)) => {
                assert_eq!(reported_at, expected.0, "watermark at replay step {index}");
                assert_eq!(
                    visit_arrived_at, expected.1,
                    "open-visit arrival at replay step {index}"
                );
                assert_eq!(
                    visit_label.as_deref(),
                    expected.2,
                    "open-visit label at replay step {index}"
                );
                assert_eq!(
                    city.as_deref(),
                    expected.3,
                    "presence city row at replay step {index}"
                );
            }
            (actual, expected) => {
                panic!(
                    "presence row existence differs at replay step {index}: {actual:?} != {expected:?}"
                )
            }
        }
    }

    assert_replay_raw_rows(pool, fixture.user_id, pings_enabled).await;
    let expected_month = if pings_enabled {
        concat!(
            "---\n",
            "kind: location-visits\n",
            "month: 2026-09\n",
            "---\n",
            "| Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n",
            "| --- | --- | --- | --- | --- | --- | --- | --- |\n",
            "| 2026-09-01T09:10-07:00 | 2026-09-01T12:42-07:00 | 3h32m | Home | home | Bellevue, WA, US | high | 47.6205,-122.2070 |\n",
            "| 2026-09-01T12:42-07:00 | — | — | passed through | transit | Ellensburg, WA, US | low | 46.9965,-120.5478 |\n",
            "| 2026-09-01T12:55-07:00 | 2026-09-01T13:40-07:00 | 45m | Din Tai Fung | restaurant | Bellevue, WA, US | medium | 47.6156,-122.2035 |\n",
        )
    } else {
        concat!(
            "---\n",
            "kind: location-visits\n",
            "month: 2026-09\n",
            "---\n",
            "| Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n",
            "| --- | --- | --- | --- | --- | --- | --- | --- |\n",
            "| 2026-09-01T09:10-07:00 | 2026-09-01T12:42-07:00 | 3h32m | Home | home | Bellevue, WA, US | high | 47.6205,-122.2070 |\n",
            "| 2026-09-01T12:55-07:00 | 2026-09-01T13:40-07:00 | 45m | Din Tai Fung | restaurant | Bellevue, WA, US | medium | 47.6156,-122.2035 |\n",
        )
    };
    let before_resend = replay_database_snapshot(pool, fixture.user_id).await;
    assert_eq!(before_resend.month_content, expected_month);
    assert_eq!(before_resend.month_version, 2);
    assert_eq!(before_resend.month_version_count, 2);
    assert_eq!(before_resend.workspace_change_count, 2);

    for body in &batches {
        let response = request_json(
            app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            body.clone(),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "re-send replay batch");
    }
    assert_eq!(
        replay_database_snapshot(pool, fixture.user_id).await,
        before_resend,
        "re-sending every replay batch must be a byte/database no-op"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replay_day_gate_is_exact_on_real_router_with_pings_on_and_off() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let pings_on = seed_fixture(&pool).await;
    let pings_off = seed_fixture(&pool).await;
    let on_app = router(state.clone());
    let mut off_state = state;
    off_state.config.location_pings_enabled = false;
    let off_app = router(off_state);

    exercise_replay_day_gate(&pool, &on_app, &pings_on, true).await;
    exercise_replay_day_gate(&pool, &off_app, &pings_off, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn location_routes_enforce_privacy_idempotence_and_live_places_edits() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    let app = router(state);
    let base = Utc::now() - Duration::hours(5);
    let first = completed_report(
        base,
        "FIRST_GEOCODE_SENTINEL",
        "FIRST_CITY_SENTINEL",
        json!([{"name":"First POI","category":"restaurant","distance_m":10}]),
    );

    assert_error(
        &request_bytes(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            Some(b"{".to_vec()),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let invalid_ping = json!({
        "type": "ping",
        "at": base.to_rfc3339(),
        "lat": 47.0,
        "lon": -122.0,
        "accuracy_m": 20,
        "poi": [{"name":"not allowed","distance_m":1}]
    });
    assert_error(
        &request_json(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            batch(invalid_ping),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    for (method, path, body) in [
        (
            Method::POST,
            "/v1/location/reports",
            Some(batch(first.clone())),
        ),
        (Method::POST, "/v1/location/rederive", Some(json!({}))),
        (Method::DELETE, "/v1/location/live", None),
    ] {
        let response = match body {
            Some(body) => request_json(&app, method, path, &fixture.reader.token, body).await,
            None => request_bytes(&app, method, path, &fixture.reader.token, None).await,
        };
        assert_error(&response, StatusCode::FORBIDDEN, "capability_denied");
    }

    assert_eq!(
        request_bytes(
            &app,
            Method::GET,
            "/v1/location/presence",
            &fixture.reader.token,
            None,
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    write_places(&app, &fixture.saver.token, places_document(50), 0).await;
    let first_response = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        batch(first.clone()),
    )
    .await;
    assert_eq!(first_response.status, StatusCode::OK);
    assert_eq!(first_response.body["accepted"], 1);
    assert_eq!(first_response.body["presence"]["status"], "between_places");
    let first_month = current_month_text(&pool, fixture.user_id).await;
    assert!(first_month.contains("First POI"));
    assert!(first_month.contains("medium"));

    let changed_resend = completed_report(
        base,
        "FIRST_GEOCODE_SENTINEL",
        "FIRST_CITY_SENTINEL",
        json!([
            {"name":"First POI","category":"restaurant","distance_m":10},
            {"name":"Must Not Be Added","category":"store","distance_m":30}
        ]),
    );
    assert_eq!(
        request_json(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            batch(changed_resend),
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        current_month_text(&pool, fixture.user_id).await,
        first_month
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.location_report_poi WHERE user_id=$1",
        )
        .bind(fixture.user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1,
        "a changed resend cannot append POI evidence"
    );

    write_places(&app, &fixture.saver.token, places_document(200), 1).await;
    let second = completed_report(
        base + Duration::hours(2),
        "SECOND_GEOCODE_SENTINEL",
        "SECOND_CITY_SENTINEL",
        json!([]),
    );
    assert_eq!(
        request_json(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            batch(second),
        )
        .await
        .status,
        StatusCode::OK
    );
    let edited_month = current_month_text(&pool, fixture.user_id).await;
    assert!(edited_month.contains("First POI"));
    assert!(edited_month.contains("Home"));
    assert!(edited_month.contains("high"));

    write_places(
        &app,
        &fixture.saver.token,
        "---\nkind: location-places\n---\nnot a table\n".to_owned(),
        2,
    )
    .await;
    let private_report = completed_report(
        base + Duration::hours(4),
        "PRIVATE_LABEL_SENTINEL",
        "PRIVATE_CITY_SENTINEL",
        json!([]),
    );
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let malformed_places_response = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        batch(private_report),
    )
    .await;
    drop(_guard);
    assert_eq!(malformed_places_response.status, StatusCode::OK);
    let captured = logs.text();
    assert_eq!(
        captured.matches("location places input degraded").count(),
        1
    );
    for secret in [
        "47.0009",
        "-122",
        "PRIVATE_LABEL_SENTINEL",
        "PRIVATE_CITY_SENTINEL",
    ] {
        assert!(!captured.contains(secret), "location log leaked {secret}");
    }
    let final_month = current_month_text(&pool, fixture.user_id).await;
    assert!(final_month.contains("PRIVATE_LABEL_SENTINEL"));
    assert!(final_month.contains("low"));

    write_places(&app, &fixture.saver.token, places_document(200), 3).await;
    let rederive_request = json!({
        "from": (base - Duration::hours(1)).to_rfc3339(),
        "to": Utc::now().to_rfc3339()
    });
    let rederived = request_json(
        &app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.saver.token,
        rederive_request.clone(),
    )
    .await;
    assert_eq!(rederived.status, StatusCode::OK);
    assert_eq!(rederived.body["reports_replayed"], 3);
    assert_eq!(rederived.body["rows_written"], 3);
    let rederived_month = current_month_text(&pool, fixture.user_id).await;
    assert_ne!(rederived_month, final_month);
    assert_eq!(rederived_month.matches("| Home | home |").count(), 3);
    assert_eq!(rederived_month.matches("| high |").count(), 3);

    let repeated_rederive = request_json(
        &app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.saver.token,
        rederive_request,
    )
    .await;
    assert_eq!(repeated_rederive.status, StatusCode::OK);
    assert_eq!(repeated_rederive.body["rows_written"], 0);
    assert_eq!(
        current_month_text(&pool, fixture.user_id).await,
        rederived_month
    );

    let presence = request_bytes(
        &app,
        Method::GET,
        "/v1/location/presence",
        &fixture.reader.token,
        None,
    )
    .await;
    assert_eq!(presence.status, StatusCode::OK);
    assert_eq!(presence.body["city"], "PRIVATE_CITY_SENTINEL");

    let invalid_rederive = request_json(
        &app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.saver.token,
        json!({
            "from": (Utc::now() - Duration::days(31)).to_rfc3339(),
            "to": Utc::now().to_rfc3339()
        }),
    )
    .await;
    assert_error(
        &invalid_rederive,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    assert_eq!(
        request_bytes(
            &app,
            Method::DELETE,
            "/v1/location/live",
            &fixture.device.token,
            None,
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.location_reports WHERE user_id=$1"
        )
        .bind(fixture.user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM brunn.location_presence WHERE user_id=$1",
        )
        .bind(fixture.user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        current_month_text(&pool, fixture.user_id).await,
        rederived_month
    );
}

// ---------------------------------------------------------------------------
// Location v1.1 §4 replay-day additions on the real router (gate 1), followed
// by a rederive of the same day (gate 2 shape) and a second no-op rederive.
// The day is two days ago so the rederive window stays inside retention.
// ---------------------------------------------------------------------------

fn field_day() -> chrono::NaiveDate {
    (Utc::now() - Duration::days(2))
        .with_timezone(&FixedOffset::west_opt(7 * 60 * 60).unwrap())
        .date_naive()
}

fn field_at(day: chrono::NaiveDate, clock: &str) -> String {
    format!("{day}T{clock}:00-07:00")
}

fn field_ping(
    day: chrono::NaiveDate,
    clock: &str,
    lat: f64,
    lon: f64,
    accuracy_m: f64,
    city: &str,
) -> Value {
    batch(json!({
        "type": "ping",
        "at": field_at(day, clock),
        "lat": lat,
        "lon": lon,
        "accuracy_m": accuracy_m,
        "geocode": {"city": city, "region": "WA", "country": "US"}
    }))
}

#[allow(clippy::too_many_arguments)]
fn field_visit(
    day: chrono::NaiveDate,
    clock: &str,
    arrived: &str,
    departed: Option<&str>,
    lat: f64,
    lon: f64,
    city: &str,
    name: &str,
    category: &str,
) -> Value {
    let mut report = json!({
        "type": if departed.is_some() { "visit_departure" } else { "visit_arrival" },
        "at": field_at(day, clock),
        "lat": lat,
        "lon": lon,
        "accuracy_m": 20,
        "arrived_at": field_at(day, arrived),
        "geocode": {"city": city, "region": "WA", "country": "US", "name": name},
        "poi": [{"name": name, "category": category, "distance_m": 12}]
    });
    if let Some(departed) = departed {
        report["departed_at"] = json!(field_at(day, departed));
    }
    batch(report)
}

/// The §4 day in delivery order, one report per batch, exactly as the phone
/// flushes on every event.
fn field_day_batches(day: chrono::NaiveDate) -> Vec<Value> {
    let d = day;
    vec![
        field_ping(d, "06:00", 47.6205, -122.2070, 20.0, "Bellevue"),
        field_ping(d, "07:40", 47.2043, -121.9915, 40.0, "Enumclaw"),
        field_ping(d, "08:30", 46.9350, -121.4740, 30.0, "Enumclaw"),
        field_ping(d, "10:15", 46.9665, -121.4740, 30.0, "Enumclaw"),
        field_ping(d, "11:00", 46.9500, -121.4740, 2_000.0, "Greenwater"),
        field_visit(
            d,
            "12:30",
            "12:00",
            Some("12:25"),
            46.9377,
            -121.4740,
            "Enumclaw",
            "Summit House",
            "restaurant",
        ),
        field_ping(d, "13:30", 47.0070, -121.4740, 40.0, "Enumclaw"),
        field_visit(
            d,
            "14:00",
            "14:00",
            None,
            47.3210,
            -122.1470,
            "Kent",
            "Pacific Raceways",
            "racetrack",
        ),
        field_ping(d, "14:20", 47.3264, -122.1470, 30.0, "Kent"),
        field_ping(d, "14:40", 47.3273, -122.1470, 30.0, "Kent"),
        field_visit(
            d,
            "16:05",
            "14:00",
            Some("16:00"),
            47.3210,
            -122.1470,
            "Kent",
            "Pacific Raceways",
            "racetrack",
        ),
        field_ping(d, "16:30", 47.1954, -120.9391, 600.0, "Cle Elum"),
        field_ping(d, "16:50", 46.9965, -120.5478, 2_000.0, "Ellensburg"),
        field_ping(d, "17:00", 46.9454, -119.9873, 5_000.0, "Vantage"),
        field_ping(d, "17:10", 46.9840, -120.4180, 2_000.0, "Kittitas"),
        field_ping(d, "17:20", 46.9965, -120.5478, 2_000.0, "Ellensburg"),
        field_ping(d, "17:30", 46.9840, -120.4180, 2_000.0, "Kittitas"),
        field_ping(d, "18:00", 47.6062, -122.3321, 20.0, "Seattle"),
        field_visit(
            d,
            "18:06",
            "18:04",
            None,
            47.6107,
            -122.3321,
            "Seattle",
            "Analog Coffee",
            "cafe",
        ),
        field_ping(d, "19:00", 47.6205, -122.2070, 20.0, "Bellevue"),
        field_visit(
            d,
            "19:30",
            "19:10",
            Some("19:28"),
            47.6170,
            -122.1980,
            "Bellevue",
            "Bellevue Gym",
            "fitness",
        ),
        field_ping(d, "20:00", 47.6205, -122.2070, 20.0, "Bellevue"),
        field_visit(
            d,
            "20:05",
            "17:45",
            Some("17:55"),
            47.6100,
            -122.2000,
            "Bellevue",
            "Bakery Nouveau",
            "bakery",
        ),
        field_visit(
            d,
            "20:30",
            "19:00",
            Some("19:10"),
            47.6205,
            -122.2070,
            "Bellevue",
            "Home",
            "home",
        ),
    ]
}

fn field_places_document() -> String {
    "---\nkind: location-places\n---\n\
     | Label | Kind | Lat | Lon | Radius m |\n\
     | --- | --- | --- | --- | --- |\n\
     | Home | home | 47.6205 | -122.2070 | 150 |\n\
     | Crystal Mountain | resort | 46.9350 | -121.4740 | 4000 |\n\
     | Office | work | 47.6062 | -122.3321 | 200 |\n"
        .to_owned()
}

/// `live` is the month as ingest writes it batch by batch: the four-minute
/// office drive-through survives live because the visit it closes was
/// opened in an earlier batch and its ping origin is not stored. Rederive
/// replays the day in one fold and applies R5.
fn field_expected_month(day: chrono::NaiveDate, pings_enabled: bool, live: bool) -> String {
    let month = day.format("%Y-%m");
    let d = day;
    let mut rows = if pings_enabled {
        vec![
            format!(
                "| {d}T06:00-07:00 | {d}T07:40-07:00 | 1h40m | Home | home | Bellevue, WA, US | high | 47.6205,-122.2070 |"
            ),
            format!(
                "| {d}T07:40-07:00 | — | — | passed through | transit | Enumclaw, WA, US | low | 47.2043,-121.9915 |"
            ),
            format!(
                "| {d}T08:30-07:00 | {d}T13:30-07:00 | 5h00m | Crystal Mountain | resort | Enumclaw, WA, US | high | 46.9350,-121.4740 |"
            ),
            format!(
                "| {d}T14:00-07:00 | {d}T16:00-07:00 | 2h00m | Pacific Raceways | racetrack | Kent, WA, US | medium | 47.3210,-122.1470 |"
            ),
            format!(
                "| {d}T16:30-07:00 | — | — | passed through | transit | Cle Elum, WA, US | low | 47.1954,-120.9391 |"
            ),
            format!(
                "| {d}T17:45-07:00 | {d}T17:55-07:00 | 10m | Bakery Nouveau | bakery | Bellevue, WA, US | medium | 47.6100,-122.2000 |"
            ),
            format!(
                "| {d}T18:04-07:00 | {d}T19:00-07:00 | 56m | Analog Coffee | cafe | Seattle, WA, US | medium | 47.6107,-122.3321 |"
            ),
            format!(
                "| {d}T19:00-07:00 | {d}T19:10-07:00 | 10m | Home | home | Bellevue, WA, US | high | 47.6205,-122.2070 |"
            ),
            format!(
                "| {d}T19:10-07:00 | {d}T19:28-07:00 | 18m | Bellevue Gym | fitness | Bellevue, WA, US | medium | 47.6170,-122.1980 |"
            ),
        ]
    } else {
        vec![
            format!(
                "| {d}T12:00-07:00 | {d}T12:25-07:00 | 25m | Crystal Mountain | resort | Enumclaw, WA, US | high | 46.9377,-121.4740 |"
            ),
            format!(
                "| {d}T14:00-07:00 | {d}T16:00-07:00 | 2h00m | Pacific Raceways | racetrack | Kent, WA, US | medium | 47.3210,-122.1470 |"
            ),
            format!(
                "| {d}T17:45-07:00 | {d}T17:55-07:00 | 10m | Bakery Nouveau | bakery | Bellevue, WA, US | medium | 47.6100,-122.2000 |"
            ),
            format!(
                "| {d}T18:04-07:00 | {d}T19:10-07:00 | 1h06m | Analog Coffee | cafe | Seattle, WA, US | medium | 47.6107,-122.3321 |"
            ),
            format!(
                "| {d}T19:00-07:00 | {d}T19:10-07:00 | 10m | Home | home | Bellevue, WA, US | high | 47.6205,-122.2070 |"
            ),
            format!(
                "| {d}T19:10-07:00 | {d}T19:28-07:00 | 18m | Bellevue Gym | fitness | Bellevue, WA, US | medium | 47.6170,-122.1980 |"
            ),
        ]
    };
    if pings_enabled && live {
        rows.insert(
            6,
            format!("| {d}T18:00-07:00 | {d}T18:04-07:00 | 4m | Office | work | Seattle, WA, US | high | 47.6062,-122.3321 |"),
        );
    }
    format!(
        "---\nkind: location-visits\nmonth: {month}\n---\n\
         | Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |\n{}\n",
        rows.join("\n")
    )
}

async fn field_snapshot(pool: &PgPool, user_id: Uuid) -> (String, Value, i64, i64) {
    let month = current_month_text(pool, user_id).await;
    let presence = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT COALESCE(
          (SELECT to_jsonb(presence_row) - 'user_id'
           FROM brunn.location_presence AS presence_row WHERE user_id=$1),
          'null'::jsonb)
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("snapshot presence");
    let (reports, versions) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM brunn.location_reports WHERE user_id=$1),
               (SELECT count(*) FROM brunn.entry_versions AS version
                JOIN brunn.entries AS entry ON entry.id=version.entry_id
                WHERE entry.user_id=$1 AND entry.path LIKE 'Location/Visits/%.md')
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("snapshot counts");
    (month, presence, reports, versions)
}

async fn exercise_field_day_gate(
    pool: &PgPool,
    app: &Router,
    fixture: &LocationFixture,
    pings_enabled: bool,
) {
    let day = field_day();
    write_places(app, &fixture.saver.token, field_places_document(), 0).await;
    let batches = field_day_batches(day);
    for (index, body) in batches.iter().enumerate() {
        let response = request_json(
            app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            body.clone(),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "field-day step {index}");
        assert!(
            response.body["ignored"]
                .as_object()
                .is_some_and(|ignored| !ignored.contains_key("late")),
            "no field-day batch is late at step {index}: {}",
            response.body
        );
    }
    let live = field_snapshot(pool, fixture.user_id).await;
    assert_eq!(live.0, field_expected_month(day, pings_enabled, true));
    assert_eq!(
        live.2,
        if pings_enabled { 24 } else { 7 },
        "raw rows stored"
    );

    // Re-sending the whole day is a byte and database no-op.
    for body in &batches {
        let response = request_json(
            app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            body.clone(),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "re-send field-day batch");
    }
    assert_eq!(field_snapshot(pool, fixture.user_id).await, live);

    // Rederive the day: one fold applies every rule, including R5.
    let window = json!({
        "from": field_at(day, "00:00"),
        "to": field_at(day, "23:59")
    });
    let rederived = request_json(
        app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.saver.token,
        window.clone(),
    )
    .await;
    assert_eq!(rederived.status, StatusCode::OK, "{}", rederived.body);
    assert_eq!(
        rederived.body["reports_replayed"],
        if pings_enabled { 24 } else { 7 }
    );
    assert_eq!(
        rederived.body["rows_written"],
        if pings_enabled { 9 } else { 0 }
    );
    let after = field_snapshot(pool, fixture.user_id).await;
    assert_eq!(after.0, field_expected_month(day, pings_enabled, false));
    assert_eq!(
        after.1, live.1,
        "presence is already consistent with the day"
    );

    let repeated = request_json(
        app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.saver.token,
        window,
    )
    .await;
    assert_eq!(repeated.status, StatusCode::OK);
    assert_eq!(repeated.body["rows_written"], 0);
    assert_eq!(field_snapshot(pool, fixture.user_id).await, after);

    let presence = request_bytes(
        app,
        Method::GET,
        "/v1/location/presence",
        &fixture.reader.token,
        None,
    )
    .await;
    assert_eq!(presence.status, StatusCode::OK);
    assert_eq!(presence.body["status"], "stale");
    assert_eq!(presence.body["city"], "Bellevue");
    assert_eq!(
        presence
            .body
            .pointer("/place/label")
            .and_then(Value::as_str),
        pings_enabled.then_some("Home")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn field_day_gate_is_exact_on_real_router_with_pings_on_and_off() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let pings_on = seed_fixture(&pool).await;
    let pings_off = seed_fixture(&pool).await;
    let on_app = router(state.clone());
    let mut off_state = state;
    off_state.config.location_pings_enabled = false;
    let off_app = router(off_state);

    exercise_field_day_gate(&pool, &on_app, &pings_on, true).await;
    exercise_field_day_gate(&pool, &off_app, &pings_off, false).await;
}

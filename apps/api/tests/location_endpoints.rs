use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Duration, FixedOffset, Timelike, Utc};
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
                place_label: Some("Din Tai Fung"),
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
                place_label: Some("Din Tai Fung"),
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 0,
                ignored: Some("pings_off"),
                presence_status: Some("stale"),
                place_label: Some("Din Tai Fung"),
                city: Some("Bellevue"),
            },
            ReplayStepExpectation {
                accepted: 1,
                ignored: None,
                presence_status: Some("stale"),
                place_label: Some("Din Tai Fung"),
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
async fn general_current_position_survives_storage_coarse_contact_and_delayed_visits() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    let app = router(state);
    write_places(&app, &fixture.saver.token, replay_places_document(), 0).await;
    let base = Utc::now() - Duration::minutes(30);
    let home =
        json!({"type":"ping","at":base.to_rfc3339(),"lat":47.6205,"lon":-122.2070,"accuracy_m":20});
    let arrived = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        batch(home),
    )
    .await;
    assert_eq!(arrived.status, StatusCode::OK);
    // Force sub-microsecond wire precision even on platforms whose clock only
    // supplies microseconds. This used to change the snapshot during rederive.
    let moved_at = (Utc::now() - Duration::minutes(3))
        .with_nanosecond(907_921_556)
        .unwrap();
    let moved = json!({"type":"ping","at":moved_at.to_rfc3339(),"lat":47.6220,"lon":-122.2070,"accuracy_m":33.9,
        "geocode":{"name":"123 Example Street","city":"Bellevue","region":"WA","country":"US"}});
    let moved_response = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        batch(moved),
    )
    .await;
    assert_eq!(moved_response.status, StatusCode::OK);
    assert_eq!(
        moved_response.body["presence"]["place"]["label"],
        "123 Example Street"
    );
    assert_eq!(moved_response.body["presence"]["visit"]["label"], "Home");
    assert_eq!(moved_response.body["presence"]["at_home"], false);

    let delayed = json!({"type":"visit_arrival","at":(Utc::now()-Duration::minutes(1)).to_rfc3339(),
        "arrived_at":base.to_rfc3339(),"lat":47.6205,"lon":-122.2070,"accuracy_m":15});
    assert_eq!(
        request_json(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            batch(delayed)
        )
        .await
        .status,
        StatusCode::OK
    );
    let coarse = json!({"type":"ping","at":Utc::now().to_rfc3339(),"lat":47.6000,"lon":-122.2000,"accuracy_m":5485});
    assert_eq!(
        request_json(
            &app,
            Method::POST,
            "/v1/location/reports",
            &fixture.device.token,
            batch(coarse)
        )
        .await
        .status,
        StatusCode::OK
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
    assert_eq!(presence.body["position"]["lat"], 47.6220);
    assert_eq!(presence.body["position"]["accuracy_m"], f64::from(33.9_f32));
    assert_eq!(
        DateTime::parse_from_rfc3339(presence.body["position"]["observed_at"].as_str().unwrap())
            .unwrap()
            .nanosecond(),
        907_921_000
    );
    assert_eq!(presence.body["place"]["label"], "123 Example Street");
    assert_eq!(presence.body["visit"]["label"], "Home");
    assert_ne!(presence.body["last_seen"], presence.body["last_contact"]);
    assert_eq!(presence.body["at_home"], false);
    assert!(presence.body["position"]["age_seconds"].as_i64().unwrap() >= 120);
    let stored_position = sqlx::query_scalar::<_, Value>(
        "SELECT current_position FROM brunn.location_presence WHERE user_id=$1",
    )
    .bind(fixture.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let rederived = request_json(
        &app,
        Method::POST,
        "/v1/location/rederive",
        &fixture.saver.token,
        json!({"from":(base-Duration::minutes(1)).to_rfc3339(),"to":Utc::now().to_rfc3339()}),
    )
    .await;
    assert_eq!(rederived.status, StatusCode::OK);
    let after = request_bytes(
        &app,
        Method::GET,
        "/v1/location/presence",
        &fixture.reader.token,
        None,
    )
    .await;
    assert_eq!(
        after.body["position"]["observed_at"],
        presence.body["position"]["observed_at"]
    );
    assert_eq!(after.body["place"], presence.body["place"]);
    assert_eq!(after.body["visit"], presence.body["visit"]);
    let rederived_position = sqlx::query_scalar::<_, Value>(
        "SELECT current_position FROM brunn.location_presence WHERE user_id=$1",
    )
    .bind(fixture.user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rederived_position, stored_position,
        "the complete persisted position must replay exactly"
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
    assert_eq!(first_response.body["presence"]["status"], "stale");
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
        Some(if pings_enabled {
            "Home"
        } else {
            "Bellevue Gym"
        })
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

fn evidence_uri(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("from", &from.to_rfc3339())
        .append_pair("to", &to.to_rfc3339())
        .append_pair("timezone", "UTC")
        .finish();
    format!("/v1/location/evidence?{query}")
}

struct SummaryFixture {
    pool: PgPool,
    state: AppState,
    app: Router,
    owner: LocationFixture,
    auth: brunn::auth::AuthContext,
    query: brunn::location::evidence::EvidenceQuery,
    packet: Value,
    canonical: brunn::dreamer_review::Source,
    raw: brunn::location::summary::RawCitation,
    content: String,
}

async fn summary_fixture() -> Option<SummaryFixture> {
    let (pool, state) = connect_test_state().await?;
    let owner = seed_fixture(&pool).await;
    let app = router(state.clone());
    let auth = brunn::auth::authenticate(&state, &owner.saver.token)
        .await
        .unwrap();
    let from = (Utc::now() - Duration::days(2))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let query = brunn::location::evidence::EvidenceQuery {
        from: from.fixed_offset(),
        to: (from + Duration::days(1)).fixed_offset(),
        timezone: "UTC".into(),
    };
    let row = format!(
        "| {} | {} | 1h | A bounded stop | visit | Bellevue | medium | 47.0000,-122.0000 |\n",
        (from + Duration::hours(12)).format("%Y-%m-%dT%H:%M%:z"),
        (from + Duration::hours(13)).format("%Y-%m-%dT%H:%M%:z")
    );
    let content = format!(
        "---\nkind: location-visits\nmonth: {}\n---\n| Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n{row}",
        from.format("%Y-%m")
    );
    let path = format!("Location/Visits/{}.md", from.format("%Y-%m"));
    let written=request_json(&app,Method::POST,"/v1/workspace/write",&owner.saver.token,json!({"path":path,"content":content,"expected_version":0,"metadata":{"kind":"location-visits"}})).await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);
    for at in [
        from - Duration::hours(2),
        from - Duration::hours(1),
        from + Duration::hours(12),
    ] {
        sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m,name) VALUES($1,$2,'ping',0,47,-122,5,'Approximate address')").bind(owner.user_id).bind(at).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO brunn.location_report_poi(user_id,at,type,rank,name,category,distance_m) VALUES($1,$2,'ping',1,'Nearby candidate','cafe',20)").bind(owner.user_id).bind(from+Duration::hours(12)).execute(&pool).await.unwrap();
    let packet = brunn::location::evidence::read_evidence(&state, &auth, &query)
        .await
        .unwrap();
    assert_eq!(packet["fingerprint_complete"], true, "{packet}");
    let doc = &packet["canonical_months"][0];
    let selector = &doc["selectors"][0];
    let canonical = brunn::dreamer_review::Source {
        entry_ref: doc["ref"].as_str().unwrap().into(),
        version: doc["version"].as_i64().unwrap(),
        path: doc["path"].as_str().unwrap().into(),
        start_line: selector["start_line"].as_u64().unwrap() as usize,
        end_line: selector["end_line"].as_u64().unwrap() as usize,
        excerpt: selector["text"].as_str().unwrap().into(),
    };
    let raw = brunn::location::summary::RawCitation {
        natural_key: packet["reports"][0]["natural_key"].clone(),
        fields: vec![
            "lat".into(),
            "accuracy_m".into(),
            "first_received_at".into(),
            "departed_at".into(),
            "poi.0.name".into(),
        ],
    };
    Some(SummaryFixture {
        pool,
        state,
        app,
        owner,
        auth,
        query,
        packet,
        canonical,
        raw,
        content,
    })
}

async fn validate_location_summary(
    f: &SummaryFixture,
    query: &brunn::location::evidence::EvidenceQuery,
    fingerprint: &str,
    canonical: &[brunn::dreamer_review::Source],
    raw: &[brunn::location::summary::RawCitation],
) -> brunn::error::ApiResult<Value> {
    let mut tx = f.state.begin_write(&f.auth).await.unwrap();
    let result = brunn::location::summary::validate_candidate_in_tx(
        &mut tx,
        &f.auth,
        query,
        fingerprint,
        canonical,
        raw,
    )
    .await;
    tx.rollback().await.unwrap();
    result
}

#[tokio::test]
async fn location_summary_publication_rechecks_raw_and_selected_canonical_rows() {
    let Some(f) = summary_fixture().await else {
        return;
    };
    let fingerprint = f.packet["evidence_fingerprint"].as_str().unwrap();
    let valid = validate_location_summary(
        &f,
        &f.query,
        fingerprint,
        std::slice::from_ref(&f.canonical),
        std::slice::from_ref(&f.raw),
    )
    .await
    .unwrap();
    assert_eq!(valid["sources_validated"], true);
    assert_eq!(valid["fingerprint"], fingerprint);
    assert!(
        valid.get("reports").is_none(),
        "validator must not create a copied raw archive"
    );
    let next = f.query.to.to_utc() + Duration::hours(12);
    let appended = format!(
        "{}| {} | {} | 1h | Unrelated later stop | visit | Seattle | medium | 47.1000,-122.1000 |\n",
        f.content,
        next.format("%Y-%m-%dT%H:%M%:z"),
        (next + Duration::hours(1)).format("%Y-%m-%dT%H:%M%:z")
    );
    let written=request_json(&f.app,Method::POST,"/v1/workspace/write",&f.owner.saver.token,json!({"path":f.canonical.path,"content":appended,"expected_version":1,"metadata":{"kind":"location-visits"}})).await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);
    validate_location_summary(
        &f,
        &f.query,
        fingerprint,
        std::slice::from_ref(&f.canonical),
        std::slice::from_ref(&f.raw),
    )
    .await
    .unwrap();
    let mut outside = f.canonical.clone();
    outside.version = 2;
    outside.start_line = appended.lines().count();
    outside.end_line = outside.start_line;
    outside.excerpt = appended.lines().last().unwrap().to_owned();
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[outside], &[])
            .await
            .is_err(),
        "an exact next-day canonical row is not evidence for the requested day"
    );
    let edited = appended.replace("A bounded stop", "A corrected bounded stop");
    let written=request_json(&f.app,Method::POST,"/v1/workspace/write",&f.owner.saver.token,json!({"path":f.canonical.path,"content":edited,"expected_version":2,"metadata":{"kind":"location-visits"}})).await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.body);
    assert!(
        validate_location_summary(
            &f,
            &f.query,
            fingerprint,
            std::slice::from_ref(&f.canonical),
            &[]
        )
        .await
        .is_err()
    );
    let edited_packet = brunn::location::evidence::read_evidence(&f.state, &f.auth, &f.query)
        .await
        .unwrap();
    assert!(
        validate_location_summary(
            &f,
            &f.query,
            edited_packet["evidence_fingerprint"].as_str().unwrap(),
            std::slice::from_ref(&f.canonical),
            &[]
        )
        .await
        .is_err(),
        "fresh packet cannot validate a historical excerpt that changed"
    );
    let generation: i64 =
        sqlx::query_scalar("SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1")
            .bind(f.owner.user_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m,arrived_at,departed_at) VALUES($1,$2,'visit_departure',0,47.002,-122.002,8,$3,$4)").bind(f.owner.user_id).bind(f.query.to.to_utc()+Duration::hours(2)).bind(f.query.from.to_utc()+Duration::hours(14)).bind(f.query.from.to_utc()+Duration::hours(15)).execute(&f.pool).await.unwrap();
    let unchanged: i64 =
        sqlx::query_scalar("SELECT max(generation) FROM brunn.workspace_changes WHERE user_id=$1")
            .bind(f.owner.user_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(generation, unchanged);
    assert!(
        validate_location_summary(
            &f,
            &f.query,
            edited_packet["evidence_fingerprint"].as_str().unwrap(),
            &[],
            std::slice::from_ref(&f.raw)
        )
        .await
        .is_err(),
        "late raw-only callback must invalidate despite unchanged workspace generation"
    );
}

#[tokio::test]
async fn location_summary_citations_reject_unavailable_scope_fields_and_partial_packets() {
    let Some(f) = summary_fixture().await else {
        return;
    };
    let fingerprint = f.packet["evidence_fingerprint"].as_str().unwrap();
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[], &[])
            .await
            .is_err()
    );
    let mut forged = f.canonical.clone();
    forged.excerpt = "Invented canonical evidence".into();
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[forged], &[])
            .await
            .is_err()
    );
    let mut forbidden = f.raw.clone();
    forbidden.fields = vec!["user_id".into()];
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[], &[forbidden])
            .await
            .is_err()
    );
    let mut missing = f.raw.clone();
    missing.fields = vec!["poi.99.name".into()];
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[], &[missing])
            .await
            .is_err()
    );
    let outside:Value=sqlx::query_scalar("SELECT jsonb_build_object('at',at,'type',type) FROM brunn.location_reports WHERE user_id=$1 ORDER BY at LIMIT 1").bind(f.owner.user_id).fetch_one(&f.pool).await.unwrap();
    let outside = brunn::location::summary::RawCitation {
        natural_key: outside,
        fields: vec!["at".into()],
    };
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[], &[outside])
            .await
            .is_err(),
        "outside non-boundary raw record is not packet evidence"
    );
    let old = brunn::location::evidence::EvidenceQuery {
        from: (f.query.from - Duration::days(40)),
        to: (f.query.to - Duration::days(40)),
        timezone: "UTC".into(),
    };
    assert!(
        validate_location_summary(&f, &old, fingerprint, &[], std::slice::from_ref(&f.raw))
            .await
            .is_err()
    );
    let mut pinned = f.state.rw_pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *pinned)
        .await
        .unwrap();
    brunn::db::set_context(&mut pinned, &f.auth).await.unwrap();
    assert!(
        brunn::location::summary::validate_candidate_in_tx(
            &mut pinned,
            &f.auth,
            &f.query,
            fingerprint,
            &[],
            std::slice::from_ref(&f.raw)
        )
        .await
        .is_err(),
        "an older pinned transaction may not validate publication after waiting for owner locks"
    );
    pinned.rollback().await.unwrap();
    let foreign = seed_fixture(&f.pool).await;
    let foreign_auth = brunn::auth::authenticate(&f.state, &foreign.saver.token)
        .await
        .unwrap();
    let mut foreign_tx = f.state.begin_write(&foreign_auth).await.unwrap();
    assert!(
        brunn::location::summary::validate_candidate_in_tx(
            &mut foreign_tx,
            &foreign_auth,
            &f.query,
            fingerprint,
            std::slice::from_ref(&f.canonical),
            std::slice::from_ref(&f.raw)
        )
        .await
        .is_err()
    );
    foreign_tx.rollback().await.unwrap();
    let reader = brunn::auth::authenticate(&f.state, &f.owner.reader.token)
        .await
        .unwrap();
    let mut read = f.state.begin_read(&reader).await.unwrap();
    assert!(
        brunn::location::summary::validate_candidate_in_tx(
            &mut read,
            &reader,
            &f.query,
            fingerprint,
            &[],
            std::slice::from_ref(&f.raw)
        )
        .await
        .is_err()
    );
    read.rollback().await.unwrap();
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m) SELECT $1,$2::timestamptz+n*interval '1 second','ping',0,47,-122,5 FROM generate_series(1,2001)n").bind(f.owner.user_id).bind(f.query.from.to_utc()).execute(&f.pool).await.unwrap();
    let partial = brunn::location::evidence::read_evidence(&f.state, &f.auth, &f.query)
        .await
        .unwrap();
    assert_eq!(partial["fingerprint_complete"], false);
    assert!(
        validate_location_summary(&f, &f.query, fingerprint, &[], std::slice::from_ref(&f.raw))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn location_summary_validation_holds_owner_evidence_fences_until_publication_ends() {
    let Some(f) = summary_fixture().await else {
        return;
    };
    let fingerprint = f.packet["evidence_fingerprint"].as_str().unwrap();
    let mut publication = f.state.begin_write(&f.auth).await.unwrap();
    brunn::location::summary::validate_candidate_in_tx(
        &mut publication,
        &f.auth,
        &f.query,
        fingerprint,
        std::slice::from_ref(&f.canonical),
        std::slice::from_ref(&f.raw),
    )
    .await
    .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let state = f.state.clone();
    let auth = brunn::auth::authenticate(&f.state, &f.owner.device.token)
        .await
        .unwrap();
    let at = f.query.from.to_utc() + Duration::hours(11);
    let mut writer = tokio::spawn(async move {
        started.send(()).unwrap();
        let mut tx = state.begin_write(&auth).await.unwrap();
        brunn::location::store::lock_location_user(&mut tx, auth.user_id.0)
            .await
            .unwrap();
        sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m) VALUES($1,$2,'ping',0,47.001,-122.001,5)").bind(auth.user_id.0).bind(at).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    });
    ready.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut writer)
            .await
            .is_err()
    );
    publication.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    assert!(
        validate_location_summary(
            &f,
            &f.query,
            fingerprint,
            std::slice::from_ref(&f.canonical),
            std::slice::from_ref(&f.raw)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn location_summary_places_citations_require_the_exact_current_definition_version() {
    let Some(f) = summary_fixture().await else {
        return;
    };
    write_places(&f.app, &f.owner.saver.token, places_document(100), 0).await;
    let packet = brunn::location::evidence::read_evidence(&f.state, &f.auth, &f.query)
        .await
        .unwrap();
    let place = &packet["places"];
    let text = place["text"].as_str().unwrap();
    let source = brunn::dreamer_review::Source {
        entry_ref: place["ref"].as_str().unwrap().into(),
        version: place["version"].as_i64().unwrap(),
        path: place["path"].as_str().unwrap().into(),
        start_line: 1,
        end_line: text.lines().count(),
        excerpt: text.lines().collect::<Vec<_>>().join("\n"),
    };
    validate_location_summary(
        &f,
        &f.query,
        packet["evidence_fingerprint"].as_str().unwrap(),
        std::slice::from_ref(&source),
        &[],
    )
    .await
    .unwrap();
    write_places(&f.app, &f.owner.saver.token, places_document(150), 1).await;
    let changed = brunn::location::evidence::read_evidence(&f.state, &f.auth, &f.query)
        .await
        .unwrap();
    assert!(
        validate_location_summary(
            &f,
            &f.query,
            packet["evidence_fingerprint"].as_str().unwrap(),
            std::slice::from_ref(&source),
            &[]
        )
        .await
        .is_err()
    );
    assert!(
        validate_location_summary(
            &f,
            &f.query,
            changed["evidence_fingerprint"].as_str().unwrap(),
            std::slice::from_ref(&source),
            &[]
        )
        .await
        .is_err(),
        "even a refreshed packet cannot validate the superseded Places definition version"
    );
}

async fn preview_location_citations(f: &SummaryFixture, scope: &Value) -> Vec<Value> {
    let mut tx = f.state.rw_pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .unwrap();
    brunn::db::set_context(&mut tx, &f.auth).await.unwrap();
    let rows = brunn::location::summary::citation_previews_in_tx(
        &mut tx,
        &f.auth,
        scope,
        std::slice::from_ref(&f.raw),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rows
}

#[tokio::test]
async fn location_raw_citation_previews_show_only_selected_current_values_from_matching_scope() {
    let Some(f) = summary_fixture().await else {
        return;
    };
    let scope = validate_location_summary(
        &f,
        &f.query,
        f.packet["evidence_fingerprint"].as_str().unwrap(),
        std::slice::from_ref(&f.canonical),
        std::slice::from_ref(&f.raw),
    )
    .await
    .unwrap();
    let rows = preview_location_citations(&f, &scope).await;
    assert_eq!(rows.len(), 1);
    let preview = &rows[0];
    assert!(
        preview["entry_ref"]
            .as_str()
            .unwrap()
            .starts_with("location-report:")
    );
    assert!(preview.get("path").is_none());
    assert!(preview.get("version").is_none());
    let text = preview["excerpt"].as_str().unwrap();
    assert!(text.contains("lat: 47"));
    assert!(text.contains("accuracy_m: 5"));
    assert!(text.contains("first_received_at: null"));
    assert!(text.contains("poi.0.name: \"Nearby candidate\""));
    assert!(text.contains("not confirmed venues"));
    assert!(
        !text.contains("Approximate address"),
        "unselected raw values are not previewed"
    );
    sqlx::query("UPDATE brunn.location_reports SET accuracy_m=45 WHERE user_id=$1 AND at=$2 AND type='ping'").bind(f.owner.user_id).bind(f.query.from.to_utc()+Duration::hours(12)).execute(&f.pool).await.unwrap();
    assert!(
        preview_location_citations(&f, &scope).await.is_empty(),
        "changed packet cannot preview new or cached raw values under the accepted old scope"
    );
    let packet = brunn::location::evidence::read_evidence(&f.state, &f.auth, &f.query)
        .await
        .unwrap();
    let fresh = validate_location_summary(
        &f,
        &f.query,
        packet["evidence_fingerprint"].as_str().unwrap(),
        std::slice::from_ref(&f.canonical),
        std::slice::from_ref(&f.raw),
    )
    .await
    .unwrap();
    let current = preview_location_citations(&f, &fresh).await;
    assert!(
        current[0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("accuracy_m: 45")
    );
    let mut expired = fresh.clone();
    expired["from"] = json!(f.query.from - Duration::days(40));
    expired["to"] = json!(f.query.to - Duration::days(40));
    assert!(preview_location_citations(&f, &expired).await.is_empty());
    let reader = brunn::auth::authenticate(&f.state, &f.owner.reader.token)
        .await
        .unwrap();
    let mut read = f.state.begin_read(&reader).await.unwrap();
    assert!(
        brunn::location::summary::citation_previews_in_tx(
            &mut read,
            &reader,
            &fresh,
            std::slice::from_ref(&f.raw)
        )
        .await
        .is_err()
    );
    read.rollback().await.unwrap();
}

#[tokio::test]
async fn historical_evidence_preserves_late_visits_receipts_snapshot_and_source_boundaries() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    let other = seed_fixture(&pool).await;
    let app = router(state.clone());
    write_places(&app, &fixture.saver.token, places_document(100), 0).await;
    let from = (Utc::now() - Duration::days(2))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let to = from + Duration::days(1);
    let uri = evidence_uri(from, to);
    let callback = to + Duration::hours(3);
    let late = json!({"type":"visit_departure","at":callback,"lat":47.02,"lon":-122.0,"accuracy_m":20,
        "arrived_at":from+Duration::hours(18),"departed_at":from+Duration::hours(19),
        "geocode":{"name":"An approximate street","city":"Bellevue"},
        "poi":[{"name":"Nearby candidate A","distance_m":15},{"name":"Nearby candidate B","distance_m":15}]});
    let reports = json!({"timezone":"UTC","reports":[
        {"type":"ping","at":from-Duration::minutes(1),"lat":47.0,"lon":-122.0,"accuracy_m":5},
        {"type":"ping","at":from+Duration::hours(12),"lat":47.0,"lon":-122.0,"accuracy_m":5},
        {"type":"ping","at":to+Duration::minutes(1),"lat":47.0,"lon":-122.0,"accuracy_m":5},late]});
    let first_receipt_start = Utc::now();
    let response = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        reports.clone(),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let receipt = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT first_received_at FROM brunn.location_reports WHERE user_id=$1 AND at=$2",
    )
    .bind(fixture.user_id)
    .bind(callback)
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert!(receipt >= first_receipt_start - Duration::milliseconds(1) && receipt <= Utc::now());
    let response = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        reports,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let duplicate_receipt = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT first_received_at FROM brunn.location_reports WHERE user_id=$1 AND at=$2",
    )
    .bind(fixture.user_id)
    .bind(callback)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(duplicate_receipt, Some(receipt));
    for token in [&fixture.reader.token, &fixture.device.token] {
        let denied = request_bytes(&app, Method::GET, &uri, token, None).await;
        assert_error(&denied, StatusCode::FORBIDDEN, "capability_denied");
    }
    let foreign = request_bytes(&app, Method::GET, &uri, &other.saver.token, None).await;
    assert_eq!(foreign.status, StatusCode::OK, "{}", foreign.body);
    assert!(foreign.body["reports"].as_array().unwrap().is_empty());
    assert!(foreign.body["places"].is_null());
    let response = request_bytes(&app, Method::GET, &uri, &fixture.saver.token, None).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(
        response.body["completeness"]["complete"], true,
        "{}",
        response.body
    );
    let rows = response.body["reports"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "one day ping and late overlapping visit");
    let visit = rows
        .iter()
        .find(|r| r["type"] == "visit_departure")
        .unwrap();
    assert_eq!(visit["origin"], "apple_visit_estimate");
    assert_eq!(visit["poi"].as_array().unwrap().len(), 2);
    assert_eq!(visit["poi"][0]["rank"], 1);
    assert_eq!(visit["poi"][1]["rank"], 2);
    assert!(
        DateTime::parse_from_rfc3339(visit["at"].as_str().unwrap())
            .unwrap()
            .to_utc()
            >= to
    );
    assert!(response.body["boundary_observations"]["before"].is_object());
    assert!(response.body["boundary_observations"]["after"].is_object());
    assert!(
        response.body["canonical_months"]
            .as_array()
            .unwrap()
            .iter()
            .any(|doc| !doc["selectors"].as_array().unwrap().is_empty())
    );
    assert_eq!(response.body["places"]["version"], 1);
    assert_eq!(
        response.body["sample_gaps"]["intervals"][0]["label"],
        "sample_gap"
    );
    let fingerprint = response.body["evidence_fingerprint"].clone();
    let next_day = completed_report(
        to + Duration::hours(12),
        "Another next-day area",
        "Seattle",
        json!([]),
    );
    let changed = request_json(
        &app,
        Method::POST,
        "/v1/location/reports",
        &fixture.device.token,
        batch(next_day),
    )
    .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    let after_unrelated = request_bytes(&app, Method::GET, &uri, &fixture.saver.token, None).await;
    assert_eq!(
        after_unrelated.body["evidence_fingerprint"], fingerprint,
        "unrelated later-day canonical append is not day invalidation"
    );

    // A backdated raw-only write changes no workspace generation. One pinned
    // snapshot remains coherent; a new snapshot observes the new evidence.
    let auth = brunn::auth::authenticate(&state, &fixture.saver.token)
        .await
        .unwrap();
    let query = brunn::location::evidence::EvidenceQuery {
        from: from.fixed_offset(),
        to: to.fixed_offset(),
        timezone: "UTC".into(),
    };
    let mut tx = state.rw_pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .unwrap();
    brunn::db::set_context(&mut tx, &auth).await.unwrap();
    let pinned = brunn::location::evidence::evidence_in_tx(&mut tx, &auth, &query)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m,name) VALUES($1,$2,'ping',0,47.001,-122.001,5,'Legacy unknown receipt')")
        .bind(fixture.user_id).bind(from+Duration::hours(14)).execute(&pool).await.unwrap();
    let still_pinned = brunn::location::evidence::evidence_in_tx(&mut tx, &auth, &query)
        .await
        .unwrap();
    assert_eq!(
        pinned["evidence_fingerprint"],
        still_pinned["evidence_fingerprint"]
    );
    tx.commit().await.unwrap();
    let fresh = brunn::location::evidence::read_evidence(&state, &auth, &query)
        .await
        .unwrap();
    assert_eq!(
        pinned["snapshot"]["workspace_generation"],
        fresh["snapshot"]["workspace_generation"]
    );
    assert_ne!(
        pinned["evidence_fingerprint"],
        fresh["evidence_fingerprint"]
    );
    assert!(
        fresh["reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|report| report["name"] == "Legacy unknown receipt"
                && report["first_received_at"].is_null())
    );
    let expired = request_bytes(
        &app,
        Method::GET,
        &evidence_uri(from - Duration::days(40), to - Duration::days(40)),
        &fixture.saver.token,
        None,
    )
    .await;
    assert_eq!(expired.status, StatusCode::OK);
    assert_eq!(expired.body["completeness"]["complete"], false);
    assert!(
        expired.body["completeness"]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_retention_expired"))
    );
}

#[tokio::test]
async fn historical_evidence_caps_defer_instead_of_claiming_complete_sources() {
    let Some((pool, state)) = connect_test_state().await else {
        return;
    };
    let fixture = seed_fixture(&pool).await;
    let app = router(state);
    let from = (Utc::now() - Duration::days(2))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    sqlx::query("INSERT INTO brunn.location_reports(user_id,at,type,offset_min,lat,lon,accuracy_m,name) SELECT $1,$2::timestamptz+n*interval '1 second','ping',0,47,-122,5,repeat('x',1000) FROM generate_series(1,2001) n")
        .bind(fixture.user_id).bind(from).execute(&pool).await.unwrap();
    let response = request_bytes(
        &app,
        Method::GET,
        &evidence_uri(from, from + Duration::days(1)),
        &fixture.saver.token,
        None,
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert_eq!(response.body["completeness"]["complete"], false);
    assert_eq!(response.body["fingerprint_complete"], false);
    assert!(response.body["evidence_fingerprint"].is_null());
    let reasons = response.body["completeness"]["reasons"].as_array().unwrap();
    assert!(reasons.contains(&json!("report_limit")));
    assert!(reasons.contains(&json!("packet_byte_limit")));
    assert!(response.body["reports"].as_array().unwrap().len() < 2000);
    assert!(serde_json::to_vec(&response.body).unwrap().len() <= 250_000);
}
